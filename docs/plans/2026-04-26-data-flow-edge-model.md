# Data Flow Edge Model Implementation Plan

**Goal:** Replace the undifferentiated `Edge::ReferencesTable` with `Edge::TableAccess` carrying read/write/lock semantics, split `Node::Procedure` into `Node::Procedure` + `Node::Function`, and implement a `TableAccessExtractor` that distinguishes write targets from read sources for every DML type.

**Architecture:** One edge per node pair with `AccessMode` bitflags (Read/Write/LockRead/Truncate) + `WriteKind` enum. Node split uses unified `RoutineId { kind: RoutineKind }` as the identity type. Lazy edge merge (post-pass dedup) in the builder. ogsql-parser's AST already provides distinct target/source fields for every DML type.

**Tech Stack:** Rust, ogsql-parser (Visitor trait + DML AST), petgraph, bitflags crate, serde (JSON + bincode).

**TDD:** Every task writes failing tests FIRST, then implements. `cargo test && cargo clippy -- -D warnings && cargo fmt -- --check` must pass before marking complete.

---

## Execution Context

**Project root:** `/Users/c2j/Projects/Desktop_Projects/CODE/cobweb`
**ogsql-parser source:** `/tmp/ogsql-parser-latest` (commit dc40706, with upstream fixes for issues #12 and #13)
**Cargo.toml ogsql-parser:** currently points to `git = "https://github.com/c2j/ogsql-parser"` — will need `cargo update` to pull latest

**Key files to understand before starting:**
- `src/graph/mod.rs` — Node/Edge/ProcedureId definitions (the core types being changed)
- `src/graph/builder.rs` — GraphBuilder (the build pipeline being restructured)
- `src/parser/extractor.rs` — CallExtractor + TableRefExtractor (the visitor being replaced)
- `src/export/json.rs` — JSON export format
- `tests/integration_test.rs` — Integration tests (many string checks to update)

**Existing patterns to follow:**
- Visitor trait: implement `ogsql_parser::Visitor` for new extractors, use `walk_statement` / `walk_pl_block` for recursion
- Builder flow: `create_procedure_nodes → collect_call_edges → create_edges → add_table_refs_from_sql → add_view_nodes`
- Edge creation: always pass `SourceLocation { file, line }`
- Package handling: manual item iteration (not `walk_statement`) for Package/PackageBody

---

## Prerequisite: AST Audit (COMPLETED)

All required fields verified in ogsql-parser `dc40706`:

| Statement | Write target field | Read source field |
|---|---|---|
| `SelectStatement` | `into_table: Option<SelectIntoTable>` | `from: Vec<TableRef>`, `lock_clause` for FOR UPDATE |
| `InsertStatement` | `table: ObjectName` | `source: InsertSource` (Values/Select/DefaultValues/Set) |
| `InsertAllStatement` | `targets: Vec<InsertAllTarget>` (.table each) | `source: Box<SelectStatement>` |
| `UpdateStatement` | `tables: Vec<TableRef>` | `from: Vec<TableRef>`, assignments[].value (Expr, may contain subqueries) |
| `DeleteStatement` | `tables: Vec<TableRef>` | `using: Vec<TableRef>` |
| `MergeStatement` | `target: TableRef` | `source: TableRef` |
| `TruncateStatement` | `tables: Vec<ObjectName>` | none |
| `LockClause` | n/a | `Update { tables }` / `Share { tables }` — for FOR UPDATE/FOR SHARE |

## Design Decisions (Pre-Confirmed)

1. **RoutineId**: Unified type with `kind: RoutineKind` field (not split into two types). Minimizes HashMap/CallEdge changes.
2. **Edge merge**: Lazy (post-pass dedup). Build all edges, then scan and OR modes.
3. **write_kinds**: `HashSet<WriteKind>` — deduplicated, serializable.
4. **JSON format**: Array of strings `["read", "write"]` for modes, `["insert", "update"]` for write_kinds.
5. **Cache**: Bump `GraphStore.version` to 2, refuse to load v1 caches.
6. **Node split**: `Node::Procedure` + `Node::Function` as separate enum variants (user requirement).

---

## Task 1: Add bitflags dependency

- [ ] Step 1: Add `bitflags = { version = "2", features = ["serde"] }` to `[dependencies]` in `Cargo.toml`
- [ ] Step 2: `cargo build` — verify it compiles
- [ ] Step 3: Commit: `chore: add bitflags dependency for AccessMode`

---

## Task 2: Define new types — AccessMode, WriteKind, RoutineKind, RoutineId

- [ ] Step 1: Write 6 failing unit tests in `src/graph/mod.rs` `#[cfg(test)]` module:
  - `access_mode_bitflags_or` — Read|Write contains both, not LockRead
  - `access_mode_empty_is_invalid` — empty contains nothing
  - `write_kind_serialization_roundtrip` — HashSet<WriteKind> serde JSON roundtrip
  - `routine_id_with_kind` — display format with package
  - `routine_id_function_display` — display format with schema
  - `routine_id_equality_includes_kind` — same name but Procedure ≠ Function
- [ ] Step 2: `cargo test -- graph::tests::access_mode` etc — verify FAIL
- [ ] Step 3: Define types in `src/graph/mod.rs`:
  - `AccessMode` bitflags: Read=0b0001, Write=0b0010, LockRead=0b0100, Truncate=0b1000
  - `WriteKind` enum: Insert, InsertSelect, Update, Delete, MergeInsert, MergeUpdate, MergeDelete, SelectInto, Truncate
  - `RoutineKind` enum: Procedure, Function
  - `RoutineId` struct: { schema: Option<String>, package: Option<String>, name: String, kind: RoutineKind } with Display, PartialEq, Eq, Hash, Serialize, Deserialize
- [ ] Step 4: `cargo test -- graph::tests` — verify PASS
- [ ] Step 5: Commit: `feat: define AccessMode, WriteKind, RoutineKind, RoutineId with tests`

---

## Task 3: Add Node::Function variant + NodeKey::Function — ATOMIC across 9 files

This task must change ALL match arms in one commit. Files: mod.rs, key.rs, store.rs, builder.rs, json.rs, dot.rs, mermaid.rs, main.rs, tui/app.rs.

- [ ] Step 1: Write failing integration test `test_function_creates_function_node` in `tests/integration_test.rs` — creates a `CREATE FUNCTION`, asserts JSON output has `"type": "function"` node
- [ ] Step 2: `cargo test test_function_creates_function_node` — verify FAIL
- [ ] Step 3: In `src/graph/mod.rs`:
  - Add `Node::Function { id: RoutineId, location: SourceLocation }` variant
  - Update `file()` match arm for `Node::Function`
- [ ] Step 4: In `src/graph/key.rs`:
  - Add `NodeKey::Function { schema, package, name }` variant
  - Update `from_node()` to map `Node::Function → NodeKey::Function`
  - Update `Display` impl for `NodeKey::Function`
- [ ] Step 5: In `src/graph/store.rs`:
  - Add `functions: usize` to `StoreStats`
  - Update `stats()` to count `Node::Function`
  - Update `node_source_file()` match arm
- [ ] Step 6: In `src/graph/builder.rs`:
  - `Statement::CreateFunction` → create `Node::Function` with `RoutineId { kind: RoutineKind::Function }`
  - `PackageItem::Function` in `create_package_nodes` → create `Node::Function`
  - `PackageItem::Function` in `collect_package_call_edges` → set `RoutineId { kind: Function }`
  - `PackageItem::Function` in `add_package_table_refs` → look up with `RoutineId { kind: Function }`
- [ ] Step 7: In `src/export/json.rs`: Add `Function` to `NodeKindJson` with `#[serde(rename = "function")]`
- [ ] Step 8: In `src/export/dot.rs`: Add `Node::Function` match arm (ellipse shape, distinct from Procedure)
- [ ] Step 9: In `src/export/mermaid.rs`: Add `Node::Function` rendering
- [ ] Step 10: In `src/main.rs`: Add `"function"` to `node_type_tag()`, add function count to stats
- [ ] Step 11: In `src/tui/app.rs`: Add `Node::Function` to `node_tag()` with a distinct color
- [ ] Step 12: **Rename ProcedureId → RoutineId everywhere**: This is a project-wide rename. The struct gains a `kind` field. Update all `HashMap<ProcedureId, _>` → `HashMap<RoutineId, _>`, all `ProcedureId::from_object_name` calls, all `ProcedureId { schema, package, name }` constructors to include `kind`. Use `lsp_rename` if possible, otherwise manual search-replace.
- [ ] Step 13: Update existing tests that match on `"procedure"` type — functions should now appear as `"function"` in JSON. Fix assertions.
- [ ] Step 14: `cargo test && cargo clippy -- -D warnings && cargo fmt -- --check` — ALL PASS
- [ ] Step 15: Commit: `feat: split Node::Procedure into Node::Procedure + Node::Function with RoutineId`

---

## Task 4: Implement TableAccessExtractor

This is the core logic. Replace `TableRefExtractor` with a new visitor that distinguishes read/write.

- [ ] Step 1: Write 13 failing unit tests in `src/parser/extractor.rs`:
  1. `select_from_reads` — `SELECT * FROM t1 JOIN t2` → t1:Read, t2:Read
  2. `insert_values_writes` — `INSERT INTO t1 VALUES(1,2)` → t1:Write(Insert)
  3. `insert_select_read_write` — `INSERT INTO t_tgt SELECT * FROM t_src` → tgt:Write(InsertSelect), src:Read
  4. `update_writes` — `UPDATE t1 SET x=1` → t1:Write(Update)
  5. `update_from_reads_writes` — `UPDATE t1 SET x=1 FROM t2 WHERE ...` → t1:Write(Update), t2:Read
  6. `delete_writes` — `DELETE FROM t1` → t1:Write(Delete)
  7. `delete_using_reads_writes` — `DELETE FROM t1 USING t2 WHERE ...` → t1:Write(Delete), t2:Read
  8. `merge_all_writes` — MERGE with UPDATE+INSERT+DELETE → target:Write(MergeUpdate, MergeInsert, MergeDelete), source:Read
  9. `truncate_table` — `TRUNCATE TABLE t1` → t1:Truncate
  10. `select_for_update_locks` — `SELECT * FROM t1 FOR UPDATE` → t1:LockRead
  11. `same_table_read_write_merge` — `UPDATE t SET x=(SELECT y FROM t) WHERE id=1` → t:Read|Write(Update)
  12. `insert_all_multi_target` — `INSERT ALL INTO t1 ... INTO t2 SELECT * FROM src` → t1:Write(Insert), t2:Write(Insert), src:Read
  13. `select_into_table` — `SELECT * INTO t_new FROM t_src` → t_new:Write(SelectInto), t_src:Read (if ogsql-parser parses this)
- [ ] Step 2: `cargo test -- extractor::tests` — verify all FAIL
- [ ] Step 3: Implement `TableAccessInfo` struct (name, schema, modes, write_kinds) and `TableAccessExtractor` struct
- [ ] Step 4: Implement Visitor methods:
  - `visit_select`: extract reads from `from`, check `into_table` for Write(SelectInto), check `lock_clause` for LockRead. Recurse into subqueries.
  - `visit_insert`: add Write(Insert) on `table`. If `source` is `InsertSource::Select`, walk the source query for reads.
  - `visit_update`: add Write(Update) on `tables`. Extract reads from `from`. Recurse into assignment expressions for subqueries.
  - `visit_delete`: add Write(Delete) on `tables`. Extract reads from `using`.
  - `visit_merge`: iterate `when_clauses`, collect MergeInsert/MergeUpdate/MergeDelete based on `action`. Extract reads from `source`.
- [ ] Step 5: Handle TRUNCATE: This is a `Statement::Truncate` variant. The Visitor trait may not have `visit_truncate`. Check if `visit_statement` catches it, or add a manual check.
- [ ] Step 6: Handle same-table read+write merge: `add_mode` method should OR modes and union write_kinds when the same table appears multiple times.
- [ ] Step 7: `cargo test -- extractor::tests` — verify all PASS
- [ ] Step 8: Commit: `feat: implement TableAccessExtractor with read/write/lock distinction`

---

## Task 5: Replace Edge::ReferencesTable with Edge::TableAccess

- [ ] Step 1: Write 3 failing integration tests in `tests/integration_test.rs`:
  - `test_procedure_table_read_access` — SELECT FROM table → edge with modes=["read"]
  - `test_procedure_table_write_access` — INSERT INTO table → edge with modes=["write"], write_kinds=["insert"]
  - `test_insert_select_read_write` — INSERT INTO t SELECT FROM s → two edges: tgt:write(insert_select), src:read
- [ ] Step 2: `cargo test` — verify FAIL (table_access not produced)
- [ ] Step 3: In `src/graph/mod.rs`: Replace `Edge::ReferencesTable { location }` with `Edge::TableAccess { modes: AccessMode, write_kinds: HashSet<WriteKind>, location: SourceLocation }`
- [ ] Step 4: In `src/graph/store.rs`:
  - `edge_type_tag`: return `"table_access"` for `Edge::TableAccess`
  - `GraphStore.version`: bump to 2
  - Add version check in `load_bincode`: refuse version != 2
  - `merge`: when dedup key matches, OR modes and union write_kinds instead of dropping
- [ ] Step 5: In `src/export/json.rs`:
  - Replace `EdgeKindJson::ReferencesTable` with `EdgeKindJson::TableAccess`
  - Custom serde: modes as `["read", "write"]` array, write_kinds as `["insert", "update"]` array
  - Include file and line fields
- [ ] Step 6: In `src/export/dot.rs`: color by mode — blue=Read, red=Write, purple=mixed, orange=LockRead. Label includes write_kinds.
- [ ] Step 7: In `src/export/mermaid.rs`: dashed arrow for read, solid for write, text label with mode
- [ ] Step 8: In `src/graph/builder.rs`:
  - Replace `extract_and_add_table_refs` with `collect_table_access` using `TableAccessExtractor`
  - Update all 5 call sites: standalone proc, standalone func, package item, iBatis mapper, Java SQL
  - Add `merge_table_access_edges(graph)` post-pass: scan all edges, find duplicate TableAccess edges between same node pair, OR modes + union write_kinds, remove duplicates
  - Update `add_view_nodes`: use `Edge::TableAccess { modes: Read, write_kinds: empty }`
  - Call `merge_table_access_edges` as final step in `build()` and `build_all()`
- [ ] Step 9: Update ALL existing tests — find `"references_table"` in integration_test.rs → replace with `"table_access"` + appropriate mode assertions
- [ ] Step 10: `cargo test && cargo clippy -- -D warnings && cargo fmt -- --check` — ALL PASS
- [ ] Step 11: Commit: `feat: replace ReferencesTable with TableAccess (read/write/lock semantics)`

---

## Task 6: StoreStats functions count + cache rejection test

- [ ] Step 1: Write failing test: `test_old_cache_version_rejected` — serialize a GraphStore with version=1, attempt load, assert error
- [ ] Step 2: `cargo test test_old_cache_version_rejected` — verify FAIL
- [ ] Step 3: Implement version check in `load_bincode` / `load_json`
- [ ] Step 4: Verify `StoreStats.functions` is counted in `stats()` (should be done in Task 3)
- [ ] Step 5: `cargo test` — ALL PASS
- [ ] Step 6: Commit: `test: add cache version rejection test`

---

## Task 7: Add comprehensive table access integration tests

- [ ] Step 1: Add these tests to `tests/integration_test.rs`:
  - `test_update_with_subquery_reads` — `UPDATE t SET x=(SELECT y FROM t2)` → t:Write(Update), t2:Read
  - `test_delete_using_reads` — `DELETE FROM t USING t2` → t:Write(Delete), t2:Read
  - `test_same_table_multiple_accesses_merge_to_one_edge` — proc with INSERT+SELECT+UPDATE on same table → ONE edge with Read|Write, write_kinds=[Insert, Update]
  - `test_view_reads_from_table` — view→table edge with Read mode
  - `test_package_procedure_table_access` — package body proc with DML → correct modes
- [ ] Step 2: `cargo test` — ALL PASS
- [ ] Step 3: Commit: `test: add comprehensive DML table access integration tests`

---

## Task 8: Cleanup — remove deprecated types

- [ ] Step 1: Remove old `ProcedureId` struct (fully replaced by `RoutineId`)
- [ ] Step 2: Remove old `TableRef` and `TableRefExtractor` (replaced by `TableAccessInfo` and `TableAccessExtractor`)
- [ ] Step 3: Remove `#[ignore]` diagnostic test `diagnose_deposit_package` in `src/graph/builder.rs`
- [ ] Step 4: Update `src/parser/mod.rs` pub use exports
- [ ] Step 5: `cargo test && cargo clippy -- -D warnings && cargo fmt -- --check` — ALL clean
- [ ] Step 6: Commit: `chore: remove deprecated ProcedureId, TableRefExtractor, diagnostic test`

---

## Task Dependency Graph

```
Task 1 (bitflags)
  └→ Task 2 (types + tests)
       ├→ Task 3 (Node::Function, ATOMIC 9 files)
       │     └→ Task 6 (store version + cache test)
       └→ Task 4 (TableAccessExtractor, 13 tests)
            └→ Task 5 (Edge::TableAccess + builder wire-up)  ← also depends on Task 3
                 └→ Task 7 (integration tests)
                      └→ Task 8 (cleanup)
```

**Critical path:** 1 → 2 → 3 → 5 → 7 → 8
**Parallelizable after Task 2:** Task 3 and Task 4 can run in parallel

---

## Reference: Existing code locations (65 sites that change)

| Symbol | Files | Sites | Nature |
|---|---|---|---|
| `ProcedureId` → `RoutineId` | mod.rs, key.rs, builder.rs, extractor.rs, store.rs, traverse.rs, json.rs | ~30 | Rename + add kind field |
| `Node::Procedure` | mod.rs, key.rs, builder.rs, store.rs, json.rs, dot.rs, mermaid.rs, main.rs, tui/app.rs | 23 | Split into Procedure + Function |
| `Edge::ReferencesTable` | mod.rs, builder.rs, store.rs, json.rs, dot.rs, mermaid.rs | 8 | Replace with TableAccess |
| `"references_table"` string | integration_test.rs | 14 | Update to `"table_access"` + modes |
| `TableRefExtractor`/`TableRef` | extractor.rs, mod.rs, builder.rs | 14 | Replace with TableAccessExtractor |

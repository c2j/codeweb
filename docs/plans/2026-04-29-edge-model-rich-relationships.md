# Edge Model Rich Relationships Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Introduce `EdgeCategory` semantic grouping, `CallScope` (intra/cross/external) for call edges, `DataFlowKind` (DML access vs definition dependency) for table access edges, and a new `DependsOn` edge for view/mview → table structural dependencies. This makes the graph semantically richer for impact analysis, coupling metrics, and trace queries.

**Architecture:** Flat `Edge` enum with derived `Edge::category()` method — no nested enums. `CallScope` determined at build time by comparing caller/callee `RoutineId.package`. View/mview table dependencies change from `TableAccess { Read }` to `DependsOn`. CGEF import/export extended with new edge type strings and optional properties. `GraphStore.version` bumped from 3 to 4.

**Tech Stack:** Rust, ogsql-parser (Visitor trait), petgraph, bitflags, serde (JSON + bincode).

---

## Execution Context

**Project root:** `/Users/c2j/Projects/Desktop_Projects/CODE/cobweb`

**Key files to understand before starting:**
- `src/graph/mod.rs` — `Edge` enum, `AccessMode`, `WriteKind`, `RoutineId` (core types being changed)
- `src/graph/builder.rs` — `GraphBuilder`, `create_edges()`, `collect_table_access_from_statements()`, view/mview node creation
- `src/graph/store.rs` — `edge_type_tag()`, `merge()`, `merge_duplicate_table_access_edges()`, `GraphStore.version`
- `src/graph/traverse.rs` — `edge_label_for()`
- `src/parser/extractor.rs` — `CallEdge { caller: Option<RoutineId>, callee_name }`
- `src/import/parser.rs` — `convert_standard_edge()`
- `src/import/validator.rs` — `standard_edge_types()`
- `src/export/json.rs` — `EdgeKindJson`, `to_json()`
- `src/export/dot.rs` — edge rendering
- `src/export/mermaid.rs` — edge rendering

**Existing patterns to follow:**
- Enum variants carry their data; no trait objects
- `SourceLocation { file: Arc<PathBuf>, line: usize }` on every edge with source info
- CGEF edge type is a flat string; standard types checked in `validator.rs`, unknown types → `CustomEdge`
- `edge_type_tag()` returns unique string per edge variant for dedup in `merge()`

---

## Design: New Types

```rust
/// Top-level semantic category for graph edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EdgeCategory {
    /// Control flow: one routine invokes another (DirectCall, DynamicCall, CallsProcedure, etc.)
    Call,
    /// Structural containment: package→routine, class→method
    Composition,
    /// Data flow: read/write between routines and tables, plus structural dependencies
    DataFlow,
    /// Type/object reference: routine references a type, sequence, trigger, synonym
    Reference,
    /// Inheritance: extends/implements
    Inheritance,
}

/// Scope of a call relationship — determined by comparing caller and callee package context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CallScope {
    /// Call within the same package (highest coupling, internal implementation detail)
    IntraPackage,
    /// Call across different packages (interface-level coupling)
    CrossPackage,
    /// Call to an external/standalone routine (no package on either side, or only one has package)
    External,
}

/// Distinguishes data flow semantics for TableAccess edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DataFlowKind {
    /// Runtime DML access (procedure/function → table via SELECT/INSERT/UPDATE/DELETE)
    DmlAccess,
    /// Definition-time dependency (view/materialized view → table; object cannot exist without target)
    DefinitionDependency,
}
```

## Design: Edge::category() method

```rust
impl Edge {
    pub fn category(&self) -> EdgeCategory {
        match self {
            Edge::DirectCall { .. }
            | Edge::DynamicCall { .. }
            | Edge::CallsProcedure { .. }
            | Edge::CallsJava { .. }
            | Edge::InvokesMapper { .. } => EdgeCategory::Call,

            Edge::ContainsRoutine | Edge::ContainsMethod => EdgeCategory::Composition,

            Edge::TableAccess { .. } | Edge::DependsOn { .. } => EdgeCategory::DataFlow,

            Edge::TriggersRoutine { .. }
            | Edge::ReferencesType { .. }
            | Edge::UsesSequence { .. }
            | Edge::IndexesTable { .. }
            | Edge::AliasesObject { .. } => EdgeCategory::Reference,

            Edge::Extends { .. } | Edge::Implements { .. } => EdgeCategory::Inheritance,

            Edge::CustomEdge { .. } => EdgeCategory::Reference, // default fallback
        }
    }
}
```

## Design: Edge enum changes

```rust
pub enum Edge {
    // ── Call edges (EdgeCategory::Call) ──
    DirectCall {
        scope: CallScope,  // NEW FIELD
        location: SourceLocation,
    },
    DynamicCall { raw_expr: String, location: SourceLocation },
    CallsProcedure { location: SourceLocation },
    InvokesMapper { location: SourceLocation },
    CallsJava { location: SourceLocation },

    // ── Composition edges (EdgeCategory::Composition) ──
    ContainsMethod,
    ContainsRoutine,

    // ── Data flow edges (EdgeCategory::DataFlow) ──
    TableAccess {
        flow_kind: DataFlowKind,  // NEW FIELD
        modes: AccessMode,
        write_kinds: std::collections::HashSet<WriteKind>,
        location: SourceLocation,
    },
    /// NEW: Structural definition dependency (view/mview → table).
    DependsOn {
        location: SourceLocation,
    },

    // ── Inheritance edges (EdgeCategory::Inheritance) ──
    Extends { location: SourceLocation },
    Implements { location: SourceLocation },

    // ── Reference edges (EdgeCategory::Reference) ──
    TriggersRoutine { location: SourceLocation },
    ReferencesType { location: SourceLocation },
    UsesSequence { location: SourceLocation },
    IndexesTable { location: SourceLocation },
    AliasesObject { location: SourceLocation },
    CustomEdge {
        type_name: String,
        properties: JsonMap,
        location: Option<SourceLocation>,
    },
}
```

## Design: CGEF format changes

| Internal Edge | CGEF `edge_type` | CGEF `properties` |
|---|---|---|
| `DirectCall { scope: IntraPackage, .. }` | `"direct"` | `{ "scope": "intra_package" }` |
| `DirectCall { scope: CrossPackage, .. }` | `"direct"` | `{ "scope": "cross_package" }` |
| `DirectCall { scope: External, .. }` | `"direct"` | `{ "scope": "external" }` or `{}` (default) |
| `TableAccess { flow_kind: DmlAccess, .. }` | `"table_access"` | `{ "flow_kind": "dml_access", "modes": [...], "write_kinds": [...] }` |
| `TableAccess { flow_kind: DefinitionDependency, .. }` | `"table_access"` | `{ "flow_kind": "definition_dependency", "modes": [...], "write_kinds": [...] }` |
| `DependsOn { .. }` | `"depends_on"` | `{}` |

**Backward compatibility:** Old CGEF documents with `"type": "direct"` and no `scope` property → default to `CallScope::External`. Old `"type": "table_access"` with no `flow_kind` → default to `DataFlowKind::DmlAccess`.

## Design: CallScope inference

```rust
fn determine_call_scope(caller: &RoutineId, callee: &RoutineId) -> CallScope {
    match (&caller.package, &callee.package) {
        (Some(pkg_a), Some(pkg_b)) if pkg_a.eq_ignore_ascii_case(pkg_b) => CallScope::IntraPackage,
        (Some(_), Some(_)) => CallScope::CrossPackage,
        _ => CallScope::External,
    }
}
```

Applied in `builder.rs::create_edges()` where both caller and callee RoutineIds are available.

For CGEF import: after all nodes and edges are created, run a post-pass that:
1. Iterates all `DirectCall` edges with `scope == External` (the default)
2. Looks up source/target nodes
3. If both are Procedure/Function with package info, re-determines scope
4. Updates edge weight in-place

## Design: edge_type_tag changes

```rust
fn edge_type_tag(edge: &Edge) -> String {
    match edge {
        Edge::DirectCall { scope, .. } => match scope {
            CallScope::IntraPackage => "intra_call",
            CallScope::CrossPackage => "cross_call",
            CallScope::External => "direct",
        },
        Edge::DependsOn { .. } => "depends_on",
        Edge::TableAccess { flow_kind, .. } => match flow_kind {
            DataFlowKind::DmlAccess => "table_access",
            DataFlowKind::DefinitionDependency => "table_access_def",
        },
        // ... rest unchanged
    }
    .to_string()
}
```

This ensures `intra_call` and `cross_call` edges between the same node pair are NOT merged together (different tags = different dedup keys). `table_access` and `table_access_def` are also kept separate.

---

## Task Dependency Graph

```
Task 1 (new types)
  └→ Task 2 (Edge enum changes + category — ATOMIC across 11 files)
       ├→ Task 3 (builder.rs — CallScope inference)
       ├→ Task 4 (builder.rs — DependsOn for view/mview)
       ├→ Task 5 (store.rs — version bump + merge)
       ├→ Task 6 (CGEF import — parser + validator)
       ├→ Task 7 (CGEF export — json + dot + mermaid)
       └→ Task 8 (traverse.rs — edge labels)
  Task 9 (integration tests)
  Task 10 (cleanup — version check, clippy, fmt)
```

Tasks 3-8 can run in parallel after Task 2. Task 9 runs after all of 3-8. Task 10 is final.

---

## Task 1: Define EdgeCategory, CallScope, DataFlowKind types

**Files:**
- Modify: `src/graph/mod.rs` (after `WriteKind` enum, before `RoutineId`)

**Step 1: Write failing unit tests**

Add to `src/graph/mod.rs` `#[cfg(test)]` module:

```rust
#[test]
fn call_scope_variants() {
    let intra = CallScope::IntraPackage;
    let cross = CallScope::CrossPackage;
    let external = CallScope::External;
    assert_ne!(intra, cross);
    assert_ne!(cross, external);
    assert_ne!(intra, external);
}

#[test]
fn call_scope_serialization_roundtrip() {
    let scope = CallScope::IntraPackage;
    let json = serde_json::to_string(&scope).unwrap();
    let de: CallScope = serde_json::from_str(&json).unwrap();
    assert_eq!(scope, de);
}

#[test]
fn data_flow_kind_serialization_roundtrip() {
    let kind = DataFlowKind::DefinitionDependency;
    let json = serde_json::to_string(&kind).unwrap();
    let de: DataFlowKind = serde_json::from_str(&json).unwrap();
    assert_eq!(kind, de);
}

#[test]
fn edge_category_from_edge() {
    // Will be enabled after Task 2 adds scope field to DirectCall
}
```

**Step 2: Run to verify fail**

```sh
cargo test -- graph::tests::call_scope 2>&1 | head -20
```

Expected: FAIL (types not defined)

**Step 3: Define types**

Add to `src/graph/mod.rs` after `WriteKind`:

```rust
/// Scope of a call relationship.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CallScope {
    /// Call within the same package.
    IntraPackage,
    /// Call across different packages.
    CrossPackage,
    /// Call to external/standalone routine.
    External,
}

/// Distinguishes data flow semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DataFlowKind {
    /// Runtime DML access.
    DmlAccess,
    /// Definition-time dependency.
    DefinitionDependency,
}

/// Top-level semantic category for graph edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EdgeCategory {
    Call,
    Composition,
    DataFlow,
    Reference,
    Inheritance,
}
```

**Step 4: Run tests**

```sh
cargo test -- graph::tests::call_scope
```

Expected: PASS

**Step 5: Commit**

```
feat: define EdgeCategory, CallScope, DataFlowKind types
```

---

## Task 2: Edge enum changes + Edge::category() — ATOMIC across 11 files

This is the critical atomic task. ALL match arms on `Edge` must be updated in one commit.

**Files to modify (all in one commit):**
1. `src/graph/mod.rs` — Edge definition + category() + tests
2. `src/graph/store.rs` — edge_type_tag(), merge logic, version
3. `src/graph/builder.rs` — all Edge::DirectCall construction sites
4. `src/graph/traverse.rs` — edge_label_for()
5. `src/export/json.rs` — EdgeKindJson, to_json()
6. `src/export/dot.rs` — edge rendering
7. `src/export/mermaid.rs` — edge rendering
8. `src/import/parser.rs` — convert_standard_edge()
9. `src/import/validator.rs` — standard_edge_types()
10. `src/main.rs` — any edge type tags
11. `src/tui/app.rs` — any edge display

**Step 1: Write failing integration test**

Add to `tests/integration_test.rs`:

```rust
#[test]
fn test_direct_call_with_scope_intra_package() {
    let sql = r#"
        CREATE OR REPLACE PACKAGE BODY pkg_api AS
            PROCEDURE do_work(p_id INT) IS
            BEGIN
                helper(p_id);
            END;
        END pkg_api;
    "#;
    // Build graph, find DirectCall edge, assert scope == IntraPackage
    // (helper should resolve to pkg_api.helper if declared in same package)
}

#[test]
fn test_view_table_dependency_is_depends_on() {
    let sql = r#"
        CREATE TABLE t_users (id INT);
        CREATE VIEW v_active AS SELECT id FROM t_users WHERE status = 'A';
    "#;
    // Build graph, find edge from View to Table
    // Assert it is Edge::DependsOn, NOT Edge::TableAccess
}

#[test]
fn test_procedure_table_access_has_dml_flow_kind() {
    let sql = r#"
        CREATE PROCEDURE sp_read() AS $$ BEGIN SELECT * FROM t_users; END; $$;
    "#;
    // Build graph, find TableAccess edge
    // Assert flow_kind == DataFlowKind::DmlAccess
}
```

**Step 2: Modify Edge enum in `src/graph/mod.rs`**

Add `scope: CallScope` to `DirectCall`:
```rust
DirectCall {
    scope: CallScope,
    location: SourceLocation,
},
```

Add `flow_kind: DataFlowKind` to `TableAccess`:
```rust
TableAccess {
    flow_kind: DataFlowKind,
    modes: AccessMode,
    write_kinds: std::collections::HashSet<WriteKind>,
    location: SourceLocation,
},
```

Add new `DependsOn` variant:
```rust
/// Structural definition dependency (view/mview → table).
DependsOn {
    location: SourceLocation,
},
```

Add `Edge::category()` method.

Update `#[cfg(test)]` to fix all Edge construction sites (add `scope: CallScope::External` to all `Edge::DirectCall { ... }`, add `flow_kind: DataFlowKind::DmlAccess` to all `Edge::TableAccess { ... }`).

**Step 3: Update `src/graph/store.rs`**

- `edge_type_tag()`: handle new scope-based tags and `DependsOn`
- `merge()`: update `table_access_merge_map` key to include flow_kind differentiation
- `merge_duplicate_table_access_edges()`: only merge edges with same `flow_kind`
- `GraphStore.version`: bump from `3` to `4`
- Update version check messages

**Step 4: Update `src/graph/builder.rs`**

All `Edge::DirectCall { location }` → `Edge::DirectCall { scope: CallScope::External, location }` (temporary default; Task 3 will add proper inference).

All `Edge::TableAccess { modes, write_kinds, location }` → `Edge::TableAccess { flow_kind: DataFlowKind::DmlAccess, modes, write_kinds, location }`.

For view and mview table references: change from `Edge::TableAccess { modes: Read, ... }` to `Edge::DependsOn { location }`. This is in:
- `create_sql_nodes()` where `Statement::CreateView` is handled
- `create_sql_nodes()` where `Statement::CreateMaterializedView` is handled

**Step 5: Update `src/graph/traverse.rs`**

- `edge_label_for()`: add match arms for `DirectCall { scope, .. }` (display scope), `DependsOn` (display "[depends_on]")

**Step 6: Update `src/export/json.rs`**

- `EdgeKindJson::Direct` → add `scope: String` field
- `EdgeKindJson::TableAccess` → add `flow_kind: String` field
- Add `EdgeKindJson::DependsOn { file, line }`
- Update `to_json()` match arms

**Step 7: Update `src/export/dot.rs`**

- `Edge::DirectCall { scope, .. }`: color by scope (intra=darkgreen, cross=blue, external=default)
- `Edge::DependsOn { .. }`: dashed arrow, label "depends_on"

**Step 8: Update `src/export/mermaid.rs`**

- `Edge::DirectCall { scope, .. }`: arrow style by scope
- `Edge::DependsOn { .. }`: dotted arrow

**Step 9: Update `src/import/parser.rs`**

- `"direct"` → parse optional `scope` from properties, default `External`
- `"table_access"` → parse optional `flow_kind` from properties, default `DmlAccess`
- Add `"depends_on"` → `Edge::DependsOn { location }`

**Step 10: Update `src/import/validator.rs`**

- Add `"depends_on"` to `standard_edge_types()`

**Step 11: Update `src/main.rs` and `src/tui/app.rs`**

- Any edge type tag display → add `"intra_call"`, `"cross_call"`, `"depends_on"`, `"table_access_def"`

**Step 12: Build and test**

```sh
cargo test && cargo clippy -- -D warnings && cargo fmt -- --check
```

Expected: ALL PASS (with scope still hardcoded to External, flow_kind to DmlAccess)

**Step 13: Commit**

```
feat: add CallScope, DataFlowKind, DependsOn to Edge model (11-file atomic change)

- DirectCall gains scope: CallScope (default External)
- TableAccess gains flow_kind: DataFlowKind (default DmlAccess)
- New DependsOn edge for view/mview → table dependencies
- Edge::category() method for semantic grouping
- CGEF import/export updated
- GraphStore version bumped to 4
```

---

## Task 3: builder.rs — CallScope inference for DirectCall

**Files:**
- Modify: `src/graph/builder.rs`

**Step 1: Write failing unit test**

```rust
#[test]
fn intra_package_call_gets_intra_scope() {
    let sql = r#"
        CREATE OR REPLACE PACKAGE BODY pkg_api AS
            PROCEDURE do_work(p_id INT) IS
            BEGIN
                helper(p_id);
            END;
            PROCEDURE helper(p_id INT) IS
            BEGIN
                NULL;
            END;
        END pkg_api;
    "#;
    let graph = build_from_sql(sql);
    // Find DirectCall edge from do_work → helper
    // Assert scope == CallScope::IntraPackage
}

#[test]
fn cross_package_call_gets_cross_scope() {
    let sql = r#"
        CREATE OR REPLACE PACKAGE BODY pkg_api AS
            PROCEDURE do_work(p_id INT) IS
            BEGIN
                pkg_utils.format_date(SYSDATE);
            END;
        END pkg_api;

        CREATE OR REPLACE PACKAGE BODY pkg_utils AS
            FUNCTION format_date(d DATE) RETURN VARCHAR2 IS
            BEGIN
                RETURN TO_CHAR(d);
            END;
        END pkg_utils;
    "#;
    let graph = build_from_sql(sql);
    // Find DirectCall edge from do_work → format_date
    // Assert scope == CallScope::CrossPackage
}
```

**Step 2: Implement scope inference**

In `create_edges()`, after resolving `callee_idx`:

```rust
let scope = match (&edge.caller, callee_idx) {
    (Some(caller_id), Some(_)) => {
        // Look up the callee's RoutineId from graph
        let callee_routine_id = extract_routine_id(&graph[callee_idx.unwrap()]);
        callee_routine_id
            .map(|callee_id| determine_call_scope(caller_id, &callee_id))
            .unwrap_or(CallScope::External)
    }
    _ => CallScope::External,
};
```

Add helper:
```rust
fn extract_routine_id(node: &Node) -> Option<RoutineId> {
    match node {
        Node::Procedure { id, .. } | Node::Function { id, .. } => Some(id.clone()),
        _ => None,
    }
}

fn determine_call_scope(caller: &RoutineId, callee: &RoutineId) -> CallScope {
    match (&caller.package, &callee.package) {
        (Some(a), Some(b)) if a.eq_ignore_ascii_case(b) => CallScope::IntraPackage,
        (Some(_), Some(_)) => CallScope::CrossPackage,
        _ => CallScope::External,
    }
}
```

**Step 3: Run tests**

```sh
cargo test -- graph::builder::tests::intra_package
cargo test -- graph::builder::tests::cross_package
```

Expected: PASS

**Step 4: Commit**

```
feat: infer CallScope (intra/cross/external) for DirectCall edges at build time
```

---

## Task 4: builder.rs — DependsOn for view/mview → table

**Files:**
- Modify: `src/graph/builder.rs`

**Step 1: Write failing unit test**

```rust
#[test]
fn view_table_is_depends_on_not_table_access() {
    let sql = r#"
        CREATE TABLE t_users (id INT, status VARCHAR(10));
        CREATE VIEW v_active AS SELECT id FROM t_users WHERE status = 'A';
    "#;
    let graph = build_from_sql(sql);
    let view_idx = graph.node_indices()
        .find(|i| matches!(&graph[*i], Node::View { name, .. } if name == "v_active"))
        .unwrap();
    let edges: Vec<_> = graph.edges_directed(view_idx, petgraph::Direction::Outgoing).collect();
    assert_eq!(edges.len(), 1);
    assert!(matches!(edges[0].weight(), Edge::DependsOn { .. }));
    assert!(!matches!(edges[0].weight(), Edge::TableAccess { .. }));
}

#[test]
fn materialized_view_table_is_depends_on() {
    let sql = r#"
        CREATE TABLE t_orders (id INT, amount NUMERIC);
        CREATE MATERIALIZED VIEW mv_summary AS SELECT SUM(amount) FROM t_orders;
    "#;
    let graph = build_from_sql(sql);
    let mview_idx = graph.node_indices()
        .find(|i| matches!(&graph[*i], Node::MaterializedView { name, .. } if name == "mv_summary"))
        .unwrap();
    let edges: Vec<_> = graph.edges_directed(mview_idx, petgraph::Direction::Outgoing).collect();
    assert_eq!(edges.len(), 1);
    assert!(matches!(edges[0].weight(), Edge::DependsOn { .. }));
}
```

Note: These should already pass from Task 2's changes (view/mview sites already switched to `DependsOn`). This test confirms correctness.

**Step 2: Verify the change sites**

Confirm these locations in `create_sql_nodes()` now produce `Edge::DependsOn`:

1. `Statement::CreateView` — the loop `for access in &extractor.accesses` should create `Edge::DependsOn { location }` instead of `Edge::TableAccess { modes: Read, ... }`
2. `Statement::CreateMaterializedView` — same change

**Step 3: Run tests**

```sh
cargo test -- graph::builder::tests::view_table
cargo test -- graph::builder::tests::materialized_view_table
```

Expected: PASS

**Step 4: Commit**

```
test: add DependsOn edge tests for view and materialized view
```

---

## Task 5: store.rs — version bump + merge logic update

**Files:**
- Modify: `src/graph/store.rs`

**Step 1: Update version**

- `version: 4` in `new()` and `from_graph()`
- Error messages: `"expected 4"`

**Step 2: Update `edge_type_tag()`**

Add the new scope-based tags and `DependsOn` as shown in Design section above.

**Step 3: Update `merge_duplicate_table_access_edges()`**

Ensure only edges with the same `flow_kind` are merged together:

```rust
fn merge_duplicate_table_access_edges(graph: &mut CodeGraph) {
    let mut merge_targets: HashMap<
        (petgraph::graph::NodeIndex, petgraph::graph::NodeIndex, DataFlowKind),
        Vec<petgraph::graph::EdgeIndex>,
    > = HashMap::new();
    for edge_idx in graph.edge_indices() {
        if let Edge::TableAccess { flow_kind, .. } = &graph[edge_idx] {
            let (src, dst) = graph.edge_endpoints(edge_idx).unwrap();
            merge_targets.entry((src, dst, *flow_kind)).or_default().push(edge_idx);
        }
    }
    // ... rest of merge logic (same as before, but keyed by flow_kind)
}
```

**Step 4: Write test**

```rust
#[test]
fn test_merge_preserves_different_flow_kinds() {
    // Create graph with two TableAccess edges between same nodes
    // One with DmlAccess, one with DefinitionDependency
    // After merge, both should still exist (not merged together)
}
```

**Step 5: Run tests**

```sh
cargo test -- graph::store
```

Expected: PASS

**Step 6: Commit**

```
feat: update GraphStore merge for flow_kind-aware dedup, bump version to 4
```

---

## Task 6: CGEF import — parser + validator + scope inference

**Files:**
- Modify: `src/import/parser.rs`
- Modify: `src/import/validator.rs`

**Step 1: Update `convert_standard_edge()`**

```rust
"direct" => {
    let scope = parse_call_scope(cgef.properties.as_ref());
    Ok(Edge::DirectCall {
        scope,
        location: location.unwrap_or_else(dummy_location),
    })
}
"depends_on" => Ok(Edge::DependsOn {
    location: location.unwrap_or_else(dummy_location),
}),
"table_access" => {
    let flow_kind = parse_flow_kind(cgef.properties.as_ref());
    let modes = parse_access_modes(cgef.properties.as_ref());
    let write_kinds = parse_write_kinds(cgef.properties.as_ref());
    Ok(Edge::TableAccess {
        flow_kind,
        modes,
        write_kinds,
        location: location.unwrap_or_else(dummy_location),
    })
}
```

Add helper functions:
```rust
fn parse_call_scope(props: Option<&serde_json::Value>) -> CallScope {
    props
        .and_then(|p| p.get("scope"))
        .and_then(|v| v.as_str())
        .and_then(|s| match s {
            "intra_package" => Some(CallScope::IntraPackage),
            "cross_package" => Some(CallScope::CrossPackage),
            "external" => Some(CallScope::External),
            _ => None,
        })
        .unwrap_or(CallScope::External)
}

fn parse_flow_kind(props: Option<&serde_json::Value>) -> DataFlowKind {
    props
        .and_then(|p| p.get("flow_kind"))
        .and_then(|v| v.as_str())
        .and_then(|s| match s {
            "dml_access" => Some(DataFlowKind::DmlAccess),
            "definition_dependency" => Some(DataFlowKind::DefinitionDependency),
            _ => None,
        })
        .unwrap_or(DataFlowKind::DmlAccess)
}
```

**Step 2: Add scope inference post-pass**

After all nodes and edges are created in `CgefParser::parse()`, add:

```rust
// Post-pass: infer CallScope from node metadata
Self::infer_call_scopes(&mut graph);
```

```rust
fn infer_call_scopes(graph: &mut CodeGraph) {
    let edge_indices: Vec<_> = graph.edge_indices().collect();
    for edge_idx in edge_indices {
        if let Edge::DirectCall { scope: CallScope::External, .. } = &graph[edge_idx] {
            let (src, dst) = graph.edge_endpoints(edge_idx).unwrap();
            let caller_id = Self::extract_routine_id(&graph[src]);
            let callee_id = Self::extract_routine_id(&graph[dst]);
            if let (Some(caller), Some(callee)) = (caller_id, callee_id) {
                let inferred = determine_call_scope(&caller, &callee);
                if let Edge::DirectCall { scope, .. } = &mut graph[edge_idx] {
                    *scope = inferred;
                }
            }
        }
    }
}
```

**Step 3: Update validator**

```rust
fn standard_edge_types() -> HashSet<&'static str> {
    [
        "direct",
        "intra_call",     // NEW
        "cross_call",     // NEW
        "depends_on",     // NEW
        "dynamic",
        "calls_procedure",
        // ... existing ...
    ]
    .into_iter()
    .collect()
}
```

**Step 4: Write tests**

```rust
#[test]
fn test_parse_direct_with_scope() {
    // Parse CGEF with "type": "direct", "properties": { "scope": "intra_package" }
    // Assert Edge::DirectCall { scope: IntraPackage, .. }
}

#[test]
fn test_parse_depends_on() {
    // Parse CGEF with "type": "depends_on"
    // Assert Edge::DependsOn { .. }
}

#[test]
fn test_parse_table_access_with_flow_kind() {
    // Parse CGEF with "type": "table_access", "properties": { "flow_kind": "definition_dependency", ... }
    // Assert Edge::TableAccess { flow_kind: DefinitionDependency, .. }
}

#[test]
fn test_scope_inference_from_node_metadata() {
    // Import CGEF with two procedure nodes (same package) and a "direct" edge (no scope)
    // After parse, assert scope was inferred to IntraPackage
}
```

**Step 5: Run tests**

```sh
cargo test -- import::parser
```

**Step 6: Commit**

```
feat: CGEF import supports CallScope, DataFlowKind, DependsOn with scope inference
```

---

## Task 7: CGEF export — json + dot + mermaid

**Files:**
- Modify: `src/export/json.rs`
- Modify: `src/export/dot.rs`
- Modify: `src/export/mermaid.rs`

**Step 1: Update `src/export/json.rs`**

```rust
Edge::DirectCall { scope, location } => EdgeJson {
    source: src.index(),
    target: dst.index(),
    kind: EdgeKindJson::Direct {
        scope: match scope {
            CallScope::IntraPackage => "intra_package",
            CallScope::CrossPackage => "cross_package",
            CallScope::External => "external",
        }.to_string(),
        file: location.file.to_string_lossy().to_string(),
        line: location.line,
    },
},

Edge::TableAccess { flow_kind, modes, write_kinds, location } => {
    // ... existing mode/write_kind serialization ...
    EdgeJson {
        source: src.index(),
        target: dst.index(),
        kind: EdgeKindJson::TableAccess {
            flow_kind: match flow_kind {
                DataFlowKind::DmlAccess => "dml_access",
                DataFlowKind::DefinitionDependency => "definition_dependency",
            }.to_string(),
            modes: mode_strs,
            write_kinds: wk_strs,
            file: location.file.to_string_lossy().to_string(),
            line: location.line,
        },
    }
}

Edge::DependsOn { location } => EdgeJson {
    source: src.index(),
    target: dst.index(),
    kind: EdgeKindJson::DependsOn {
        file: location.file.to_string_lossy().to_string(),
        line: location.line,
    },
},
```

Update `EdgeKindJson`:
```rust
#[serde(rename = "direct")]
Direct { scope: String, file: String, line: usize },
#[serde(rename = "table_access")]
TableAccess { flow_kind: String, modes: Vec<String>, write_kinds: Vec<String>, file: String, line: usize },
#[serde(rename = "depends_on")]
DependsOn { file: String, line: usize },
```

**Step 2: Update `src/export/dot.rs`**

```rust
Edge::DirectCall { scope, .. } => {
    let (label, style) = match scope {
        CallScope::IntraPackage => (String::new(), "color=darkgreen,".to_string()),
        CallScope::CrossPackage => (String::new(), "color=blue,".to_string()),
        CallScope::External => (String::new(), String::new()),
    };
    (label, style)
}
Edge::DependsOn { .. } => {
    ("label=\"depends_on\"".to_string(), "style=dashed,color=teal,".to_string())
}
```

**Step 3: Update `src/export/mermaid.rs`**

```rust
Edge::DirectCall { scope, .. } => match scope {
    CallScope::IntraPackage => "==>",
    CallScope::CrossPackage => "-->",
    CallScope::External => "-->",
},
Edge::DependsOn { .. } => "-.->|depends_on|",
```

**Step 4: Run tests**

```sh
cargo test && cargo clippy -- -D warnings
```

**Step 5: Commit**

```
feat: export scope, flow_kind, DependsOn in JSON/DOT/Mermaid formats
```

---

## Task 8: traverse.rs — edge labels for new types

**Files:**
- Modify: `src/graph/traverse.rs`

**Step 1: Update `edge_label_for()`**

```rust
Edge::DirectCall { scope, .. } => Some(match scope {
    CallScope::IntraPackage => "[intra]".into(),
    CallScope::CrossPackage => "[cross]".into(),
    CallScope::External => "[external]".into(),
}),
Edge::DependsOn { .. } => Some("[depends_on]".into()),
```

**Step 2: Run tests**

```sh
cargo test
```

**Step 3: Commit**

```
feat: display CallScope and DependsOn labels in trace output
```

---

## Task 9: Integration tests

**Files:**
- Modify: `tests/integration_test.rs`

**Step 1: Add comprehensive tests**

```rust
#[test]
fn test_intra_package_call_scope() {
    // Package body with two procs, one calls the other
    // Verify DirectCall { scope: IntraPackage }
}

#[test]
fn test_cross_package_call_scope() {
    // Two package bodies, proc in pkg_A calls proc in pkg_B
    // Verify DirectCall { scope: CrossPackage }
}

#[test]
fn test_external_call_scope() {
    // Standalone proc calls another standalone proc
    // Verify DirectCall { scope: External }
}

#[test]
fn test_view_depends_on_table() {
    // CREATE VIEW ... SELECT FROM table
    // Verify DependsOn edge (not TableAccess)
}

#[test]
fn test_materialized_view_depends_on_table() {
    // CREATE MATERIALIZED VIEW ... SELECT FROM table
    // Verify DependsOn edge
}

#[test]
fn test_procedure_table_access_is_dml() {
    // Proc with SELECT FROM table
    // Verify TableAccess { flow_kind: DmlAccess }
}

#[test]
fn test_edge_category_method() {
    // Build graph, verify Edge::category() returns correct category for each edge type
}

#[test]
fn test_cgef_roundtrip_with_scope() {
    // Export to JSON, re-import, verify scope preserved
}

#[test]
fn test_cgef_roundtrip_with_depends_on() {
    // Export graph with DependsOn edge, re-import, verify preserved
}
```

**Step 2: Run all tests**

```sh
cargo test
```

**Step 3: Commit**

```
test: add integration tests for CallScope, DataFlowKind, DependsOn
```

---

## Task 10: Final verification and cleanup

**Step 1: Full test suite**

```sh
cargo test && cargo clippy -- -D warnings && cargo fmt -- --check
```

**Step 2: Verify cache version rejection**

```sh
cargo test test_old_cache_version_rejected
```

**Step 3: Final commit**

```
chore: verify all tests pass with rich edge model
```

---

## Reference: All Edge match sites (from codebase audit)

| File | Match sites | Change nature |
|---|---|---|
| `src/graph/mod.rs` | Edge definition (1), tests (3) | Add scope, flow_kind, DependsOn |
| `src/graph/store.rs` | edge_type_tag (14), merge (2), stats (0), tests (2) | New tags, flow_kind-aware merge |
| `src/graph/builder.rs` | Edge construction (~12), tests (6) | Add scope inference, DependsOn for views |
| `src/graph/traverse.rs` | edge_label_for (14) | Add scope label, DependsOn label |
| `src/export/json.rs` | EdgeKindJson (14), to_json (14) | Add scope, flow_kind, DependsOn fields |
| `src/export/dot.rs` | edge rendering (14) | Scope-based color, DependsOn |
| `src/export/mermaid.rs` | edge rendering (14) | Scope-based arrow, DependsOn |
| `src/import/parser.rs` | convert_standard_edge (14), tests (4) | Parse scope, flow_kind, depends_on |
| `src/import/validator.rs` | standard_edge_types (14) | Add new strings |
| `src/main.rs` | edge type display | Add new tags |
| `src/tui/app.rs` | edge display | Add new edge types |

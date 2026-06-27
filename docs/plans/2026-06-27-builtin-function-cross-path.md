# Built-in Function Cross-Path Modeling Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make built-in SQL functions (COUNT, SUBSTR, ...) extracted from XML mapper / Java / JSP SQL paths become first-class `BuiltinFunction` nodes + `UsesBuiltinFunction` edges — matching the SQL stored-procedure path that already works, with cross-path deduplication so the same builtin called from multiple paths is a single node.

**Architecture:** A shared `builtin_index: HashMap<String, NodeIndex>` lives on `GraphBuildContext` and is threaded through all four extraction paths (SQL-proc / XML-mapper / Java / JSP). `extract_calls_from_statements` stops dropping builtin edges and instead returns a rich `ExtractedCall` record; each consumer loop branches on `builtin_meta` to find-or-create a `BuiltinFunction` node via a single shared `find_or_create_builtin_node` helper (DRY across 4 call sites). Dedup is by lowercased name, consistent with `NodeKey::BuiltinFunction`.

**Tech Stack:** Rust, petgraph, ogsql-parser (provides `BuiltinFuncMeta { category, domain }`).

---

## Background Context

### What already works (do NOT change)
The SQL stored-procedure path models builtins correctly. In `src/graph/builder.rs`:
- `create_edges` (L1512) has a local `builtin_index` (L1527) and a builtin branch (L1555-1580) that finds-or-creates a `Node::BuiltinFunction` and connects the caller with `Edge::UsesBuiltinFunction`.
- `Node::BuiltinFunction`, `Edge::UsesBuiltinFunction`, `NodeKey::BuiltinFunction`, and the `skip_builtins` presentation filter in `traverse.rs` are all already implemented (see plan `2026-06-26-builtin-function-tracking.md`, Phases 1-4, shipped).

### What's broken (this plan fixes)
`extract_calls_from_statements` (L2038-2053) returns `Vec<String>` (callee names only) and **filters out builtin edges** at L2047 (`if edge.builtin_meta.is_none()`). Three consumers of this function therefore never see builtins:
- XML mapper: `add_ibatis_nodes_from_parsed_with_source_paths` loop at L1788-1829
- Java: `add_java_nodes_from_parsed_with_source_paths` loop at L1952-2005
- JSP: `add_jsp_nodes_from_parsed` loop at L2217-2264

### Key data structures (already exist, no changes needed)
```rust
// src/graph/mod.rs — already defined
Node::BuiltinFunction { name: String, category: String, domain: String, location: SourceLocation }
Edge::UsesBuiltinFunction { location: SourceLocation }

// src/graph/key.rs:245 — dedup key is lowercased name
Node::BuiltinFunction { name, .. } => NodeKey::BuiltinFunction { name: name.to_lowercase() }

// ogsql-parser ast — metadata on FunctionCall
pub struct BuiltinFuncMeta { pub category: String, pub domain: String }  // Clone-able

// src/parser/extractor.rs:150 — call record
pub struct CallEdge {
    pub caller: Option<RoutineId>,
    pub callee_name: String,
    pub is_dynamic: bool,
    pub location: SourceLocation,
    pub builtin_meta: Option<ogsql_parser::ast::BuiltinFuncMeta>,
}
```

### Design decisions (locked)
- **Shared `builtin_index` on `GraphBuildContext`** — not per-function local. This gives cross-path dedup for free: a builtin first created by a SQL proc is reused when a mapper later references it (all 4 paths run within one `ctx` lifetime).
- **Rich `ExtractedCall` return type** — not "caller re-iterates extractor.edges". Rationale: keeps extraction logic in one place, consumers stay simple.
- **Single `find_or_create_builtin_node` helper** — used by all 4 paths (proc + XML + Java + JSP). DRY.
- **Location** — builtin node + edge location uses the statement-level location (`stmt.line` / `extraction.origin.line`), matching how each consumer already builds `CallsProcedure { location }`. The extractor's per-edge location has line 0 (not useful for statement-level SQL).

---

## Phase 1: Data Plumbing (no behavior change — foundation)

These tasks add the shared infrastructure without changing observable behavior. Existing tests must still pass after each task.

### Task 1.1: Add `builtin_index` to `GraphBuildContext`

**Files:**
- Modify: `src/graph/builder.rs:21-43`

**Step 1:** Add the field to the struct (after `sequence_index`, L28):

```rust
pub struct GraphBuildContext {
    pub graph: CodeGraph,
    pub proc_index: HashMap<RoutineId, petgraph::graph::NodeIndex>,
    pub package_index: HashMap<String, petgraph::graph::NodeIndex>,
    pub mapper_index: HashMap<String, petgraph::graph::NodeIndex>,
    pub table_index: HashMap<String, petgraph::graph::NodeIndex>,
    pub type_index: HashMap<String, petgraph::graph::NodeIndex>,
    pub sequence_index: HashMap<String, petgraph::graph::NodeIndex>,
    /// Shared dedup index for BuiltinFunction nodes (keyed by lowercased name).
    /// Threaded through SQL-proc / XML-mapper / Java / JSP paths so the same
    /// builtin called from multiple paths is a single graph node.
    pub builtin_index: HashMap<String, petgraph::graph::NodeIndex>,
}
```

**Step 2:** Initialize in `GraphBuildContext::new()` (add after `sequence_index: HashMap::new()`, L40):

```rust
builtin_index: HashMap::new(),
```

**Step 3:** Verify it compiles (no callers changed yet — field is just unused):

```sh
cargo build
```
Expected: clean compile. (If "field never read" warning appears, it is harmless and removed in Phase 2.)

### Task 1.2: Add `ExtractedCall` struct and `find_or_create_builtin_node` helper

**Files:**
- Modify: `src/graph/builder.rs` (add near other helper types, above `impl GraphBuilder` around L45)

**Step 1:** Add the struct (insert before `impl GraphBuilder {` at L45):

```rust
/// A call extracted from a SQL statement (XML-mapper / Java / JSP path).
///
/// Carries builtin metadata so consumers can branch: builtins become
/// `BuiltinFunction` nodes + `UsesBuiltinFunction` edges; everything else
/// follows the existing `CallsProcedure` path.
#[derive(Clone)]
pub(crate) struct ExtractedCall {
    pub callee_name: String,
    pub builtin_meta: Option<ogsql_parser::ast::BuiltinFuncMeta>,
}
```

**Step 2:** Add the helper method inside `impl GraphBuilder` (place it right before `fn create_edges` at L1512):

```rust
/// Find an existing `BuiltinFunction` node by lowercased name, or create a new one.
///
/// Shared across the SQL-proc, XML-mapper, Java, and JSP paths so that the same
/// builtin called from multiple sources collapses to a single node (dedup key:
/// lowercased name, matching `NodeKey::BuiltinFunction`).
fn find_or_create_builtin_node(
    graph: &mut CodeGraph,
    builtin_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
    name: &str,
    meta: &ogsql_parser::ast::BuiltinFuncMeta,
    location: SourceLocation,
) -> petgraph::graph::NodeIndex {
    let name_lower = name.to_lowercase();
    if let Some(&idx) = builtin_index.get(&name_lower) {
        return idx;
    }
    let idx = graph.add_node(Node::BuiltinFunction {
        name: name.to_string(),
        category: meta.category.clone(),
        domain: meta.domain.clone(),
        location,
    });
    builtin_index.insert(name_lower, idx);
    idx
}
```

**Step 3:** Verify compile:

```sh
cargo build
```
Expected: clean compile (helper is unused until later tasks; `#[allow(dead_code)]` not needed since it will be used soon — but if a warning blocks `-D warnings`, defer this task's clippy to the end).

### Task 1.3: Change `extract_calls_from_statements` to return `Vec<ExtractedCall>`

**Files:**
- Modify: `src/graph/builder.rs:2038-2053` (the function body)
- Modify: `src/graph/builder.rs:1788-1789` (XML consumer)
- Modify: `src/graph/builder.rs:1952-1954` (Java consumer)
- Modify: `src/graph/builder.rs:2217-2219` (JSP consumer)

This task changes the return type and **minimally adapts** the 3 consumers so they still compile and behave identically (builtins are still skipped — we enable them in Phases 3-5).

**Step 1:** Rewrite `extract_calls_from_statements` (L2038-2053). Keep the filter for now (builtins still dropped — behavior preserved):

```rust
fn extract_calls_from_statements(
    statements: &[ogsql_parser::StatementInfo],
    file_path: &Arc<PathBuf>,
) -> Vec<ExtractedCall> {
    let mut calls = Vec::new();
    for info in statements {
        let mut extractor = CallExtractor::new(file_path.clone(), HashSet::new());
        walk_statement(&mut extractor, &info.statement);
        for edge in extractor.edges {
            calls.push(ExtractedCall {
                callee_name: edge.callee_name,
                builtin_meta: edge.builtin_meta,
            });
        }
    }
    calls
}
```

> Note: We intentionally **removed** the `if edge.builtin_meta.is_none()` filter here. Builtins will now flow to consumers. Step 2 makes each consumer skip them so behavior is unchanged until Phases 3-5.

**Step 2:** Update each consumer to destructure and skip builtins.

**XML consumer** (L1788-1789), change:
```rust
                    let calls = Self::extract_calls_from_statements(statements, &xml_path);
                    for callee_name in calls {
```
to:
```rust
                    let calls = Self::extract_calls_from_statements(statements, &xml_path);
                    for call in calls {
                        let callee_name = call.callee_name;
                        if call.builtin_meta.is_some() {
                            continue;
                        }
```
(The rest of the XML loop body uses `callee_name` unchanged. Indent the existing loop body one level deeper, or keep flat — match surrounding style.)

**Java consumer** (L1952-1954), change:
```rust
                    let calls =
                        Self::extract_calls_from_statements(&parse_result.statements, &java_path);
                    for callee_name in calls {
```
to:
```rust
                    let calls =
                        Self::extract_calls_from_statements(&parse_result.statements, &java_path);
                    for call in calls {
                        let callee_name = call.callee_name;
                        if call.builtin_meta.is_some() {
                            continue;
                        }
```

**JSP consumer** (L2217-2219), change:
```rust
                    let calls =
                        Self::extract_calls_from_statements(&parse_result.statements, &jsp_path);
                    for callee_name in calls {
```
to:
```rust
                    let calls =
                        Self::extract_calls_from_statements(&parse_result.statements, &jsp_path);
                    for call in calls {
                        let callee_name = call.callee_name;
                        if call.builtin_meta.is_some() {
                            continue;
                        }
```

> Implementation note: because the existing loop bodies reference `callee_name` in closure-like `or_insert_with` blocks, moving `callee_name` out via `let callee_name = call.callee_name;` before the `if` keeps the borrow checker happy (we do not borrow `call` after this move). Do NOT use `&call.callee_name`.

**Step 3:** Verify compile + existing tests:

```sh
cargo build
cargo test --features jsp
```
Expected: clean compile, all tests pass (behavior unchanged — builtins still not modeled for these 3 paths yet).

```sh
cargo clippy --features full -- -D warnings
```
Expected: clean. (If `find_or_create_builtin_node` is flagged dead_code, it will be used in Phase 2.)

---

## Phase 2: Refactor SQL-proc path to shared `builtin_index` (no behavior change)

Move the SQL-proc builtin handling from a local map to the shared `ctx.builtin_index`, and route it through the helper. This is a pure refactor — the existing SQL-proc builtin test must still pass.

### Task 2.1: Thread `builtin_index` through `create_sql_edges` → `create_edges`

**Files:**
- Modify: `src/graph/builder.rs:150-166` (`build_sql_chunk` call site)
- Modify: `src/graph/builder.rs:1076` (`create_sql_edges` signature)
- Modify: `src/graph/builder.rs:1199` (`create_edges` call inside `create_sql_edges`)
- Modify: `src/graph/builder.rs:1512-1527` (`create_edges` signature + remove local `builtin_index`)
- Modify: `src/graph/builder.rs:1555-1580` (use helper)

**Step 1:** `build_sql_chunk` (L160-166) — add `&mut ctx.builtin_index` to the `create_sql_edges` call:

```rust
        Self::create_sql_edges(
            sql_files,
            &mut ctx.graph,
            &mut ctx.proc_index,
            &mut ctx.table_index,
            &ctx.type_index,
            &mut ctx.builtin_index,
        );
```

**Step 2:** `create_sql_edges` signature (L1076) — add `builtin_index` param. Find the signature and add:
```rust
    fn create_sql_edges(
        sql_files: &[ParsedFile],
        graph: &mut CodeGraph,
        proc_index: &mut HashMap<RoutineId, petgraph::graph::NodeIndex>,
        table_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        type_index: &HashMap<String, petgraph::graph::NodeIndex>,
        builtin_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
    ) {
```

**Step 3:** The `create_edges` call at L1199 — pass it through:
```rust
        Self::create_edges(&all_edges, graph, proc_index, builtin_index);
```

**Step 4:** `create_edges` signature (L1512) — add `builtin_index` param and **delete** the local `let mut builtin_index = HashMap::new();` at L1527:
```rust
    fn create_edges(
        edges: &[CallEdge],
        graph: &mut CodeGraph,
        proc_index: &mut HashMap<RoutineId, petgraph::graph::NodeIndex>,
        builtin_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
    ) {
```
Remove line L1527 (`let mut builtin_index: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();`).

**Step 5:** Refactor the builtin branch (L1555-1580) to use the helper. Replace the block:
```rust
            if let Some(meta) = &edge.builtin_meta {
                let builtin_name_lower = edge.callee_name.to_lowercase();
                let builtin_idx = if let Some(&idx) = builtin_index.get(&builtin_name_lower) {
                    idx
                } else {
                    let idx = graph.add_node(Node::BuiltinFunction {
                        name: edge.callee_name.clone(),
                        category: meta.category.clone(),
                        domain: meta.domain.clone(),
                        location: edge.location.clone(),
                    });
                    builtin_index.insert(builtin_name_lower, idx);
                    idx
                };
                if let Some(caller_idx) = caller_idx {
                    graph.add_edge(
                        caller_idx,
                        builtin_idx,
                        Edge::UsesBuiltinFunction {
                            location: edge.location.clone(),
                        },
                    );
                }
                continue;
            }
```
with:
```rust
            if let Some(meta) = &edge.builtin_meta {
                let builtin_idx = Self::find_or_create_builtin_node(
                    graph,
                    builtin_index,
                    &edge.callee_name,
                    meta,
                    edge.location.clone(),
                );
                if let Some(caller_idx) = caller_idx {
                    graph.add_edge(
                        caller_idx,
                        builtin_idx,
                        Edge::UsesBuiltinFunction {
                            location: edge.location.clone(),
                        },
                    );
                }
                continue;
            }
```

**Step 6:** Verify the existing SQL-proc builtin test still passes (this is the refactor safety net):

```sh
cargo test builtin_function -- --nocapture
```
Expected: the existing `builtin_function_captured_as_node` (or similarly named) test PASSES. If it fails, the refactor broke dedup — recheck Step 5.

```sh
cargo build --features full
```
Expected: clean.

**Step 7:** Commit:
```sh
git add src/graph/builder.rs
git commit -m "refactor: share builtin_index across SQL-proc path via GraphBuildContext"
```

---

## Phase 3: XML mapper path (new behavior)

### Task 3.1: Write failing test — mapper SQL with COUNT produces BuiltinFunction node

**Files:**
- Modify: `src/graph/builder.rs` test module (find existing `builtin_function_captured_as_node` test, add new test adjacent)

**Step 1:** Add this test:

```rust
#[test]
fn builtin_function_captured_from_mapper_sql() {
    use crate::parser::ibatis_loader::{IbatisParsedFile, IbatisParsedStatement, IbatisStatementKind};
    use ogsql_parser::StatementInfo;

    // Mapper SQL containing a builtin aggregate: SELECT COUNT(*) FROM orders
    let sql = "SELECT COUNT(*) FROM orders";
    let stmt = ogsql_parser::parse_sql(sql).expect("parse").0;
    let info: StatementInfo = stmt.into_iter().next().expect("one statement");

    let ibatis_file = IbatisParsedFile {
        result: crate::parser::ibatis_loader::IbatisParseResult {
            file_path: Some("mapper/OrderMapper.xml".into()),
            namespace: "com.example.OrderMapper".into(),
            statements: vec![IbatisParsedStatement {
                id: "countOrders".into(),
                kind: IbatisStatementKind::Select,
                flat_sql: sql.into(),
                line: 5,
                parse_result: Some((vec![info], vec![])),
            }],
        },
    };

    let mut ctx = GraphBuilder::new();
    let _ = ctx; // unused, placeholder if needed
    let mut builder_ctx = crate::graph::builder::GraphBuildContext::new();
    GraphBuilder::add_ibatis_nodes_from_parsed(
        std::slice::from_ref(&ibatis_file),
        &mut builder_ctx.graph,
        &mut builder_ctx.proc_index,
        &mut builder_ctx.mapper_index,
        &mut builder_ctx.table_index,
        &mut builder_ctx.builtin_index,
    );

    // Assert a BuiltinFunction node named "count" exists
    let has_count = builder_ctx.graph.node_weights().any(|n| {
        matches!(n, crate::graph::Node::BuiltinFunction { name, .. } if name.eq_ignore_ascii_case("count"))
    });
    assert!(has_count, "expected a BuiltinFunction node for COUNT");

    // Assert a UsesBuiltinFunction edge connects the mapper to the builtin
    let has_edge = builder_ctx.graph.edge_weights().any(|e| {
        matches!(e, crate::graph::Edge::UsesBuiltinFunction { .. })
    });
    assert!(has_edge, "expected a UsesBuiltinFunction edge from the mapper");
}
```

> NOTE: The exact `IbatisParsedFile` / `IbatisParsedStatement` / `IbatisStatementKind` field names and the `add_ibatis_nodes_from_parsed` signature MUST match the current codebase. Before finalizing this test, read `src/parser/ibatis_loader.rs` for the real struct definitions and adjust field names. The signature of `add_ibatis_nodes_from_parsed` will gain a `builtin_index` param in Task 3.2 — so this test only compiles AFTER Task 3.2. Run it then.

**Step 2:** (Run is deferred to after Task 3.2 — it will not compile yet.)

### Task 3.2: Thread `builtin_index` + add builtin branch to XML consumer

**Files:**
- Modify: `src/graph/builder.rs:1727-1742` (`add_ibatis_nodes_from_parsed` wrapper)
- Modify: `src/graph/builder.rs:1745-1752` (`add_ibatis_nodes_from_parsed_with_source_paths` signature)
- Modify: `src/graph/builder.rs:1787-1829` (consumer loop)
- Modify: `src/graph/builder.rs:84-90` (`build_graph_internal` call site)
- Modify: `src/graph/builder.rs:120-126` (`build_all_with_jsp` call site)
- Modify: `src/project/mod.rs:291-298` (production call site)

**Step 1:** `add_ibatis_nodes_from_parsed` (L1727) — add `builtin_index` param and forward it:
```rust
    pub(crate) fn add_ibatis_nodes_from_parsed(
        ibatis_files: &[crate::parser::ibatis_loader::IbatisParsedFile],
        graph: &mut CodeGraph,
        proc_index: &mut HashMap<RoutineId, petgraph::graph::NodeIndex>,
        mapper_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        table_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        builtin_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
    ) {
        Self::add_ibatis_nodes_from_parsed_with_source_paths(
            ibatis_files,
            graph,
            proc_index,
            mapper_index,
            table_index,
            builtin_index,
            &[],
        )
    }
```

**Step 2:** `add_ibatis_nodes_from_parsed_with_source_paths` (L1745) — add `builtin_index` to signature (after `table_index`, before `source_paths`):
```rust
    pub(crate) fn add_ibatis_nodes_from_parsed_with_source_paths(
        ibatis_files: &[crate::parser::ibatis_loader::IbatisParsedFile],
        graph: &mut CodeGraph,
        proc_index: &mut HashMap<RoutineId, petgraph::graph::NodeIndex>,
        mapper_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        table_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        builtin_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        source_paths: &[PathBuf],
    ) {
```

**Step 3:** XML consumer loop (L1788-1829) — replace the `if call.builtin_meta.is_some() { continue; }` placeholder from Task 1.3 with real builtin handling. The loop head becomes:

```rust
                    let calls = Self::extract_calls_from_statements(statements, &xml_path);
                    for call in calls {
                        let callee_name = call.callee_name;
                        if let Some(meta) = &call.builtin_meta {
                            let builtin_idx = Self::find_or_create_builtin_node(
                                graph,
                                builtin_index,
                                &callee_name,
                                meta,
                                SourceLocation {
                                    file: xml_path.clone(),
                                    line: stmt.line,
                                },
                            );
                            graph.add_edge(
                                node_idx,
                                builtin_idx,
                                Edge::UsesBuiltinFunction {
                                    location: SourceLocation {
                                        file: xml_path.clone(),
                                        line: stmt.line,
                                    },
                                },
                            );
                            continue;
                        }
                        // ── existing CallsProcedure logic (uses callee_name) unchanged ──
                        let callee_id =
                            RoutineId::from_qualified_name(&callee_name, RoutineKind::Procedure);
                        // ... rest of existing loop body ...
                    }
```

> IMPORTANT borrow-checker note: `let callee_name = call.callee_name;` moves the name out BEFORE borrowing `call.builtin_meta`. This compiles because the borrow of `call.builtin_meta` happens in the same statement that also reads the already-moved `callee_name` — but Rust evaluates `&callee_name` (a `&String` to the local) and `meta = &call.builtin_meta` where the latter borrows `call`. Since `callee_name` is a separate local (not a borrow of `call`), this is fine. If the compiler complains, change to destructure: `let ExtractedCall { callee_name, builtin_meta } = call;` then `if let Some(meta) = &builtin_meta { ... }`.

**Step 4:** Update the two builder call sites. `build_graph_internal` (L84-90):
```rust
        Self::add_ibatis_nodes_from_parsed(
            ibatis_files,
            &mut ctx.graph,
            &mut ctx.proc_index,
            &mut ctx.mapper_index,
            &mut ctx.table_index,
            &mut ctx.builtin_index,
        );
```
`build_all_with_jsp` (L120-126): same addition.

**Step 5:** Update the production call site in `src/project/mod.rs:291-298`:
```rust
        GraphBuilder::add_ibatis_nodes_from_parsed_with_source_paths(
            &ibatis_files,
            &mut ctx.graph,
            &mut ctx.proc_index,
            &mut ctx.mapper_index,
            &mut ctx.table_index,
            &mut ctx.builtin_index,
            &source_paths,
        );
```

**Step 6:** Compile + run the Task 3.1 test:
```sh
cargo build
cargo test builtin_function_captured_from_mapper_sql -- --nocapture
```
Expected: PASS. If the test's struct construction doesn't compile, fix the fixture to match real `ibatis_loader` types (read `src/parser/ibatis_loader.rs`).

**Step 7:** Commit:
```sh
git add src/graph/builder.rs src/project/mod.rs
git commit -m "feat: model BuiltinFunction nodes from XML mapper SQL"
```

---

## Phase 4: Java path (new behavior)

### Task 4.1: Write failing test — Java @Query with SUBSTR produces BuiltinFunction node

**Files:**
- Modify: `src/graph/builder.rs` test module

**Step 1:** Add this test (adapt struct names to real `java_loader` types — read `src/parser/java_loader.rs` first):

```rust
#[test]
fn builtin_function_captured_from_java_sql() {
    // Java @Query SQL containing a builtin: SELECT SUBSTR(name, 1, 3) FROM users
    // Build a JavaParsedFile with one extraction whose parse_result yields the statement.
    // (See existing Java tests in builder.rs for the exact JavaParsedFile / JavaExtraction shape.)
    //
    // ... construct java_file ...
    //
    // let mut ctx = GraphBuildContext::new();
    // GraphBuilder::add_java_nodes_from_parsed(
    //     std::slice::from_ref(&java_file),
    //     &mut ctx.graph, &mut ctx.proc_index, &ctx.mapper_index,
    //     &mut ctx.table_index, &mut ctx.builtin_index,
    // );
    //
    // assert!(ctx.graph.node_weights().any(|n|
    //     matches!(n, Node::BuiltinFunction { name, .. } if name.eq_ignore_ascii_case("substr"))));
    // assert!(ctx.graph.edge_weights().any(|e|
    //     matches!(e, Edge::UsesBuiltinFunction { .. })));
    todo!("fill in fixture once JavaParsedFile shape confirmed");
}
```

> NOTE: Locate an existing Java builder test in `builder.rs` (search `add_java_nodes_from_parsed` in the test module) and copy its fixture-construction pattern exactly, then swap the SQL to `SELECT SUBSTR(name, 1, 3) FROM users` and add the assertions above. This guarantees the fixture matches real types.

### Task 4.2: Thread `builtin_index` + add builtin branch to Java consumer

**Files:**
- Modify: `src/graph/builder.rs:1890-1905` (`add_java_nodes_from_parsed` wrapper)
- Modify: `src/graph/builder.rs:1909-1916` (`add_java_nodes_from_parsed_with_source_paths` signature)
- Modify: `src/graph/builder.rs:1952-2005` (consumer loop)
- Modify: `src/graph/builder.rs:91-97` (`build_graph_internal` call site)
- Modify: `src/graph/builder.rs:127-133` (`build_all_with_jsp` call site)
- Modify: `src/project/mod.rs:299-306` (production call site)

**Step 1:** `add_java_nodes_from_parsed` (L1890) — add `builtin_index` param + forward (mirror Task 3.2 Step 1):
```rust
    pub(crate) fn add_java_nodes_from_parsed(
        java_files: &[crate::parser::java_loader::JavaParsedFile],
        graph: &mut CodeGraph,
        proc_index: &mut HashMap<RoutineId, petgraph::graph::NodeIndex>,
        mapper_index: &HashMap<String, petgraph::graph::NodeIndex>,
        table_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        builtin_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
    ) {
        Self::add_java_nodes_from_parsed_with_source_paths(
            java_files, graph, proc_index, mapper_index, table_index, builtin_index, &[],
        )
    }
```

**Step 2:** `add_java_nodes_from_parsed_with_source_paths` (L1909) — add `builtin_index` to signature (after `table_index`, before `source_paths`).

**Step 3:** Java consumer loop (L1952-2005) — replace the `if call.builtin_meta.is_some() { continue; }` placeholder with real handling (mirror Task 3.2 Step 3, but use `java_path` and `extraction.origin.line` for locations):
```rust
                    let calls =
                        Self::extract_calls_from_statements(&parse_result.statements, &java_path);
                    for call in calls {
                        let callee_name = call.callee_name;
                        if let Some(meta) = &call.builtin_meta {
                            let builtin_idx = Self::find_or_create_builtin_node(
                                graph,
                                builtin_index,
                                &callee_name,
                                meta,
                                SourceLocation {
                                    file: java_path.clone(),
                                    line: extraction.origin.line,
                                },
                            );
                            graph.add_edge(
                                node_idx,
                                builtin_idx,
                                Edge::UsesBuiltinFunction {
                                    location: SourceLocation {
                                        file: java_path.clone(),
                                        line: extraction.origin.line,
                                    },
                                },
                            );
                            continue;
                        }
                        // ── existing CallsProcedure logic unchanged ──
                        ...
                    }
```

**Step 4:** Update builder call sites (`build_graph_internal` L91-97 and `build_all_with_jsp` L127-133) and the production call site in `src/project/mod.rs:299-306` — add `&mut ctx.builtin_index`.

**Step 5:** Compile + run test:
```sh
cargo build
cargo test builtin_function_captured_from_java_sql -- --nocapture
```
Expected: PASS.

**Step 6:** Commit:
```sh
git add src/graph/builder.rs src/project/mod.rs
git commit -m "feat: model BuiltinFunction nodes from Java @Query/JDBC SQL"
```

---

## Phase 5: JSP path (new behavior)

### Task 5.1: Write failing test — JSP SQL with COUNT produces BuiltinFunction node

**Files:**
- Modify: `src/graph/builder.rs` test module (under `#[cfg(test)]`, and gate the test with `#[cfg(feature = "jsp")]`)

**Step 1:** Add the test, mirroring an existing JSP builder test (search `add_jsp_nodes_from_parsed` in the test module for the `JspFileResult` fixture shape). SQL: `SELECT COUNT(*) FROM products`. Assert a `BuiltinFunction` node + `UsesBuiltinFunction` edge exist. Gate with `#[cfg(feature = "jsp")]`.

### Task 5.2: Add builtin branch to JSP consumer

**Files:**
- Modify: `src/graph/builder.rs:2217-2264` (JSP consumer loop)

> NOTE: The JSP consumer already receives `ctx: &mut GraphBuildContext` (L2181), so `ctx.builtin_index` is already available — **no signature change or call-site changes needed.** Only the loop body changes.

**Step 1:** JSP consumer loop (L2217-2264) — replace the `if call.builtin_meta.is_some() { continue; }` placeholder with real handling (mirror Task 3.2/4.2, using `jsp_path` and `extraction.origin.line`, and `ctx.graph` / `ctx.builtin_index`):
```rust
                    let calls =
                        Self::extract_calls_from_statements(&parse_result.statements, &jsp_path);
                    for call in calls {
                        let callee_name = call.callee_name;
                        if let Some(meta) = &call.builtin_meta {
                            let builtin_idx = Self::find_or_create_builtin_node(
                                &mut ctx.graph,
                                &mut ctx.builtin_index,
                                &callee_name,
                                meta,
                                SourceLocation {
                                    file: jsp_path.clone(),
                                    line: extraction.origin.line,
                                },
                            );
                            ctx.graph.add_edge(
                                sql_idx,
                                builtin_idx,
                                Edge::UsesBuiltinFunction {
                                    location: SourceLocation {
                                        file: jsp_path.clone(),
                                        line: extraction.origin.line,
                                    },
                                },
                            );
                            continue;
                        }
                        // ── existing CallsProcedure logic unchanged ──
                        ...
                    }
```

**Step 2:** Compile + run test (requires `jsp` feature):
```sh
cargo build --features jsp
cargo test --features jsp builtin_function_captured_from_jsp_sql -- --nocapture
```
Expected: PASS.

**Step 3:** Commit:
```sh
git add src/graph/builder.rs
git commit -m "feat: model BuiltinFunction nodes from JSP scriptlet SQL"
```

---

## Phase 6: Cross-path dedup + full verification

### Task 6.1: Write cross-path dedup test

**Files:**
- Modify: `src/graph/builder.rs` test module

**Step 1:** Add a test that builds a graph where a SQL stored procedure AND an XML mapper both call the same builtin (e.g. `COUNT`). Assert exactly ONE `BuiltinFunction` node for that name exists, with TWO `UsesBuiltinFunction` edges (one from the proc, one from the mapper):

```rust
#[test]
fn builtin_node_cross_path_dedup() {
    // 1. SQL proc: CREATE PROCEDURE use_count() IS BEGIN ... COUNT(*) ... END;
    // 2. XML mapper: SELECT COUNT(*) FROM ...
    // Build via build_sql_chunk + add_ibatis_nodes_from_parsed (shared ctx.builtin_index).
    //
    // let count_nodes = ctx.graph.node_weights()
    //     .filter(|n| matches!(n, Node::BuiltinFunction { name, .. } if name.eq_ignore_ascii_case("count")))
    //     .count();
    // assert_eq!(count_nodes, 1, "cross-path dedup: one COUNT node");
    //
    // let builtin_edges = ctx.graph.edge_weights()
    //     .filter(|e| matches!(e, Edge::UsesBuiltinFunction { .. }))
    //     .count();
    // assert_eq!(builtin_edges, 2, "two callers → two edges");
    todo!("fill in both fixtures");
}
```

**Step 2:** Run:
```sh
cargo test builtin_node_cross_path_dedup -- --nocapture
```
Expected: PASS (proves the shared `builtin_index` collapses duplicates).

### Task 6.2: Full verification matrix (AGENTS.md Definition of Done)

Run ALL of these. Every one must be clean. Document any pre-existing failure unrelated to this change.

```sh
# Compilation under every feature combination
cargo build
cargo build --features serve
cargo build --features jsp
cargo build --features full

# Tests
cargo test
cargo test --features jsp
cargo test --features full

# Lint (CI-gating level)
cargo clippy -- -D warnings
cargo clippy --features full -- -D warnings

# Format
cargo fmt -- --check
```

If `cargo fmt -- --check` fails, run `cargo fmt` and re-check.

### Task 6.3: Final commit + verify no behavior regression

```sh
git add -A
git commit -m "test: cross-path builtin dedup + full verification"
```

Sanity-check that builtin functions are still **hidden by default** in `detail`/`trace` output (the Phase 3 presentation filter from the prior plan applies automatically to new edges — no UI work needed):

```sh
# Should NOT show builtins unless --builtfunc
cargo run -- detail "some_proc" 2>/dev/null | head
```

---

## File Change Summary

| File | Phase | Change |
|---|---|---|
| `src/graph/builder.rs` | 1-6 | `GraphBuildContext` field + `ExtractedCall` + helper + `extract_calls_from_statements` return type + 4 consumer loop branches + call-site threading |
| `src/project/mod.rs` | 3,4 | 2 call sites gain `&mut ctx.builtin_index` arg |

**Total: 2 files.** No new enum variants, no new node/edge types, no presentation-layer changes (all inherited from the prior builtin-tracking plan). This is pure plumbing to extend an existing capability to 3 more extraction paths.

---

## Risk Notes

1. **Borrow checker in consumer loops** — moving `call.callee_name` out before borrowing `call.builtin_meta`. If problematic, destructure: `let ExtractedCall { callee_name, builtin_meta } = call;`. Documented in Task 3.2.
2. **Test fixtures** — the `IbatisParsedFile` / `JavaParsedFile` / `JspFileResult` struct construction in tests must match real field names. READ `src/parser/ibatis_loader.rs`, `java_loader.rs`, `jsp_loader.rs` before finalizing each test. Copy from existing builder tests.
3. **`extract_calls_from_statements` is `pub(crate)`?** — verify visibility. If private, the signature change is internal-only. If used elsewhere, update those callers too (grep `extract_calls_from_statements`).
4. **Store serialization** — no Node/Edge enum changes, so `GraphStore` bincode format is unchanged. **No breaking store change.** (Unlike the prior builtin-tracking plan.)
5. **JSP feature gate** — the JSP test and loop branch are only compiled under `--features jsp`. XML/Java changes compile under default features.
6. **`ogsql-parser` version** — issue states v0.8.11 provides statement-path builtin metadata. Verify `Cargo.toml` pins ≥ 0.8.11 (or the git rev that includes it). If older, builtins will have `builtin_meta: None` and nothing is modeled — silently. Add a `cargo tree` check in Task 1.1.

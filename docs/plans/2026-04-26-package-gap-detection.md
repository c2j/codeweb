# Package Spec-vs-Body Gap Detection Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Detect and report package procedures/functions declared in spec but missing from body due to parse failures, creating placeholder nodes for them.

**Architecture:** During graph build, collect spec-declared routine names (`CreatePackage.items`) and body-implemented routine names (`CreatePackageBody.items`). Compute the difference. For each missing routine, create a placeholder `Node::Procedure`/`Node::Function` with a `partial: true` flag, and emit parse_log warnings. No changes to store format needed — `partial` is a field on the existing Node variants.

**Tech Stack:** Rust, ogsql-parser AST (PackageItem, CreatePackageStatement, CreatePackageBodyStatement)

---

### Task 1: Add `partial` field to Node::Procedure and Node::Function

**Files:**
- Modify: `src/graph/mod.rs` (Node enum)
- Test: `src/graph/builder.rs` (inline tests)

**Step 1: Write the failing test**

Add a test in `src/graph/builder.rs` at the end of the test module:

```rust
#[test]
fn partial_flag_on_procedure_node() {
    let node = Node::Procedure {
        id: RoutineId::from_qualified_name("pkg.proc", RoutineKind::Procedure),
        location: SourceLocation {
            file: PathBuf::from("test.sql"),
            line: 1,
        },
        partial: true,
    };
    if let Node::Procedure { partial, .. } = node {
        assert!(partial, "partial flag should be true");
    } else {
        panic!("Expected Procedure variant");
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test partial_flag_on_procedure_node`
Expected: FAIL — `partial` field does not exist on `Node::Procedure`

**Step 3: Add `partial` field to Node variants**

In `src/graph/mod.rs`, modify `Node::Procedure` and `Node::Function`:

```rust
Procedure {
    id: RoutineId,
    location: SourceLocation,
    /// True if this node was created from a spec declaration but the body
    /// implementation could not be parsed (partial/placeholder node).
    #[serde(default)]
    partial: bool,
},
Function {
    id: RoutineId,
    location: SourceLocation,
    /// True if this node was created from a spec declaration but the body
    /// implementation could not be parsed (partial/placeholder node).
    #[serde(default)]
    partial: bool,
},
```

Update ALL existing `Node::Procedure { ... }` and `Node::Function { ... }` constructions to include `partial: false`. Search the entire codebase for these patterns.

**Step 4: Run test to verify it passes**

Run: `cargo test partial_flag_on_procedure_node`
Expected: PASS

**Step 5: Run full test suite + clippy**

Run: `cargo test && cargo clippy -- -D warnings`
Expected: All 67 tests pass, clippy clean

**Step 6: Commit**

```bash
git add src/graph/mod.rs src/graph/builder.rs src/export/ src/main.rs src/tui/ tests/
git commit -m "feat: add partial flag to Node::Procedure and Node::Function for gap detection"
```

---

### Task 2: Implement gap detection in builder

**Files:**
- Modify: `src/graph/builder.rs` (new function `detect_and_create_partial_nodes`)
- Test: `src/graph/builder.rs` (inline tests)

**Step 1: Write the failing test**

```rust
#[test]
fn gap_detection_creates_partial_nodes_for_missing_body_items() {
    use ogsql_parser::ast::{CreatePackageBodyStatement, CreatePackageStatement, PackageItem, PackageProcedure};

    // Package spec declares 2 procedures
    let spec = StatementInfo {
        sql_text: String::new(),
        start_line: 1, start_col: 0, end_line: 5, end_col: 0,
        statement: Statement::CreatePackage(CreatePackageStatement {
            replace: true,
            name: vec!["pkg_test".into()],
            authid: None,
            items: vec![
                PackageItem::Procedure(PackageProcedure {
                    name: vec!["prc_found".into()],
                    parameters: vec![],
                    block: None,
                    start_line: 2, end_line: 2,
                }),
                PackageItem::Procedure(PackageProcedure {
                    name: vec!["prc_missing".into()],
                    parameters: vec![],
                    block: None,
                    start_line: 3, end_line: 3,
                }),
            ],
        }),
    };

    // Package body only implements 1 procedure (the other was lost to parse errors)
    let body = StatementInfo {
        sql_text: String::new(),
        start_line: 7, start_col: 0, end_line: 20, end_col: 0,
        statement: Statement::CreatePackageBody(CreatePackageBodyStatement {
            replace: true,
            name: vec!["pkg_test".into()],
            items: vec![
                PackageItem::Procedure(PackageProcedure {
                    name: vec!["prc_found".into()],
                    parameters: vec![],
                    block: Some(ogsql_parser::ast::plpgsql::PlBlock::default()),
                    start_line: 8, end_line: 18,
                }),
                // prc_missing is missing — would be PackageItem::Raw in real parse
            ],
        }),
    };

    let files = vec![ParsedFile {
        path: PathBuf::from("test.sql"),
        statements: vec![spec, body],
    }];

    let graph = GraphBuilder::new().build(&files);

    // Should have 3 nodes: package + prc_found + prc_missing(partial)
    let procs: Vec<_> = graph.node_indices()
        .filter(|i| matches!(&graph[*i], Node::Procedure { .. }))
        .collect();
    assert_eq!(procs.len(), 2, "Expected 2 procedure nodes");

    // Check prc_found is NOT partial
    let found_node = procs.iter().find(|i| {
        if let Node::Procedure { id, .. } = &graph[**i] {
            id.name == "prc_found"
        } else { false }
    }).expect("prc_found should exist");
    if let Node::Procedure { partial, .. } = &graph[*found_node] {
        assert!(!partial, "prc_found should NOT be partial");
    }

    // Check prc_missing IS partial
    let missing_node = procs.iter().find(|i| {
        if let Node::Procedure { id, .. } = &graph[**i] {
            id.name == "prc_missing"
        } else { false }
    }).expect("prc_missing should exist");
    if let Node::Procedure { partial, .. } = &graph[*missing_node] {
        assert!(partial, "prc_missing SHOULD be partial");
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test gap_detection_creates_partial_nodes`
Expected: FAIL — gap detection not yet implemented

**Step 3: Implement `detect_and_create_partial_nodes`**

In `src/graph/builder.rs`, add a new method to `GraphBuilder`:

```rust
/// Compare package spec declarations against body implementations.
/// Create placeholder (partial) nodes for routines declared in spec but
/// missing from body (due to parse failures).
fn detect_and_create_partial_nodes(
    files: &[ParsedFile],
    graph: &mut CodeGraph,
    proc_index: &mut HashMap<RoutineId, petgraph::graph::NodeIndex>,
    package_index: &HashMap<String, petgraph::graph::NodeIndex>,
) {
    use ogsql_parser::ast::{Statement, PackageItem};

    for file in files {
        // Collect spec declarations: (package_name, routine_name, kind)
        let mut spec_decls: Vec<(String, String, RoutineKind)> = Vec::new();
        let mut body_impls: Vec<(String, String)> = Vec::new();

        for info in &file.statements {
            match &info.statement {
                Statement::CreatePackage(pkg) => {
                    let pkg_name = pkg.name.last().cloned().unwrap_or_default();
                    for item in &pkg.items {
                        let (name, kind) = match item {
                            PackageItem::Procedure(p) => (p.name.join("."), RoutineKind::Procedure),
                            PackageItem::Function(f) => (f.name.join("."), RoutineKind::Function),
                            PackageItem::Raw(_) => continue,
                        };
                        spec_decls.push((pkg_name.clone(), name, kind));
                    }
                }
                Statement::CreatePackageBody(pkg) => {
                    let pkg_name = pkg.name.last().cloned().unwrap_or_default();
                    for item in &pkg.items {
                        let name = match item {
                            PackageItem::Procedure(p) => p.name.join("."),
                            PackageItem::Function(f) => f.name.join("."),
                            PackageItem::Raw(_) => continue,
                        };
                        body_impls.push((pkg_name.clone(), name));
                    }
                }
                _ => {}
            }
        }

        // Find spec items missing from body
        for (pkg_name, routine_name, kind) in &spec_decls {
            let found_in_body = body_impls.iter().any(|(pn, rn)| {
                pn == pkg_name && rn == routine_name
            });
            if !found_in_body {
                let routine_id = RoutineId {
                    schema: None,
                    package: Some(pkg_name.clone()),
                    name: routine_name.clone(),
                    kind: *kind,
                };
                // Only create if not already in index (avoid duplicates)
                if !proc_index.contains_key(&routine_id) {
                    let file_str = file.path.to_string_lossy().to_string();
                    crate::parse_log::warn(
                        &file_str,
                        &format!(
                            "package '{}' spec declares '{}' but body implementation could not be parsed (creating partial node)",
                            pkg_name, routine_name
                        ),
                    );
                    let node = match kind {
                        RoutineKind::Procedure => Node::Procedure {
                            id: routine_id.clone(),
                            location: SourceLocation {
                                file: file.path.clone(),
                                line: 0, // Unknown — from spec, not body
                            },
                            partial: true,
                        },
                        RoutineKind::Function => Node::Function {
                            id: routine_id.clone(),
                            location: SourceLocation {
                                file: file.path.clone(),
                                line: 0,
                            },
                            partial: true,
                        },
                    };
                    let idx = graph.add_node(node);
                    proc_index.insert(routine_id.clone(), idx);

                    // Link to package node
                    let qualified = pkg_name.clone();
                    if let Some(&pkg_idx) = package_index.get(&qualified) {
                        graph.add_edge(pkg_idx, idx, Edge::ContainsRoutine);
                    }
                }
            }
        }
    }
}
```

**Step 4: Wire into build() and build_store()**

In `build()`, add after `create_procedure_nodes`:

```rust
Self::detect_and_create_partial_nodes(files, &mut graph, &mut proc_index, &package_index);
```

In `build_store()`, add after `create_procedure_nodes`:

```rust
Self::detect_and_create_partial_nodes(&all.sql_files, &mut graph, &mut proc_index, &package_index);
```

**Step 5: Run test to verify it passes**

Run: `cargo test gap_detection_creates_partial_nodes`
Expected: PASS

**Step 6: Run full suite**

Run: `cargo test && cargo clippy -- -D warnings`
Expected: All tests pass

**Step 7: Commit**

```bash
git add src/graph/builder.rs
git commit -m "feat: gap detection — create partial nodes for spec-declared routines missing from body"
```

---

### Task 3: Update CLI display for partial nodes

**Files:**
- Modify: `src/main.rs` (cmd_detail, cmd_nodes, print_stats)
- Modify: `src/export/dot.rs` (partial node styling)
- Modify: `src/export/mermaid.rs` (partial node styling)
- Test: `tests/integration_test.rs`

**Step 1: Write the failing test**

Add integration test:

```rust
#[test]
fn test_partial_nodes_in_json_export() {
    // Build graph with a package that has spec-only procedure
    let sql = r#"
create or replace package pkg_gap is
  PROCEDURE prc_exists(p1 in varchar2);
  PROCEDURE prc_lost(p1 in varchar2);
end pkg_gap;
/
create or replace package body pkg_gap is
  PROCEDURE prc_exists(p1 in varchar2) is
  begin
    insert into t_orders(id) values(1);
  END;
end pkg_gap;
/
"#;
    let graph = parse_and_build(sql);
    let json = export_json(&graph);

    // Should have prc_exists (not partial) and prc_lost (partial)
    assert!(json.contains("prc_exists"), "prc_exists should be in export");
    assert!(json.contains("prc_lost"), "prc_lost (partial) should be in export");
    assert!(json.contains("partial"), "partial field should appear in JSON");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_partial_nodes_in_json_export`
Expected: FAIL — partial field not yet in JSON export

**Step 3: Update JSON export**

In `src/export/json.rs`, update the `Node::Procedure` and `Node::Function` match arms to include the `partial` field:

```rust
Node::Procedure { id, location, partial } => {
    serde_json::json!({
        "id": node_id,
        "type": "procedure",
        "schema": id.schema,
        "package": id.package,
        "name": id.name,
        "file": location.file,
        "line": location.line,
        "partial": partial,
    })
}
Node::Function { id, location, partial } => {
    serde_json::json!({
        "id": node_id,
        "type": "function",
        "schema": id.schema,
        "package": id.package,
        "name": id.name,
        "file": location.file,
        "line": location.line,
        "partial": partial,
    })
}
```

**Step 4: Update DOT export**

In `src/export/dot.rs`, style partial nodes with dashed borders:

```rust
Node::Procedure { id, partial, .. } => {
    let label = id.display_short();
    let shape = if *partial { "shape=box,style=\"dashed\"" } else { "shape=box" };
    writeln!(f, "  {} [label=\"proc:{}\" {}];", dot_id, label, shape)
}
Node::Function { id, partial, .. } => {
    let label = id.display_short();
    let shape = if *partial { "shape=ellipse,style=\"dashed\"" } else { "shape=ellipse" };
    writeln!(f, "  {} [label=\"func:{}\" {}];", dot_id, label, shape)
}
```

**Step 5: Update Mermaid export**

In `src/export/mermaid.rs`, style partial nodes with `---` (dashed) syntax.

**Step 6: Update cmd_detail to show partial status**

In `cmd_detail`, when displaying a procedure/function, show `[partial]` tag:

```rust
Node::Procedure { id, partial, .. } => {
    let tag = if *partial { "proc [partial]" } else { "proc" };
    println!("  {} {}:{}", tag, ...);
}
```

**Step 7: Update print_stats**

Add `partial` count to stats output:

```rust
Node::Procedure { partial: true, .. } => partial_procs += 1,
Node::Function { partial: true, .. } => partial_funcs += 1,
```

At end: `if partial_procs + partial_funcs > 0 { eprintln!("  ⚠ {} partial nodes (unparsed body)", partial_procs + partial_funcs); }`

**Step 8: Run all tests**

Run: `cargo test && cargo clippy -- -D warnings`
Expected: All pass

**Step 9: Commit**

```bash
git add src/export/ src/main.rs tests/
git commit -m "feat: display partial nodes in CLI, JSON, DOT, Mermaid exports"
```

---

### Task 4: Verify with demo data

**Files:** None (verification only)

**Step 1: Re-run demo with gap detection**

```bash
rm -f codeweb.toml && rm -rf .codeweb
./target/debug/codeweb init demo --dir lib/codeweb-complex-demo/sql/
```

**Step 2: Verify partial node exists**

```bash
./target/debug/codeweb detail prc_acnt_info_exp
```

Expected: Should now find `prc_acnt_info_exp` as a partial node with `[partial]` tag.

**Step 3: Verify parse.log shows warning**

```bash
cat .codeweb/parse.log | grep prc_acnt_info_exp
```

Expected: Warning about spec declaring `prc_acnt_info_exp` but body could not be parsed.

**Step 4: Verify full export**

```bash
./target/debug/codeweb export --format json | python3 -m json.tool | grep -A5 prc_acnt_info_exp
```

Expected: Node with `"partial": true`.

---

### Task 5: Clean up and final verification

**Step 1: Run full test suite**

```bash
cargo test && cargo clippy -- -D warnings && cargo fmt -- --check
```

**Step 2: Verify all new test counts**

Note the new test count. Should be 67 + ~4 new tests.

**Step 3: Final commit if any cleanup needed**

# Operator & Hint Usage Tracking Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make SQL operators (ANY, ALL, SOME, EXISTS, IN) and GaussDB optimizer hints (tablescan, hashjoin, etc.) first-class nodes in the code graph — queryable by name with reverse-reference lookup — by reusing the existing `Node::BuiltinFunction` infrastructure with category-based distinction.

**Architecture:** Zero new enum variants. Operator and hint usage is extracted at the ogsql-parser Visitor level, pushed as `CallEdge` records with synthetic `BuiltinFuncMeta` (category="Operator" or "Hint"), and flows through the existing `create_edges` / consumer-loop builtin branches to create `BuiltinFunction` nodes + `UsesBuiltinFunction` edges. All 6 query entry points (CLI detail/trace/nodes, HTTP API, MCP, QuerySpec, impact) auto-inherit.

**Tech Stack:** Rust, ogsql-parser v0.8.13 (provides `SelectStatement.hints: Vec<String>`, `Expr::ScalarSublink`, `Expr::Exists`, `Expr::InSubquery`), petgraph.

---

## Background Context

### What already works (do NOT change)
- `Node::BuiltinFunction { name, category, domain, location }` — first-class node type, tag "builtin"
- `Edge::UsesBuiltinFunction { location }` — edge type in `EdgeCategory::Call`
- `CallExtractor::push_builtin_call(name, meta, line)` — creates a `CallEdge` with `builtin_meta: Some(meta)`
- `GraphBuilder::find_or_create_builtin_node()` — dedup by lowercased name via `builtin_index`
- `GraphBuilder::create_edges()` and 4 consumer loops (SQL-proc, XML, Java, JSP) — all branch on `builtin_meta.is_some()` to create BuiltinFunction nodes
- Presentation layer: `skip_builtins` filter in `traverse.rs`, `--builtfunc` flag on `detail`/`trace`

### What this plan adds
- Two new extraction sources in `CallExtractor`:
  1. **Operators**: `visit_expr` matches `Expr::ScalarSublink`, `Expr::Exists`, `Expr::InSubquery`
  2. **Hints**: `visit_select` reads `select.hints: Vec<String>`

### Key data structures (already exist)
```rust
// ogsql-parser ast/mod.rs — ScalarSublink
pub enum ScalarSublinkType { Any, Some, All }
pub enum Expr {
    ScalarSublink { expr, op: String, sublink_type: ScalarSublinkType, subquery },
    Exists(Box<SelectStatement>),
    InSubquery { expr, subquery, negated: bool },
    Subquery(Box<SelectStatement>),  // ← NOT tracked (bare scalar subquery)
}

// ogsql-parser ast/mod.rs — SelectStatement.hints
pub struct SelectStatement {
    pub hints: Vec<String>,  // hint names (lowercased), e.g. ["tablescan", "hashjoin"]
    ...
}
```

### Design decisions (locked)
- **Reuse BuiltinFunction, no new enum variants** — category="Operator" / "Hint" distinguishes from existing builtins
- **ANY and SOME kept separate** — each tracks as its own BuiltinFunction node ("ANY" / "SOME")
- **NOT IN tracked as "NOT_IN"** — `Expr::InSubquery { negated: true }` → node name "NOT_IN"
- **Hint names kept lowercase** — matches existing `SelectStatement.hints` convention
- **Hints dedup within a single SELECT** — hint names naturally dedup via `builtin_index`
- **Location** — operator location uses `self.make_location(line)` with line from AST (available as `self.current_line` from the visitor context); for hints, use statement-level location

---

## Phase 1: Operator Extraction (ANY, ALL, SOME, EXISTS, IN)

### Task 1.1: Add operator detection to `visit_expr`

**Files:**
- Modify: `src/parser/extractor.rs:642-663` (`visit_expr` method)

**Step 1:** Replace the current `visit_expr` with operator-aware version.

Current code (L642-663):
```rust
fn visit_expr(&mut self, expr: &Expr) -> VisitorResult {
    if let Expr::FunctionCall { name, builtin, .. } = expr {
        if name.is_empty() {
            return VisitorResult::Continue;
        }
        let first = name[0].to_lowercase();
        if self.local_vars.contains(&first) || self.known_types.contains(&first) {
            return VisitorResult::Continue;
        }
        match builtin {
            None => {
                self.push_call(&name.join("."), false, 0);
            }
            Some(meta) => {
                self.push_builtin_call(&name.join("."), meta.clone(), 0);
            }
        }
    }
    VisitorResult::Continue
}
```

Replace with:
```rust
fn visit_expr(&mut self, expr: &Expr) -> VisitorResult {
    // ── Operator detection (ANY / ALL / SOME / EXISTS / IN / NOT_IN) ──
    match expr {
        Expr::ScalarSublink { sublink_type, .. } => {
            let name = match sublink_type {
                ogsql_parser::ast::ScalarSublinkType::Any => "ANY",
                ogsql_parser::ast::ScalarSublinkType::Some => "SOME",
                ogsql_parser::ast::ScalarSublinkType::All => "ALL",
            };
            self.push_builtin_call(
                name,
                ogsql_parser::ast::BuiltinFuncMeta {
                    category: "Operator".into(),
                    domain: "Comparison".into(),
                },
                0,
            );
            // Continue traversal so the subquery's internal CALLs are also extracted
            return VisitorResult::Continue;
        }
        Expr::Exists(_) => {
            self.push_builtin_call(
                "EXISTS",
                ogsql_parser::ast::BuiltinFuncMeta {
                    category: "Operator".into(),
                    domain: "Predicate".into(),
                },
                0,
            );
            return VisitorResult::Continue;
        }
        Expr::InSubquery { negated, .. } => {
            let name = if *negated { "NOT_IN" } else { "IN" };
            self.push_builtin_call(
                name,
                ogsql_parser::ast::BuiltinFuncMeta {
                    category: "Operator".into(),
                    domain: "Predicate".into(),
                },
                0,
            );
            return VisitorResult::Continue;
        }
        _ => {}
    }

    // ── Existing FunctionCall handling ──
    if let Expr::FunctionCall { name, builtin, .. } = expr {
        if name.is_empty() {
            return VisitorResult::Continue;
        }
        let first = name[0].to_lowercase();
        if self.local_vars.contains(&first) || self.known_types.contains(&first) {
            return VisitorResult::Continue;
        }
        match builtin {
            None => {
                self.push_call(&name.join("."), false, 0);
            }
            Some(meta) => {
                self.push_builtin_call(&name.join("."), meta.clone(), 0);
            }
        }
    }
    VisitorResult::Continue
}
```

> **Important**: The `Expr::Subquery` variant (bare scalar subquery w/o ANY/ALL/EXISTS) is intentionally **not** tracked. It's a value-producing expression, not a predicate/operator.

**Step 2:** Compile to verify:
```sh
cargo build
```
Expected: clean compile. `Expr::ScalarSublink` and `ScalarSublinkType` are re-exported by ogsql-parser.

### Task 1.2: Write and run operator extraction test

**Files:**
- Modify: `src/parser/extractor.rs` test module

**Step 1:** Add test at the end of the existing `#[cfg(test)] mod tests { ... }`:

```rust
#[test]
fn operator_any_extracted_as_builtin() {
    let sql = "CREATE OR REPLACE PROCEDURE test_any() AS $$
    BEGIN
        FOR r IN (SELECT * FROM t1 WHERE col > ANY(SELECT col FROM t2)) LOOP
            NULL;
        END LOOP;
    END;
    $$ LANGUAGE plpgsql;";
    let stmts = ogsql_parser::Parser::parse_sql(sql).0;
    let mut extractor = CallExtractor::new(
        std::sync::Arc::new(std::path::PathBuf::from("test.sql")),
        std::collections::HashSet::new(),
    );
    ogsql_parser::walk_statement(&mut extractor, &stmts[0].statement);

    let any_edges: Vec<_> = extractor
        .edges
        .iter()
        .filter(|e| e.builtin_meta.is_some() && e.callee_name == "ANY")
        .collect();
    assert!(!any_edges.is_empty(), "expected ANY operator to be extracted");
    assert_eq!(any_edges[0].builtin_meta.as_ref().unwrap().category, "Operator");
    assert_eq!(any_edges[0].builtin_meta.as_ref().unwrap().domain, "Comparison");
}

#[test]
fn operator_exists_extracted_as_builtin() {
    let sql = "CREATE OR REPLACE PROCEDURE test_exists() AS $$
    BEGIN
        FOR r IN (SELECT * FROM t1 WHERE EXISTS(SELECT 1 FROM t2 WHERE t2.id = t1.id)) LOOP
            NULL;
        END LOOP;
    END;
    $$ LANGUAGE plpgsql;";
    let stmts = ogsql_parser::Parser::parse_sql(sql).0;
    let mut extractor = CallExtractor::new(
        std::sync::Arc::new(std::path::PathBuf::from("test.sql")),
        std::collections::HashSet::new(),
    );
    ogsql_parser::walk_statement(&mut extractor, &stmts[0].statement);

    let exists_edges: Vec<_> = extractor
        .edges
        .iter()
        .filter(|e| e.builtin_meta.is_some() && e.callee_name == "EXISTS")
        .collect();
    assert!(!exists_edges.is_empty(), "expected EXISTS operator to be extracted");
    assert_eq!(exists_edges[0].builtin_meta.as_ref().unwrap().category, "Operator");
    assert_eq!(exists_edges[0].builtin_meta.as_ref().unwrap().domain, "Predicate");
}

#[test]
fn operator_in_subquery_extracted_as_builtin() {
    let sql = "CREATE OR REPLACE PROCEDURE test_in() AS $$
    BEGIN
        FOR r IN (SELECT * FROM t1 WHERE col IN (SELECT col FROM t2)) LOOP
            NULL;
        END LOOP;
    END;
    $$ LANGUAGE plpgsql;";
    let stmts = ogsql_parser::Parser::parse_sql(sql).0;
    let mut extractor = CallExtractor::new(
        std::sync::Arc::new(std::path::PathBuf::from("test.sql")),
        std::collections::HashSet::new(),
    );
    ogsql_parser::walk_statement(&mut extractor, &stmts[0].statement);

    let in_edges: Vec<_> = extractor
        .edges
        .iter()
        .filter(|e| e.builtin_meta.is_some() && e.callee_name == "IN")
        .collect();
    assert!(!in_edges.is_empty(), "expected IN operator to be extracted");
}

#[test]
fn operator_all_extracted_as_builtin() {
    let sql = "CREATE OR REPLACE PROCEDURE test_all() AS $$
    BEGIN
        FOR r IN (SELECT * FROM t1 WHERE col > ALL(SELECT col FROM t2)) LOOP
            NULL;
        END LOOP;
    END;
    $$ LANGUAGE plpgsql;";
    let stmts = ogsql_parser::Parser::parse_sql(sql).0;
    let mut extractor = CallExtractor::new(
        std::sync::Arc::new(std::path::PathBuf::from("test.sql")),
        std::collections::HashSet::new(),
    );
    ogsql_parser::walk_statement(&mut extractor, &stmts[0].statement);

    let all_edges: Vec<_> = extractor
        .edges
        .iter()
        .filter(|e| e.builtin_meta.is_some() && e.callee_name == "ALL")
        .collect();
    assert!(!all_edges.is_empty(), "expected ALL operator to be extracted");
}

#[test]
fn operator_some_kept_separate_from_any() {
    let sql = "CREATE OR REPLACE PROCEDURE test_some() AS $$
    BEGIN
        FOR r IN (SELECT * FROM t1 WHERE col = SOME(SELECT col FROM t2)) LOOP
            NULL;
        END LOOP;
    END;
    $$ LANGUAGE plpgsql;";
    let stmts = ogsql_parser::Parser::parse_sql(sql).0;
    let mut extractor = CallExtractor::new(
        std::sync::Arc::new(std::path::PathBuf::from("test.sql")),
        std::collections::HashSet::new(),
    );
    ogsql_parser::walk_statement(&mut extractor, &stmts[0].statement);

    let some_edges: Vec<_> = extractor
        .edges
        .iter()
        .filter(|e| e.builtin_meta.is_some() && e.callee_name == "SOME")
        .collect();
    assert!(!some_edges.is_empty(), "expected SOME operator to be extracted as 'SOME'");

    // also verify ANY is NOT extracted (SOME ≠ ANY)
    let any_edges: Vec<_> = extractor
        .edges
        .iter()
        .filter(|e| e.builtin_meta.is_some() && e.callee_name == "ANY")
        .collect();
    assert!(any_edges.is_empty(), "SOME should NOT create an ANY node");
}

#[test]
fn operator_not_in_extracted_as_builtin() {
    let sql = "CREATE OR REPLACE PROCEDURE test_not_in() AS $$
    BEGIN
        FOR r IN (SELECT * FROM t1 WHERE col NOT IN (SELECT col FROM t2)) LOOP
            NULL;
        END LOOP;
    END;
    $$ LANGUAGE plpgsql;";
    let stmts = ogsql_parser::Parser::parse_sql(sql).0;
    let mut extractor = CallExtractor::new(
        std::sync::Arc::new(std::path::PathBuf::from("test.sql")),
        std::collections::HashSet::new(),
    );
    ogsql_parser::walk_statement(&mut extractor, &stmts[0].statement);

    let edges: Vec<_> = extractor
        .edges
        .iter()
        .filter(|e| e.builtin_meta.is_some() && e.callee_name == "NOT_IN")
        .collect();
    assert!(!edges.is_empty(), "expected NOT_IN operator to be extracted");
    assert_eq!(edges[0].builtin_meta.as_ref().unwrap().domain, "Predicate");
}
```

**Step 2:** Run tests:
```sh
cargo test operator_ -- --nocapture
```
Expected: all 5 tests PASS.

**Step 3:** Commit:
```sh
git add src/parser/extractor.rs
git commit -m "feat: extract SQL operators (ANY/ALL/SOME/EXISTS/IN) as BuiltinFunction nodes"
```

---

## Phase 2: Hint Extraction

### Task 2.1: Add hint extraction to `visit_select`

**Files:**
- Modify: `src/parser/extractor.rs:635-639` (`visit_select` method)

**Step 1:** Replace the current `visit_select`:

Current code (L635-639):
```rust
fn visit_select(&mut self, select: &SelectStatement) -> VisitorResult {
    for tr in &select.from {
        self.extract_func_from_table_ref(tr);
    }
    VisitorResult::Continue
}
```

Replace with:
```rust
fn visit_select(&mut self, select: &SelectStatement) -> VisitorResult {
    // ── Hint extraction ──
    for hint_name in select.hints.iter() {
        if hint_name.is_empty() {
            continue;
        }
        self.push_builtin_call(
            hint_name,
            ogsql_parser::ast::BuiltinFuncMeta {
                category: "Hint".into(),
                domain: "QueryPlan".into(),
            },
            0,
        );
    }

    // ── Existing FROM-clause function extraction ──
    for tr in &select.from {
        self.extract_func_from_table_ref(tr);
    }
    VisitorResult::Continue
}
```

**Step 2:** Compile:
```sh
cargo build
```
Expected: clean compile.

### Task 2.2: Write and run hint extraction test

**Files:**
- Modify: `src/parser/extractor.rs` test module

**Step 1:** Add test:

```rust
#[test]
fn hint_tablescan_extracted_as_builtin() {
    let sql = "CREATE OR REPLACE PROCEDURE test_hint() AS $$
    DECLARE
        r RECORD;
    BEGIN
        SELECT /*+ tablescan(t1) */ * INTO r FROM t1;
    END;
    $$ LANGUAGE plpgsql;";
    let stmts = ogsql_parser::Parser::parse_sql(sql).0;
    let mut extractor = CallExtractor::new(
        std::sync::Arc::new(std::path::PathBuf::from("test.sql")),
        std::collections::HashSet::new(),
    );
    ogsql_parser::walk_statement(&mut extractor, &stmts[0].statement);

    let hint_edges: Vec<_> = extractor
        .edges
        .iter()
        .filter(|e| e.builtin_meta.is_some() && e.callee_name == "tablescan")
        .collect();
    assert!(!hint_edges.is_empty(), "expected tablescan hint to be extracted");
    assert_eq!(hint_edges[0].builtin_meta.as_ref().unwrap().category, "Hint");
    assert_eq!(hint_edges[0].builtin_meta.as_ref().unwrap().domain, "QueryPlan");
}

#[test]
fn hint_multiple_extracted_as_builtin() {
    let sql = "CREATE OR REPLACE PROCEDURE test_hints() AS $$
    DECLARE
        r RECORD;
    BEGIN
        SELECT /*+ tablescan(t1) hashjoin(t1 t2) */ * INTO r FROM t1 JOIN t2 ON t1.id = t2.id;
    END;
    $$ LANGUAGE plpgsql;";
    let stmts = ogsql_parser::Parser::parse_sql(sql).0;
    let mut extractor = CallExtractor::new(
        std::sync::Arc::new(std::path::PathBuf::from("test.sql")),
        std::collections::HashSet::new(),
    );
    ogsql_parser::walk_statement(&mut extractor, &stmts[0].statement);

    let tablescan_edges: Vec<_> = extractor
        .edges
        .iter()
        .filter(|e| e.builtin_meta.is_some() && e.callee_name == "tablescan")
        .collect();
    let hashjoin_edges: Vec<_> = extractor
        .edges
        .iter()
        .filter(|e| e.builtin_meta.is_some() && e.callee_name == "hashjoin")
        .collect();
    assert!(!tablescan_edges.is_empty(), "expected tablescan hint");
    assert!(!hashjoin_edges.is_empty(), "expected hashjoin hint");
}
```

**Step 2:** Run tests:
```sh
cargo test hint_ -- --nocapture
```
Expected: both tests PASS.

**Step 3:** Commit:
```sh
git add src/parser/extractor.rs
git commit -m "feat: extract GaussDB optimizer hints as BuiltinFunction nodes"
```

---

## Phase 3: Integration Tests (Graph-Level)

Verify that the extracted edges flow through the builder and produce correct graph nodes.

### Task 3.1: Operator builtin node creation test

**Files:**
- Modify: `src/graph/builder.rs` test module

**Step 1:** Add tests (near existing `builtin_function_captured_as_node`). Uses the existing `build_from_sql` test helper (defined at L3548):

```rust
#[test]
fn operator_any_creates_builtin_node() {
    let sql = "CREATE OR REPLACE PROCEDURE proc_any() AS $$
    BEGIN
        FOR r IN (SELECT * FROM t1 WHERE col > ANY(SELECT col FROM t2)) LOOP
            NULL;
        END LOOP;
    END;
    $$ LANGUAGE plpgsql;";
    let graph = build_from_sql(sql);

    // Assert a BuiltinFunction node named "any" exists with Operator category
    let any_nodes: Vec<_> = graph
        .node_indices()
        .filter(|i| {
            matches!(&graph[*i], Node::BuiltinFunction { name, category, .. }
                if name.eq_ignore_ascii_case("any") && category == "Operator")
        })
        .collect();
    assert_eq!(any_nodes.len(), 1, "expected one BuiltinFunction node for ANY");

    // Assert a UsesBuiltinFunction edge exists
    let has_edge = graph
        .edge_indices()
        .any(|e| matches!(&graph[e], Edge::UsesBuiltinFunction { .. }));
    assert!(has_edge, "expected UsesBuiltinFunction edge");
}

#[test]
fn operator_exists_creates_builtin_node() {
    let sql = "CREATE OR REPLACE PROCEDURE proc_exists() AS $$
    BEGIN
        FOR r IN (SELECT * FROM t1 WHERE EXISTS(SELECT 1 FROM t2 WHERE t2.id = t1.id)) LOOP
            NULL;
        END LOOP;
    END;
    $$ LANGUAGE plpgsql;";
    let graph = build_from_sql(sql);

    let exists_nodes: Vec<_> = graph
        .node_indices()
        .filter(|i| {
            matches!(&graph[*i], Node::BuiltinFunction { name, category, domain, .. }
                if name.eq_ignore_ascii_case("exists")
                && category == "Operator"
                && domain == "Predicate")
        })
        .collect();
    assert_eq!(exists_nodes.len(), 1, "expected one BuiltinFunction node for EXISTS");
}

#[test]
fn operator_in_subquery_creates_builtin_node() {
    let sql = "CREATE OR REPLACE PROCEDURE proc_in() AS $$
    BEGIN
        FOR r IN (SELECT * FROM t1 WHERE col IN (SELECT col FROM t2)) LOOP
            NULL;
        END LOOP;
    END;
    $$ LANGUAGE plpgsql;";
    let graph = build_from_sql(sql);

    let in_nodes: Vec<_> = graph
        .node_indices()
        .filter(|i| {
            matches!(&graph[*i], Node::BuiltinFunction { name, domain, .. }
                if name.eq_ignore_ascii_case("in") && domain == "Predicate")
        })
        .collect();
    assert_eq!(in_nodes.len(), 1, "expected one BuiltinFunction node for IN");
}

#[test]
fn operator_all_creates_builtin_node() {
    let sql = "CREATE OR REPLACE PROCEDURE proc_all() AS $$
    BEGIN
        FOR r IN (SELECT * FROM t1 WHERE col > ALL(SELECT col FROM t2)) LOOP
            NULL;
        END LOOP;
    END;
    $$ LANGUAGE plpgsql;";
    let graph = build_from_sql(sql);

    let all_nodes: Vec<_> = graph
        .node_indices()
        .filter(|i| {
            matches!(&graph[*i], Node::BuiltinFunction { name, domain, .. }
                if name.eq_ignore_ascii_case("all") && domain == "Comparison")
        })
        .collect();
    assert_eq!(all_nodes.len(), 1, "expected one BuiltinFunction node for ALL");
}

#[test]
fn operator_some_creates_builtin_node() {
    let sql = "CREATE OR REPLACE PROCEDURE proc_some() AS $$
    BEGIN
        FOR r IN (SELECT * FROM t1 WHERE col = SOME(SELECT col FROM t2)) LOOP
            NULL;
        END LOOP;
    END;
    $$ LANGUAGE plpgsql;";
    let graph = build_from_sql(sql);

    let some_nodes: Vec<_> = graph
        .node_indices()
        .filter(|i| {
            matches!(&graph[*i], Node::BuiltinFunction { name, .. }
                if name.eq_ignore_ascii_case("some"))
        })
        .collect();
    assert_eq!(some_nodes.len(), 1, "expected one BuiltinFunction node for SOME");

    // Verify SOME and ANY are separate nodes (not deduped together)
    let any_nodes: Vec<_> = graph
        .node_indices()
        .filter(|i| {
            matches!(&graph[*i], Node::BuiltinFunction { name, .. }
                if name.eq_ignore_ascii_case("any"))
        })
        .collect();
    assert!(any_nodes.is_empty(), "SOME should NOT create an ANY node");
}

#[test]
fn operator_not_in_creates_builtin_node() {
    let sql = "CREATE OR REPLACE PROCEDURE proc_not_in() AS $$
    BEGIN
        FOR r IN (SELECT * FROM t1 WHERE col NOT IN (SELECT col FROM t2)) LOOP
            NULL;
        END LOOP;
    END;
    $$ LANGUAGE plpgsql;";
    let graph = build_from_sql(sql);

    let not_in_nodes: Vec<_> = graph
        .node_indices()
        .filter(|i| {
            matches!(&graph[*i], Node::BuiltinFunction { name, domain, .. }
                if name.eq_ignore_ascii_case("not_in") && domain == "Predicate")
        })
        .collect();
    assert_eq!(not_in_nodes.len(), 1, "expected one BuiltinFunction node for NOT_IN");
}

#[test]
fn operator_extraction_does_not_break_function_call() {
    // Regression: ensure adding operator detection doesn't break existing
    // FunctionCall extraction (builtin functions like COUNT).
    let sql = "CREATE OR REPLACE PROCEDURE proc_with_count() AS $$
    BEGIN
        PERFORM COUNT(*) FROM dual;
    END;
    $$ LANGUAGE plpgsql;";
    let graph = build_from_sql(sql);

    // COUNT should still be extracted as BuiltinFunction
    let count_nodes: Vec<_> = graph
        .node_indices()
        .filter(|i| {
            matches!(&graph[*i], Node::BuiltinFunction { name, .. }
                if name.eq_ignore_ascii_case("count"))
        })
        .collect();
    assert_eq!(count_nodes.len(), 1, "COUNT should still be extracted as BuiltinFunction");

    // No false operator nodes should be created
    for idx in graph.node_indices() {
        if let Node::BuiltinFunction { name, category, .. } = &graph[idx] {
            if category == "Operator" {
                panic!("unexpected Operator node '{}' from FunctionCall-only SQL", name);
            }
        }
    }
}
```

**Step 2:** Run:
```sh
cargo test operator_any_creates -- --nocapture
cargo test operator_exists_creates -- --nocapture
cargo test operator_in_subquery_creates -- --nocapture
cargo test operator_all_creates -- --nocapture
cargo test operator_some_creates -- --nocapture
cargo test operator_not_in_creates -- --nocapture
cargo test operator_extraction_does_not_break -- --nocapture
```
Expected: all 7 PASS.

### Task 3.2: Hint builtin node creation test

**Files:**
- Modify: `src/graph/builder.rs` test module

**Step 1:** Add test:

```rust
#[test]
fn hint_tablescan_creates_builtin_node() {
    let sql = "CREATE OR REPLACE PROCEDURE proc_hint() AS $$
    DECLARE
        r RECORD;
    BEGIN
        SELECT /*+ tablescan(t1) */ * INTO r FROM t1;
    END;
    $$ LANGUAGE plpgsql;";
    let graph = build_from_sql(sql);

    let hint_nodes: Vec<_> = graph
        .node_indices()
        .filter(|i| {
            matches!(&graph[*i], Node::BuiltinFunction { name, category, .. }
                if name.eq_ignore_ascii_case("tablescan") && category == "Hint")
        })
        .collect();
    assert_eq!(hint_nodes.len(), 1, "expected one BuiltinFunction node for tablescan hint");
}

#[test]
fn hint_multiple_creates_builtin_nodes() {
    let sql = "CREATE OR REPLACE PROCEDURE proc_hints() AS $$
    DECLARE
        r RECORD;
    BEGIN
        SELECT /*+ tablescan(t1) hashjoin(t1 t2) */ * INTO r FROM t1 JOIN t2 ON t1.id = t2.id;
    END;
    $$ LANGUAGE plpgsql;";
    let graph = build_from_sql(sql);

    let tablescan: Vec<_> = graph
        .node_indices()
        .filter(|i| matches!(&graph[*i], Node::BuiltinFunction { name, .. } if name == "tablescan"))
        .collect();
    let hashjoin: Vec<_> = graph
        .node_indices()
        .filter(|i| matches!(&graph[*i], Node::BuiltinFunction { name, .. } if name == "hashjoin"))
        .collect();
    assert_eq!(tablescan.len(), 1, "expected tablescan node");
    assert_eq!(hashjoin.len(), 1, "expected hashjoin node");
}
```

**Step 2:** Run:
```sh
cargo test hint_tablescan_creates -- --nocapture
cargo test hint_multiple_creates -- --nocapture
```
Expected: both PASS.

**Step 3:** Commit:
```sh
git add src/graph/builder.rs
git commit -m "test: integration tests for operator/hint builtin node creation"
```

---

## Phase 4: Full Verification Matrix

### Task 4.1: Full verification (AGENTS.md Definition of Done)

Run ALL of these. Every one must be clean. Document any pre-existing failure unrelated to this change.

```sh
# Compilation under every feature combination
cargo build
cargo build --features serve
cargo build --features jsp
cargo build --features mcp
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

### Task 4.2: Sanity-check CLI queryability

```sh
# Build the binary
cargo build

# After analyzing a project with operators/hints:
# codeweb detail "any"     → should show callers (procedures using ANY)
# codeweb detail "exists"  → should show callers
# codeweb detail "tablescan" → should show callers
# codeweb nodes -t builtin → should include operator/hint nodes
```

---

## File Change Summary

| File | Phase | Change |
|---|---|---|
| `src/parser/extractor.rs` | 1, 2 | Operator detection in `visit_expr` + hint extraction in `visit_select` + 8 unit tests |
| `src/graph/builder.rs` | 3 | 9 integration tests (6 operators + 1 regression + 2 hints) |

**Total: 2 files changed, 17 new tests. Zero enum variant additions.**

---

## Risk Notes

1. **`Expr::ScalarSublink` re-export** — verify ogsql-parser v0.8.13 re-exports `ScalarSublinkType` from its public API. If not, match on the variant directly without referencing the type:
   ```rust
   Expr::ScalarSublink { sublink_type, .. } => {
       let name = if matches!(sublink_type, _) { "ANY" } else { "ALL" };
   }
   ```
   Actually, `ScalarSublinkType` is a public enum in `ogsql_parser::ast`. The compiler will confirm.

2. **`add_sql_nodes` availability** — the integration tests in Phase 3 need the exact builder API. Search for `add_sql_nodes` in `builder.rs` before writing tests. If it's named differently (e.g., `build_sql_chunk`, `create_sql_edges`), adapt.

3. **Hint dedup within SELECT** — `SelectStatement.hints` may contain duplicates if the same hint name appears with different arguments (e.g., `/*+ tablescan(t1) tablescan(t2) */`). The builder's `builtin_index` deduplication by name handles this automatically — multiple push calls for the same name resolve to one node with multiple edges.

4. **Hints on DML** — GaussDB hints on INSERT/UPDATE/DELETE are not captured because only `SelectStatement` has the `hints` field. This is a known limitation. Nested SELECTs within DML (e.g., `INSERT ... SELECT`) will have their hints captured.

5. **Store backward compatibility** — no `Node`/`Edge` enum changes, so `GraphStore` bincode format is unchanged. Existing store files work without migration.

# Package / Trigger / View / DO Block Support — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Extend cobweb to recognize Package (spec + body, including PackageProcedure/PackageFunction), Trigger, View, and DO anonymous blocks as first-class graph objects, extracting their call relationships and table references.

**Architecture:** The change spans 3 layers: (1) Model layer — `ProcedureId` gains a `package` field; `Node` gains `Package`/`Trigger` variants; `Edge` gains `ContainsRoutine`/`TriggersRoutine`. (2) Builder layer — `GraphBuilder` recognizes the new Statement variants and creates nodes/edges. (3) Extractor layer — `CallExtractor` tracks package context so calls inside PackageProcedure bodies have the correct caller. The approach avoids modifying ogsql-parser upstream.

**Tech Stack:** Rust, ogsql-parser (existing enhanced Visitor with `walk_statement` Package support), petgraph, tempfile for tests.

**Prerequisite verified:** ogsql-parser commit `b9c9076` already parses `CREATE PACKAGE BODY` with full `PackageProcedure`/`PackageFunction` items including `PlBlock` bodies, and `walk_statement` recursively enters those blocks triggering `visit_pl_block` + `visit_procedure_call`. The walker does NOT call `visit_statement` for individual Package items — cobweb must handle context tracking manually.

---

## Key Design Decisions

### D1: ProcedureId gains `package: Option<String>`

```rust
pub struct ProcedureId {
    pub schema: Option<String>,
    pub package: Option<String>,  // NEW
    pub name: String,
}
```

- `CREATE PROCEDURE foo` → `{ schema: None, package: None, name: "foo" }`
- `CREATE PROCEDURE pkg.foo` → `{ schema: None, package: None, name: "pkg.foo" }` (ambiguous, resolved by proc_index lookup)
- Inside `CREATE PACKAGE BODY pkg AS ... PROCEDURE foo ...` → `{ schema: None, package: Some("pkg"), name: "foo" }`
- `CALL pkg.foo()` → `{ schema: None, package: None, name: "pkg.foo" }` at call site, resolved by matching proc_index
- `CREATE PROCEDURE schema.pkg.foo` — invalid SQL, won't happen
- `CALL schema.pkg.foo()` — `{ schema: Some("schema"), package: None, name: "pkg.foo" }` at call site

**Disambiguation rule:** Call sites always use `from_qualified_name` which joins multi-part names into `schema`. The builder's `create_edges` does `proc_index.get(&callee_id)` — if a package routine is registered as `{ package: Some("pkg"), name: "foo" }`, it won't match `{ schema: Some("pkg"), name: "foo" }`. Solution: in `create_edges`, try multiple lookup strategies (see D2).

### D2: Multi-strategy callee resolution in create_edges

When `proc_index` lookup fails for the raw `ProcedureId` from the call site, try:
1. Exact match (current behavior)
2. If `schema.is_some() && package.is_none()`: try `{ schema: None, package: schema, name }` — handles `CALL pkg.foo()` → package routine
3. If `schema.is_some()` and name has no dot: try `{ schema: schema, package: None, name }` — handles `CALL schema.proc()` → standalone routine

If none match → create `Unresolved` node (current behavior).

### D3: Package context tracking in builder (not extractor)

The `CallExtractor` is called via `walk_statement` which recursively enters Package blocks without calling `visit_statement`. We cannot reliably set `current_procedure` inside the extractor without an upstream hook.

**Solution:** In `builder.rs::collect_call_edges`, handle Package/PackageBody specially:
1. Don't pass `Statement::CreatePackage`/`CreatePackageBody` to `walk_statement` via the generic path
2. Instead, manually iterate `items`, and for each `PackageProcedure`/`PackageFunction`:
   - Set `extractor.current_procedure = ProcedureId { package: Some(pkg_name), name: item_name }`
   - Call `walk_pl_block` on the item's block
   - Collect the edges
3. This avoids modifying ogsql-parser and keeps context management in cobweb's control

### D4: Node/Edge extensions

New variants:
```rust
// Node
Package {
    schema: Option<String>,
    name: String,
    location: SourceLocation,
}
Trigger {
    name: String,
    table: ObjectName,  // the table this trigger is on
    location: SourceLocation,
}

// Edge
ContainsRoutine,  // Package → Procedure (package member)
TriggersRoutine { location: SourceLocation },  // Trigger → Procedure (the trigger's func_name)
```

---

## Test Fixture SQL

These SQL snippets represent the patterns we must handle. Tests reference them.

### F1: Package Body with Procedure + Function

```sql
CREATE OR REPLACE PACKAGE BODY pkg_api AS
    PROCEDURE do_work(p_id INT) IS
    BEGIN
        helper.validate(p_id);
        helper.process(p_id);
    END;

    FUNCTION get_status(p_id INT) RETURN VARCHAR IS
    BEGIN
        RETURN helper.check_status(p_id);
    END;
END pkg_api;
```

**Expected graph:**
- Node `Package(pkg_api)` — 1 node
- Node `Procedure(do_work, package=pkg_api)` — 1 node
- Node `Procedure(get_status, package=pkg_api)` — 1 node
- Edge `pkg_api → do_work` (ContainsRoutine)
- Edge `pkg_api → get_status` (ContainsRoutine)
- Node `Procedure(helper.validate)` — unresolved
- Edge `do_work → helper.validate` (DirectCall)
- Node `Procedure(helper.process)` — unresolved
- Edge `do_work → helper.process` (DirectCall)
- Node `Procedure(helper.check_status)` — unresolved
- Edge `get_status → helper.check_status` (DirectCall)

### F2: Package with cross-package call

```sql
CREATE OR REPLACE PROCEDURE standalone_proc() AS $$
BEGIN
    pkg_api.do_work(42);
END;
$$;

CREATE OR REPLACE PACKAGE BODY pkg_api AS
    PROCEDURE do_work(p_id INT) IS
    BEGIN
        helper.validate(p_id);
    END;
END pkg_api;
```

**Expected:** `standalone_proc → pkg_api.do_work` (DirectCall edge, callee resolved to the package routine node)

### F3: Trigger referencing a function

```sql
CREATE OR REPLACE FUNCTION trg_func() RETURNS TRIGGER AS $$
BEGIN
    INSERT INTO t_audit_log(action) VALUES('TRIGGER_FIRED');
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_after_insert
AFTER INSERT ON t_users
FOR EACH ROW EXECUTE PROCEDURE trg_func();
```

**Expected graph:**
- Node `Trigger(trg_after_insert)` — 1 node
- Node `Procedure(trg_func)` — 1 node
- Edge `Trigger → trg_func` (TriggersRoutine)
- Node `Table(t_audit_log)` — via table refs
- Edge `trg_func → t_audit_log` (ReferencesTable)

### F4: View with table references

```sql
CREATE VIEW v_active_users AS
SELECT u.id, u.name, o.total
FROM t_users u
JOIN t_orders o ON u.id = o.user_id
WHERE u.status = 'ACTIVE';
```

**Expected graph:**
- Node `View(v_active_users)` — 1 node
- Node `Table(t_users)` — existing
- Node `Table(t_orders)` — existing
- Edge `v_active_users → t_users` (ReferencesTable)
- Edge `v_active_users → t_orders` (ReferencesTable)

### F5: DO anonymous block

```sql
DO $$
BEGIN
    pkg_api.do_work(1);
END;
$$;
```

**Expected graph:**
- Node for `pkg_api.do_work` resolved or unresolved
- Edge from anonymous context → `pkg_api.do_work`
- (Design decision: DO blocks have no name. Options: (a) skip DO blocks entirely, (b) create an `AnonymousBlock` node, (c) treat like top-level calls with `caller: None`. **Recommend option (c)** for simplicity — the calls are captured but have no caller, matching current behavior for top-level CALL statements outside any routine.)

### F6: Schema-qualified package

```sql
CREATE OR REPLACE PACKAGE BODY myschema.pkg_utils AS
    PROCEDURE cleanup() IS
    BEGIN
        myschema.audit_log('cleanup');
    END;
END myschema.pkg_utils;
```

**Expected graph:**
- Node `Package(myschema.pkg_utils)` — schema=myschema, name=pkg_utils
- Node `Procedure(cleanup, package=pkg_utils, schema=myschema)` — with schema + package
- Edge `cleanup → myschema.audit_log` (DirectCall)

---

## Implementation Tasks (TDD)

### Task 1: Model layer — ProcedureId.package field

**Files:**
- Modify: `src/graph/mod.rs` — `ProcedureId` struct, `from_qualified_name`, `from_object_name`, `Display`
- Modify: `src/graph/key.rs` — `NodeKey::Procedure` variant, `Display`, `from_node`

**Step 1: Write the failing test for ProcedureId with package**

Add to `src/graph/mod.rs` in a `#[cfg(test)]` module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn procedure_id_standalone() {
        let id = ProcedureId::from_qualified_name("my_proc");
        assert_eq!(id.schema, None);
        assert_eq!(id.package, None);
        assert_eq!(id.name, "my_proc");
        assert_eq!(id.to_string(), "my_proc");
    }

    #[test]
    fn procedure_id_schema_qualified() {
        let id = ProcedureId::from_qualified_name("public.my_proc");
        assert_eq!(id.schema, Some("public".to_string()));
        assert_eq!(id.package, None);
        assert_eq!(id.name, "my_proc");
        assert_eq!(id.to_string(), "public.my_proc");
    }

    #[test]
    fn procedure_id_package_member() {
        // Package members are constructed via from_object_name in builder
        let id = ProcedureId {
            schema: None,
            package: Some("pkg_api".to_string()),
            name: "do_work".to_string(),
        };
        assert_eq!(id.to_string(), "pkg_api.do_work");
    }

    #[test]
    fn procedure_id_schema_package_member() {
        let id = ProcedureId {
            schema: Some("myschema".to_string()),
            package: Some("pkg_utils".to_string()),
            name: "cleanup".to_string(),
        };
        assert_eq!(id.to_string(), "myschema.pkg_utils.cleanup");
    }

    #[test]
    fn procedure_id_from_object_name_three_parts() {
        // "myschema.pkg_utils.cleanup" → schema=myschema, package=pkg_utils, name=cleanup
        // NOTE: from_object_name cannot distinguish schema.package.name from schema.name
        // It will be { schema: Some("myschema.pkg_utils"), name: "cleanup" }
        // Package members should be constructed explicitly, not via from_object_name
        let parts: Vec<String> = vec!["myschema".into(), "pkg_utils".into(), "cleanup".into()];
        let id = ProcedureId::from_object_name(&parts);
        assert_eq!(id.schema, Some("myschema.pkg_utils".to_string()));
        assert_eq!(id.name, "cleanup");
    }

    #[test]
    fn procedure_id_equality_independent_of_construction() {
        // Two ProcedureIds with same package+name should be equal
        let a = ProcedureId {
            schema: None,
            package: Some("pkg".to_string()),
            name: "proc".to_string(),
        };
        let b = ProcedureId {
            schema: None,
            package: Some("pkg".to_string()),
            name: "proc".to_string(),
        };
        assert_eq!(a, b);
    }

    #[test]
    fn procedure_id_hash_in_hashmap() {
        use std::collections::HashMap;
        let mut map = HashMap::new();
        let id = ProcedureId {
            schema: None,
            package: Some("pkg".to_string()),
            name: "proc".to_string(),
        };
        map.insert(id.clone(), 42);
        assert_eq!(map.get(&id), Some(&42));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib graph::tests`
Expected: FAIL — `ProcedureId` does not have `package` field

**Step 3: Add `package` field to ProcedureId**

In `src/graph/mod.rs`, add `package: Option<String>` to `ProcedureId`, update `from_qualified_name` (leave as-is, doesn't set package), `from_object_name` (leave as-is), `Display`:

```rust
impl fmt::Display for ProcedureId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.schema, &self.package) {
            (Some(s), Some(p)) => write!(f, "{}.{}.{}", s, p, self.name),
            (Some(s), None) => write!(f, "{}.{}", s, self.name),
            (None, Some(p)) => write!(f, "{}.{}", p, self.name),
            (None, None) => write!(f, "{}", self.name),
        }
    }
}
```

Update all `ProcedureId` construction sites (compiler will show them):
- `from_qualified_name` → add `package: None`
- `from_object_name` → add `package: None`

**Step 4: Run test to verify it passes**

Run: `cargo test --lib graph::tests`
Expected: PASS

**Step 5: Update NodeKey::Procedure**

In `src/graph/key.rs`, add `package: Option<String>` to `NodeKey::Procedure`, update Display and `from_node`.

**Step 6: Run all tests**

Run: `cargo test`
Expected: PASS (no behavioral change yet, just model extension)

**Step 7: Commit**

```
feat: add package field to ProcedureId for package member identification
```

---

### Task 2: Model layer — Node::Package, Node::Trigger, Edge variants

**Files:**
- Modify: `src/graph/mod.rs` — `Node` enum, `Edge` enum, `Node::file()` match
- Modify: `src/graph/key.rs` — `NodeKey` enum, `Display`, `from_node`
- Modify: `src/export/json.rs` — `NodeKindJson`, `EdgeKindJson`, `to_json` match arms
- Modify: `src/export/dot.rs` — `to_dot` match arms
- Modify: `src/export/mermaid.rs` — `to_mermaid` match arms

**Step 1: Write the failing test for Node::Package in graph model**

Add to `src/graph/mod.rs` test module:

```rust
#[test]
fn node_package_variant_exists() {
    let node = Node::Package {
        schema: Some("myschema".to_string()),
        name: "pkg_api".to_string(),
        location: SourceLocation {
            file: std::path::PathBuf::from("test.sql"),
            line: 1,
        },
    };
    assert_eq!(node.file(), std::path::Path::new("test.sql"));
}

#[test]
fn node_trigger_variant_exists() {
    let node = Node::Trigger {
        name: "trg_after_insert".to_string(),
        table: vec!["t_users".to_string()],
        location: SourceLocation {
            file: std::path::PathBuf::from("test.sql"),
            line: 5,
        },
    };
    assert_eq!(node.file(), std::path::Path::new("test.sql"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib graph::tests`
Expected: FAIL — `Node::Package` / `Node::Trigger` don't exist

**Step 3: Add Node variants**

```rust
// In Node enum:
Package {
    schema: Option<String>,
    name: String,
    location: SourceLocation,
}
Trigger {
    name: String,
    table: ObjectName,
    location: SourceLocation,
}
```

Note: `ObjectName` is `Vec<String>` in ogsql-parser. We'll use `Vec<String>` directly.

Update `Node::file()` match.

**Step 4: Add Edge variants**

```rust
// In Edge enum:
ContainsRoutine,
TriggersRoutine { location: SourceLocation },
```

**Step 5: Update key.rs**

Add `NodeKey::Package` and `NodeKey::Trigger` variants, Display, from_node.

**Step 6: Update exports (json, dot, mermaid)**

Add match arms for new variants. Package → `component` shape, Trigger → `hexagon` shape.

**Step 7: Run all tests**

Run: `cargo test`
Expected: PASS

**Step 8: Commit**

```
feat: add Package/Trigger node types and ContainsRoutine/TriggersRoutine edges
```

---

### Task 3: Builder — Package node creation + PackageProcedure/PackageFunction as Procedure nodes

**Files:**
- Modify: `src/graph/builder.rs` — `create_procedure_nodes`, `collect_call_edges`, `create_edges`

**Step 1: Write the failing test — Package body creates correct nodes**

This is a unit test in `src/graph/builder.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use ogsql_parser::Tokenizer;

    fn parse_sql(sql: &str) -> Vec<ogsql_parser::StatementInfo> {
        let tokens = Tokenizer::new(sql).tokenize().unwrap();
        let mut parser = ogsql_parser::Parser::with_source(tokens, sql.to_string());
        parser.parse_with_text()
    }

    fn build_from_sql(sql: &str) -> CodeGraph {
        let stmts = parse_sql(sql);
        let parsed = vec![crate::parser::ParsedFile {
            path: std::path::PathBuf::from("test.sql"),
            statements: stmts,
        }];
        GraphBuilder::new().build(&parsed)
    }

    #[test]
    fn package_body_creates_package_and_procedure_nodes() {
        let sql = r#"
            CREATE OR REPLACE PACKAGE BODY pkg_api AS
                PROCEDURE do_work(p_id INT) IS
                BEGIN
                    helper.validate(p_id);
                END;
            END pkg_api;
        "#;
        let graph = build_from_sql(sql);

        // Should have: 1 Package node + 1 Procedure node + 1 Unresolved node
        let package_nodes: Vec<_> = graph.node_indices()
            .filter(|i| matches!(graph[*i], Node::Package { .. }))
            .collect();
        assert_eq!(package_nodes.len(), 1, "Expected 1 Package node");

        let proc_nodes: Vec<_> = graph.node_indices()
            .filter(|i| matches!(graph[*i], Node::Procedure { .. }))
            .collect();
        assert_eq!(proc_nodes.len(), 1, "Expected 1 Procedure node (package member)");

        // Verify package node name
        if let Node::Package { name, .. } = &graph[package_nodes[0]] {
            assert_eq!(name, "pkg_api");
        }

        // Verify procedure has package context
        if let Node::Procedure { id, .. } = &graph[proc_nodes[0]] {
            assert_eq!(id.package, Some("pkg_api".to_string()));
            assert_eq!(id.name, "do_work");
        }

        // Verify ContainsRoutine edge
        let contains_edges: Vec<_> = graph.edge_indices()
            .filter(|e| matches!(graph[*e], Edge::ContainsRoutine))
            .collect();
        assert_eq!(contains_edges.len(), 1, "Expected 1 ContainsRoutine edge");
    }

    #[test]
    fn package_body_procedure_calls_have_correct_caller() {
        let sql = r#"
            CREATE OR REPLACE PACKAGE BODY pkg_api AS
                PROCEDURE do_work(p_id INT) IS
                BEGIN
                    helper.validate(p_id);
                    helper.process(p_id);
                END;
            END pkg_api;
        "#;
        let graph = build_from_sql(sql);

        // The calls from do_work should create edges from the Procedure node
        let proc_idx = graph.node_indices()
            .find(|i| matches!(&graph[*i], Node::Procedure { id, .. } if id.name == "do_work"))
            .expect("should find do_work procedure");

        let outgoing: Vec<_> = graph.neighbors_directed(proc_idx, petgraph::Direction::Outgoing)
            .collect();
        // Should have: 1 ContainsRoutine (from Package) + 2 DirectCall edges
        // Wait, ContainsRoutine is from Package→Procedure, not Procedure→...
        // So outgoing from proc should be the 2 DirectCall edges
        let call_edges_from_proc: Vec<_> = graph.edges_directed(proc_idx, petgraph::Direction::Outgoing)
            .filter(|e| matches!(e.weight(), Edge::DirectCall { .. }))
            .collect();
        assert_eq!(call_edges_from_proc.len(), 2, "Expected 2 DirectCall edges from do_work");
    }

    #[test]
    fn package_body_function_creates_procedure_node() {
        let sql = r#"
            CREATE OR REPLACE PACKAGE BODY pkg_api AS
                FUNCTION get_val(p_id INT) RETURN NUMBER IS
                BEGIN
                    RETURN helper.compute(p_id);
                END;
            END pkg_api;
        "#;
        let graph = build_from_sql(sql);

        let proc_nodes: Vec<_> = graph.node_indices()
            .filter(|i| matches!(&graph[*i], Node::Procedure { id, .. } if id.name == "get_val"))
            .collect();
        assert_eq!(proc_nodes.len(), 1, "Expected 1 Procedure node for package function get_val");

        if let Node::Procedure { id, .. } = &graph[proc_nodes[0]] {
            assert_eq!(id.package, Some("pkg_api".to_string()));
        }
    }

    #[test]
    fn standalone_call_to_package_routine_resolves() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURE caller_proc() AS $$
            BEGIN
                pkg_api.do_work(42);
            END;
            $$;

            CREATE OR REPLACE PACKAGE BODY pkg_api AS
                PROCEDURE do_work(p_id INT) IS
                BEGIN
                    helper.validate(p_id);
                END;
            END pkg_api;
        "#;
        let graph = build_from_sql(sql);

        // caller_proc should have a DirectCall edge to pkg_api.do_work (resolved)
        let caller_idx = graph.node_indices()
            .find(|i| matches!(&graph[*i], Node::Procedure { id, .. } if id.name == "caller_proc"))
            .expect("should find caller_proc");

        let direct_calls: Vec<_> = graph.edges_directed(caller_idx, petgraph::Direction::Outgoing)
            .filter_map(|e| {
                if matches!(e.weight(), Edge::DirectCall { .. }) {
                    Some(e.target())
                } else {
                    None
                }
            })
            .collect();

        // At least one resolved call
        let resolved_targets: Vec<_> = direct_calls.iter()
            .filter(|&&t| matches!(graph[t], Node::Procedure { .. }))
            .collect();
        assert!(!resolved_targets.is_empty(), "caller_proc should have a resolved call to pkg_api.do_work");
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib graph::builder::tests`
Expected: FAIL — Package body statement not handled in `create_procedure_nodes`

**Step 3: Implement Package handling in builder**

In `create_procedure_nodes`, add handling for `Statement::CreatePackage` and `Statement::CreatePackageBody`:

```rust
Statement::CreatePackage(pkg) | Statement::CreatePackageBody(pkg) => {
    // Create Package node
    let pkg_id = ProcedureId::from_object_name(&pkg.name);
    let pkg_key = format!("pkg:{}", pkg_id);  // or use a separate index
    // ... create Node::Package, index it, iterate items ...
    for item in &pkg.items {
        match item {
            ogsql_parser::ast::PackageItem::Procedure(p) => {
                let proc_id = ProcedureId {
                    schema: pkg_id.schema.clone(),
                    package: Some(pkg_id.name.clone()),
                    name: p.name.join("."),
                };
                // Create Node::Procedure with this id
                // Create Edge::ContainsRoutine from Package to Procedure
            }
            ogsql_parser::ast::PackageItem::Function(f) => {
                // Same as Procedure
            }
            ogsql_parser::ast::PackageItem::Raw(_) => {}
        }
    }
}
```

In `collect_call_edges`, handle Package specially to set `current_procedure`:

```rust
Statement::CreatePackage(pkg) | Statement::CreatePackageBody(pkg) => {
    let pkg_id = ProcedureId::from_object_name(&pkg.name);
    for item in &pkg.items {
        match item {
            ogsql_parser::ast::PackageItem::Procedure(p) => {
                if let Some(ref block) = p.block {
                    extractor.current_procedure = Some(ProcedureId {
                        schema: pkg_id.schema.clone(),
                        package: Some(pkg_id.name.clone()),
                        name: p.name.join("."),
                    });
                    ogsql_parser::walk_pl_block(&mut extractor, block);
                }
            }
            ogsql_parser::ast::PackageItem::Function(f) => {
                if let Some(ref block) = f.block {
                    extractor.current_procedure = Some(ProcedureId {
                        schema: pkg_id.schema.clone(),
                        package: Some(pkg_id.name.clone()),
                        name: f.name.join("."),
                    });
                    ogsql_parser::walk_pl_block(&mut extractor, block);
                }
            }
            _ => {}
        }
    }
}
```

In `create_edges`, add multi-strategy resolution for package routines.

**Step 4: Run test to verify it passes**

Run: `cargo test --lib graph::builder::tests`
Expected: PASS

**Step 5: Commit**

```
feat: builder creates Package nodes and resolves package routine calls
```

---

### Task 4: Builder — Trigger node creation

**Files:**
- Modify: `src/graph/builder.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn trigger_creates_trigger_node_and_edge() {
    let sql = r#"
        CREATE OR REPLACE FUNCTION trg_func() RETURNS TRIGGER AS $$
        BEGIN
            RETURN NEW;
        END;
        $$ LANGUAGE plpgsql;

        CREATE TRIGGER trg_after_insert
        AFTER INSERT ON t_users
        FOR EACH ROW EXECUTE PROCEDURE trg_func();
    "#;
    let graph = build_from_sql(sql);

    // Should have: 1 Procedure (trg_func) + 1 Trigger + 1 TriggersRoutine edge
    let trigger_nodes: Vec<_> = graph.node_indices()
        .filter(|i| matches!(graph[*i], Node::Trigger { .. }))
        .collect();
    assert_eq!(trigger_nodes.len(), 1, "Expected 1 Trigger node");

    if let Node::Trigger { name, .. } = &graph[trigger_nodes[0]] {
        assert_eq!(name, "trg_after_insert");
    }

    // Verify TriggersRoutine edge
    let trigger_edges: Vec<_> = graph.edge_indices()
        .filter(|e| matches!(graph[*e], Edge::TriggersRoutine { .. }))
        .collect();
    assert_eq!(trigger_edges.len(), 1, "Expected 1 TriggersRoutine edge");

    // Verify edge connects Trigger → trg_func
    let (src, dst) = graph.edge_endpoints(trigger_edges[0]).unwrap();
    assert!(matches!(graph[src], Node::Trigger { .. }));
    assert!(matches!(graph[dst], Node::Procedure { .. }));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib graph::builder::tests`
Expected: FAIL — `Statement::CreateTrigger` not handled

**Step 3: Implement Trigger handling in builder**

In `create_procedure_nodes` (or a new helper), add:

```rust
Statement::CreateTrigger(t) => {
    // Create Node::Trigger
    // Look up func_name in proc_index
    // Create Edge::TriggersRoutine from Trigger → Procedure (or Unresolved)
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test --lib graph::builder::tests`
Expected: PASS

**Step 5: Commit**

```
feat: builder creates Trigger nodes with TriggersRoutine edges
```

---

### Task 5: Builder — View node creation + table reference extraction

**Files:**
- Modify: `src/graph/builder.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn view_creates_view_node_and_table_refs() {
    let sql = r#"
        CREATE VIEW v_active_users AS
        SELECT u.id, u.name
        FROM t_users u
        WHERE u.status = 'ACTIVE';
    "#;
    let graph = build_from_sql(sql);

    let view_nodes: Vec<_> = graph.node_indices()
        .filter(|i| matches!(graph[*i], Node::View { .. }))
        .collect();
    assert_eq!(view_nodes.len(), 1, "Expected 1 View node");

    if let Node::View { name, .. } = &graph[view_nodes[0]] {
        assert_eq!(name, "v_active_users");
    }

    // View should reference t_users
    let refs_from_view: Vec<_> = graph.edges_directed(view_nodes[0], petgraph::Direction::Outgoing)
        .filter(|e| matches!(e.weight(), Edge::ReferencesTable { .. }))
        .collect();
    assert_eq!(refs_from_view.len(), 1, "View should reference 1 table");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib graph::builder::tests`
Expected: FAIL — View node never created

**Step 3: Implement View handling**

In builder, add handling for `Statement::CreateView`:
- Create `Node::View`
- Extract table refs from the view's query using `TableRefExtractor`
- Add `ReferencesTable` edges

**Step 4: Run test to verify it passes**

Run: `cargo test --lib graph::builder::tests`
Expected: PASS

**Step 5: Commit**

```
feat: builder creates View nodes with table reference extraction
```

---

### Task 6: Extractor — CallExtractor unit tests for edge cases

**Files:**
- Modify: `src/parser/extractor.rs` — add `#[cfg(test)]` module

**Step 1: Write unit tests for CallExtractor**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use ogsql_parser::Tokenizer;
    use std::path::PathBuf;

    fn extract_edges(sql: &str) -> Vec<CallEdge> {
        let tokens = Tokenizer::new(sql).tokenize().unwrap();
        let mut parser = ogsql_parser::Parser::with_source(tokens, sql.to_string());
        let stmts = parser.parse_with_text();

        let mut all_edges = Vec::new();
        for info in &stmts {
            let mut extractor = CallExtractor::new(PathBuf::from("test.sql"));
            ogsql_parser::walk_statement(&mut extractor, &info.statement);
            all_edges.extend(extractor.edges);
        }
        all_edges
    }

    #[test]
    fn standalone_procedure_call() {
        let edges = extract_edges("CREATE PROCEDURE a() AS $$ BEGIN b(); END; $$;");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].caller.as_ref().unwrap().name, "a");
        assert_eq!(edges[0].callee_name, "b");
        assert!(!edges[0].is_dynamic);
    }

    #[test]
    fn schema_qualified_call() {
        let edges = extract_edges(
            "CREATE PROCEDURE a() AS $$ BEGIN pkg.b(); END; $$;"
        );
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].callee_name, "pkg.b");
    }

    #[test]
    fn function_in_select_from() {
        let edges = extract_edges(
            "CREATE FUNCTION f() RETURNS INT AS $$ BEGIN PERFORM * FROM generate_series(1,10); RETURN 1; END; $$ LANGUAGE plpgsql;"
        );
        // generate_series should be captured as a function call in FROM clause
        let callee_names: Vec<&str> = edges.iter().map(|e| e.callee_name.as_str()).collect();
        assert!(callee_names.iter().any(|n| n.contains("generate_series")),
            "Expected generate_series in callees: {:?}", callee_names);
    }

    #[test]
    fn dynamic_sql_is_marked() {
        let sql = r#"
            CREATE PROCEDURE a() AS $$
            BEGIN
                EXECUTE IMMEDIATE 'CALL ' || v_proc || '()';
            END;
            $$;
        "#;
        let tokens = Tokenizer::new(sql).tokenize().unwrap();
        let mut parser = ogsql_parser::Parser::with_source(tokens, sql.to_string());
        let stmts = parser.parse_with_text();

        let mut extractor = CallExtractor::new(PathBuf::from("test.sql"));
        for info in &stmts {
            ogsql_parser::walk_statement(&mut extractor, &info.statement);
        }

        let dynamic_edges: Vec<_> = extractor.edges.iter().filter(|e| e.is_dynamic).collect();
        assert!(!dynamic_edges.is_empty(), "Expected at least one dynamic call edge");
    }

    #[test]
    fn multiple_calls_in_one_procedure() {
        let sql = r#"
            CREATE PROCEDURE a() AS $$
            BEGIN
                b();
                c(1);
                d(1, 2);
            END;
            $$;
        "#;
        let edges = extract_edges(sql);
        assert_eq!(edges.len(), 3);
        let callees: Vec<&str> = edges.iter().map(|e| e.callee_name.as_str()).collect();
        assert!(callees.contains(&"b"));
        assert!(callees.contains(&"c"));
        assert!(callees.contains(&"d"));
    }

    #[test]
    fn top_level_call_statement() {
        // CALL outside any procedure body
        let edges = extract_edges("CALL my_proc(1, 2);");
        // This should create an edge with caller: None
        assert_eq!(edges.len(), 1);
        assert!(edges[0].caller.is_none());
        assert_eq!(edges[0].callee_name, "my_proc");
    }
}
```

**Step 2: Run tests to verify they pass (these test existing behavior)**

Run: `cargo test --lib parser::extractor::tests`
Expected: PASS (these tests verify existing extractor behavior, establishing baseline)

**Step 3: Commit**

```
test: add unit tests for CallExtractor edge extraction
```

---

### Task 7: Extractor — Package context tracking unit test

**Files:**
- Modify: `src/parser/extractor.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn package_body_procedure_calls_have_caller_context() {
    // When we manually set current_procedure and walk the PL block,
    // the calls inside should have the correct caller
    let sql = r#"
        CREATE OR REPLACE PACKAGE BODY pkg_api AS
            PROCEDURE do_work(p_id INT) IS
            BEGIN
                helper.validate(p_id);
                helper.process(p_id);
            END;
        END pkg_api;
    "#;
    let tokens = Tokenizer::new(sql).tokenize().unwrap();
    let mut parser = ogsql_parser::Parser::with_source(tokens, sql.to_string());
    let stmts = parser.parse_with_text();

    // Simulate what builder will do: manually iterate Package items
    let mut extractor = CallExtractor::new(PathBuf::from("test.sql"));

    for info in &stmts {
        if let ogsql_parser::ast::Statement::CreatePackageBody(pkg) = &info.statement {
            for item in &pkg.items {
                if let ogsql_parser::ast::PackageItem::Procedure(p) = item {
                    if let Some(ref block) = p.block {
                        extractor.current_procedure = Some(ProcedureId {
                            schema: None,
                            package: Some("pkg_api".to_string()),
                            name: "do_work".to_string(),
                        });
                        ogsql_parser::walk_pl_block(&mut extractor, block);
                    }
                }
            }
        }
    }

    assert_eq!(extractor.edges.len(), 2);
    // Verify caller is set correctly
    for edge in &extractor.edges {
        let caller = edge.caller.as_ref().expect("caller should be set");
        assert_eq!(caller.package, Some("pkg_api".to_string()));
        assert_eq!(caller.name, "do_work");
    }
    assert_eq!(extractor.edges[0].callee_name, "helper.validate");
    assert_eq!(extractor.edges[1].callee_name, "helper.process");
}
```

**Step 2: Run test to verify it passes**

Run: `cargo test --lib parser::extractor::tests::package_body_procedure_calls_have_caller_context`
Expected: PASS — this test verifies that the extractor correctly uses `current_procedure` when we set it manually and walk the PL block. This is the pattern the builder will use.

**Step 3: Commit**

```
test: verify extractor tracks package context for procedure calls
```

---

### Task 8: Integration test — Package + Trigger + View end-to-end

**Files:**
- Modify: `tests/integration_test.rs`
- Add: `lib/codeweb-e2e-demo/sql/pkg_api.sql` (new fixture)

**Step 1: Write failing integration tests**

```rust
#[test]
fn test_package_body_in_call_graph() {
    let dir = TempDir::new().unwrap();
    write_sql(&dir, "pkg.sql", r#"
        CREATE OR REPLACE PACKAGE BODY pkg_api AS
            PROCEDURE do_work(p_id INT) IS
            BEGIN
                helper.validate(p_id);
            END;
        END pkg_api;
    "#);

    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let nodes = parsed["nodes"].as_array().unwrap();

    // Should have Package node
    let has_package = nodes.iter().any(|n| n["type"] == "package");
    assert!(has_package, "Expected a package node");

    // Should have Procedure node with package context
    let has_package_proc = nodes.iter().any(|n|
        n["type"] == "procedure" && n["name"] == "do_work" && n["package"] == Some("pkg_api")
    );
    assert!(has_package_proc, "Expected a procedure node with package=pkg_api");
}

#[test]
fn test_trigger_in_call_graph() {
    let dir = TempDir::new().unwrap();
    write_sql(&dir, "trigger.sql", r#"
        CREATE OR REPLACE FUNCTION trg_func() RETURNS TRIGGER AS $$
        BEGIN
            INSERT INTO t_log(action) VALUES('FIRED');
            RETURN NEW;
        END;
        $$ LANGUAGE plpgsql;

        CREATE TRIGGER trg_after_insert
        AFTER INSERT ON t_users
        FOR EACH ROW EXECUTE PROCEDURE trg_func();
    "#);

    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let nodes = parsed["nodes"].as_array().unwrap();

    let has_trigger = nodes.iter().any(|n| n["type"] == "trigger");
    assert!(has_trigger, "Expected a trigger node");

    let edges = parsed["edges"].as_array().unwrap();
    let has_triggers_routine = edges.iter().any(|e| e["type"] == "triggers_routine");
    assert!(has_triggers_routine, "Expected a triggers_routine edge");
}

#[test]
fn test_view_table_references() {
    let dir = TempDir::new().unwrap();
    write_sql(&dir, "view.sql", r#"
        CREATE VIEW v_active_users AS
        SELECT u.id, u.name
        FROM t_users u
        WHERE u.status = 'ACTIVE';
    "#);

    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let nodes = parsed["nodes"].as_array().unwrap();

    let has_view = nodes.iter().any(|n| n["type"] == "view" && n["name"] == "v_active_users");
    assert!(has_view, "Expected a view node named v_active_users");

    let edges = parsed["edges"].as_array().unwrap();
    let refs_from_view = edges.iter().filter(|e| e["type"] == "references_table");
    // The view should reference t_users
    assert!(refs_from_view.count() >= 1, "Expected at least 1 references_table edge from view");
}

#[test]
fn test_package_cross_call_resolution() {
    let dir = TempDir::new().unwrap();
    write_sql(&dir, "mixed.sql", r#"
        CREATE OR REPLACE PROCEDURE caller_proc() AS $$
        BEGIN
            pkg_api.do_work(42);
        END;
        $$;

        CREATE OR REPLACE PACKAGE BODY pkg_api AS
            PROCEDURE do_work(p_id INT) IS
            BEGIN
                helper.validate(p_id);
            END;
        END pkg_api;
    "#);

    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "dot"]);
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    // In DOT output, caller_proc should have an edge to do_work (resolved)
    assert!(stdout.contains("caller_proc"));
    assert!(stdout.contains("do_work"));
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test --test integration_test -- test_package_body_in_call_graph test_trigger_in_call_graph test_view_table_references test_package_cross_call_resolution`
Expected: FAIL — new object types not recognized yet

**Step 3: This is the gate — do NOT implement yet**

These tests define the acceptance criteria. Implementation happens in Tasks 1-5 above. This test is written first per TDD, implementation follows.

**Step 4: After implementation, run tests to verify they pass**

Run: `cargo test --test integration_test`
Expected: ALL PASS

**Step 5: Commit**

```
test: add integration tests for Package, Trigger, View support
```

---

### Task 9: Update e2e demo + test_e2e_full_chain

**Files:**
- Modify: `lib/codeweb-e2e-demo/sql/pkg_audit.sql` — add a real `CREATE PACKAGE BODY` for pkg_audit
- Modify: `lib/codeweb-e2e-demo/sql/pkg_notify.sql` — convert to `CREATE PACKAGE BODY`
- Add: `lib/codeweb-e2e-demo/sql/views.sql` — add a view
- Add: `lib/codeweb-e2e-demo/sql/triggers.sql` — add a trigger
- Modify: `tests/integration_test.rs` — extend `test_e2e_full_chain` to assert Package, Trigger, View node types

**Step 1: Add fixture files**

Convert existing "fake package" SQL files (procedures using `pkg_xxx.name` naming convention) to real `CREATE PACKAGE BODY` syntax where appropriate. Add view and trigger fixtures.

**Step 2: Extend e2e test assertions**

```rust
// In test_e2e_full_chain, add:
assert!(
    node_types.contains("package"),
    "Expected package nodes"
);
assert!(
    node_types.contains("trigger"),
    "Expected trigger nodes"
);
assert!(
    node_types.contains("view"),
    "Expected view nodes"
);
assert!(
    edge_types.contains("contains_routine"),
    "Expected contains_routine edges: package → procedure"
);
assert!(
    edge_types.contains("triggers_routine"),
    "Expected triggers_routine edges: trigger → procedure"
);
```

**Step 3: Run all tests**

Run: `cargo test`
Expected: PASS (this should work after Tasks 1-8 are complete)

**Step 4: Commit**

```
feat: add Package/Trigger/View fixtures to e2e demo and extend test assertions
```

---

### Task 10: Final verification — clippy + fmt + full test suite

**Step 1: Run linter**

Run: `cargo clippy -- -D warnings`
Expected: No warnings

**Step 2: Run formatter check**

Run: `cargo fmt -- --check`
Expected: No issues

**Step 3: Run full test suite**

Run: `cargo test`
Expected: ALL PASS

**Step 4: Commit any fixes**

```
chore: address clippy warnings and format
```

---

## Summary

| Task | Layer | What | Tests First? |
|---|---|---|---|
| 1 | Model | `ProcedureId.package` field | ✅ Failing test → implement |
| 2 | Model | `Node::Package`/`Trigger`, `Edge::ContainsRoutine`/`TriggersRoutine` | ✅ Failing test → implement |
| 3 | Builder | Package node creation + routine resolution | ✅ Failing test → implement |
| 4 | Builder | Trigger node creation | ✅ Failing test → implement |
| 5 | Builder | View node + table refs | ✅ Failing test → implement |
| 6 | Extractor | Baseline unit tests (existing behavior) | ✅ Tests for existing code |
| 7 | Extractor | Package context tracking pattern | ✅ Test the builder→extractor contract |
| 8 | Integration | E2E tests for all new objects | ✅ Failing tests → gate |
| 9 | E2E Demo | Update fixtures + extend assertions | ✅ |
| 10 | Verification | clippy + fmt + full suite | ✅ |

**Total: ~10 tasks, ~2-3 days of work.**

**Execution order:** Tasks 6-7 (establish baseline) → Task 8 (integration tests as gate) → Tasks 1-2 (model) → Tasks 3-5 (builder) → Task 9 (e2e) → Task 10 (verification)

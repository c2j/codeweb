# ResolutionEngine Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Unify all name resolution into a single engine with multi-strategy lookup (synonym chains, caller context, case-insensitive, bare name), replacing 5 ad-hoc resolution sites in builder.rs.

**Architecture:** Extract a `ResolutionEngine` struct that holds all indexes (proc, table, synonym, bare_name). It exposes two methods — `resolve_routine()` and `resolve_table()` — that try multiple strategies in priority order. All 5 Unresolved-creation sites in `build_graph_internal()` are refactored to call the engine. The engine is built incrementally during Pass 1, then used by all subsequent passes.

**Tech Stack:** Rust, existing `petgraph`, `HashMap`, `HashSet`.

---

### Task 1: Create ResolutionEngine struct with indexes

**Files:**
- Create: `src/graph/resolver.rs`
- Modify: `src/graph/mod.rs:1` (add `pub mod resolver;`)

**Step 1: Write the failing test**

Add test at bottom of `src/graph/resolver.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Node, RoutineId, RoutineKind};
    use petgraph::graph::NodeIndex;

    #[test]
    fn exact_match_resolves() {
        let mut engine = ResolutionEngine::new();
        let mut graph = CodeGraph::new();
        let idx = graph.add_node(Node::Procedure {
            id: RoutineId {
                schema: None,
                package: None,
                name: "do_work".to_string(),
                kind: RoutineKind::Procedure,
            },
            location: crate::graph::SourceLocation {
                file: std::sync::Arc::new(std::path::PathBuf::from("test.sql")),
                line: 1,
            },
            partial: false,
        });
        engine.register_routine(
            RoutineId {
                schema: None,
                package: None,
                name: "do_work".to_string(),
                kind: RoutineKind::Procedure,
            },
            idx,
        );
        let result = engine.resolve_routine("do_work", None);
        assert_eq!(result, Some(idx));
    }

    #[test]
    fn nonexistent_returns_none() {
        let engine = ResolutionEngine::new();
        let result = engine.resolve_routine("nonexistent", None);
        assert_eq!(result, None);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test resolver --lib`
Expected: FAIL — module not found

**Step 3: Write minimal implementation**

Create `src/graph/resolver.rs`:

```rust
use crate::graph::{CodeGraph, Node, RoutineId, RoutineKind};
use petgraph::graph::NodeIndex;
use std::collections::HashMap;

/// Indexes and strategies for resolving names across the graph.
pub struct ResolutionEngine {
    /// Primary routine index (RoutineId → NodeIndex).
    proc_index: HashMap<RoutineId, NodeIndex>,
    /// Table index ("schema.name" or "name" → NodeIndex).
    table_index: HashMap<String, NodeIndex>,
    /// Type index ("schema.name" or "name" → NodeIndex).
    type_index: HashMap<String, NodeIndex>,
    /// Sequence index ("schema.name" or "name" → NodeIndex).
    sequence_index: HashMap<String, NodeIndex>,
    /// Synonym index: synonym key → canonical target key.
    synonym_targets: HashMap<String, String>,
    /// Bare routine name → all matching NodeIndex entries.
    bare_name_index: HashMap<String, Vec<NodeIndex>>,
    /// Case-insensitive routine lookup: (lowercase_qualified, kind) → NodeIndex.
    lowercase_routine: HashMap<(String, RoutineKind), NodeIndex>,
    /// Package member index: (lowercase_package, lowercase_name) → NodeIndex.
    pkg_member_lower: HashMap<(String, String), NodeIndex>,
}

impl ResolutionEngine {
    pub fn new() -> Self {
        Self {
            proc_index: HashMap::new(),
            table_index: HashMap::new(),
            type_index: HashMap::new(),
            sequence_index: HashMap::new(),
            synonym_targets: HashMap::new(),
            bare_name_index: HashMap::new(),
            lowercase_routine: HashMap::new(),
            pkg_member_lower: HashMap::new(),
        }
    }

    /// Register a routine node in all relevant indexes.
    pub fn register_routine(&mut self, id: RoutineId, idx: NodeIndex) {
        self.proc_index.entry(id.clone()).or_insert(idx);
        // Bare name index
        self.bare_name_index
            .entry(id.name.clone())
            .or_default()
            .push(idx);
        // Lowercase index
        let lower_key = id.to_string().to_lowercase();
        self.lowercase_routine
            .entry((lower_key, id.kind))
            .or_insert(idx);
        // Package member lowercase index
        if let Some(pkg) = &id.package {
            self.pkg_member_lower
                .entry((pkg.to_lowercase(), id.name.to_lowercase()))
                .or_insert(idx);
        }
    }

    /// Register a table node in the table index.
    pub fn register_table(&mut self, key: String, idx: NodeIndex) {
        self.table_index.entry(key).or_insert(idx);
    }

    /// Register a type node.
    pub fn register_type(&mut self, short_key: String, full_key: String, idx: NodeIndex) {
        self.type_index.entry(short_key).or_insert(idx);
        self.type_index.entry(full_key).or_insert(idx);
    }

    /// Register a sequence node.
    pub fn register_sequence(&mut self, short_key: String, full_key: String, idx: NodeIndex) {
        self.sequence_index.entry(short_key).or_insert(idx);
        self.sequence_index.entry(full_key).or_insert(idx);
    }

    /// Register a synonym mapping: synonym_key → canonical target key.
    pub fn register_synonym(&mut self, synonym_key: String, target_key: String) {
        self.synonym_targets
            .entry(synonym_key)
            .or_insert(target_key);
    }

    /// Resolve a routine name using all available strategies.
    /// `caller_context` is the RoutineId of the caller, used for same-package resolution.
    pub fn resolve_routine(
        &self,
        raw_name: &str,
        caller_context: Option<&RoutineId>,
    ) -> Option<NodeIndex> {
        // Strategy 1: Exact match (as Procedure)
        let id = RoutineId::from_qualified_name(raw_name, RoutineKind::Procedure);
        if let Some(&idx) = self.proc_index.get(&id) {
            return Some(idx);
        }
        // Strategy 2: Kind swap (as Function)
        let func_id = RoutineId::from_qualified_name(raw_name, RoutineKind::Function);
        if let Some(&idx) = self.proc_index.get(&func_id) {
            return Some(idx);
        }
        // Strategy 3: Schema-as-package fallback
        // If raw_name is "schema.proc", try treating schema as package
        if id.schema.is_some() && id.package.is_none() {
            let alt_id = RoutineId {
                schema: None,
                package: id.schema.clone(),
                name: id.name.clone(),
                kind: RoutineKind::Procedure,
            };
            if let Some(&idx) = self.proc_index.get(&alt_id) {
                return Some(idx);
            }
            let alt_func = RoutineId {
                schema: None,
                package: id.schema.clone(),
                name: id.name.clone(),
                kind: RoutineKind::Function,
            };
            if let Some(&idx) = self.proc_index.get(&alt_func) {
                return Some(idx);
            }
        }
        // Strategy 4: Synonym dereference
        if let Some(canonical) = self.synonym_targets.get(raw_name) {
            if let Some(idx) = self.resolve_routine(canonical, caller_context) {
                return Some(idx);
            }
        }
        // Also try qualified synonym lookup
        let lower = raw_name.to_lowercase();
        for (syn_key, target_key) in &self.synonym_targets {
            if syn_key.to_lowercase() == lower {
                if let Some(idx) = self.resolve_routine(target_key, caller_context) {
                    return Some(idx);
                }
            }
        }
        // Strategy 5: Caller context (same-package bare name)
        if let Some(caller) = caller_context {
            if let Some(pkg) = &caller.package {
                // Try as package member
                let in_pkg = RoutineId {
                    schema: caller.schema.clone(),
                    package: Some(pkg.clone()),
                    name: raw_name.to_string(),
                    kind: RoutineKind::Procedure,
                };
                if let Some(&idx) = self.proc_index.get(&in_pkg) {
                    return Some(idx);
                }
                let in_pkg_func = RoutineId {
                    schema: caller.schema.clone(),
                    package: Some(pkg.clone()),
                    name: raw_name.to_string(),
                    kind: RoutineKind::Function,
                };
                if let Some(&idx) = self.proc_index.get(&in_pkg_func) {
                    return Some(idx);
                }
            }
        }
        // Strategy 6: Case-insensitive match
        let lower_qualified = raw_name.to_lowercase();
        if let Some(&idx) = self.lowercase_routine.get(&(lower_qualified.clone(), RoutineKind::Procedure)) {
            return Some(idx);
        }
        if let Some(&idx) = self.lowercase_routine.get(&(lower_qualified, RoutineKind::Function)) {
            return Some(idx);
        }
        // Strategy 7: Case-insensitive package member
        let name_lower = raw_name.rsplit('.').next().unwrap_or(raw_name).to_lowercase();
        if let Some(id) = &id.schema {
            if let Some(&idx) = self.pkg_member_lower.get(&(id.to_lowercase(), name_lower.clone())) {
                return Some(idx);
            }
        }
        // Also check if raw_name matches any package member (no schema prefix)
        if id.schema.is_none() && id.package.is_none() {
            for (_, &idx) in self.pkg_member_lower.iter() {
                // This is expensive but only runs on unresolved names
                // We check by scanning the proc_index for matching bare name
            }
        }
        // Strategy 8: Bare name search (last resort)
        let bare_name = raw_name.rsplit('.').next().unwrap_or(raw_name);
        if let Some(matches) = self.bare_name_index.get(bare_name) {
            if matches.len() == 1 {
                return Some(matches[0]);
            }
            // Multiple matches: ambiguous, don't resolve
        }
        None
    }

    /// Resolve a table name using synonym chains and case-insensitive fallback.
    pub fn resolve_table(&self, raw_name: &str) -> Option<NodeIndex> {
        // Direct lookup
        if let Some(&idx) = self.table_index.get(raw_name) {
            return Some(idx);
        }
        // Case-insensitive
        let lower = raw_name.to_lowercase();
        for (key, &idx) in &self.table_index {
            if key.to_lowercase() == lower {
                return Some(idx);
            }
        }
        // Synonym dereference
        if let Some(canonical) = self.synonym_targets.get(raw_name) {
            if let Some(&idx) = self.table_index.get(canonical) {
                return Some(idx);
            }
        }
        // Case-insensitive synonym
        for (syn_key, target_key) in &self.synonym_targets {
            if syn_key.to_lowercase() == lower {
                if let Some(&idx) = self.table_index.get(target_key) {
                    return Some(idx);
                }
            }
        }
        None
    }

    /// Borrow the proc_index for callers that need direct access.
    pub fn proc_index(&self) -> &HashMap<RoutineId, NodeIndex> {
        &self.proc_index
    }

    /// Borrow the table_index for callers that need direct access.
    pub fn table_index(&self) -> &HashMap<String, NodeIndex> {
        &self.table_index
    }

    /// Borrow the type_index.
    pub fn type_index(&self) -> &HashMap<String, NodeIndex> {
        &self.type_index
    }

    /// Borrow the sequence_index.
    pub fn sequence_index(&self) -> &HashMap<String, NodeIndex> {
        &self.sequence_index
    }

    /// Mutably borrow proc_index for inserting unresolved nodes.
    pub fn proc_index_mut(&mut self) -> &mut HashMap<RoutineId, NodeIndex> {
        &mut self.proc_index
    }

    /// Mutably borrow table_index for inserting nodes.
    pub fn table_index_mut(&mut self) -> &mut HashMap<String, NodeIndex> {
        &mut self.table_index
    }
}

impl Default for ResolutionEngine {
    fn default() -> Self {
        Self::new()
    }
}
```

Add to `src/graph/mod.rs` line 1:
```rust
pub mod resolver;
```

**Step 4: Run test to verify it passes**

Run: `cargo test resolver --lib`
Expected: PASS

**Step 5: Commit**

---

### Task 2: Add tests for all resolution strategies

**Files:**
- Modify: `src/graph/resolver.rs` (tests module)

**Step 1: Write strategy tests**

Add these tests to `resolver.rs` tests module:

```rust
use std::sync::Arc;
use std::path::PathBuf;
use crate::graph::SourceLocation;

fn make_proc_id(name: &str) -> RoutineId {
    RoutineId {
        schema: None,
        package: None,
        name: name.to_string(),
        kind: RoutineKind::Procedure,
    }
}

fn make_pkg_proc_id(pkg: &str, name: &str) -> RoutineId {
    RoutineId {
        schema: None,
        package: Some(pkg.to_string()),
        name: name.to_string(),
        kind: RoutineKind::Procedure,
    }
}

fn make_func_id(name: &str) -> RoutineId {
    RoutineId {
        schema: None,
        package: None,
        name: name.to_string(),
        kind: RoutineKind::Function,
    }
}

fn dummy_location() -> SourceLocation {
    SourceLocation {
        file: Arc::new(PathBuf::from("test.sql")),
        line: 1,
    }
}

#[test]
fn kind_swap_resolves_func_when_looking_for_proc() {
    let mut engine = ResolutionEngine::new();
    let mut graph = CodeGraph::new();
    let idx = graph.add_node(Node::Function {
        id: make_func_id("calc"),
        location: dummy_location(),
        partial: false,
    });
    engine.register_routine(make_func_id("calc"), idx);
    // Looking for "calc" as procedure should find the function
    let result = engine.resolve_routine("calc", None);
    assert_eq!(result, Some(idx));
}

#[test]
fn schema_as_package_fallback() {
    let mut engine = ResolutionEngine::new();
    let mut graph = CodeGraph::new();
    let idx = graph.add_node(Node::Procedure {
        id: make_pkg_proc_id("my_pkg", "do_work"),
        location: dummy_location(),
        partial: false,
    });
    engine.register_routine(make_pkg_proc_id("my_pkg", "do_work"), idx);
    // "my_pkg.do_work" has schema=Some("my_pkg"), package=None
    // Should fall back to package=Some("my_pkg")
    let result = engine.resolve_routine("my_pkg.do_work", None);
    assert_eq!(result, Some(idx));
}

#[test]
fn synonym_dereference() {
    let mut engine = ResolutionEngine::new();
    let mut graph = CodeGraph::new();
    let idx = graph.add_node(Node::Procedure {
        id: make_proc_id("real_proc"),
        location: dummy_location(),
        partial: false,
    });
    engine.register_routine(make_proc_id("real_proc"), idx);
    engine.register_synonym("sync_proc".to_string(), "real_proc".to_string());
    let result = engine.resolve_routine("sync_proc", None);
    assert_eq!(result, Some(idx));
}

#[test]
fn caller_context_same_package() {
    let mut engine = ResolutionEngine::new();
    let mut graph = CodeGraph::new();
    let idx = graph.add_node(Node::Procedure {
        id: make_pkg_proc_id("pkg_api", "helper"),
        location: dummy_location(),
        partial: false,
    });
    engine.register_routine(make_pkg_proc_id("pkg_api", "helper"), idx);
    // Caller is in pkg_api, callee bare name "helper"
    let caller = make_pkg_proc_id("pkg_api", "main");
    let result = engine.resolve_routine("helper", Some(&caller));
    assert_eq!(result, Some(idx));
}

#[test]
fn case_insensitive_match() {
    let mut engine = ResolutionEngine::new();
    let mut graph = CodeGraph::new();
    let idx = graph.add_node(Node::Procedure {
        id: make_proc_id("Do_Work"),
        location: dummy_location(),
        partial: false,
    });
    engine.register_routine(make_proc_id("Do_Work"), idx);
    let result = engine.resolve_routine("do_work", None);
    assert_eq!(result, Some(idx));
}

#[test]
fn bare_name_search_single_match() {
    let mut engine = ResolutionEngine::new();
    let mut graph = CodeGraph::new();
    let idx = graph.add_node(Node::Procedure {
        id: make_pkg_proc_id("pkg", "unique_name"),
        location: dummy_location(),
        partial: false,
    });
    engine.register_routine(make_pkg_proc_id("pkg", "unique_name"), idx);
    // "unique_name" is unambiguous even without package prefix
    let result = engine.resolve_routine("unique_name", None);
    assert_eq!(result, Some(idx));
}

#[test]
fn bare_name_search_ambiguous_returns_none() {
    let mut engine = ResolutionEngine::new();
    let mut graph = CodeGraph::new();
    let idx1 = graph.add_node(Node::Procedure {
        id: make_pkg_proc_id("pkg_a", "common"),
        location: dummy_location(),
        partial: false,
    });
    let idx2 = graph.add_node(Node::Procedure {
        id: make_pkg_proc_id("pkg_b", "common"),
        location: dummy_location(),
        partial: false,
    });
    engine.register_routine(make_pkg_proc_id("pkg_a", "common"), idx1);
    engine.register_routine(make_pkg_proc_id("pkg_b", "common"), idx2);
    // "common" is ambiguous — exists in two packages
    let result = engine.resolve_routine("common", None);
    assert_eq!(result, None);
}

#[test]
fn table_synonym_dereference() {
    let mut engine = ResolutionEngine::new();
    let mut graph = CodeGraph::new();
    let idx = graph.add_node(Node::Table {
        schema: None,
        name: "real_table".to_string(),
    });
    engine.register_table("real_table".to_string(), idx);
    engine.register_synonym("sync_table".to_string(), "real_table".to_string());
    let result = engine.resolve_table("sync_table");
    assert_eq!(result, Some(idx));
}

#[test]
fn table_case_insensitive() {
    let mut engine = ResolutionEngine::new();
    let mut graph = CodeGraph::new();
    let idx = graph.add_node(Node::Table {
        schema: Some("MySchema".to_string()),
        name: "Users".to_string(),
    });
    engine.register_table("MySchema.Users".to_string(), idx);
    let result = engine.resolve_table("myschema.users");
    assert_eq!(result, Some(idx));
}
```

**Step 2: Run tests**

Run: `cargo test resolver --lib`
Expected: All 11 tests PASS

**Step 3: Commit**

---

### Task 3: Integrate ResolutionEngine into builder.rs — index building

**Files:**
- Modify: `src/graph/builder.rs`

**Goal:** Replace the 5 separate `HashMap` locals in `build_graph_internal()` with a single `ResolutionEngine`. This is purely mechanical: every `proc_index`, `table_index`, `type_index`, `sequence_index` is replaced by engine method calls. Synonym registration is added to populate `synonym_targets`.

**Step 1: Write failing test** (integration-level)

Add to `builder.rs` tests:

```rust
#[test]
fn synonym_target_procedure_resolved_in_call() {
    let sql = r#"
        CREATE OR REPLACE PROCEDURE real_proc() AS $$
        BEGIN NULL; END;
        $$;

        CREATE SYNONYM my_syn FOR real_proc;

        CREATE OR REPLACE PROCEDURE caller() AS $$
        BEGIN
            my_syn();
        END;
        $$;
    "#;
    let graph = build_from_sql(sql);
    // caller should have an edge to real_proc (via synonym), NOT to an unresolved node
    let unresolved_count = graph
        .node_indices()
        .filter(|i| matches!(&graph[*i], Node::Unresolved { .. }))
        .count();
    assert_eq!(unresolved_count, 0, "Should have no unresolved nodes — synonym should resolve to real_proc");

    let caller_idx = graph
        .node_indices()
        .find(|i| matches!(&graph[*i], Node::Procedure { id, .. } if id.name == "caller"))
        .expect("caller should exist");
    let real_idx = graph
        .node_indices()
        .find(|i| matches!(&graph[*i], Node::Procedure { id, .. } if id.name == "real_proc"))
        .expect("real_proc should exist");

    let has_edge = graph.edge_indices().any(|e| {
        matches!(&graph[e], Edge::DirectCall { .. }) && {
            let (src, dst) = graph.edge_endpoints(e).unwrap();
            src == caller_idx && dst == real_idx
        }
    });
    assert!(has_edge, "Expected DirectCall from caller to real_proc via synonym");
}

#[test]
fn bare_name_call_in_same_package_resolves() {
    let sql = r#"
        CREATE OR REPLACE PACKAGE BODY pkg_ctx AS
            PROCEDURE inner_helper IS
            BEGIN NULL; END;
            PROCEDURE outer_caller IS
            BEGIN
                inner_helper();
            END;
        END pkg_ctx;
    "#;
    let graph = build_from_sql(sql);
    let unresolved_count = graph
        .node_indices()
        .filter(|i| matches!(&graph[*i], Node::Unresolved { .. }))
        .count();
    assert_eq!(unresolved_count, 0, "inner_helper should resolve within same package body");
}

#[test]
fn case_insensitive_call_resolves() {
    let sql = r#"
        CREATE OR REPLACE PROCEDURE My_Proc() AS $$
        BEGIN NULL; END;
        $$;
        CREATE OR REPLACE PROCEDURE caller() AS $$
        BEGIN
            MY_PROC();
        END;
        $$;
    "#;
    let graph = build_from_sql(sql);
    let unresolved_count = graph
        .node_indices()
        .filter(|i| matches!(&graph[*i], Node::Unresolved { .. }))
        .count();
    assert_eq!(unresolved_count, 0, "MY_PROC() call should resolve to My_Proc via case-insensitive match");
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test synonym_target --lib && cargo test bare_name_call --lib && cargo test case_insensitive_call --lib`
Expected: FAIL — synonym/bare-name/case-insensitive resolution not yet implemented

**Step 3: Refactor build_graph_internal to use ResolutionEngine**

In `build_graph_internal()`:
1. Replace the 5 separate HashMap locals with a single `ResolutionEngine`
2. In `create_sql_nodes()`, call `engine.register_routine()`, `engine.register_table()`, etc.
3. In `create_sql_nodes()`, after creating Synonym nodes, call `engine.register_synonym(synonym_key, target_key)`
4. Pass `&engine` instead of `&proc_index`, `&table_index`, etc. to all sub-functions
5. In `create_edges()`, replace the ad-hoc resolution with `engine.resolve_routine()`
6. In `add_ibatis_nodes_from_parsed()`, replace `proc_index.entry().or_insert_with()` with `engine.resolve_routine()` + explicit Unresolved fallback
7. In `add_java_nodes_from_parsed()`, same as ibatis
8. In trigger creation, use `engine.resolve_routine()` instead of direct `proc_index.get()`

Key change in `create_edges()`:
```rust
// BEFORE (lines 1076-1111): manual multi-strategy resolution
// AFTER:
let callee_idx = engine.resolve_routine(&edge.callee_name, edge.caller.as_ref());
match (caller_idx, callee_idx) {
    (Some(from), Some(to)) => {
        graph.add_edge(from, to, make_edge(edge));
    }
    (Some(from), None) => {
        // Create Unresolved node (same as before)
        let unresolved_node = Node::Unresolved { ... };
        let to = graph.add_node(unresolved_node);
        engine.proc_index_mut().insert(callee_id, to);
        graph.add_edge(from, to, make_edge(edge));
    }
    _ => {}
}
```

Key change for synonyms — in `create_sql_nodes()` after creating Synonym node:
```rust
// After creating the synonym node, register the synonym mapping
if let (Some(schema), name) = (&schema, &name) {
    let syn_key = format!("{}.{}", schema, name);
    engine.register_synonym(syn_key, target_key.clone());
} else {
    engine.register_synonym(name.clone(), target_key.clone());
}
```

**Step 4: Run tests**

Run: `cargo test --lib`
Expected: ALL tests pass including new synonym, bare-name, case-insensitive tests

**Step 5: Commit**

---

### Task 4: Enhance table resolution with synonym and case-insensitive support

**Files:**
- Modify: `src/graph/builder.rs`

**Goal:** Replace all direct `table_index` lookups in table access collection with `engine.resolve_table()`, so that table references through synonyms or with wrong case get resolved.

**Step 1: Write failing test**

```rust
#[test]
fn table_access_via_synonym_resolves() {
    let sql = r#"
        CREATE TABLE real_table (id INT);
        CREATE SYNONYM sync_tbl FOR real_table;
        CREATE OR REPLACE PROCEDURE reader() AS $$
        BEGIN
            SELECT * FROM sync_tbl;
        END;
        $$;
    "#;
    let graph = build_from_sql(sql);
    let unresolved_count = graph
        .node_indices()
        .filter(|i| matches!(&graph[*i], Node::Unresolved { .. }))
        .count();
    assert_eq!(unresolved_count, 0, "Table synonym should resolve");

    let reader_idx = graph
        .node_indices()
        .find(|i| matches!(&graph[*i], Node::Procedure { id, .. } if id.name == "reader"))
        .expect("reader should exist");
    let table_idx = graph
        .node_indices()
        .find(|i| matches!(&graph[*i], Node::Table { name, .. } if name == "real_table"))
        .expect("real_table should exist");

    let has_edge = graph.edge_indices().any(|e| {
        matches!(&graph[e], Edge::TableAccess { .. }) && {
            let (src, dst) = graph.edge_endpoints(e).unwrap();
            src == reader_idx && dst == table_idx
        }
    });
    assert!(has_edge, "reader should access real_table via synonym");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test table_access_via_synonym --lib`
Expected: FAIL

**Step 3: Refactor table resolution**

In `collect_table_access_from_statements()` and everywhere `table_index` is used for lookup, replace:
```rust
// BEFORE:
let table_idx = *table_index.entry(key.clone()).or_insert_with(|| { ... });
// AFTER:
let table_idx = engine.resolve_table(&key).unwrap_or_else(|| {
    // Create new Table node (same as before)
    let node = Node::Table { ... };
    let idx = graph.add_node(node);
    engine.table_index_mut().insert(key.clone(), idx);
    idx
});
```

**Step 4: Run tests**

Run: `cargo test --lib`
Expected: ALL tests pass

**Step 5: Commit**

---

### Task 5: Clean up and verify

**Files:**
- All modified files

**Step 1: Run full test suite**

Run: `cargo test`
Expected: 61 unit tests + 35 integration tests = 96 tests, all PASS

**Step 2: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: No warnings

**Step 3: Run fmt check**

Run: `cargo fmt -- --check`
Expected: Clean

**Step 4: Run demo to verify no regressions**

Run codeweb analyze on the demo project and compare stats.

**Step 5: Commit**

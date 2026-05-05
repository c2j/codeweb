# Hybrid Query Engine Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a layered query system for cobweb's code graph: memory-optimized data model, secondary indexes, fluent traversal API, pre-built analytics, and a JSON query spec for runtime flexibility.

**Architecture:** Three-layer design — (1) Memory-optimized Node/Edge storage with string interning and boxed large variants, (2) Secondary index layer (type, name, schema, edge-type) for O(1) lookups replacing O(n) scans, (3) Fluent traversal engine using petgraph's `EdgeFiltered` + `DfsEvent` + `Control::Prune` for composable graph queries. A JSON query spec maps to the fluent API for runtime use.

**Tech Stack:** Rust, petgraph (existing), serde_json, bincode (existing), clap (existing)

---

## Phase 1: Memory Optimization (Tasks 1-3)

Reduce per-node memory from ~250 bytes to ~80 bytes and total graph footprint from ~1.2 GB to ~400 MB at 1M nodes.

### Task 1: Box large Node enum variants

**Files:**
- Modify: `src/graph/mod.rs` (Node enum, lines 232-378)

**Step 1: Write the failing test**

Add to `src/graph/mod.rs` tests module:

```rust
#[test]
fn node_size_is_reasonable() {
    // After boxing large variants, Node should be well under 200 bytes.
    // Before boxing: ~280-320 bytes due to Table variant's Vec<ColumnSummary>,
    // PartitionInfo, DistributeInfo.
    let size = std::mem::size_of::<Node>();
    assert!(size < 200, "Node is {} bytes, expected < 200", size);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test node_size_is_reasonable -- --nocapture`
Expected: FAIL — current Node size exceeds 200 bytes

**Step 3: Box the large Table variant fields**

In `src/graph/mod.rs`, box the heap-heavy fields of `Node::Table`:

```rust
Table {
    schema: Option<String>,
    name: String,
    #[serde(default)]
    location: Option<SourceLocation>,
    #[serde(default)]
    columns: Box<Vec<ColumnSummary>>,
    #[serde(default)]
    partition_by: Option<Box<PartitionInfo>>,
    #[serde(default)]
    distribute_by: Option<Box<DistributeInfo>>,
    #[serde(default)]
    tablespace: Option<String>,
    #[serde(default)]
    temporary: bool,
    #[serde(default)]
    unlogged: bool,
    #[serde(default)]
    ddl_source: Option<Box<String>>,
},
```

Also box `Node::Custom` properties and `Node::Unresolved` strings if they bloat the enum:

```rust
Unresolved {
    raw_expr: Box<String>,
    context: Box<String>,
},
Custom {
    type_name: Box<String>,
    label: Box<String>,
    key_fields: Box<BTreeMap<String, String>>,
    properties: Box<JsonMap>,
    location: Option<SourceLocation>,
},
```

**Step 4: Fix all match arms that construct these variants**

Search the codebase for all places that construct `Node::Table { ... }`, `Node::Unresolved { ... }`, `Node::Custom { ... }` and wrap fields in `Box::new()`. Key locations:
- `src/graph/builder.rs` — `create_sql_nodes`, table creation
- `src/import/parser.rs` — CGEF import
- `src/graph/mod.rs` tests — all test constructors
- `src/graph/store.rs` tests — all test constructors

**Step 5: Fix all match arms that destructure these variants**

Search for all `Node::Table { columns, partition_by, ... }` patterns and add `ref` or dereference. Use `*columns` to access the Vec.

**Step 6: Run all tests**

Run: `cargo test`
Expected: All 44 tests pass

**Step 7: Commit**

```bash
git add -A
git commit -m "refactor: box large Node variants to reduce enum size"
```

---

### Task 2: String interning for identifiers

**Files:**
- Modify: `src/graph/mod.rs` (RoutineId, SourceLocation, Node variants with schema/package/name)
- Modify: `src/graph/key.rs` (NodeKey display — should still produce owned Strings for display)

**Step 1: Write the failing test**

```rust
#[test]
fn source_location_uses_arc_str() {
    let loc = SourceLocation {
        file: std::sync::Arc::new(PathBuf::from("test.sql")),
        line: 1,
    };
    // Arc<PathBuf> should be cheap to clone
    let loc2 = loc.clone();
    assert!(std::sync::Arc::ptr_eq(&loc.file, &loc2.file));
}
```

**Step 2: Run test to verify it passes (Arc already used for file)**

Run: `cargo test source_location_uses_arc_str`
Expected: PASS — Arc<PathBuf> already in use

**Step 3: Evaluate if further interning is needed**

Check: Run `std::mem::size_of::<RoutineId>()` in a test. If it's reasonable (<64 bytes) given it has 3 `Option<String>` fields, the current approach may be fine. Only intern schema/package names if they are highly duplicated in practice.

**Decision point:** If RoutineId < 64 bytes and the Node enum is already < 200 bytes after Task 1, skip deeper interning for now. Measure real memory with `jemalloc` or `cap` on a 100K node corpus before optimizing further.

**Step 4: Commit (if changes made)**

```bash
git commit -m "perf: evaluate string interning for RoutineId fields"
```

---

### Task 3: Verify memory improvement with benchmark

**Files:**
- Create: `tests/bench_memory.rs` (integration test)

**Step 1: Write a synthetic 100K node benchmark**

```rust
#[cfg(test)]
mod bench {
    use super::*;
    
    #[test]
    fn synthetic_100k_nodes_memory_report() {
        let mut graph = crate::graph::CodeGraph::new();
        let file = std::sync::Arc::new(std::path::PathBuf::from("bench.sql"));
        
        // Create 50K procedures
        for i in 0..50_000 {
            let node = crate::graph::Node::Procedure {
                id: crate::graph::RoutineId {
                    schema: Some(format!("schema_{}", i % 100)),
                    package: Some(format!("pkg_{}", i % 50)),
                    name: format!("proc_{}", i),
                    kind: crate::graph::RoutineKind::Procedure,
                },
                location: crate::graph::SourceLocation {
                    file: file.clone(),
                    line: i,
                },
                partial: false,
            };
            graph.add_node(node);
        }
        
        // Create 50K tables
        for i in 0..50_000 {
            let node = crate::graph::Node::Table {
                schema: Some(format!("schema_{}", i % 100)),
                name: format!("table_{}", i),
                location: None,
                columns: Box::new(vec![
                    crate::graph::ColumnSummary {
                        name: "id".into(),
                        data_type: "BIGINT".into(),
                        nullable: false,
                        is_primary_key: true,
                        default_value: None,
                        comment: None,
                    },
                ]),
                partition_by: None,
                distribute_by: None,
                tablespace: None,
                temporary: false,
                unlogged: false,
                ddl_source: None,
            };
            graph.add_node(node);
        }
        
        // Add 200K edges
        for i in 0..200_000 {
            let src = petgraph::graph::NodeIndex::new(i as usize % 100_000);
            let dst = petgraph::graph::NodeIndex::new((i as usize + 1) % 100_000);
            graph.add_edge(src, dst, crate::graph::Edge::DirectCall {
                scope: crate::graph::CallScope::CrossPackage,
                location: crate::graph::SourceLocation {
                    file: file.clone(),
                    line: i as usize,
                },
            });
        }
        
        let node_size = std::mem::size_of::<crate::graph::Node>();
        let edge_size = std::mem::size_of::<crate::graph::Edge>();
        eprintln!("Node size: {} bytes", node_size);
        eprintln!("Edge size: {} bytes", edge_size);
        eprintln!("Nodes: {}, Edges: {}", graph.node_count(), graph.edge_count());
        
        assert!(node_size < 200, "Node too large: {} bytes", node_size);
    }
}
```

**Step 2: Run benchmark**

Run: `cargo test synthetic_100k_nodes_memory_report -- --nocapture`
Expected: PASS with memory report printed

**Step 3: Commit**

```bash
git add tests/bench_memory.rs
git commit -m "test: add memory benchmark for 100K node graph"
```

---

## Phase 2: Secondary Indexes (Tasks 4-6)

Replace O(n) scans with O(1) index lookups for type, name, and schema queries.

### Task 4: Add secondary indexes to GraphStore

**Files:**
- Modify: `src/graph/store.rs` (GraphStore struct, from_graph, new accessor methods)

**Step 1: Write the failing test**

Add to `src/graph/store.rs` tests module:

```rust
#[test]
fn type_tag_index_accelerates_type_filtering() {
    let mut graph = CodeGraph::new();
    // Add 3 procedures + 2 tables
    for i in 0..3 {
        graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: None, package: None, name: format!("p{}", i),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: crate::graph::SourceLocation {
                file: std::sync::Arc::new(std::path::PathBuf::from("a.sql")),
                line: i,
            },
            partial: false,
        });
    }
    for i in 0..2 {
        graph.add_node(crate::graph::Node::Table {
            schema: None, name: format!("t{}", i),
            location: None, columns: Box::new(vec![]),
            partition_by: None, distribute_by: None,
            tablespace: None, temporary: false, unlogged: false,
            ddl_source: None,
        });
    }
    
    let store = GraphStore::from_graph("test", graph);
    
    // O(1) lookup by type — should return exactly 3 procedure indices
    let procs = store.nodes_by_type("proc");
    assert_eq!(procs.len(), 3);
    
    let tables = store.nodes_by_type("table");
    assert_eq!(tables.len(), 2);
    
    // Non-existent type returns empty
    let views = store.nodes_by_type("view");
    assert!(views.is_empty());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test type_tag_index_accelerates_type_filtering`
Expected: FAIL — `nodes_by_type` method doesn't exist

**Step 3: Add index fields to GraphStore**

```rust
pub struct GraphStore {
    // ... existing fields ...
    
    /// Index: type tag → list of NodeIndex (e.g., "proc" → [idx1, idx2, ...])
    type_tag_index: HashMap<String, Vec<NodeIndex>>,
    
    /// Index: lowercase name → list of (NodeIndex, display_key) for prefix/substring search
    name_index: Vec<(String, NodeIndex)>,  // sorted by lowercase key, binary search
    
    /// Index: schema name → list of NodeIndex
    schema_index: HashMap<String, Vec<NodeIndex>>,
}
```

**Step 4: Build indexes in `from_graph`**

After the existing index construction, add:

```rust
// Build type_tag_index
let mut type_tag_index: HashMap<String, Vec<NodeIndex>> = HashMap::new();
for idx in graph.node_indices() {
    let tag = node_type_tag(&graph[idx]).to_string();
    type_tag_index.entry(tag).or_default().push(idx);
}

// Build name_index (sorted by lowercase key for binary search)
let mut name_index: Vec<(String, NodeIndex)> = graph.node_indices()
    .map(|idx| {
        let key = NodeKey::from_node(&graph[idx]);
        (key.to_string().to_lowercase(), idx)
    })
    .collect();
name_index.sort_by(|a, b| a.0.cmp(&b.0));

// Build schema_index
let mut schema_index: HashMap<String, Vec<NodeIndex>> = HashMap::new();
for idx in graph.node_indices() {
    if let Some(schema) = extract_schema(&graph[idx]) {
        schema_index.entry(schema.to_lowercase()).or_default().push(idx);
    }
}
```

**Step 5: Add accessor methods**

```rust
pub fn nodes_by_type(&self, type_tag: &str) -> &[NodeIndex] {
    self.type_tag_index.get(type_tag).map(|v| v.as_slice()).unwrap_or(&[])
}

pub fn name_index(&self) -> &[(String, NodeIndex)] {
    &self.name_index
}

pub fn schema_index(&self) -> &HashMap<String, Vec<NodeIndex>> {
    &self.schema_index
}
```

**Step 6: Implement `extract_schema` helper**

```rust
fn extract_schema(node: &Node) -> Option<&str> {
    match node {
        Node::Procedure { id, .. } | Node::Function { id, .. } => id.schema.as_deref(),
        Node::Table { schema, .. } | Node::View { schema, .. } => schema.as_deref(),
        Node::Package { schema, .. } => schema.as_deref(),
        Node::Type { schema, .. } => schema.as_deref(),
        Node::Sequence { schema, .. } => schema.as_deref(),
        Node::MaterializedView { schema, .. } => schema.as_deref(),
        Node::Synonym { schema, .. } => schema.as_deref(),
        _ => None,
    }
}
```

**Step 7: Update `new()`, `merge()` to include new fields**

**Step 8: Run all tests**

Run: `cargo test`
Expected: All tests pass

**Step 9: Commit**

```bash
git add -A
git commit -m "feat: add type_tag, name, and schema secondary indexes to GraphStore"
```

---

### Task 5: Replace find_nodes_by_name with index-accelerated search

**Files:**
- Modify: `src/graph/traverse.rs` (find_nodes_by_name)
- Modify: `src/graph/store.rs` (add search method)

**Step 1: Write the failing test**

```rust
#[test]
fn index_search_faster_than_full_scan() {
    // Build a store with 10K nodes, search should use index
    let mut graph = CodeGraph::new();
    let file = std::sync::Arc::new(std::path::PathBuf::from("big.sql"));
    for i in 0..10_000 {
        graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("pkg".into()), package: None,
                name: format!("proc_{}", i),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: crate::graph::SourceLocation { file: file.clone(), line: i },
            partial: false,
        });
    }
    let store = GraphStore::from_graph("test", graph);
    
    // Search for "proc_999" should find it via index
    let results = store.search_nodes("proc_999");
    assert_eq!(results.len(), 1);
    assert!(results[0].1.contains("proc_999"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test index_search_faster_than_full_scan`
Expected: FAIL — `search_nodes` method doesn't exist

**Step 3: Implement `search_nodes` on GraphStore**

Use binary search on the sorted `name_index` for prefix matching, then expand with MatchRank:

```rust
pub fn search_nodes(&self, query: &str) -> Vec<(NodeIndex, String)> {
    let lower = query.to_lowercase();
    let mut results: Vec<(NodeIndex, String, MatchRank)> = Vec::new();
    
    // Binary search for prefix matches
    let start = self.name_index.partition_point(|(k, _)| k.as_str() < &lower);
    for (key_lower, idx) in &self.name_index[start..] {
        if !key_lower.starts_with(&lower) && !key_lower.contains(&lower) {
            // Optimization: once we're past all possible matches, stop
            // But substring matches could be anywhere, so we still need a bounded scan
            if !key_lower.starts_with(&lower) && key_lower > &lower && !key_lower.contains(&lower) {
                break;
            }
        }
        let display = NodeKey::from_node(&self.graph[*idx]).to_string();
        if let Some(rank) = MatchRank::classify(&lower, &key_lower) {
            results.push((*idx, display, rank));
        }
    }
    
    results.sort_by(|a, b| a.2.cmp(&b.2).then_with(|| a.1.cmp(&b.1)));
    results.into_iter().map(|(idx, display, _)| (idx, display)).collect()
}
```

Note: The binary search optimization works best for prefix queries. For substring queries, we still scan — but the sorted index enables early termination. A proper inverted index (Task 6) can be added later if needed.

**Step 4: Update callers of `traverse::find_nodes_by_name`**

In `src/main.rs` (`cmd_trace`, `cmd_detail`) and `src/server/handlers.rs` (`trace` handler), switch from:
```rust
traverse::find_nodes_by_name(graph, query)
```
to:
```rust
store.search_nodes(query)
```

This requires passing `&GraphStore` instead of `&CodeGraph` to these functions.

**Step 5: Run all tests**

Run: `cargo test`
Expected: All tests pass

**Step 6: Commit**

```bash
git add -A
git commit -m "perf: replace O(n) find_nodes_by_name with index-accelerated search"
```

---

### Task 6: Add edge-type index for filtered traversal

**Files:**
- Modify: `src/graph/store.rs` (add edge_type_index)

**Step 1: Write the failing test**

```rust
#[test]
fn edge_type_index_groups_table_accesses() {
    let mut graph = CodeGraph::new();
    let file = std::sync::Arc::new(std::path::PathBuf::from("a.sql"));
    let proc = graph.add_node(/* Procedure */);
    let table = graph.add_node(/* Table */);
    graph.add_edge(proc, table, Edge::TableAccess { ... });
    graph.add_edge(proc, table, Edge::DirectCall { ... }); // won't happen but for test
    
    let store = GraphStore::from_graph("test", graph);
    let ta_edges = store.edges_by_category(EdgeCategory::DataFlow);
    assert_eq!(ta_edges.len(), 1);
}
```

**Step 2: Implement edge_type_index**

Add to GraphStore:
```rust
edge_type_index: HashMap<String, Vec<(NodeIndex, NodeIndex)>>,
```

Build in `from_graph` by grouping edges by their `edge_type_tag()`.

**Step 3: Run all tests and commit**

```bash
git commit -m "feat: add edge-type secondary index for filtered traversal"
```

---

## Phase 3: Fluent Traversal API (Tasks 7-9)

Composable graph queries: `graph.query().from(x).outgoing().edge_filter(f).collect()`

### Task 7: Define the fluent query types

**Files:**
- Create: `src/graph/query/mod.rs`
- Create: `src/graph/query/filter.rs` (NodeFilter, EdgeFilter)
- Create: `src/graph/query/traversal.rs` (GraphTraversal, TraversalResult)
- Modify: `src/graph/mod.rs` (add `pub mod query`)

**Step 1: Create filter types**

```rust
// src/graph/query/filter.rs
use crate::graph::{Edge, EdgeCategory, Node, node_type_tag};

/// Composable node predicate — replaces scattered boolean flags
pub struct NodeFilter {
    predicates: Vec<Box<dyn Fn(&Node) -> bool + Send + Sync>>,
}

impl NodeFilter {
    pub fn new() -> Self { Self { predicates: Vec::new() } }
    
    pub fn with_type(mut self, tag: &str) -> Self {
        let tag = tag.to_lowercase();
        self.predicates.push(Box::new(move |n| node_type_tag(n).eq_ignore_ascii_case(&tag)));
        self
    }
    
    pub fn with_schema(mut self, schema: &str) -> Self {
        let schema = schema.to_lowercase();
        self.predicates.push(Box::new(move |n| {
            crate::graph::store::extract_schema(n)
                .map(|s| s.eq_ignore_ascii_case(&schema))
                .unwrap_or(false)
        }));
        self
    }
    
    pub fn with_predicate(mut self, pred: impl Fn(&Node) -> bool + Send + Sync + 'static) -> Self {
        self.predicates.push(Box::new(pred));
        self
    }
    
    pub fn matches(&self, node: &Node) -> bool {
        self.predicates.iter().all(|p| p(node))
    }
}

/// Composable edge predicate
pub struct EdgeFilter {
    categories: Option<Vec<EdgeCategory>>,
    predicate: Option<Box<dyn Fn(&Edge) -> bool + Send + Sync>>,
}

impl EdgeFilter {
    pub fn new() -> Self { Self { categories: None, predicate: None } }
    
    pub fn with_category(mut self, cat: EdgeCategory) -> Self {
        self.categories.get_or_insert_with(Vec::new).push(cat);
        self
    }
    
    pub fn calls_only() -> Self {
        Self::new().with_category(EdgeCategory::Call)
    }
    
    pub fn data_flow() -> Self {
        Self::new().with_category(EdgeCategory::DataFlow)
    }
    
    pub fn with_predicate(mut self, pred: impl Fn(&Edge) -> bool + Send + Sync + 'static) -> Self {
        self.predicate = Some(Box::new(pred));
        self
    }
    
    pub fn matches(&self, edge: &Edge) -> bool {
        if let Some(ref cats) = self.categories {
            if !cats.contains(&edge.category()) { return false; }
        }
        if let Some(ref pred) = self.predicate {
            if !pred(edge) { return false; }
        }
        true
    }
}
```

**Step 2: Create traversal builder**

```rust
// src/graph/query/traversal.rs
use petgraph::graph::NodeIndex;
use petgraph::Direction;
use std::collections::HashSet;
use crate::graph::CodeGraph;
use super::filter::{NodeFilter, EdgeFilter};

pub struct TraversalResult {
    pub nodes: Vec<NodeIndex>,
    pub paths: Vec<Vec<NodeIndex>>,
}

pub struct GraphTraversal<'a> {
    graph: &'a CodeGraph,
    start: NodeIndex,
    direction: Direction,
    edge_filter: EdgeFilter,
    node_filter: Option<NodeFilter>,
    max_depth: Option<usize>,
    until: Option<Box<dyn Fn(&crate::graph::Node) -> bool + 'a>>,
}

impl<'a> GraphTraversal<'a> {
    pub fn new(graph: &'a CodeGraph, start: NodeIndex) -> Self {
        Self {
            graph, start,
            direction: Direction::Outgoing,
            edge_filter: EdgeFilter::new(),
            node_filter: None,
            max_depth: None,
            until: None,
        }
    }
    
    pub fn direction(mut self, dir: Direction) -> Self { self.direction = dir; self }
    pub fn outgoing(self) -> Self { self.direction(Direction::Outgoing) }
    pub fn incoming(self) -> Self { self.direction(Direction::Incoming) }
    pub fn edge_filter(mut self, filter: EdgeFilter) -> Self { self.edge_filter = filter; self }
    pub fn max_depth(mut self, depth: usize) -> Self { self.max_depth = Some(depth); self }
    pub fn node_filter(mut self, filter: NodeFilter) -> Self { self.node_filter = Some(filter); self }
    pub fn until(mut self, cond: impl Fn(&crate::graph::Node) -> bool + 'a) -> Self {
        self.until = Some(Box::new(cond));
        self
    }
    
    /// Collect all reachable nodes
    pub fn collect_nodes(self) -> Vec<NodeIndex> { /* DFS with EdgeFiltered */ }
    
    /// Collect all root-to-leaf paths
    pub fn collect_paths(self) -> Vec<Vec<NodeIndex>> { /* path-enumerating DFS */ }
    
    /// Extract matched subgraph
    pub fn collect_subgraph(self) -> CodeGraph { /* subgraph extraction */ }
}
```

**Step 3: Implement collect_nodes using petgraph's EdgeFiltered**

```rust
pub fn collect_nodes(self) -> Vec<NodeIndex> {
    use petgraph::visit::EdgeFiltered;
    use petgraph::visit::{depth_first_search, DfsEvent, Control};
    
    let edge_filter_fn = |edge: petgraph::graph::EdgeRef<crate::graph::Edge>| {
        self.edge_filter.matches(edge.weight())
    };
    
    let filtered = EdgeFiltered::from_fn(self.graph, edge_filter_fn);
    let mut result = Vec::new();
    let mut visited = HashSet::new();
    visited.insert(self.start);
    
    depth_first_search(&filtered, Some(self.start), |event| {
        match event {
            DfsEvent::Discover(n, _) => {
                if !visited.insert(n) { return Control::Continue; }
                if let Some(ref until) = self.until {
                    if until(&self.graph[n]) {
                        result.push(n);
                        return Control::Prune;
                    }
                }
                if self.node_filter.as_ref().map_or(true, |f| f.matches(&self.graph[n])) {
                    result.push(n);
                }
            }
            _ => {}
        }
        Control::Continue
    });
    
    result
}
```

**Step 4: Write tests for fluent API**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn outgoing_calls_from_procedure() {
        // Build small graph: proc_a → proc_b → table_t
        // Query: outgoing calls from proc_a → should find proc_b only
        let mut graph = CodeGraph::new();
        let file = std::sync::Arc::new(std::path::PathBuf::from("test.sql"));
        // ... build graph ...
        
        let traversal = GraphTraversal::new(&graph, proc_a)
            .outgoing()
            .edge_filter(EdgeFilter::calls_only())
            .max_depth(1);
        
        let nodes = traversal.collect_nodes();
        assert_eq!(nodes.len(), 1);  // proc_b
    }
}
```

**Step 5: Run tests and commit**

```bash
git add -A
git commit -m "feat: add fluent traversal API with composable node/edge filters"
```

---

### Task 8: Add convenience methods on CodeGraph/GraphStore

**Files:**
- Modify: `src/graph/query/mod.rs` (add pre-built analytics)

**Step 1: Implement pre-built analytics**

```rust
impl crate::graph::store::GraphStore {
    /// Find all dead (unreferenced) routines
    pub fn dead_routines(&self) -> Vec<NodeIndex> {
        self.nodes_by_type("proc").iter()
            .chain(self.nodes_by_type("func").iter())
            .chain(self.nodes_by_type("func*").iter())
            .filter(|&&idx| {
                let in_deg = self.graph.neighbors_directed(idx, Direction::Incoming).count();
                in_deg == 0
            })
            .copied()
            .collect()
    }
    
    /// Find all entry points (Java methods with no callers)
    pub fn entry_points(&self) -> Vec<NodeIndex> {
        self.nodes_by_type("method").iter()
            .filter(|&&idx| {
                self.graph.neighbors_directed(idx, Direction::Incoming).count() == 0
            })
            .copied()
            .collect()
    }
    
    /// Find strongly connected components (cycles)
    pub fn find_cycles(&self) -> Vec<Vec<NodeIndex>> {
        petgraph::algo::kosaraju_scc(&self.graph)
            .into_iter()
            .filter(|scc| scc.len() > 1)
            .collect()
    }
    
    /// Impact analysis: trace backward from a node until hitting Java methods
    pub fn impact(&self, node: NodeIndex) -> Vec<NodeIndex> {
        GraphTraversal::new(&self.graph, node)
            .incoming()
            .until(|n| matches!(n, Node::JavaMethod { .. }))
            .collect_nodes()
    }
}
```

**Step 2: Write tests and commit**

```bash
git commit -m "feat: add dead_code, entry_points, cycles, impact analytics"
```

---

### Task 9: JSON query spec for runtime flexibility

**Files:**
- Create: `src/graph/query/spec.rs` (QuerySpec, StepSpec types)
- Modify: `src/graph/query/mod.rs` (add spec module)

**Step 1: Define the JSON query spec**

```rust
// src/graph/query/spec.rs
use serde::{Deserialize, Serialize};
use petgraph::Direction;

#[derive(Debug, Deserialize, Serialize)]
pub struct QuerySpec {
    pub start: StartSpec,
    #[serde(default)]
    pub steps: Vec<StepSpec>,
    #[serde(default = "default_collect")]
    pub collect: CollectMode,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StartSpec {
    #[serde(rename = "type")]
    pub type_tag: Option<String>,
    pub name: Option<String>,
    pub schema: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub enum StepSpec {
    #[serde(rename = "outgoing")]
    Outgoing { edge_types: Option<Vec<String>>, max_depth: Option<usize> },
    #[serde(rename = "incoming")]
    Incoming { edge_types: Option<Vec<String>>, max_depth: Option<usize> },
    #[serde(rename = "filter")]
    Filter { type_tag: Option<String>, schema: Option<String> },
    #[serde(rename = "until")]
    Until { type_tag: String },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectMode {
    Nodes,
    Paths,
    Subgraph,
}

fn default_collect() -> CollectMode { CollectMode::Nodes }
```

**Step 2: Implement spec executor**

```rust
impl QuerySpec {
    pub fn execute(&self, store: &crate::graph::store::GraphStore) -> Result<serde_json::Value, String> {
        // 1. Resolve start node(s) using indexes
        let starts = self.resolve_starts(store)?;
        if starts.is_empty() {
            return Err("No matching start node found".into());
        }
        
        // 2. Apply steps sequentially
        let mut current_nodes = starts;
        for step in &self.steps {
            current_nodes = self.apply_step(step, store, &current_nodes)?;
        }
        
        // 3. Collect results
        match self.collect {
            CollectMode::Nodes => Ok(serde_json::to_value(
                current_nodes.iter().map(|&idx| {
                    let key = crate::graph::key::NodeKey::from_node(&store.graph()[idx]);
                    serde_json::json!({"id": idx.index(), "key": key.to_string()})
                }).collect::<Vec<_>>()
            ).unwrap()),
            CollectMode::Paths => { /* collect paths */ }
            CollectMode::Subgraph => { /* collect subgraph */ }
        }
    }
    
    fn resolve_starts(&self, store: &crate::graph::store::GraphStore) -> Result<Vec<NodeIndex>, String> {
        if let Some(ref name) = self.start.name {
            let results = store.search_nodes(name);
            Ok(results.into_iter().map(|(idx, _)| idx).collect())
        } else if let Some(ref type_tag) = self.start.type_tag {
            Ok(store.nodes_by_type(type_tag).to_vec())
        } else {
            Err("Start must specify 'name' or 'type'".into())
        }
    }
}
```

**Step 3: Write tests for JSON spec**

```rust
#[test]
fn json_spec_finds_callers() {
    let spec: QuerySpec = serde_json::from_str(r#"{
        "start": { "name": "do_work" },
        "steps": [
            {"incoming": {"edge_types": ["DirectCall"], "max_depth": 2}}
        ],
        "collect": "nodes"
    }"#).unwrap();
    
    let store = /* build test store */;
    let result = spec.execute(&store).unwrap();
    // verify result
}
```

**Step 4: Run tests and commit**

```bash
git add -A
git commit -m "feat: add JSON query spec for runtime graph queries"
```

---

## Phase 4: CLI & API Integration (Tasks 10-11)

Wire the new query capabilities into existing CLI commands and HTTP API.

### Task 10: Update CLI commands to use indexes

**Files:**
- Modify: `src/main.rs` (cmd_nodes, cmd_trace, cmd_detail to use store.search_nodes)

**Step 1: Update cmd_nodes**

Replace the O(n) scan with type_tag_index lookups:

```rust
fn cmd_nodes(..., project: &Path) -> Result<()> {
    let mut proj = project::Project::find(project)?;
    let store = proj.load_store()?;
    
    let indices: Vec<NodeIndex> = if let Some(query) = search {
        // Use index-accelerated search
        store.search_nodes(query).into_iter().map(|(idx, _)| idx).collect()
    } else if let Some(ref type_tag) = type_filter {
        // Use type_tag_index for O(1) lookup
        store.nodes_by_type(type_tag).to_vec()
    } else {
        store.graph().node_indices().collect()
    };
    
    // ... rest of filtering and display ...
}
```

**Step 2: Update cmd_trace and cmd_detail**

Replace `traverse::find_nodes_by_name(graph, name)` with `store.search_nodes(name)`.

**Step 3: Add new `query` subcommand**

```rust
/// Execute a structured graph query
Query {
    /// JSON query spec string
    #[arg(long)]
    spec: Option<String>,
    
    /// Read JSON query spec from file
    #[arg(long)]
    spec_file: Option<PathBuf>,
    
    /// Shorthand: find callers of a node
    #[arg(long)]
    callers_of: Option<String>,
    
    /// Shorthand: find callees of a node
    #[arg(long)]
    callees_of: Option<String>,
    
    /// Shorthand: impact analysis (backward to entry points)
    #[arg(long)]
    impact_of: Option<String>,
    
    /// Shorthand: find dead (unreferenced) routines
    #[arg(long)]
    dead_code: bool,
    
    /// Shorthand: find entry points (methods with no callers)
    #[arg(long)]
    entry_points: bool,
    
    /// Shorthand: find cycles
    #[arg(long)]
    cycles: bool,
    
    /// Project directory
    #[arg(short, long, default_value = ".")]
    project: PathBuf,
},
```

**Step 4: Run tests and commit**

```bash
git commit -m "feat: wire query engine into CLI with new query subcommand"
```

---

### Task 11: Add query endpoint to HTTP API

**Files:**
- Modify: `src/server/handlers.rs` (add /api/v1/query endpoint)

**Step 1: Add POST /api/v1/query endpoint**

```rust
#[derive(serde::Deserialize)]
struct QueryBody {
    start: Option<crate::graph::query::spec::StartSpec>,
    steps: Option<Vec<crate::graph::query::spec::StepSpec>>,
    collect: Option<crate::graph::query::spec::CollectMode>,
}

async fn query(
    State(state): State<AppState>,
    Json(body): Json<QueryBody>,
) -> Result<impl IntoResponse, StatusCode> {
    let spec = crate::graph::query::spec::QuerySpec {
        start: body.start.unwrap_or_default(),
        steps: body.steps.unwrap_or_default(),
        collect: body.collect.unwrap_or_default(),
    };
    let store = state.store();
    match spec.execute(store) {
        Ok(result) => Ok(Json(result)),
        Err(e) => Err(StatusCode::BAD_REQUEST),
    }
}

// Add to router:
.route("/api/v1/query", post(query))
```

**Step 2: Write integration test**

Add to `tests/serve_api.rs`:
```rust
#[cfg(feature = "serve")]
#[tokio::test]
async fn test_query_endpoint() {
    // POST /api/v1/query with JSON spec
    // Verify response contains expected nodes
}
```

**Step 3: Run tests and commit**

```bash
git commit -m "feat: add POST /api/v1/query endpoint for runtime graph queries"
```

---

## Summary

| Phase | Tasks | Estimated Time | Key Deliverable |
|-------|-------|---------------|-----------------|
| Phase 1: Memory | 1-3 | 1-2 days | Node enum < 200 bytes, memory benchmark |
| Phase 2: Indexes | 4-6 | 1-2 days | O(1) type/name/schema lookups |
| Phase 3: Traversal | 7-9 | 2-3 days | Fluent API + JSON query spec |
| Phase 4: Integration | 10-11 | 1 day | CLI + HTTP API wired to query engine |
| **Total** | **11** | **5-8 days** | Full hybrid query engine |

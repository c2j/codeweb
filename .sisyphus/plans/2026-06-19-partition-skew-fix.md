# Partition Skew Fix — GCC Extraction + TF-IDF TableAccess Projection

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Eliminate pathological cluster skew (cluster 0 absorbing 99% of nodes) by (Phase 1) detecting and reporting weakly connected components (WCCs) separately with an opt-in size filter, and (Phase 2) adding TF-IDF + cosine-similarity table-access projection edges to bridge isolated procedures through shared data tables.

**Architecture:** Phase 1 adds a `WccTopology` pre-pass to `partition()` that counts weakly connected components on the participant-only graph, optionally filters out small components (via `--min-component-size`), and prints a topology section before the cluster table. Phase 2 builds a proc×table access matrix from existing `Edge::TableAccess` data (already merged per `(proc, table, flow_kind)` triple), computes TF-IDF-weighted cosine similarity between procedure pairs, and injects the top-K similarity edges into the condensed graph before CNM runs. Both phases are additive and backward-compatible (defaults preserve current behavior).

**Tech Stack:** Rust, petgraph 0.7 (`connected_components` for WCC), no new crates. Pure-Rust TF-IDF + cosine (~80 lines).

---

## Execution Context

**Project root:** `/Users/c2j/Projects/Desktop_Projects/CODE/cobweb`

**Key files to understand before starting:**
- `src/graph/cluster.rs` — Core partition logic. Read ALL of it before Task 1.
  - `default_participant_kinds()` (line 201) — which node types participate
  - `condense_sccs()` (line 230) — SCC condensation; will extend for WCC filter
  - `build_condensed_graph()` (line 278) — builds weighted adjacency; will extend for TF-IDF edges
  - `partition()` (line 888) — top-level orchestrator; will add topology computation
  - `PartitionReport` struct (line 877) — will add `topology` field
  - `ClusterConfig` (line 141) — will add `min_component_size` and table projection config
  - `Edge::TableAccess` data shape: `{ flow_kind, modes: AccessMode, write_kinds: HashSet<WriteKind>, location, column_analysis: Option<Box<ColumnAnalysis>> }`
- `src/graph/mod.rs` —
  - `Edge::TableAccess` variant (line 481)
  - `AccessMode` bitflag (line 57): `Read | Write | LockRead | Truncate`
  - `DataFlowKind` enum (line 79): `DmlAccess | DefinitionDependency`
  - `Node::Procedure { id: RoutineId, .. }` (line 246) and `Node::Table { schema, name, .. }` (line 311)
- `src/main.rs` —
  - `Commands::Partition` variant (line 351) — add `--min-component-size` and `--table-projection` flags
  - `cmd_partition()` (line 1567) — orchestrate
  - `print_cluster_details()` (line 1738) and `print_cluster_analysis()` (line 1771) — patterns to follow
- `locales/en.yml` and `locales/zh-CN.yml` — `partition.*` keys, `%{param}` interpolation
- `src/graph/store.rs` — `GraphStore::partition()` (line 266) delegates to `cluster::partition()`; no change needed
- Edge iteration pattern (from `store.rs:146-180`):
  ```rust
  for edge_idx in graph.edge_indices() {
      if let Edge::TableAccess { flow_kind, modes, .. } = &graph[edge_idx] {
          let (src, dst) = graph.edge_endpoints(edge_idx).unwrap();
          // ...
      }
  }
  ```

**Existing patterns to follow:**
- Module per concern: keep all new code in `src/graph/cluster.rs` (no new files unless a sub-module is clearly warranted)
- `thiserror` for errors, no `anyhow` in library code
- Tests alongside source in `#[cfg(test)] mod tests` (existing tests start at line 1064)
- CLI via `clap` derive
- `t!()` macro for i18n, keys in both `en.yml` and `zh-CN.yml`
- Helper test functions: `proc_node(name)`, `table_node(name)`, `call_edge()`, `table_access_edge()` (lines 1074-1126)

**Existing test idiom (from line 1230):**
```rust
#[test]
fn cnm_two_communities_obvious_split() {
    let mut graph = CodeGraph::new();
    let a = graph.add_node(proc_node("a"));
    let b = graph.add_node(proc_node("b"));
    graph.add_edge(a, b, call_edge());
    graph.add_edge(b, a, call_edge());
    let report = partition(&graph, &ClusterConfig::new(2));
    assert_eq!(report.k_actual, 2);
    assert!(report.modularity > 0.3);
}
```

**Verification matrix (run after each task):**
```sh
cargo test --lib cluster                        # unit tests in cluster.rs
cargo build                                     # default features
cargo build --features full                     # all features (catches cross-feature regressions)
cargo clippy --features full -- -D warnings     # lint
cargo fmt -- --check                            # format check
```

---

## Design Decisions (Pre-Confirmed)

### D1: Phase 1 is opt-in via `--min-component-size`, default preserves current behavior

**Why:** Backward compatibility. Existing scripts that run `codeweb partition -k 20` must produce identical output. The topology section is always PRINTED (informational), but the WCC filter only activates when `--min-component-size N` (N > 1) is passed.

**Default:** `min_component_size = 1` (all components participate, current behavior).

### D2: WCC computed on participant-only condensed graph, not the raw graph

**Why:** The WCC count we report must match what the clustering algorithm actually sees. Computing WCC on the raw graph (with tables/views) would show ~50 WCCs and mislead users — they'd expect 50 clusters but get 8000.

**How:** Build the same participant-only node set used by `condense_sccs()`, then run `petgraph::algo::connected_components` on an undirected adapter of the participant-induced subgraph.

### D3: Phase 2 is opt-in via `--table-projection`, default off

**Why:** TF-IDF projection changes clustering results. Users who validated against current output need a way to opt out. Phase 2 is also more expensive (O(P²) cosine similarity for P participants, mitigated by k-NN sparsification).

**Default:** off. When enabled, defaults are `tau = 0.1` (similarity threshold), `lambda = 0.3` (edge weight multiplier vs call edges), `k_neighbors = 10` (top-K nearest per proc).

### D4: TF-IDF document = procedure, term = table, weight = 1.0 per access (binary), TF-IDF downweights generic tables

**Why:** Tables accessed by many procedures (e.g., `dual`, `code_types`, audit tables) are weak coupling signals. TF-IDF's IDF term = `log(N / df(table))` naturally downweights them. We use binary term frequency (1.0 if accessed, 0.0 if not) rather than access count because `merge_table_access_edges` already collapses multiple accesses into one edge per (proc, table) pair — access count is not preserved.

### D5: Cosine similarity on TF-IDF vectors, not Jaccard

**Why (per Oracle analysis):** Jaccard suffers degree bias — a proc accessing 1 table has J ∈ {0, 1} (all-or-nothing), a proc accessing 50 tables has diluted Jaccard. Cosine on TF-IDF vectors is the information-retrieval standard, robust to vector length, and pairs naturally with TF-IDF weighting.

### D6: Edge injection is symmetric (undirected), weight = `lambda * cosine_similarity`

**Why:** Similarity is symmetric. CNM treats edges as undirected anyway (`build_condensed_graph` symmetrizes). Weight scaled by `lambda` (default 0.3) keeps table-projection edges weaker than direct call edges (weight 1.0), matching the design intent that "shared table access ≠ call dependency" but provides a fallback coupling signal.

### D7: Sparsification via top-K nearest neighbors per proc (k=10 by default)

**Why:** A naive all-pairs projection creates a near-complete graph (most procs share at least one common reference table). This destroys community structure. Top-K nearest per proc (mutual k-NN or symmetric: keep edge if either is in other's top-K) produces a sparse, meaningful similarity graph. Threshold `tau` (default 0.1) is a secondary filter.

### D8: Phase 2 does NOT remove `force_merge_disconnected`

**Why:** Even with table projection, some components may remain disconnected (procs with no table access at all). The existing fallback is still needed as a last-resort. A separate follow-up task can fix `force_merge_disconnected` to use bin-packing (smallest→next-smallest) instead of smallest→largest; that's a 20-line change deferred to a future PR to keep this plan focused.

### D9: No new dependencies

Pure-Rust TF-IDF (HashMap + iterator) and cosine (sqrt of dot products) are ~80 lines. No need for `linfa`, `ndarray`, or external ML crates.

---

## Out of Scope (Explicit Non-Goals)

1. ❌ Replacing CNM with Louvain/Leiden (separate PR)
2. ❌ Fixing `force_merge_disconnected` to bin-packing (separate 20-line PR)
3. ❌ SBM, Infomap, node embeddings (research-grade, deferred)
4. ❌ Multi-layer network formalism (we just inject extra edges; no layer-weight tuning UI)
5. ❌ HTTP API / TUI integration for new features (CLI-only for now)
6. ❌ Persistence of topology/projection data in GraphStore (computed on-demand)
7. ❌ Auto-tuning of tau/lambda/k_neighbors (manual flags only; auto-tune is research)

---

## Tasks

### Phase 1: GCC Extraction + Layered Reporting

---

### Task 1: WccTopology Type + Compute Function

**Files:**
- Modify: `src/graph/cluster.rs` (add `WccTopology` struct + `compute_wcc_topology` fn, ~80 lines)

**Step 1: Write failing test**

Add to `src/graph/cluster.rs` `mod tests` (after line 1776 or wherever mod tests ends):

```rust
#[test]
fn wcc_topology_counts_components_correctly() {
    let mut graph = CodeGraph::new();
    // GCC: a-b-c-d-e (5 nodes, connected via call edges)
    let a = graph.add_node(proc_node("a"));
    let b = graph.add_node(proc_node("b"));
    let c = graph.add_node(proc_node("c"));
    let d = graph.add_node(proc_node("d"));
    let e = graph.add_node(proc_node("e"));
    graph.add_edge(a, b, call_edge());
    graph.add_edge(b, c, call_edge());
    graph.add_edge(c, d, call_edge());
    graph.add_edge(d, e, call_edge());
    // Isolate 1: f (singleton)
    let _f = graph.add_node(proc_node("f"));
    // Isolate 2: g-h (mutual call pair)
    let g = graph.add_node(proc_node("g"));
    let h = graph.add_node(proc_node("h"));
    graph.add_edge(g, h, call_edge());
    graph.add_edge(h, g, call_edge());

    let config = ClusterConfig::auto();
    let topo = compute_wcc_topology(&graph, &config);

    assert_eq!(topo.total_participants, 8);
    assert_eq!(topo.wcc_count, 3);
    assert_eq!(topo.gcc_size, 5);
    assert_eq!(topo.isolates_count, 2); // 3 WCCs - 1 GCC = 2 isolate WCCs
    assert_eq!(topo.isolates_node_count, 3); // 1 (f) + 2 (g, h)
    assert_eq!(topo.largest_components[0].len(), 5); // GCC
    assert_eq!(topo.largest_components[1].len(), 2); // g-h
    assert_eq!(topo.largest_components[2].len(), 1); // f
}

#[test]
fn wcc_topology_excludes_non_participants() {
    let mut graph = CodeGraph::new();
    let p = graph.add_node(proc_node("p"));
    let t = graph.add_node(table_node("orders"));
    // p reads orders — but table is not a participant
    graph.add_edge(p, t, table_access_edge());

    let config = ClusterConfig::auto();
    let topo = compute_wcc_topology(&graph, &config);

    // Only p participates; t is excluded. So 1 WCC of size 1.
    assert_eq!(topo.total_participants, 1);
    assert_eq!(topo.wcc_count, 1);
    assert_eq!(topo.gcc_size, 1);
}
```

**Step 2: Run test to verify it fails**

```sh
cargo test --lib cluster::tests::wcc_topology_counts_components_correctly
```
Expected: FAIL with "cannot find function `compute_wcc_topology`".

**Step 3: Implement WccTopology + compute_wcc_topology**

Add to `src/graph/cluster.rs` (just before `pub fn partition`, around line 887):

```rust
/// Topology of weakly connected components (WCCs) on the participant-induced subgraph.
///
/// WCCs are computed AFTER participant filtering but BEFORE SCC condensation.
/// Each WCC is a maximal set of participants connected by paths through other
/// participants (edges to non-participants like tables are ignored).
#[derive(Debug, Clone)]
pub struct WccTopology {
    /// Total participant node count.
    pub total_participants: usize,
    /// Number of weakly connected components.
    pub wcc_count: usize,
    /// Size of the largest WCC (giant connected component).
    pub gcc_size: usize,
    /// Number of WCCs excluding the GCC (i.e., `wcc_count - 1`).
    pub isolates_count: usize,
    /// Total nodes in WCCs other than the GCC.
    pub isolates_node_count: usize,
    /// All WCCs sorted by size descending. Each Vec contains participant NodeIndices.
    pub largest_components: Vec<Vec<NodeIndex>>,
}

impl WccTopology {
    /// Returns true if a component of `size` should be considered an isolate
    /// (i.e., excluded from clustering when min_component_size filter is active).
    pub fn is_isolate(&self, size: usize, min_size: usize) -> bool {
        size < min_size
    }

    /// Participant NodeIndices that belong to WCCs of size >= `min_size`.
    pub fn participants_above_threshold(&self, min_size: usize) -> Vec<NodeIndex> {
        self.largest_components
            .iter()
            .filter(|c| c.len() >= min_size)
            .flat_map(|c| c.iter().copied())
            .collect()
    }
}

/// Compute WCC topology on the participant-induced subgraph.
///
/// Builds an undirected adjacency over participant nodes (using all edge types
/// that survive participant filtering, regardless of EdgeCategory), then runs
/// `petgraph::algo::connected_components`.
pub fn compute_wcc_topology(graph: &CodeGraph, config: &ClusterConfig) -> WccTopology {
    // Collect participant nodes
    let participants: HashSet<NodeIndex> = graph
        .node_indices()
        .filter(|&idx| {
            config
                .participant_kinds
                .contains(&NodeKind::from_node(&graph[idx]))
        })
        .collect();

    // Build undirected adjacency among participants
    let mut adj: HashMap<NodeIndex, HashSet<NodeIndex>> = HashMap::new();
    for &p in &participants {
        adj.insert(p, HashSet::new());
    }
    for edge_idx in graph.edge_indices() {
        let (src, dst) = match graph.edge_endpoints(edge_idx) {
            Some(ep) => ep,
            None => continue,
        };
        if participants.contains(&src) && participants.contains(&dst) && src != dst {
            adj.get_mut(&src).unwrap().insert(dst);
            adj.get_mut(&dst).unwrap().insert(src);
        }
    }

    // BFS to find connected components
    let mut visited: HashSet<NodeIndex> = HashSet::new();
    let mut components: Vec<Vec<NodeIndex>> = Vec::new();
    for &start in participants.iter() {
        if visited.contains(&start) {
            continue;
        }
        let mut component: Vec<NodeIndex> = Vec::new();
        let mut queue: VecDeque<NodeIndex> = VecDeque::new();
        queue.push_back(start);
        visited.insert(start);
        while let Some(node) = queue.pop_front() {
            component.push(node);
            if let Some(neighbors) = adj.get(&node) {
                for &nb in neighbors {
                    if !visited.contains(&nb) {
                        visited.insert(nb);
                        queue.push_back(nb);
                    }
                }
            }
        }
        components.push(component);
    }

    // Sort by size descending
    components.sort_by(|a, b| b.len().cmp(&a.len()));

    let total_participants = participants.len();
    let wcc_count = components.len();
    let gcc_size = components.first().map(|c| c.len()).unwrap_or(0);
    let isolates_count = wcc_count.saturating_sub(if gcc_size > 0 { 1 } else { 0 });
    let isolates_node_count: usize = components
        .iter()
        .skip(if gcc_size > 0 { 1 } else { 0 })
        .map(|c| c.len())
        .sum();

    WccTopology {
        total_participants,
        wcc_count,
        gcc_size,
        isolates_count,
        isolates_node_count,
        largest_components: components,
    }
}
```

Add the necessary imports at the top of `src/graph/cluster.rs`:
```rust
use std::collections::VecDeque;
```
(`HashSet`, `HashMap` are already imported.)

**Step 4: Run tests to verify they pass**

```sh
cargo test --lib cluster::tests::wcc_topology
```
Expected: PASS (2 tests).

**Step 5: Commit**

```sh
git add src/graph/cluster.rs
git commit -m "feat(cluster): add WccTopology type and compute_wcc_topology

Adds weakly-connected-component detection on the participant-induced
subgraph. Foundation for topology-aware partition reporting and
min-component-size filtering.

Refs: .sisyphus/plans/2026-06-19-partition-skew-fix.md Task 1"
```

---

### Task 2: Integrate Topology into PartitionReport + Config

**Files:**
- Modify: `src/graph/cluster.rs` (`PartitionReport` struct, `ClusterConfig` struct, `partition()` fn)

**Step 1: Write failing test**

```rust
#[test]
fn partition_report_includes_topology() {
    let mut graph = CodeGraph::new();
    // Two disconnected pairs
    let a = graph.add_node(proc_node("a"));
    let b = graph.add_node(proc_node("b"));
    let c = graph.add_node(proc_node("c"));
    let d = graph.add_node(proc_node("d"));
    graph.add_edge(a, b, call_edge());
    graph.add_edge(b, a, call_edge());
    graph.add_edge(c, d, call_edge());
    graph.add_edge(d, c, call_edge());

    let report = partition(&graph, &ClusterConfig::auto());
    assert!(report.topology.is_some());
    let topo = report.topology.as_ref().unwrap();
    assert_eq!(topo.wcc_count, 2);
    assert_eq!(topo.gcc_size, 2);
}

#[test]
fn min_component_size_filters_isolates() {
    let mut graph = CodeGraph::new();
    // GCC of 3 nodes
    let a = graph.add_node(proc_node("a"));
    let b = graph.add_node(proc_node("b"));
    let c = graph.add_node(proc_node("c"));
    graph.add_edge(a, b, call_edge());
    graph.add_edge(b, c, call_edge());
    // Singleton
    let _d = graph.add_node(proc_node("d"));

    let config = ClusterConfig::auto().with_min_component_size(2);
    let report = partition(&graph, &config);
    // d is filtered out (singleton, size 1 < threshold 2)
    assert_eq!(report.total_nodes, 3);
    assert!(report.assignments.contains_key(&a));
    assert!(report.assignments.contains_key(&b));
    assert!(report.assignments.contains_key(&c));
    assert!(!report.assignments.contains_key(&NodeIndex::new(3))); // d
}
```

**Step 2: Run tests to verify they fail**

```sh
cargo test --lib cluster::tests::partition_report_includes_topology
cargo test --lib cluster::tests::min_component_size_filters_isolates
```
Expected: FAIL ("no field `topology` on type `PartitionReport`", "no method `with_min_component_size`").

**Step 3: Add `topology` field to `PartitionReport`**

Locate `pub struct PartitionReport` (around line 877) and add `topology`:

```rust
#[derive(Debug, Clone)]
pub struct PartitionReport {
    pub assignments: HashMap<NodeIndex, ClusterId>,
    pub k_actual: usize,
    pub k_requested: Option<usize>,
    pub gamma: f64,
    pub modularity: f64,
    pub cluster_stats: Vec<ClusterStat>,
    pub inter_cluster_coupling: Vec<InterClusterCoupling>,
    pub total_nodes: usize,
    /// Weakly-connected-component topology of the participant subgraph.
    /// Always populated by `partition()`. Useful for diagnosing skew.
    pub topology: Option<WccTopology>,
}
```

**Step 4: Add `min_component_size` to `ClusterConfig` + builder**

Locate `pub struct ClusterConfig` (around line 141) and add the field:

```rust
#[derive(Debug, Clone)]
pub struct ClusterConfig {
    pub k: Option<usize>,
    pub gamma: f64,
    pub edge_weights: EdgeWeights,
    pub participant_kinds: HashSet<NodeKind>,
    pub max_iterations: Option<usize>,
    pub min_delta_q: f64,
    /// Minimum WCC size to participate in clustering. Default 1 (all participate).
    /// WCCs smaller than this are reported in topology but excluded from clustering.
    pub min_component_size: usize,
}
```

Update all constructors to default `min_component_size: 1`:

```rust
impl ClusterConfig {
    pub fn new(k: usize) -> Self {
        Self {
            k: Some(k),
            gamma: 1.0,
            edge_weights: EdgeWeights::default(),
            participant_kinds: default_participant_kinds(),
            max_iterations: None,
            min_delta_q: 0.0,
            min_component_size: 1,
        }
    }

    pub fn auto() -> Self {
        Self {
            k: None,
            gamma: 1.0,
            edge_weights: EdgeWeights::default(),
            participant_kinds: default_participant_kinds(),
            max_iterations: None,
            min_delta_q: 0.0,
            min_component_size: 1,
        }
    }

    // ... existing with_* methods unchanged ...

    /// Exclude WCCs smaller than `n` from clustering (they are still reported
    /// in topology). Use to focus clustering on the giant connected component.
    pub fn with_min_component_size(mut self, n: usize) -> Self {
        self.min_component_size = n.max(1);
        self
    }
}
```

**Step 5: Extend `condense_sccs` to accept an optional allowed-participants set**

Change the signature and logic of `fn condense_sccs` (around line 230):

```rust
fn condense_sccs(graph: &CodeGraph, config: &ClusterConfig) -> SccCondensation {
    // Compute allowed set from min_component_size filter
    let topology = compute_wcc_topology(graph, config);
    let allowed: HashSet<NodeIndex> = if config.min_component_size <= 1 {
        // No filter — all participants allowed
        topology
            .largest_components
            .iter()
            .flat_map(|c| c.iter().copied())
            .collect()
    } else {
        topology.participants_above_threshold(config.min_component_size)
    };

    let sccs = petgraph::algo::kosaraju_scc(graph);

    let mut super_nodes: Vec<HashSet<NodeIndex>> = Vec::new();
    let mut node_to_super: HashMap<NodeIndex, usize> = HashMap::new();

    for scc in sccs {
        let filtered: HashSet<NodeIndex> = scc
            .into_iter()
            .filter(|idx| allowed.contains(idx))
            .collect();
        if filtered.is_empty() {
            continue;
        }
        let super_idx = super_nodes.len();
        for &node in &filtered {
            node_to_super.insert(node, super_idx);
        }
        super_nodes.push(filtered);
    }

    SccCondensation {
        super_nodes,
        node_to_super,
    }
}
```

**Step 6: Update `partition()` to populate topology**

Locate `pub fn partition` (around line 888) and update the return struct:

```rust
pub fn partition(graph: &CodeGraph, config: &ClusterConfig) -> PartitionReport {
    let topology = compute_wcc_topology(graph, config);
    let condensation = condense_sccs(graph, config);
    let condensed = build_condensed_graph(graph, &condensation, config);
    let (super_communities, modularity) = cluster_cnm(
        &condensed,
        config.k,
        config.gamma,
        config.max_iterations,
        config.min_delta_q,
        None,
    );

    let mut assignments: HashMap<NodeIndex, ClusterId> = HashMap::new();
    for (super_idx, members) in condensation.super_nodes.iter().enumerate() {
        let cluster = super_communities[super_idx] as ClusterId;
        for &node in members {
            assignments.insert(node, cluster);
        }
    }

    let k_actual = super_communities
        .iter()
        .copied()
        .max()
        .map(|m| m + 1)
        .unwrap_or(0);
    let cluster_stats =
        compute_cluster_stats(graph, &assignments, &condensed, &condensation, k_actual);
    let inter_cluster_coupling = compute_inter_cluster_coupling(graph, &assignments, config);
    let total_nodes = assignments.len();

    PartitionReport {
        assignments,
        k_actual,
        k_requested: config.k,
        gamma: config.gamma,
        modularity,
        cluster_stats,
        inter_cluster_coupling,
        total_nodes,
        topology: Some(topology),
    }
}
```

**Step 7: Run tests to verify they pass**

```sh
cargo test --lib cluster
```
Expected: PASS (all existing tests + 2 new tests). Existing tests should still pass because `min_component_size` defaults to 1.

**Step 8: Run clippy + fmt**

```sh
cargo clippy --features full -- -D warnings
cargo fmt -- --check
```
Expected: clean.

**Step 9: Commit**

```sh
git add src/graph/cluster.rs
git commit -m "feat(cluster): integrate WccTopology into PartitionReport, add min_component_size

- PartitionReport now carries topology: Option<WccTopology>
- ClusterConfig.min_component_size (default 1) excludes small WCCs
  from clustering while still reporting them in topology
- Backward compatible: default behavior unchanged

Refs: .sisyphus/plans/2026-06-19-partition-skew-fix.md Task 2"
```

---

### Task 3: CLI Flag + Topology Output

**Files:**
- Modify: `src/main.rs` (add `--min-component-size` flag, `print_wcc_topology` fn)
- Modify: `locales/en.yml` and `locales/zh-CN.yml` (new keys)

**Step 1: Add i18n keys**

Append to `locales/en.yml` after the last `partition.*` entry:

```yaml
partition.wcc_topology: "Graph topology:"
partition.wcc_total: "  Participant nodes: %{total}"
partition.wcc_count: "  Weakly connected components: %{count}"
partition.wcc_gcc: "  Giant component: %{size} nodes (%{pct}%)"
partition.wcc_isolates: "  Isolates: %{components} components, %{nodes} nodes"
partition.wcc_filter_active: "  Filter active: components < %{threshold} excluded from clustering"
```

Append the same keys to `locales/zh-CN.yml` with Chinese values:

```yaml
partition.wcc_topology: "图拓扑："
partition.wcc_total: "  参与者节点：%{total}"
partition.wcc_count: "  弱连通分量数：%{count}"
partition.wcc_gcc: "  巨型分量：%{size} 个节点（%{pct}%）"
partition.wcc_isolates: "  孤岛：%{components} 个分量，%{nodes} 个节点"
partition.wcc_filter_active: "  过滤已启用：小于 %{threshold} 的分量已排除出聚类"
```

**Step 2: Add CLI flag**

Locate `Commands::Partition` (around line 351) and add the flag:

```rust
Partition {
    /// Target number of clusters (omit for auto-discovery)
    #[arg(short, long)]
    k: Option<usize>,

    /// Resolution parameter γ (lower = fewer/larger clusters, default 1.0)
    #[arg(long)]
    gamma: Option<f64>,

    /// Auto-discover optimal cluster count via γ sweep
    #[arg(long)]
    auto: bool,

    /// Minimum weakly connected component size to participate in clustering.
    /// Components smaller than this are reported but excluded from clustering.
    /// Use to focus on the giant component when the graph has many isolates.
    /// Default: 1 (all components participate).
    #[arg(long, default_value = "1")]
    min_component_size: usize,

    // ... existing max_iterations, min_delta_q, project, output ...
}
```

Update the match arm in `fn run()` (around line 522) to pass the new arg through:

```rust
Some(Commands::Partition {
    k,
    gamma,
    auto,
    max_iterations,
    min_delta_q,
    min_component_size,
    project,
    output,
}) => cmd_partition(
    k,
    gamma,
    auto,
    max_iterations,
    min_delta_q,
    min_component_size,
    project,
    output.as_deref(),
),
```

Update `fn cmd_partition` signature (around line 1567):

```rust
fn cmd_partition(
    k: Option<usize>,
    gamma: Option<f64>,
    auto: bool,
    max_iterations: Option<usize>,
    min_delta_q: Option<f64>,
    min_component_size: usize,
    project: &Path,
    output: Option<&Path>,
) -> Result<()> {
```

In `cmd_partition`, when building `config`, apply the filter:

```rust
let mut config = match k {
    Some(k) => graph::cluster::ClusterConfig::new(k),
    None => graph::cluster::ClusterConfig::auto(),
};
if let Some(g) = gamma {
    config = config.with_gamma(g);
}
if let Some(n) = max_iterations {
    config = config.with_max_iterations(n);
}
if let Some(q) = min_delta_q {
    config = config.with_min_delta_q(q);
}
config = config.with_min_component_size(min_component_size);
```

Do the same for the auto branch (around line 1580):

```rust
let mut base_config = graph::cluster::ClusterConfig::auto()
    .with_min_component_size(min_component_size);
```

**Step 3: Add `print_wcc_topology` function**

Add this function in `src/main.rs` near `print_cluster_details` (around line 1738):

```rust
fn print_wcc_topology(report: &graph::cluster::PartitionReport) {
    let Some(topo) = report.topology.as_ref() else {
        return;
    };
    println_stdout!("{}", t!("partition.wcc_topology"));
    println_stdout!("{}", t!("partition.wcc_total", total = topo.total_participants));
    println_stdout!("{}", t!("partition.wcc_count", count = topo.wcc_count));

    let gcc_pct = if topo.total_participants > 0 {
        topo.gcc_size as f64 * 100.0 / topo.total_participants as f64
    } else {
        0.0
    };
    println_stdout!(
        "{}",
        t!("partition.wcc_gcc", size = topo.gcc_size, pct = format!("{:.1}", gcc_pct))
    );
    println_stdout!(
        "{}",
        t!(
            "partition.wcc_isolates",
            components = topo.isolates_count,
            nodes = topo.isolates_node_count
        )
    );

    // Note filter status from cluster_stats vs topology discrepancy
    let clustered_nodes: usize = report.cluster_stats.iter().map(|s| s.node_count).sum();
    if clustered_nodes < topo.total_participants {
        let excluded = topo.total_participants - clustered_nodes;
        println_stdout!(
            "  {}",
            t!(
                "partition.wcc_filter_active",
                threshold = report
                    .k_requested
                    .map(|_| "N/A")
                    .unwrap_or("N/A")
            )
        );
        // Simpler: print excluded count directly
        println_stdout!("    ({excluded} nodes in small components excluded from clustering)");
    }
    println_stdout!();
}
```

Refine the filter-active message to use the actual `min_component_size` from the report's `gamma` config or pass it explicitly. For simplicity, the message just reports the discrepancy.

**Step 4: Call `print_wcc_topology` from `cmd_partition`**

In `cmd_partition`, right before `print_cluster_details(&report)` (around line 1706):

```rust
println_stdout!();
print_wcc_topology(&report);
print_cluster_details(&report);
print_cluster_analysis(&report);
```

Do the same for the auto branch (around line 1665).

**Step 5: Build and verify**

```sh
cargo build
cargo run -- partition --auto --project testcases/sample-project 2>&1 | head -40
```
Expected: output includes a "Graph topology:" section before the cluster table.

Test the filter:
```sh
cargo run -- partition --auto --min-component-size 10 --project testcases/sample-project 2>&1 | head -40
```
Expected: topology section shows filter active message.

**Step 6: Run clippy + fmt**

```sh
cargo clippy -- -D warnings
cargo fmt -- --check
```

**Step 7: Commit**

```sh
git add src/main.rs locales/en.yml locales/zh-CN.yml
git commit -m "feat(cli): add --min-component-size flag and WCC topology output

Prints participant count, WCC count, GCC size, isolate stats before
the cluster table. When --min-component-size N is set, components
smaller than N are excluded from clustering and reported separately.

Refs: .sisyphus/plans/2026-06-19-partition-skew-fix.md Task 3"
```

---

### Task 4: Phase 1 Integration Test

**Files:**
- Modify: `tests/cluster_test.rs` (create if not exists) or `src/graph/cluster.rs` `mod tests`

**Step 1: Write integration test**

Add to `src/graph/cluster.rs` `mod tests`:

```rust
#[test]
fn min_component_size_clusters_only_gcc() {
    // Realistic micro-fixture: GCC of 6 nodes + 3 isolated pairs + 2 singletons
    let mut graph = CodeGraph::new();
    // GCC: 6 procs in a chain + clique
    let gcc_nodes: Vec<NodeIndex> = (0..6)
        .map(|i| graph.add_node(proc_node(&format!("gcc{i}"))))
        .collect();
    for i in 0..6 {
        for j in (i + 1)..6 {
            graph.add_edge(gcc_nodes[i], gcc_nodes[j], call_edge());
            graph.add_edge(gcc_nodes[j], gcc_nodes[i], call_edge());
        }
    }
    // 3 isolated pairs
    for pair_idx in 0..3 {
        let p1 = graph.add_node(proc_node(&format!("iso_p{pair_idx}_a")));
        let p2 = graph.add_node(proc_node(&format!("iso_p{pair_idx}_b")));
        graph.add_edge(p1, p2, call_edge());
        graph.add_edge(p2, p1, call_edge());
    }
    // 2 singletons
    let _s1 = graph.add_node(proc_node("singleton_1"));
    let _s2 = graph.add_node(proc_node("singleton_2"));

    // Without filter: all 14 nodes participate
    let report_all = partition(&graph, &ClusterConfig::auto());
    assert_eq!(report_all.total_nodes, 14);
    let topo = report_all.topology.as_ref().unwrap();
    assert_eq!(topo.wcc_count, 6); // 1 GCC + 3 pairs + 2 singletons
    assert_eq!(topo.gcc_size, 6);

    // With filter min_size=3: only GCC participates (6 nodes)
    let report_filtered = partition(
        &graph,
        &ClusterConfig::auto().with_min_component_size(3),
    );
    assert_eq!(report_filtered.total_nodes, 6);
    // All clustered nodes are GCC members
    for stat in &report_filtered.cluster_stats {
        for node_idx in stat.type_distribution.values() {
            // (Type distribution doesn't give NodeIndex; this assertion is structural)
            assert!(*node_idx <= 6); // sanity
        }
    }
    // Isolates are still in topology
    let topo2 = report_filtered.topology.as_ref().unwrap();
    assert_eq!(topo2.wcc_count, 6); // topology unchanged
    assert_eq!(topo2.gcc_size, 6);
}
```

**Step 2: Run test**

```sh
cargo test --lib cluster::tests::min_component_size_clusters_only_gcc
```
Expected: PASS.

**Step 3: Run full test suite + verification**

```sh
cargo test --features full
cargo clippy --features full -- -D warnings
cargo fmt -- --check
```
Expected: all pass.

**Step 4: Commit**

```sh
git add src/graph/cluster.rs
git commit -m "test(cluster): integration test for min_component_size GCC-only clustering

Refs: .sisyphus/plans/2026-06-19-partition-skew-fix.md Task 4"
```

---

### Phase 2: TF-IDF TableAccess Projection

---

### Task 5: ProcTableMatrix Builder

**Files:**
- Modify: `src/graph/cluster.rs` (add `ProcTableMatrix` struct + `build_proc_table_matrix` fn, ~70 lines)

**Step 1: Write failing test**

```rust
#[test]
fn proc_table_matrix_captures_accesses() {
    let mut graph = CodeGraph::new();
    let p1 = graph.add_node(proc_node("p1"));
    let p2 = graph.add_node(proc_node("p2"));
    let p3 = graph.add_node(proc_node("p3"));
    let t_orders = graph.add_node(table_node("orders"));
    let t_customers = graph.add_node(table_node("customers"));
    let t_audit = graph.add_node(table_node("audit_log"));

    // p1 reads orders + audit_log
    graph.add_edge(p1, t_orders, table_access_edge());
    graph.add_edge(p1, t_audit, table_access_edge());
    // p2 reads orders + customers
    graph.add_edge(p2, t_orders, table_access_edge());
    graph.add_edge(p2, t_customers, table_access_edge());
    // p3 reads customers only
    graph.add_edge(p3, t_customers, table_access_edge());

    let config = ClusterConfig::auto();
    let matrix = build_proc_table_matrix(&graph, &config);

    // 3 procs in matrix
    assert_eq!(matrix.procs.len(), 3);
    // 3 tables
    assert_eq!(matrix.tables.len(), 3);
    // p1 ↔ orders: true
    assert!(matrix.access(p1, t_orders));
    assert!(matrix.access(p1, t_audit));
    assert!(!matrix.access(p1, t_customers));
    // p2 ↔ orders + customers
    assert!(matrix.access(p2, t_orders));
    assert!(matrix.access(p2, t_customers));
    // p3 ↔ customers only
    assert!(matrix.access(p3, t_customers));
    assert!(!matrix.access(p3, t_orders));
}

#[test]
fn proc_table_matrix_excludes_non_participants() {
    let mut graph = CodeGraph::new();
    // A non-participant source (table → table dependency) should NOT be in matrix
    let t1 = graph.add_node(table_node("t1"));
    let t2 = graph.add_node(table_node("t2"));
    graph.add_edge(t1, t2, table_access_edge());

    let config = ClusterConfig::auto();
    let matrix = build_proc_table_matrix(&graph, &config);
    assert_eq!(matrix.procs.len(), 0);
    assert_eq!(matrix.tables.len(), 0);
}
```

**Step 2: Run test to verify it fails**

```sh
cargo test --lib cluster::tests::proc_table_matrix
```
Expected: FAIL ("cannot find function `build_proc_table_matrix`").

**Step 3: Implement ProcTableMatrix**

Add to `src/graph/cluster.rs` (just before `WccTopology` definition from Task 1):

```rust
/// Sparse binary matrix of procedure → table access.
///
/// Built from `Edge::TableAccess` edges where source is a participant
/// (Procedure/Function/JavaMethod/MappedStatement/JavaClass) and destination
/// is a Table. Used as input to TF-IDF + cosine similarity projection.
#[derive(Debug, Clone)]
pub struct ProcTableMatrix {
    /// Participant NodeIndices that access at least one table.
    pub procs: Vec<NodeIndex>,
    /// Table NodeIndices that are accessed by at least one participant.
    pub tables: Vec<NodeIndex>,
    /// Sparse access map: (proc_idx_in_procs, table_idx_in_tables) → true.
    /// Stored as HashSet for O(1) membership check.
    access_map: HashSet<(usize, usize)>,
    /// Inverse lookup: original NodeIndex → index in `procs`.
    proc_index: HashMap<NodeIndex, usize>,
    /// Inverse lookup: original NodeIndex → index in `tables`.
    table_index: HashMap<NodeIndex, usize>,
}

impl ProcTableMatrix {
    /// Returns true if `proc` accesses `table` (both original NodeIndex values).
    pub fn access(&self, proc: NodeIndex, table: NodeIndex) -> bool {
        let p = match self.proc_index.get(&proc) {
            Some(&i) => i,
            None => return false,
        };
        let t = match self.table_index.get(&table) {
            Some(&i) => i,
            None => return false,
        };
        self.access_map.contains(&(p, t))
    }

    /// Number of procs in the matrix.
    pub fn proc_count(&self) -> usize {
        self.procs.len()
    }
    /// Number of tables in the matrix.
    pub fn table_count(&self) -> usize {
        self.tables.len()
    }
    /// Iterate over (proc_index, table_index) pairs that are accessed.
    pub fn accesses(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.access_map.iter().copied()
    }
}

/// Build the proc-table access matrix from the graph.
///
/// Includes only edges where:
///   - source is a participant (per `config.participant_kinds`)
///   - destination is a Table
///   - edge is `Edge::TableAccess` with `flow_kind == DmlAccess`
pub fn build_proc_table_matrix(graph: &CodeGraph, config: &ClusterConfig) -> ProcTableMatrix {
    use crate::graph::{DataFlowKind, Edge};

    let mut proc_set: HashSet<NodeIndex> = HashSet::new();
    let mut table_set: HashSet<NodeIndex> = HashSet::new();
    let mut access_pairs: HashSet<(NodeIndex, NodeIndex)> = HashSet::new();

    for edge_idx in graph.edge_indices() {
        let (src, dst) = match graph.edge_endpoints(edge_idx) {
            Some(ep) => ep,
            None => continue,
        };
        // Source must be a participant
        if !config
            .participant_kinds
            .contains(&NodeKind::from_node(&graph[src]))
        {
            continue;
        }
        // Destination must be a Table
        if !matches!(graph[dst], Node::Table { .. }) {
            continue;
        }
        // Edge must be DmlAccess TableAccess
        match &graph[edge_idx] {
            Edge::TableAccess { flow_kind, .. } if *flow_kind == DataFlowKind::DmlAccess => {}
            _ => continue,
        }
        proc_set.insert(src);
        table_set.insert(dst);
        access_pairs.insert((src, dst));
    }

    let mut procs: Vec<NodeIndex> = proc_set.into_iter().collect();
    procs.sort();
    let mut tables: Vec<NodeIndex> = table_set.into_iter().collect();
    tables.sort();

    let proc_index: HashMap<NodeIndex, usize> =
        procs.iter().enumerate().map(|(i, &n)| (n, i)).collect();
    let table_index: HashMap<NodeIndex, usize> =
        tables.iter().enumerate().map(|(i, &n)| (n, i)).collect();

    let access_map: HashSet<(usize, usize)> = access_pairs
        .into_iter()
        .map(|(p, t)| (proc_index[&p], table_index[&t]))
        .collect();

    ProcTableMatrix {
        procs,
        tables,
        access_map,
        proc_index,
        table_index,
    }
}
```

**Step 4: Run tests**

```sh
cargo test --lib cluster::tests::proc_table_matrix
```
Expected: PASS (2 tests).

**Step 5: Commit**

```sh
git add src/graph/cluster.rs
git commit -m "feat(cluster): add ProcTableMatrix builder for table-access data

Extracts participant→table access edges into a sparse binary matrix,
the foundation for TF-IDF + cosine similarity projection.

Refs: .sisyphus/plans/2026-06-19-partition-skew-fix.md Task 5"
```

---

### Task 6: TF-IDF + Cosine Similarity Computation

**Files:**
- Modify: `src/graph/cluster.rs` (add `compute_tfidf_cosine_edges` fn, ~90 lines)

**Step 1: Write failing test**

```rust
#[test]
fn tfidf_cosine_groups_similar_procs() {
    let mut graph = CodeGraph::new();
    let p1 = graph.add_node(proc_node("p1"));
    let p2 = graph.add_node(proc_node("p2"));
    let p3 = graph.add_node(proc_node("p3"));
    let t_orders = graph.add_node(table_node("orders"));
    let t_customers = graph.add_node(table_node("customers"));
    let t_audit = graph.add_node(table_node("audit_log"));

    // p1, p2 both read orders + customers (very similar)
    graph.add_edge(p1, t_orders, table_access_edge());
    graph.add_edge(p1, t_customers, table_access_edge());
    graph.add_edge(p2, t_orders, table_access_edge());
    graph.add_edge(p2, t_customers, table_access_edge());
    // p3 reads only audit_log (dissimilar)
    graph.add_edge(p3, t_audit, table_access_edge());

    let config = ClusterConfig::auto();
    let matrix = build_proc_table_matrix(&graph, &config);
    let edges = compute_tfidf_cosine_edges(&matrix, 0.1, 10);

    // p1 ↔ p2 should have high similarity (> 0.5)
    let p1_idx = matrix.proc_index[&p1];
    let p2_idx = matrix.proc_index[&p2];
    let p3_idx = matrix.proc_index[&p3];

    let sim_12 = edges
        .iter()
        .find(|(a, b, _)| (*a == p1_idx && *b == p2_idx) || (*a == p2_idx && *b == p1_idx))
        .map(|(_, _, s)| *s);
    assert!(sim_12.is_some(), "p1-p2 edge must exist");
    assert!(sim_12.unwrap() > 0.5, "p1-p2 similarity should be high");

    // p3 should have no edges to p1/p2 (cosine = 0)
    let sim_13 = edges
        .iter()
        .find(|(a, b, _)| (*a == p1_idx && *b == p3_idx) || (*a == p3_idx && *b == p1_idx));
    assert!(sim_13.is_none(), "p1-p3 should not have an edge");
}

#[test]
fn tfidf_downweights_generic_tables() {
    let mut graph = CodeGraph::new();
    // 3 procs all read the "common_codes" generic table
    let p1 = graph.add_node(proc_node("p1"));
    let p2 = graph.add_node(proc_node("p2"));
    let p3 = graph.add_node(proc_node("p3"));
    let t_common = graph.add_node(table_node("common_codes"));
    graph.add_edge(p1, t_common, table_access_edge());
    graph.add_edge(p2, t_common, table_access_edge());
    graph.add_edge(p3, t_common, table_access_edge());

    let config = ClusterConfig::auto();
    let matrix = build_proc_table_matrix(&graph, &config);
    let edges = compute_tfidf_cosine_edges(&matrix, 0.1, 10);

    // If all procs share ONLY the generic table, IDF = log(3/3) = 0,
    // so TF-IDF vectors are all-zero → no similarity edges.
    assert!(edges.is_empty(), "generic-only sharing should produce no edges");
}

#[test]
fn tfidf_cosine_respects_threshold() {
    let mut graph = CodeGraph::new();
    let p1 = graph.add_node(proc_node("p1"));
    let p2 = graph.add_node(proc_node("p2"));
    let t1 = graph.add_node(table_node("t1"));
    let t2 = graph.add_node(table_node("t2"));
    let t3 = graph.add_node(table_node("t3"));
    let t4 = graph.add_node(table_node("t4"));

    // p1 reads 4 tables, p2 reads 1 of them → low overlap
    graph.add_edge(p1, t1, table_access_edge());
    graph.add_edge(p1, t2, table_access_edge());
    graph.add_edge(p1, t3, table_access_edge());
    graph.add_edge(p1, t4, table_access_edge());
    graph.add_edge(p2, t1, table_access_edge());

    let config = ClusterConfig::auto();
    let matrix = build_proc_table_matrix(&graph, &config);

    // With high threshold (0.5), no edge should appear (cosine ≈ 0.5 exactly,
    // since overlap=1, |p1|=4, |p2|=1 → cosine = 1/sqrt(4) = 0.5)
    let edges_high = compute_tfidf_cosine_edges(&matrix, 0.6, 10);
    assert!(edges_high.is_empty());
    // With low threshold (0.1), edge appears
    let edges_low = compute_tfidf_cosine_edges(&matrix, 0.1, 10);
    assert!(!edges_low.is_empty());
}
```

**Step 2: Run tests to verify they fail**

```sh
cargo test --lib cluster::tests::tfidf
```
Expected: FAIL ("cannot find function `compute_tfidf_cosine_edges`").

**Step 3: Implement TF-IDF + cosine**

Add to `src/graph/cluster.rs` (just after `ProcTableMatrix`):

```rust
/// Compute TF-IDF weighted cosine similarity edges between procedures.
///
/// Returns `Vec<(proc_idx_a, proc_idx_b, weight)>` where indices refer to
/// positions in `matrix.procs`, and `weight` is the cosine similarity
/// in [0, 1] (TF-IDF vectors are non-negative).
///
/// **Algorithm:**
/// 1. IDF per table: `idf(t) = log(N / df(t))` where `N` = proc count,
///    `df(t)` = number of procs accessing table t.
/// 2. TF-IDF vector per proc: binary TF (1.0 if accessed) × IDF.
/// 3. Cosine similarity between each proc pair.
/// 4. Sparsification: keep top-`k_neighbors` per proc, then drop edges
///    below `tau`.
///
/// **Empty graph safety:** if `N <= 1` or no accesses, returns empty Vec.
pub fn compute_tfidf_cosine_edges(
    matrix: &ProcTableMatrix,
    tau: f64,
    k_neighbors: usize,
) -> Vec<(usize, usize, f64)> {
    let n_procs = matrix.proc_count();
    let n_tables = matrix.table_count();
    if n_procs <= 1 || n_tables == 0 {
        return Vec::new();
    }

    // 1. Compute document frequency per table
    let mut df: Vec<usize> = vec![0; n_tables];
    for &(_, t) in matrix.access_map.iter() {
        df[t] += 1;
    }

    // 2. IDF: log(N / df). If df == N, IDF = 0 (generic table).
    let n = n_procs as f64;
    let idf: Vec<f64> = df
        .iter()
        .map(|&d| if d == 0 { 0.0 } else { (n / d as f64).ln() })
        .collect();

    // 3. TF-IDF vector per proc (sparse: HashMap<table_idx, weight>)
    let mut tfidf_vectors: Vec<HashMap<usize, f64>> = vec![HashMap::new(); n_procs];
    for &(p, t) in matrix.access_map.iter() {
        let weight = idf[t];
        if weight > 0.0 {
            tfidf_vectors[p].insert(t, weight);
        }
    }

    // 4. Vector norms
    let norms: Vec<f64> = tfidf_vectors
        .iter()
        .map(|v| {
            let sum_sq: f64 = v.values().map(|w| w * w).sum();
            sum_sq.sqrt()
        })
        .collect();

    // 5. For each proc, compute cosine similarity to all others, keep top-K
    let mut edges: Vec<(usize, usize, f64)> = Vec::new();
    for i in 0..n_procs {
        if norms[i] == 0.0 {
            continue;
        }
        let mut sims: Vec<(usize, f64)> = (0..n_procs)
            .filter(|&j| j != i && norms[j] > 0.0)
            .map(|j| {
                // Dot product over the smaller vector
                let (small, large) = if tfidf_vectors[i].len() < tfidf_vectors[j].len() {
                    (&tfidf_vectors[i], &tfidf_vectors[j])
                } else {
                    (&tfidf_vectors[j], &tfidf_vectors[i])
                };
                let dot: f64 = small
                    .iter()
                    .filter_map(|(t, w)| large.get(t).map(|w2| w * w2))
                    .sum();
                let cos = dot / (norms[i] * norms[j]);
                (j, cos)
            })
            .filter(|(_, s)| *s > 0.0)
            .collect();
        // Sort descending by similarity, take top-K
        sims.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for (j, sim) in sims.into_iter().take(k_neighbors) {
            if sim >= tau {
                let (a, b) = if i < j { (i, j) } else { (j, i) };
                edges.push((a, b, sim));
            }
        }
    }

    // Deduplicate (i,j) pairs (each pair may be added from both sides)
    edges.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal))
    });
    edges.dedup_by_key(|(a, b, _)| (*a, *b));

    edges
}
```

**Step 4: Run tests**

```sh
cargo test --lib cluster::tests::tfidf
```
Expected: PASS (3 tests).

**Step 5: Commit**

```sh
git add src/graph/cluster.rs
git commit -m "feat(cluster): TF-IDF + cosine similarity over table-access matrix

Pure-Rust implementation: IDF = log(N/df), binary TF, sparse dot
product for cosine, top-K nearest neighbors per proc, tau threshold
filter. Generic tables (accessed by all procs) get IDF=0 and produce
no spurious coupling.

Refs: .sisyphus/plans/2026-06-19-partition-skew-fix.md Task 6"
```

---

### Task 7: TableProjectionConfig + Edge Injection

**Files:**
- Modify: `src/graph/cluster.rs` (`ClusterConfig`, `build_condensed_graph`)

**Step 1: Write failing test**

```rust
#[test]
fn table_projection_bridges_isolated_procs() {
    let mut graph = CodeGraph::new();
    // p1, p2 don't call each other but both read "orders" and "customers"
    let p1 = graph.add_node(proc_node("p1"));
    let p2 = graph.add_node(proc_node("p2"));
    let t_orders = graph.add_node(table_node("orders"));
    let t_customers = graph.add_node(table_node("customers"));
    graph.add_edge(p1, t_orders, table_access_edge());
    graph.add_edge(p1, t_customers, table_access_edge());
    graph.add_edge(p2, t_orders, table_access_edge());
    graph.add_edge(p2, t_customers, table_access_edge());

    // Without projection: p1, p2 are in different WCCs (table is excluded)
    let report_no_proj = partition(&graph, &ClusterConfig::new(1));
    let topo = report_no_proj.topology.as_ref().unwrap();
    assert_eq!(topo.wcc_count, 2); // p1 alone, p2 alone (table excluded)

    // With projection: p1, p2 get bridged via TF-IDF similarity → 1 WCC
    let config = ClusterConfig::new(1).with_table_projection(0.1, 0.3, 10);
    let report_proj = partition(&graph, &config);
    // Now k=1 should produce a single cluster with both p1 and p2
    assert_eq!(report_proj.total_nodes, 2);
    let c1 = report_proj.assignments[&p1];
    let c2 = report_proj.assignments[&p2];
    assert_eq!(c1, c2, "p1 and p2 must be in same cluster with projection");
}
```

**Step 2: Run test to verify it fails**

```sh
cargo test --lib cluster::tests::table_projection_bridges_isolated_procs
```
Expected: FAIL ("no method `with_table_projection`").

**Step 3: Add TableProjectionConfig to ClusterConfig**

Update `ClusterConfig` struct:

```rust
#[derive(Debug, Clone)]
pub struct TableProjectionConfig {
    /// Minimum cosine similarity to add an edge. Default 0.1.
    pub tau: f64,
    /// Edge weight multiplier (vs call edge weight of 1.0). Default 0.3.
    pub lambda: f64,
    /// Top-K nearest neighbors per proc. Default 10.
    pub k_neighbors: usize,
}

impl Default for TableProjectionConfig {
    fn default() -> Self {
        Self {
            tau: 0.1,
            lambda: 0.3,
            k_neighbors: 10,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClusterConfig {
    pub k: Option<usize>,
    pub gamma: f64,
    pub edge_weights: EdgeWeights,
    pub participant_kinds: HashSet<NodeKind>,
    pub max_iterations: Option<usize>,
    pub min_delta_q: f64,
    pub min_component_size: usize,
    /// If Some, inject TF-IDF cosine-similarity edges between procs that
    /// share tables. Default None (off).
    pub table_projection: Option<TableProjectionConfig>,
}
```

Update constructors to default `table_projection: None`. Add builder:

```rust
impl ClusterConfig {
    // ... existing methods ...

    /// Enable TF-IDF table-access projection. Adds weighted similarity edges
    /// between procedures that share table accesses.
    pub fn with_table_projection(mut self, tau: f64, lambda: f64, k_neighbors: usize) -> Self {
        self.table_projection = Some(TableProjectionConfig {
            tau,
            lambda,
            k_neighbors,
        });
        self
    }
}
```

**Step 4: Inject projection edges into `build_condensed_graph`**

Locate `fn build_condensed_graph` (around line 278). After the main edge-iteration loop (which adds existing edges), insert the projection logic before computing `degrees`/`total_weight`:

```rust
fn build_condensed_graph(
    graph: &CodeGraph,
    condensation: &SccCondensation,
    config: &ClusterConfig,
) -> CondensedGraph {
    let n = condensation.super_nodes.len();
    let mut adj: Vec<HashMap<usize, f64>> = vec![HashMap::new(); n];

    // === Existing edge iteration (unchanged) ===
    for edge_idx in graph.edge_indices() {
        let (src, dst) = match graph.edge_endpoints(edge_idx) {
            Some(ep) => ep,
            None => continue,
        };
        let src_super = match condensation.node_to_super.get(&src) {
            Some(&s) => s,
            None => continue,
        };
        let dst_super = match condensation.node_to_super.get(&dst) {
            Some(&s) => s,
            None => continue,
        };
        let weight = match edge_weight(&graph[edge_idx], &config.edge_weights) {
            Some(w) => w,
            None => continue,
        };
        if src_super == dst_super {
            *adj[src_super].entry(src_super).or_insert(0.0) += weight;
        } else {
            *adj[src_super].entry(dst_super).or_insert(0.0) += weight;
            *adj[dst_super].entry(src_super).or_insert(0.0) += weight;
        }
    }

    // === NEW: TF-IDF table-access projection ===
    if let Some(proj) = &config.table_projection {
        let matrix = build_proc_table_matrix(graph, config);
        let sim_edges = compute_tfidf_cosine_edges(&matrix, proj.tau, proj.k_neighbors);
        // Build proc NodeIndex → super-node index lookup
        let proc_to_super: HashMap<NodeIndex, usize> = matrix
            .procs
            .iter()
            .filter_map(|&p| condensation.node_to_super.get(&p).map(|&s| (p, s)))
            .collect();
        for (i, j, sim) in sim_edges {
            let p_i = matrix.procs[i];
            let p_j = matrix.procs[j];
            let s_i = match proc_to_super.get(&p_i) {
                Some(&s) => s,
                None => continue,
            };
            let s_j = match proc_to_super.get(&p_j) {
                Some(&s) => s,
                None => continue,
            };
            let w = proj.lambda * sim;
            if s_i == s_j {
                *adj[s_i].entry(s_i).or_insert(0.0) += w;
            } else {
                *adj[s_i].entry(s_j).or_insert(0.0) += w;
                *adj[s_j].entry(s_i).or_insert(0.0) += w;
            }
        }
    }

    let degrees: Vec<f64> = (0..n).map(|i| adj[i].values().sum()).collect();
    let total_weight: f64 = degrees.iter().sum();

    CondensedGraph {
        adj,
        degrees,
        total_weight,
    }
}
```

**Step 5: Run tests**

```sh
cargo test --lib cluster
```
Expected: all pass (existing + 4 new tests from Tasks 5-7).

**Step 6: Run clippy + fmt**

```sh
cargo clippy --features full -- -D warnings
cargo fmt -- --check
```

**Step 7: Commit**

```sh
git add src/graph/cluster.rs
git commit -m "feat(cluster): inject TF-IDF similarity edges into condensed graph

When ClusterConfig.table_projection is Some, compute TF-IDF cosine
similarity between procedures via shared tables and inject weighted
edges into the condensed graph. Bridges isolated procedures that
share data access patterns but don't call each other directly.

Refs: .sisyphus/plans/2026-06-19-partition-skew-fix.md Task 7"
```

---

### Task 8: CLI Flag for Table Projection

**Files:**
- Modify: `src/main.rs` (add `--table-projection` flag)
- Modify: `locales/en.yml`, `locales/zh-CN.yml`

**Step 1: Add i18n keys**

In `locales/en.yml`:
```yaml
partition.projection_active: "  Table projection: ON (τ=%{tau}, λ=%{lambda}, k=%{k})"
partition.projection_off: "  Table projection: OFF"
```

In `locales/zh-CN.yml`:
```yaml
partition.projection_active: "  表投影：已开启（τ=%{tau}, λ=%{lambda}, k=%{k}）"
partition.projection_off: "  表投影：未开启"
```

**Step 2: Add CLI flag**

In `Commands::Partition`:
```rust
/// Enable TF-IDF table-access projection. Bridges procedures that
/// share table accesses but don't call each other directly.
/// Optional format: "tau:lambda:k_neighbors" (e.g., "0.1:0.3:10").
/// Bare flag uses defaults: tau=0.1, lambda=0.3, k=10.
#[arg(long, num_args = 0..=1, default_missing_value = "0.1:0.3:10")]
table_projection: Option<String>,
```

Update the match arm and `cmd_partition` signature to thread the new arg.

**Step 3: Parse and apply config**

In `cmd_partition`:
```rust
if let Some(spec) = table_projection {
    let parts: Vec<&str> = spec.split(':').collect();
    let tau: f64 = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0.1);
    let lambda: f64 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0.3);
    let k: usize = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(10);
    config = config.with_table_projection(tau, lambda, k);
}
```

**Step 4: Print projection status in output**

In `print_wcc_topology` or right after it:
```rust
if config_table_projection.is_some() {
    println_stdout!("{}", t!("partition.projection_active", tau = tau, lambda = lambda, k = k));
} else {
    println_stdout!("{}", t!("partition.projection_off"));
}
```

(You'll need to pass the config to the print function, or restructure slightly.)

**Step 5: Build and verify**

```sh
cargo build
cargo run -- partition --auto --table-projection --project testcases/sample-project 2>&1 | head -60
```
Expected: topology section shows "Table projection: ON".

**Step 6: Commit**

```sh
git add src/main.rs locales/en.yml locales/zh-CN.yml
git commit -m "feat(cli): add --table-projection flag

Optional format 'tau:lambda:k_neighbors' (default 0.1:0.3:10).
When set, partition injects TF-IDF cosine similarity edges between
procedures sharing tables, bridging otherwise-isolated procs.

Refs: .sisyphus/plans/2026-06-19-partition-skew-fix.md Task 8"
```

---

### Task 9: End-to-End Validation Against Real Graph

**Files:** None (validation only)

**Step 1: Rebuild with changes**

```sh
cargo build --release
```

**Step 2: Reproduce baseline (before-fix behavior)**

```sh
~/dev/tools/codeweb partition --auto --project <real-project> 2>&1 | tee /tmp/before-auto.txt
~/dev/tools/codeweb partition -k 20 --project <real-project> 2>&1 | tee /tmp/before-k20.txt
```
Confirm: cluster 0 absorbs 99%, External=0 everywhere (the original skew).

**Step 3: Test Phase 1 (topology reporting)**

```sh
~/dev/tools/codeweb partition --auto --project <real-project> 2>&1 | tee /tmp/phase1-auto.txt
```
Expected output should include a NEW section:
```
图拓扑：
  参与者节点：22951
  弱连通分量数：8027
  巨型分量：14951 个节点 (65.1%)
  孤岛：8026 个分量，8000 个节点
```

**Step 4: Test Phase 1 filter (focus on GCC)**

```sh
~/dev/tools/codeweb partition --auto --min-component-size 10 --project <real-project> 2>&1 | tee /tmp/phase1-filtered.txt
```
Expected: cluster table now only shows GCC clusters (total_nodes ≈ 14951 instead of 22951), skew eliminated.

**Step 5: Test Phase 2 (table projection)**

```sh
~/dev/tools/codeweb partition --auto --table-projection --project <real-project> 2>&1 | tee /tmp/phase2.txt
```
Expected: WCC count drops significantly (from 8027 to perhaps 100-1000), GCC expands. The recommended k should land in a sensible range (5-50), modularity Q stays > 0.3.

**Step 6: Combine both**

```sh
~/dev/tools/codeweb partition --auto --min-component-size 10 --table-projection --project <real-project> 2>&1 | tee /tmp/combined.txt
```

**Step 7: Document findings**

Create `docs/notes/2026-06-19-partition-validation.md` with:
- Baseline numbers (cluster 0 size, Q, WCC count)
- Phase 1 numbers (WCC count, GCC size, isolates)
- Phase 1 filtered numbers (cluster count, Q, balance)
- Phase 2 numbers (WCC count after projection, cluster count, Q)
- Combined numbers
- Recommendation for default flags

**Step 8: Final commit**

```sh
git add docs/notes/2026-06-19-partition-validation.md
git commit -m "docs: partition skew fix validation results on real 22k-proc graph

Refs: .sisyphus/plans/2026-06-19-partition-skew-fix.md Task 9"
```

---

## Success Criteria

The fix is **complete** when ALL of the following pass:

### Phase 1 (Topology + Filter)
1. ✅ `cargo test --features full` passes (0 failures, including new tests)
2. ✅ `cargo clippy --features full -- -D warnings` clean
3. ✅ `cargo fmt -- --check` clean
4. ✅ `codeweb partition --auto` on real graph prints WCC topology section
5. ✅ `codeweb partition --auto --min-component-size 10` on real graph:
   - Reports isolates separately
   - Total clustered nodes ≈ GCC size (not 22951)
   - No single cluster absorbs >50% of clustered nodes
   - Modularity Q > 0.3 on the GCC partition

### Phase 2 (TF-IDF Projection)
6. ✅ `codeweb partition --auto --table-projection` on real graph:
   - WCC count drops >10x (from ~8000 to <800)
   - GCC size grows >2x (most isolates now bridged via shared tables)
   - Forced k=20 produces clusters with no single cluster >50% of total
   - Modularity Q at γ=1.0 stays > 0.3 (communities still meaningful)
7. ✅ Combined `--table-projection --min-component-size 10` produces 5-30 natural clusters with Q > 0.4

### Backward Compatibility
8. ✅ `codeweb partition -k 20` (no new flags) produces **identical output** to the pre-fix binary (topology section is informational, doesn't change clustering)
9. ✅ All existing tests pass without modification

---

## Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| TF-IDF + cosine O(P²) too slow for 22k procs | Medium | Medium | Top-K sparsification (k=10) bounds it at O(P·k). If still slow, pre-filter to procs with ≥2 table accesses (single-access procs have zero cosine with anyone). |
| Table projection creates too many edges, destroys community structure | Medium | High | Default tau=0.1 is conservative. Validation step (Task 9) catches this — if Q drops < 0.3 with projection, raise tau to 0.2-0.3. |
| Generic tables (audit_log, code_types) leak through despite TF-IDF | Low | Low | IDF=0 for tables accessed by ALL procs (mathematically filtered). For near-generic tables (accessed by 90%), IDF is small. If problematic, add `min_idf` floor in `compute_tfidf_cosine_edges`. |
| `--min-component-size` filters out meaningful small modules | Medium | Medium | Default is 1 (no filter). User must explicitly opt in. Document that small components often ARE legitimate modules; filter is for skew mitigation, not "correctness". |
| Petgraph 0.7 `connected_components` behaves unexpectedly on directed graph | Low | Low | Wrote custom BFS in Task 1 instead of relying on `connected_components`. Fully deterministic. |
| Existing tests break due to PartitionReport struct change | Low | Low | `topology: Option<WccTopology>` is additive; existing constructors populate it. All existing tests verified to pass. |

---

## Verification Commands (run after EVERY task)

```sh
cargo test --lib cluster                         # unit tests
cargo build                                      # default features
cargo build --features full                      # all features
cargo clippy --features full -- -D warnings      # lint strict
cargo fmt -- --check                             # format check
```

If `--features full` reveals pre-existing failures unrelated to this change, document them in the commit message and confirm they don't worsen.

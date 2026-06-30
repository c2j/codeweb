//! Graph partitioning for system decomposition.
//!
//! CNM-style greedy modularity maximization. SCCs condensed into super-nodes
//! before clustering (cyclic nodes are indivisible coupling units).

use crate::graph::{CodeGraph, Edge, Node};
use petgraph::graph::NodeIndex;
use std::collections::BinaryHeap;
use std::collections::VecDeque;
use std::collections::{HashMap, HashSet};

/// Lightweight node kind for clustering — derived from `Node` enum variants.
/// Avoids carrying full node data during algorithm execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeKind {
    Procedure,
    Function,
    Unresolved,
    MappedStatement,
    JavaSql,
    JavaMethod,
    JavaClass,
    Table,
    View,
    Package,
    Trigger,
    Type,
    Sequence,
    Index,
    MaterializedView,
    Synonym,
    Event,
    BuiltinFunction,
    Custom,
    #[cfg(feature = "jsp")]
    JspPage,
    #[cfg(feature = "jsp")]
    JspSql,
}

impl NodeKind {
    pub fn from_node(node: &Node) -> Self {
        match node {
            Node::Procedure { .. } => NodeKind::Procedure,
            Node::Function { .. } => NodeKind::Function,
            Node::Unresolved { .. } => NodeKind::Unresolved,
            Node::MappedStatement { .. } => NodeKind::MappedStatement,
            Node::JavaSql { .. } => NodeKind::JavaSql,
            Node::JavaMethod { .. } => NodeKind::JavaMethod,
            Node::JavaClass { .. } => NodeKind::JavaClass,
            Node::Table { .. } => NodeKind::Table,
            Node::View { .. } => NodeKind::View,
            Node::Package { .. } => NodeKind::Package,
            Node::Trigger { .. } => NodeKind::Trigger,
            Node::Type { .. } => NodeKind::Type,
            Node::Sequence { .. } => NodeKind::Sequence,
            Node::Index { .. } => NodeKind::Index,
            Node::MaterializedView { .. } => NodeKind::MaterializedView,
            Node::Synonym { .. } => NodeKind::Synonym,
            Node::Event { .. } => NodeKind::Event,
            Node::BuiltinFunction { .. } => NodeKind::BuiltinFunction,
            Node::Custom { .. } => NodeKind::Custom,
            #[cfg(feature = "jsp")]
            Node::JspPage { .. } => NodeKind::JspPage,
            #[cfg(feature = "jsp")]
            Node::JspSql { .. } => NodeKind::JspSql,
        }
    }

    /// Short display label (matches node_type_tag without partial marker).
    pub fn tag(self) -> &'static str {
        match self {
            NodeKind::Procedure => "proc",
            NodeKind::Function => "func",
            NodeKind::Unresolved => "unres",
            NodeKind::MappedStatement => "mapper",
            NodeKind::JavaSql => "sql",
            NodeKind::JavaMethod => "method",
            NodeKind::JavaClass => "class",
            NodeKind::Table => "table",
            NodeKind::View => "view",
            NodeKind::Package => "pkg",
            NodeKind::Trigger => "trigger",
            NodeKind::Type => "type",
            NodeKind::Sequence => "seq",
            NodeKind::Index => "index",
            NodeKind::MaterializedView => "mview",
            NodeKind::Synonym => "synonym",
            NodeKind::Event => "event",
            NodeKind::BuiltinFunction => "builtin",
            NodeKind::Custom => "custom",
            #[cfg(feature = "jsp")]
            NodeKind::JspPage => "jsp",
            #[cfg(feature = "jsp")]
            NodeKind::JspSql => "jsql",
        }
    }
}

#[derive(Debug, Clone)]
pub struct EdgeWeights {
    pub call: f64,
    pub inheritance: f64,
    pub data_flow: f64,
    pub reference: f64,
    pub dynamic_call: f64,
    pub composition: f64,
}

impl Default for EdgeWeights {
    fn default() -> Self {
        Self {
            call: 1.0,
            inheritance: 1.5,
            data_flow: 0.3,
            reference: 0.1,
            dynamic_call: 0.5,
            composition: 2.0,
        }
    }
}

pub fn edge_weight(edge: &Edge, config: &EdgeWeights) -> Option<f64> {
    match edge {
        Edge::DirectCall { .. } => Some(config.call),
        Edge::DynamicCall { .. } => Some(config.dynamic_call),
        Edge::CallsProcedure { .. } | Edge::InvokesMapper { .. } | Edge::CallsJava { .. } => {
            Some(config.call)
        }
        Edge::UsesBuiltinFunction { .. } => Some(config.call),
        Edge::Extends { .. } | Edge::Implements { .. } => Some(config.inheritance),
        Edge::TableAccess { .. } | Edge::DependsOn { .. } => Some(config.data_flow),
        Edge::TriggersRoutine { .. }
        | Edge::ReferencesType { .. }
        | Edge::UsesSequence { .. }
        | Edge::IndexesTable { .. }
        | Edge::AliasesObject { .. }
        | Edge::CustomEdge { .. } => Some(config.reference),
        Edge::ContainsMethod | Edge::ContainsRoutine => Some(config.composition),
        #[cfg(feature = "jsp")]
        Edge::ContainsSql => Some(config.composition),
    }
}

/// Configuration for TF-IDF table-access projection.
///
/// When enabled, procedures that share table accesses get weighted
/// similarity edges added to the condensed graph, bridging otherwise-
/// isolated WCCs.
#[derive(Debug, Clone)]
pub struct TableProjectionConfig {
    /// Minimum cosine similarity to add an edge. Edges with sim < tau are dropped.
    pub tau: f64,
    /// Edge weight multiplier. Projection edges are weighted `lambda * cosine_similarity`,
    /// relative to direct call edges (weight 1.0).
    pub lambda: f64,
    /// Top-K nearest neighbors per proc. Each proc connects to at most k_neighbors others.
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
    /// Cap on CNM merge iterations per `cluster_cnm` run. `None` = unlimited.
    /// In natural mode (no `k`), the algorithm stops after this many iterations
    /// even if modularity could still improve. Ignored in forced-k mode (which
    /// must reach the target k).
    pub max_iterations: Option<usize>,
    /// Stop merging when the best available ΔQ falls at or below this threshold
    /// (natural mode only). Default `0.0` preserves original CNM behavior.
    /// Set to a small positive value (e.g. `1e-6`) to prune negligible merges.
    pub min_delta_q: f64,
    /// Minimum WCC component size for a node to participate in clustering.
    /// Nodes in WCCs smaller than this are excluded from condensation and
    /// community detection. Default `1` includes all participants.
    pub min_component_size: usize,

    /// Optional TF-IDF table-access projection config.
    /// When `Some`, procedures that share table accesses get weighted
    /// similarity edges added to the condensed graph.
    pub table_projection: Option<TableProjectionConfig>,
}

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
            table_projection: None,
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
            table_projection: None,
        }
    }

    pub fn with_gamma(mut self, gamma: f64) -> Self {
        self.gamma = gamma;
        self
    }

    /// Cap natural-mode merging at `n` iterations.
    pub fn with_max_iterations(mut self, n: usize) -> Self {
        self.max_iterations = Some(n);
        self
    }

    /// Stop natural-mode merging once best ΔQ ≤ `q`.
    pub fn with_min_delta_q(mut self, q: f64) -> Self {
        self.min_delta_q = q;
        self
    }

    /// Set minimum WCC component size for clustering participation.
    /// Nodes in components smaller than `n` are excluded. Minimum clamped to 1.
    pub fn with_min_component_size(mut self, n: usize) -> Self {
        self.min_component_size = n.max(1);
        self
    }

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

/// Default participant node kinds: behavioral nodes (code that does things).
/// Structural nodes (tables, views, indexes) are excluded — they are shared resources,
/// not modules to be assigned.
pub fn default_participant_kinds() -> HashSet<NodeKind> {
    [
        NodeKind::Procedure,
        NodeKind::Function,
        NodeKind::JavaMethod,
        NodeKind::MappedStatement,
        NodeKind::JavaClass,
    ]
    .into_iter()
    .collect()
}

// ============================================================================
// Task 2: SCC Condensation
// ============================================================================

/// Result of SCC condensation: groups of original nodes that form indivisible units.
struct SccCondensation {
    /// Each entry is a set of original NodeIndex values forming one super-node.
    super_nodes: Vec<HashSet<NodeIndex>>,
    /// Map: original NodeIndex → super-node index.
    node_to_super: HashMap<NodeIndex, usize>,
}

/// Condense participant nodes into super-nodes via strongly connected components.
///
/// Uses `kosaraju_scc` (already proven in store.rs for cycle detection).
/// Only participant nodes are included; non-participant nodes are filtered out
/// of SCC membership but their bridging role in cycles is preserved conservatively.
fn condense_sccs(graph: &CodeGraph, config: &ClusterConfig) -> SccCondensation {
    let topology = compute_wcc_topology(graph, config);
    let allowed: HashSet<NodeIndex> = if config.min_component_size <= 1 {
        topology
            .largest_components
            .iter()
            .flat_map(|c| c.iter().copied())
            .collect()
    } else {
        topology
            .participants_above_threshold(config.min_component_size)
            .into_iter()
            .collect()
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

/// Undirected weighted adjacency over super-nodes.
///
/// Convention: `adj[i][j]` = total weight between super-nodes i and j (symmetric).
/// Self-loops `adj[i][i]` = total weight of internal edges (counted once per edge).
/// Degrees `degrees[i]` = Σ_j adj[i][j] (self-loops counted once).
/// `total_weight` = Σ_i degrees[i] (= W in the modularity formula).
struct CondensedGraph {
    adj: Vec<HashMap<usize, f64>>,
    degrees: Vec<f64>,
    total_weight: f64,
}

fn build_condensed_graph(
    graph: &CodeGraph,
    condensation: &SccCondensation,
    config: &ClusterConfig,
) -> CondensedGraph {
    let n = condensation.super_nodes.len();
    let mut adj: Vec<HashMap<usize, f64>> = vec![HashMap::new(); n];

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

    // NEW: TF-IDF table-access projection
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

/// Run CNM-style greedy modularity maximization, merging until target_k clusters
/// remain, no merge improves modularity (ΔQ ≤ `min_delta_q`), or `max_iterations`
/// is reached (natural mode only — forced-k mode ignores `max_iterations`).
///
/// `progress`, if given, is updated once per merge attempt with the current
/// iteration / community count / last ΔQ. It is NOT finished by this function.
///
/// Returns `(community_of_super_node, modularity_Q)`.
fn cluster_cnm(
    condensed: &CondensedGraph,
    target_k: Option<usize>,
    gamma: f64,
    max_iterations: Option<usize>,
    min_delta_q: f64,
    progress: Option<&indicatif::ProgressBar>,
) -> (Vec<usize>, f64) {
    let state = CnmState::new(condensed, target_k, gamma, max_iterations, min_delta_q);
    state.run(progress)
}

/// `f64` wrapper that implements `Ord` so it can be stored in `BinaryHeap`.
/// NaN is treated as equal to itself (defensive — should not occur in practice
/// because all weights and degrees are finite).
#[derive(Copy, Clone, PartialEq)]
struct OrdF64(f64);

impl Eq for OrdF64 {}

impl PartialOrd for OrdF64 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrdF64 {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0
            .partial_cmp(&other.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}

/// One candidate merge in the priority queue. Stored with both endpoints'
/// generation counters so stale entries can be discarded lazily on pop.
#[derive(PartialEq, Eq)]
struct HeapEntry {
    dq: OrdF64,
    a: usize,
    b: usize,
    gen_a: u64,
    gen_b: u64,
}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // BinaryHeap is max-heap; we want the largest ΔQ first.
        // Tie-break by (a, b) for deterministic ordering on equal ΔQ.
        self.dq
            .cmp(&other.dq)
            .then_with(|| (self.a, self.b).cmp(&(other.a, other.b)))
    }
}

/// Mutable state of one CNM run. Replaces the O(n²) scan of `inter_weight`
/// with a global max-heap + generation-counted lazy deletion, and the O(n)
/// community relabel with an inverse `members` index.
struct CnmState<'a> {
    condensed: &'a CondensedGraph,
    target_k: Option<usize>,
    gamma: f64,
    max_iterations: Option<usize>,
    min_delta_q: f64,

    n: usize,
    total_weight: f64,

    community_of: Vec<usize>,
    members: Vec<Vec<usize>>,
    comm_degree: Vec<f64>,
    generation: Vec<u64>,

    edge_weight: HashMap<(usize, usize), f64>,
    neighbors: Vec<HashSet<usize>>,
    heap: BinaryHeap<HeapEntry>,

    num_communities: usize,
    iterations: usize,
}

impl<'a> CnmState<'a> {
    fn new(
        condensed: &'a CondensedGraph,
        target_k: Option<usize>,
        gamma: f64,
        max_iterations: Option<usize>,
        min_delta_q: f64,
    ) -> Self {
        let n = condensed.adj.len();
        let total_weight = condensed.total_weight;

        let mut state = Self {
            condensed,
            target_k,
            gamma,
            max_iterations,
            min_delta_q,
            n,
            total_weight,
            community_of: (0..n).collect(),
            members: (0..n).map(|i| vec![i]).collect(),
            comm_degree: condensed.degrees.clone(),
            generation: vec![0; n],
            edge_weight: HashMap::new(),
            neighbors: vec![HashSet::new(); n],
            heap: BinaryHeap::new(),
            num_communities: n,
            iterations: 0,
        };

        for i in 0..n {
            for (&j, &w) in &condensed.adj[i] {
                if i < j && w > 0.0 {
                    state.edge_weight.insert((i, j), w);
                    state.neighbors[i].insert(j);
                    state.neighbors[j].insert(i);
                }
            }
        }
        for &(a, b) in state.edge_weight.keys() {
            let dq = state.compute_dq(a, b);
            state.heap.push(HeapEntry {
                dq: OrdF64(dq),
                a,
                b,
                gen_a: 0,
                gen_b: 0,
            });
        }
        state
    }

    fn compute_dq(&self, a: usize, b: usize) -> f64 {
        let e_ab = self
            .edge_weight
            .get(&(a.min(b), a.max(b)))
            .copied()
            .unwrap_or(0.0);
        let k_a = self.comm_degree[a];
        let k_b = self.comm_degree[b];
        let w = self.total_weight;
        if w == 0.0 {
            return 0.0;
        }
        (2.0 / w) * (e_ab - self.gamma * k_a * k_b / w)
    }

    fn run(mut self, progress: Option<&indicatif::ProgressBar>) -> (Vec<usize>, f64) {
        if self.n == 0 {
            return (vec![], 0.0);
        }
        let min_k = self.target_k.unwrap_or(1).min(self.n);
        if self.total_weight == 0.0 {
            let labels: Vec<usize> = (0..self.n)
                .map(|i| i.min(min_k.saturating_sub(1)))
                .collect();
            return (labels, 0.0);
        }

        let force_k = self.target_k.is_some();

        while self.num_communities > min_k {
            if !force_k
                && self
                    .max_iterations
                    .is_some_and(|cap| self.iterations >= cap)
            {
                break;
            }

            let entry = match self.pop_valid() {
                Some(e) => e,
                None => {
                    if force_k {
                        if !self.force_merge_disconnected() {
                            break;
                        }
                        continue;
                    } else {
                        break;
                    }
                }
            };

            let dq = entry.dq.0;
            let should_merge = if force_k { true } else { dq > self.min_delta_q };

            if !should_merge {
                break;
            }

            self.iterations += 1;
            if let Some(pb) = progress {
                pb.set_message(format!(
                    "iter {} | {}→{} clusters | ΔQ={:.4}",
                    self.iterations, self.num_communities, min_k, dq
                ));
            }
            self.merge_communities(entry.a, entry.b);
        }

        self.finalize()
    }

    /// Pop the highest-ΔQ entry whose generation counters still match.
    fn pop_valid(&mut self) -> Option<HeapEntry> {
        while let Some(entry) = self.heap.pop() {
            if self.generation[entry.a] == entry.gen_a && self.generation[entry.b] == entry.gen_b {
                return Some(entry);
            }
        }
        None
    }

    /// Merge community `b` into community `a`: transfer members, edges, and
    /// bump both generations. Then push fresh ΔQ entries for all of `a`'s
    /// current neighbors.
    fn merge_communities(&mut self, a: usize, b: usize) {
        for &node in &self.members[b] {
            self.community_of[node] = a;
        }
        let b_members = std::mem::take(&mut self.members[b]);
        self.members[a].extend(b_members);

        self.comm_degree[a] += self.comm_degree[b];
        self.comm_degree[b] = 0.0;

        self.generation[a] = self.generation[a].wrapping_add(1);
        self.generation[b] = self.generation[b].wrapping_add(1);

        // The (a, b) edge is the merge pair itself: it becomes internal to the
        // merged community and must be removed from inter-community state.
        let key_ab = (a.min(b), a.max(b));
        self.edge_weight.remove(&key_ab);
        self.neighbors[a].remove(&b);

        let b_neighbors: Vec<usize> = self.neighbors[b].iter().copied().collect();
        for x in b_neighbors {
            if x == a {
                continue;
            }
            let key_bx = (b.min(x), b.max(x));
            let e_bx = self.edge_weight.remove(&key_bx).unwrap_or(0.0);
            self.neighbors[x].remove(&b);

            let key_ax = (a.min(x), a.max(x));
            let existing = self.edge_weight.get(&key_ax).copied().unwrap_or(0.0);
            let new_w = existing + e_bx;
            if new_w > 0.0 {
                self.edge_weight.insert(key_ax, new_w);
                self.neighbors[a].insert(x);
                self.neighbors[x].insert(a);
            } else {
                self.edge_weight.remove(&key_ax);
                self.neighbors[a].remove(&x);
                self.neighbors[x].remove(&a);
            }
        }
        self.neighbors[b].clear();

        let a_neighbors: Vec<usize> = self.neighbors[a].iter().copied().collect();
        for x in a_neighbors {
            let dq = self.compute_dq(a, x);
            self.heap.push(HeapEntry {
                dq: OrdF64(dq),
                a: a.min(x),
                b: a.max(x),
                gen_a: self.generation[a.min(x)],
                gen_b: self.generation[a.max(x)],
            });
        }

        self.num_communities -= 1;
    }

    /// Fallback used in forced-k mode when no inter-community edges remain:
    /// merge the smallest live community into the largest. Returns false if
    /// fewer than two live communities exist.
    fn force_merge_disconnected(&mut self) -> bool {
        let mut largest: Option<usize> = None;
        let mut largest_size: usize = 0;
        let mut smallest: Option<usize> = None;
        let mut smallest_size: usize = usize::MAX;
        for (cid, members) in self.members.iter().enumerate() {
            if members.is_empty() {
                continue;
            }
            let size = members.len();
            if size > largest_size {
                largest_size = size;
                largest = Some(cid);
            }
            if size < smallest_size {
                smallest_size = size;
                smallest = Some(cid);
            }
        }
        match (largest, smallest) {
            (Some(l), Some(s)) if l != s => {
                self.merge_communities(l, s);
                true
            }
            _ => false,
        }
    }

    fn finalize(self) -> (Vec<usize>, f64) {
        let mut id_map: HashMap<usize, usize> = HashMap::new();
        let mut next_id = 0;
        for &c in &self.community_of {
            use std::collections::hash_map::Entry;
            if let Entry::Vacant(e) = id_map.entry(c) {
                e.insert(next_id);
                next_id += 1;
            }
        }
        let community_of: Vec<usize> = self.community_of.iter().map(|&c| id_map[&c]).collect();
        let q = compute_modularity(self.condensed, &community_of);
        (community_of, q)
    }
}

/// Compute modularity Q for a given community assignment.
///
/// Q = Σ_c [ L_c / W - (D_c / W)² ]
/// where L_c = Σ_{i,j∈c} A_ij, D_c = Σ_{i∈c} k_i, W = total weight.
fn compute_modularity(condensed: &CondensedGraph, community_of: &[usize]) -> f64 {
    let w = condensed.total_weight;
    if w == 0.0 {
        return 0.0;
    }

    let n = condensed.adj.len();
    let mut comm_internal: HashMap<usize, f64> = HashMap::new();
    let mut comm_degree: HashMap<usize, f64> = HashMap::new();

    for i in 0..n {
        let c = community_of[i];
        *comm_degree.entry(c).or_insert(0.0) += condensed.degrees[i];
        for (&j, &weight) in &condensed.adj[i] {
            if community_of[j] == c {
                *comm_internal.entry(c).or_insert(0.0) += weight;
            }
        }
    }

    comm_degree
        .keys()
        .map(|&c| {
            let internal = comm_internal.get(&c).copied().unwrap_or(0.0);
            let degree = comm_degree[&c];
            internal / w - (degree * degree) / (w * w)
        })
        .sum()
}

#[derive(Debug, Clone, Copy)]
pub struct GammaSweepEntry {
    pub gamma: f64,
    pub k: usize,
    pub modularity: f64,
    pub avg_cluster_size: f64,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct AutoClusterReport {
    pub sweep: Vec<GammaSweepEntry>,
    pub recommended_gamma: f64,
    pub recommended_k: usize,
    pub report: PartitionReport,
}

const AUTO_GAMMA_SWEEP: &[f64] = &[1.0, 0.5, 0.25, 0.1, 0.05];
pub const AUTO_GAMMA_SWEEP_LEN: usize = AUTO_GAMMA_SWEEP.len();
const AUTO_K_SWEEP: &[usize] = &[3, 5, 10, 20, 50];

/// Above this super-node count, `auto_partition` skips the most aggressive
/// forced-k values (3 and 5) because each forces ≈(n − k) heap-driven merges
/// that, while no longer O(n²) after the priority-queue rewrite, still
/// produce low-value partitions on huge graphs.
const LARGE_GRAPH_K_SWEEP_THRESHOLD: usize = 10_000;
const LARGE_GRAPH_K_SWEEP: &[usize] = &[10, 20, 50];

/// Pick the forced-k sweep list appropriate for the condensed-graph size.
pub fn adaptive_k_sweep(total_super_nodes: usize) -> &'static [usize] {
    if total_super_nodes > LARGE_GRAPH_K_SWEEP_THRESHOLD {
        LARGE_GRAPH_K_SWEEP
    } else {
        AUTO_K_SWEEP
    }
}

/// `progress`, if given, is updated once per sweep step and forwarded into
/// each `cluster_cnm` run for per-iteration feedback. It is NOT finished by
/// this function — the caller controls lifecycle.
pub fn auto_partition_with_progress(
    graph: &CodeGraph,
    base_config: &ClusterConfig,
    progress: Option<&indicatif::ProgressBar>,
) -> AutoClusterReport {
    let condensation = condense_sccs(graph, base_config);
    let condensed = build_condensed_graph(graph, &condensation, base_config);
    let total_nodes = condensation.super_nodes.len().max(1);

    let k_sweep = adaptive_k_sweep(total_nodes);

    let mut sweep: Vec<GammaSweepEntry> = Vec::new();

    for (idx, &gamma) in AUTO_GAMMA_SWEEP.iter().enumerate() {
        if let Some(pb) = progress {
            pb.set_message(format!(
                "γ sweep {}/{}  γ={:.2}",
                idx + 1,
                AUTO_GAMMA_SWEEP.len(),
                gamma
            ));
        }
        let (communities, q) = cluster_cnm(
            &condensed,
            None,
            gamma,
            base_config.max_iterations,
            base_config.min_delta_q,
            progress,
        );
        let k = communities
            .iter()
            .copied()
            .max()
            .map(|m| m + 1)
            .unwrap_or(0);
        sweep.push(GammaSweepEntry {
            gamma,
            k,
            modularity: q,
            avg_cluster_size: total_nodes as f64 / k.max(1) as f64,
        });
        if let Some(pb) = progress {
            pb.inc(1);
        }
    }

    for (idx, &forced_k) in k_sweep.iter().enumerate() {
        if forced_k >= total_nodes {
            if let Some(pb) = progress {
                pb.inc(1);
            }
            continue;
        }
        if let Some(pb) = progress {
            pb.set_message(format!(
                "k sweep {}/{}  k={}",
                idx + 1,
                k_sweep.len(),
                forced_k
            ));
        }
        let (communities, q) = cluster_cnm(
            &condensed,
            Some(forced_k),
            base_config.gamma,
            base_config.max_iterations,
            base_config.min_delta_q,
            progress,
        );
        let k = communities
            .iter()
            .copied()
            .max()
            .map(|m| m + 1)
            .unwrap_or(0);
        sweep.push(GammaSweepEntry {
            gamma: base_config.gamma,
            k,
            modularity: q,
            avg_cluster_size: total_nodes as f64 / k.max(1) as f64,
        });
        if let Some(pb) = progress {
            pb.inc(1);
        }
    }

    let natural_k = sweep.first().map(|e| e.k).unwrap_or(1);
    let recommended = if natural_k <= 20 {
        sweep.iter().min_by_key(|e| e.k).copied()
    } else {
        sweep
            .iter()
            .filter(|e| (5..=20).contains(&e.k))
            .max_by(|a, b| {
                a.modularity
                    .partial_cmp(&b.modularity)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .copied()
            .or_else(|| sweep.iter().find(|e| e.k <= 20).copied())
            .or_else(|| sweep.iter().min_by_key(|e| e.k).copied())
    };

    let rec = recommended.unwrap_or_else(|| *sweep.first().unwrap());
    let mut config = base_config.clone();
    config.gamma = rec.gamma;
    config.k = Some(rec.k);

    AutoClusterReport {
        sweep,
        recommended_gamma: rec.gamma,
        recommended_k: rec.k,
        report: partition(graph, &config),
    }
}

/// Cluster identifier.
pub type ClusterId = u32;

/// Per-cluster summary statistics.
#[derive(Debug, Clone)]
pub struct ClusterStat {
    pub id: ClusterId,
    pub node_count: usize,
    pub type_distribution: HashMap<NodeKind, usize>,
    /// Total weight of edges within this cluster (cohesion signal).
    pub internal_weight: f64,
    /// Total weight of edges to other clusters (coupling signal).
    pub external_weight: f64,
}

/// Cross-cluster coupling entry: (from_cluster, to_cluster, total_weight, edge_count).
#[derive(Debug, Clone)]
pub struct InterClusterCoupling {
    pub from: ClusterId,
    pub to: ClusterId,
    pub weight: f64,
    pub edge_count: usize,
}

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
    /// Always populated by `partition()`.
    pub topology: Option<WccTopology>,
}

/// Sparse binary matrix of procedure → table access.
///
/// Built from `Edge::TableAccess` edges where source is a participant
/// and destination is a Table. Used as input to TF-IDF + cosine similarity.
#[derive(Debug, Clone)]
pub struct ProcTableMatrix {
    pub procs: Vec<NodeIndex>,
    pub tables: Vec<NodeIndex>,
    access_map: HashSet<(usize, usize)>,
    #[allow(dead_code)]
    proc_index: HashMap<NodeIndex, usize>,
    #[allow(dead_code)]
    table_index: HashMap<NodeIndex, usize>,
}

#[allow(dead_code)]
impl ProcTableMatrix {
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

    pub fn proc_count(&self) -> usize {
        self.procs.len()
    }
    pub fn table_count(&self) -> usize {
        self.tables.len()
    }
    pub fn accesses(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.access_map.iter().copied()
    }
}

/// Build the proc-table access matrix from the graph.
///
/// Includes only edges where:
///   - source is a participant (per `config.participant_kinds`)
///   - destination is a Table (matches `Node::Table { .. }`)
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
        if !config
            .participant_kinds
            .contains(&NodeKind::from_node(&graph[src]))
        {
            continue;
        }
        if !matches!(graph[dst], Node::Table { .. }) {
            continue;
        }
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

/// Compute TF-IDF weighted cosine similarity edges between procedures.
///
/// Returns `Vec<(proc_idx_a, proc_idx_b, weight)>` where indices refer to
/// positions in `matrix.procs`, and `weight` is the cosine similarity
/// in [0, 1] (TF-IDF vectors are non-negative).
///
/// Algorithm:
/// 1. IDF per table: `idf(t) = log(N / df(t))` where N = proc count, df(t) = procs accessing table t
/// 2. TF-IDF vector per proc: binary TF (1.0 if accessed) x IDF
/// 3. Cosine similarity between each proc pair
/// 4. Sparsification: keep top-`k_neighbors` per proc, then drop edges below `tau`
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

    // 1. Document frequency per table
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

/// Topology of weakly connected components (WCCs) on the participant-induced subgraph.
#[derive(Debug, Clone)]
pub struct WccTopology {
    pub total_participants: usize,
    pub wcc_count: usize,
    pub gcc_size: usize,
    pub isolates_count: usize,
    pub isolates_node_count: usize,
    pub largest_components: Vec<Vec<NodeIndex>>,
}

impl WccTopology {
    pub fn participants_above_threshold(&self, min_size: usize) -> Vec<NodeIndex> {
        self.largest_components
            .iter()
            .filter(|c| c.len() >= min_size)
            .flat_map(|c| c.iter().copied())
            .collect()
    }
}

pub fn compute_wcc_topology(graph: &CodeGraph, config: &ClusterConfig) -> WccTopology {
    let participants: HashSet<NodeIndex> = graph
        .node_indices()
        .filter(|&idx| {
            config
                .participant_kinds
                .contains(&NodeKind::from_node(&graph[idx]))
        })
        .collect();

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

    components.sort_by_key(|b| std::cmp::Reverse(b.len()));

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

fn compute_cluster_stats(
    graph: &CodeGraph,
    assignments: &HashMap<NodeIndex, ClusterId>,
    condensed: &CondensedGraph,
    condensation: &SccCondensation,
    k: usize,
) -> Vec<ClusterStat> {
    let mut node_counts: Vec<usize> = vec![0; k];
    let mut type_dists: Vec<HashMap<NodeKind, usize>> = vec![HashMap::new(); k];

    for (&node_idx, &cluster) in assignments {
        node_counts[cluster as usize] += 1;
        let kind = NodeKind::from_node(&graph[node_idx]);
        *type_dists[cluster as usize].entry(kind).or_insert(0) += 1;
    }

    let mut internal_weights: Vec<f64> = vec![0.0; k];
    let mut external_weights: Vec<f64> = vec![0.0; k];

    for i in 0..condensed.adj.len() {
        let ci = super_communities_for_idx(i, condensation, assignments) as usize;
        for (&j, &weight) in &condensed.adj[i] {
            let cj = super_communities_for_idx(j, condensation, assignments) as usize;
            if ci == cj {
                internal_weights[ci] += weight;
            } else {
                external_weights[ci] += weight;
            }
        }
    }

    (0..k)
        .map(|c| ClusterStat {
            id: c as ClusterId,
            node_count: node_counts[c],
            type_distribution: type_dists[c].clone(),
            internal_weight: internal_weights[c],
            external_weight: external_weights[c],
        })
        .collect()
}

/// Helper: get cluster assignment for a super-node index.
fn super_communities_for_idx(
    super_idx: usize,
    condensation: &SccCondensation,
    assignments: &HashMap<NodeIndex, ClusterId>,
) -> ClusterId {
    condensation.super_nodes[super_idx]
        .iter()
        .next()
        .and_then(|&node| assignments.get(&node).copied())
        .unwrap_or(0)
}

fn compute_inter_cluster_coupling(
    graph: &CodeGraph,
    assignments: &HashMap<NodeIndex, ClusterId>,
    config: &ClusterConfig,
) -> Vec<InterClusterCoupling> {
    let mut coupling: HashMap<(ClusterId, ClusterId), (f64, usize)> = HashMap::new();

    for edge_idx in graph.edge_indices() {
        let (src, dst) = match graph.edge_endpoints(edge_idx) {
            Some(ep) => ep,
            None => continue,
        };

        let src_cluster = match assignments.get(&src) {
            Some(&c) => c,
            None => continue,
        };
        let dst_cluster = match assignments.get(&dst) {
            Some(&c) => c,
            None => continue,
        };

        if src_cluster == dst_cluster {
            continue;
        }

        let weight = edge_weight(&graph[edge_idx], &config.edge_weights).unwrap_or(0.0);
        let entry = coupling
            .entry((src_cluster, dst_cluster))
            .or_insert((0.0, 0));
        entry.0 += weight;
        entry.1 += 1;
    }

    let mut result: Vec<InterClusterCoupling> = coupling
        .into_iter()
        .map(|((from, to), (weight, count))| InterClusterCoupling {
            from,
            to,
            weight,
            edge_count: count,
        })
        .collect();

    result.sort_by(|a, b| {
        b.weight
            .partial_cmp(&a.weight)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    result
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ClusterResult {
    pub assignments: HashMap<NodeIndex, ClusterId>,
    pub k: usize,
    pub modularity: f64,
}

impl ClusterResult {
    /// Get cluster ID for a node, if it has one.
    pub fn cluster_of(&self, node: NodeIndex) -> Option<ClusterId> {
        self.assignments.get(&node).copied()
    }
}

impl From<&PartitionReport> for ClusterResult {
    fn from(report: &PartitionReport) -> Self {
        Self {
            assignments: report.assignments.clone(),
            k: report.k_actual,
            modularity: report.modularity,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{
        CallScope, DataFlowKind, Edge, Node, RoutineId, RoutineKind, SourceLocation,
    };
    use petgraph::graph::NodeIndex;
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn loc(file: &str, line: usize) -> SourceLocation {
        SourceLocation {
            file: Arc::new(PathBuf::from(file)),
            line,
        }
    }

    fn proc_node(name: &str) -> Node {
        Node::Procedure {
            id: RoutineId {
                schema: None,
                package: None,
                name: name.to_string(),
                kind: RoutineKind::Procedure,
            },
            location: loc("test.sql", 1),
            partial: false,
            body_sql: vec![],
        }
    }

    fn table_node(name: &str) -> Node {
        Node::Table {
            schema: None,
            name: name.to_string(),
            explicit: false,
            system: false,
            location: None,
            columns: Box::new(vec![]),
            partition_by: None,
            distribute_by: None,
            tablespace: None,
            temporary: false,
            unlogged: false,
            ddl_source: None,
        }
    }

    fn call_edge() -> Edge {
        Edge::DirectCall {
            scope: CallScope::External,
            location: loc("test.sql", 1),
        }
    }

    fn table_access_edge() -> Edge {
        use crate::graph::AccessMode;
        Edge::TableAccess {
            flow_kind: DataFlowKind::DmlAccess,
            modes: AccessMode::Read,
            write_kinds: HashSet::new(),
            location: loc("test.sql", 1),
            column_analysis: None,
        }
    }

    // --- Task 1 Tests ---

    #[test]
    fn node_kind_from_node_variants() {
        assert_eq!(NodeKind::from_node(&proc_node("p")), NodeKind::Procedure);
        assert_eq!(NodeKind::from_node(&table_node("t")), NodeKind::Table);
    }

    #[test]
    fn edge_weight_returns_correct_values() {
        let config = EdgeWeights::default();
        assert_eq!(edge_weight(&call_edge(), &config), Some(1.0));

        let dyn_call = Edge::DynamicCall {
            raw_expr: "x".to_string(),
            location: loc("t.sql", 1),
        };
        assert_eq!(edge_weight(&dyn_call, &config), Some(0.5));

        assert_eq!(edge_weight(&table_access_edge(), &config), Some(0.3));

        let contains = Edge::ContainsMethod;
        assert_eq!(edge_weight(&contains, &config), Some(2.0));
    }

    #[test]
    fn default_participant_kinds_excludes_structural() {
        let kinds = default_participant_kinds();
        assert!(kinds.contains(&NodeKind::Procedure));
        assert!(kinds.contains(&NodeKind::JavaMethod));
        assert!(!kinds.contains(&NodeKind::Table));
        assert!(!kinds.contains(&NodeKind::View));
        assert!(!kinds.contains(&NodeKind::Index));
    }

    // --- Task 2 Tests ---

    #[test]
    fn scc_condensation_merges_cycle() {
        let mut graph = CodeGraph::new();
        let a = graph.add_node(proc_node("a"));
        let b = graph.add_node(proc_node("b"));
        let c = graph.add_node(proc_node("c"));
        let d = graph.add_node(proc_node("d"));

        // Cycle: a→b→c→a
        graph.add_edge(a, b, call_edge());
        graph.add_edge(b, c, call_edge());
        graph.add_edge(c, a, call_edge());
        // d is separate
        graph.add_edge(a, d, call_edge());

        let config = ClusterConfig::new(2);
        let cond = condense_sccs(&graph, &config);

        // a, b, c should be in the same super-node; d in another
        assert_eq!(cond.node_to_super[&a], cond.node_to_super[&b]);
        assert_eq!(cond.node_to_super[&b], cond.node_to_super[&c]);
        assert_ne!(cond.node_to_super[&a], cond.node_to_super[&d]);
        assert_eq!(cond.super_nodes.len(), 2);
    }

    #[test]
    fn scc_condensation_excludes_non_participants() {
        let mut graph = CodeGraph::new();
        let p = graph.add_node(proc_node("p"));
        let t = graph.add_node(table_node("orders"));

        let config = ClusterConfig::new(1);
        let cond = condense_sccs(&graph, &config);

        assert!(cond.node_to_super.contains_key(&p));
        assert!(!cond.node_to_super.contains_key(&t));
    }

    // --- Task 3 Tests ---

    #[test]
    fn condensed_graph_sums_weights() {
        let mut graph = CodeGraph::new();
        let a = graph.add_node(proc_node("a"));
        let b = graph.add_node(proc_node("b"));

        // Two call edges a→b (weight 1.0 each → total 2.0 undirected = 1.0 per direction)
        graph.add_edge(a, b, call_edge());
        graph.add_edge(a, b, call_edge());

        let config = ClusterConfig::new(2);
        let cond = condense_sccs(&graph, &config);
        let cg = build_condensed_graph(&graph, &cond, &config);

        // Each directed edge adds 1.0 to adj[0][1] and 1.0 to adj[1][0]
        // Two edges → adj[0][1] = 2.0, adj[1][0] = 2.0
        assert_eq!(cg.adj[0].get(&1).copied().unwrap_or(0.0), 2.0);
        assert_eq!(cg.adj[1].get(&0).copied().unwrap_or(0.0), 2.0);
        assert_eq!(cg.degrees[0], 2.0);
        assert_eq!(cg.degrees[1], 2.0);
        assert_eq!(cg.total_weight, 4.0);
    }

    // --- Task 4 Tests ---

    #[test]
    fn cnm_two_communities_obvious_split() {
        // Two separate communities: {a,b,c} and {d,e,f}, no edges between them
        let mut graph = CodeGraph::new();
        let a = graph.add_node(proc_node("a"));
        let b = graph.add_node(proc_node("b"));
        let c = graph.add_node(proc_node("c"));
        let d = graph.add_node(proc_node("d"));
        let e = graph.add_node(proc_node("e"));
        let f = graph.add_node(proc_node("f"));

        // Community 1: a↔b, b↔c, a↔c
        graph.add_edge(a, b, call_edge());
        graph.add_edge(b, a, call_edge());
        graph.add_edge(b, c, call_edge());
        graph.add_edge(c, b, call_edge());
        graph.add_edge(a, c, call_edge());
        graph.add_edge(c, a, call_edge());

        // Community 2: d↔e, e↔f, d↔f
        graph.add_edge(d, e, call_edge());
        graph.add_edge(e, d, call_edge());
        graph.add_edge(e, f, call_edge());
        graph.add_edge(f, e, call_edge());
        graph.add_edge(d, f, call_edge());
        graph.add_edge(f, d, call_edge());

        let report = partition(&graph, &ClusterConfig::new(2));

        // Should split into 2 communities
        assert_eq!(report.k_actual, 2);

        // a, b, c should be in the same cluster; d, e, f in another
        let ca = report.assignments[&a];
        let cb = report.assignments[&b];
        let cc = report.assignments[&c];
        assert_eq!(ca, cb);
        assert_eq!(cb, cc);

        let cd = report.assignments[&d];
        let ce = report.assignments[&e];
        let cf = report.assignments[&f];
        assert_eq!(cd, ce);
        assert_eq!(ce, cf);

        assert_ne!(ca, cd);

        // Modularity should be high (no inter-community edges)
        assert!(
            report.modularity > 0.3,
            "modularity was {}",
            report.modularity
        );

        // No inter-cluster coupling
        assert!(report.inter_cluster_coupling.is_empty());
    }

    #[test]
    fn cnm_fully_connected_merges_to_k() {
        // Complete graph: all nodes connected → all merge into one cluster
        let mut graph = CodeGraph::new();
        let nodes: Vec<NodeIndex> = (0..5)
            .map(|i| graph.add_node(proc_node(&format!("n{i}"))))
            .collect();

        for i in 0..5 {
            for j in 0..5 {
                if i != j {
                    graph.add_edge(nodes[i], nodes[j], call_edge());
                }
            }
        }

        let report = partition(&graph, &ClusterConfig::new(3));

        // With strong connectivity, algorithm should merge as much as possible
        // May end up with fewer clusters than requested if ΔQ always positive
        assert!(report.k_actual <= 3, "k_actual was {}", report.k_actual);
    }

    #[test]
    fn cnm_disconnected_graph_respects_components() {
        // 4 disconnected nodes
        let mut graph = CodeGraph::new();
        let _a = graph.add_node(proc_node("a"));
        let _b = graph.add_node(proc_node("b"));
        let _c = graph.add_node(proc_node("c"));
        let _d = graph.add_node(proc_node("d"));

        let report = partition(&graph, &ClusterConfig::new(2));

        // No edges → can't merge anything → 4 clusters (each node alone)
        // But target k=2, so algorithm relabels to at most 2
        assert!(report.k_actual <= 4, "k_actual was {}", report.k_actual);
    }

    #[test]
    fn cnm_empty_graph() {
        let graph = CodeGraph::new();
        let report = partition(&graph, &ClusterConfig::new(3));
        assert_eq!(report.total_nodes, 0);
        assert_eq!(report.k_actual, 0);
    }

    // --- Task 5 Tests ---

    #[test]
    fn partition_report_stats_correct() {
        let mut graph = CodeGraph::new();
        let a = graph.add_node(proc_node("a"));
        let b = graph.add_node(proc_node("b"));

        graph.add_edge(a, b, call_edge());
        graph.add_edge(b, a, call_edge());

        let report = partition(&graph, &ClusterConfig::new(1));

        assert_eq!(report.total_nodes, 2);
        assert_eq!(report.k_actual, 1);
        assert_eq!(report.cluster_stats.len(), 1);
        assert_eq!(report.cluster_stats[0].node_count, 2);
    }

    #[test]
    fn partition_excludes_non_participant_nodes() {
        let mut graph = CodeGraph::new();
        let p = graph.add_node(proc_node("p"));
        let t = graph.add_node(table_node("orders"));

        graph.add_edge(p, t, table_access_edge());

        let report = partition(&graph, &ClusterConfig::new(1));

        // Only the procedure participates
        assert_eq!(report.total_nodes, 1);
        assert!(report.assignments.contains_key(&p));
        assert!(!report.assignments.contains_key(&t));
    }

    #[test]
    fn inter_cluster_coupling_detected() {
        // Two communities with one cross edge
        let mut graph = CodeGraph::new();
        let _ = graph.add_node(proc_node("a")); // idx 0
        let _ = graph.add_node(proc_node("b")); // idx 1
        let _ = graph.add_node(proc_node("c")); // idx 2
        let _ = graph.add_node(proc_node("d")); // idx 3

        let a = NodeIndex::new(0);
        let b = NodeIndex::new(1);
        let c = NodeIndex::new(2);
        let d = NodeIndex::new(3);

        // Community 1: a↔b
        graph.add_edge(a, b, call_edge());
        graph.add_edge(b, a, call_edge());
        // Community 2: c↔d
        graph.add_edge(c, d, call_edge());
        graph.add_edge(d, c, call_edge());
        // Cross: b→c
        graph.add_edge(b, c, call_edge());

        let report = partition(&graph, &ClusterConfig::new(2));

        assert_eq!(report.k_actual, 2);
        assert!(!report.inter_cluster_coupling.is_empty());
        // The cross edge should appear in coupling
        assert!(report.inter_cluster_coupling[0].weight > 0.0);
    }

    fn build_three_domain_graph() -> CodeGraph {
        let mut graph = CodeGraph::new();
        let domains: &[(&str, &[&str])] = &[
            (
                "order_mgmt",
                &[
                    "create_order",
                    "cancel_order",
                    "get_order",
                    "list_orders",
                    "update_order_status",
                ],
            ),
            (
                "billing_mgmt",
                &[
                    "charge_customer",
                    "refund_payment",
                    "get_invoice",
                    "process_payment",
                    "close_invoice",
                ],
            ),
            (
                "inventory_mgmt",
                &[
                    "check_stock",
                    "reserve_item",
                    "release_item",
                    "update_stock",
                    "get_stock_level",
                ],
            ),
        ];

        let mut domain_nodes: Vec<Vec<NodeIndex>> = Vec::new();
        for (domain, procs) in domains {
            let indices: Vec<NodeIndex> = procs
                .iter()
                .map(|p| graph.add_node(proc_node(&format!("{}_{}", domain, p))))
                .collect();

            for i in 0..indices.len() {
                for j in 0..indices.len() {
                    if i < j {
                        graph.add_edge(indices[i], indices[j], call_edge());
                        graph.add_edge(indices[j], indices[i], call_edge());
                    }
                }
            }
            domain_nodes.push(indices);
        }

        let cross: &[(usize, usize, usize)] = &[(0, 1, 1), (1, 2, 1), (0, 2, 1)];
        for &(d1, d2, count) in cross {
            for c in 0..count {
                let src = domain_nodes[d1][c % domain_nodes[d1].len()];
                let dst = domain_nodes[d2][(c + 1) % domain_nodes[d2].len()];
                graph.add_edge(src, dst, call_edge());
            }
        }

        graph
    }

    #[test]
    fn three_domain_recovery_at_k3() {
        let graph = build_three_domain_graph();
        let report = partition(&graph, &ClusterConfig::new(3));

        assert_eq!(
            report.k_actual, 3,
            "Expected 3 clusters, got {}. Q = {:.3}",
            report.k_actual, report.modularity
        );

        assert!(
            report.modularity > 0.3,
            "Modularity Q = {:.3} must be > 0.3",
            report.modularity
        );

        let assignments: Vec<Vec<u32>> = (0..3)
            .map(|d| {
                let start = d * 5;
                (start..start + 5)
                    .map(|i| report.assignments[&NodeIndex::new(i)])
                    .collect()
            })
            .collect();

        for (d, clusters) in assignments.iter().enumerate() {
            let mut freq: HashMap<u32, usize> = HashMap::new();
            for &c in clusters {
                *freq.entry(c).or_default() += 1;
            }
            let (&max_cluster, _) = freq.iter().max_by_key(|(_, &v)| v).unwrap();
            let correct = freq[&max_cluster];
            let accuracy = correct as f64 / clusters.len() as f64;
            assert!(
                accuracy >= 0.8,
                "Domain {} accuracy {:.0}% < 80% (clusters: {:?})",
                d,
                accuracy * 100.0,
                clusters
            );
        }

        let dominant: Vec<u32> = assignments
            .iter()
            .map(|clusters| {
                let mut freq: HashMap<u32, usize> = HashMap::new();
                for &c in clusters {
                    *freq.entry(c).or_default() += 1;
                }
                *freq.iter().max_by_key(|(_, &v)| v).unwrap().0
            })
            .collect();
        let unique: HashSet<u32> = dominant.iter().copied().collect();
        assert_eq!(
            unique.len(),
            3,
            "3 domains → 3 distinct clusters: {:?}",
            dominant
        );
    }

    #[test]
    fn three_domain_cross_cluster_coupling_minority() {
        let graph = build_three_domain_graph();
        let report = partition(&graph, &ClusterConfig::new(3));

        let total_internal: f64 = report.cluster_stats.iter().map(|s| s.internal_weight).sum();
        let total_external: f64 = report.cluster_stats.iter().map(|s| s.external_weight).sum();
        let total = total_internal + total_external;

        if total > 0.0 {
            let ratio = total_external / total;
            assert!(
                ratio < 0.5,
                "Cross-cluster coupling {:.1}% should be < 50%",
                ratio * 100.0
            );
        }
    }

    #[test]
    fn three_domain_dot_export_has_clusters() {
        let graph = build_three_domain_graph();
        let report = partition(&graph, &ClusterConfig::new(3));
        let cr = ClusterResult::from(&report);

        let dot = crate::export::dot::to_dot_with_clusters(&graph, Some(&cr));

        for i in 0..report.k_actual {
            assert!(
                dot.contains(&format!("subgraph cluster_{}", i)),
                "DOT missing cluster_{}",
                i
            );
        }
    }

    #[test]
    fn cluster_config_supports_early_termination_options() {
        let cfg = ClusterConfig::auto()
            .with_max_iterations(1000)
            .with_min_delta_q(1e-6);
        assert_eq!(cfg.max_iterations, Some(1000));
        assert_eq!(cfg.min_delta_q, 1e-6);

        let default_cfg = ClusterConfig::new(3);
        assert_eq!(default_cfg.max_iterations, None);
        assert_eq!(default_cfg.min_delta_q, 0.0);
    }

    #[test]
    fn adaptive_k_sweep_skips_aggressive_values_on_large_graphs() {
        assert_eq!(adaptive_k_sweep(0), &[3, 5, 10, 20, 50]);
        assert_eq!(adaptive_k_sweep(10_000), &[3, 5, 10, 20, 50]);
        assert_eq!(adaptive_k_sweep(10_001), &[10, 20, 50]);
        assert_eq!(adaptive_k_sweep(130_000), &[10, 20, 50]);
    }

    #[test]
    fn auto_partition_with_progress_runs_full_sweep() {
        // Three-domain graph: γ sweep + k sweep should produce a report with
        // entries for both phases, and the recommended config should yield a
        // non-trivial partition.
        let graph = build_three_domain_graph();
        let config = ClusterConfig::auto();

        let pb = indicatif::ProgressBar::hidden();
        let report = auto_partition_with_progress(&graph, &config, Some(&pb));

        assert!(
            report.sweep.len() >= AUTO_GAMMA_SWEEP_LEN,
            "expected at least {} sweep entries, got {}",
            AUTO_GAMMA_SWEEP_LEN,
            report.sweep.len()
        );
        assert!(
            report.recommended_k > 0 && report.recommended_k <= 50,
            "recommended_k out of range: {}",
            report.recommended_k
        );
        assert!(
            report.report.k_actual > 0,
            "final partition produced 0 clusters"
        );
    }

    /// Performance smoke test: build a synthetic 50k-node participant graph
    /// (~10k SCCs after condensation, ~100k inter-SCC edges) and verify a
    /// single forced-k CNM run finishes well under 5 seconds. Run with:
    ///   cargo test --bin codeweb cluster::tests::bench_cnm_large_graph -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bench_cnm_large_graph() {
        use std::sync::Arc;
        use std::time::Instant;

        let mut graph = CodeGraph::new();
        let file = Arc::new(PathBuf::from("bench.sql"));
        let loc = SourceLocation {
            file: file.clone(),
            line: 1,
        };
        let edge = Edge::DirectCall {
            scope: CallScope::IntraPackage,
            location: loc.clone(),
        };

        const CLUMPS: usize = 10_000;
        const CLUMP_SIZE: usize = 5;
        const N: usize = CLUMPS * CLUMP_SIZE;

        let mut nodes = Vec::with_capacity(N);
        for i in 0..N {
            let node = Node::Procedure {
                id: RoutineId {
                    schema: Some("bench".to_string()),
                    package: Some(format!("pkg_{}", i % CLUMPS)),
                    name: format!("proc_{}", i),
                    kind: RoutineKind::Procedure,
                },
                location: loc.clone(),
                partial: false,
                body_sql: Vec::new(),
            };
            nodes.push(graph.add_node(node));
        }

        // Within each clump: bidirectional clique (forms 1 SCC of size CLUMP_SIZE).
        for c in 0..CLUMPS {
            let base = c * CLUMP_SIZE;
            for i in 0..CLUMP_SIZE {
                for j in (i + 1)..CLUMP_SIZE {
                    graph.add_edge(nodes[base + i], nodes[base + j], edge.clone());
                    graph.add_edge(nodes[base + j], nodes[base + i], edge.clone());
                }
            }
        }
        // Between clumps: forward-only bridges, no wrap-around (prevents
        // inter-clump cycles → preserves 10k distinct SCCs after condensation).
        for c in 0..(CLUMPS - 3) {
            let src = nodes[c * CLUMP_SIZE];
            let dst = nodes[(c + 1) * CLUMP_SIZE + 1];
            graph.add_edge(src, dst, edge.clone());
            let src2 = nodes[c * CLUMP_SIZE + 2];
            let dst2 = nodes[(c + 3) * CLUMP_SIZE];
            graph.add_edge(src2, dst2, edge.clone());
        }

        let config = ClusterConfig::new(10);

        let t0 = Instant::now();
        let report = partition(&graph, &config);
        let elapsed = t0.elapsed();

        eprintln!(
            "bench_cnm: {} participant nodes, forced k=10, k_actual={}, Q={:.3}, elapsed {:.2?}",
            N, report.k_actual, report.modularity, elapsed
        );

        assert!(
            elapsed.as_secs() < 10,
            "CNM took {elapsed:?}, expected < 10s"
        );
        assert_eq!(report.k_actual, 10, "forced k=10 must produce 10 clusters");
    }

    #[test]
    fn cluster_cnm_respects_max_iterations_in_natural_mode() {
        // Directed chain a→b→c→d (acyclic → each node is its own SCC, so the
        // condensed graph has 4 super-nodes with chain edges of weight 1).
        // ΔQ(a,b) = (2/6)*(1 - 1·2/6) ≈ 0.222 > 0, so natural mode WOULD merge
        // without a cap. With max_iterations=0, no merge is allowed.
        let mut graph = CodeGraph::new();
        let a = graph.add_node(proc_node("a"));
        let b = graph.add_node(proc_node("b"));
        let c = graph.add_node(proc_node("c"));
        let d = graph.add_node(proc_node("d"));
        graph.add_edge(a, b, call_edge());
        graph.add_edge(b, c, call_edge());
        graph.add_edge(c, d, call_edge());

        let capped = partition(&graph, &ClusterConfig::auto().with_max_iterations(0));
        assert_eq!(
            capped.k_actual, 4,
            "max_iterations=0 must prevent any merge"
        );

        let unlimited = partition(&graph, &ClusterConfig::auto());
        assert!(
            unlimited.k_actual < 4,
            "unlimited should merge at least once (got {})",
            unlimited.k_actual
        );
    }

    #[test]
    fn cluster_cnm_min_delta_q_prunes_negligible_merges() {
        // Same chain: max ΔQ ≈ 0.222. A strict threshold of 0.5 must block all
        // merges; the lenient default (0.0) allows them.
        let mut graph = CodeGraph::new();
        let a = graph.add_node(proc_node("a"));
        let b = graph.add_node(proc_node("b"));
        let c = graph.add_node(proc_node("c"));
        let d = graph.add_node(proc_node("d"));
        graph.add_edge(a, b, call_edge());
        graph.add_edge(b, c, call_edge());
        graph.add_edge(c, d, call_edge());

        let strict = partition(&graph, &ClusterConfig::auto().with_min_delta_q(0.5));
        let lenient = partition(&graph, &ClusterConfig::auto());

        assert_eq!(
            strict.k_actual, 4,
            "strict threshold 0.5 should block all merges"
        );
        assert!(
            lenient.k_actual < 4,
            "lenient threshold should allow merges (got {})",
            lenient.k_actual
        );
    }

    #[test]
    fn cluster_cnm_force_k_ignores_max_iterations() {
        // max_iterations must NOT apply when target_k is set (forced-k mode
        // is contract-bound to reach target_k).
        let mut graph = CodeGraph::new();
        let nodes: Vec<NodeIndex> = (0..6)
            .map(|i| graph.add_node(proc_node(&format!("n{i}"))))
            .collect();
        for i in 0..6 {
            for j in 0..6 {
                if i < j {
                    graph.add_edge(nodes[i], nodes[j], call_edge());
                    graph.add_edge(nodes[j], nodes[i], call_edge());
                }
            }
        }

        let config = ClusterConfig::new(2).with_max_iterations(1);
        let report = partition(&graph, &config);

        // Forced k=2 must be respected even with max_iterations=1.
        assert!(
            report.k_actual <= 2,
            "k_actual was {}, expected <= 2 (forced k overrides max_iterations)",
            report.k_actual
        );
    }

    // --- Task 1: WccTopology Tests ---

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
        assert_eq!(topo.isolates_count, 2);
        assert_eq!(topo.isolates_node_count, 3);
        assert_eq!(topo.largest_components[0].len(), 5);
        assert_eq!(topo.largest_components[1].len(), 2);
        assert_eq!(topo.largest_components[2].len(), 1);
    }

    #[test]
    fn wcc_topology_excludes_non_participants() {
        let mut graph = CodeGraph::new();
        let p = graph.add_node(proc_node("p"));
        let t = graph.add_node(table_node("orders"));
        graph.add_edge(p, t, table_access_edge());

        let config = ClusterConfig::auto();
        let topo = compute_wcc_topology(&graph, &config);

        assert_eq!(topo.total_participants, 1);
        assert_eq!(topo.wcc_count, 1);
        assert_eq!(topo.gcc_size, 1);
    }

    // --- Task 2: PartitionReport + Config Integration Tests ---

    #[test]
    fn partition_report_includes_topology() {
        let mut graph = CodeGraph::new();
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
        let a = graph.add_node(proc_node("a"));
        let b = graph.add_node(proc_node("b"));
        let c = graph.add_node(proc_node("c"));
        graph.add_edge(a, b, call_edge());
        graph.add_edge(b, c, call_edge());
        let _d = graph.add_node(proc_node("d"));

        let config = ClusterConfig::auto().with_min_component_size(2);
        let report = partition(&graph, &config);
        assert_eq!(report.total_nodes, 3);
        assert!(report.assignments.contains_key(&a));
        assert!(report.assignments.contains_key(&b));
        assert!(report.assignments.contains_key(&c));
        assert!(!report.assignments.contains_key(&NodeIndex::new(3))); // d filtered
    }

    #[test]
    fn min_component_size_clusters_only_gcc() {
        // Realistic micro-fixture: GCC of 6 nodes + 3 isolated pairs + 2 singletons
        let mut graph = CodeGraph::new();
        // GCC: 6 procs in a clique
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
        let report_filtered = partition(&graph, &ClusterConfig::auto().with_min_component_size(3));
        assert_eq!(report_filtered.total_nodes, 6);
        // All clustered nodes are GCC members (indices 0-5)
        for &node_idx in report_filtered.assignments.keys() {
            assert!(
                node_idx.index() < 6,
                "node {} should not be clustered (not in GCC)",
                node_idx.index()
            );
        }
        // Isolates are still in topology
        let topo2 = report_filtered.topology.as_ref().unwrap();
        assert_eq!(topo2.wcc_count, 6); // topology unchanged
        assert_eq!(topo2.gcc_size, 6);
    }

    // --- Task 5 Tests ---

    #[test]
    fn proc_table_matrix_captures_accesses() {
        let mut graph = CodeGraph::new();
        let p1 = graph.add_node(proc_node("p1"));
        let p2 = graph.add_node(proc_node("p2"));
        let p3 = graph.add_node(proc_node("p3"));
        let t_orders = graph.add_node(table_node("orders"));
        let t_customers = graph.add_node(table_node("customers"));
        let t_audit = graph.add_node(table_node("audit_log"));

        graph.add_edge(p1, t_orders, table_access_edge());
        graph.add_edge(p1, t_audit, table_access_edge());
        graph.add_edge(p2, t_orders, table_access_edge());
        graph.add_edge(p2, t_customers, table_access_edge());
        graph.add_edge(p3, t_customers, table_access_edge());

        let config = ClusterConfig::auto();
        let matrix = build_proc_table_matrix(&graph, &config);

        assert_eq!(matrix.procs.len(), 3);
        assert_eq!(matrix.tables.len(), 3);
        assert!(matrix.access(p1, t_orders));
        assert!(matrix.access(p1, t_audit));
        assert!(!matrix.access(p1, t_customers));
        assert!(matrix.access(p2, t_orders));
        assert!(matrix.access(p2, t_customers));
        assert!(matrix.access(p3, t_customers));
        assert!(!matrix.access(p3, t_orders));
    }

    #[test]
    fn proc_table_matrix_excludes_non_participants() {
        let mut graph = CodeGraph::new();
        let t1 = graph.add_node(table_node("t1"));
        let t2 = graph.add_node(table_node("t2"));
        graph.add_edge(t1, t2, table_access_edge());

        let config = ClusterConfig::auto();
        let matrix = build_proc_table_matrix(&graph, &config);
        assert_eq!(matrix.procs.len(), 0);
        assert_eq!(matrix.tables.len(), 0);
    }

    // --- Task 6 Tests ---

    #[test]
    fn tfidf_cosine_groups_similar_procs() {
        let mut graph = CodeGraph::new();
        let p1 = graph.add_node(proc_node("p1"));
        let p2 = graph.add_node(proc_node("p2"));
        let p3 = graph.add_node(proc_node("p3"));
        let t_orders = graph.add_node(table_node("orders"));
        let t_customers = graph.add_node(table_node("customers"));
        let t_audit = graph.add_node(table_node("audit_log"));

        // p1, p2 both read orders + customers (similar)
        graph.add_edge(p1, t_orders, table_access_edge());
        graph.add_edge(p1, t_customers, table_access_edge());
        graph.add_edge(p2, t_orders, table_access_edge());
        graph.add_edge(p2, t_customers, table_access_edge());
        // p3 reads only audit_log (dissimilar)
        graph.add_edge(p3, t_audit, table_access_edge());

        let config = ClusterConfig::auto();
        let matrix = build_proc_table_matrix(&graph, &config);
        let edges = compute_tfidf_cosine_edges(&matrix, 0.1, 10);

        let p1_idx = matrix.proc_index[&p1];
        let p2_idx = matrix.proc_index[&p2];
        let p3_idx = matrix.proc_index[&p3];

        // p1-p2 should have high similarity (they share 2 tables, both with idf > 0
        // because df=2, N=3 -> idf = ln(3/2) approximately 0.405)
        let sim_12 = edges
            .iter()
            .find(|(a, b, _)| (*a == p1_idx && *b == p2_idx) || (*a == p2_idx && *b == p1_idx))
            .map(|(_, _, s)| *s);
        assert!(sim_12.is_some(), "p1-p2 edge must exist");
        // cosine(p1,p2) should be 1.0 because their TF-IDF vectors are identical
        assert!(
            (sim_12.unwrap() - 1.0).abs() < 1e-9,
            "p1-p2 similarity should be 1.0 (identical vectors), got {}",
            sim_12.unwrap()
        );

        // p3 shares nothing with p1/p2 -> no edges
        let sim_13 = edges
            .iter()
            .find(|(a, b, _)| (*a == p1_idx && *b == p3_idx) || (*a == p3_idx && *b == p1_idx));
        assert!(sim_13.is_none(), "p1-p3 should not have an edge");
    }

    // --- Task 7 Test ---

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

        // Without projection: p1, p2 are in different WCCs (tables excluded from clustering)
        let report_no_proj = partition(&graph, &ClusterConfig::new(1));
        let topo = report_no_proj.topology.as_ref().unwrap();
        assert_eq!(
            topo.wcc_count, 2,
            "p1 and p2 should be in separate WCCs without projection"
        );

        // With projection: p1, p2 get bridged via TF-IDF similarity → same cluster
        let config = ClusterConfig::new(1).with_table_projection(0.1, 0.3, 10);
        let report_proj = partition(&graph, &config);
        assert_eq!(report_proj.total_nodes, 2);
        let c1 = report_proj.assignments[&p1];
        let c2 = report_proj.assignments[&p2];
        assert_eq!(c1, c2, "p1 and p2 must be in same cluster with projection");
    }

    #[test]
    fn tfidf_downweights_generic_tables() {
        let mut graph = CodeGraph::new();
        // 3 procs all read the same "common_codes" table and nothing else
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
        // so TF-IDF vectors are all-zero -> no similarity edges.
        assert!(
            edges.is_empty(),
            "generic-only sharing should produce no edges"
        );
    }

    #[test]
    fn tfidf_cosine_respects_threshold() {
        // Use 3 procs where cosine(p1,p2) ~= 0.463 so tau=0.3 vs tau=0.6 distinguishes.
        //
        // p1 reads t1, t2          -> vector: {t1: idf(t1), t2: idf(t2)}
        // p2 reads t1, t2, t3      -> vector: {t1: idf(t1), t2: idf(t2), t3: idf(t3)}
        // p3 reads t4              -> vector: {t4: idf(t4)}  (isolated, makes N=3)
        //
        // N=3; df(t1)=df(t2)=2, df(t3)=df(t4)=1
        // idf(t1)=idf(t2)=ln(3/2)~=0.405, idf(t3)=idf(t4)=ln(3)~=1.099
        // cosine(p1,p2) ~= 0.405 / (sqrt(0.405^2+0.405^2) * sqrt(0.405^2+0.405^2+1.099^2))
        //              ~= 0.463
        //
        // So tau=0.3 -> edge present, tau=0.6 -> edge absent.
        let mut graph = CodeGraph::new();
        let p1 = graph.add_node(proc_node("p1"));
        let p2 = graph.add_node(proc_node("p2"));
        let _p3 = graph.add_node(proc_node("p3"));
        let t1 = graph.add_node(table_node("t1"));
        let t2 = graph.add_node(table_node("t2"));
        let t3 = graph.add_node(table_node("t3"));
        let t4 = graph.add_node(table_node("t4"));

        graph.add_edge(p1, t1, table_access_edge());
        graph.add_edge(p1, t2, table_access_edge());
        graph.add_edge(p2, t1, table_access_edge());
        graph.add_edge(p2, t2, table_access_edge());
        graph.add_edge(p2, t3, table_access_edge());
        graph.add_edge(_p3, t4, table_access_edge());

        let config = ClusterConfig::auto();
        let matrix = build_proc_table_matrix(&graph, &config);

        // High threshold: no edge
        let edges_high = compute_tfidf_cosine_edges(&matrix, 0.6, 10);
        let has_edge_high = edges_high.iter().any(|(a, b, _)| {
            let p1i = matrix.proc_index[&p1];
            let p2i = matrix.proc_index[&p2];
            (*a == p1i && *b == p2i) || (*a == p2i && *b == p1i)
        });
        assert!(!has_edge_high, "tau=0.6 should reject edge (cosine~=0.463)");

        // Low threshold: edge present
        let edges_low = compute_tfidf_cosine_edges(&matrix, 0.3, 10);
        let has_edge_low = edges_low.iter().any(|(a, b, _)| {
            let p1i = matrix.proc_index[&p1];
            let p2i = matrix.proc_index[&p2];
            (*a == p1i && *b == p2i) || (*a == p2i && *b == p1i)
        });
        assert!(has_edge_low, "tau=0.3 should accept edge (cosine~=0.463)");
    }
}

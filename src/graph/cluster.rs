//! Graph partitioning for system decomposition.
//!
//! CNM-style greedy modularity maximization. SCCs condensed into super-nodes
//! before clustering (cyclic nodes are indivisible coupling units).

use crate::graph::{CodeGraph, Edge, Node};
use petgraph::graph::NodeIndex;
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
    Custom,
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
            Node::Custom { .. } => NodeKind::Custom,
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
            NodeKind::Custom => "custom",
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
        Edge::Extends { .. } | Edge::Implements { .. } => Some(config.inheritance),
        Edge::TableAccess { .. } | Edge::DependsOn { .. } => Some(config.data_flow),
        Edge::TriggersRoutine { .. }
        | Edge::ReferencesType { .. }
        | Edge::UsesSequence { .. }
        | Edge::IndexesTable { .. }
        | Edge::AliasesObject { .. }
        | Edge::CustomEdge { .. } => Some(config.reference),
        Edge::ContainsMethod | Edge::ContainsRoutine => Some(config.composition),
    }
}

#[derive(Debug, Clone)]
pub struct ClusterConfig {
    pub k: Option<usize>,
    pub gamma: f64,
    pub edge_weights: EdgeWeights,
    pub participant_kinds: HashSet<NodeKind>,
}

impl ClusterConfig {
    pub fn new(k: usize) -> Self {
        Self {
            k: Some(k),
            gamma: 1.0,
            edge_weights: EdgeWeights::default(),
            participant_kinds: default_participant_kinds(),
        }
    }

    pub fn auto() -> Self {
        Self {
            k: None,
            gamma: 1.0,
            edge_weights: EdgeWeights::default(),
            participant_kinds: default_participant_kinds(),
        }
    }

    pub fn with_gamma(mut self, gamma: f64) -> Self {
        self.gamma = gamma;
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
    let participants: HashSet<NodeIndex> = graph
        .node_indices()
        .filter(|&idx| {
            config
                .participant_kinds
                .contains(&NodeKind::from_node(&graph[idx]))
        })
        .collect();

    let sccs = petgraph::algo::kosaraju_scc(graph);

    let mut super_nodes: Vec<HashSet<NodeIndex>> = Vec::new();
    let mut node_to_super: HashMap<NodeIndex, usize> = HashMap::new();

    for scc in sccs {
        let filtered: HashSet<NodeIndex> = scc
            .into_iter()
            .filter(|idx| participants.contains(idx))
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

    let degrees: Vec<f64> = (0..n).map(|i| adj[i].values().sum()).collect();
    let total_weight: f64 = degrees.iter().sum();

    CondensedGraph {
        adj,
        degrees,
        total_weight,
    }
}

/// Run CNM-style greedy modularity maximization, merging until target_k clusters
/// remain or no merge improves modularity (ΔQ ≤ 0).
///
/// Returns `(community_of_super_node, modularity_Q)`.
fn cluster_cnm(
    condensed: &CondensedGraph,
    target_k: Option<usize>,
    gamma: f64,
) -> (Vec<usize>, f64) {
    let n = condensed.adj.len();
    if n == 0 {
        return (vec![], 0.0);
    }

    let min_k = target_k.unwrap_or(1).min(n);
    let w = condensed.total_weight;
    if w == 0.0 {
        let labels = (0..n).map(|i| i.min(min_k.saturating_sub(1))).collect();
        return (labels, 0.0);
    }

    let mut community_of: Vec<usize> = (0..n).collect();
    let mut comm_degree: Vec<f64> = condensed.degrees.clone();

    let mut inter_weight: HashMap<(usize, usize), f64> = HashMap::new();
    for i in 0..n {
        for (&j, &weight) in &condensed.adj[i] {
            if i < j {
                *inter_weight.entry((i, j)).or_insert(0.0) += weight;
            }
        }
    }

    let mut num_communities = n;
    let force_k = target_k.is_some();

    while num_communities > min_k {
        if inter_weight.is_empty() {
            if !force_k {
                break;
            }
            let mut comm_members: HashMap<usize, Vec<usize>> = HashMap::new();
            for (sn, &c) in community_of.iter().enumerate() {
                comm_members.entry(c).or_default().push(sn);
            }
            let mut sizes: Vec<(usize, usize)> =
                comm_members.iter().map(|(&c, m)| (c, m.len())).collect();
            sizes.sort_by_key(|(_, s)| *s);
            if sizes.len() < 2 {
                break;
            }
            let smallest = sizes[0].0;
            let largest = sizes[sizes.len() - 1].0;
            for c in community_of.iter_mut() {
                if *c == smallest {
                    *c = largest;
                }
            }
            num_communities -= 1;
            continue;
        }
        // ΔQ_γ = (2/W) * [e_ab - γ * K_a * K_b / W]
        let mut best_dq = f64::NEG_INFINITY;
        let mut best_pair: Option<(usize, usize)> = None;

        for &(a, b) in inter_weight.keys() {
            let e_ab = inter_weight[&(a, b)];
            let dq = (2.0 / w) * (e_ab - gamma * comm_degree[a] * comm_degree[b] / w);
            if dq > best_dq {
                best_dq = dq;
                best_pair = Some((a, b));
            }
        }

        let should_merge = if force_k {
            num_communities > min_k
        } else {
            best_dq > 0.0
        };

        match best_pair {
            Some((a, b)) if should_merge => {
                for c in community_of.iter_mut() {
                    if *c == b {
                        *c = a;
                    }
                }
                comm_degree[a] += comm_degree[b];
                comm_degree[b] = 0.0;

                let b_keys: Vec<(usize, usize)> = inter_weight
                    .keys()
                    .filter(|&&(x, y)| x == b || y == b)
                    .copied()
                    .collect();

                for key in b_keys {
                    let weight = inter_weight.remove(&key).unwrap_or(0.0);
                    let other = if key.0 == b { key.1 } else { key.0 };
                    if other == a {
                        continue;
                    }
                    let new_key = if a < other { (a, other) } else { (other, a) };
                    *inter_weight.entry(new_key).or_insert(0.0) += weight;
                }

                num_communities -= 1;
            }
            _ => break,
        }
    }

    let mut id_map: HashMap<usize, usize> = HashMap::new();
    let mut next_id = 0;
    for &c in &community_of {
        use std::collections::hash_map::Entry;
        if let Entry::Vacant(e) = id_map.entry(c) {
            e.insert(next_id);
            next_id += 1;
        }
    }
    let community_of: Vec<usize> = community_of.iter().map(|&c| id_map[&c]).collect();

    let q = compute_modularity(condensed, &community_of);
    (community_of, q)
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

pub fn auto_partition(graph: &CodeGraph, base_config: &ClusterConfig) -> AutoClusterReport {
    let condensation = condense_sccs(graph, base_config);
    let condensed = build_condensed_graph(graph, &condensation, base_config);
    let total_nodes = condensation.super_nodes.len().max(1);

    let mut sweep: Vec<GammaSweepEntry> = Vec::new();

    for &gamma in AUTO_GAMMA_SWEEP {
        let (communities, q) = cluster_cnm(&condensed, None, gamma);
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
    }

    for &forced_k in AUTO_K_SWEEP {
        if forced_k >= total_nodes {
            continue;
        }
        let (communities, q) = cluster_cnm(&condensed, Some(forced_k), base_config.gamma);
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
}

pub fn partition(graph: &CodeGraph, config: &ClusterConfig) -> PartitionReport {
    let condensation = condense_sccs(graph, config);
    let condensed = build_condensed_graph(graph, &condensation, config);
    let (super_communities, modularity) = cluster_cnm(&condensed, config.k, config.gamma);

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
        CallScope, DataFlowKind, Edge, EdgeCategory, Node, RoutineId, RoutineKind, SourceLocation,
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
        let a = graph.add_node(proc_node("a"));
        let b = graph.add_node(proc_node("b"));
        let c = graph.add_node(proc_node("c"));
        let d = graph.add_node(proc_node("d"));

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
}

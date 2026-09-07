use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use petgraph::graph::NodeIndex;
use petgraph::Direction;

use crate::graph::key::NodeKey;

/// Display style for call chain output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChainStyle {
    /// Traditional tree view with box-drawing characters.
    #[default]
    Tree,
    /// Path-based view: each root-to-leaf path shown as a separate trace line.
    Path,
}

impl std::str::FromStr for ChainStyle {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "tree" => Ok(ChainStyle::Tree),
            "path" => Ok(ChainStyle::Path),
            other => Err(format!(
                "unknown chain style '{}', expected 'tree' or 'path'",
                other
            )),
        }
    }
}

pub struct TreeNode {
    pub idx: NodeIndex,
    pub edge_label: Option<String>,
    pub children: Vec<TreeNode>,
    /// True when direct children of this node could not all be added to the
    /// tree (node budget / depth clamp hit). A node with `children: []` and
    /// `has_more: false` is a genuine leaf; with `has_more: true` it is a
    /// truncated "fake leaf".
    pub has_more: bool,
    /// Number of direct children that exist in the graph but are missing
    /// from the tree (always 0 when `has_more` is false).
    pub more_count: usize,
}

pub struct CallChain {
    pub target: NodeIndex,
    pub callers: Vec<TreeNode>,
    pub callees: Vec<TreeNode>,
}

pub struct DegreeInfo {
    pub idx: NodeIndex,
    pub in_degree: usize,
    pub out_degree: usize,
    pub total_degree: usize,
}

pub(crate) fn edge_label_for(
    graph: &crate::graph::CodeGraph,
    from: NodeIndex,
    to: NodeIndex,
) -> Option<String> {
    use crate::graph::{CallScope, DataFlowKind, Edge};
    let edge = graph.edges_connecting(from, to).next()?;
    match edge.weight() {
        Edge::DirectCall { scope, .. } => Some(match scope {
            CallScope::IntraPackage => "[intra]".into(),
            CallScope::CrossPackage => "[cross]".into(),
            CallScope::External => "[external]".into(),
        }),
        Edge::TableAccess {
            flow_kind,
            modes,
            write_kinds,
            ..
        } => {
            let mut parts = Vec::new();
            if matches!(flow_kind, DataFlowKind::DefinitionDependency) {
                parts.push("dep".to_string());
            }
            if let Some(mode_label) = crate::graph::access_mode_label(*modes, write_kinds) {
                parts.push(mode_label);
            }
            if parts.is_empty() {
                None
            } else {
                Some(format!("[{}]", parts.join(",")))
            }
        }
        Edge::DependsOn { .. } => Some("[depends_on]".into()),
        Edge::DynamicCall { .. } => Some("[dynamic]".into()),
        Edge::UsesBuiltinFunction { .. } => Some("[builtin]".into()),
        Edge::CallsProcedure { .. } => Some("[calls]".into()),
        Edge::InvokesMapper { .. } => Some("[invokes]".into()),
        Edge::CallsJava { .. } => Some("[calls_java]".into()),
        Edge::Extends { .. } => Some("[extends]".into()),
        Edge::Implements { .. } => Some("[implements]".into()),
        Edge::TriggersRoutine { .. } => Some("[triggers]".into()),
        Edge::ReferencesType { .. } => Some("[ref_type]".into()),
        Edge::UsesSequence { .. } => Some("[uses_seq]".into()),
        Edge::IndexesTable { .. } => Some("[indexes]".into()),
        Edge::AliasesObject { .. } => Some("[aliases]".into()),
        Edge::ContainsRoutine | Edge::ContainsMethod => None,
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn build_tree_dfs(
    graph: &crate::graph::CodeGraph,
    start: NodeIndex,
    direction: Direction,
    ancestors: &mut HashSet<NodeIndex>,
    depth: usize,
    max_depth: usize,
    max_nodes: usize,
    visited: &mut usize,
    skip_builtins: bool,
) -> (Vec<TreeNode>, usize) {
    let neighbors: Vec<NodeIndex> = graph
        .neighbors_directed(start, direction)
        .filter(|n| !ancestors.contains(n))
        .filter(|n| {
            !skip_builtins || !matches!(graph[*n], crate::graph::Node::BuiltinFunction { .. })
        })
        .collect();
    let effective = neighbors.len();

    if *visited >= max_nodes {
        return (Vec::new(), effective);
    }
    if depth > max_depth {
        if max_depth > 20 {
            // Warn only at high limits — small limits are intentional, not runaway chains.
            let key = crate::graph::key::NodeKey::from_node(&graph[start]);
            eprintln!(
                "  ⚠ trace depth exceeded {} at '{}' — possible runaway chain, stopping",
                max_depth, key
            );
        }
        return (Vec::new(), effective);
    }

    let mut roots = Vec::new();
    for neighbor in neighbors {
        if *visited >= max_nodes {
            break;
        }
        let (from, to) = match direction {
            Direction::Outgoing => (start, neighbor),
            Direction::Incoming => (neighbor, start),
        };
        let edge_label = edge_label_for(graph, from, to);
        ancestors.insert(neighbor);
        *visited += 1;
        let (children, unexpanded) = build_tree_dfs(
            graph,
            neighbor,
            direction,
            ancestors,
            depth + 1,
            max_depth,
            max_nodes,
            visited,
            skip_builtins,
        );
        ancestors.remove(&neighbor);
        roots.push(TreeNode {
            idx: neighbor,
            edge_label,
            children,
            has_more: unexpanded > 0,
            more_count: unexpanded,
        });
    }
    let unexpanded_here = effective - roots.len();
    (roots, unexpanded_here)
}

pub fn trace_chain(
    graph: &crate::graph::CodeGraph,
    start: NodeIndex,
    max_depth: usize,
    max_nodes: usize,
    skip_builtins: bool,
) -> (CallChain, usize) {
    let mut visited = 0usize;

    let callers = if max_depth == 0 {
        Vec::new()
    } else {
        let mut caller_ancestors = HashSet::new();
        caller_ancestors.insert(start);
        let (nodes, _) = build_tree_dfs(
            graph,
            start,
            Direction::Incoming,
            &mut caller_ancestors,
            1,
            max_depth,
            max_nodes,
            &mut visited,
            skip_builtins,
        );
        nodes
    };

    let callees = if max_depth == 0 {
        Vec::new()
    } else {
        let mut callee_ancestors = HashSet::new();
        callee_ancestors.insert(start);
        let (nodes, _) = build_tree_dfs(
            graph,
            start,
            Direction::Outgoing,
            &mut callee_ancestors,
            1,
            max_depth,
            max_nodes,
            &mut visited,
            skip_builtins,
        );
        nodes
    };

    (
        CallChain {
            target: start,
            callers,
            callees,
        },
        visited,
    )
}

/// Collect all unique nodes within `depth` hops of `start` in the given direction.
///
/// Returns a flat list excluding `start` itself. Each node appears at most once
/// (first discovery wins). `depth=1` returns direct neighbors only;
/// `depth=0` means unlimited (expands until the connected component is exhausted).
#[cfg(any(feature = "serve", feature = "mcp"))]
pub fn neighbors_at_depth(
    graph: &crate::graph::CodeGraph,
    start: NodeIndex,
    direction: Direction,
    depth: usize,
) -> Vec<NodeIndex> {
    let depth = if depth == 0 { usize::MAX } else { depth };
    let mut visited = HashSet::new();
    visited.insert(start);
    let mut frontier = vec![start];
    let mut result = Vec::new();
    for _ in 0..depth {
        let mut next = Vec::new();
        for &node in &frontier {
            for neighbor in graph.neighbors_directed(node, direction) {
                if visited.insert(neighbor) {
                    result.push(neighbor);
                    next.push(neighbor);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    result
}

/// Collect all unique source files involved in a call chain.
///
/// Returns a sorted list of `(file_path, node_labels)` tuples, ordered by
/// the number of nodes in descending order (most-referenced files first).
pub fn collect_chain_files(
    chain: &CallChain,
    graph: &crate::graph::CodeGraph,
) -> Vec<(PathBuf, Vec<String>)> {
    let mut file_nodes: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();

    fn insert_node(
        graph: &crate::graph::CodeGraph,
        idx: NodeIndex,
        file_nodes: &mut BTreeMap<PathBuf, Vec<String>>,
    ) {
        let file = graph[idx].file();
        if !file.as_os_str().is_empty() {
            let key = crate::graph::node_display_name(&graph[idx]);
            let entry = file_nodes.entry(file.to_path_buf()).or_default();
            if !entry.contains(&key) {
                entry.push(key);
            }
        }
    }

    fn collect_from_tree(
        nodes: &[TreeNode],
        graph: &crate::graph::CodeGraph,
        file_nodes: &mut BTreeMap<PathBuf, Vec<String>>,
    ) {
        for node in nodes {
            insert_node(graph, node.idx, file_nodes);
            collect_from_tree(&node.children, graph, file_nodes);
        }
    }

    insert_node(graph, chain.target, &mut file_nodes);

    collect_from_tree(&chain.callers, graph, &mut file_nodes);
    collect_from_tree(&chain.callees, graph, &mut file_nodes);

    let mut result: Vec<_> = file_nodes.into_iter().collect();
    result.sort_by_key(|b| std::cmp::Reverse(b.1.len()));
    result
}

pub fn find_nodes_by_name(
    graph: &crate::graph::CodeGraph,
    query: &str,
) -> Vec<(NodeIndex, String)> {
    let lower = query.to_lowercase();
    let mut results: Vec<(NodeIndex, String, MatchRank)> = Vec::new();
    for idx in graph.node_indices() {
        let key = NodeKey::from_node(&graph[idx]);
        let display = key.to_string();
        let display_lower = display.to_lowercase();
        if let Some(rank) = MatchRank::classify(&lower, &display_lower) {
            results.push((idx, display, rank));
        }
    }
    results.sort_by(|a, b| a.2.cmp(&b.2).then_with(|| a.1.cmp(&b.1)));
    results
        .into_iter()
        .map(|(idx, display, _)| (idx, display))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchRank {
    Exact,
    WordBoundary,
    Substring,
}

impl MatchRank {
    pub fn classify(query: &str, candidate: &str) -> Option<Self> {
        if candidate == query {
            return Some(MatchRank::Exact);
        }
        let name_part = candidate
            .split_once(':')
            .map(|(_, n)| n)
            .unwrap_or(candidate);
        if name_part == query {
            return Some(MatchRank::Exact);
        }
        let bare_name = name_part
            .rsplit_once('.')
            .map(|(_, n)| n)
            .unwrap_or(name_part);
        if bare_name == query {
            return Some(MatchRank::Exact);
        }
        if candidate.contains(query) {
            let idx = candidate.find(query)?;
            let after = idx + query.len();
            let is_word_char = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
            let at_boundary = idx == 0
                || !is_word_char(candidate.as_bytes()[idx - 1])
                || (after < candidate.len() && !is_word_char(candidate.as_bytes()[after]));
            if at_boundary {
                Some(MatchRank::WordBoundary)
            } else {
                Some(MatchRank::Substring)
            }
        } else {
            None
        }
    }
}

pub fn low_degree_nodes(graph: &crate::graph::CodeGraph, max_degree: usize) -> Vec<DegreeInfo> {
    let mut nodes: Vec<DegreeInfo> = graph
        .node_indices()
        .filter_map(|idx| {
            let in_deg = graph.neighbors_directed(idx, Direction::Incoming).count();
            let out_deg = graph.neighbors_directed(idx, Direction::Outgoing).count();
            let total = in_deg + out_deg;
            if total <= max_degree {
                Some(DegreeInfo {
                    idx,
                    in_degree: in_deg,
                    out_degree: out_deg,
                    total_degree: total,
                })
            } else {
                None
            }
        })
        .collect();

    nodes.sort_by(|a, b| {
        a.total_degree.cmp(&b.total_degree).then_with(|| {
            let key_a = NodeKey::from_node(&graph[a.idx]).to_string();
            let key_b = NodeKey::from_node(&graph[b.idx]).to_string();
            key_a.cmp(&key_b)
        })
    });

    nodes
}

fn format_tree_node(
    node: &TreeNode,
    graph: &crate::graph::CodeGraph,
    prefix: &str,
    is_last: bool,
    lines: &mut Vec<String>,
) {
    let connector = if is_last { "└── " } else { "├── " };
    let key = crate::graph::node_display_name(&graph[node.idx]);
    let label = node
        .edge_label
        .as_deref()
        .map(|l| format!(" {}", l))
        .unwrap_or_default();
    lines.push(format!("{}{}{}{}", prefix, connector, key, label));

    let child_prefix = if is_last {
        format!("{}    ", prefix)
    } else {
        format!("{}│   ", prefix)
    };
    for (i, child) in node.children.iter().enumerate() {
        let last = i == node.children.len() - 1;
        format_tree_node(child, graph, &child_prefix, last, lines);
    }
}

pub fn format_chain_tree(chain: &CallChain, graph: &crate::graph::CodeGraph) -> String {
    let mut lines = Vec::new();

    lines.push("── CALLERS ──".to_string());
    if chain.callers.is_empty() {
        lines.push("  (none)".to_string());
    } else {
        for (i, caller) in chain.callers.iter().enumerate() {
            let is_last = i == chain.callers.len() - 1;
            let prefix = "  ";
            let connector = if is_last { "└── " } else { "├── " };
            let key = crate::graph::node_display_name(&graph[caller.idx]);
            let label = caller
                .edge_label
                .as_deref()
                .map(|l| format!(" {}", l))
                .unwrap_or_default();
            lines.push(format!("{}{}{}{}", prefix, connector, key, label));

            let child_prefix = if is_last { "      " } else { "  │   " };
            for (ci, child) in caller.children.iter().enumerate() {
                let child_last = ci == caller.children.len() - 1;
                format_tree_node(child, graph, child_prefix, child_last, &mut lines);
            }
        }
    }

    lines.push(String::new());
    lines.push("── TARGET ──".to_string());
    let key = crate::graph::node_display_name(&graph[chain.target]);
    lines.push(format!("  {}", key));

    lines.push(String::new());
    lines.push("── CALLEES ──".to_string());
    if chain.callees.is_empty() {
        lines.push("  (none)".to_string());
    } else {
        for (i, callee) in chain.callees.iter().enumerate() {
            let is_last = i == chain.callees.len() - 1;
            let prefix = "  ";
            let connector = if is_last { "└── " } else { "├── " };
            let key = crate::graph::node_display_name(&graph[callee.idx]);
            let label = callee
                .edge_label
                .as_deref()
                .map(|l| format!(" {}", l))
                .unwrap_or_default();
            lines.push(format!("{}{}{}{}", prefix, connector, key, label));

            let child_prefix = if is_last { "      " } else { "  │   " };
            for (ci, child) in callee.children.iter().enumerate() {
                let child_last = ci == callee.children.len() - 1;
                format_tree_node(child, graph, child_prefix, child_last, &mut lines);
            }
        }
    }

    lines.join("\n")
}

#[derive(Clone)]
struct PathStep {
    idx: NodeIndex,
    edge_label: Option<String>,
}

fn collect_leaf_paths(roots: &[TreeNode]) -> Vec<Vec<PathStep>> {
    let mut paths = Vec::new();
    let mut current = Vec::new();
    for root in roots {
        collect_paths_recursive(root, &mut current, &mut paths);
    }
    paths
}

fn collect_paths_recursive(
    node: &TreeNode,
    current: &mut Vec<PathStep>,
    paths: &mut Vec<Vec<PathStep>>,
) {
    current.push(PathStep {
        idx: node.idx,
        edge_label: node.edge_label.clone(),
    });
    if node.children.is_empty() {
        paths.push(current.clone());
    } else {
        for child in &node.children {
            collect_paths_recursive(child, current, paths);
        }
    }
    current.pop();
}

pub fn format_chain_paths(chain: &CallChain, graph: &crate::graph::CodeGraph) -> String {
    let mut lines = Vec::new();
    let target_key = crate::graph::node_display_name(&graph[chain.target]);

    // --- CALLERS: each path from farthest caller → direct caller → T0 ---
    let caller_paths = collect_leaf_paths(&chain.callers);
    // Reverse each path so it reads: farthest entry → ... → direct caller → T0
    let mut caller_paths: Vec<_> = caller_paths
        .into_iter()
        .map(|mut p| {
            p.reverse();
            p
        })
        .collect();
    // Sort by path length descending (longest first = deepest call chain)
    caller_paths.sort_by_key(|b| std::cmp::Reverse(b.len()));

    lines.push(format!("── CALLERS ({} paths) ──", caller_paths.len()));
    if caller_paths.is_empty() {
        lines.push("  (none)".to_string());
    } else {
        for (pi, path) in caller_paths.iter().enumerate() {
            let path_num = pi + 1;
            let hops = path.len();
            lines.push(format!("Path {:<4} {:>3} hops", path_num, hops));
            for (depth, step) in path.iter().enumerate() {
                let indent = "    ".repeat(depth);
                let key = crate::graph::node_display_name(&graph[step.idx]);
                let label = step
                    .edge_label
                    .as_deref()
                    .map(|l| format!(" {}", l))
                    .unwrap_or_default();
                lines.push(format!("{}→ {}{}", indent, key, label));
            }
            let t0_indent = "    ".repeat(path.len());
            lines.push(format!("{}→ T0 {}", t0_indent, target_key));
            if pi < caller_paths.len() - 1 {
                lines.push(String::new());
            }
        }
    }

    // --- CONVERGENCE summary ---
    if !caller_paths.is_empty() {
        let mut convergence: std::collections::HashMap<NodeIndex, usize> =
            std::collections::HashMap::new();
        for path in &caller_paths {
            for step in path {
                *convergence.entry(step.idx).or_insert(0) += 1;
            }
        }
        let mut conv_entries: Vec<_> = convergence.into_iter().collect();
        conv_entries.sort_by_key(|b| std::cmp::Reverse(b.1));
        lines.push(String::new());
        lines.push("── CONVERGENCE ──".to_string());
        for (idx, count) in &conv_entries {
            if *count > 1 {
                let key = crate::graph::node_display_name(&graph[*idx]);
                lines.push(format!("  {} ← {} paths", key, count));
            }
        }
    }

    // --- TARGET ---
    lines.push(String::new());
    lines.push("── TARGET ──".to_string());
    lines.push(format!("  {}", target_key));

    // --- CALLEES: each path from T0 → direct callee → deepest leaf ---
    let callee_paths = collect_leaf_paths(&chain.callees);
    let mut callee_paths = callee_paths;
    callee_paths.sort_by_key(|b| std::cmp::Reverse(b.len()));

    lines.push(String::new());
    lines.push(format!("── CALLEES ({} paths) ──", callee_paths.len()));
    if callee_paths.is_empty() {
        lines.push("  (none)".to_string());
    } else {
        for (pi, path) in callee_paths.iter().enumerate() {
            let path_num = pi + 1;
            let hops = path.len();
            lines.push(format!("Path {:<4} {:>3} hops", path_num, hops));
            for (depth, step) in path.iter().enumerate() {
                let indent = "    ".repeat(depth);
                let key = crate::graph::node_display_name(&graph[step.idx]);
                let label = step
                    .edge_label
                    .as_deref()
                    .map(|l| format!(" {}", l))
                    .unwrap_or_default();
                lines.push(format!("{}→ {}{}", indent, key, label));
            }
            if pi < callee_paths.len() - 1 {
                lines.push(String::new());
            }
        }
    }

    lines.join("\n")
}

pub fn format_chain(
    chain: &CallChain,
    graph: &crate::graph::CodeGraph,
    style: ChainStyle,
) -> String {
    match style {
        ChainStyle::Tree => format_chain_tree(chain, graph),
        ChainStyle::Path => format_chain_paths(chain, graph),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_rank_exact_beats_word_boundary() {
        let query = "func:bigfund.fnc_get_bank_account";
        let candidate_exact = "func:bigfund.fnc_get_bank_account";
        let candidate_longer = "func:bigfund.fnc_get_bank_account2";

        let rank_exact = MatchRank::classify(query, candidate_exact).unwrap();
        let rank_longer = MatchRank::classify(query, candidate_longer).unwrap();
        assert!(rank_exact < rank_longer);
    }

    #[test]
    fn match_rank_bare_name_is_exact() {
        let query = "fnc_get_bank_account";
        let candidate = "func:bigfund.fnc_get_bank_account";
        let rank = MatchRank::classify(query, candidate).unwrap();
        assert_eq!(rank, MatchRank::Exact);
    }

    #[test]
    fn match_rank_schema_qualified_is_exact() {
        let query = "bigfund.fnc_get_bank_account";
        let candidate = "func:bigfund.fnc_get_bank_account";
        let rank = MatchRank::classify(query, candidate).unwrap();
        assert_eq!(rank, MatchRank::Exact);
    }

    #[test]
    fn match_rank_word_boundary_beats_substring() {
        let query = "bank";
        let candidate_boundary = "func:bank_stuff";
        let candidate_mid = "func:someabankc_stuff";

        let rank_boundary = MatchRank::classify(query, candidate_boundary).unwrap();
        let rank_mid = MatchRank::classify(query, candidate_mid).unwrap();
        assert!(rank_boundary < rank_mid);
    }

    // ── builtin function filter tests ──

    /// Helper: collect all node keys from a tree into a set.
    fn tree_node_keys(nodes: &[TreeNode], graph: &crate::graph::CodeGraph) -> Vec<String> {
        let mut keys = Vec::new();
        for node in nodes {
            keys.push(NodeKey::from_node(&graph[node.idx]).to_string());
            keys.extend(tree_node_keys(&node.children, graph));
        }
        keys
    }

    fn make_test_graph() -> (
        crate::graph::CodeGraph,
        petgraph::graph::NodeIndex,
        petgraph::graph::NodeIndex,
    ) {
        use crate::graph::{Edge, RoutineId, RoutineKind, SourceLocation};
        use std::path::PathBuf;
        use std::sync::Arc;

        let mut graph = petgraph::Graph::new();
        let loc = SourceLocation {
            file: Arc::new(PathBuf::from("test.sql")),
            line: 1,
        };

        let proc_a = graph.add_node(crate::graph::Node::Procedure {
            id: RoutineId {
                schema: None,
                package: None,
                name: "proc_a".into(),
                kind: RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: vec![],
        });

        let builtin_count = graph.add_node(crate::graph::Node::BuiltinFunction {
            name: "count".into(),
            category: "aggregate".into(),
            domain: "sql".into(),
            location: loc.clone(),
        });

        graph.add_edge(
            proc_a,
            builtin_count,
            Edge::UsesBuiltinFunction { location: loc },
        );

        (graph, proc_a, builtin_count)
    }

    #[test]
    fn trace_chain_hides_builtins_when_skip() {
        let (graph, proc_a, _builtin) = make_test_graph();

        let (chain, _) = trace_chain(&graph, proc_a, 10, 100, true);

        // Proc_a's callees tree should NOT contain the builtin node
        let callee_keys = tree_node_keys(&chain.callees, &graph);
        assert!(
            !callee_keys.iter().any(|k| k == "builtin:count"),
            "Expected builtin:count to be filtered out from callees, but found it in: {:?}",
            callee_keys
        );

        // Callers should still be empty (no incoming edges to proc_a)
        assert!(chain.callers.is_empty(), "Expected no callers");
    }

    #[test]
    fn trace_chain_shows_builtins_when_not_skip() {
        let (graph, proc_a, _builtin) = make_test_graph();

        let (chain, _) = trace_chain(&graph, proc_a, 10, 100, false);

        // Callees tree SHOULD contain the builtin node
        let callee_keys = tree_node_keys(&chain.callees, &graph);
        assert!(
            callee_keys.iter().any(|k| k == "builtin:count"),
            "Expected builtin:count to be present in callees when skip_builtins=false, got: {:?}",
            callee_keys
        );
    }

    #[test]
    fn trace_chain_from_builtin_shows_callers() {
        let (graph, _proc_a, builtin_count) = make_test_graph();

        // Start from builtin, skip_builtins=true — the builtin node itself
        // should still be traversed (it's the start node), and its callers
        // (incoming neighbors) should include proc_a regardless of skip_builtins
        // because skip_builtins only filters the *neighbors* of the current node,
        // not the start node.
        let (chain, _) = trace_chain(&graph, builtin_count, 10, 100, true);

        // Callers should include proc_a
        let caller_keys = tree_node_keys(&chain.callers, &graph);
        assert!(
            caller_keys.iter().any(|k| k == "proc:proc_a"),
            "Expected proc_a to be in callers when tracing from builtin, got: {:?}",
            caller_keys
        );
    }

    // ── edge_label_for completeness tests ──
    // Regression: edge_label_for returns None for many edge types, causing
    // inspect --style tree to show edges without directional markers.
    // See Issue #4, #6, #7 in the inspect direction bug report.

    fn make_loc() -> crate::graph::SourceLocation {
        crate::graph::SourceLocation {
            file: std::sync::Arc::new(std::path::PathBuf::from("test.sql")),
            line: 1,
        }
    }

    fn add_proc_node(
        graph: &mut crate::graph::CodeGraph,
        name: &str,
    ) -> petgraph::graph::NodeIndex {
        graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("sch".into()),
                package: None,
                name: name.to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: make_loc(),
            partial: false,
            body_sql: vec![],
        })
    }

    #[test]
    fn edge_label_direct_call_intra_package() {
        let mut graph = crate::graph::CodeGraph::new();
        let a = add_proc_node(&mut graph, "a");
        let b = add_proc_node(&mut graph, "b");
        graph.add_edge(
            a,
            b,
            crate::graph::Edge::DirectCall {
                scope: crate::graph::CallScope::IntraPackage,
                location: make_loc(),
            },
        );

        let label = edge_label_for(&graph, a, b);
        assert_eq!(
            label.as_deref(),
            Some("[intra]"),
            "DirectCall[IntraPackage] must produce [intra] label"
        );
    }

    #[test]
    fn edge_label_direct_call_cross_package() {
        let mut graph = crate::graph::CodeGraph::new();
        let a = add_proc_node(&mut graph, "a");
        let b = add_proc_node(&mut graph, "b");
        graph.add_edge(
            a,
            b,
            crate::graph::Edge::DirectCall {
                scope: crate::graph::CallScope::CrossPackage,
                location: make_loc(),
            },
        );

        let label = edge_label_for(&graph, a, b);
        assert_eq!(
            label.as_deref(),
            Some("[cross]"),
            "DirectCall[CrossPackage] must produce [cross] label"
        );
    }

    #[test]
    fn edge_label_direct_call_external_has_label() {
        // REGRESSION: CallScope::External currently returns None via `None?`
        // in edge_label_for, leaving the edge unmarked.
        // This test documents the current (suboptimal) behavior.
        let mut graph = crate::graph::CodeGraph::new();
        let a = add_proc_node(&mut graph, "a");
        let b = add_proc_node(&mut graph, "b");
        graph.add_edge(
            a,
            b,
            crate::graph::Edge::DirectCall {
                scope: crate::graph::CallScope::External,
                location: make_loc(),
            },
        );

        let label = edge_label_for(&graph, a, b);
        assert_eq!(
            label.as_deref(),
            Some("[external]"),
            "DirectCall[External] must produce [external] label"
        );
    }

    #[test]
    fn edge_label_dynamic_call() {
        let mut graph = crate::graph::CodeGraph::new();
        let a = add_proc_node(&mut graph, "a");
        let b = add_proc_node(&mut graph, "b");
        graph.add_edge(
            a,
            b,
            crate::graph::Edge::DynamicCall {
                raw_expr: "some_var".into(),
                location: make_loc(),
            },
        );

        let label = edge_label_for(&graph, a, b);
        assert_eq!(
            label.as_deref(),
            Some("[dynamic]"),
            "DynamicCall must produce [dynamic] label"
        );
    }

    #[test]
    fn edge_label_calls_procedure_has_label() {
        // REGRESSION: CallsProcedure edges (used by MyBatis/JavaSql→Procedure)
        // currently fall through to `_ => None` in edge_label_for.
        // In inspect --style tree, these edges appear without ← markers.
        let mut graph = crate::graph::CodeGraph::new();
        let a = add_proc_node(&mut graph, "a"); // stand-in for a non-proc caller
        let b = add_proc_node(&mut graph, "b");
        graph.add_edge(
            a,
            b,
            crate::graph::Edge::CallsProcedure {
                location: make_loc(),
            },
        );

        let label = edge_label_for(&graph, a, b);
        assert_eq!(
            label.as_deref(),
            Some("[calls]"),
            "CallsProcedure must produce [calls] label for direction clarity"
        );
    }

    #[test]
    fn edge_label_uses_builtin_function() {
        let mut graph = crate::graph::CodeGraph::new();
        let a = add_proc_node(&mut graph, "a");
        let b = graph.add_node(crate::graph::Node::BuiltinFunction {
            name: "count".into(),
            category: "aggregate".into(),
            domain: "sql".into(),
            location: make_loc(),
        });
        graph.add_edge(
            a,
            b,
            crate::graph::Edge::UsesBuiltinFunction {
                location: make_loc(),
            },
        );

        let label = edge_label_for(&graph, a, b);
        assert_eq!(
            label.as_deref(),
            Some("[builtin]"),
            "UsesBuiltinFunction must produce [builtin] label"
        );
    }

    // ── has_more / more_count truncation markers (Issue #152) ──

    /// Build a linear call chain n0 → n1 → … → n_{k-1} of procedure nodes.
    fn make_chain(names: &[&str]) -> (crate::graph::CodeGraph, Vec<petgraph::graph::NodeIndex>) {
        let mut graph = crate::graph::CodeGraph::new();
        let idxs: Vec<_> = names.iter().map(|n| add_proc_node(&mut graph, n)).collect();
        for w in idxs.windows(2) {
            graph.add_edge(
                w[0],
                w[1],
                crate::graph::Edge::DirectCall {
                    scope: crate::graph::CallScope::IntraPackage,
                    location: make_loc(),
                },
            );
        }
        (graph, idxs)
    }

    fn add_call(graph: &mut crate::graph::CodeGraph, from: NodeIndex, to: NodeIndex) {
        graph.add_edge(
            from,
            to,
            crate::graph::Edge::DirectCall {
                scope: crate::graph::CallScope::IntraPackage,
                location: make_loc(),
            },
        );
    }

    #[test]
    fn truncated_by_budget_marks_has_more_with_exact_count() {
        // Chain a→b→c→d with budget 2: b and c are visited; recursion into c
        // hits the budget cap, so c is a "fake leaf" with one hidden child (d).
        let (graph, idxs) = make_chain(&["a", "b", "c", "d"]);
        let (chain, _) = trace_chain(&graph, idxs[0], 10, 2, false);

        let b = &chain.callees[0];
        assert!(!b.has_more, "b is fully expanded within budget");
        assert_eq!(b.more_count, 0);

        let c = &b.children[0];
        assert!(c.has_more, "budget-truncated node must report has_more");
        assert_eq!(c.more_count, 1, "d is the single unexpanded direct child");
        assert!(
            c.children.is_empty(),
            "truncated node must have no children"
        );
    }

    #[test]
    fn sibling_truncation_marks_parent_has_more() {
        // a→b, b→{c,d} with budget 2: only one of c/d fits; the other is
        // silently dropped by the loop break — b must report it.
        let (mut graph, idxs) = make_chain(&["a", "b"]);
        let c = add_proc_node(&mut graph, "c");
        let d = add_proc_node(&mut graph, "d");
        add_call(&mut graph, idxs[1], c);
        add_call(&mut graph, idxs[1], d);
        let (chain, _) = trace_chain(&graph, idxs[0], 10, 2, false);

        let b = &chain.callees[0];
        assert!(
            b.has_more,
            "loop-break truncation must mark parent has_more"
        );
        assert_eq!(b.more_count, 1, "exactly one direct child was dropped");
        assert_eq!(b.children.len(), 1, "only one of c/d fits in budget");
    }

    #[test]
    fn depth_boundary_marks_has_more() {
        // a→b→c with max_depth=1: b is shown but its children are not explored.
        let (graph, idxs) = make_chain(&["a", "b", "c"]);
        let (chain, _) = trace_chain(&graph, idxs[0], 1, 100, false);

        let b = &chain.callees[0];
        assert!(b.has_more, "depth-clamped node must report has_more");
        assert_eq!(b.more_count, 1, "c is the single unexplored direct child");
        assert!(b.children.is_empty());
    }

    #[test]
    fn true_leaf_has_no_more() {
        let (graph, idxs) = make_chain(&["a", "b"]);
        let (chain, _) = trace_chain(&graph, idxs[0], 10, 100, false);

        let b = &chain.callees[0];
        assert!(!b.has_more, "a real leaf must not be flagged as truncated");
        assert_eq!(b.more_count, 0);
    }

    #[test]
    fn cycle_neighbors_do_not_false_positive() {
        // a↔b: b's only neighbor is its own ancestor and must be excluded,
        // so b is a genuine leaf — has_more must stay false.
        let (mut graph, idxs) = make_chain(&["a", "b"]);
        add_call(&mut graph, idxs[1], idxs[0]);
        let (chain, _) = trace_chain(&graph, idxs[0], 10, 100, false);

        let b = &chain.callees[0];
        assert!(
            !b.has_more,
            "ancestor-filtered neighbors must not count as more"
        );
        assert_eq!(b.more_count, 0);
    }

    #[test]
    fn skip_builtins_respected_in_more_count() {
        // b→{builtin, c} with skip_builtins=true: the builtin is filtered
        // before counting, so b shows exactly one child and no truncation.
        let (mut graph, idxs) = make_chain(&["a", "b"]);
        let c = add_proc_node(&mut graph, "c");
        add_call(&mut graph, idxs[1], c);
        let builtin = graph.add_node(crate::graph::Node::BuiltinFunction {
            name: "count".into(),
            category: "aggregate".into(),
            domain: "sql".into(),
            location: make_loc(),
        });
        graph.add_edge(
            idxs[1],
            builtin,
            crate::graph::Edge::UsesBuiltinFunction {
                location: make_loc(),
            },
        );
        let (chain, _) = trace_chain(&graph, idxs[0], 10, 100, true);

        let b = &chain.callees[0];
        assert!(!b.has_more, "filtered builtins must not inflate more_count");
        assert_eq!(b.more_count, 0);
        assert_eq!(b.children.len(), 1, "only proc c remains after filtering");
    }

    #[test]
    fn edge_label_none_for_unlabeled_edge_types() {
        let unlabeled_edges: Vec<crate::graph::Edge> = vec![
            crate::graph::Edge::ContainsRoutine,
            crate::graph::Edge::ContainsMethod,
        ];

        for edge in &unlabeled_edges {
            let mut g = crate::graph::CodeGraph::new();
            let x = add_proc_node(&mut g, "x");
            let y = add_proc_node(&mut g, "y");
            g.add_edge(x, y, edge.clone());
            let label = edge_label_for(&g, x, y);
            assert!(
                label.is_none(),
                "{:?} currently returns None — consider adding a label",
                edge
            );
        }
    }
}

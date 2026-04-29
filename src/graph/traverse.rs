use std::collections::HashSet;

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

fn edge_label_for(
    graph: &crate::graph::CodeGraph,
    from: NodeIndex,
    to: NodeIndex,
) -> Option<String> {
    use crate::graph::{AccessMode, Edge, WriteKind};
    let edge = graph.edges_connecting(from, to).next()?;
    match edge.weight() {
        Edge::TableAccess {
            modes, write_kinds, ..
        } => {
            let mut parts = Vec::new();
            if modes.contains(AccessMode::Read) {
                parts.push("R".to_string());
            }
            if modes.contains(AccessMode::Write) {
                let wk: Vec<&str> = write_kinds
                    .iter()
                    .map(|wk| match wk {
                        WriteKind::Insert => "insert",
                        WriteKind::InsertSelect => "insert_select",
                        WriteKind::Update => "update",
                        WriteKind::Delete => "delete",
                        WriteKind::MergeInsert => "merge_insert",
                        WriteKind::MergeUpdate => "merge_update",
                        WriteKind::MergeDelete => "merge_delete",
                        WriteKind::SelectInto => "select_into",
                        WriteKind::Truncate => "truncate",
                    })
                    .collect();
                if wk.is_empty() {
                    parts.push("W".to_string());
                } else {
                    parts.push(format!("W:{}", wk.join(",")));
                }
            }
            if modes.contains(AccessMode::LockRead) {
                parts.push("lock".to_string());
            }
            if modes.contains(AccessMode::Truncate) {
                parts.push("truncate".to_string());
            }
            if parts.is_empty() {
                None
            } else {
                Some(format!("[{}]", parts.join(",")))
            }
        }
        Edge::DynamicCall { .. } => Some("[dynamic]".into()),
        _ => None,
    }
}

fn build_tree_dfs(
    graph: &crate::graph::CodeGraph,
    start: NodeIndex,
    direction: Direction,
    visited: &mut HashSet<NodeIndex>,
    depth: usize,
) -> Vec<TreeNode> {
    let max_depth = 50;
    if depth > max_depth {
        let key = crate::graph::key::NodeKey::from_node(&graph[start]);
        eprintln!(
            "  ⚠ trace depth exceeded {} at '{}' — possible runaway chain, stopping",
            max_depth, key
        );
        return Vec::new();
    }

    let mut roots = Vec::new();
    let neighbors: Vec<NodeIndex> = graph
        .neighbors_directed(start, direction)
        .filter(|n| !visited.contains(n))
        .collect();

    for neighbor in neighbors {
        if visited.insert(neighbor) {
            let (from, to) = match direction {
                Direction::Outgoing => (start, neighbor),
                Direction::Incoming => (neighbor, start),
            };
            let edge_label = edge_label_for(graph, from, to);
            let children = build_tree_dfs(graph, neighbor, direction, visited, depth + 1);
            roots.push(TreeNode {
                idx: neighbor,
                edge_label,
                children,
            });
        }
    }
    roots
}

pub fn trace_chain(graph: &crate::graph::CodeGraph, start: NodeIndex) -> CallChain {
    let mut visited_callers = HashSet::new();
    visited_callers.insert(start);
    let callers = build_tree_dfs(graph, start, Direction::Incoming, &mut visited_callers, 0);

    let mut visited_callees = HashSet::new();
    visited_callees.insert(start);
    let callees = build_tree_dfs(graph, start, Direction::Outgoing, &mut visited_callees, 0);

    CallChain {
        target: start,
        callers,
        callees,
    }
}

pub fn find_nodes_by_name(
    graph: &crate::graph::CodeGraph,
    query: &str,
) -> Vec<(NodeIndex, String)> {
    let lower = query.to_lowercase();
    let mut results = Vec::new();
    for idx in graph.node_indices() {
        let key = NodeKey::from_node(&graph[idx]);
        let display = key.to_string();
        if display.to_lowercase().contains(&lower) {
            results.push((idx, display));
        }
    }
    results
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
    let key = NodeKey::from_node(&graph[node.idx]);
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
            let key = NodeKey::from_node(&graph[caller.idx]);
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
    let key = NodeKey::from_node(&graph[chain.target]);
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
            let key = NodeKey::from_node(&graph[callee.idx]);
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
    let target_key = NodeKey::from_node(&graph[chain.target]);

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

    lines.push(format!(
        "── CALLERS ({} paths) ──",
        caller_paths.len()
    ));
    if caller_paths.is_empty() {
        lines.push("  (none)".to_string());
    } else {
        for (pi, path) in caller_paths.iter().enumerate() {
            let path_num = pi + 1;
            let hops = path.len();
            lines.push(format!("Path {:<4} {:>3} hops", path_num, hops));
            for (depth, step) in path.iter().enumerate() {
                let indent = "    ".repeat(depth);
                let key = NodeKey::from_node(&graph[step.idx]);
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
        conv_entries.sort_by(|a, b| b.1.cmp(&a.1));
        lines.push(String::new());
        lines.push("── CONVERGENCE ──".to_string());
        for (idx, count) in &conv_entries {
            if *count > 1 {
                let key = NodeKey::from_node(&graph[*idx]);
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
    lines.push(format!(
        "── CALLEES ({} paths) ──",
        callee_paths.len()
    ));
    if callee_paths.is_empty() {
        lines.push("  (none)".to_string());
    } else {
        for (pi, path) in callee_paths.iter().enumerate() {
            let path_num = pi + 1;
            let hops = path.len();
            lines.push(format!("Path {:<4} {:>3} hops", path_num, hops));
            for (depth, step) in path.iter().enumerate() {
                let indent = "    ".repeat(depth);
                let key = NodeKey::from_node(&graph[step.idx]);
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

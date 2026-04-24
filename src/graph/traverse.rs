use std::collections::HashSet;

use petgraph::graph::NodeIndex;
use petgraph::Direction;

use crate::graph::key::NodeKey;

pub struct TreeNode {
    pub idx: NodeIndex,
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

fn build_tree_dfs(
    graph: &crate::graph::CodeGraph,
    start: NodeIndex,
    direction: Direction,
    visited: &mut HashSet<NodeIndex>,
) -> Vec<TreeNode> {
    let mut roots = Vec::new();
    let neighbors: Vec<NodeIndex> = graph
        .neighbors_directed(start, direction)
        .filter(|n| !visited.contains(n))
        .collect();

    for neighbor in neighbors {
        if visited.insert(neighbor) {
            let children = build_tree_dfs(graph, neighbor, direction, visited);
            roots.push(TreeNode {
                idx: neighbor,
                children,
            });
        }
    }
    roots
}

pub fn trace_chain(graph: &crate::graph::CodeGraph, start: NodeIndex) -> CallChain {
    let mut visited = HashSet::new();
    visited.insert(start);

    let callers = build_tree_dfs(graph, start, Direction::Incoming, &mut visited);
    let callees = build_tree_dfs(graph, start, Direction::Outgoing, &mut visited);

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
    lines.push(format!("{}{}{}", prefix, connector, key));

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
            lines.push(format!("{}{}{}", prefix, connector, key));

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
            lines.push(format!("{}{}{}", prefix, connector, key));

            let child_prefix = if is_last { "      " } else { "  │   " };
            for (ci, child) in callee.children.iter().enumerate() {
                let child_last = ci == callee.children.len() - 1;
                format_tree_node(child, graph, child_prefix, child_last, &mut lines);
            }
        }
    }

    lines.join("\n")
}

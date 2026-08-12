use std::collections::{HashMap, HashSet, VecDeque};
use petgraph::graph::NodeIndex;
use petgraph::Direction;
use petgraph::visit::EdgeRef;

use crate::graph::{CodeGraph, Edge, AccessMode};

/// Direction of lineage traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineageDirection {
    /// Upstream: who writes/defines this node
    Upstream,
    /// Downstream: who consumes/reads this node
    Downstream,
}

/// A step in the lineage chain (routine node with access metadata).
#[derive(Debug, Clone)]
pub struct LineageStep {
    pub routine_idx: NodeIndex,
    pub modes: AccessMode,
}

/// A node in the lineage result tree.
#[derive(Debug, Clone)]
pub struct LineageNode {
    pub idx: NodeIndex,
    pub _depth: usize,
    pub steps: Vec<LineageStep>,  // How we got here (immediate incoming routine steps)
    pub children: Vec<LineageNode>,
}

/// Compute table-level lineage using breadth-first search.
///
/// Algorithm:
/// - UPSTREAM: table → (Incoming Write TableAccess) → routine → (Outgoing Read TableAccess) → table → recurse
/// - DOWNSTREAM: table → (Incoming Read TableAccess) → routine → (Outgoing Write TableAccess) → table → recurse
/// - Views: follow DependsOn edges as if they were transparent
pub fn lineage_table(
    graph: &CodeGraph,
    start_idx: NodeIndex,
    direction: LineageDirection,
    depth: usize,
) -> LineageNode {
    let mut visited: HashSet<NodeIndex> = HashSet::new();
    let mut queue: VecDeque<(NodeIndex, usize)> = VecDeque::new();

    queue.push_back((start_idx, 0));
    visited.insert(start_idx);

    let mut children_map: HashMap<NodeIndex, Vec<(LineageNode, LineageStep)>> = HashMap::new();

    while let Some((current_idx, current_depth)) = queue.pop_front() {
        if current_depth >= depth {
            continue;
        }

        match direction {
            LineageDirection::Upstream => {
                // Get all incoming Write edges (who writes this table)
                let mut routine_accesses: Vec<(NodeIndex, AccessMode)> = Vec::new();
                for edge_ref in graph.edges_directed(current_idx, Direction::Incoming) {
                    if let Edge::TableAccess { modes, .. } = edge_ref.weight() {
                        if modes.contains(AccessMode::Write) {
                            routine_accesses.push((edge_ref.source(), *modes));
                        }
                    }
                }

                // From each writing routine, get all tables it reads
                for (routine_idx, routine_modes) in routine_accesses {
                    for edge_ref in graph.edges_directed(routine_idx, Direction::Outgoing) {
                        match edge_ref.weight() {
                            Edge::TableAccess { modes, .. } if modes.contains(AccessMode::Read) => {
                                let target_table = edge_ref.target();
                                if !visited.contains(&target_table) {
                                    visited.insert(target_table);
                                    queue.push_back((target_table, current_depth + 1));

                                    let step = LineageStep {
                                        routine_idx,
                                        modes: routine_modes,
                                    };
                                    children_map
                                        .entry(current_idx)
                                        .or_insert_with(Vec::new)
                                        .push((
                                            LineageNode {
                                                idx: target_table,
                                                _depth: current_depth + 1,
                                                steps: vec![],
                                                children: Vec::new(),
                                            },
                                            step,
                                        ));
                                }
                            }
                            _ => {}
                        }
                    }
                }

                // Also follow DependsOn edges (view dependencies)
                for edge_ref in graph.edges_directed(current_idx, Direction::Incoming) {
                    if matches!(edge_ref.weight(), Edge::DependsOn { .. }) {
                        let source_view = edge_ref.source();
                        if !visited.contains(&source_view) {
                            visited.insert(source_view);
                            queue.push_back((source_view, current_depth + 1));

                            children_map
                                .entry(current_idx)
                                .or_insert_with(Vec::new)
                                .push((
                                    LineageNode {
                                        idx: source_view,
                                        _depth: current_depth + 1,
                                        steps: vec![],
                                        children: Vec::new(),
                                    },
                                    LineageStep {
                                        routine_idx: source_view,  // View itself as "step"
                                        modes: AccessMode::Read,
                                    },
                                ));
                        }
                    }
                }
            }
            LineageDirection::Downstream => {
                // Get all incoming Read edges (who reads this table)
                let mut routine_accesses: Vec<(NodeIndex, AccessMode)> = Vec::new();
                for edge_ref in graph.edges_directed(current_idx, Direction::Incoming) {
                    if let Edge::TableAccess { modes, .. } = edge_ref.weight() {
                        if modes.contains(AccessMode::Read) {
                            routine_accesses.push((edge_ref.source(), *modes));
                        }
                    }
                }

                // From each reading routine, get all tables it writes
                for (routine_idx, routine_modes) in routine_accesses {
                    for edge_ref in graph.edges_directed(routine_idx, Direction::Outgoing) {
                        match edge_ref.weight() {
                            Edge::TableAccess { modes, .. } if modes.contains(AccessMode::Write) => {
                                let target_table = edge_ref.target();
                                if !visited.contains(&target_table) {
                                    visited.insert(target_table);
                                    queue.push_back((target_table, current_depth + 1));

                                    let step = LineageStep {
                                        routine_idx,
                                        modes: routine_modes,
                                    };
                                    children_map
                                        .entry(current_idx)
                                        .or_insert_with(Vec::new)
                                        .push((
                                            LineageNode {
                                                idx: target_table,
                                                _depth: current_depth + 1,
                                                steps: vec![],
                                                children: Vec::new(),
                                            },
                                            step,
                                        ));
                                }
                            }
                            _ => {}
                        }
                    }
                }

                // Also follow DependsOn edges reverse (tables that depend on this view)
                for edge_ref in graph.edges_directed(current_idx, Direction::Outgoing) {
                    if matches!(edge_ref.weight(), Edge::DependsOn { .. }) {
                        let target_view = edge_ref.target();
                        if !visited.contains(&target_view) {
                            visited.insert(target_view);
                            queue.push_back((target_view, current_depth + 1));

                            children_map
                                .entry(current_idx)
                                .or_insert_with(Vec::new)
                                .push((
                                    LineageNode {
                                        idx: target_view,
                                        _depth: current_depth + 1,
                                        steps: vec![],
                                        children: Vec::new(),
                                    },
                                    LineageStep {
                                        routine_idx: target_view,  // View itself
                                        modes: AccessMode::Write,
                                    },
                                ));
                        }
                    }
                }
            }
        }
    }

    // Reconstruct tree: build root node
    fn build_node(
        idx: NodeIndex,
        children_map: &HashMap<NodeIndex, Vec<(LineageNode, LineageStep)>>,
    ) -> LineageNode {
        let (mut immediate_children, mut steps) = if let Some(entries) = children_map.get(&idx) {
            let mut stps: Vec<LineageStep> = entries.iter().map(|(_, s)| s.clone()).collect();
            stps.sort_by(|a, b| {
                a.routine_idx.index().cmp(&b.routine_idx.index())
            });
            let children: Vec<LineageNode> = entries
                .iter()
                .map(|(child_template, _)| build_node(child_template.idx, children_map))
                .collect();
            (children, stps)
        } else {
            (Vec::new(), Vec::new())
        };

        // Deduplicate: group by (routine_idx, modes) and keep unique
        let mut seen: HashSet<(usize, u8)> = HashSet::new();
        steps.retain(|s| {
            let key = (s.routine_idx.index(), s.modes.bits());
            seen.insert(key)
        });

        // De-duplicate children
        immediate_children.sort_by(|a, b| a.idx.index().cmp(&b.idx.index()));
        immediate_children.dedup_by(|a, b| a.idx == b.idx);

        LineageNode {
            idx,
            _depth: 0,
            steps,
            children: immediate_children,
        }
    }

    build_node(start_idx, &children_map)
}

/// Format lineage node as tree string.
pub fn format_lineage_tree(
    node: &LineageNode,
    graph: &CodeGraph,
    indent: usize,
) -> String {
    let mut result = String::new();
    let indent_str = "  ".repeat(indent);

    let node_key = crate::graph::key::NodeKey::from_node(&graph[node.idx]);
    result.push_str(&format!("{}{}", indent_str, node_key));

    // Show incoming steps as edge labels
    if !node.steps.is_empty() {
        let step_labels: Vec<String> = node
            .steps
            .iter()
            .map(|step| {
                let routine_key = crate::graph::key::NodeKey::from_node(&graph[step.routine_idx]);
                format!("{}", routine_key)
            })
            .collect();
        result.push_str(&format!("  [← {}]", step_labels.join(", ")));
    }
    result.push('\n');

    for child in &node.children {
        result.push_str(&format_lineage_tree(child, graph, indent + 1));
    }

    result
}

/// Format lineage node as JSON.
pub fn format_lineage_json(node: &LineageNode, graph: &CodeGraph) -> serde_json::Value {
    let node_key = crate::graph::key::NodeKey::from_node(&graph[node.idx]);
    let step_labels: Vec<String> = node
        .steps
        .iter()
        .map(|step| crate::graph::key::NodeKey::from_node(&graph[step.routine_idx]).to_string())
        .collect();

    let children_json: Vec<serde_json::Value> = node
        .children
        .iter()
        .map(|child| format_lineage_json(child, graph))
        .collect();

    serde_json::json!({
        "node": node_key.to_string(),
        "via": step_labels,
        "children": children_json,
    })
}

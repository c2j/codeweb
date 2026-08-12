use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use petgraph::Direction;
use std::collections::{HashMap, HashSet, VecDeque};

use crate::graph::{AccessMode, CodeGraph, Edge};

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
    pub steps: Vec<LineageStep>, // How we got here (immediate incoming routine steps)
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

                // Follow DependsOn edges: view → base table. When the current node is a
                // view, its base tables are upstream, so walk outgoing edges.
                for edge_ref in graph.edges_directed(current_idx, Direction::Outgoing) {
                    if matches!(edge_ref.weight(), Edge::DependsOn { .. }) {
                        let base_table = edge_ref.target();
                        if !visited.contains(&base_table) {
                            visited.insert(base_table);
                            queue.push_back((base_table, current_depth + 1));

                            children_map
                                .entry(current_idx)
                                .or_insert_with(Vec::new)
                                .push((
                                    LineageNode {
                                        idx: base_table,
                                        _depth: current_depth + 1,
                                        steps: vec![],
                                        children: Vec::new(),
                                    },
                                    LineageStep {
                                        routine_idx: current_idx, // the view's own definition
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
                            Edge::TableAccess { modes, .. }
                                if modes.contains(AccessMode::Write) =>
                            {
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

                // Follow DependsOn edges: view → base table. Views that select from the
                // current table are downstream consumers, so walk incoming edges.
                for edge_ref in graph.edges_directed(current_idx, Direction::Incoming) {
                    if matches!(edge_ref.weight(), Edge::DependsOn { .. }) {
                        let consuming_view = edge_ref.source();
                        if !visited.contains(&consuming_view) {
                            visited.insert(consuming_view);
                            queue.push_back((consuming_view, current_depth + 1));

                            children_map
                                .entry(current_idx)
                                .or_insert_with(Vec::new)
                                .push((
                                    LineageNode {
                                        idx: consuming_view,
                                        _depth: current_depth + 1,
                                        steps: vec![],
                                        children: Vec::new(),
                                    },
                                    LineageStep {
                                        routine_idx: consuming_view, // the view's own definition
                                        modes: AccessMode::Read,
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
            stps.sort_by_key(|a| a.routine_idx.index());
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
        immediate_children.sort_by_key(|a| a.idx.index());
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
    direction: LineageDirection,
    indent: usize,
) -> String {
    let mut result = String::new();
    let indent_str = "  ".repeat(indent);

    let node_key = crate::graph::key::NodeKey::from_node(&graph[node.idx]);
    result.push_str(&format!("{}{}", indent_str, node_key));

    // Label the routines/views connecting this node to the children listed below it.
    // A view reaching its own base tables lists itself; that adds no information.
    let step_labels = step_labels(node, graph);
    if !step_labels.is_empty() {
        let arrow = match direction {
            LineageDirection::Upstream => "written by",
            LineageDirection::Downstream => "read by",
        };
        result.push_str(&format!("  [{} {}]", arrow, step_labels.join(", ")));
    }
    result.push('\n');

    for child in &node.children {
        result.push_str(&format_lineage_tree(child, graph, direction, indent + 1));
    }

    result
}

fn step_labels(node: &LineageNode, graph: &CodeGraph) -> Vec<String> {
    node.steps
        .iter()
        .filter(|step| step.routine_idx != node.idx)
        .map(|step| crate::graph::key::NodeKey::from_node(&graph[step.routine_idx]).to_string())
        .collect()
}

/// Format lineage node as JSON.
pub fn format_lineage_json(node: &LineageNode, graph: &CodeGraph) -> serde_json::Value {
    let node_key = crate::graph::key::NodeKey::from_node(&graph[node.idx]);

    let children_json: Vec<serde_json::Value> = node
        .children
        .iter()
        .map(|child| format_lineage_json(child, graph))
        .collect();

    serde_json::json!({
        "node": node_key.to_string(),
        "via": step_labels(node, graph),
        "children": children_json,
    })
}

// ── Column-level lineage (#136) ──────────────────────────────────────────────

use crate::parser::{ColumnMapping, ColumnSource, MappingKind};

/// One column of one table or view, and where its value comes from (or goes to).
#[derive(Debug, Clone)]
pub struct ColumnLineageNode {
    pub table: String,
    pub column: String,
    pub steps: Vec<ColumnLineageStep>,
}

/// A single hop of column-level data flow.
#[derive(Debug, Clone)]
pub struct ColumnLineageStep {
    /// The routine performing the write (upstream) or the read (downstream).
    pub via: NodeIndex,
    pub kind: MappingKind,
    /// Source expression text, when the value is not a plain column copy.
    pub expression: Option<String>,
    /// The column, literal, variable, or dynamic marker on the other side.
    pub source: ColumnSource,
    /// Set only when `source` is a column that could be resolved and walked further.
    /// A literal, an unresolved variable, or dynamic SQL ends the chain here.
    pub next: Option<ColumnLineageNode>,
}

/// Collect the column mappings a routine declares, deduplicated.
///
/// The same per-statement `ColumnAnalysis` is attached to every edge of that statement,
/// so `INSERT INTO a SELECT FROM b` carries identical mappings on both the write edge to
/// `a` and the read edge from `b`. Gathering across all of a routine's edges and then
/// deduplicating is what keeps a self-referencing `UPDATE` from reporting every mapping
/// twice.
fn mappings_of_routine(graph: &CodeGraph, routine: NodeIndex) -> Vec<ColumnMapping> {
    let mut out: Vec<ColumnMapping> = Vec::new();
    for dir in [Direction::Outgoing, Direction::Incoming] {
        for edge_ref in graph.edges_directed(routine, dir) {
            if let Edge::TableAccess {
                column_analysis: Some(analysis),
                ..
            } = edge_ref.weight()
            {
                for m in &analysis.column_mappings {
                    if !out.contains(m) {
                        out.push(m.clone());
                    }
                }
            }
        }
    }
    out
}

/// Find the Table or View node named `name`, case-insensitively.
fn find_table_node(graph: &CodeGraph, name: &str) -> Option<NodeIndex> {
    graph.node_indices().find(|idx| {
        let node_name = match &graph[*idx] {
            crate::graph::Node::Table { name, .. } | crate::graph::Node::View { name, .. } => name,
            _ => return false,
        };
        node_name.eq_ignore_ascii_case(name)
    })
}

fn eq(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// Trace one column's data flow.
///
/// Upstream answers "what feeds this column": for each routine writing the table, the
/// mappings targeting this column, then recursively each of their source columns.
/// Downstream answers "where does this column go": for each routine reading the table,
/// the mappings that consume this column, then recursively their target columns.
pub fn lineage_column(
    graph: &CodeGraph,
    table: &str,
    column: &str,
    direction: LineageDirection,
    depth: usize,
) -> ColumnLineageNode {
    let mut seen = HashSet::new();
    lineage_column_inner(graph, table, column, direction, depth, &mut seen)
}

fn lineage_column_inner(
    graph: &CodeGraph,
    table: &str,
    column: &str,
    direction: LineageDirection,
    depth: usize,
    seen: &mut HashSet<(String, String)>,
) -> ColumnLineageNode {
    let mut node = ColumnLineageNode {
        table: table.to_string(),
        column: column.to_string(),
        steps: Vec::new(),
    };

    let key = (table.to_lowercase(), column.to_lowercase());
    if depth == 0 || !seen.insert(key) {
        return node;
    }

    let Some(table_idx) = find_table_node(graph, table) else {
        return node;
    };

    // Routines on the relevant side of this table: writers for upstream, readers for
    // downstream.
    let wanted = match direction {
        LineageDirection::Upstream => AccessMode::Write,
        LineageDirection::Downstream => AccessMode::Read,
    };
    let mut routines: Vec<NodeIndex> = Vec::new();
    for edge_ref in graph.edges_directed(table_idx, Direction::Incoming) {
        if let Edge::TableAccess { modes, .. } = edge_ref.weight() {
            if modes.contains(wanted) && !routines.contains(&edge_ref.source()) {
                routines.push(edge_ref.source());
            }
        }
    }

    for routine in routines {
        for mapping in mappings_of_routine(graph, routine) {
            match direction {
                LineageDirection::Upstream => {
                    let targets_this_column = mapping
                        .target_table
                        .as_deref()
                        .is_some_and(|t| eq(t, table))
                        && eq(&mapping.target_column, column);
                    if !targets_this_column {
                        continue;
                    }
                    for source in &mapping.sources {
                        let next = match source {
                            ColumnSource::Column {
                                table: Some(src_table),
                                column: src_column,
                            } => Some(lineage_column_inner(
                                graph,
                                src_table,
                                src_column,
                                direction,
                                depth - 1,
                                seen,
                            )),
                            _ => None,
                        };
                        node.steps.push(ColumnLineageStep {
                            via: routine,
                            kind: mapping.kind.clone(),
                            expression: mapping.expression.clone(),
                            source: source.clone(),
                            next,
                        });
                    }
                }
                LineageDirection::Downstream => {
                    // This column is consumed if it appears among the mapping's sources.
                    let consumes_this_column = mapping.sources.iter().any(|s| match s {
                        ColumnSource::Column {
                            table: Some(t),
                            column: c,
                        } => eq(t, table) && eq(c, column),
                        _ => false,
                    });
                    if !consumes_this_column {
                        continue;
                    }
                    let Some(target_table) = mapping.target_table.as_deref() else {
                        continue;
                    };
                    let next = lineage_column_inner(
                        graph,
                        target_table,
                        &mapping.target_column,
                        direction,
                        depth - 1,
                        seen,
                    );
                    node.steps.push(ColumnLineageStep {
                        via: routine,
                        kind: mapping.kind.clone(),
                        expression: mapping.expression.clone(),
                        source: ColumnSource::Column {
                            table: Some(target_table.to_string()),
                            column: mapping.target_column.clone(),
                        },
                        next: Some(next),
                    });
                }
            }
        }
    }

    seen.remove(&(table.to_lowercase(), column.to_lowercase()));
    node
}

fn describe_kind(kind: &MappingKind) -> String {
    match kind {
        MappingKind::Direct => "direct".to_string(),
        MappingKind::Derived => "derived".to_string(),
        MappingKind::Aggregated { function, distinct } => {
            if *distinct {
                format!("{} DISTINCT", function)
            } else {
                function.clone()
            }
        }
    }
}

fn describe_source(source: &ColumnSource) -> String {
    match source {
        ColumnSource::Column {
            table: Some(t),
            column,
        } => format!("{}.{}", t, column),
        ColumnSource::Column {
            table: None,
            column,
        } => format!("?.{}", column),
        ColumnSource::Literal { value } => format!("literal {}", value),
        ColumnSource::Variable { name } => format!("variable {}", name),
        ColumnSource::Dynamic => "dynamic SQL".to_string(),
    }
}

pub fn format_column_lineage_tree(
    node: &ColumnLineageNode,
    graph: &CodeGraph,
    direction: LineageDirection,
    indent: usize,
) -> String {
    let mut out = String::new();
    if indent == 0 {
        out.push_str(&format!("{}.{}\n", node.table, node.column));
    }
    let pad = "  ".repeat(indent + 1);
    let arrow = match direction {
        LineageDirection::Upstream => "\u{2190}",
        LineageDirection::Downstream => "\u{2192}",
    };

    // One mapping fans out into a step per source, all sharing its expression. Printing
    // that expression on every line buries the sources it is meant to explain, so it is
    // shown once per run and abbreviated when long.
    let mut last_expression: Option<&str> = None;
    for step in &node.steps {
        let via = crate::graph::key::NodeKey::from_node(&graph[step.via]);
        let mut line = format!(
            "{}{} {}  [{}]",
            pad,
            arrow,
            describe_source(&step.source),
            describe_kind(&step.kind)
        );
        match step.expression.as_deref() {
            Some(expr) if last_expression != Some(expr) => {
                line.push_str(&format!(" {}", abbreviate(expr, 96)));
                last_expression = Some(expr);
            }
            Some(_) => {}
            None => last_expression = None,
        }
        line.push_str(&format!("  via {}", via));
        out.push_str(&line);
        out.push('\n');

        if let Some(next) = &step.next {
            out.push_str(&format_column_lineage_tree(
                next,
                graph,
                direction,
                indent + 1,
            ));
        }
    }
    out
}

/// Shorten an expression for display, cutting on a char boundary so multi-byte text
/// (Chinese identifiers and string literals are common in this codebase) stays valid.
fn abbreviate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let head: String = text.chars().take(max_chars).collect();
    format!("{}…", head.trim_end())
}

pub fn format_column_lineage_json(
    node: &ColumnLineageNode,
    graph: &CodeGraph,
) -> serde_json::Value {
    let steps: Vec<serde_json::Value> = node
        .steps
        .iter()
        .map(|step| {
            serde_json::json!({
                "source": describe_source(&step.source),
                "kind": describe_kind(&step.kind),
                "expression": step.expression,
                "via": crate::graph::key::NodeKey::from_node(&graph[step.via]).to_string(),
                "next": step.next.as_ref().map(|n| format_column_lineage_json(n, graph)),
            })
        })
        .collect();

    serde_json::json!({
        "table": node.table,
        "column": node.column,
        "steps": steps,
    })
}

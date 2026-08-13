use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use petgraph::Direction;
use std::collections::HashSet;

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

/// Compute table-level lineage using depth-first traversal with per-path ancestors.
///
/// Algorithm:
/// - UPSTREAM: table → (Incoming Write TableAccess) → routine → (Outgoing Read TableAccess) → table → recurse
/// - DOWNSTREAM: table → (Incoming Read TableAccess) → routine → (Outgoing Write TableAccess) → table → recurse
/// - Views: follow DependsOn edges as if they were transparent
///
/// A per-path ancestor set (rather than one global `visited` set) keeps a table reached
/// through two different routines visible under both parents, while still cutting off a
/// cycle that returns to an ancestor.
pub fn lineage_table(
    graph: &CodeGraph,
    start_idx: NodeIndex,
    direction: LineageDirection,
    depth: usize,
) -> LineageNode {
    let mut ancestors: HashSet<NodeIndex> = HashSet::new();
    ancestors.insert(start_idx);
    build_table_lineage(graph, start_idx, direction, depth, &mut ancestors)
}

fn build_table_lineage(
    graph: &CodeGraph,
    idx: NodeIndex,
    direction: LineageDirection,
    depth: usize,
    ancestors: &mut HashSet<NodeIndex>,
) -> LineageNode {
    let mut steps: Vec<LineageStep> = Vec::new();
    let mut children: Vec<LineageNode> = Vec::new();
    let mut child_seen: HashSet<NodeIndex> = HashSet::new();

    if depth == 0 {
        return LineageNode {
            idx,
            _depth: 0,
            steps,
            children,
        };
    }

    let wanted = match direction {
        LineageDirection::Upstream => AccessMode::Write,
        LineageDirection::Downstream => AccessMode::Read,
    };
    let hop = match direction {
        LineageDirection::Upstream => AccessMode::Read,
        LineageDirection::Downstream => AccessMode::Write,
    };

    let mut routines: Vec<(NodeIndex, AccessMode)> = Vec::new();
    for edge_ref in graph.edges_directed(idx, Direction::Incoming) {
        if let Edge::TableAccess { modes, .. } = edge_ref.weight() {
            if modes.contains(wanted) {
                routines.push((edge_ref.source(), *modes));
            }
        }
    }
    routines.sort_by_key(|(r, _)| r.index());
    routines.dedup_by_key(|(r, _)| *r);

    for (routine_idx, routine_modes) in routines {
        let mut hop_targets: Vec<NodeIndex> = Vec::new();
        for edge_ref in graph.edges_directed(routine_idx, Direction::Outgoing) {
            if let Edge::TableAccess { modes, .. } = edge_ref.weight() {
                if modes.contains(hop) {
                    hop_targets.push(edge_ref.target());
                }
            }
        }
        if hop_targets.is_empty() {
            // The routine writes (upstream) or reads (downstream) this table but touches
            // no counterpart table — INSERT ... VALUES, a bare DELETE, or a SELECT-only
            // read. Record it so the write/read is not silently invisible.
            steps.push(LineageStep {
                routine_idx,
                modes: routine_modes,
            });
        } else {
            for target in hop_targets {
                steps.push(LineageStep {
                    routine_idx,
                    modes: routine_modes,
                });
                if child_seen.insert(target) && !ancestors.contains(&target) {
                    ancestors.insert(target);
                    children.push(build_table_lineage(
                        graph,
                        target,
                        direction,
                        depth - 1,
                        ancestors,
                    ));
                    ancestors.remove(&target);
                }
            }
        }
    }

    let depends_dir = match direction {
        LineageDirection::Upstream => Direction::Outgoing,
        LineageDirection::Downstream => Direction::Incoming,
    };
    for edge_ref in graph.edges_directed(idx, depends_dir) {
        if matches!(edge_ref.weight(), Edge::DependsOn { .. }) {
            let other = match direction {
                LineageDirection::Upstream => edge_ref.target(),
                LineageDirection::Downstream => edge_ref.source(),
            };
            let step_routine = match direction {
                LineageDirection::Upstream => idx,
                LineageDirection::Downstream => other,
            };
            steps.push(LineageStep {
                routine_idx: step_routine,
                modes: AccessMode::Read,
            });
            if child_seen.insert(other) && !ancestors.contains(&other) {
                ancestors.insert(other);
                children.push(build_table_lineage(
                    graph,
                    other,
                    direction,
                    depth - 1,
                    ancestors,
                ));
                ancestors.remove(&other);
            }
        }
    }

    // Deduplicate steps by (routine, modes): a routine that both writes and reads a
    // table, or a diamond reaching the same child twice, would otherwise repeat it.
    let mut seen: HashSet<(usize, u8)> = HashSet::new();
    steps.retain(|s| {
        let key = (s.routine_idx.index(), s.modes.bits());
        seen.insert(key)
    });

    LineageNode {
        idx,
        _depth: 0,
        steps,
        children,
    }
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
    let mut seen: HashSet<ColumnMapping> = HashSet::new();
    for dir in [Direction::Outgoing, Direction::Incoming] {
        for edge_ref in graph.edges_directed(routine, dir) {
            if let Edge::TableAccess {
                column_analysis: Some(analysis),
                ..
            } = edge_ref.weight()
            {
                for m in &analysis.column_mappings {
                    if seen.insert(m.clone()) {
                        out.push(m.clone());
                    }
                }
            }
        }
    }
    out
}

/// Find the Table or View node named `name`.
///
/// A schema-qualified name (`schema.table`) matches schema and table together. A bare
/// name resolves only when a single node carries it — with two schemas each holding a
/// table of the same name, returning the first would report one schema's pipeline as the
/// other's, so an ambiguous bare name resolves to `None`.
fn find_table_node(graph: &CodeGraph, name: &str) -> Option<NodeIndex> {
    let name_of = |idx: &NodeIndex| match &graph[*idx] {
        crate::graph::Node::Table { schema, name, .. }
        | crate::graph::Node::View { schema, name, .. } => Some((schema.as_deref(), name.as_str())),
        _ => None,
    };

    if let Some((schema, table)) = name.rsplit_once('.') {
        if !schema.is_empty() {
            return graph.node_indices().find(|idx| {
                name_of(idx)
                    .is_some_and(|(s, n)| n.eq_ignore_ascii_case(table) && s == Some(schema))
            });
        }
    }

    let mut matches = graph
        .node_indices()
        .filter(|idx| name_of(idx).is_some_and(|(_, n)| n.eq_ignore_ascii_case(name)));
    let first = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(first)
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

    // Views define their columns through a body, not a routine: the mappings live on
    // DependsOn edges (view → base table). Upstream reads the view's own outgoing edges,
    // downstream reads the incoming edges of a base table the view selects from.
    match direction {
        LineageDirection::Upstream => {
            for edge_ref in graph.edges_directed(table_idx, Direction::Outgoing) {
                if let Edge::DependsOn {
                    column_analysis: Some(analysis),
                    ..
                } = edge_ref.weight()
                {
                    for mapping in &analysis.column_mappings {
                        let targets_this = mapping
                            .target_table
                            .as_deref()
                            .is_some_and(|t| eq(t, table))
                            && eq(&mapping.target_column, column);
                        if !targets_this {
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
                                via: table_idx,
                                kind: mapping.kind.clone(),
                                expression: mapping.expression.clone(),
                                source: source.clone(),
                                next,
                            });
                        }
                    }
                }
            }
        }
        LineageDirection::Downstream => {
            for edge_ref in graph.edges_directed(table_idx, Direction::Incoming) {
                if let Edge::DependsOn {
                    column_analysis: Some(analysis),
                    ..
                } = edge_ref.weight()
                {
                    let view_idx = edge_ref.source();
                    for mapping in &analysis.column_mappings {
                        let consumes_this = mapping.sources.iter().any(|s| match s {
                            ColumnSource::Column {
                                table: Some(t),
                                column: c,
                            } => eq(t, table) && eq(c, column),
                            _ => false,
                        });
                        if !consumes_this {
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
                            via: view_idx,
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

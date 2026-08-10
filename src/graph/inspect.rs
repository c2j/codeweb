use petgraph::graph::NodeIndex;
use std::collections::{BTreeMap, HashSet};

use crate::graph::query::filter::EdgeFilter;
use crate::graph::query::traversal::GraphTraversal;
use crate::graph::{node_display_name, node_type_tag, CodeGraph};

pub struct InspectOptions {
    pub max_depth: usize,
    pub max_paths_per_pair: usize,
    pub max_total_paths: usize,
}

impl Default for InspectOptions {
    fn default() -> Self {
        Self {
            max_depth: 15,
            max_paths_per_pair: 10,
            max_total_paths: 100,
        }
    }
}

pub struct InspectPath {
    pub from: NodeIndex,
    pub to: NodeIndex,
    pub hops: Vec<NodeIndex>,
    pub edge_labels: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InspectStyle {
    Summary,
    Paths,
    Both,
    /// Reverse convergence tree: roots are destination nodes,
    /// children are callers. Edges marked with `←`.
    Tree,
}

pub struct InspectResult {
    pub summary: Vec<(NodeIndex, NodeIndex, usize)>,
    pub paths: Vec<InspectPath>,
}

pub fn find_paths_between(
    graph: &CodeGraph,
    targets: &[NodeIndex],
    opts: &InspectOptions,
) -> InspectResult {
    let mut all_paths: Vec<InspectPath> = Vec::new();

    for i in 0..targets.len() {
        for j in i + 1..targets.len() {
            let from = targets[i];
            let to = targets[j];

            let forward =
                collect_paths_between(graph, from, to, opts.max_depth, opts.max_paths_per_pair);
            let backward =
                collect_paths_between(graph, to, from, opts.max_depth, opts.max_paths_per_pair);

            all_paths.extend(forward);
            all_paths.extend(backward);
        }
    }

    // Sort by shortest first
    all_paths.sort_by_key(|p| p.hops.len());

    let mut pair_counts: std::collections::HashMap<(NodeIndex, NodeIndex), usize> =
        std::collections::HashMap::new();
    all_paths.retain(|p| {
        let count = pair_counts.entry((p.from, p.to)).or_insert(0);
        *count += 1;
        *count <= opts.max_paths_per_pair
    });

    if all_paths.len() > opts.max_total_paths {
        all_paths.truncate(opts.max_total_paths);
    }

    // Build summary: for each (from, to) pair, count paths
    let mut summary_map: std::collections::HashMap<(NodeIndex, NodeIndex), usize> =
        std::collections::HashMap::new();
    for i in 0..targets.len() {
        for j in 0..targets.len() {
            if i != j {
                summary_map.insert((targets[i], targets[j]), 0);
            }
        }
    }
    for p in &all_paths {
        *summary_map.entry((p.from, p.to)).or_insert(0) += 1;
    }
    let mut summary: Vec<(NodeIndex, NodeIndex, usize)> = summary_map
        .into_iter()
        .map(|((from, to), count)| (from, to, count))
        .collect();
    summary.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

    InspectResult {
        summary,
        paths: all_paths,
    }
}

fn collect_paths_between(
    graph: &CodeGraph,
    from: NodeIndex,
    to: NodeIndex,
    max_depth: usize,
    max_paths: usize,
) -> Vec<InspectPath> {
    use petgraph::algo::has_path_connecting;

    if !has_path_connecting(graph, from, to, None) {
        return vec![];
    }

    let raw_paths = GraphTraversal::new(graph, from)
        .outgoing()
        .edge_filter(EdgeFilter::new())
        .max_depth(max_depth)
        .max_paths(max_paths)
        .target(to)
        .collect_paths_to_target();

    let mut seen: HashSet<Vec<NodeIndex>> = HashSet::new();
    let mut paths: Vec<InspectPath> = Vec::new();

    for hops in raw_paths {
        if !seen.insert(hops.clone()) {
            continue;
        }
        let edge_labels: Vec<String> = hops
            .windows(2)
            .map(|w| {
                crate::graph::traverse::edge_label_for(graph, w[0], w[1])
                    .map(|s| s.to_string())
                    .unwrap_or_default()
            })
            .collect();
        paths.push(InspectPath {
            from,
            to,
            hops,
            edge_labels,
        });
    }

    paths
}

/// Format a single path as a tree chain using `detail`-compatible box-drawing characters.
///
/// The first node in `hops` is the source (shown without connector),
/// each subsequent hop is shown with `└──` and its edge label.
fn format_path_chain(graph: &CodeGraph, path: &InspectPath, lines: &mut Vec<String>) {
    let hops = &path.hops;
    let labels = &path.edge_labels;

    for (hi, hop) in hops.iter().enumerate() {
        let name = node_display_name(&graph[*hop]);
        if hi == 0 {
            // Source node
            lines.push(format!("    {}", name));
        } else {
            let label = if hi - 1 < labels.len() {
                format!(" {}", labels[hi - 1])
            } else {
                String::new()
            };
            let prefix = "    ";
            lines.push(format!("{}└── {}{}", prefix, name, label));
        }
    }
}

/// Format PATHS section using detail-compatible tree style.
/// Groups paths by (from, to) and renders each path as an indented chain.
fn format_paths_tree(result: &InspectResult, graph: &CodeGraph) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();

    // Group paths by (from, to) pair
    let mut path_groups: BTreeMap<(NodeIndex, NodeIndex), Vec<&InspectPath>> = BTreeMap::new();
    for p in &result.paths {
        path_groups.entry((p.from, p.to)).or_default().push(p);
    }

    for ((from, to), paths) in &path_groups {
        let from_name = node_display_name(&graph[*from]);
        let to_name = node_display_name(&graph[*to]);
        lines.push(String::new());
        lines.push(format!(
            "── {} → {} ({} path{}) ──",
            from_name,
            to_name,
            paths.len(),
            if paths.len() == 1 { "" } else { "s" }
        ));

        for (pi, path) in paths.iter().enumerate() {
            let hops_count = path.hops.len() - 1;
            if paths.len() > 1 {
                lines.push(format!(
                    "  Path {}/{} ({} hop{}):",
                    pi + 1,
                    paths.len(),
                    hops_count,
                    if hops_count == 1 { "" } else { "s" }
                ));
            }
            format_path_chain(graph, path, &mut lines);
        }
    }

    lines
}

/// Build a reverse adjacency map from paths.
/// Key = callee, Value = list of (caller, edge_label) pairs.
/// Deduplicates edges that appear in multiple paths.
fn build_reverse_adjacency(
    graph: &CodeGraph,
    paths: &[&InspectPath],
) -> BTreeMap<NodeIndex, Vec<(NodeIndex, String)>> {
    let mut adj: BTreeMap<NodeIndex, Vec<(NodeIndex, String)>> = BTreeMap::new();

    for path in paths {
        let hops = &path.hops;
        let labels = &path.edge_labels;
        for i in 0..hops.len().saturating_sub(1) {
            let caller = hops[i];
            let callee = hops[i + 1];
            let label = if i < labels.len() {
                labels[i].clone()
            } else {
                String::new()
            };
            let entry = adj.entry(callee).or_default();
            if !entry.iter().any(|(n, _)| *n == caller) {
                entry.push((caller, label));
            }
        }
    }

    for children in adj.values_mut() {
        children.sort_by(|(a, _), (b, _)| {
            node_display_name(&graph[*a]).cmp(&node_display_name(&graph[*b]))
        });
    }

    adj
}

/// Recursively render a child node and its subtree in the reverse convergence tree.
/// `edge_label` describes the call from this child to its parent.
/// `visited` prevents infinite recursion on cyclic reverse adjacencies.
#[allow(clippy::too_many_arguments)]
fn render_reverse_child(
    graph: &CodeGraph,
    adj: &BTreeMap<NodeIndex, Vec<(NodeIndex, String)>>,
    node: NodeIndex,
    edge_label: &str,
    prefix: &str,
    is_last: bool,
    visited: &mut HashSet<NodeIndex>,
    lines: &mut Vec<String>,
) {
    if !visited.insert(node) {
        // Cycle detected: show the node name but don't recurse
        let name = node_display_name(&graph[node]);
        let connector = if is_last { "└── " } else { "├── " };
        lines.push(format!("{}{}{}  (cycle)", prefix, connector, name));
        return;
    }

    let name = node_display_name(&graph[node]);
    let child_count = adj.get(&node).map(|c| c.len()).unwrap_or(0);

    let connector = if is_last { "└── " } else { "├── " };
    let label_str = if edge_label.is_empty() {
        String::new()
    } else {
        format!("  ← {}", edge_label)
    };
    let caller_info = if child_count > 0 {
        format!("  (called by {})", child_count)
    } else {
        String::new()
    };

    lines.push(format!(
        "{}{}{}{}{}",
        prefix, connector, name, label_str, caller_info
    ));

    let extension = if is_last { "    " } else { "│   " };
    let new_prefix = format!("{}{}", prefix, extension);

    if let Some(grandchildren) = adj.get(&node) {
        let count = grandchildren.len();
        for (gi, (gc, gc_label)) in grandchildren.iter().enumerate() {
            render_reverse_child(
                graph,
                adj,
                *gc,
                gc_label,
                &new_prefix,
                gi == count - 1,
                visited,
                lines,
            );
        }
    }
}

/// Format PATHS as reverse convergence trees.
/// Groups paths by destination (to), then builds a tree with
/// the destination as root and all callers as children.
fn format_paths_reverse_tree(result: &InspectResult, graph: &CodeGraph) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();

    lines.push(
        "(direction: children are callers of parent; ← = called by; [intra]/[cross]/[external] = call scope; [calls]/[invokes]/[dynamic]/[builtin]/... = edge type)"
            .to_string(),
    );

    // Group paths by destination (to)
    let mut by_dest: BTreeMap<NodeIndex, Vec<&InspectPath>> = BTreeMap::new();
    for p in &result.paths {
        by_dest.entry(p.to).or_default().push(p);
    }

    for (root, paths) in &by_dest {
        let root_name = node_display_name(&graph[*root]);
        let mut direct_callers: HashSet<NodeIndex> = HashSet::new();
        for p in paths {
            if p.hops.len() >= 2 {
                direct_callers.insert(p.hops[p.hops.len() - 2]);
            }
        }
        let caller_count = direct_callers.len();

        lines.push(String::new());
        lines.push(format!(
            "── {} (root, called by {}) ──",
            root_name, caller_count
        ));

        let adj = build_reverse_adjacency(graph, paths);

        if let Some(children) = adj.get(root) {
            let count = children.len();
            let mut visited: HashSet<NodeIndex> = HashSet::new();
            visited.insert(*root);
            for (ci, (child, label)) in children.iter().enumerate() {
                render_reverse_child(
                    graph,
                    &adj,
                    *child,
                    label,
                    "    ",
                    ci == count - 1,
                    &mut visited,
                    &mut lines,
                );
            }
        }
    }

    lines
}

pub fn format_inspect_result(
    result: &InspectResult,
    graph: &CodeGraph,
    target_names: &[String],
    style: InspectStyle,
    show_unreachable: bool,
) -> String {
    let mut lines = Vec::new();

    // ── TARGET NODES ──
    lines.push("── TARGET NODES ──".to_string());
    let mut referenced: HashSet<NodeIndex> = HashSet::new();
    for &(from, to, _) in &result.summary {
        referenced.insert(from);
        referenced.insert(to);
    }
    let mut ref_sorted: Vec<NodeIndex> = referenced.into_iter().collect();
    ref_sorted.sort();
    for idx in &ref_sorted {
        let tag = node_type_tag(&graph[*idx]);
        let name = node_display_name(&graph[*idx]);
        // Find original input name
        let input_name = target_names
            .iter()
            .find(|n| {
                let lower_n = n.to_lowercase();
                let lower_disp = name.to_lowercase();
                lower_disp.contains(&lower_n)
            })
            .cloned()
            .unwrap_or_else(|| name.clone());
        lines.push(format!(
            "  {}  {}  (matched: \"{}\")",
            tag, name, input_name
        ));
    }
    lines.push(String::new());

    if style != InspectStyle::Paths {
        // ── CONNECTIONS ──
        lines.push("── CONNECTIONS ──".to_string());
        let has_any_path = result.summary.iter().any(|(_, _, c)| *c > 0);
        if !has_any_path && !show_unreachable {
            lines.push("  (no paths found between any pair)".to_string());
        } else if has_any_path {
            // Group by from
            let mut groups: std::collections::BTreeMap<
                NodeIndex,
                Vec<&(NodeIndex, NodeIndex, usize)>,
            > = std::collections::BTreeMap::new();
            for entry in &result.summary {
                groups.entry(entry.0).or_default().push(entry);
            }
            for (from, entries) in &groups {
                let from_name = node_display_name(&graph[*from]);
                for (_, to, count) in entries {
                    if !show_unreachable && *count == 0 {
                        continue;
                    }
                    let to_name = node_display_name(&graph[*to]);
                    let shortest = result
                        .paths
                        .iter()
                        .filter(|p| p.from == *from && p.to == *to)
                        .map(|p| p.hops.len() - 1)
                        .min()
                        .unwrap_or(0);
                    if *count == 0 {
                        lines.push(format!(
                            "  {} → {} : 0 paths (unreachable)",
                            from_name, to_name
                        ));
                    } else {
                        lines.push(format!(
                            "  {} → {} : {} path(s)  (shortest {} hop{})",
                            from_name,
                            to_name,
                            count,
                            shortest,
                            if shortest == 1 { "" } else { "s" }
                        ));
                    }
                }
            }
        }
        lines.push(String::new());

        // Show unreachable pairs (only when explicitly requested)
        if show_unreachable {
            let mut unreachable: Vec<String> = Vec::new();
            for (from, to, count) in &result.summary {
                if *count == 0 {
                    let from_name = node_display_name(&graph[*from]);
                    let to_name = node_display_name(&graph[*to]);
                    unreachable.push(format!("  {} → {}", from_name, to_name));
                }
            }
            if !unreachable.is_empty() {
                lines.push("── UNREACHABLE ──".to_string());
                lines.extend(unreachable);
                lines.push(String::new());
            }
        }
    }

    if style != InspectStyle::Summary && !result.paths.is_empty() {
        lines.push("── PATHS ──".to_string());
        if style == InspectStyle::Tree {
            lines.extend(format_paths_reverse_tree(result, graph));
        } else {
            lines.extend(format_paths_tree(result, graph));
        }
    }

    // ── SUMMARY ──
    if !result.paths.is_empty() || show_unreachable {
        lines.push(String::new());
        lines.push("── SUMMARY ──".to_string());
        for &(from, to, count) in &result.summary {
            let from_name = node_display_name(&graph[from]);
            let to_name = node_display_name(&graph[to]);
            if count > 0 {
                let shortest = result
                    .paths
                    .iter()
                    .filter(|p| p.from == from && p.to == to)
                    .map(|p| p.hops.len() - 1)
                    .min()
                    .unwrap_or(0);
                lines.push(format!(
                    "  ✅ {} → {} : reachable ({} hop{})",
                    from_name,
                    to_name,
                    shortest,
                    if shortest == 1 { "" } else { "s" }
                ));
            } else if show_unreachable || result.paths.is_empty() {
                lines.push(format!("  ❌ {} → {} : unreachable", from_name, to_name));
            }
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn make_file() -> Arc<PathBuf> {
        Arc::new(PathBuf::from("test.sql"))
    }

    fn make_proc(name: &str, schema: Option<&str>) -> Node {
        Node::Procedure {
            id: RoutineId {
                schema: schema.map(String::from),
                package: None,
                name: name.to_string(),
                kind: RoutineKind::Procedure,
            },
            location: SourceLocation {
                file: make_file(),
                line: 0,
            },
            partial: false,
            body_sql: Vec::new(),
        }
    }

    fn make_direct_call(graph: &mut CodeGraph, from: NodeIndex, to: NodeIndex) {
        graph.add_edge(
            from,
            to,
            Edge::DirectCall {
                scope: CallScope::CrossPackage,
                location: SourceLocation {
                    file: make_file(),
                    line: 1,
                },
            },
        );
    }

    #[test]
    fn find_path_between_two_nodes() {
        let mut graph = CodeGraph::new();
        let a = graph.add_node(make_proc("proc_a", Some("public")));
        let b = graph.add_node(make_proc("proc_b", Some("public")));
        make_direct_call(&mut graph, a, b);

        let result = find_paths_between(&graph, &[a, b], &InspectOptions::default());

        // A → B should have 1 path, B → A should have 0
        let ab_entry = result
            .summary
            .iter()
            .find(|(f, t, _)| *f == a && *t == b)
            .unwrap();
        let ba_entry = result
            .summary
            .iter()
            .find(|(f, t, _)| *f == b && *t == a)
            .unwrap();
        assert_eq!(ab_entry.2, 1, "A→B should have 1 path");
        assert_eq!(ba_entry.2, 0, "B→A should have 0 paths");
        assert_eq!(result.paths.len(), 1);
        assert_eq!(result.paths[0].from, a);
        assert_eq!(result.paths[0].to, b);
        assert_eq!(result.paths[0].hops.len(), 2); // a → b
    }

    #[test]
    fn find_path_with_intermediate_node() {
        let mut graph = CodeGraph::new();
        let a = graph.add_node(make_proc("a", None));
        let x = graph.add_node(make_proc("x", None));
        let b = graph.add_node(make_proc("b", None));
        make_direct_call(&mut graph, a, x);
        make_direct_call(&mut graph, x, b);

        let result = find_paths_between(&graph, &[a, b], &InspectOptions::default());

        assert_eq!(result.paths.len(), 1);
        assert_eq!(result.paths[0].hops, vec![a, x, b]);
    }

    #[test]
    fn unreachable_nodes_yield_zero_paths() {
        let mut graph = CodeGraph::new();
        let a = graph.add_node(make_proc("a", None));
        let b = graph.add_node(make_proc("b", None));
        // a and b disconnected

        let result = find_paths_between(&graph, &[a, b], &InspectOptions::default());

        assert!(result.paths.is_empty());
        assert!(result.summary.iter().all(|(_, _, c)| *c == 0));
    }

    #[test]
    fn bidirectional_finds_both_directions() {
        let mut graph = CodeGraph::new();
        let a = graph.add_node(make_proc("a", None));
        let b = graph.add_node(make_proc("b", None));
        make_direct_call(&mut graph, a, b); // a → b

        let result = find_paths_between(&graph, &[a, b], &InspectOptions::default());

        // Should find a → b (forward) but not b → a (backward, no edge)
        let has_ab = result.paths.iter().any(|p| p.from == a && p.to == b);
        let has_ba = result.paths.iter().any(|p| p.from == b && p.to == a);
        assert!(has_ab, "should find a→b");
        assert!(!has_ba, "should not find b→a (no reverse edge)");
    }

    #[test]
    fn multiple_target_nodes() {
        let mut graph = CodeGraph::new();
        let a = graph.add_node(make_proc("a", None));
        let b = graph.add_node(make_proc("b", None));
        let c = graph.add_node(make_proc("c", None));
        make_direct_call(&mut graph, a, b);
        make_direct_call(&mut graph, b, c);

        let result = find_paths_between(&graph, &[a, b, c], &InspectOptions::default());

        // a → b: 1 path
        // b → c: 1 path
        // a → c: a→b→c: 1 path (forward from pair a,c)
        // Total 3 paths
        assert_eq!(result.paths.len(), 3, "should find a→b, b→c, a→c");
    }

    #[test]
    fn max_depth_limits_paths() {
        let mut graph = CodeGraph::new();
        let a = graph.add_node(make_proc("a", None));
        let x = graph.add_node(make_proc("x", None));
        let y = graph.add_node(make_proc("y", None));
        let b = graph.add_node(make_proc("b", None));
        make_direct_call(&mut graph, a, x);
        make_direct_call(&mut graph, x, y);
        make_direct_call(&mut graph, y, b);

        let opts = InspectOptions {
            max_depth: 2,
            ..InspectOptions::default()
        };
        let result = find_paths_between(&graph, &[a, b], &opts);

        // a→x→y→b is 3 hops, max_depth=2 should not reach b
        assert!(result.paths.is_empty());
    }

    #[test]
    fn deduplicates_identical_paths() {
        let mut graph = CodeGraph::new();
        let a = graph.add_node(make_proc("a", None));
        let b = graph.add_node(make_proc("b", None));
        // Two parallel edges a→b (e.g., DirectCall + CallsProcedure)
        make_direct_call(&mut graph, a, b);
        graph.add_edge(
            a,
            b,
            Edge::CallsProcedure {
                location: SourceLocation {
                    file: make_file(),
                    line: 1,
                },
            },
        );

        let result = find_paths_between(&graph, &[a, b], &InspectOptions::default());

        // Both edges should result in the same path a→b, dedup'd to 1
        assert_eq!(result.paths.len(), 1);
    }

    #[test]
    fn format_inspect_result_output() {
        let mut graph = CodeGraph::new();
        let a = graph.add_node(make_proc("proc_a", Some("public")));
        let b = graph.add_node(make_proc("proc_b", Some("public")));
        make_direct_call(&mut graph, a, b);

        let result = find_paths_between(&graph, &[a, b], &InspectOptions::default());
        let output = format_inspect_result(
            &result,
            &graph,
            &["proc_a".into(), "proc_b".into()],
            InspectStyle::Both,
            false, // show_unreachable
        );

        assert!(output.contains("── CONNECTIONS ──"));
        assert!(output.contains("proc_a"));
        assert!(output.contains("proc_b"));
        assert!(output.contains("── PATHS ──"));
    }

    #[test]
    fn format_inspect_result_hides_unreachable_by_default() {
        let mut graph = CodeGraph::new();
        let a = graph.add_node(make_proc("proc_a", Some("public")));
        let b = graph.add_node(make_proc("proc_b", Some("public")));
        make_direct_call(&mut graph, a, b);

        let result = find_paths_between(&graph, &[a, b], &InspectOptions::default());

        // Default (show_unreachable=false): no UNREACHABLE section, no 0-path lines
        let output_default = format_inspect_result(
            &result,
            &graph,
            &["proc_a".into(), "proc_b".into()],
            InspectStyle::Both,
            false,
        );
        assert!(!output_default.contains("UNREACHABLE"));
        assert!(!output_default.contains("0 paths (unreachable)"));

        // show_unreachable=true: should have UNREACHABLE section with the B→A pair
        let output_all = format_inspect_result(
            &result,
            &graph,
            &["proc_a".into(), "proc_b".into()],
            InspectStyle::Both,
            true,
        );
        assert!(output_all.contains("── UNREACHABLE ──"));
        assert!(output_all.contains("0 paths (unreachable)"));
    }

    #[test]
    fn format_inspect_result_tree_paths_uses_box_drawing() {
        let mut graph = CodeGraph::new();
        let a = graph.add_node(make_proc("proc_a", Some("public")));
        let b = graph.add_node(make_proc("proc_b", Some("public")));
        make_direct_call(&mut graph, a, b);

        let result = find_paths_between(&graph, &[a, b], &InspectOptions::default());
        let output = format_inspect_result(
            &result,
            &graph,
            &["proc_a".into(), "proc_b".into()],
            InspectStyle::Both,
            false,
        );

        // PATHS section body should use └── connector (detail-compatible), not the old → prefix
        let paths_section = &output[output.find("── PATHS ──").unwrap()..];
        assert!(
            paths_section.contains("└──"),
            "PATHS body should use box-drawing characters"
        );
        // Old format had "    → proc:name", new format has "    └── proc:name"
        assert!(
            !paths_section.contains("    \u{2192} "),
            "PATHS body should not use old arrow connectors: {}",
            paths_section
        );
    }

    #[test]
    fn format_inspect_result_multi_target_tree() {
        let mut graph = CodeGraph::new();
        let a = graph.add_node(make_proc("root", None));
        let b = graph.add_node(make_proc("child1", None));
        let c = graph.add_node(make_proc("child2", None));
        make_direct_call(&mut graph, a, b);
        make_direct_call(&mut graph, a, c);

        let result = find_paths_between(&graph, &[a, b, c], &InspectOptions::default());
        let output = format_inspect_result(
            &result,
            &graph,
            &["root".into(), "child1".into(), "child2".into()],
            InspectStyle::Both,
            false,
        );

        // CONNECTIONS should show only the 2 reachable pairs, not the 4 0-path ones
        assert!(output.contains("proc:root → proc:child1"));
        assert!(output.contains("proc:root → proc:child2"));
        assert!(!output.contains("0 paths"), "default: no 0-path lines");
        assert!(
            !output.contains("UNREACHABLE"),
            "default: no UNREACHABLE section"
        );
    }

    // ────────────────────────────────────────────────
    // Regression: forked DAG topology (two branches converging on c)
    //   a1 → b1 → c
    //   a2 → b2 → c
    //
    // inspect a1 a2       → 0 paths (a1 and a2 are not reachable from each other)
    // inspect a1 a2 c     → 2 paths (a1→c via b1, a2→c via b2)
    // ────────────────────────────────────────────────

    /// Given `a1 → b1 → c ← b2 ← a2`, calling `find_paths_between(&[a1, a2])`
    /// should return zero paths — a1 and a2 are not directly reachable.
    #[test]
    fn forked_dag_targets_only_leaves_zero_paths() {
        let mut graph = CodeGraph::new();
        let a1 = graph.add_node(make_proc("a1", None));
        let b1 = graph.add_node(make_proc("b1", None));
        let c = graph.add_node(make_proc("c", None));
        let b2 = graph.add_node(make_proc("b2", None));
        let a2 = graph.add_node(make_proc("a2", None));

        // a1 → b1 → c
        make_direct_call(&mut graph, a1, b1);
        make_direct_call(&mut graph, b1, c);
        // a2 → b2 → c
        make_direct_call(&mut graph, a2, b2);
        make_direct_call(&mut graph, b2, c);

        let result = find_paths_between(&graph, &[a1, a2], &InspectOptions::default());

        // No directed path exists between a1 and a2 in either direction.
        assert!(
            result.paths.is_empty(),
            "a1 and a2 should have 0 paths between them"
        );
        // All summary entries should be zero.
        for (_, _, count) in &result.summary {
            assert_eq!(*count, 0, "every pair summary should be 0");
        }
    }

    /// Same graph (`a1 → b1 → c ← b2 ← a2`), but with c included as a target.
    /// Should find a1→c (via b1) and a2→c (via b2).
    #[test]
    fn forked_dag_targets_include_convergence_finds_paths() {
        let mut graph = CodeGraph::new();
        let a1 = graph.add_node(make_proc("a1", None));
        let b1 = graph.add_node(make_proc("b1", None));
        let c = graph.add_node(make_proc("c", None));
        let b2 = graph.add_node(make_proc("b2", None));
        let a2 = graph.add_node(make_proc("a2", None));

        make_direct_call(&mut graph, a1, b1);
        make_direct_call(&mut graph, b1, c);
        make_direct_call(&mut graph, a2, b2);
        make_direct_call(&mut graph, b2, c);

        let result = find_paths_between(&graph, &[a1, a2, c], &InspectOptions::default());

        // Should find exactly 2 paths: a1→c and a2→c
        assert_eq!(result.paths.len(), 2, "should find a1→c and a2→c");

        // Verify a1 → b1 → c
        let a1_to_c = result
            .paths
            .iter()
            .find(|p| p.from == a1 && p.to == c)
            .expect("should have a1→c path");
        assert_eq!(a1_to_c.hops, vec![a1, b1, c], "a1→c path should go via b1");

        // Verify a2 → b2 → c
        let a2_to_c = result
            .paths
            .iter()
            .find(|p| p.from == a2 && p.to == c)
            .expect("should have a2→c path");
        assert_eq!(a2_to_c.hops, vec![a2, b2, c], "a2→c path should go via b2");

        // Summary should show a1→c count=1, a2→c count=1
        let a1c_summary = result
            .summary
            .iter()
            .find(|(f, t, _)| *f == a1 && *t == c)
            .unwrap();
        assert_eq!(a1c_summary.2, 1);
        let a2c_summary = result
            .summary
            .iter()
            .find(|(f, t, _)| *f == a2 && *t == c)
            .unwrap();
        assert_eq!(a2c_summary.2, 1);
    }

    #[test]
    fn format_reverse_tree_forked_dag() {
        let mut graph = CodeGraph::new();
        let a1 = graph.add_node(make_proc("a1", None));
        let b1 = graph.add_node(make_proc("b1", None));
        let c = graph.add_node(make_proc("c", None));
        let b2 = graph.add_node(make_proc("b2", None));
        let a2 = graph.add_node(make_proc("a2", None));

        make_direct_call(&mut graph, a1, b1);
        make_direct_call(&mut graph, b1, c);
        make_direct_call(&mut graph, a2, b2);
        make_direct_call(&mut graph, b2, c);

        let result = find_paths_between(&graph, &[a1, a2, c], &InspectOptions::default());
        let output = format_inspect_result(
            &result,
            &graph,
            &["a1".into(), "a2".into(), "c".into()],
            InspectStyle::Tree,
            false,
        );

        assert!(output.contains("── proc:c (root, called by 2) ──"));
        assert!(output.contains("├── proc:b1"));
        assert!(output.contains("← [cross]"));
        assert!(output.contains("(called by 1)"));
        assert!(output.contains("└── proc:b2"));
        assert!(output.contains("── PATHS ──"));
        assert!(output.contains("── CONNECTIONS ──"));
        // a1 and a2 are leaf nodes — no (called by) annotation
        let paths_section = &output[output.find("── PATHS ──").unwrap()..];
        let a1_line = paths_section
            .lines()
            .find(|l| l.contains("proc:a1"))
            .unwrap();
        assert!(!a1_line.contains("(called by)"), "a1 is leaf: no called-by");
        let a2_line = paths_section
            .lines()
            .find(|l| l.contains("proc:a2"))
            .unwrap();
        assert!(!a2_line.contains("(called by)"), "a2 is leaf: no called-by");
    }

    #[test]
    fn format_reverse_tree_shared_intermediate_merged() {
        let mut graph = CodeGraph::new();
        let a1 = graph.add_node(make_proc("a1", None));
        let a2 = graph.add_node(make_proc("a2", None));
        let b = graph.add_node(make_proc("b", None));
        let c = graph.add_node(make_proc("c", None));

        make_direct_call(&mut graph, a1, b);
        make_direct_call(&mut graph, a2, b);
        make_direct_call(&mut graph, b, c);

        let result = find_paths_between(&graph, &[a1, a2, c], &InspectOptions::default());
        let output = format_inspect_result(
            &result,
            &graph,
            &["a1".into(), "a2".into(), "c".into()],
            InspectStyle::Tree,
            false,
        );

        assert!(output.contains("── proc:c (root, called by 1) ──"));
        assert!(output.contains("proc:b  ← [cross]  (called by 2)"));
        // b should appear exactly once (merged)
        assert_eq!(
            output.matches("proc:b").count(),
            1,
            "b should appear exactly once in output"
        );
        assert!(output.contains("├── proc:a1"));
        assert!(output.contains("└── proc:a2"));
    }

    #[test]
    fn format_reverse_tree_no_paths_empty() {
        let mut graph = CodeGraph::new();
        let a = graph.add_node(make_proc("a", None));
        let b = graph.add_node(make_proc("b", None));

        let result = find_paths_between(&graph, &[a, b], &InspectOptions::default());
        let output = format_inspect_result(
            &result,
            &graph,
            &["a".into(), "b".into()],
            InspectStyle::Tree,
            false,
        );

        assert!(!output.contains("── PATHS ──"));
        assert!(output.contains("(no paths found between any pair)"));
    }

    #[test]
    fn format_reverse_tree_cyclic_graph_does_not_overflow() {
        // A↔B mutual recursion, both call C
        // Paths: A→C, A→B→C, B→C, B→A→C
        // Reverse adjacency merges to: C←{A,B}, A←{B}, B←{A} → cycle A↔B
        let mut graph = CodeGraph::new();
        let a = graph.add_node(make_proc("a", None));
        let b = graph.add_node(make_proc("b", None));
        let c = graph.add_node(make_proc("c", None));

        make_direct_call(&mut graph, a, b);
        make_direct_call(&mut graph, b, a);
        make_direct_call(&mut graph, a, c);
        make_direct_call(&mut graph, b, c);

        let result = find_paths_between(&graph, &[a, b, c], &InspectOptions::default());
        // Must not stack overflow
        let output = format_inspect_result(
            &result,
            &graph,
            &["a".into(), "b".into(), "c".into()],
            InspectStyle::Tree,
            false,
        );

        // Both a and b should appear as direct callers of c
        assert!(output.contains("── proc:c (root, called by 2) ──"));
        // a appears under c as direct child, and a again via b→a → marked (cycle)
        let pats_section = &output[output.find("── PATHS ──").unwrap()..];
        assert!(
            pats_section.contains("(cycle)"),
            "should contain cycle marker for revisited node in reverse tree"
        );
    }

    // ── Regression: direction clarity in Tree mode output ──
    // Issue #1, #4, #7: reverse tree display is confusing because
    // destination is root and callers are children, but without clear
    // directional markers users read top-down and misinterpret direction.

    #[test]
    fn reverse_tree_root_is_destination_not_caller() {
        let mut graph = CodeGraph::new();
        let a = graph.add_node(make_proc("proc_caller", None));
        let b = graph.add_node(make_proc("proc_callee", None));
        make_direct_call(&mut graph, a, b); // a → b

        let result = find_paths_between(&graph, &[a, b], &InspectOptions::default());
        let output = format_inspect_result(
            &result,
            &graph,
            &["proc_caller".into(), "proc_callee".into()],
            InspectStyle::Tree,
            false,
        );

        let paths_section = &output[output.find("── PATHS ──").unwrap()..];
        // Root must be the destination (callee), not the source (caller)
        assert!(
            paths_section.contains("proc:proc_callee (root, called by 1)"),
            "Tree mode root must be the destination node (proc_callee), not the caller.\nGot:\n{}",
            paths_section
        );
        // The child must be the caller
        assert!(
            paths_section.contains("proc:proc_caller"),
            "Tree mode child must be the caller (proc_caller).\nGot:\n{}",
            paths_section
        );
    }

    #[test]
    fn reverse_tree_edge_labels_present_for_cross_package() {
        let mut graph = CodeGraph::new();
        let loc = SourceLocation {
            file: make_file(),
            line: 1,
        };
        let a = graph.add_node(make_proc("pkg_a_call", Some("pkg_a")));
        let b = graph.add_node(make_proc("pkg_b_proc", Some("pkg_b")));
        graph.add_edge(
            a,
            b,
            Edge::DirectCall {
                scope: CallScope::CrossPackage,
                location: loc,
            },
        );

        let result = find_paths_between(&graph, &[a, b], &InspectOptions::default());
        let output = format_inspect_result(
            &result,
            &graph,
            &["pkg_a_call".into(), "pkg_b_proc".into()],
            InspectStyle::Tree,
            false,
        );

        let paths_section = &output[output.find("── PATHS ──").unwrap()..];
        assert!(
            paths_section.contains("← [cross]"),
            "Cross-package DirectCall must show ← [cross] marker.\nGot:\n{}",
            paths_section
        );
    }

    #[test]
    fn reverse_tree_child_is_always_caller_of_parent() {
        let mut graph = CodeGraph::new();
        let a = graph.add_node(make_proc("upstream", None));
        let x = graph.add_node(make_proc("middle", None));
        let b = graph.add_node(make_proc("downstream", None));
        make_direct_call(&mut graph, a, x); // a → x
        make_direct_call(&mut graph, x, b); // x → b

        let result = find_paths_between(&graph, &[a, b], &InspectOptions::default());
        let output = format_inspect_result(
            &result,
            &graph,
            &["upstream".into(), "downstream".into()],
            InspectStyle::Tree,
            false,
        );

        let paths_section = &output[output.find("── PATHS ──").unwrap()..];

        // Root = downstream (b), called by middle (x)
        assert!(
            paths_section.contains("proc:downstream (root, called by 1)"),
            "Root should be downstream (destination).\nGot:\n{}",
            paths_section
        );

        // middle's only caller is upstream
        assert!(
            paths_section.contains("(called by 1)"),
            "Intermediate node should show '(called by 1)'.\nGot:\n{}",
            paths_section
        );

        // upstream (leaf) has no callers — verify in tree output only
        let tree_only = if let Some(summary_idx) = paths_section.find("── SUMMARY ──") {
            &paths_section[..summary_idx]
        } else {
            paths_section
        };
        let upstream_lines: Vec<&str> = tree_only
            .lines()
            .filter(|l| l.contains("proc:upstream"))
            .collect();
        assert_eq!(
            upstream_lines.len(),
            1,
            "upstream should appear exactly once in tree output"
        );
        assert!(
            !upstream_lines[0].contains("(called by"),
            "Leaf node upstream should not show '(called by)'. Got: {}",
            upstream_lines[0]
        );
    }

    #[test]
    fn tree_mode_vs_paths_mode_direction_consistency() {
        let mut graph = CodeGraph::new();
        let a = graph.add_node(make_proc("caller", None));
        let b = graph.add_node(make_proc("callee", None));
        make_direct_call(&mut graph, a, b);

        let result = find_paths_between(&graph, &[a, b], &InspectOptions::default());

        // Paths mode: shows forward direction explicitly
        let paths_output = format_inspect_result(
            &result,
            &graph,
            &["caller".into(), "callee".into()],
            InspectStyle::Paths,
            false,
        );
        assert!(
            paths_output.contains("proc:caller → proc:callee"),
            "Paths mode must show A → B direction. Got:\n{}",
            paths_output
        );

        // Tree mode: shows reverse direction (callee as root, caller as child)
        let tree_output = format_inspect_result(
            &result,
            &graph,
            &["caller".into(), "callee".into()],
            InspectStyle::Tree,
            false,
        );
        let tree_paths = &tree_output[tree_output.find("── PATHS ──").unwrap()..];
        assert!(
            tree_paths.contains("proc:callee (root, called by 1)"),
            "Tree mode root is callee (destination). Got:\n{}",
            tree_paths
        );
        assert!(
            tree_paths.contains("proc:caller"),
            "Tree mode child is caller. Got:\n{}",
            tree_paths
        );
    }

    #[test]
    fn disconnected_nodes_produce_zero_paths_no_fabrication() {
        let mut graph = CodeGraph::new();
        let a = graph.add_node(make_proc("isolated_a", None));
        let b = graph.add_node(make_proc("isolated_b", None));
        // no edges

        let result = find_paths_between(&graph, &[a, b], &InspectOptions::default());
        assert!(
            result.paths.is_empty(),
            "disconnected nodes must produce 0 paths"
        );
        assert!(
            result.summary.iter().all(|(_, _, c)| *c == 0),
            "all summary entries must be 0 for disconnected nodes"
        );
    }

    #[test]
    fn inspect_summary_shows_direction_explicitly() {
        let mut graph = CodeGraph::new();
        let a = graph.add_node(make_proc("alpha", None));
        let b = graph.add_node(make_proc("beta", None));
        make_direct_call(&mut graph, a, b); // alpha → beta

        let result = find_paths_between(&graph, &[a, b], &InspectOptions::default());
        let output = format_inspect_result(
            &result,
            &graph,
            &["alpha".into(), "beta".into()],
            InspectStyle::Both,
            true,
        );

        // Connections must show:
        //   alpha → beta : 1 path(s)  (shortest 1 hop)
        //   beta → alpha : 0 paths (unreachable)
        assert!(
            output.contains("proc:alpha → proc:beta"),
            "CONNECTIONS must show reachable direction. Got:\n{}",
            output
        );
        assert!(
            output.contains("proc:beta → proc:alpha"),
            "CONNECTIONS must show unreachable direction. Got:\n{}",
            output
        );
        assert!(
            output.contains("0 paths (unreachable)"),
            "beta→alpha must be marked unreachable. Got:\n{}",
            output
        );
    }
}

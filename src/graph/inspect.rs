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
        lines.extend(format_paths_tree(result, graph));
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
}

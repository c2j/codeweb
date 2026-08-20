use crate::graph::cluster::ClusterResult;
use crate::graph::{AccessMode, CodeGraph, Edge, Node};
use petgraph::graph::NodeIndex;
use std::collections::HashMap;

pub fn to_dot(graph: &CodeGraph) -> String {
    let mut out = String::from("digraph callgraph {\n");
    out.push_str("    node [shape=box];\n");

    for idx in graph.node_indices() {
        if let Some(line) = node_dot_line(graph, idx) {
            out.push_str("    ");
            out.push_str(&line);
            out.push('\n');
        }
    }

    for edge_idx in graph.edge_indices() {
        let (src, dst) = graph
            .edge_endpoints(edge_idx)
            .expect("edge should have endpoints");
        let (label_attr, style_attr) = edge_dot_attrs(&graph[edge_idx]);
        out.push_str(&format!(
            "    {} -> {} [{}{}];\n",
            src.index(),
            dst.index(),
            style_attr,
            label_attr
        ));
    }

    out.push_str("}\n");
    out
}

fn dot_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

const CLUSTER_COLORS: &[&str] = &[
    "lightyellow",
    "lightcyan",
    "lightgreen",
    "lightpink",
    "lavender",
    "wheat",
    "azure",
    "honeydew",
];

fn node_dot_line(graph: &CodeGraph, idx: NodeIndex) -> Option<String> {
    match &graph[idx] {
        Node::Table { .. } | Node::View { .. } | Node::MaterializedView { .. } => {
            use petgraph::Direction;
            if graph.edges_directed(idx, Direction::Outgoing).count()
                + graph.edges_directed(idx, Direction::Incoming).count()
                == 0
            {
                return None;
            }
        }
        _ => {}
    }
    let (label, shape, style) = match &graph[idx] {
        Node::Procedure { id, partial, .. } => {
            let style = if *partial { ", style=dashed" } else { "" };
            (id.to_string(), "box", style)
        }
        Node::Function { id, partial, .. } => {
            let style = if *partial { ", style=dashed" } else { "" };
            (id.to_string(), "ellipse", style)
        }
        Node::Unresolved { raw_expr, .. } => (
            format!("?{}", truncate(raw_expr, 40)),
            "box",
            ", style=dashed",
        ),
        Node::MappedStatement {
            namespace,
            statement_id,
            ..
        } => (format!("{}.{}", namespace, statement_id), "cylinder", ""),
        Node::JavaSql {
            class_name,
            method_name,
            extraction_method,
            ..
        } => {
            let name = match (class_name, method_name) {
                (Some(c), Some(m)) => format!("{}.{}", c, m),
                (Some(c), None) => c.clone(),
                (None, Some(m)) => m.clone(),
                (None, None) => extraction_method.clone(),
            };
            (name, "ellipse", "")
        }
        Node::JavaMethod {
            name, class_fqn, ..
        } => (format!("{}.{}", class_fqn, name), "diamond", ""),
        Node::JavaClass { fqn, .. } => (fqn.clone(), "folder", ""),
        Node::Table { schema, name, .. } => {
            let label = match schema {
                Some(s) => format!("{}.{}", s, name),
                None => name.clone(),
            };
            (label, "cylinder", ", style=filled, fillcolor=lightyellow")
        }
        Node::View { schema, name, .. } => {
            let label = match schema {
                Some(s) => format!("{}.{}", s, name),
                None => name.clone(),
            };
            (label, "cylinder", ", style=filled, fillcolor=lightcyan")
        }
        Node::Package { name, schema, .. } => {
            let label = match schema {
                Some(s) => format!("{}.{}", s, name),
                None => name.clone(),
            };
            (label, "component", "")
        }
        Node::Trigger { name, .. } => (name.clone(), "hexagon", ""),
        Node::Type { schema, name, .. } => {
            let label = match schema {
                Some(s) => format!("{}.{}", s, name),
                None => name.clone(),
            };
            (
                label,
                "parallelogram",
                ", style=filled, fillcolor=lightyellow",
            )
        }
        Node::Sequence { schema, name, .. } => {
            let label = match schema {
                Some(s) => format!("{}.{}", s, name),
                None => name.clone(),
            };
            (label, "box3d", "")
        }
        Node::Index {
            name,
            table_name,
            unique,
            ..
        } => {
            let label = match name {
                Some(n) => format!("{}[{}]", table_name, n),
                None => table_name.clone(),
            };
            let style = if *unique {
                ", style=filled, fillcolor=lightgreen"
            } else {
                ""
            };
            (label, "house", style)
        }
        Node::MaterializedView { schema, name, .. } => {
            let label = match schema {
                Some(s) => format!("{}.{}", s, name),
                None => name.clone(),
            };
            (label, "cylinder", ", style=filled, fillcolor=lightcyan")
        }
        Node::Synonym {
            schema,
            name,
            target_schema,
            target_name,
            ..
        } => {
            let label = match schema {
                Some(s) => format!("{}.{}", s, name),
                None => name.clone(),
            };
            let target = match target_schema {
                Some(s) => format!("{}.{}", s, target_name),
                None => target_name.clone(),
            };
            (format!("{}→{}", label, target), "trapezium", "")
        }
        Node::Event { name, .. } => (name.clone(), "octagon", ""),
        Node::BuiltinFunction { name, .. } => (name.clone(), "ellipse", ", style=dashed"),
        Node::Custom { label, .. } => (
            (**label).clone(),
            "box",
            ", style=filled, fillcolor=lightgray",
        ),
        #[cfg(feature = "jsp")]
        Node::JspPage { display_name, .. } => (
            display_name.clone(),
            "component",
            ", style=filled, fillcolor=\"#FFE4B5\"",
        ),
        #[cfg(feature = "jsp")]
        Node::JspSql { sql, kind, .. } => {
            let short: String = sql.chars().take(40).collect();
            (
                format!("{}|{}", kind.as_str(), short),
                "note",
                ", style=filled, fillcolor=\"#FFFACD\"",
            )
        }
    };
    let escaped = dot_escape(&label);
    Some(format!(
        "{} [label=\"{}\" shape={}{}];",
        idx.index(),
        escaped,
        shape,
        style
    ))
}

pub fn to_dot_with_clusters(graph: &CodeGraph, clusters: Option<&ClusterResult>) -> String {
    match clusters {
        Some(c) => to_dot_clustered(graph, c),
        None => to_dot(graph),
    }
}

fn to_dot_clustered(graph: &CodeGraph, clusters: &ClusterResult) -> String {
    let mut out = String::from("digraph callgraph {\n");
    out.push_str("    node [shape=box];\n");

    let mut clustered_nodes: HashMap<u32, Vec<NodeIndex>> = HashMap::new();
    let mut unclustered: Vec<NodeIndex> = Vec::new();

    for idx in graph.node_indices() {
        match clusters.cluster_of(idx) {
            Some(cid) => clustered_nodes.entry(cid).or_default().push(idx),
            None => unclustered.push(idx),
        }
    }

    let mut cluster_ids: Vec<u32> = clustered_nodes.keys().copied().collect();
    cluster_ids.sort();

    for cid in cluster_ids {
        let members = &clustered_nodes[&cid];
        let color = CLUSTER_COLORS[(cid as usize) % CLUSTER_COLORS.len()];
        out.push_str(&format!("    subgraph cluster_{} {{\n", cid));
        out.push_str(&format!(
            "        label=\"Cluster {} ({} nodes)\";\n",
            cid,
            members.len()
        ));
        out.push_str(&format!(
            "        style=filled;\n        color={};\n",
            color
        ));
        for &idx in members {
            if let Some(line) = node_dot_line(graph, idx) {
                out.push_str("        ");
                out.push_str(&line);
                out.push('\n');
            }
        }
        out.push_str("    }\n");
    }

    for idx in unclustered {
        if let Some(line) = node_dot_line(graph, idx) {
            out.push_str("    ");
            out.push_str(&line);
            out.push('\n');
        }
    }

    for edge_idx in graph.edge_indices() {
        let (src, dst) = graph
            .edge_endpoints(edge_idx)
            .expect("edge should have endpoints");
        let (label_attr, style_attr) = edge_dot_attrs(&graph[edge_idx]);
        out.push_str(&format!(
            "    {} -> {} [{}{}];\n",
            src.index(),
            dst.index(),
            style_attr,
            label_attr
        ));
    }

    out.push_str("}\n");
    out
}

fn edge_dot_attrs(edge: &Edge) -> (String, String) {
    match edge {
        Edge::DynamicCall { raw_expr, .. } => (
            format!("label=\"{}\"", dot_escape(&truncate(raw_expr, 30))),
            "style=dashed,".to_string(),
        ),
        Edge::CallsProcedure { .. } => (String::new(), "color=blue,".to_string()),
        Edge::InvokesMapper { .. } => (String::new(), "color=green,".to_string()),
        Edge::CallsJava { .. } => (String::new(), "color=orange,".to_string()),
        Edge::UsesBuiltinFunction { .. } => (String::new(), "color=cyan,".to_string()),
        Edge::ContainsMethod => (String::new(), "style=dotted,".to_string()),
        #[cfg(feature = "jsp")]
        Edge::ContainsSql => (String::new(), "style=dotted,".to_string()),
        Edge::Extends { .. } => ("label=\"extends\"".to_string(), "style=bold,".to_string()),
        Edge::Implements { .. } => (
            "label=\"implements\"".to_string(),
            "style=dashed,".to_string(),
        ),
        Edge::DirectCall { scope, .. } => {
            let label = match scope {
                crate::graph::CallScope::IntraPackage => "label=\"intra\"",
                crate::graph::CallScope::CrossPackage => "label=\"cross\"",
                crate::graph::CallScope::External => "",
            };
            (label.to_string(), String::new())
        }
        Edge::TableAccess {
            modes, write_kinds, ..
        } => {
            let color = if modes.contains(AccessMode::Read) && modes.contains(AccessMode::Write) {
                "purple"
            } else if modes.contains(AccessMode::LockRead) {
                "orange"
            } else if modes.contains(AccessMode::Write) || modes.contains(AccessMode::AccessExclusive) {
                "red"
            } else {
                "blue"
            };
            let label = if write_kinds.is_empty() {
                String::new()
            } else {
                let mut parts: Vec<&str> = write_kinds
                    .iter()
                    .map(crate::graph::write_kind_label)
                    .collect();
                parts.sort_unstable();
                format!("label=\"{}\"", parts.join(","))
            };
            (label, format!("color={color},"))
        }
        Edge::DependsOn { .. } => (
            "label=\"depends_on\"".to_string(),
            "color=darkviolet,style=dashed,".to_string(),
        ),
        Edge::ContainsRoutine => (
            "label=\"contains\"".to_string(),
            "style=dotted,".to_string(),
        ),
        Edge::TriggersRoutine { .. } => {
            ("label=\"triggers\"".to_string(), "color=red,".to_string())
        }
        Edge::ReferencesType { .. } => ("label=\"refs\"".to_string(), "color=teal,".to_string()),
        Edge::UsesSequence { .. } => ("label=\"uses\"".to_string(), "color=olive,".to_string()),
        Edge::IndexesTable { .. } => (
            "label=\"indexes\"".to_string(),
            "color=gray, style=dotted,".to_string(),
        ),
        Edge::AliasesObject { .. } => (
            "label=\"aliases\"".to_string(),
            "color=purple, style=dashed,".to_string(),
        ),
        Edge::CustomEdge { type_name, .. } => (
            format!("label=\"{}\"", dot_escape(type_name)),
            "style=dashed,".to_string(),
        ),
    }
}

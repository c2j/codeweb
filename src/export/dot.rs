use crate::graph::{CodeGraph, Edge, Node};

pub fn to_dot(graph: &CodeGraph) -> String {
    let mut out = String::from("digraph callgraph {\n");
    out.push_str("    node [shape=box];\n");

    for idx in graph.node_indices() {
        let (label, shape, style) = match &graph[idx] {
            Node::Procedure { id, .. } => (id.to_string(), "box", ""),
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
            Node::Table { schema, name } => {
                let label = match schema {
                    Some(s) => format!("{}.{}", s, name),
                    None => name.clone(),
                };
                (label, "cylinder", ", style=filled, fillcolor=lightyellow")
            }
            Node::View { schema, name } => {
                let label = match schema {
                    Some(s) => format!("{}.{}", s, name),
                    None => name.clone(),
                };
                (label, "cylinder", ", style=filled, fillcolor=lightcyan")
            }
        };
        let escaped = dot_escape(&label);
        out.push_str(&format!(
            "    {} [label=\"{}\" shape={}{}];\n",
            idx.index(),
            escaped,
            shape,
            style
        ));
    }

    for edge_idx in graph.edge_indices() {
        let (src, dst) = graph
            .edge_endpoints(edge_idx)
            .expect("edge should have endpoints");
        let (label_attr, style_attr) = match &graph[edge_idx] {
            Edge::DynamicCall { raw_expr, .. } => (
                format!("label=\"{}\"", dot_escape(&truncate(raw_expr, 30))),
                "style=dashed,",
            ),
            Edge::CallsProcedure { .. } => (String::new(), "color=blue,"),
            Edge::InvokesMapper { .. } => (String::new(), "color=green,"),
            Edge::CallsJava { .. } => (String::new(), "color=orange,"),
            Edge::ContainsMethod => (String::new(), "style=dotted,"),
            Edge::Extends { .. } => ("label=\"extends\"".to_string(), "style=bold,"),
            Edge::Implements { .. } => ("label=\"implements\"".to_string(), "style=dashed,"),
            Edge::DirectCall { .. } => (String::new(), ""),
            Edge::ReferencesTable { .. } => (String::new(), "color=purple,"),
        };
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

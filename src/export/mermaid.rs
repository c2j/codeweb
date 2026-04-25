use crate::graph::{CodeGraph, Edge, Node};

pub fn to_mermaid(graph: &CodeGraph) -> String {
    let mut out = String::from("graph LR\n");

    for idx in graph.node_indices() {
        let (label, shape_fmt) = match &graph[idx] {
            Node::Procedure { id, .. } => (id.to_string(), ("[\"", "\"]")),
            Node::Function { id, .. } => (id.to_string(), ("{{\"", "\"}}")),
            Node::Unresolved { raw_expr, .. } => {
                (format!("?{}", truncate(raw_expr, 30)), ("[\"", "\"]"))
            }
            Node::MappedStatement {
                namespace,
                statement_id,
                ..
            } => (format!("{}.{}", namespace, statement_id), ("([", "])")),
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
                (name, ("{{", "}}"))
            }
            Node::JavaMethod {
                name, class_fqn, ..
            } => (format!("{}.{}", class_fqn, name), ("{{\"", "\"}}")),
            Node::JavaClass { fqn, .. } => (fqn.clone(), ("[/", "/]")),
            Node::Table { schema, name } => {
                let label = match schema {
                    Some(s) => format!("{}.{}", s, name),
                    None => name.clone(),
                };
                (label, ("([", "])"))
            }
            Node::View { schema, name } => {
                let label = match schema {
                    Some(s) => format!("{}.{}", s, name),
                    None => name.clone(),
                };
                (label, ("([", "])"))
            }
            Node::Package { schema, name, .. } => {
                let label = match schema {
                    Some(s) => format!("{}.{}", s, name),
                    None => name.clone(),
                };
                (label, ("[/", "/]"))
            }
            Node::Trigger { name, .. } => (name.clone(), ("{{\"", "\"}}")),
        };
        let safe_id = safe_mermaid_id(idx.index());
        let escaped = mermaid_escape(&label);
        out.push_str(&format!(
            "    {}{}\"{}\"{}\n",
            safe_id, shape_fmt.0, escaped, shape_fmt.1
        ));
    }

    for edge_idx in graph.edge_indices() {
        let (src, dst) = graph
            .edge_endpoints(edge_idx)
            .expect("edge should have endpoints");

        let src_id = safe_mermaid_id(src.index());
        let dst_id = safe_mermaid_id(dst.index());

        let arrow = match &graph[edge_idx] {
            Edge::DynamicCall { .. } => "-.->",
            Edge::CallsProcedure { .. } => "==>",
            Edge::InvokesMapper { .. } => "-->",
            Edge::CallsJava { .. } => "-.->",
            Edge::ContainsMethod => "-.->",
            Edge::Extends { .. } => "==>",
            Edge::Implements { .. } => "-->",
            Edge::DirectCall { .. } => "-->",
            Edge::ReferencesTable { .. } => "-.->",
            Edge::ContainsRoutine => "-.->",
            Edge::TriggersRoutine { .. } => "==>",
        };

        out.push_str(&format!("    {} {} {}\n", src_id, arrow, dst_id));
    }

    out
}

fn safe_mermaid_id(idx: usize) -> String {
    format!("n{}", idx)
}

fn mermaid_escape(s: &str) -> String {
    s.replace('"', "&quot;")
        .replace('[', "&#91;")
        .replace(']', "&#93;")
        .replace('{', "&#123;")
        .replace('}', "&#125;")
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

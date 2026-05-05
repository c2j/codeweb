use crate::graph::{AccessMode, CodeGraph, Edge, Node};

pub fn to_mermaid(graph: &CodeGraph) -> String {
    let mut out = String::from("graph LR\n");

    for idx in graph.node_indices() {
        let (label, shape_fmt) = match &graph[idx] {
            Node::Procedure { id, partial, .. } => {
                let label = if *partial {
                    format!("⚠ {}", id)
                } else {
                    id.to_string()
                };
                (label, ("[\"", "\"]"))
            }
            Node::Function { id, partial, .. } => {
                let label = if *partial {
                    format!("⚠ {}", id)
                } else {
                    id.to_string()
                };
                (label, ("{{\"", "\"}}"))
            }
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
            Node::Table { schema, name, .. } => {
                let label = match schema {
                    Some(s) => format!("{}.{}", s, name),
                    None => name.clone(),
                };
                (label, ("([", "])"))
            }
            Node::View { schema, name, .. } => {
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
            Node::Type { schema, name, .. } => {
                let label = match schema {
                    Some(s) => format!("{}.{}", s, name),
                    None => name.clone(),
                };
                (label, ("[/", "/]"))
            }
            Node::Sequence { schema, name, .. } => {
                let label = match schema {
                    Some(s) => format!("{}.{}", s, name),
                    None => name.clone(),
                };
                (label, ("[", "]"))
            }
            Node::Index {
                name, table_name, ..
            } => {
                let label = match name {
                    Some(n) => format!("{}[{}]", table_name, n),
                    None => table_name.clone(),
                };
                (label, ("{{", "}}"))
            }
            Node::MaterializedView { schema, name, .. } => {
                let label = match schema {
                    Some(s) => format!("{}.{}", s, name),
                    None => name.clone(),
                };
                (label, ("([", "])"))
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
                (format!("{}→{}", label, target), ("[\\", "/]"))
            }
            Node::Event { name, .. } => (name.clone(), ("{{", "}}")),
            Node::Custom {
                label, type_name, ..
            } => (format!("{}:{}", **type_name, **label), ("[\"", "\"]")),
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
            Edge::TableAccess { modes, .. } => {
                if modes.contains(AccessMode::Write) || modes.contains(AccessMode::Truncate) {
                    "==>"
                } else {
                    "-.->"
                }
            }
            Edge::DependsOn { .. } => "-.->",
            Edge::ContainsRoutine => "-.->",
            Edge::TriggersRoutine { .. } => "==>",
            Edge::ReferencesType { .. } => "-->",
            Edge::UsesSequence { .. } => "-->",
            Edge::IndexesTable { .. } => "-.->",
            Edge::AliasesObject { .. } => "-.->",
            Edge::CustomEdge { .. } => "-.->",
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

use serde::{Deserialize, Serialize};
use std::fmt;

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeKey {
    Procedure {
        schema: Option<String>,
        name: String,
    },
    Mapper {
        namespace: String,
        statement_id: String,
    },
    JavaMethod {
        fqn: String,
    },
    JavaClass {
        fqn: String,
    },
    Table {
        schema: Option<String>,
        name: String,
    },
    View {
        schema: Option<String>,
        name: String,
    },
    JavaSql {
        file: String,
        line: usize,
    },
    Unresolved {
        raw_expr: String,
        context: String,
    },
}

impl fmt::Display for NodeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NodeKey::Procedure { schema, name } => match schema {
                Some(s) => write!(f, "proc:{}.{}", s, name),
                None => write!(f, "proc:{}", name),
            },
            NodeKey::Mapper {
                namespace,
                statement_id,
            } => write!(f, "mapper:{}.{}", namespace, statement_id),
            NodeKey::JavaMethod { fqn } => write!(f, "method:{}", fqn),
            NodeKey::JavaClass { fqn } => write!(f, "class:{}", fqn),
            NodeKey::Table { schema, name } => match schema {
                Some(s) => write!(f, "table:{}.{}", s, name),
                None => write!(f, "table:{}", name),
            },
            NodeKey::View { schema, name } => match schema {
                Some(s) => write!(f, "view:{}.{}", s, name),
                None => write!(f, "view:{}", name),
            },
            NodeKey::JavaSql { file, line } => write!(f, "javasql:{}:{}", file, line),
            NodeKey::Unresolved { raw_expr, context } => {
                write!(f, "unresolved:{} (in {})", raw_expr, context)
            }
        }
    }
}

impl NodeKey {
    #[allow(dead_code)]
    pub fn from_node(node: &super::Node) -> Self {
        match node {
            super::Node::Procedure { id, .. } => NodeKey::Procedure {
                schema: id.schema.clone(),
                name: id.name.clone(),
            },
            super::Node::MappedStatement {
                namespace,
                statement_id,
                ..
            } => NodeKey::Mapper {
                namespace: namespace.clone(),
                statement_id: statement_id.clone(),
            },
            super::Node::JavaMethod { fqn, .. } => NodeKey::JavaMethod {
                fqn: fqn.clone(),
            },
            super::Node::JavaClass { fqn, .. } => NodeKey::JavaClass {
                fqn: fqn.clone(),
            },
            super::Node::Table { schema, name } => NodeKey::Table {
                schema: schema.clone(),
                name: name.clone(),
            },
            super::Node::View { schema, name } => NodeKey::View {
                schema: schema.clone(),
                name: name.clone(),
            },
            super::Node::JavaSql { java_file, line, .. } => NodeKey::JavaSql {
                file: java_file.to_string_lossy().to_string(),
                line: *line,
            },
            super::Node::Unresolved { raw_expr, context } => NodeKey::Unresolved {
                raw_expr: raw_expr.clone(),
                context: context.clone(),
            },
        }
    }
}

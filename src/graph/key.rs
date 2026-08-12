use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeKey {
    Procedure {
        schema: Option<String>,
        package: Option<String>,
        name: String,
    },
    Function {
        schema: Option<String>,
        package: Option<String>,
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
    Package {
        schema: Option<String>,
        name: String,
    },
    Trigger {
        name: String,
    },
    Type {
        schema: Option<String>,
        name: String,
    },
    Sequence {
        schema: Option<String>,
        name: String,
    },
    Index {
        name: Option<String>,
        table_name: String,
    },
    MaterializedView {
        schema: Option<String>,
        name: String,
    },
    Synonym {
        schema: Option<String>,
        name: String,
    },
    Event {
        name: String,
    },
    BuiltinFunction {
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
    Custom {
        type_name: String,
        key: String,
    },
    #[cfg(feature = "jsp")]
    JspPage {
        path: String,
    },
    #[cfg(feature = "jsp")]
    JspSql {
        file: String,
        line: usize,
        sql_hash: String,
    },
    /// 列节点
    Column {
        table: String,
        name: String,
    },
}

impl fmt::Display for NodeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NodeKey::Procedure {
                schema,
                package,
                name,
            } => match (schema, package) {
                (Some(s), Some(p)) => write!(f, "proc:{}.{}.{}", s, p, name),
                (Some(s), None) => write!(f, "proc:{}.{}", s, name),
                (None, Some(p)) => write!(f, "proc:{}.{}", p, name),
                (None, None) => write!(f, "proc:{}", name),
            },
            NodeKey::Function {
                schema,
                package,
                name,
            } => match (schema, package) {
                (Some(s), Some(p)) => write!(f, "func:{}.{}.{}", s, p, name),
                (Some(s), None) => write!(f, "func:{}.{}", s, name),
                (None, Some(p)) => write!(f, "func:{}.{}", p, name),
                (None, None) => write!(f, "func:{}", name),
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
            NodeKey::Package { schema, name } => match schema {
                Some(s) => write!(f, "pkg:{}.{}", s, name),
                None => write!(f, "pkg:{}", name),
            },
            NodeKey::Trigger { name } => write!(f, "trigger:{}", name),
            NodeKey::Type { schema, name } => match schema {
                Some(s) => write!(f, "type:{}.{}", s, name),
                None => write!(f, "type:{}", name),
            },
            NodeKey::Sequence { schema, name } => match schema {
                Some(s) => write!(f, "seq:{}.{}", s, name),
                None => write!(f, "seq:{}", name),
            },
            NodeKey::Index { name, table_name } => match name {
                Some(n) => write!(f, "idx:{}[{}]", table_name, n),
                None => write!(f, "idx:{}", table_name),
            },
            NodeKey::MaterializedView { schema, name } => match schema {
                Some(s) => write!(f, "mview:{}.{}", s, name),
                None => write!(f, "mview:{}", name),
            },
            NodeKey::Synonym { schema, name } => match schema {
                Some(s) => write!(f, "syn:{}.{}", s, name),
                None => write!(f, "syn:{}", name),
            },
            NodeKey::Event { name } => write!(f, "event:{}", name),
            NodeKey::BuiltinFunction { name } => write!(f, "builtin:{}", name),
            NodeKey::JavaSql { file, line } => write!(f, "javasql:{}:{}", file, line),
            NodeKey::Unresolved { raw_expr, context } => {
                write!(f, "unresolved:{} (in {})", raw_expr, context)
            }
            NodeKey::Custom { type_name, key } => {
                if let Ok(map) = serde_json::from_str::<BTreeMap<String, String>>(key) {
                    let vals: Vec<&str> = map.values().map(|s| s.as_str()).collect();
                    write!(f, "{}:{}", type_name, vals.join(":"))
                } else {
                    write!(f, "custom:{}:{}", type_name, key)
                }
            }
            #[cfg(feature = "jsp")]
            NodeKey::JspPage { path } => write!(f, "jsp:{}", path),
            #[cfg(feature = "jsp")]
            NodeKey::JspSql {
                file,
                line,
                sql_hash,
            } => write!(f, "jspsql:{}:{}:{}", file, line, sql_hash),
            NodeKey::Column { table, name } => {
                write!(f, "col:{}.{}", table, name)
            }
        }
    }
}

impl NodeKey {
    #[allow(dead_code)]
    pub fn from_node(node: &super::Node) -> Self {
        match node {
            super::Node::Procedure { id, .. } => NodeKey::Procedure {
                schema: id.schema.as_ref().map(|s| s.to_lowercase()),
                package: id.package.as_ref().map(|p| p.to_lowercase()),
                name: id.name.to_lowercase(),
            },
            super::Node::Function { id, .. } => NodeKey::Function {
                schema: id.schema.as_ref().map(|s| s.to_lowercase()),
                package: id.package.as_ref().map(|p| p.to_lowercase()),
                name: id.name.to_lowercase(),
            },
            super::Node::MappedStatement {
                namespace,
                statement_id,
                ..
            } => NodeKey::Mapper {
                namespace: namespace.clone(),
                statement_id: statement_id.clone(),
            },
            super::Node::JavaMethod { fqn, .. } => NodeKey::JavaMethod { fqn: fqn.clone() },
            super::Node::JavaClass { fqn, .. } => NodeKey::JavaClass { fqn: fqn.clone() },
            super::Node::Table { schema, name, .. } => NodeKey::Table {
                schema: schema.as_ref().map(|s| s.to_lowercase()),
                name: name.to_lowercase(),
            },
            super::Node::View { schema, name, .. } => NodeKey::View {
                schema: schema.as_ref().map(|s| s.to_lowercase()),
                name: name.to_lowercase(),
            },
            super::Node::Package { schema, name, .. } => NodeKey::Package {
                schema: schema.as_ref().map(|s| s.to_lowercase()),
                name: name.to_lowercase(),
            },
            super::Node::Trigger { name, .. } => NodeKey::Trigger {
                name: name.to_lowercase(),
            },
            super::Node::Type { schema, name, .. } => NodeKey::Type {
                schema: schema.as_ref().map(|s| s.to_lowercase()),
                name: name.to_lowercase(),
            },
            super::Node::Sequence { schema, name, .. } => NodeKey::Sequence {
                schema: schema.as_ref().map(|s| s.to_lowercase()),
                name: name.to_lowercase(),
            },
            super::Node::Index {
                name, table_name, ..
            } => NodeKey::Index {
                name: name.as_ref().map(|n| n.to_lowercase()),
                table_name: table_name.to_lowercase(),
            },
            super::Node::MaterializedView { schema, name, .. } => NodeKey::MaterializedView {
                schema: schema.as_ref().map(|s| s.to_lowercase()),
                name: name.to_lowercase(),
            },
            super::Node::Synonym { schema, name, .. } => NodeKey::Synonym {
                schema: schema.as_ref().map(|s| s.to_lowercase()),
                name: name.to_lowercase(),
            },
            super::Node::Event { name, .. } => NodeKey::Event {
                name: name.to_lowercase(),
            },
            super::Node::BuiltinFunction { name, .. } => NodeKey::BuiltinFunction {
                name: name.to_lowercase(),
            },
            super::Node::JavaSql {
                java_file, line, ..
            } => NodeKey::JavaSql {
                file: java_file.to_string_lossy().to_string(),
                line: *line,
            },
            super::Node::Unresolved { raw_expr, context } => NodeKey::Unresolved {
                raw_expr: (**raw_expr).clone(),
                context: (**context).clone(),
            },
            super::Node::Custom {
                type_name,
                key_fields,
                ..
            } => {
                let key = serde_json::to_string(&**key_fields)
                    .unwrap_or_default()
                    .to_string();
                NodeKey::Custom {
                    type_name: (**type_name).clone(),
                    key,
                }
            }
            #[cfg(feature = "jsp")]
            super::Node::JspPage { path, .. } => NodeKey::JspPage {
                path: path.to_string_lossy().to_string(),
            },
            #[cfg(feature = "jsp")]
            super::Node::JspSql {
                sql, file, line, ..
            } => {
                let sql_hash = blake3::hash(sql.as_bytes()).to_hex();
                let sql_hash = sql_hash.as_str()[..16].to_string();
                NodeKey::JspSql {
                    file: file.to_string_lossy().to_string(),
                    line: *line,
                    sql_hash,
                }
            }
            super::Node::Column {
                ref owner_table,
                ref name,
                ..
            } => NodeKey::Column {
                table: owner_table.clone(),
                name: name.clone(),
            },
        }
    }

    /// Return a "relaxed" key that ignores the schema field.
    ///
    /// Used during merge to match nodes where one side has schema information
    /// (e.g. from SQL analysis: `proc:BIGFUND.PKG_IMPORT_EXCEL.proc_import_excel`)
    /// and the other does not (e.g. from CGEF import: `proc:pkg_import_excel.proc_import_excel`).
    ///
    /// Returns `None` for variants where schema is not part of the key,
    /// or when the key already has no schema.
    pub fn relaxed(&self) -> Option<NodeKey> {
        match self {
            NodeKey::Procedure {
                schema: Some(_),
                package,
                name,
            } => Some(NodeKey::Procedure {
                schema: None,
                package: package.clone(),
                name: name.clone(),
            }),
            NodeKey::Function {
                schema: Some(_),
                package,
                name,
            } => Some(NodeKey::Function {
                schema: None,
                package: package.clone(),
                name: name.clone(),
            }),
            NodeKey::Package {
                schema: Some(_),
                name,
            } => Some(NodeKey::Package {
                schema: None,
                name: name.clone(),
            }),
            NodeKey::Type {
                schema: Some(_),
                name,
            } => Some(NodeKey::Type {
                schema: None,
                name: name.clone(),
            }),
            NodeKey::Sequence {
                schema: Some(_),
                name,
            } => Some(NodeKey::Sequence {
                schema: None,
                name: name.clone(),
            }),
            NodeKey::MaterializedView {
                schema: Some(_),
                name,
            } => Some(NodeKey::MaterializedView {
                schema: None,
                name: name.clone(),
            }),
            NodeKey::Synonym {
                schema: Some(_),
                name,
            } => Some(NodeKey::Synonym {
                schema: None,
                name: name.clone(),
            }),
            _ => None,
        }
    }
}

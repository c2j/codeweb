pub mod builder;
pub mod key;
pub mod store;
pub mod traverse;

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};

/// Unique identifier for a stored procedure or function.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProcedureId {
    pub schema: Option<String>,
    pub package: Option<String>,
    pub name: String,
}

impl ProcedureId {
    /// Parse a qualified name like "schema.name" or just "name".
    pub fn from_qualified_name(qualified: &str) -> Self {
        if let Some((schema, name)) = qualified.rsplit_once('.') {
            Self {
                schema: Some(schema.to_string()),
                package: None,
                name: name.to_string(),
            }
        } else {
            Self {
                schema: None,
                package: None,
                name: qualified.to_string(),
            }
        }
    }

    /// Build from an ObjectName (Vec<String>).
    pub fn from_object_name(parts: &[String]) -> Self {
        match parts.len() {
            0 => Self {
                schema: None,
                package: None,
                name: String::new(),
            },
            1 => Self {
                schema: None,
                package: None,
                name: parts[0].clone(),
            },
            _ => Self {
                schema: Some(parts[..parts.len() - 1].join(".")),
                package: None,
                name: parts[parts.len() - 1].clone(),
            },
        }
    }
}

impl fmt::Display for ProcedureId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.schema, &self.package) {
            (Some(s), Some(p)) => write!(f, "{}.{}.{}", s, p, self.name),
            (Some(s), None) => write!(f, "{}.{}", s, self.name),
            (None, Some(p)) => write!(f, "{}.{}", p, self.name),
            (None, None) => write!(f, "{}", self.name),
        }
    }
}

/// Source location within a file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceLocation {
    pub file: PathBuf,
    pub line: usize,
}

/// A node in the call graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Node {
    /// A resolved procedure or function.
    Procedure {
        id: ProcedureId,
        location: SourceLocation,
    },
    /// An unresolved call target (e.g. dynamic SQL).
    Unresolved { raw_expr: String, context: String },

    /// A MyBatis/iBatis mapped statement from XML.
    MappedStatement {
        namespace: String,
        statement_id: String,
        kind: String,
        xml_file: PathBuf,
        line: usize,
    },

    /// SQL extracted from Java source (annotations, JDBC calls, constants).
    JavaSql {
        class_name: Option<String>,
        method_name: Option<String>,
        extraction_method: String,
        java_file: PathBuf,
        line: usize,
    },

    /// A Java method declaration.
    JavaMethod {
        fqn: String,
        class_fqn: String,
        name: String,
        signature: String,
        file: PathBuf,
        line: usize,
    },
    /// A Java class or interface declaration.
    JavaClass {
        fqn: String,
        name: String,
        package: Option<String>,
        file: PathBuf,
        line: usize,
    },
    /// A database table.
    Table {
        schema: Option<String>,
        name: String,
    },
    #[allow(dead_code)]
    View {
        schema: Option<String>,
        name: String,
    },
    Package {
        schema: Option<String>,
        name: String,
        location: SourceLocation,
    },
    Trigger {
        name: String,
        table: Vec<String>,
        location: SourceLocation,
    },
}

/// An edge in the call graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Edge {
    /// Direct static call to a known procedure.
    DirectCall {
        location: SourceLocation,
    },
    /// Dynamic call via EXECUTE with unparseable SQL.
    DynamicCall {
        raw_expr: String,
        location: SourceLocation,
    },

    /// A MappedStatement or JavaSql calls a stored procedure.
    CallsProcedure {
        location: SourceLocation,
    },
    /// A JavaSql is linked to a MappedStatement via namespace.method matching.
    InvokesMapper {
        location: SourceLocation,
    },

    /// A Java method calls another Java method.
    CallsJava {
        location: SourceLocation,
    },
    /// A Java class contains a Java method.
    ContainsMethod,
    /// A Java class extends another class.
    Extends {
        location: SourceLocation,
    },
    /// A Java class implements an interface.
    Implements {
        location: SourceLocation,
    },
    /// A statement references a table or view.
    ReferencesTable {
        location: SourceLocation,
    },
    ContainsRoutine,
    TriggersRoutine {
        location: SourceLocation,
    },
}

/// The call graph itself.
pub type CodeGraph = petgraph::Graph<Node, Edge>;

impl Node {
    #[allow(dead_code)]
    pub fn file(&self) -> &Path {
        match self {
            Node::Procedure { location, .. } => &location.file,
            Node::Unresolved { .. } => Path::new(""),
            Node::MappedStatement { xml_file, .. } => xml_file,
            Node::JavaSql { java_file, .. } => java_file,
            Node::JavaMethod { file, .. } => file,
            Node::JavaClass { file, .. } => file,
            Node::Table { .. } => Path::new(""),
            Node::View { .. } => Path::new(""),
            Node::Package { location, .. } => &location.file,
            Node::Trigger { location, .. } => &location.file,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn procedure_id_standalone() {
        let id = ProcedureId::from_qualified_name("my_proc");
        assert_eq!(id.schema, None);
        assert_eq!(id.package, None);
        assert_eq!(id.name, "my_proc");
        assert_eq!(id.to_string(), "my_proc");
    }

    #[test]
    fn procedure_id_schema_qualified() {
        let id = ProcedureId::from_qualified_name("public.my_proc");
        assert_eq!(id.schema, Some("public".to_string()));
        assert_eq!(id.package, None);
        assert_eq!(id.name, "my_proc");
        assert_eq!(id.to_string(), "public.my_proc");
    }

    #[test]
    fn procedure_id_package_member_display() {
        let id = ProcedureId {
            schema: None,
            package: Some("pkg_api".to_string()),
            name: "do_work".to_string(),
        };
        assert_eq!(id.to_string(), "pkg_api.do_work");
    }

    #[test]
    fn procedure_id_schema_package_member_display() {
        let id = ProcedureId {
            schema: Some("myschema".to_string()),
            package: Some("pkg_utils".to_string()),
            name: "cleanup".to_string(),
        };
        assert_eq!(id.to_string(), "myschema.pkg_utils.cleanup");
    }

    #[test]
    fn procedure_id_equality() {
        let a = ProcedureId {
            schema: None,
            package: Some("pkg".to_string()),
            name: "proc".to_string(),
        };
        let b = ProcedureId {
            schema: None,
            package: Some("pkg".to_string()),
            name: "proc".to_string(),
        };
        assert_eq!(a, b);
    }

    #[test]
    fn procedure_id_hash_in_hashmap() {
        let mut map = HashMap::new();
        let id = ProcedureId {
            schema: None,
            package: Some("pkg".to_string()),
            name: "proc".to_string(),
        };
        map.insert(id.clone(), 42);
        assert_eq!(map.get(&id), Some(&42));
    }
}

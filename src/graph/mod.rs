pub mod builder;
pub mod key;
pub mod store;
pub mod traverse;

use bitflags::bitflags;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

bitflags! {
    /// Access mode for table references (read/write/lock/truncate).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct AccessMode: u8 {
        const Read     = 0b0001;
        const Write    = 0b0010;
        const LockRead = 0b0100;
        const Truncate = 0b1000;
    }
}

/// What kind of write operation was performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WriteKind {
    Insert,
    InsertSelect,
    Update,
    Delete,
    MergeInsert,
    MergeUpdate,
    MergeDelete,
    SelectInto,
    Truncate,
}

/// Whether a routine is a procedure or a function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RoutineKind {
    Procedure,
    Function,
}

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

/// Unique identifier for a stored procedure or function (unified).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RoutineId {
    pub schema: Option<String>,
    pub package: Option<String>,
    pub name: String,
    pub kind: RoutineKind,
}

impl RoutineId {
    pub fn from_qualified_name(qualified: &str, kind: RoutineKind) -> Self {
        if let Some((schema, name)) = qualified.rsplit_once('.') {
            Self {
                schema: Some(schema.to_string()),
                package: None,
                name: name.to_string(),
                kind,
            }
        } else {
            Self {
                schema: None,
                package: None,
                name: qualified.to_string(),
                kind,
            }
        }
    }

    pub fn from_object_name(parts: &[String], kind: RoutineKind) -> Self {
        match parts.len() {
            0 => Self {
                schema: None,
                package: None,
                name: String::new(),
                kind,
            },
            1 => Self {
                schema: None,
                package: None,
                name: parts[0].clone(),
                kind,
            },
            _ => Self {
                schema: Some(parts[..parts.len() - 1].join(".")),
                package: None,
                name: parts[parts.len() - 1].clone(),
                kind,
            },
        }
    }
}

impl fmt::Display for RoutineId {
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
    fn access_mode_bitflags_or() {
        let rw = AccessMode::Read | AccessMode::Write;
        assert!(rw.contains(AccessMode::Read));
        assert!(rw.contains(AccessMode::Write));
        assert!(!rw.contains(AccessMode::LockRead));
        assert!(!rw.contains(AccessMode::Truncate));
    }

    #[test]
    fn access_mode_empty_is_invalid() {
        let empty = AccessMode::empty();
        assert!(!empty.contains(AccessMode::Read));
        assert!(!empty.contains(AccessMode::Write));
        assert!(empty.is_empty());
    }

    #[test]
    fn write_kind_serialization_roundtrip() {
        let mut kinds = HashSet::new();
        kinds.insert(WriteKind::Insert);
        kinds.insert(WriteKind::Update);
        let json = serde_json::to_string(&kinds).unwrap();
        let deserialized: HashSet<WriteKind> = serde_json::from_str(&json).unwrap();
        assert_eq!(kinds, deserialized);
    }

    #[test]
    fn routine_id_with_kind() {
        let id = RoutineId {
            schema: Some("myschema".to_string()),
            package: Some("pkg_api".to_string()),
            name: "do_work".to_string(),
            kind: RoutineKind::Procedure,
        };
        assert_eq!(id.to_string(), "myschema.pkg_api.do_work");
    }

    #[test]
    fn routine_id_function_display() {
        let id = RoutineId {
            schema: Some("public".to_string()),
            package: None,
            name: "calc_total".to_string(),
            kind: RoutineKind::Function,
        };
        assert_eq!(id.to_string(), "public.calc_total");
    }

    #[test]
    fn routine_id_equality_includes_kind() {
        let proc = RoutineId {
            schema: None,
            package: None,
            name: "do_thing".to_string(),
            kind: RoutineKind::Procedure,
        };
        let func = RoutineId {
            schema: None,
            package: None,
            name: "do_thing".to_string(),
            kind: RoutineKind::Function,
        };
        assert_ne!(proc, func, "Same name but different kind should not be equal");
    }

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

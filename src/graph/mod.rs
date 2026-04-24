pub mod builder;
pub mod key;
pub mod store;

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};

/// Unique identifier for a stored procedure or function.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProcedureId {
    pub schema: Option<String>,
    pub name: String,
}

impl ProcedureId {
    /// Parse a qualified name like "schema.name" or just "name".
    pub fn from_qualified_name(qualified: &str) -> Self {
        if let Some((schema, name)) = qualified.rsplit_once('.') {
            Self {
                schema: Some(schema.to_string()),
                name: name.to_string(),
            }
        } else {
            Self {
                schema: None,
                name: qualified.to_string(),
            }
        }
    }

    /// Build from an ObjectName (Vec<String>).
    pub fn from_object_name(parts: &[String]) -> Self {
        match parts.len() {
            0 => Self {
                schema: None,
                name: String::new(),
            },
            1 => Self {
                schema: None,
                name: parts[0].clone(),
            },
            _ => Self {
                schema: Some(parts[..parts.len() - 1].join(".")),
                name: parts[parts.len() - 1].clone(),
            },
        }
    }
}

impl fmt::Display for ProcedureId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.schema {
            Some(s) => write!(f, "{}.{}", s, self.name),
            None => write!(f, "{}", self.name),
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
}

/// An edge in the call graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Edge {
    /// Direct static call to a known procedure.
    DirectCall { location: SourceLocation },
    /// Dynamic call via EXECUTE with unparseable SQL.
    DynamicCall {
        raw_expr: String,
        location: SourceLocation,
    },

    /// A MappedStatement or JavaSql calls a stored procedure.
    CallsProcedure { location: SourceLocation },
    /// A JavaSql is linked to a MappedStatement via namespace.method matching.
    InvokesMapper { location: SourceLocation },

    /// A Java method calls another Java method.
    CallsJava { location: SourceLocation },
    /// A Java class contains a Java method.
    ContainsMethod,
    /// A Java class extends another class.
    Extends { location: SourceLocation },
    /// A Java class implements an interface.
    Implements { location: SourceLocation },
    /// A statement references a table or view.
    ReferencesTable { location: SourceLocation },
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
        }
    }
}

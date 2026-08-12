use crate::parser::ColumnAnalysis;

pub mod builder;
pub mod cluster;
pub mod format;
pub mod inspect;
pub mod key;
pub mod query;
pub mod search;
pub mod store;
pub mod traverse;

use bitflags::bitflags;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct JsonMap(pub BTreeMap<String, serde_json::Value>);

impl JsonMap {
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }
}

impl Default for JsonMap {
    fn default() -> Self {
        Self::new()
    }
}

impl Serialize for JsonMap {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            self.0.serialize(serializer)
        } else {
            let json_str = serde_json::to_string(&self.0).unwrap_or_default();
            serializer.serialize_str(&json_str)
        }
    }
}

impl<'de> Deserialize<'de> for JsonMap {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if deserializer.is_human_readable() {
            let map: BTreeMap<String, serde_json::Value> = BTreeMap::deserialize(deserializer)?;
            Ok(JsonMap(map))
        } else {
            let json_str: String = String::deserialize(deserializer)?;
            let map: BTreeMap<String, serde_json::Value> =
                serde_json::from_str(&json_str).unwrap_or_default();
            Ok(JsonMap(map))
        }
    }
}

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

/// Scope of a call relationship — determined by comparing caller and callee package context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CallScope {
    /// Call within the same package (highest coupling, internal implementation detail).
    IntraPackage,
    /// Call across different packages (interface-level coupling).
    CrossPackage,
    /// Call to an external/standalone routine.
    External,
}

/// Distinguishes data flow semantics for TableAccess edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DataFlowKind {
    /// Runtime DML access (procedure/function → table via SELECT/INSERT/UPDATE/DELETE).
    DmlAccess,
    /// Definition-time dependency (view/materialized view → table).
    DefinitionDependency,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EdgeCategory {
    /// Control flow: one routine invokes another.
    Call,
    /// Structural containment: package→routine, class→method.
    Composition,
    /// Data flow: read/write between routines and tables, structural dependencies.
    DataFlow,
    /// Type/object reference: routine references a type, sequence, trigger, synonym.
    Reference,
    /// Inheritance: extends/implements.
    Inheritance,
}

impl FromStr for EdgeCategory {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "call" => Ok(EdgeCategory::Call),
            "dataflow" => Ok(EdgeCategory::DataFlow),
            "reference" => Ok(EdgeCategory::Reference),
            "composition" => Ok(EdgeCategory::Composition),
            "inheritance" => Ok(EdgeCategory::Inheritance),
            other => Err(format!(
                "unknown edge type: '{}'. Valid: call, dataflow, reference, composition, inheritance",
                other
            )),
        }
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

    pub fn from_object_name(parts: &[ogsql_parser::Ident], kind: RoutineKind) -> Self {
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
                name: parts[0].to_string(),
                kind,
            },
            _ => Self {
                schema: Some(parts[..parts.len() - 1].join(".")),
                package: None,
                name: parts[parts.len() - 1].to_string(),
                kind,
            },
        }
    }

    pub fn normalized(&self) -> Self {
        Self {
            schema: self.schema.as_ref().map(|s| s.to_lowercase()),
            package: self.package.as_ref().map(|p| p.to_lowercase()),
            name: self.name.to_lowercase(),
            kind: self.kind,
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
    pub file: Arc<PathBuf>,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnSummary {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub is_primary_key: bool,
    #[serde(default)]
    pub default_value: Option<String>,
    #[serde(default)]
    pub comment: Option<String>,
}

/// Records the source table column that a view column is derived from.
// TODO: wire this into builder + format_referenced_tables for reliable column lineage
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct ViewColumnSource {
    pub view_column: String,
    pub table_schema: Option<String>,
    pub table_name: String,
    pub table_column: String,
    pub data_type: Option<String>,
}

/// Partition strategy for a table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartitionInfo {
    Range {
        columns: Vec<String>,
        #[serde(default)]
        partitions: Vec<String>,
    },
    List {
        columns: Vec<String>,
        #[serde(default)]
        partitions: Vec<String>,
    },
    Hash {
        columns: Vec<String>,
        #[serde(default)]
        partitions_count: Option<u32>,
    },
}

/// Distribution strategy for a distributed table (openGauss/GaussDB).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DistributeInfo {
    Hash { columns: Vec<String> },
    Replication,
    RoundRobin { columns: Vec<String> },
    Modulo { columns: Vec<String> },
}

/// Describes why an index exists — standalone CREATE INDEX or via a constraint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexConstraint {
    /// Index backs a PRIMARY KEY constraint.
    PrimaryKey,
    /// Index backs a UNIQUE constraint.
    Unique,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcedureBodySql {
    pub sql_text: String,
    pub kind: String,
    pub line: Option<usize>,
}

/// 聚合信息（OLAP 场景：区分维度键和聚合度量）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregationInfo {
    /// 聚合函数名 (SUM, COUNT, AVG, MAX, MIN, ...)
    pub function: String,
    /// 是否包含 DISTINCT
    #[serde(default)]
    pub distinct: bool,
    /// GROUP BY 列列表
    #[serde(default)]
    pub group_by_cols: Vec<String>,
}

/// A node in the call graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Node {
    /// A resolved stored procedure.
    Procedure {
        id: RoutineId,
        location: SourceLocation,
        #[serde(default)]
        partial: bool,
        #[serde(default)]
        body_sql: Vec<ProcedureBodySql>,
    },
    /// A resolved SQL function.
    Function {
        id: RoutineId,
        location: SourceLocation,
        #[serde(default)]
        partial: bool,
        #[serde(default)]
        body_sql: Vec<ProcedureBodySql>,
    },
    /// An unresolved call target (e.g. dynamic SQL).
    #[allow(clippy::box_collection)]
    Unresolved {
        raw_expr: Box<String>,
        context: Box<String>,
    },

    /// A MyBatis/iBatis mapped statement from XML.
    MappedStatement {
        namespace: String,
        statement_id: String,
        kind: String,
        xml_file: PathBuf,
        line: usize,
        #[serde(default)]
        sql: Option<String>,
    },

    /// SQL extracted from Java source (annotations, JDBC calls, constants).
    JavaSql {
        class_name: Option<String>,
        method_name: Option<String>,
        extraction_method: String,
        java_file: PathBuf,
        line: usize,
        #[serde(default)]
        sql: Option<String>,
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
    #[allow(clippy::box_collection)]
    Table {
        schema: Option<String>,
        name: String,
        /// true when table has a DDL definition (CREATE TABLE), false when only inferred from DML references.
        #[serde(default)]
        explicit: bool,
        /// true when table belongs to a known system schema (pg_catalog, sys, dbe_*, etc.).
        #[serde(default)]
        system: bool,
        /// None when table node was created implicitly (referenced but not parsed from DDL).
        #[serde(default)]
        location: Option<SourceLocation>,
        #[serde(default)]
        columns: Box<Vec<ColumnSummary>>,
        #[serde(default)]
        partition_by: Option<Box<PartitionInfo>>,
        #[serde(default)]
        distribute_by: Option<Box<DistributeInfo>>,
        #[serde(default)]
        tablespace: Option<String>,
        #[serde(default)]
        temporary: bool,
        #[serde(default)]
        unlogged: bool,
        #[serde(default)]
        ddl_source: Option<Box<String>>,
    },
    #[allow(dead_code)]
    #[allow(clippy::box_collection)]
    View {
        schema: Option<String>,
        name: String,
        /// true when view has a DDL definition (CREATE VIEW), false when only inferred from DML references.
        #[serde(default)]
        explicit: bool,
        /// true when view belongs to a known system schema (pg_catalog, sys, information_schema, etc.).
        #[serde(default)]
        system: bool,
        #[serde(default)]
        location: Option<SourceLocation>,
        #[serde(default)]
        columns: Box<Vec<ColumnSummary>>,
        #[serde(default)]
        ddl_source: Option<Box<String>>,
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
    /// A user-defined TYPE (composite, enum, range, base, table-of, shell).
    Type {
        schema: Option<String>,
        name: String,
        type_kind: String,
        location: SourceLocation,
    },
    /// A database SEQUENCE.
    Sequence {
        schema: Option<String>,
        name: String,
        location: SourceLocation,
    },
    /// A database INDEX.
    Index {
        name: Option<String>,
        table_schema: Option<String>,
        table_name: String,
        unique: bool,
        #[serde(default)]
        global: bool,
        #[serde(default)]
        index_method: Option<String>,
        #[serde(default)]
        columns: Vec<String>,
        #[serde(default)]
        tablespace: Option<String>,
        #[serde(default)]
        where_clause: Option<String>,
        #[serde(default)]
        constraint: Option<IndexConstraint>,
        location: SourceLocation,
    },
    /// A MATERIALIZED VIEW.
    #[allow(clippy::box_collection)]
    MaterializedView {
        schema: Option<String>,
        name: String,
        location: SourceLocation,
        #[serde(default)]
        columns: Box<Vec<ColumnSummary>>,
        #[serde(default)]
        ddl_source: Option<Box<String>>,
    },
    /// A database SYNONYM (alias for another object).
    Synonym {
        schema: Option<String>,
        name: String,
        target_schema: Option<String>,
        target_name: String,
        location: SourceLocation,
    },
    /// A scheduled EVENT (openGauss JOB).
    Event {
        name: String,
        location: SourceLocation,
    },
    /// A built-in SQL function (COUNT, SUBSTR, DBE_OUTPUT.PUT_LINE, ...).
    ///
    /// Created on-demand when a FunctionCall tagged `builtin: Some(..)` is
    /// encountered during extraction. Deduplication key: lowercased `name`.
    BuiltinFunction {
        name: String,
        category: String,
        domain: String,
        location: SourceLocation,
    },
    #[allow(clippy::box_collection)]
    Custom {
        type_name: Box<String>,
        label: Box<String>,
        key_fields: Box<BTreeMap<String, String>>,
        properties: Box<JsonMap>,
        location: Option<SourceLocation>,
    },
    /// A JSP page that contains embedded SQL.
    #[cfg(feature = "jsp")]
    JspPage {
        path: PathBuf,
        display_name: String,
        #[serde(default)]
        line: usize,
        #[serde(default)]
        url_pattern: Option<String>,
    },
    /// SQL extracted from a JSP page (scriptlet, declaration, or JSTL tag).
    #[cfg(feature = "jsp")]
    JspSql {
        sql: String,
        file: PathBuf,
        line: usize,
        kind: crate::parser::jsp_types::JspSqlKind,
        #[serde(default)]
        parsed: bool,
    },
    /// 列节点 — 列级血缘分析启用时创建
    Column {
        /// 稳定 ID: "col:<table_canonical>.<column_name>"
        id: String,
        /// 所属表/视图的 canonical 名称
        owner_table: String,
        /// 列名
        name: String,
        /// 数据类型（来自 DDL 推断，或 None）
        #[serde(default)]
        data_type: Option<String>,
        /// 表达式文本（仅当列为计算派生列时非 None）
        #[serde(default)]
        expression: Option<String>,
        /// 聚合信息（仅当列涉及聚合时非 None）
        #[serde(default)]
        aggregation: Option<Box<AggregationInfo>>,
        /// 是否为 GROUP BY 键（OLAP: 维度列）
        #[serde(default)]
        is_grouping_key: bool,
        /// 来源位置
        #[serde(default)]
        location: Option<SourceLocation>,
    },
}

/// Returns the short type tag string for a node (e.g. "proc", "table", "mapper").
pub fn node_type_tag(node: &Node) -> &'static str {
    match node {
        Node::Procedure { partial: true, .. } => "proc*",
        Node::Procedure { .. } => "proc",
        Node::Function { partial: true, .. } => "func*",
        Node::Function { .. } => "func",
        Node::Unresolved { .. } => "unres",
        Node::MappedStatement { .. } => "mapper",
        Node::JavaSql { .. } => "sql",
        Node::JavaMethod { .. } => "method",
        Node::JavaClass { .. } => "class",
        Node::Table {
            explicit: false, ..
        } => "table*",
        Node::Table { .. } => "table",
        Node::View {
            explicit: false, ..
        } => "view*",
        Node::View { .. } => "view",
        Node::Package { .. } => "pkg",
        Node::Trigger { .. } => "trigger",
        Node::Type { .. } => "type",
        Node::Sequence { .. } => "seq",
        Node::Index { .. } => "index",
        Node::MaterializedView { .. } => "mview",
        Node::Synonym { .. } => "synonym",
        Node::Event { .. } => "event",
        Node::BuiltinFunction { .. } => "builtin",
        Node::Custom { .. } => "custom",
        #[cfg(feature = "jsp")]
        Node::JspPage { .. } => "jsp",
        #[cfg(feature = "jsp")]
        Node::JspSql { .. } => "jspsql",
        Node::Column { .. } => "col",
    }
}

/// Human-readable display name for a node, suitable for CLI output.
///
/// Unlike [`NodeKey`] (which is a stable internal identifier based on full
/// paths), this returns a readable name — for JspPage/JspSql it uses a
/// shortened relative path, for others it falls back to [`NodeKey::to_string`].
pub fn node_display_name(node: &Node) -> String {
    #[cfg(feature = "jsp")]
    if let Node::JspPage {
        ref display_name, ..
    } = node
    {
        return format!("jsp:{}", display_name);
    }
    #[cfg(feature = "jsp")]
    if let Node::JspSql { ref file, line, .. } = node {
        let short = crate::parser::jsp_preprocessor::compute_display_name(file);
        return format!("jspsql:{}:{}", short, line);
    }
    match node {
        Node::Column { ref id, .. } => id.clone(),
        _ => key::NodeKey::from_node(node).to_string(),
    }
}

/// Returns a detailed sub-type tag for nodes that have internal categories.
///
/// For [`Node::BuiltinFunction`], the sub-tag distinguishes operators,
/// hints, and regular functions (e.g. "builtin:op", "builtin:hint",
/// "builtin:func"). Falls back to [`node_type_tag`] for all other node
/// types.
pub fn node_sub_type_tag(node: &Node) -> std::borrow::Cow<'static, str> {
    if let Node::BuiltinFunction { category, .. } = node {
        match category.as_str() {
            "Operator" => return std::borrow::Cow::Borrowed("builtin:op"),
            "Hint" => return std::borrow::Cow::Borrowed("builtin:hint"),
            "Special" => return std::borrow::Cow::Borrowed("builtin:special"),
            _ => return std::borrow::Cow::Borrowed("builtin:func"),
        }
    }
    std::borrow::Cow::Borrowed(node_type_tag(node))
}

/// An edge in the call graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::enum_variant_names)]
pub enum Edge {
    DirectCall {
        scope: CallScope,
        location: SourceLocation,
    },
    DynamicCall {
        raw_expr: String,
        location: SourceLocation,
    },

    CallsProcedure {
        location: SourceLocation,
    },
    InvokesMapper {
        location: SourceLocation,
    },

    CallsJava {
        location: SourceLocation,
    },
    /// A procedure/function calls a built-in SQL function.
    UsesBuiltinFunction {
        location: SourceLocation,
    },
    ContainsMethod,
    #[cfg(feature = "jsp")]
    ContainsSql,
    Extends {
        location: SourceLocation,
    },
    Implements {
        location: SourceLocation,
    },
    TableAccess {
        flow_kind: DataFlowKind,
        modes: AccessMode,
        write_kinds: std::collections::HashSet<WriteKind>,
        location: SourceLocation,
        #[serde(default)]
        column_analysis: Option<Box<ColumnAnalysis>>,
    },
    DependsOn {
        location: SourceLocation,
    },
    ContainsRoutine,
    TriggersRoutine {
        location: SourceLocation,
    },
    ReferencesType {
        location: SourceLocation,
    },
    UsesSequence {
        location: SourceLocation,
    },
    IndexesTable {
        location: SourceLocation,
    },
    AliasesObject {
        location: SourceLocation,
    },
    /// 列直接映射: source.col → target.col (SELECT a AS b)
    DataFlow {
        source_col_id: String,
        target_col_id: String,
        location: Option<SourceLocation>,
    },
    /// 列表达式派生: source.col(s) → target.col (SELECT a + 1 AS b)
    Derived {
        source_col_ids: Vec<String>,
        target_col_id: String,
        /// SQL 表达式文本
        expression: String,
        location: Option<SourceLocation>,
    },
    /// 列聚合: source.col → target.col (SELECT SUM(a) AS total GROUP BY x)
    Aggregated {
        source_col_ids: Vec<String>,
        target_col_id: String,
        /// 聚合函数名
        function: String,
        /// 是否 DISTINCT
        #[serde(default)]
        distinct: bool,
        /// GROUP BY 列节点 ID 列表
        #[serde(default)]
        group_by_col_ids: Vec<String>,
        location: Option<SourceLocation>,
    },
    CustomEdge {
        type_name: String,
        properties: JsonMap,
        location: Option<SourceLocation>,
    },
}

/// The call graph itself.
pub type CodeGraph = petgraph::Graph<Node, Edge>;

impl Edge {
    pub fn category(&self) -> EdgeCategory {
        match self {
            Edge::DirectCall { .. }
            | Edge::DynamicCall { .. }
            | Edge::CallsProcedure { .. }
            | Edge::InvokesMapper { .. }
            | Edge::CallsJava { .. }
            | Edge::UsesBuiltinFunction { .. } => EdgeCategory::Call,
            Edge::ContainsRoutine | Edge::ContainsMethod => EdgeCategory::Composition,
            #[cfg(feature = "jsp")]
            Edge::ContainsSql => EdgeCategory::Composition,
            Edge::TableAccess { .. } | Edge::DependsOn { .. } => EdgeCategory::DataFlow,
            Edge::DataFlow { .. } | Edge::Derived { .. } | Edge::Aggregated { .. } => {
                EdgeCategory::DataFlow
            }
            Edge::TriggersRoutine { .. }
            | Edge::ReferencesType { .. }
            | Edge::UsesSequence { .. }
            | Edge::IndexesTable { .. }
            | Edge::AliasesObject { .. } => EdgeCategory::Reference,
            Edge::Extends { .. } | Edge::Implements { .. } => EdgeCategory::Inheritance,
            Edge::CustomEdge { .. } => EdgeCategory::Reference,
        }
    }
}

pub fn determine_call_scope(caller: &RoutineId, callee: &RoutineId) -> CallScope {
    match (&caller.package, &callee.package) {
        (Some(a), Some(b)) if a.eq_ignore_ascii_case(b) => CallScope::IntraPackage,
        (Some(_), Some(_)) => CallScope::CrossPackage,
        _ => CallScope::External,
    }
}

fn extract_routine_id(node: &Node) -> Option<RoutineId> {
    match node {
        Node::Procedure { id, .. } | Node::Function { id, .. } => Some(id.clone()),
        _ => None,
    }
}

impl Node {
    #[allow(dead_code)]
    pub fn file(&self) -> &Path {
        match self {
            Node::Procedure { location, .. } => &location.file,
            Node::Function { location, .. } => &location.file,
            Node::Unresolved { .. } => Path::new(""),
            Node::MappedStatement { xml_file, .. } => xml_file,
            Node::JavaSql { java_file, .. } => java_file,
            Node::JavaMethod { file, .. } => file,
            Node::JavaClass { file, .. } => file,
            Node::Table { location, .. } => location
                .as_ref()
                .map(|l| l.file.as_path())
                .unwrap_or(Path::new("")),
            Node::View { location, .. } => location
                .as_ref()
                .map(|l| l.file.as_path())
                .unwrap_or(Path::new("")),
            Node::Package { location, .. } => &location.file,
            Node::Trigger { location, .. } => &location.file,
            Node::Type { location, .. } => &location.file,
            Node::Sequence { location, .. } => &location.file,
            Node::Index { location, .. } => &location.file,
            Node::MaterializedView { location, .. } => &location.file,
            Node::Synonym { location, .. } => &location.file,
            Node::Event { location, .. } => &location.file,
            Node::BuiltinFunction { location, .. } => &location.file,
            Node::Custom { location, .. } => location
                .as_ref()
                .map(|l| l.file.as_path())
                .unwrap_or(Path::new("")),
            #[cfg(feature = "jsp")]
            Node::JspPage { path, .. } => path.as_path(),
            #[cfg(feature = "jsp")]
            Node::JspSql { file, .. } => file.as_path(),
            Node::Column { location, .. } => location
                .as_ref()
                .map(|l| l.file.as_path())
                .unwrap_or(Path::new("")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

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
        assert_ne!(
            proc, func,
            "Same name but different kind should not be equal"
        );
    }

    #[test]
    fn routine_id_standalone() {
        let id = RoutineId::from_qualified_name("my_proc", RoutineKind::Procedure);
        assert_eq!(id.schema, None);
        assert_eq!(id.package, None);
        assert_eq!(id.name, "my_proc");
        assert_eq!(id.to_string(), "my_proc");
    }

    #[test]
    fn routine_id_schema_qualified() {
        let id = RoutineId::from_qualified_name("public.my_proc", RoutineKind::Procedure);
        assert_eq!(id.schema, Some("public".to_string()));
        assert_eq!(id.package, None);
        assert_eq!(id.name, "my_proc");
        assert_eq!(id.to_string(), "public.my_proc");
    }

    #[test]
    fn routine_id_package_member_display() {
        let id = RoutineId {
            schema: None,
            package: Some("pkg_api".to_string()),
            name: "do_work".to_string(),
            kind: RoutineKind::Procedure,
        };
        assert_eq!(id.to_string(), "pkg_api.do_work");
    }

    #[test]
    fn routine_id_schema_package_member_display() {
        let id = RoutineId {
            schema: Some("myschema".to_string()),
            package: Some("pkg_utils".to_string()),
            name: "cleanup".to_string(),
            kind: RoutineKind::Procedure,
        };
        assert_eq!(id.to_string(), "myschema.pkg_utils.cleanup");
    }

    #[test]
    fn routine_id_equality() {
        let a = RoutineId {
            schema: None,
            package: Some("pkg".to_string()),
            name: "proc".to_string(),
            kind: RoutineKind::Procedure,
        };
        let b = RoutineId {
            schema: None,
            package: Some("pkg".to_string()),
            name: "proc".to_string(),
            kind: RoutineKind::Procedure,
        };
        assert_eq!(a, b);
    }

    #[test]
    fn routine_id_hash_in_hashmap() {
        let mut map = HashMap::new();
        let id = RoutineId {
            schema: None,
            package: Some("pkg".to_string()),
            name: "proc".to_string(),
            kind: RoutineKind::Procedure,
        };
        map.insert(id.clone(), 42);
        assert_eq!(map.get(&id), Some(&42));
    }

    #[test]
    fn new_node_variants_file() {
        let file = Arc::new(PathBuf::from("test.sql"));
        let loc = SourceLocation {
            file: file.clone(),
            line: 42,
        };

        let type_node = Node::Type {
            schema: Some("public".to_string()),
            name: "my_type".to_string(),
            type_kind: "composite".to_string(),
            location: loc.clone(),
        };
        assert_eq!(type_node.file(), Path::new("test.sql"));

        let seq_node = Node::Sequence {
            schema: Some("public".to_string()),
            name: "my_seq".to_string(),
            location: loc.clone(),
        };
        assert_eq!(seq_node.file(), Path::new("test.sql"));

        let idx_node = Node::Index {
            name: Some("idx_name".to_string()),
            table_schema: Some("public".to_string()),
            table_name: "my_table".to_string(),
            unique: true,
            global: false,
            index_method: Some("btree".to_string()),
            columns: vec!["col_a".to_string(), "col_b".to_string()],
            tablespace: None,
            where_clause: None,
            constraint: Some(IndexConstraint::Unique),
            location: loc.clone(),
        };
        assert_eq!(idx_node.file(), Path::new("test.sql"));

        let mview_node = Node::MaterializedView {
            schema: Some("public".to_string()),
            name: "my_mview".to_string(),
            location: loc.clone(),
            columns: Box::new(vec![]),
            ddl_source: None,
        };
        assert_eq!(mview_node.file(), Path::new("test.sql"));

        let syn_node = Node::Synonym {
            schema: Some("public".to_string()),
            name: "my_syn".to_string(),
            target_schema: Some("other".to_string()),
            target_name: "real_obj".to_string(),
            location: loc.clone(),
        };
        assert_eq!(syn_node.file(), Path::new("test.sql"));

        let event_node = Node::Event {
            name: "my_event".to_string(),
            location: loc.clone(),
        };
        assert_eq!(event_node.file(), Path::new("test.sql"));
    }

    #[test]
    fn new_edge_variants_construct() {
        let file = Arc::new(PathBuf::from("test.sql"));
        let loc = SourceLocation { file, line: 1 };

        let _ = Edge::ReferencesType {
            location: loc.clone(),
        };
        let _ = Edge::UsesSequence {
            location: loc.clone(),
        };
        let _ = Edge::IndexesTable {
            location: loc.clone(),
        };
        let _ = Edge::AliasesObject {
            location: loc.clone(),
        };
    }

    #[test]
    fn table_node_with_location_and_columns() {
        let file = Arc::new(PathBuf::from("create_tables.sql"));
        let table = Node::Table {
            explicit: true,
            system: false,
            schema: Some("public".to_string()),
            name: "orders".to_string(),
            location: Some(SourceLocation {
                file: file.clone(),
                line: 10,
            }),
            columns: Box::new(vec![
                ColumnSummary {
                    name: "id".to_string(),
                    data_type: "INTEGER".to_string(),
                    nullable: false,
                    is_primary_key: true,
                    default_value: None,
                    comment: None,
                },
                ColumnSummary {
                    name: "amount".to_string(),
                    data_type: "NUMERIC(10,2)".to_string(),
                    nullable: true,
                    is_primary_key: false,
                    default_value: Some("0".to_string()),
                    comment: Some("order amount".to_string()),
                },
            ]),
            partition_by: Some(Box::new(PartitionInfo::Range {
                columns: vec!["created_at".to_string()],
                partitions: vec!["p_2024".to_string(), "p_2025".to_string()],
            })),
            distribute_by: Some(Box::new(DistributeInfo::Hash {
                columns: vec!["id".to_string()],
            })),
            tablespace: Some("pg_default".to_string()),
            temporary: false,
            unlogged: false,
            ddl_source: Some(Box::new("CREATE TABLE public.orders (...)".to_string())),
        };
        assert_eq!(table.file(), Path::new("create_tables.sql"));
    }

    #[test]
    fn table_node_minimal_implicit() {
        let table = Node::Table {
            schema: None,
            explicit: false,
            system: false,
            name: "my_table".to_string(),
            location: None,
            columns: Box::new(vec![]),
            partition_by: None,
            distribute_by: None,
            tablespace: None,
            temporary: false,
            unlogged: false,
            ddl_source: None,
        };
        assert_eq!(table.file(), Path::new(""));
    }

    #[test]
    fn partition_info_serialization_roundtrip() {
        let info = PartitionInfo::Range {
            columns: vec!["created_at".to_string()],
            partitions: vec!["p_2024".to_string()],
        };
        let json = serde_json::to_string(&info).unwrap();
        let de: PartitionInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info, de);
    }

    #[test]
    fn distribute_info_serialization_roundtrip() {
        let info = DistributeInfo::Hash {
            columns: vec!["user_id".to_string()],
        };
        let json = serde_json::to_string(&info).unwrap();
        let de: DistributeInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info, de);
    }

    #[test]
    fn column_summary_serialization_roundtrip() {
        let col = ColumnSummary {
            name: "id".to_string(),
            data_type: "BIGINT".to_string(),
            nullable: false,
            is_primary_key: true,
            default_value: Some("nextval('seq')".to_string()),
            comment: None,
        };
        let json = serde_json::to_string(&col).unwrap();
        let de: ColumnSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(col, de);
    }

    #[test]
    fn table_node_serde_backward_compat() {
        let json = r#"{"schema":"public","name":"t"}"#;
        let table: Node = serde_json::from_str(&format!("{{\"Table\":{}}}", json)).unwrap();
        if let Node::Table {
            schema,
            name,
            location,
            columns,
            ..
        } = table
        {
            assert_eq!(schema, Some("public".to_string()));
            assert_eq!(name, "t");
            assert!(location.is_none());
            assert!(columns.is_empty());
        } else {
            panic!("expected Table");
        }
    }

    #[test]
    fn node_enum_size_below_200_bytes() {
        assert!(
            std::mem::size_of::<Node>() < 200,
            "Node enum size is {} bytes, expected < 200",
            std::mem::size_of::<Node>()
        );
    }

    #[test]
    fn synthetic_100k_nodes_memory_report() {
        use std::sync::Arc;

        let mut graph = CodeGraph::new();
        let file = Arc::new(PathBuf::from("bench.sql"));
        let loc = SourceLocation {
            file: file.clone(),
            line: 1,
        };
        let edge = Edge::DirectCall {
            scope: CallScope::IntraPackage,
            location: loc.clone(),
        };

        let mut node_indices = Vec::with_capacity(100_000);

        // 50K Procedure nodes
        for i in 0..50_000 {
            let node = Node::Procedure {
                id: RoutineId {
                    schema: Some("bench".to_string()),
                    package: Some(format!("pkg_{}", i % 100)),
                    name: format!("proc_{}", i),
                    kind: RoutineKind::Procedure,
                },
                location: loc.clone(),
                partial: false,
                body_sql: Vec::new(),
            };
            node_indices.push(graph.add_node(node));
        }

        // 50K Table nodes
        for i in 0..50_000 {
            let node = Node::Table {
                schema: Some("bench".to_string()),
                name: format!("table_{}", i),
                explicit: false,
                system: false,
                location: None,
                columns: Box::new(vec![ColumnSummary {
                    name: "id".to_string(),
                    data_type: "BIGINT".to_string(),
                    nullable: false,
                    is_primary_key: true,
                    default_value: None,
                    comment: None,
                }]),
                partition_by: None,
                distribute_by: None,
                tablespace: None,
                temporary: false,
                unlogged: false,
                ddl_source: None,
            };
            node_indices.push(graph.add_node(node));
        }

        // 200K edges between sequential nodes (bidirectional cycle)
        for i in 0..100_000 {
            let a = node_indices[i];
            let b = node_indices[(i + 1) % 100_000];
            graph.add_edge(a, b, edge.clone());
            graph.add_edge(b, a, edge.clone());
        }

        let node_count = graph.node_count();
        let edge_count = graph.edge_count();
        eprintln!("Node size: {} bytes", std::mem::size_of::<Node>());
        eprintln!("Edge size: {} bytes", std::mem::size_of::<Edge>());
        eprintln!("node_count: {}", node_count);
        eprintln!("edge_count: {}", edge_count);

        assert_eq!(node_count, 100_000);
        assert_eq!(edge_count, 200_000);

        let store = crate::graph::store::GraphStore::from_graph("bench", graph);
        let stats = store.stats();
        eprintln!("GraphStore stats: {:?}", stats);
    }

    #[test]
    fn node_type_tag_all_variants_non_empty() {
        use std::sync::Arc;

        let file = Arc::new(PathBuf::from("test.sql"));
        let loc = SourceLocation {
            file: file.clone(),
            line: 1,
        };

        let nodes: Vec<Node> = vec![
            Node::Procedure {
                id: RoutineId {
                    schema: Some("public".to_string()),
                    package: None,
                    name: "p".to_string(),
                    kind: RoutineKind::Procedure,
                },
                location: loc.clone(),
                partial: false,
                body_sql: Vec::new(),
            },
            Node::Procedure {
                id: RoutineId {
                    schema: Some("public".to_string()),
                    package: None,
                    name: "p_partial".to_string(),
                    kind: RoutineKind::Procedure,
                },
                location: loc.clone(),
                partial: true,
                body_sql: Vec::new(),
            },
            Node::Function {
                id: RoutineId {
                    schema: Some("public".to_string()),
                    package: None,
                    name: "f".to_string(),
                    kind: RoutineKind::Function,
                },
                location: loc.clone(),
                partial: false,
                body_sql: Vec::new(),
            },
            Node::Function {
                id: RoutineId {
                    schema: Some("public".to_string()),
                    package: None,
                    name: "f_partial".to_string(),
                    kind: RoutineKind::Function,
                },
                location: loc.clone(),
                partial: true,
                body_sql: Vec::new(),
            },
            Node::Package {
                schema: Some("public".to_string()),
                name: "my_pkg".to_string(),
                location: loc.clone(),
            },
            Node::Table {
                schema: Some("public".to_string()),
                name: "t".to_string(),
                explicit: true,
                system: false,
                location: None,
                columns: Box::new(vec![]),
                partition_by: None,
                distribute_by: None,
                tablespace: None,
                temporary: false,
                unlogged: false,
                ddl_source: None,
            },
            Node::Table {
                schema: Some("public".to_string()),
                name: "t_implicit".to_string(),
                explicit: false,
                system: false,
                location: None,
                columns: Box::new(vec![]),
                partition_by: None,
                distribute_by: None,
                tablespace: None,
                temporary: false,
                unlogged: false,
                ddl_source: None,
            },
            Node::View {
                schema: Some("public".to_string()),
                name: "v".to_string(),
                explicit: true,
                system: false,
                location: None,
                columns: Box::new(vec![]),
                ddl_source: None,
            },
            Node::View {
                schema: Some("public".to_string()),
                name: "v_implicit".to_string(),
                explicit: false,
                system: false,
                location: None,
                columns: Box::new(vec![]),
                ddl_source: None,
            },
            Node::MaterializedView {
                schema: Some("public".to_string()),
                name: "mv".to_string(),
                location: loc.clone(),
                columns: Box::new(vec![]),
                ddl_source: None,
            },
            Node::Trigger {
                name: "trig".to_string(),
                table: vec!["t".to_string()],
                location: loc.clone(),
            },
            Node::Type {
                name: "my_type".to_string(),
                schema: Some("public".to_string()),
                type_kind: "object".to_string(),
                location: loc.clone(),
            },
            Node::Sequence {
                name: "seq".to_string(),
                schema: Some("public".to_string()),
                location: loc.clone(),
            },
            Node::Index {
                name: Some("idx".to_string()),
                table_schema: Some("public".to_string()),
                table_name: "t".to_string(),
                unique: false,
                global: false,
                index_method: None,
                columns: vec![],
                tablespace: None,
                where_clause: None,
                constraint: None,
                location: loc.clone(),
            },
            Node::Synonym {
                name: "syn".to_string(),
                schema: Some("public".to_string()),
                target_schema: Some("public".to_string()),
                target_name: "t".to_string(),
                location: loc.clone(),
            },
            Node::Event {
                name: "ev".to_string(),
                location: loc.clone(),
            },
            Node::BuiltinFunction {
                name: "count".to_string(),
                category: "Aggregate".to_string(),
                domain: "sql".to_string(),
                location: loc.clone(),
            },
            Node::Unresolved {
                raw_expr: Box::new("unknown_func".to_string()),
                context: Box::new("procedure body".to_string()),
            },
            Node::MappedStatement {
                namespace: "com.example.Mapper".to_string(),
                statement_id: "selectOrders".to_string(),
                kind: "select".to_string(),
                xml_file: PathBuf::from("Mapper.xml"),
                line: 10,
                sql: None,
            },
            Node::JavaSql {
                class_name: Some("OrderDao".to_string()),
                method_name: Some("findAll".to_string()),
                extraction_method: "annotation".to_string(),
                java_file: PathBuf::from("OrderDao.java"),
                line: 42,
                sql: Some("SELECT * FROM orders".to_string()),
            },
            Node::JavaMethod {
                fqn: "com.example.OrderService.process".to_string(),
                class_fqn: "com.example.OrderService".to_string(),
                name: "process".to_string(),
                signature: "(Order)void".to_string(),
                file: PathBuf::from("OrderService.java"),
                line: 15,
            },
            Node::JavaClass {
                fqn: "com.example.OrderService".to_string(),
                name: "OrderService".to_string(),
                package: Some("com.example".to_string()),
                file: PathBuf::from("OrderService.java"),
                line: 1,
            },
        ];

        let mut tags_found = std::collections::HashSet::new();
        for node in &nodes {
            let tag = node_type_tag(node);
            assert!(
                !tag.is_empty(),
                "node_type_tag returned empty string for node"
            );
            tags_found.insert(tag.to_string());
        }

        // All expected short tags must appear
        let expected_tags = [
            "proc", "proc*", "func", "func*", "pkg", "table", "table*", "view", "view*", "mview",
            "trigger", "type", "seq", "index", "synonym", "event", "builtin", "unres", "mapper",
            "sql", "method", "class",
        ];
        for expected in &expected_tags {
            assert!(
                tags_found.contains(*expected),
                "expected tag '{}' not found in node_type_tag output; found: {:?}",
                expected,
                tags_found
            );
        }
    }

    #[test]
    fn node_type_tag_short_names() {
        use std::sync::Arc;

        let file = Arc::new(PathBuf::from("test.sql"));
        let loc = SourceLocation {
            file: file.clone(),
            line: 1,
        };

        // Verify specific short-tag → variant mappings that
        // must NEVER change (CLI and JSON consumers depend on them).
        let proc = Node::Procedure {
            id: RoutineId {
                schema: Some("s".to_string()),
                package: None,
                name: "p".to_string(),
                kind: RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        };
        assert_eq!(node_type_tag(&proc), "proc");

        let partial_proc = Node::Procedure {
            id: RoutineId {
                schema: Some("s".to_string()),
                package: None,
                name: "pp".to_string(),
                kind: RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: true,
            body_sql: Vec::new(),
        };
        assert_eq!(node_type_tag(&partial_proc), "proc*");

        let pkg = Node::Package {
            schema: Some("s".to_string()),
            name: "pkg".to_string(),
            location: loc.clone(),
        };
        assert_eq!(node_type_tag(&pkg), "pkg");

        let tbl = Node::Table {
            schema: None,
            name: "t".to_string(),
            explicit: true,
            system: false,
            location: None,
            columns: Box::new(vec![]),
            partition_by: None,
            distribute_by: None,
            tablespace: None,
            temporary: false,
            unlogged: false,
            ddl_source: None,
        };
        assert_eq!(node_type_tag(&tbl), "table");

        let tbl_implicit = Node::Table {
            schema: None,
            name: "ti".to_string(),
            explicit: false,
            system: false,
            location: None,
            columns: Box::new(vec![]),
            partition_by: None,
            distribute_by: None,
            tablespace: None,
            temporary: false,
            unlogged: false,
            ddl_source: None,
        };
        assert_eq!(node_type_tag(&tbl_implicit), "table*");

        let mview = Node::MaterializedView {
            schema: None,
            name: "mv".to_string(),
            location: loc.clone(),
            columns: Box::new(vec![]),
            ddl_source: None,
        };
        assert_eq!(node_type_tag(&mview), "mview");

        let builtin = Node::BuiltinFunction {
            name: "count".to_string(),
            category: "Aggregate".to_string(),
            domain: "sql".to_string(),
            location: loc.clone(),
        };
        assert_eq!(node_type_tag(&builtin), "builtin");

        let unres = Node::Unresolved {
            raw_expr: Box::new("x".to_string()),
            context: Box::new("body".to_string()),
        };
        assert_eq!(node_type_tag(&unres), "unres");
    }
}

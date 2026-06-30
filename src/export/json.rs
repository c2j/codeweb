use crate::error::Result;
use crate::graph::{
    AccessMode, CodeGraph, ColumnSummary, DistributeInfo, Edge, Node, PartitionInfo, WriteKind,
};
use crate::parser::ColumnAnalysis;
use serde::Serialize;

fn is_false(v: &bool) -> bool {
    !v
}

#[derive(Serialize)]
struct GraphJson {
    nodes: Vec<NodeJson>,
    edges: Vec<EdgeJson>,
}

#[derive(Serialize)]
struct NodeJson {
    id: usize,
    #[serde(flatten)]
    kind: NodeKindJson,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum NodeKindJson {
    Procedure {
        name: String,
        schema: Option<String>,
        file: String,
        line: usize,
        #[serde(skip_serializing_if = "is_false")]
        partial: bool,
    },
    Function {
        name: String,
        schema: Option<String>,
        file: String,
        line: usize,
        #[serde(skip_serializing_if = "is_false")]
        partial: bool,
    },
    Unresolved {
        raw_expr: String,
        context: String,
    },
    MappedStatement {
        namespace: String,
        statement_id: String,
        kind: String,
        file: String,
        line: usize,
        sql: Option<String>,
    },
    JavaSql {
        class_name: Option<String>,
        method_name: Option<String>,
        extraction_method: String,
        file: String,
        line: usize,
        sql: Option<String>,
    },
    JavaMethod {
        fqn: String,
        class_fqn: String,
        name: String,
        signature: String,
        file: String,
        line: usize,
    },
    JavaClass {
        fqn: String,
        name: String,
        package: Option<String>,
        file: String,
        line: usize,
    },
    Table {
        schema: Option<String>,
        name: String,
        #[serde(skip_serializing_if = "is_false")]
        explicit: bool,
        #[serde(skip_serializing_if = "is_false")]
        system: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        file: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        line: Option<usize>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        columns: Vec<ColumnSummary>,
        #[serde(skip_serializing_if = "Option::is_none")]
        partition_by: Option<PartitionInfo>,
        #[serde(skip_serializing_if = "Option::is_none")]
        distribute_by: Option<DistributeInfo>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tablespace: Option<String>,
        #[serde(skip_serializing_if = "is_false")]
        temporary: bool,
        #[serde(skip_serializing_if = "is_false")]
        unlogged: bool,
    },
    View {
        schema: Option<String>,
        name: String,
        #[serde(skip_serializing_if = "is_false")]
        explicit: bool,
        #[serde(skip_serializing_if = "is_false")]
        system: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        file: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        line: Option<usize>,
    },
    #[serde(rename = "package")]
    Package {
        schema: Option<String>,
        name: String,
        file: String,
        line: usize,
    },
    #[serde(rename = "trigger")]
    Trigger {
        name: String,
        table: String,
        file: String,
        line: usize,
    },
    Type {
        name: String,
        schema: Option<String>,
        type_kind: String,
        file: String,
        line: usize,
    },
    Sequence {
        name: String,
        schema: Option<String>,
        file: String,
        line: usize,
    },
    Index {
        name: Option<String>,
        table_name: String,
        unique: bool,
        #[serde(skip_serializing_if = "is_false")]
        global: bool,
        file: String,
        line: usize,
    },
    MaterializedView {
        name: String,
        schema: Option<String>,
        file: String,
        line: usize,
    },
    Synonym {
        name: String,
        schema: Option<String>,
        target_name: String,
        target_schema: Option<String>,
        file: String,
        line: usize,
    },
    Event {
        name: String,
        file: String,
        line: usize,
    },
    BuiltinFunction {
        name: String,
        category: String,
        domain: String,
        file: String,
        line: usize,
    },
    Custom {
        custom_type: String,
        label: String,
        key_fields: serde_json::Value,
        properties: serde_json::Value,
        file: Option<String>,
        line: Option<usize>,
    },
    #[cfg(feature = "jsp")]
    #[serde(rename = "jsp")]
    JspPage {
        file: String,
        display_name: String,
        url_pattern: Option<String>,
    },
    #[cfg(feature = "jsp")]
    #[serde(rename = "jsql")]
    JspSql {
        sql: String,
        file: String,
        line: usize,
        kind: String,
        parsed: bool,
    },
}

#[derive(Serialize)]
struct EdgeJson {
    source: usize,
    target: usize,
    #[serde(flatten)]
    kind: EdgeKindJson,
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum EdgeKindJson {
    #[serde(rename = "direct")]
    Direct {
        scope: String,
        file: String,
        line: usize,
    },
    #[serde(rename = "dynamic")]
    Dynamic {
        raw_expr: String,
        file: String,
        line: usize,
    },
    #[serde(rename = "calls_procedure")]
    CallsProcedure { file: String, line: usize },
    #[serde(rename = "invokes_mapper")]
    InvokesMapper { file: String, line: usize },
    #[serde(rename = "calls_java")]
    CallsJava { file: String, line: usize },
    #[serde(rename = "uses_builtin_function")]
    UsesBuiltinFunction { file: String, line: usize },
    #[serde(rename = "contains_method")]
    ContainsMethod,
    #[serde(rename = "extends")]
    Extends { file: String, line: usize },
    #[serde(rename = "implements")]
    Implements { file: String, line: usize },
    #[serde(rename = "table_access")]
    TableAccess {
        flow_kind: String,
        modes: Vec<String>,
        write_kinds: Vec<String>,
        file: String,
        line: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        column_analysis: Option<ColumnAnalysis>,
    },
    #[serde(rename = "depends_on")]
    DependsOn { file: String, line: usize },
    #[serde(rename = "contains_routine")]
    ContainsRoutine,
    #[serde(rename = "triggers_routine")]
    TriggersRoutine { file: String, line: usize },
    #[serde(rename = "references_type")]
    ReferencesType { file: String, line: usize },
    #[serde(rename = "uses_sequence")]
    UsesSequence { file: String, line: usize },
    #[serde(rename = "indexes_table")]
    IndexesTable { file: String, line: usize },
    #[serde(rename = "aliases_object")]
    AliasesObject { file: String, line: usize },
    #[serde(rename = "custom")]
    CustomEdge {
        custom_type: String,
        properties: serde_json::Value,
        file: Option<String>,
        line: Option<usize>,
    },
    #[cfg(feature = "jsp")]
    #[serde(rename = "contains_sql")]
    ContainsSql,
}

pub fn to_json(graph: &CodeGraph) -> Result<String> {
    let mut nodes = Vec::new();
    for idx in graph.node_indices() {
        let node_json = match &graph[idx] {
            Node::Procedure {
                id,
                location,
                partial,
                ..
            } => NodeJson {
                id: idx.index(),
                kind: NodeKindJson::Procedure {
                    name: id.name.clone(),
                    schema: id.schema.clone(),
                    file: location.file.to_string_lossy().to_string(),
                    line: location.line,
                    partial: *partial,
                },
            },
            Node::Function {
                id,
                location,
                partial,
                ..
            } => NodeJson {
                id: idx.index(),
                kind: NodeKindJson::Function {
                    name: id.name.clone(),
                    schema: id.schema.clone(),
                    file: location.file.to_string_lossy().to_string(),
                    line: location.line,
                    partial: *partial,
                },
            },
            Node::Unresolved { raw_expr, context } => NodeJson {
                id: idx.index(),
                kind: NodeKindJson::Unresolved {
                    raw_expr: (**raw_expr).clone(),
                    context: (**context).clone(),
                },
            },
            Node::MappedStatement {
                namespace,
                statement_id,
                kind,
                xml_file,
                line,
                sql,
            } => NodeJson {
                id: idx.index(),
                kind: NodeKindJson::MappedStatement {
                    namespace: namespace.clone(),
                    statement_id: statement_id.clone(),
                    kind: kind.clone(),
                    file: xml_file.to_string_lossy().to_string(),
                    line: *line,
                    sql: sql.clone(),
                },
            },
            Node::JavaSql {
                class_name,
                method_name,
                extraction_method,
                java_file,
                line,
                sql,
            } => NodeJson {
                id: idx.index(),
                kind: NodeKindJson::JavaSql {
                    class_name: class_name.clone(),
                    method_name: method_name.clone(),
                    extraction_method: extraction_method.clone(),
                    file: java_file.to_string_lossy().to_string(),
                    line: *line,
                    sql: sql.clone(),
                },
            },
            Node::JavaMethod {
                fqn,
                class_fqn,
                name,
                signature,
                file,
                line,
            } => NodeJson {
                id: idx.index(),
                kind: NodeKindJson::JavaMethod {
                    fqn: fqn.clone(),
                    class_fqn: class_fqn.clone(),
                    name: name.clone(),
                    signature: signature.clone(),
                    file: file.to_string_lossy().to_string(),
                    line: *line,
                },
            },
            Node::JavaClass {
                fqn,
                name,
                package,
                file,
                line,
            } => NodeJson {
                id: idx.index(),
                kind: NodeKindJson::JavaClass {
                    fqn: fqn.clone(),
                    name: name.clone(),
                    package: package.clone(),
                    file: file.to_string_lossy().to_string(),
                    line: *line,
                },
            },
            Node::Table {
                explicit,
                system,
                schema,
                name,
                location,
                columns,
                partition_by,
                distribute_by,
                tablespace,
                temporary,
                unlogged,
                ..
            } => NodeJson {
                id: idx.index(),
                kind: NodeKindJson::Table {
                    explicit: *explicit,
                    system: *system,
                    schema: schema.clone(),
                    name: name.clone(),
                    file: location
                        .as_ref()
                        .map(|l| l.file.to_string_lossy().to_string()),
                    line: location.as_ref().map(|l| l.line),
                    columns: (**columns).clone(),
                    partition_by: partition_by.as_deref().cloned(),
                    distribute_by: distribute_by.as_deref().cloned(),
                    tablespace: tablespace.clone(),
                    temporary: *temporary,
                    unlogged: *unlogged,
                },
            },
            Node::View {
                explicit,
                system,
                schema,
                name,
                location,
                ..
            } => NodeJson {
                id: idx.index(),
                kind: NodeKindJson::View {
                    explicit: *explicit,
                    system: *system,
                    schema: schema.clone(),
                    name: name.clone(),
                    file: location
                        .as_ref()
                        .map(|l| l.file.to_string_lossy().to_string()),
                    line: location.as_ref().map(|l| l.line),
                },
            },
            Node::Package {
                schema,
                name,
                location,
            } => NodeJson {
                id: idx.index(),
                kind: NodeKindJson::Package {
                    schema: schema.clone(),
                    name: name.clone(),
                    file: location.file.to_string_lossy().to_string(),
                    line: location.line,
                },
            },
            Node::Trigger {
                name,
                table,
                location,
            } => NodeJson {
                id: idx.index(),
                kind: NodeKindJson::Trigger {
                    name: name.clone(),
                    table: table.join("."),
                    file: location.file.to_string_lossy().to_string(),
                    line: location.line,
                },
            },
            Node::Type {
                schema,
                name,
                type_kind,
                location,
            } => NodeJson {
                id: idx.index(),
                kind: NodeKindJson::Type {
                    name: name.clone(),
                    schema: schema.clone(),
                    type_kind: type_kind.clone(),
                    file: location.file.to_string_lossy().to_string(),
                    line: location.line,
                },
            },
            Node::Sequence {
                schema,
                name,
                location,
            } => NodeJson {
                id: idx.index(),
                kind: NodeKindJson::Sequence {
                    name: name.clone(),
                    schema: schema.clone(),
                    file: location.file.to_string_lossy().to_string(),
                    line: location.line,
                },
            },
            Node::Index {
                name,
                table_schema: _,
                table_name,
                unique,
                global,
                location,
            } => NodeJson {
                id: idx.index(),
                kind: NodeKindJson::Index {
                    name: name.clone(),
                    table_name: table_name.clone(),
                    unique: *unique,
                    global: *global,
                    file: location.file.to_string_lossy().to_string(),
                    line: location.line,
                },
            },
            Node::MaterializedView {
                schema,
                name,
                location,
            } => NodeJson {
                id: idx.index(),
                kind: NodeKindJson::MaterializedView {
                    name: name.clone(),
                    schema: schema.clone(),
                    file: location.file.to_string_lossy().to_string(),
                    line: location.line,
                },
            },
            Node::Synonym {
                schema,
                name,
                target_schema,
                target_name,
                location,
            } => NodeJson {
                id: idx.index(),
                kind: NodeKindJson::Synonym {
                    name: name.clone(),
                    schema: schema.clone(),
                    target_name: target_name.clone(),
                    target_schema: target_schema.clone(),
                    file: location.file.to_string_lossy().to_string(),
                    line: location.line,
                },
            },
            Node::Event { name, location } => NodeJson {
                id: idx.index(),
                kind: NodeKindJson::Event {
                    name: name.clone(),
                    file: location.file.to_string_lossy().to_string(),
                    line: location.line,
                },
            },
            Node::BuiltinFunction {
                name,
                category,
                domain,
                location,
            } => NodeJson {
                id: idx.index(),
                kind: NodeKindJson::BuiltinFunction {
                    name: name.clone(),
                    category: category.clone(),
                    domain: domain.clone(),
                    file: location.file.to_string_lossy().to_string(),
                    line: location.line,
                },
            },
            Node::Custom {
                type_name,
                label,
                key_fields,
                properties,
                location,
            } => NodeJson {
                id: idx.index(),
                kind: NodeKindJson::Custom {
                    custom_type: (**type_name).clone(),
                    label: (**label).clone(),
                    key_fields: serde_json::to_value(&**key_fields)
                        .unwrap_or(serde_json::Value::Null),
                    properties: serde_json::to_value(&**properties)
                        .unwrap_or(serde_json::Value::Null),
                    file: location
                        .as_ref()
                        .map(|l| l.file.to_string_lossy().to_string()),
                    line: location.as_ref().map(|l| l.line),
                },
            },
            #[cfg(feature = "jsp")]
            Node::JspPage {
                path,
                display_name,
                url_pattern,
            } => NodeJson {
                id: idx.index(),
                kind: NodeKindJson::JspPage {
                    file: path.to_string_lossy().to_string(),
                    display_name: display_name.clone(),
                    url_pattern: url_pattern.clone(),
                },
            },
            #[cfg(feature = "jsp")]
            Node::JspSql {
                sql,
                file,
                line,
                kind,
                parsed,
            } => NodeJson {
                id: idx.index(),
                kind: NodeKindJson::JspSql {
                    sql: sql.clone(),
                    file: file.to_string_lossy().to_string(),
                    line: *line,
                    kind: kind.as_str().to_string(),
                    parsed: *parsed,
                },
            },
        };
        nodes.push(node_json);
    }

    let mut edges = Vec::new();
    for edge_idx in graph.edge_indices() {
        let (src, dst) = graph
            .edge_endpoints(edge_idx)
            .expect("edge should have endpoints");
        let edge_json = match &graph[edge_idx] {
            Edge::DirectCall { scope, location } => EdgeJson {
                source: src.index(),
                target: dst.index(),
                kind: EdgeKindJson::Direct {
                    scope: match scope {
                        crate::graph::CallScope::IntraPackage => "intra".to_string(),
                        crate::graph::CallScope::CrossPackage => "cross".to_string(),
                        crate::graph::CallScope::External => "external".to_string(),
                    },
                    file: location.file.to_string_lossy().to_string(),
                    line: location.line,
                },
            },
            Edge::DynamicCall { raw_expr, location } => EdgeJson {
                source: src.index(),
                target: dst.index(),
                kind: EdgeKindJson::Dynamic {
                    raw_expr: raw_expr.clone(),
                    file: location.file.to_string_lossy().to_string(),
                    line: location.line,
                },
            },
            Edge::CallsProcedure { location } => EdgeJson {
                source: src.index(),
                target: dst.index(),
                kind: EdgeKindJson::CallsProcedure {
                    file: location.file.to_string_lossy().to_string(),
                    line: location.line,
                },
            },
            Edge::InvokesMapper { location } => EdgeJson {
                source: src.index(),
                target: dst.index(),
                kind: EdgeKindJson::InvokesMapper {
                    file: location.file.to_string_lossy().to_string(),
                    line: location.line,
                },
            },
            Edge::CallsJava { location } => EdgeJson {
                source: src.index(),
                target: dst.index(),
                kind: EdgeKindJson::CallsJava {
                    file: location.file.to_string_lossy().to_string(),
                    line: location.line,
                },
            },
            Edge::UsesBuiltinFunction { location } => EdgeJson {
                source: src.index(),
                target: dst.index(),
                kind: EdgeKindJson::UsesBuiltinFunction {
                    file: location.file.to_string_lossy().to_string(),
                    line: location.line,
                },
            },
            Edge::ContainsMethod => EdgeJson {
                source: src.index(),
                target: dst.index(),
                kind: EdgeKindJson::ContainsMethod,
            },
            Edge::Extends { location } => EdgeJson {
                source: src.index(),
                target: dst.index(),
                kind: EdgeKindJson::Extends {
                    file: location.file.to_string_lossy().to_string(),
                    line: location.line,
                },
            },
            Edge::Implements { location } => EdgeJson {
                source: src.index(),
                target: dst.index(),
                kind: EdgeKindJson::Implements {
                    file: location.file.to_string_lossy().to_string(),
                    line: location.line,
                },
            },
            Edge::TableAccess {
                flow_kind,
                modes,
                write_kinds,
                location,
                column_analysis,
            } => {
                let mode_strs: Vec<String> = [
                    (AccessMode::Read, "read"),
                    (AccessMode::Write, "write"),
                    (AccessMode::LockRead, "lock_read"),
                    (AccessMode::Truncate, "truncate"),
                ]
                .iter()
                .filter(|(flag, _)| modes.contains(*flag))
                .map(|(_, s)| s.to_string())
                .collect();

                let wk_strs: Vec<String> = write_kinds
                    .iter()
                    .map(|wk| match wk {
                        WriteKind::Insert => "insert",
                        WriteKind::InsertSelect => "insert_select",
                        WriteKind::Update => "update",
                        WriteKind::Delete => "delete",
                        WriteKind::MergeInsert => "merge_insert",
                        WriteKind::MergeUpdate => "merge_update",
                        WriteKind::MergeDelete => "merge_delete",
                        WriteKind::SelectInto => "select_into",
                        WriteKind::Truncate => "truncate",
                    })
                    .map(String::from)
                    .collect();

                EdgeJson {
                    source: src.index(),
                    target: dst.index(),
                    kind: EdgeKindJson::TableAccess {
                        flow_kind: match flow_kind {
                            crate::graph::DataFlowKind::DmlAccess => "dml".to_string(),
                            crate::graph::DataFlowKind::DefinitionDependency => {
                                "definition".to_string()
                            }
                        },
                        modes: mode_strs,
                        write_kinds: wk_strs,
                        file: location.file.to_string_lossy().to_string(),
                        line: location.line,
                        column_analysis: column_analysis.as_deref().cloned(),
                    },
                }
            }
            Edge::DependsOn { location } => EdgeJson {
                source: src.index(),
                target: dst.index(),
                kind: EdgeKindJson::DependsOn {
                    file: location.file.to_string_lossy().to_string(),
                    line: location.line,
                },
            },
            Edge::ContainsRoutine => EdgeJson {
                source: src.index(),
                target: dst.index(),
                kind: EdgeKindJson::ContainsRoutine,
            },
            Edge::TriggersRoutine { location } => EdgeJson {
                source: src.index(),
                target: dst.index(),
                kind: EdgeKindJson::TriggersRoutine {
                    file: location.file.to_string_lossy().to_string(),
                    line: location.line,
                },
            },
            Edge::ReferencesType { location } => EdgeJson {
                source: src.index(),
                target: dst.index(),
                kind: EdgeKindJson::ReferencesType {
                    file: location.file.to_string_lossy().to_string(),
                    line: location.line,
                },
            },
            Edge::UsesSequence { location } => EdgeJson {
                source: src.index(),
                target: dst.index(),
                kind: EdgeKindJson::UsesSequence {
                    file: location.file.to_string_lossy().to_string(),
                    line: location.line,
                },
            },
            Edge::IndexesTable { location } => EdgeJson {
                source: src.index(),
                target: dst.index(),
                kind: EdgeKindJson::IndexesTable {
                    file: location.file.to_string_lossy().to_string(),
                    line: location.line,
                },
            },
            Edge::AliasesObject { location } => EdgeJson {
                source: src.index(),
                target: dst.index(),
                kind: EdgeKindJson::AliasesObject {
                    file: location.file.to_string_lossy().to_string(),
                    line: location.line,
                },
            },
            Edge::CustomEdge {
                type_name,
                properties,
                location,
            } => EdgeJson {
                source: src.index(),
                target: dst.index(),
                kind: EdgeKindJson::CustomEdge {
                    custom_type: type_name.clone(),
                    properties: serde_json::to_value(properties).unwrap_or(serde_json::Value::Null),
                    file: location
                        .as_ref()
                        .map(|l| l.file.to_string_lossy().to_string()),
                    line: location.as_ref().map(|l| l.line),
                },
            },
            #[cfg(feature = "jsp")]
            Edge::ContainsSql => EdgeJson {
                source: src.index(),
                target: dst.index(),
                kind: EdgeKindJson::ContainsSql,
            },
        };
        edges.push(edge_json);
    }

    let output = GraphJson { nodes, edges };
    serde_json::to_string_pretty(&output).map_err(|e| crate::error::CodeWebError::ExportError {
        message: e.to_string(),
    })
}

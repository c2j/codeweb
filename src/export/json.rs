use crate::error::Result;
use crate::graph::{AccessMode, CodeGraph, Edge, Node, WriteKind};
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
    },
    JavaSql {
        class_name: Option<String>,
        method_name: Option<String>,
        extraction_method: String,
        file: String,
        line: usize,
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
    },
    View {
        schema: Option<String>,
        name: String,
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
    Direct { file: String, line: usize },
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
    #[serde(rename = "contains_method")]
    ContainsMethod,
    #[serde(rename = "extends")]
    Extends { file: String, line: usize },
    #[serde(rename = "implements")]
    Implements { file: String, line: usize },
    #[serde(rename = "table_access")]
    TableAccess {
        modes: Vec<String>,
        write_kinds: Vec<String>,
        file: String,
        line: usize,
    },
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
}

pub fn to_json(graph: &CodeGraph) -> Result<String> {
    let mut nodes = Vec::new();
    for idx in graph.node_indices() {
        let node_json = match &graph[idx] {
            Node::Procedure {
                id,
                location,
                partial,
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
                    raw_expr: raw_expr.clone(),
                    context: context.clone(),
                },
            },
            Node::MappedStatement {
                namespace,
                statement_id,
                kind,
                xml_file,
                line,
            } => NodeJson {
                id: idx.index(),
                kind: NodeKindJson::MappedStatement {
                    namespace: namespace.clone(),
                    statement_id: statement_id.clone(),
                    kind: kind.clone(),
                    file: xml_file.to_string_lossy().to_string(),
                    line: *line,
                },
            },
            Node::JavaSql {
                class_name,
                method_name,
                extraction_method,
                java_file,
                line,
            } => NodeJson {
                id: idx.index(),
                kind: NodeKindJson::JavaSql {
                    class_name: class_name.clone(),
                    method_name: method_name.clone(),
                    extraction_method: extraction_method.clone(),
                    file: java_file.to_string_lossy().to_string(),
                    line: *line,
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
            Node::Table { schema, name } => NodeJson {
                id: idx.index(),
                kind: NodeKindJson::Table {
                    schema: schema.clone(),
                    name: name.clone(),
                },
            },
            Node::View { schema, name } => NodeJson {
                id: idx.index(),
                kind: NodeKindJson::View {
                    schema: schema.clone(),
                    name: name.clone(),
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
                location,
            } => NodeJson {
                id: idx.index(),
                kind: NodeKindJson::Index {
                    name: name.clone(),
                    table_name: table_name.clone(),
                    unique: *unique,
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
        };
        nodes.push(node_json);
    }

    let mut edges = Vec::new();
    for edge_idx in graph.edge_indices() {
        let (src, dst) = graph
            .edge_endpoints(edge_idx)
            .expect("edge should have endpoints");
        let edge_json = match &graph[edge_idx] {
            Edge::DirectCall { location } => EdgeJson {
                source: src.index(),
                target: dst.index(),
                kind: EdgeKindJson::Direct {
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
                modes,
                write_kinds,
                location,
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
                        modes: mode_strs,
                        write_kinds: wk_strs,
                        file: location.file.to_string_lossy().to_string(),
                        line: location.line,
                    },
                }
            }
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
        };
        edges.push(edge_json);
    }

    let output = GraphJson { nodes, edges };
    serde_json::to_string_pretty(&output).map_err(|e| crate::error::CodeWebError::ExportError {
        message: e.to_string(),
    })
}

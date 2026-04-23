use crate::error::Result;
use crate::graph::{CodeGraph, Edge, Node};
use serde::Serialize;

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
    #[serde(rename = "references_table")]
    ReferencesTable { file: String, line: usize },
}

pub fn to_json(graph: &CodeGraph) -> Result<String> {
    let mut nodes = Vec::new();
    for idx in graph.node_indices() {
        let node_json = match &graph[idx] {
            Node::Procedure { id, location } => NodeJson {
                id: idx.index(),
                kind: NodeKindJson::Procedure {
                    name: id.name.clone(),
                    schema: id.schema.clone(),
                    file: location.file.to_string_lossy().to_string(),
                    line: location.line,
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
            Edge::ReferencesTable { location } => EdgeJson {
                source: src.index(),
                target: dst.index(),
                kind: EdgeKindJson::ReferencesTable {
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

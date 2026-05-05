use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use petgraph::graph::NodeIndex;

use crate::graph::{
    AccessMode, CallScope, CodeGraph, DataFlowKind, Edge, JsonMap, Node, RoutineId, RoutineKind,
    SourceLocation, WriteKind,
};
use crate::import::format::{CgefDocument, CgefEdge, CgefLocation, CgefNode};
use crate::import::path_mapper::PathMapper;
use crate::import::schema::SchemaRegistry;
use crate::import::validator::{is_standard_edge_type, is_standard_node_type};

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("node '{node_id}': missing required key field '{field}' for type '{node_type}'")]
    MissingKeyField {
        node_id: String,
        node_type: String,
        field: String,
    },
    #[error("node '{node_id}': unknown node type '{node_type}'")]
    UnknownNodeType { node_id: String, node_type: String },
    #[error("edge '{src}' -> '{tgt}': unknown edge type '{edge_type}'")]
    UnknownEdgeType {
        src: String,
        tgt: String,
        edge_type: String,
    },
    #[error("edge '{src}' -> '{tgt}': source node id not found")]
    SourceNotFound { src: String, tgt: String },
    #[error("edge '{src}' -> '{tgt}': target node id not found")]
    TargetNotFound { src: String, tgt: String },
    #[error("node '{node_id}': {message}")]
    InvalidNode { node_id: String, message: String },
    #[error("edge '{src}' -> '{tgt}': {message}")]
    InvalidEdge {
        src: String,
        tgt: String,
        message: String,
    },
}

impl From<crate::import::schema::SchemaError> for ParseError {
    fn from(err: crate::import::schema::SchemaError) -> Self {
        match err {
            crate::import::schema::SchemaError::MissingKeyField {
                node_id,
                type_name,
                field,
            } => ParseError::MissingKeyField {
                node_id,
                node_type: type_name,
                field,
            },
        }
    }
}

pub struct ParsedCgef {
    pub graph: CodeGraph,
    pub id_map: HashMap<String, NodeIndex>,
    pub errors: Vec<ParseError>,
}

pub struct CgefParser {
    path_mapper: PathMapper,
    schema_registry: SchemaRegistry,
}

impl CgefParser {
    pub fn new(path_mapper: PathMapper, schema_registry: SchemaRegistry) -> Self {
        Self {
            path_mapper,
            schema_registry,
        }
    }

    pub fn parse(&self, doc: CgefDocument) -> ParsedCgef {
        let mut graph = CodeGraph::new();
        let mut id_map: HashMap<String, NodeIndex> = HashMap::new();
        let mut errors = Vec::new();

        for cgef_node in &doc.nodes {
            match self.convert_node(cgef_node) {
                Ok(node) => {
                    let idx = graph.add_node(node);
                    id_map.insert(cgef_node.id.clone(), idx);
                }
                Err(e) => errors.push(e),
            }
        }

        for cgef_edge in &doc.edges {
            let Some(&src_idx) = id_map.get(&cgef_edge.source) else {
                errors.push(ParseError::SourceNotFound {
                    src: cgef_edge.source.clone(),
                    tgt: cgef_edge.target.clone(),
                });
                continue;
            };
            let Some(&dst_idx) = id_map.get(&cgef_edge.target) else {
                errors.push(ParseError::TargetNotFound {
                    src: cgef_edge.source.clone(),
                    tgt: cgef_edge.target.clone(),
                });
                continue;
            };
            match self.convert_edge(cgef_edge) {
                Ok(edge) => {
                    graph.add_edge(src_idx, dst_idx, edge);
                }
                Err(e) => errors.push(e),
            }
        }

        Self::infer_call_scopes(&mut graph);

        ParsedCgef {
            graph,
            id_map,
            errors,
        }
    }

    fn convert_node(&self, cgef: &CgefNode) -> Result<Node, ParseError> {
        let node_type = &cgef.node_type;

        if is_standard_node_type(node_type) {
            self.convert_standard_node(cgef)
        } else {
            self.convert_custom_node(cgef)
        }
    }

    fn convert_standard_node(&self, cgef: &CgefNode) -> Result<Node, ParseError> {
        let key = &cgef.key;
        let location = cgef.location.as_ref().map(|l| self.make_source_location(l));

        match cgef.node_type.as_str() {
            "procedure" => {
                let name = key_get_str(key, "name").ok_or_else(|| ParseError::MissingKeyField {
                    node_id: cgef.id.clone(),
                    node_type: "procedure".to_string(),
                    field: "name".to_string(),
                })?;
                let id = RoutineId {
                    schema: key_get_str(key, "schema").map(String::from),
                    package: key_get_str(key, "package").map(String::from),
                    name: name.to_string(),
                    kind: RoutineKind::Procedure,
                };
                let loc = location.ok_or_else(|| ParseError::InvalidNode {
                    node_id: cgef.id.clone(),
                    message: "procedure requires location".to_string(),
                })?;
                let partial = cgef
                    .properties
                    .as_ref()
                    .and_then(|p| p.get("partial"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                Ok(Node::Procedure {
                    id,
                    location: loc,
                    partial,
                })
            }
            "function" => {
                let name = key_get_str(key, "name").ok_or_else(|| ParseError::MissingKeyField {
                    node_id: cgef.id.clone(),
                    node_type: "function".to_string(),
                    field: "name".to_string(),
                })?;
                let id = RoutineId {
                    schema: key_get_str(key, "schema").map(String::from),
                    package: key_get_str(key, "package").map(String::from),
                    name: name.to_string(),
                    kind: RoutineKind::Function,
                };
                let loc = location.ok_or_else(|| ParseError::InvalidNode {
                    node_id: cgef.id.clone(),
                    message: "function requires location".to_string(),
                })?;
                let partial = cgef
                    .properties
                    .as_ref()
                    .and_then(|p| p.get("partial"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                Ok(Node::Function {
                    id,
                    location: loc,
                    partial,
                })
            }
            "table" => {
                let name = key_get_str(key, "name").ok_or_else(|| ParseError::MissingKeyField {
                    node_id: cgef.id.clone(),
                    node_type: "table".to_string(),
                    field: "name".to_string(),
                })?;
                Ok(Node::Table {
                    schema: key_get_str(key, "schema").map(String::from),
                    name: name.to_string(),
                    location: None,
                    columns: Box::new(vec![]),
                    partition_by: None,
                    distribute_by: None,
                    tablespace: None,
                    temporary: false,
                    unlogged: false,
                    ddl_source: None,
                })
            }
            "view" => {
                let name = key_get_str(key, "name").ok_or_else(|| ParseError::MissingKeyField {
                    node_id: cgef.id.clone(),
                    node_type: "view".to_string(),
                    field: "name".to_string(),
                })?;
                Ok(Node::View {
                    schema: key_get_str(key, "schema").map(String::from),
                    name: name.to_string(),
                    location: None,
                })
            }
            "mapped_statement" => {
                let namespace =
                    key_get_str(key, "namespace").ok_or_else(|| ParseError::MissingKeyField {
                        node_id: cgef.id.clone(),
                        node_type: "mapped_statement".to_string(),
                        field: "namespace".to_string(),
                    })?;
                let statement_id = key_get_str(key, "statement_id").ok_or_else(|| {
                    ParseError::MissingKeyField {
                        node_id: cgef.id.clone(),
                        node_type: "mapped_statement".to_string(),
                        field: "statement_id".to_string(),
                    }
                })?;
                let kind = key_get_str(key, "kind").unwrap_or("select").to_string();
                let xml_file = key_get_str(key, "xml_file")
                    .map(|s| self.path_mapper.map(s))
                    .unwrap_or_else(|| {
                        location
                            .as_ref()
                            .map(|l| l.file.to_path_buf())
                            .unwrap_or_default()
                    });
                let line = key_get_usize(key, "line")
                    .unwrap_or_else(|| cgef.location.as_ref().map(|l| l.line).unwrap_or(0));
                Ok(Node::MappedStatement {
                    namespace: namespace.to_string(),
                    statement_id: statement_id.to_string(),
                    kind,
                    xml_file,
                    line,
                })
            }
            "java_method" => {
                let fqn = key_get_str(key, "fqn").ok_or_else(|| ParseError::MissingKeyField {
                    node_id: cgef.id.clone(),
                    node_type: "java_method".to_string(),
                    field: "fqn".to_string(),
                })?;
                let class_fqn =
                    key_get_str(key, "class_fqn").ok_or_else(|| ParseError::MissingKeyField {
                        node_id: cgef.id.clone(),
                        node_type: "java_method".to_string(),
                        field: "class_fqn".to_string(),
                    })?;
                let name = key_get_str(key, "name").ok_or_else(|| ParseError::MissingKeyField {
                    node_id: cgef.id.clone(),
                    node_type: "java_method".to_string(),
                    field: "name".to_string(),
                })?;
                let signature = key_get_str(key, "signature").unwrap_or("").to_string();
                let file = key_get_str(key, "file")
                    .map(|s| self.path_mapper.map(s))
                    .unwrap_or_default();
                let line = key_get_usize(key, "line")
                    .unwrap_or_else(|| cgef.location.as_ref().map(|l| l.line).unwrap_or(0));
                Ok(Node::JavaMethod {
                    fqn: fqn.to_string(),
                    class_fqn: class_fqn.to_string(),
                    name: name.to_string(),
                    signature,
                    file,
                    line,
                })
            }
            "java_class" => {
                let fqn = key_get_str(key, "fqn").ok_or_else(|| ParseError::MissingKeyField {
                    node_id: cgef.id.clone(),
                    node_type: "java_class".to_string(),
                    field: "fqn".to_string(),
                })?;
                let name = key_get_str(key, "name").ok_or_else(|| ParseError::MissingKeyField {
                    node_id: cgef.id.clone(),
                    node_type: "java_class".to_string(),
                    field: "name".to_string(),
                })?;
                let package = key_get_str(key, "package").map(String::from);
                let file = key_get_str(key, "file")
                    .map(|s| self.path_mapper.map(s))
                    .unwrap_or_default();
                let line = key_get_usize(key, "line")
                    .unwrap_or_else(|| cgef.location.as_ref().map(|l| l.line).unwrap_or(0));
                Ok(Node::JavaClass {
                    fqn: fqn.to_string(),
                    name: name.to_string(),
                    package,
                    file,
                    line,
                })
            }
            "java_sql" => {
                let extraction_method = key_get_str(key, "extraction_method").ok_or_else(|| {
                    ParseError::MissingKeyField {
                        node_id: cgef.id.clone(),
                        node_type: "java_sql".to_string(),
                        field: "extraction_method".to_string(),
                    }
                })?;
                let class_name = key_get_str(key, "class_name").map(String::from);
                let method_name = key_get_str(key, "method_name").map(String::from);
                let java_file = key_get_str(key, "java_file")
                    .map(|s| self.path_mapper.map(s))
                    .unwrap_or_default();
                let line = key_get_usize(key, "line")
                    .unwrap_or_else(|| cgef.location.as_ref().map(|l| l.line).unwrap_or(0));
                Ok(Node::JavaSql {
                    class_name,
                    method_name,
                    extraction_method: extraction_method.to_string(),
                    java_file,
                    line,
                })
            }
            "package" => {
                let name = key_get_str(key, "name").ok_or_else(|| ParseError::MissingKeyField {
                    node_id: cgef.id.clone(),
                    node_type: "package".to_string(),
                    field: "name".to_string(),
                })?;
                let loc = location.ok_or_else(|| ParseError::InvalidNode {
                    node_id: cgef.id.clone(),
                    message: "package requires location".to_string(),
                })?;
                Ok(Node::Package {
                    schema: key_get_str(key, "schema").map(String::from),
                    name: name.to_string(),
                    location: loc,
                })
            }
            "trigger" => {
                let name = key_get_str(key, "name").ok_or_else(|| ParseError::MissingKeyField {
                    node_id: cgef.id.clone(),
                    node_type: "trigger".to_string(),
                    field: "name".to_string(),
                })?;
                let loc = location.ok_or_else(|| ParseError::InvalidNode {
                    node_id: cgef.id.clone(),
                    message: "trigger requires location".to_string(),
                })?;
                let table = key_get_str(key, "table")
                    .map(|t| vec![t.to_string()])
                    .unwrap_or_default();
                Ok(Node::Trigger {
                    name: name.to_string(),
                    table,
                    location: loc,
                })
            }
            "type" => {
                let name = key_get_str(key, "name").ok_or_else(|| ParseError::MissingKeyField {
                    node_id: cgef.id.clone(),
                    node_type: "type".to_string(),
                    field: "name".to_string(),
                })?;
                let loc = location.ok_or_else(|| ParseError::InvalidNode {
                    node_id: cgef.id.clone(),
                    message: "type requires location".to_string(),
                })?;
                Ok(Node::Type {
                    schema: key_get_str(key, "schema").map(String::from),
                    name: name.to_string(),
                    type_kind: key_get_str(key, "type_kind")
                        .unwrap_or("composite")
                        .to_string(),
                    location: loc,
                })
            }
            "sequence" => {
                let name = key_get_str(key, "name").ok_or_else(|| ParseError::MissingKeyField {
                    node_id: cgef.id.clone(),
                    node_type: "sequence".to_string(),
                    field: "name".to_string(),
                })?;
                let loc = location.ok_or_else(|| ParseError::InvalidNode {
                    node_id: cgef.id.clone(),
                    message: "sequence requires location".to_string(),
                })?;
                Ok(Node::Sequence {
                    schema: key_get_str(key, "schema").map(String::from),
                    name: name.to_string(),
                    location: loc,
                })
            }
            "index" => {
                let table_name =
                    key_get_str(key, "table_name").ok_or_else(|| ParseError::MissingKeyField {
                        node_id: cgef.id.clone(),
                        node_type: "index".to_string(),
                        field: "table_name".to_string(),
                    })?;
                let loc = location.ok_or_else(|| ParseError::InvalidNode {
                    node_id: cgef.id.clone(),
                    message: "index requires location".to_string(),
                })?;
                Ok(Node::Index {
                    name: key_get_str(key, "name").map(String::from),
                    table_schema: key_get_str(key, "table_schema").map(String::from),
                    table_name: table_name.to_string(),
                    unique: key_get_bool(key, "unique").unwrap_or(false),
                    global: key_get_bool(key, "global").unwrap_or(false),
                    location: loc,
                })
            }
            "materialized_view" => {
                let name = key_get_str(key, "name").ok_or_else(|| ParseError::MissingKeyField {
                    node_id: cgef.id.clone(),
                    node_type: "materialized_view".to_string(),
                    field: "name".to_string(),
                })?;
                let loc = location.ok_or_else(|| ParseError::InvalidNode {
                    node_id: cgef.id.clone(),
                    message: "materialized_view requires location".to_string(),
                })?;
                Ok(Node::MaterializedView {
                    schema: key_get_str(key, "schema").map(String::from),
                    name: name.to_string(),
                    location: loc,
                })
            }
            "synonym" => {
                let name = key_get_str(key, "name").ok_or_else(|| ParseError::MissingKeyField {
                    node_id: cgef.id.clone(),
                    node_type: "synonym".to_string(),
                    field: "name".to_string(),
                })?;
                let target_name =
                    key_get_str(key, "target_name").ok_or_else(|| ParseError::MissingKeyField {
                        node_id: cgef.id.clone(),
                        node_type: "synonym".to_string(),
                        field: "target_name".to_string(),
                    })?;
                let loc = location.ok_or_else(|| ParseError::InvalidNode {
                    node_id: cgef.id.clone(),
                    message: "synonym requires location".to_string(),
                })?;
                Ok(Node::Synonym {
                    schema: key_get_str(key, "schema").map(String::from),
                    name: name.to_string(),
                    target_schema: key_get_str(key, "target_schema").map(String::from),
                    target_name: target_name.to_string(),
                    location: loc,
                })
            }
            "event" => {
                let name = key_get_str(key, "name").ok_or_else(|| ParseError::MissingKeyField {
                    node_id: cgef.id.clone(),
                    node_type: "event".to_string(),
                    field: "name".to_string(),
                })?;
                let loc = location.ok_or_else(|| ParseError::InvalidNode {
                    node_id: cgef.id.clone(),
                    message: "event requires location".to_string(),
                })?;
                Ok(Node::Event {
                    name: name.to_string(),
                    location: loc,
                })
            }
            "unresolved" => {
                let raw_expr =
                    key_get_str(key, "raw_expr").ok_or_else(|| ParseError::MissingKeyField {
                        node_id: cgef.id.clone(),
                        node_type: "unresolved".to_string(),
                        field: "raw_expr".to_string(),
                    })?;
                let context =
                    key_get_str(key, "context").ok_or_else(|| ParseError::MissingKeyField {
                        node_id: cgef.id.clone(),
                        node_type: "unresolved".to_string(),
                        field: "context".to_string(),
                    })?;
                Ok(Node::Unresolved {
                    raw_expr: Box::new(raw_expr.to_string()),
                    context: Box::new(context.to_string()),
                })
            }
            other => Err(ParseError::UnknownNodeType {
                node_id: cgef.id.clone(),
                node_type: other.to_string(),
            }),
        }
    }

    fn convert_custom_node(&self, cgef: &CgefNode) -> Result<Node, ParseError> {
        let type_name = cgef.node_type.clone();

        self.schema_registry
            .validate_custom_node_keys(&cgef.id, &type_name, &cgef.key)?;

        let key_fields = json_value_to_string_map(&cgef.key);
        let label = key_fields
            .values()
            .next()
            .cloned()
            .unwrap_or_else(|| type_name.clone());
        let properties = cgef
            .properties
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let location = cgef.location.as_ref().map(|l| self.make_source_location(l));

        Ok(Node::Custom {
            type_name: Box::new(type_name),
            label: Box::new(label),
            key_fields: Box::new(key_fields),
            properties: Box::new(JsonMap(properties)),
            location,
        })
    }

    fn convert_edge(&self, cgef: &CgefEdge) -> Result<Edge, ParseError> {
        let edge_type = &cgef.edge_type;
        let location = cgef.location.as_ref().map(|l| self.make_source_location(l));

        if is_standard_edge_type(edge_type) {
            self.convert_standard_edge(cgef, location)
        } else {
            self.convert_custom_edge(cgef, location)
        }
    }

    fn convert_standard_edge(
        &self,
        cgef: &CgefEdge,
        location: Option<SourceLocation>,
    ) -> Result<Edge, ParseError> {
        match cgef.edge_type.as_str() {
            "direct" | "intra_call" | "cross_call" => {
                let scope = parse_call_scope(cgef.properties.as_ref(), cgef.edge_type.as_str());
                Ok(Edge::DirectCall {
                    scope,
                    location: location.unwrap_or_else(dummy_location),
                })
            }
            "dynamic" => {
                let raw_expr = cgef
                    .properties
                    .as_ref()
                    .and_then(|p| p.get("raw_expr"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                Ok(Edge::DynamicCall {
                    raw_expr,
                    location: location.unwrap_or_else(dummy_location),
                })
            }
            "calls_procedure" => Ok(Edge::CallsProcedure {
                location: location.unwrap_or_else(dummy_location),
            }),
            "invokes_mapper" => Ok(Edge::InvokesMapper {
                location: location.unwrap_or_else(dummy_location),
            }),
            "calls_java" => Ok(Edge::CallsJava {
                location: location.unwrap_or_else(dummy_location),
            }),
            "contains_method" => Ok(Edge::ContainsMethod),
            "extends" => Ok(Edge::Extends {
                location: location.unwrap_or_else(dummy_location),
            }),
            "implements" => Ok(Edge::Implements {
                location: location.unwrap_or_else(dummy_location),
            }),
            "table_access" => {
                let modes = parse_access_modes(cgef.properties.as_ref());
                let write_kinds = parse_write_kinds(cgef.properties.as_ref());
                let flow_kind = parse_flow_kind(cgef.properties.as_ref());
                Ok(Edge::TableAccess {
                    flow_kind,
                    modes,
                    write_kinds,
                    location: location.unwrap_or_else(dummy_location),
                })
            }
            "depends_on" => Ok(Edge::DependsOn {
                location: location.unwrap_or_else(dummy_location),
            }),
            "contains_routine" => Ok(Edge::ContainsRoutine),
            "triggers_routine" => Ok(Edge::TriggersRoutine {
                location: location.unwrap_or_else(dummy_location),
            }),
            "references_type" => Ok(Edge::ReferencesType {
                location: location.unwrap_or_else(dummy_location),
            }),
            "uses_sequence" => Ok(Edge::UsesSequence {
                location: location.unwrap_or_else(dummy_location),
            }),
            "indexes_table" => Ok(Edge::IndexesTable {
                location: location.unwrap_or_else(dummy_location),
            }),
            "aliases_object" => Ok(Edge::AliasesObject {
                location: location.unwrap_or_else(dummy_location),
            }),
            other => Err(ParseError::UnknownEdgeType {
                src: cgef.source.clone(),
                tgt: cgef.target.clone(),
                edge_type: other.to_string(),
            }),
        }
    }

    fn convert_custom_edge(
        &self,
        cgef: &CgefEdge,
        location: Option<SourceLocation>,
    ) -> Result<Edge, ParseError> {
        let type_name = cgef.edge_type.clone();
        let properties = cgef
            .properties
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        Ok(Edge::CustomEdge {
            type_name,
            properties: JsonMap(properties),
            location,
        })
    }

    fn make_source_location(&self, loc: &CgefLocation) -> SourceLocation {
        let file = self.path_mapper.map(&loc.file);
        SourceLocation {
            file: Arc::new(file),
            line: loc.line,
        }
    }

    fn infer_call_scopes(graph: &mut CodeGraph) {
        let edge_indices: Vec<_> = graph.edge_indices().collect();
        for edge_idx in edge_indices {
            if let Edge::DirectCall {
                scope: CallScope::External,
                ..
            } = &graph[edge_idx]
            {
                let (src, dst) = graph.edge_endpoints(edge_idx).unwrap();
                let caller_id = extract_routine_id_from_node(&graph[src]);
                let callee_id = extract_routine_id_from_node(&graph[dst]);
                if let (Some(caller), Some(callee)) = (caller_id, callee_id) {
                    let inferred = crate::graph::determine_call_scope(&caller, &callee);
                    if let Edge::DirectCall { scope, .. } = &mut graph[edge_idx] {
                        *scope = inferred;
                    }
                }
            }
        }
    }
}

fn dummy_location() -> SourceLocation {
    SourceLocation {
        file: Arc::new(PathBuf::new()),
        line: 0,
    }
}

fn extract_routine_id_from_node(node: &Node) -> Option<RoutineId> {
    match node {
        Node::Procedure { id, .. } | Node::Function { id, .. } => Some(id.clone()),
        _ => None,
    }
}

fn key_get_str<'a>(key: &'a serde_json::Value, field: &str) -> Option<&'a str> {
    key.get(field).and_then(|v| v.as_str())
}

fn key_get_usize(key: &serde_json::Value, field: &str) -> Option<usize> {
    key.get(field).and_then(|v| v.as_u64()).map(|v| v as usize)
}

fn key_get_bool(key: &serde_json::Value, field: &str) -> Option<bool> {
    key.get(field).and_then(|v| v.as_bool())
}

fn json_value_to_string_map(value: &serde_json::Value) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    if let Some(obj) = value.as_object() {
        for (k, v) in obj {
            let s = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            map.insert(k.clone(), s);
        }
    }
    map
}

fn parse_access_modes(props: Option<&serde_json::Value>) -> AccessMode {
    let mut modes = AccessMode::empty();
    if let Some(arr) = props
        .and_then(|p| p.get("modes"))
        .and_then(|v| v.as_array())
    {
        for mode in arr {
            if let Some(s) = mode.as_str() {
                match s {
                    "read" => modes |= AccessMode::Read,
                    "write" => modes |= AccessMode::Write,
                    "lock_read" => modes |= AccessMode::LockRead,
                    "truncate" => modes |= AccessMode::Truncate,
                    _ => {}
                }
            }
        }
    }
    modes
}

fn parse_write_kinds(props: Option<&serde_json::Value>) -> std::collections::HashSet<WriteKind> {
    let mut kinds = std::collections::HashSet::new();
    if let Some(arr) = props
        .and_then(|p| p.get("write_kinds"))
        .and_then(|v| v.as_array())
    {
        for kind in arr {
            if let Some(s) = kind.as_str() {
                match s {
                    "insert" => _ = kinds.insert(WriteKind::Insert),
                    "insert_select" => _ = kinds.insert(WriteKind::InsertSelect),
                    "update" => _ = kinds.insert(WriteKind::Update),
                    "delete" => _ = kinds.insert(WriteKind::Delete),
                    "merge_insert" => _ = kinds.insert(WriteKind::MergeInsert),
                    "merge_update" => _ = kinds.insert(WriteKind::MergeUpdate),
                    "merge_delete" => _ = kinds.insert(WriteKind::MergeDelete),
                    "select_into" => _ = kinds.insert(WriteKind::SelectInto),
                    "truncate" => _ = kinds.insert(WriteKind::Truncate),
                    _ => {}
                }
            }
        }
    }
    kinds
}

fn parse_call_scope(props: Option<&serde_json::Value>, edge_type: &str) -> CallScope {
    if let Some(scope_str) = props.and_then(|p| p.get("scope")).and_then(|v| v.as_str()) {
        match scope_str {
            "intra" => return CallScope::IntraPackage,
            "cross" => return CallScope::CrossPackage,
            _ => return CallScope::External,
        }
    }
    match edge_type {
        "intra_call" => CallScope::IntraPackage,
        "cross_call" => CallScope::CrossPackage,
        _ => CallScope::External,
    }
}

fn parse_flow_kind(props: Option<&serde_json::Value>) -> DataFlowKind {
    match props
        .and_then(|p| p.get("flow_kind"))
        .and_then(|v| v.as_str())
    {
        Some("definition") => DataFlowKind::DefinitionDependency,
        _ => DataFlowKind::DmlAccess,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::format::*;
    use std::collections::HashMap;

    fn make_parser() -> CgefParser {
        CgefParser::new(
            PathMapper::new(None),
            SchemaRegistry::from_document(&CgefDocument {
                format_version: 1,
                metadata: CgefMetadata {
                    source: "test".to_string(),
                    generated_at: "2026-01-01T00:00:00Z".to_string(),
                    description: None,
                },
                node_schemas: HashMap::from([(
                    "dubbo_service".to_string(),
                    CgefNodeSchema {
                        display_name: Some("Dubbo".to_string()),
                        key_fields: vec!["interface".to_string()],
                        properties: HashMap::new(),
                    },
                )]),
                edge_schemas: HashMap::new(),
                nodes: vec![],
                edges: vec![],
            }),
        )
    }

    #[test]
    fn test_parse_procedure() {
        let parser = make_parser();
        let doc = CgefDocument {
            format_version: 1,
            metadata: CgefMetadata {
                source: "test".to_string(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
                description: None,
            },
            node_schemas: HashMap::new(),
            edge_schemas: HashMap::new(),
            nodes: vec![CgefNode {
                id: "n1".to_string(),
                node_type: "procedure".to_string(),
                key: serde_json::json!({"schema": "pkg", "name": "do_work"}),
                location: Some(CgefLocation {
                    file: "a.sql".to_string(),
                    line: 1,
                }),
                properties: None,
            }],
            edges: vec![],
        };
        let result = parser.parse(doc);
        assert!(result.errors.is_empty());
        assert_eq!(result.graph.node_count(), 1);
        if let Node::Procedure { id, .. } = &result.graph[NodeIndex::new(0)] {
            assert_eq!(id.name, "do_work");
            assert_eq!(id.schema.as_deref(), Some("pkg"));
        } else {
            panic!("Expected Procedure node");
        }
    }

    #[test]
    fn test_parse_custom_node() {
        let parser = make_parser();
        let doc = CgefDocument {
            format_version: 1,
            metadata: CgefMetadata {
                source: "test".to_string(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
                description: None,
            },
            node_schemas: HashMap::from([(
                "dubbo_service".to_string(),
                CgefNodeSchema {
                    display_name: Some("Dubbo".to_string()),
                    key_fields: vec!["interface".to_string()],
                    properties: HashMap::new(),
                },
            )]),
            edge_schemas: HashMap::new(),
            nodes: vec![CgefNode {
                id: "n1".to_string(),
                node_type: "dubbo_service".to_string(),
                key: serde_json::json!({"interface": "com.example.Svc"}),
                location: Some(CgefLocation {
                    file: "svc.java".to_string(),
                    line: 10,
                }),
                properties: Some(serde_json::json!({"version": "2.0"})),
            }],
            edges: vec![],
        };
        let result = parser.parse(doc);
        assert!(result.errors.is_empty());
        if let Node::Custom {
            type_name,
            label,
            properties,
            ..
        } = &result.graph[NodeIndex::new(0)]
        {
            assert_eq!(type_name.as_ref(), "dubbo_service");
            assert_eq!(label.as_ref(), "com.example.Svc");
            assert_eq!(properties.0.get("version").unwrap().as_str(), Some("2.0"));
        } else {
            panic!("Expected Custom node");
        }
    }

    #[test]
    fn test_parse_table_access_edge() {
        let parser = make_parser();
        let doc = CgefDocument {
            format_version: 1,
            metadata: CgefMetadata {
                source: "test".to_string(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
                description: None,
            },
            node_schemas: HashMap::new(),
            edge_schemas: HashMap::new(),
            nodes: vec![
                CgefNode {
                    id: "n1".to_string(),
                    node_type: "procedure".to_string(),
                    key: serde_json::json!({"name": "sp"}),
                    location: Some(CgefLocation {
                        file: "a.sql".to_string(),
                        line: 1,
                    }),
                    properties: None,
                },
                CgefNode {
                    id: "n2".to_string(),
                    node_type: "table".to_string(),
                    key: serde_json::json!({"name": "orders"}),
                    location: None,
                    properties: None,
                },
            ],
            edges: vec![CgefEdge {
                source: "n1".to_string(),
                target: "n2".to_string(),
                edge_type: "table_access".to_string(),
                location: Some(CgefLocation {
                    file: "a.sql".to_string(),
                    line: 5,
                }),
                properties: Some(serde_json::json!({
                    "modes": ["read", "write"],
                    "write_kinds": ["insert", "update"]
                })),
            }],
        };
        let result = parser.parse(doc);
        assert!(result.errors.is_empty());
        assert_eq!(result.graph.edge_count(), 1);
        let edge_idx = result.graph.edge_indices().next().unwrap();
        if let Edge::TableAccess {
            flow_kind,
            modes,
            write_kinds,
            ..
        } = &result.graph[edge_idx]
        {
            assert!(matches!(flow_kind, DataFlowKind::DmlAccess));
            assert!(modes.contains(AccessMode::Read));
            assert!(modes.contains(AccessMode::Write));
            assert!(write_kinds.contains(&WriteKind::Insert));
            assert!(write_kinds.contains(&WriteKind::Update));
        } else {
            panic!("Expected TableAccess edge");
        }
    }

    #[test]
    fn test_parse_custom_edge() {
        let parser = make_parser();
        let doc = CgefDocument {
            format_version: 1,
            metadata: CgefMetadata {
                source: "test".to_string(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
                description: None,
            },
            node_schemas: HashMap::from([(
                "dubbo_service".to_string(),
                CgefNodeSchema {
                    display_name: Some("Dubbo".to_string()),
                    key_fields: vec!["interface".to_string()],
                    properties: HashMap::new(),
                },
            )]),
            edge_schemas: HashMap::from([(
                "dubbo_invokes".to_string(),
                CgefEdgeSchema {
                    display_name: Some("Dubbo Invoke".to_string()),
                    source_types: vec![],
                    target_types: vec![],
                    properties: HashMap::new(),
                },
            )]),
            nodes: vec![
                CgefNode {
                    id: "n1".to_string(),
                    node_type: "procedure".to_string(),
                    key: serde_json::json!({"name": "sp"}),
                    location: Some(CgefLocation {
                        file: "a.sql".to_string(),
                        line: 1,
                    }),
                    properties: None,
                },
                CgefNode {
                    id: "n2".to_string(),
                    node_type: "dubbo_service".to_string(),
                    key: serde_json::json!({"interface": "svc"}),
                    location: None,
                    properties: None,
                },
            ],
            edges: vec![CgefEdge {
                source: "n1".to_string(),
                target: "n2".to_string(),
                edge_type: "dubbo_invokes".to_string(),
                location: None,
                properties: Some(serde_json::json!({"timeout": 5000})),
            }],
        };
        let result = parser.parse(doc);
        assert!(result.errors.is_empty());
        let edge_idx = result.graph.edge_indices().next().unwrap();
        if let Edge::CustomEdge {
            type_name,
            properties,
            ..
        } = &result.graph[edge_idx]
        {
            assert_eq!(type_name, "dubbo_invokes");
            assert_eq!(properties.0.get("timeout").unwrap().as_i64(), Some(5000));
        } else {
            panic!("Expected CustomEdge");
        }
    }

    #[test]
    fn test_parse_dangling_edge_error() {
        let parser = make_parser();
        let doc = CgefDocument {
            format_version: 1,
            metadata: CgefMetadata {
                source: "test".to_string(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
                description: None,
            },
            node_schemas: HashMap::new(),
            edge_schemas: HashMap::new(),
            nodes: vec![CgefNode {
                id: "n1".to_string(),
                node_type: "procedure".to_string(),
                key: serde_json::json!({"name": "sp"}),
                location: Some(CgefLocation {
                    file: "a.sql".to_string(),
                    line: 1,
                }),
                properties: None,
            }],
            edges: vec![CgefEdge {
                source: "n1".to_string(),
                target: "nonexistent".to_string(),
                edge_type: "direct".to_string(),
                location: None,
                properties: None,
            }],
        };
        let result = parser.parse(doc);
        assert!(!result.errors.is_empty());
        assert!(result
            .errors
            .iter()
            .any(|e| matches!(e, ParseError::TargetNotFound { .. })));
    }

    #[test]
    fn test_parse_missing_key_field_error() {
        let parser = make_parser();
        let doc = CgefDocument {
            format_version: 1,
            metadata: CgefMetadata {
                source: "test".to_string(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
                description: None,
            },
            node_schemas: HashMap::new(),
            edge_schemas: HashMap::new(),
            nodes: vec![CgefNode {
                id: "n1".to_string(),
                node_type: "procedure".to_string(),
                key: serde_json::json!({}),
                location: Some(CgefLocation {
                    file: "a.sql".to_string(),
                    line: 1,
                }),
                properties: None,
            }],
            edges: vec![],
        };
        let result = parser.parse(doc);
        assert!(!result.errors.is_empty());
    }

    #[test]
    fn test_parse_all_standard_node_types() {
        let parser = make_parser();
        let nodes = vec![
            CgefNode {
                id: "proc".into(),
                node_type: "procedure".into(),
                key: serde_json::json!({"name":"p"}),
                location: Some(CgefLocation {
                    file: "a.sql".into(),
                    line: 1,
                }),
                properties: None,
            },
            CgefNode {
                id: "func".into(),
                node_type: "function".into(),
                key: serde_json::json!({"name":"f"}),
                location: Some(CgefLocation {
                    file: "a.sql".into(),
                    line: 2,
                }),
                properties: None,
            },
            CgefNode {
                id: "tbl".into(),
                node_type: "table".into(),
                key: serde_json::json!({"name":"t"}),
                location: None,
                properties: None,
            },
            CgefNode {
                id: "view".into(),
                node_type: "view".into(),
                key: serde_json::json!({"name":"v"}),
                location: None,
                properties: None,
            },
            CgefNode {
                id: "pkg".into(),
                node_type: "package".into(),
                key: serde_json::json!({"name":"pk"}),
                location: Some(CgefLocation {
                    file: "a.sql".into(),
                    line: 3,
                }),
                properties: None,
            },
            CgefNode {
                id: "trg".into(),
                node_type: "trigger".into(),
                key: serde_json::json!({"name":"tr"}),
                location: Some(CgefLocation {
                    file: "a.sql".into(),
                    line: 4,
                }),
                properties: None,
            },
            CgefNode {
                id: "typ".into(),
                node_type: "type".into(),
                key: serde_json::json!({"name":"tp"}),
                location: Some(CgefLocation {
                    file: "a.sql".into(),
                    line: 5,
                }),
                properties: None,
            },
            CgefNode {
                id: "seq".into(),
                node_type: "sequence".into(),
                key: serde_json::json!({"name":"sq"}),
                location: Some(CgefLocation {
                    file: "a.sql".into(),
                    line: 6,
                }),
                properties: None,
            },
            CgefNode {
                id: "idx".into(),
                node_type: "index".into(),
                key: serde_json::json!({"table_name":"t"}),
                location: Some(CgefLocation {
                    file: "a.sql".into(),
                    line: 7,
                }),
                properties: None,
            },
            CgefNode {
                id: "mv".into(),
                node_type: "materialized_view".into(),
                key: serde_json::json!({"name":"mv"}),
                location: Some(CgefLocation {
                    file: "a.sql".into(),
                    line: 8,
                }),
                properties: None,
            },
            CgefNode {
                id: "syn".into(),
                node_type: "synonym".into(),
                key: serde_json::json!({"name":"sy", "target_name":"t2"}),
                location: Some(CgefLocation {
                    file: "a.sql".into(),
                    line: 9,
                }),
                properties: None,
            },
            CgefNode {
                id: "evt".into(),
                node_type: "event".into(),
                key: serde_json::json!({"name":"ev"}),
                location: Some(CgefLocation {
                    file: "a.sql".into(),
                    line: 10,
                }),
                properties: None,
            },
            CgefNode {
                id: "ms".into(),
                node_type: "mapped_statement".into(),
                key: serde_json::json!({"namespace":"ns", "statement_id":"sid", "kind":"select"}),
                location: Some(CgefLocation {
                    file: "a.xml".into(),
                    line: 1,
                }),
                properties: None,
            },
            CgefNode {
                id: "jm".into(),
                node_type: "java_method".into(),
                key: serde_json::json!({"fqn":"com.Foo.bar", "class_fqn":"com.Foo", "name":"bar", "signature":"()V"}),
                location: Some(CgefLocation {
                    file: "Foo.java".into(),
                    line: 1,
                }),
                properties: None,
            },
            CgefNode {
                id: "jc".into(),
                node_type: "java_class".into(),
                key: serde_json::json!({"fqn":"com.Foo", "name":"Foo"}),
                location: Some(CgefLocation {
                    file: "Foo.java".into(),
                    line: 1,
                }),
                properties: None,
            },
            CgefNode {
                id: "js".into(),
                node_type: "java_sql".into(),
                key: serde_json::json!({"extraction_method":"annotation"}),
                location: Some(CgefLocation {
                    file: "Foo.java".into(),
                    line: 5,
                }),
                properties: None,
            },
            CgefNode {
                id: "unres".into(),
                node_type: "unresolved".into(),
                key: serde_json::json!({"raw_expr":"dynamic_sql", "context":"sp"}),
                location: None,
                properties: None,
            },
        ];
        let doc = CgefDocument {
            format_version: 1,
            metadata: CgefMetadata {
                source: "test".to_string(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
                description: None,
            },
            node_schemas: HashMap::new(),
            edge_schemas: HashMap::new(),
            nodes,
            edges: vec![],
        };
        let result = parser.parse(doc);
        assert!(result.errors.is_empty());
        assert_eq!(result.graph.node_count(), 17);
    }

    #[test]
    fn test_parse_all_standard_edge_types() {
        let parser = make_parser();
        let loc = CgefLocation {
            file: "a.sql".into(),
            line: 1,
        };
        let nodes = vec![
            CgefNode {
                id: "n1".into(),
                node_type: "procedure".into(),
                key: serde_json::json!({"name":"sp1"}),
                location: Some(loc.clone()),
                properties: None,
            },
            CgefNode {
                id: "n2".into(),
                node_type: "procedure".into(),
                key: serde_json::json!({"name":"sp2"}),
                location: Some(loc.clone()),
                properties: None,
            },
            CgefNode {
                id: "n3".into(),
                node_type: "table".into(),
                key: serde_json::json!({"name":"t1"}),
                location: None,
                properties: None,
            },
            CgefNode {
                id: "n4".into(),
                node_type: "mapped_statement".into(),
                key: serde_json::json!({"namespace":"ns","statement_id":"s","kind":"select"}),
                location: Some(loc.clone()),
                properties: None,
            },
            CgefNode {
                id: "n5".into(),
                node_type: "java_method".into(),
                key: serde_json::json!({"fqn":"a.b","class_fqn":"a","name":"b","signature":"()V"}),
                location: Some(loc.clone()),
                properties: None,
            },
            CgefNode {
                id: "n6".into(),
                node_type: "java_class".into(),
                key: serde_json::json!({"fqn":"a.A","name":"A"}),
                location: Some(loc.clone()),
                properties: None,
            },
            CgefNode {
                id: "n7".into(),
                node_type: "package".into(),
                key: serde_json::json!({"name":"pk"}),
                location: Some(loc.clone()),
                properties: None,
            },
            CgefNode {
                id: "n8".into(),
                node_type: "trigger".into(),
                key: serde_json::json!({"name":"tr"}),
                location: Some(loc.clone()),
                properties: None,
            },
            CgefNode {
                id: "n9".into(),
                node_type: "type".into(),
                key: serde_json::json!({"name":"tp"}),
                location: Some(loc.clone()),
                properties: None,
            },
            CgefNode {
                id: "n10".into(),
                node_type: "sequence".into(),
                key: serde_json::json!({"name":"sq"}),
                location: Some(loc.clone()),
                properties: None,
            },
            CgefNode {
                id: "n11".into(),
                node_type: "index".into(),
                key: serde_json::json!({"table_name":"t1"}),
                location: Some(loc.clone()),
                properties: None,
            },
            CgefNode {
                id: "n12".into(),
                node_type: "synonym".into(),
                key: serde_json::json!({"name":"sy","target_name":"x"}),
                location: Some(loc.clone()),
                properties: None,
            },
        ];
        let edges = vec![
            CgefEdge {
                source: "n1".into(),
                target: "n2".into(),
                edge_type: "direct".into(),
                location: Some(loc.clone()),
                properties: None,
            },
            CgefEdge {
                source: "n1".into(),
                target: "n2".into(),
                edge_type: "dynamic".into(),
                location: Some(loc.clone()),
                properties: Some(serde_json::json!({"raw_expr": "dyn_sql"})),
            },
            CgefEdge {
                source: "n4".into(),
                target: "n2".into(),
                edge_type: "calls_procedure".into(),
                location: Some(loc.clone()),
                properties: None,
            },
            CgefEdge {
                source: "n5".into(),
                target: "n4".into(),
                edge_type: "invokes_mapper".into(),
                location: Some(loc.clone()),
                properties: None,
            },
            CgefEdge {
                source: "n5".into(),
                target: "n5".into(),
                edge_type: "calls_java".into(),
                location: Some(loc.clone()),
                properties: None,
            },
            CgefEdge {
                source: "n6".into(),
                target: "n5".into(),
                edge_type: "contains_method".into(),
                location: None,
                properties: None,
            },
            CgefEdge {
                source: "n6".into(),
                target: "n6".into(),
                edge_type: "extends".into(),
                location: Some(loc.clone()),
                properties: None,
            },
            CgefEdge {
                source: "n6".into(),
                target: "n6".into(),
                edge_type: "implements".into(),
                location: Some(loc.clone()),
                properties: None,
            },
            CgefEdge {
                source: "n1".into(),
                target: "n3".into(),
                edge_type: "table_access".into(),
                location: Some(loc.clone()),
                properties: Some(serde_json::json!({"modes": ["read"]})),
            },
            CgefEdge {
                source: "n7".into(),
                target: "n2".into(),
                edge_type: "contains_routine".into(),
                location: None,
                properties: None,
            },
            CgefEdge {
                source: "n8".into(),
                target: "n2".into(),
                edge_type: "triggers_routine".into(),
                location: Some(loc.clone()),
                properties: None,
            },
            CgefEdge {
                source: "n1".into(),
                target: "n9".into(),
                edge_type: "references_type".into(),
                location: Some(loc.clone()),
                properties: None,
            },
            CgefEdge {
                source: "n1".into(),
                target: "n10".into(),
                edge_type: "uses_sequence".into(),
                location: Some(loc.clone()),
                properties: None,
            },
            CgefEdge {
                source: "n11".into(),
                target: "n3".into(),
                edge_type: "indexes_table".into(),
                location: Some(loc.clone()),
                properties: None,
            },
            CgefEdge {
                source: "n12".into(),
                target: "n3".into(),
                edge_type: "aliases_object".into(),
                location: Some(loc.clone()),
                properties: None,
            },
        ];
        let doc = CgefDocument {
            format_version: 1,
            metadata: CgefMetadata {
                source: "test".into(),
                generated_at: "2026-01-01T00:00:00Z".into(),
                description: None,
            },
            node_schemas: HashMap::new(),
            edge_schemas: HashMap::new(),
            nodes,
            edges,
        };
        let result = parser.parse(doc);
        assert!(result.errors.is_empty());
        assert_eq!(result.graph.edge_count(), 15);
    }

    #[test]
    fn test_path_mapping_applied() {
        let parser = CgefParser::new(
            PathMapper::new(Some("/prefix")),
            SchemaRegistry::from_document(&CgefDocument {
                format_version: 1,
                metadata: CgefMetadata {
                    source: "test".into(),
                    generated_at: "2026-01-01T00:00:00Z".into(),
                    description: None,
                },
                node_schemas: HashMap::new(),
                edge_schemas: HashMap::new(),
                nodes: vec![],
                edges: vec![],
            }),
        );
        let doc = CgefDocument {
            format_version: 1,
            metadata: CgefMetadata {
                source: "test".into(),
                generated_at: "2026-01-01T00:00:00Z".into(),
                description: None,
            },
            node_schemas: HashMap::new(),
            edge_schemas: HashMap::new(),
            nodes: vec![CgefNode {
                id: "n1".into(),
                node_type: "procedure".into(),
                key: serde_json::json!({"name":"sp"}),
                location: Some(CgefLocation {
                    file: "sql/a.sql".into(),
                    line: 1,
                }),
                properties: None,
            }],
            edges: vec![],
        };
        let result = parser.parse(doc);
        assert!(result.errors.is_empty());
        if let Node::Procedure { location, .. } = &result.graph[NodeIndex::new(0)] {
            assert_eq!(location.file.to_string_lossy(), "/prefix/sql/a.sql");
        }
    }
}

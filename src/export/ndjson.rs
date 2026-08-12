use crate::error::Result;
use crate::graph::{node_type_tag, CodeGraph, Edge, Node};
use serde::Serialize;
use std::io::Write;

/// Write the graph as NDJSON (Newline Delimited JSON).
///
/// Each node and edge is written as a single JSON object on its own line,
/// enabling streaming consumption without loading the entire graph into memory.
///
/// Node records have `"record": "node"` and include both `type_tag` (CLI short name)
/// and `type` (JSON long name) fields. Edge records have `"record": "edge"`.
pub fn to_ndjson(graph: &CodeGraph, writer: &mut impl Write) -> Result<()> {
    let mut buf = Vec::with_capacity(4096);

    // Write nodes
    for idx in graph.node_indices() {
        buf.clear();
        let node = &graph[idx];
        let record = NdjsonNodeRecord::from_node(idx.index(), node);
        serde_json::to_writer(&mut buf, &record).map_err(|e| {
            crate::error::CodeWebError::ExportError {
                message: e.to_string(),
            }
        })?;
        buf.push(b'\n');
        writer
            .write_all(&buf)
            .map_err(|e| crate::error::CodeWebError::ExportError {
                message: e.to_string(),
            })?;
    }

    // Write edges
    for edge_idx in graph.edge_indices() {
        buf.clear();
        if let Some((src, dst)) = graph.edge_endpoints(edge_idx) {
            let edge = &graph[edge_idx];
            let record = NdjsonEdgeRecord::from_edge(src.index(), dst.index(), edge);
            serde_json::to_writer(&mut buf, &record).map_err(|e| {
                crate::error::CodeWebError::ExportError {
                    message: e.to_string(),
                }
            })?;
            buf.push(b'\n');
            writer
                .write_all(&buf)
                .map_err(|e| crate::error::CodeWebError::ExportError {
                    message: e.to_string(),
                })?;
        }
    }

    Ok(())
}

#[derive(Serialize)]
struct NdjsonNodeRecord {
    record: &'static str,
    id: usize,
    #[serde(rename = "type_tag")]
    type_tag: &'static str,
    #[serde(rename = "type")]
    type_long: &'static str,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    schema: Option<String>,
}

impl NdjsonNodeRecord {
    fn from_node(id: usize, node: &Node) -> Self {
        let (name, schema) = node_name_and_schema(node);
        NdjsonNodeRecord {
            record: "node",
            id,
            type_tag: node_type_tag(node),
            type_long: node_json_type(node),
            name,
            schema,
        }
    }
}

#[derive(Serialize)]
struct NdjsonEdgeRecord {
    record: &'static str,
    source: usize,
    target: usize,
    #[serde(rename = "type")]
    edge_type: String,
}

impl NdjsonEdgeRecord {
    fn from_edge(source: usize, target: usize, edge: &Edge) -> Self {
        NdjsonEdgeRecord {
            record: "edge",
            source,
            target,
            edge_type: edge_json_type(edge).to_string(),
        }
    }
}

fn node_name_and_schema(node: &Node) -> (String, Option<String>) {
    match node {
        Node::Procedure { id, .. } => (id.name.clone(), id.schema.clone()),
        Node::Function { id, .. } => (id.name.clone(), id.schema.clone()),
        Node::Package { name, schema, .. } => (name.clone(), schema.clone()),
        Node::Table { name, schema, .. } => (name.clone(), schema.clone()),
        Node::View { name, schema, .. } => (name.clone(), schema.clone()),
        Node::MaterializedView { name, schema, .. } => (name.clone(), schema.clone()),
        Node::Trigger { name, .. } => (name.clone(), None),
        Node::Type { name, schema, .. } => (name.clone(), schema.clone()),
        Node::Sequence { name, schema, .. } => (name.clone(), schema.clone()),
        Node::Index { name, .. } => (name.clone().unwrap_or_default(), None),
        Node::Synonym { name, schema, .. } => (name.clone(), schema.clone()),
        Node::Event { name, .. } => (name.clone(), None),
        Node::BuiltinFunction { name, .. } => (name.clone(), None),
        Node::Unresolved { raw_expr, .. } => (raw_expr.to_string(), None),
        Node::MappedStatement {
            statement_id,
            namespace,
            ..
        } => (format!("{}.{}", namespace, statement_id), None),
        Node::JavaSql {
            class_name,
            method_name,
            ..
        } => {
            let name = match (class_name, method_name) {
                (Some(c), Some(m)) => format!("{}.{}", c, m),
                (Some(c), None) => c.clone(),
                (None, Some(m)) => m.clone(),
                (None, None) => String::new(),
            };
            (name, None)
        }
        Node::JavaMethod { fqn, .. } => (fqn.clone(), None),
        Node::JavaClass { fqn, .. } => (fqn.clone(), None),
        Node::Custom { label, .. } => (label.to_string(), None),
        #[cfg(feature = "jsp")]
        Node::JspPage { display_name, .. } => (display_name.clone(), None),
        #[cfg(feature = "jsp")]
        Node::JspSql { sql, .. } => {
            let preview: String = sql.chars().take(60).collect();
            (preview, None)
        }
        Node::Column { name, .. } => (name.clone(), None),
    }
}

fn node_json_type(node: &Node) -> &'static str {
    match node {
        Node::Procedure { .. } => "procedure",
        Node::Function { .. } => "function",
        Node::Unresolved { .. } => "unresolved",
        Node::MappedStatement { .. } => "mapped_statement",
        Node::JavaSql { .. } => "java_sql",
        Node::JavaMethod { .. } => "java_method",
        Node::JavaClass { .. } => "java_class",
        Node::Table { .. } => "table",
        Node::View { .. } => "view",
        Node::Package { .. } => "package",
        Node::Trigger { .. } => "trigger",
        Node::Type { .. } => "type",
        Node::Sequence { .. } => "sequence",
        Node::Index { .. } => "index",
        Node::MaterializedView { .. } => "materialized_view",
        Node::Synonym { .. } => "synonym",
        Node::Event { .. } => "event",
        Node::BuiltinFunction { .. } => "builtin_function",
        Node::Custom { .. } => "custom",
        #[cfg(feature = "jsp")]
        Node::JspPage { .. } => "jsp",
        #[cfg(feature = "jsp")]
        Node::JspSql { .. } => "jspsql",
        Node::Column { .. } => "column",
    }
}

fn edge_json_type(edge: &Edge) -> &str {
    match edge {
        Edge::DirectCall { .. } => "direct",
        Edge::DynamicCall { .. } => "dynamic",
        Edge::CallsProcedure { .. } => "calls_procedure",
        Edge::InvokesMapper { .. } => "invokes_mapper",
        Edge::CallsJava { .. } => "calls_java",
        Edge::UsesBuiltinFunction { .. } => "uses_builtin_function",
        Edge::ContainsMethod => "contains_method",
        #[cfg(feature = "jsp")]
        Edge::ContainsSql => "contains_sql",
        Edge::Extends { .. } => "extends",
        Edge::Implements { .. } => "implements",
        Edge::TableAccess { .. } => "table_access",
        Edge::DependsOn { .. } => "depends_on",
        Edge::ContainsRoutine => "contains_routine",
        Edge::TriggersRoutine { .. } => "triggers_routine",
        Edge::ReferencesType { .. } => "references_type",
        Edge::UsesSequence { .. } => "uses_sequence",
        Edge::IndexesTable { .. } => "indexes_table",
        Edge::AliasesObject { .. } => "aliases_object",
        Edge::CustomEdge { type_name, .. } => type_name.as_str(),
        Edge::DataFlow { .. } => "data_flow",
        Edge::Derived { .. } => "derived",
        Edge::Aggregated { .. } => "aggregated",
    }
}

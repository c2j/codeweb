use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const CURRENT_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CgefDocument {
    pub format_version: u32,
    pub metadata: CgefMetadata,
    #[serde(default)]
    pub node_schemas: HashMap<String, CgefNodeSchema>,
    #[serde(default)]
    pub edge_schemas: HashMap<String, CgefEdgeSchema>,
    pub nodes: Vec<CgefNode>,
    pub edges: Vec<CgefEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CgefMetadata {
    pub source: String,
    pub generated_at: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CgefNodeSchema {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub key_fields: Vec<String>,
    #[serde(default)]
    pub properties: HashMap<String, CgefPropertyDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CgefEdgeSchema {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub source_types: Vec<String>,
    #[serde(default)]
    pub target_types: Vec<String>,
    #[serde(default)]
    pub properties: HashMap<String, CgefPropertyDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CgefPropertyDef {
    #[serde(rename = "type", default)]
    pub prop_type: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CgefNode {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    pub key: serde_json::Value,
    #[serde(default)]
    pub location: Option<CgefLocation>,
    #[serde(default)]
    pub properties: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CgefEdge {
    pub source: String,
    pub target: String,
    #[serde(rename = "type")]
    pub edge_type: String,
    #[serde(default)]
    pub location: Option<CgefLocation>,
    #[serde(default)]
    pub properties: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CgefLocation {
    pub file: String,
    pub line: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_minimal_cgef_deserialize() {
        let json = r#"{
            "format_version": 1,
            "metadata": { "source": "test", "generated_at": "2026-01-01T00:00:00Z" },
            "nodes": [],
            "edges": []
        }"#;
        let doc: CgefDocument = serde_json::from_str(json).unwrap();
        assert_eq!(doc.format_version, 1);
        assert_eq!(doc.metadata.source, "test");
        assert!(doc.nodes.is_empty());
        assert!(doc.edges.is_empty());
    }

    #[test]
    fn test_full_cgef_deserialize() {
        let json = r#"{
            "format_version": 1,
            "metadata": { "source": "enterprise-tool", "generated_at": "2026-04-15T10:00:00Z", "description": "test graph" },
            "node_schemas": {
                "dubbo_service": { "display_name": "Dubbo Service", "key_fields": ["interface"] }
            },
            "edge_schemas": {
                "dubbo_invokes": { "display_name": "Dubbo Invocation" }
            },
            "nodes": [
                { "id": "n1", "type": "procedure", "key": {"name": "do_work"}, "location": {"file": "a.sql", "line": 1} },
                { "id": "n2", "type": "dubbo_service", "key": {"interface": "com.example.Svc"}, "properties": {"version": "2.0"} }
            ],
            "edges": [
                { "source": "n1", "target": "n2", "type": "dubbo_invokes", "properties": {"timeout": 5000} }
            ]
        }"#;
        let doc: CgefDocument = serde_json::from_str(json).unwrap();
        assert_eq!(doc.nodes.len(), 2);
        assert_eq!(doc.edges.len(), 1);
        assert!(doc.node_schemas.contains_key("dubbo_service"));
        assert!(doc.edge_schemas.contains_key("dubbo_invokes"));
        assert_eq!(doc.metadata.description.as_deref(), Some("test graph"));
    }

    #[test]
    fn test_missing_format_version_fails() {
        let json = r#"{
            "metadata": { "source": "test", "generated_at": "2026-01-01T00:00:00Z" },
            "nodes": [],
            "edges": []
        }"#;
        let result: Result<CgefDocument, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }
}

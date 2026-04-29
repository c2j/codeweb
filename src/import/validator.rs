use std::collections::HashSet;

use crate::import::format::{CgefDocument, CURRENT_FORMAT_VERSION};

#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("unsupported format version {found}, max supported is {max_supported}")]
    UnsupportedVersion { found: u32, max_supported: u32 },
    #[error("duplicate node id: {id}")]
    DuplicateNodeId { id: String },
    #[error("edge references non-existent node: {missing_ref} (from {src} -> {tgt})")]
    InvalidNodeReference {
        src: String,
        tgt: String,
        missing_ref: String,
    },
    #[error("custom node type '{type_name}' used but not declared in node_schemas")]
    UndeclaredNodeType { type_name: String },
    #[error("custom edge type '{type_name}' used but not declared in edge_schemas")]
    UndeclaredEdgeType { type_name: String },
}

#[derive(Debug)]
pub struct ValidationWarning {
    pub message: String,
}

pub struct ValidationReport {
    pub errors: Vec<ValidationError>,
    pub warnings: Vec<ValidationWarning>,
}

pub fn validate(doc: &CgefDocument) -> ValidationReport {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    if doc.format_version != CURRENT_FORMAT_VERSION {
        errors.push(ValidationError::UnsupportedVersion {
            found: doc.format_version,
            max_supported: CURRENT_FORMAT_VERSION,
        });
        return ValidationReport { errors, warnings };
    }

    if doc.nodes.is_empty() && doc.edges.is_empty() {
        warnings.push(ValidationWarning {
            message: "document contains no nodes and no edges".to_string(),
        });
    }

    let mut seen_ids: HashSet<&str> = HashSet::new();
    for node in &doc.nodes {
        if !seen_ids.insert(&node.id) {
            errors.push(ValidationError::DuplicateNodeId {
                id: node.id.clone(),
            });
        }
    }

    let id_set: HashSet<&str> = doc.nodes.iter().map(|n| n.id.as_str()).collect();
    for edge in &doc.edges {
        if !id_set.contains(edge.source.as_str()) {
            errors.push(ValidationError::InvalidNodeReference {
                src: edge.source.clone(),
                tgt: edge.target.clone(),
                missing_ref: edge.source.clone(),
            });
        }
        if !id_set.contains(edge.target.as_str()) {
            errors.push(ValidationError::InvalidNodeReference {
                src: edge.source.clone(),
                tgt: edge.target.clone(),
                missing_ref: edge.target.clone(),
            });
        }
    }

    let standard_node_types = standard_node_types();
    let standard_edge_types = standard_edge_types();

    for node in &doc.nodes {
        if !standard_node_types.contains(node.node_type.as_str())
            && !doc.node_schemas.contains_key(&node.node_type)
        {
            errors.push(ValidationError::UndeclaredNodeType {
                type_name: node.node_type.clone(),
            });
        }
    }

    for edge in &doc.edges {
        if !standard_edge_types.contains(edge.edge_type.as_str())
            && !doc.edge_schemas.contains_key(&edge.edge_type)
        {
            errors.push(ValidationError::UndeclaredEdgeType {
                type_name: edge.edge_type.clone(),
            });
        }
    }

    ValidationReport { errors, warnings }
}

fn standard_node_types() -> HashSet<&'static str> {
    [
        "procedure",
        "function",
        "package",
        "trigger",
        "type",
        "sequence",
        "index",
        "materialized_view",
        "synonym",
        "event",
        "table",
        "view",
        "mapped_statement",
        "java_sql",
        "java_method",
        "java_class",
        "unresolved",
    ]
    .into_iter()
    .collect()
}

fn standard_edge_types() -> HashSet<&'static str> {
    [
        "direct",
        "dynamic",
        "calls_procedure",
        "invokes_mapper",
        "calls_java",
        "contains_method",
        "extends",
        "implements",
        "table_access",
        "contains_routine",
        "triggers_routine",
        "references_type",
        "uses_sequence",
        "indexes_table",
        "aliases_object",
    ]
    .into_iter()
    .collect()
}

pub fn is_standard_node_type(type_name: &str) -> bool {
    standard_node_types().contains(type_name)
}

pub fn is_standard_edge_type(type_name: &str) -> bool {
    standard_edge_types().contains(type_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::format::*;
    use std::collections::HashMap;

    fn minimal_doc() -> CgefDocument {
        CgefDocument {
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
                key: serde_json::json!({"name": "test_proc"}),
                location: None,
                properties: None,
            }],
            edges: vec![],
        }
    }

    #[test]
    fn test_valid_document_passes() {
        let doc = minimal_doc();
        let report = validate(&doc);
        assert!(report.errors.is_empty());
    }

    #[test]
    fn test_unsupported_version() {
        let mut doc = minimal_doc();
        doc.format_version = 99;
        let report = validate(&doc);
        assert!(matches!(
            report.errors.first(),
            Some(ValidationError::UnsupportedVersion { .. })
        ));
    }

    #[test]
    fn test_duplicate_node_id() {
        let mut doc = minimal_doc();
        doc.nodes.push(CgefNode {
            id: "n1".to_string(),
            node_type: "table".to_string(),
            key: serde_json::json!({"name": "dup"}),
            location: None,
            properties: None,
        });
        let report = validate(&doc);
        assert!(matches!(
            report.errors.first(),
            Some(ValidationError::DuplicateNodeId { .. })
        ));
    }

    #[test]
    fn test_invalid_edge_reference() {
        let mut doc = minimal_doc();
        doc.edges.push(CgefEdge {
            source: "n1".to_string(),
            target: "nonexistent".to_string(),
            edge_type: "direct".to_string(),
            location: None,
            properties: None,
        });
        let report = validate(&doc);
        assert!(matches!(
            report.errors.first(),
            Some(ValidationError::InvalidNodeReference { .. })
        ));
    }

    #[test]
    fn test_undeclared_custom_node_type() {
        let mut doc = minimal_doc();
        doc.nodes.push(CgefNode {
            id: "n2".to_string(),
            node_type: "dubbo_service".to_string(),
            key: serde_json::json!({"interface": "x"}),
            location: None,
            properties: None,
        });
        let report = validate(&doc);
        assert!(report
            .errors
            .iter()
            .any(|e| matches!(e, ValidationError::UndeclaredNodeType { .. })));
    }

    #[test]
    fn test_undeclared_custom_edge_type() {
        let mut doc = minimal_doc();
        doc.nodes.push(CgefNode {
            id: "n2".to_string(),
            node_type: "table".to_string(),
            key: serde_json::json!({"name": "t"}),
            location: None,
            properties: None,
        });
        doc.edges.push(CgefEdge {
            source: "n1".to_string(),
            target: "n2".to_string(),
            edge_type: "custom_edge".to_string(),
            location: None,
            properties: None,
        });
        let report = validate(&doc);
        assert!(report
            .errors
            .iter()
            .any(|e| matches!(e, ValidationError::UndeclaredEdgeType { .. })));
    }

    #[test]
    fn test_declared_custom_types_pass() {
        let mut doc = minimal_doc();
        doc.node_schemas.insert(
            "dubbo_service".to_string(),
            CgefNodeSchema {
                display_name: Some("Dubbo".to_string()),
                key_fields: vec!["interface".to_string()],
                properties: HashMap::new(),
            },
        );
        doc.nodes.push(CgefNode {
            id: "n2".to_string(),
            node_type: "dubbo_service".to_string(),
            key: serde_json::json!({"interface": "x"}),
            location: None,
            properties: None,
        });
        let report = validate(&doc);
        assert!(report.errors.is_empty());
    }

    #[test]
    fn test_multiple_errors_collected() {
        let mut doc = minimal_doc();
        doc.nodes.push(CgefNode {
            id: "n1".to_string(),
            node_type: "table".to_string(),
            key: serde_json::json!({"name": "dup"}),
            location: None,
            properties: None,
        });
        doc.nodes.push(CgefNode {
            id: "n2".to_string(),
            node_type: "dubbo_service".to_string(),
            key: serde_json::json!({"interface": "x"}),
            location: None,
            properties: None,
        });
        doc.edges.push(CgefEdge {
            source: "n1".to_string(),
            target: "ghost".to_string(),
            edge_type: "direct".to_string(),
            location: None,
            properties: None,
        });
        let report = validate(&doc);
        assert!(report.errors.len() >= 3);
    }

    #[test]
    fn test_empty_document_warning() {
        let doc = CgefDocument {
            format_version: 1,
            metadata: CgefMetadata {
                source: "test".to_string(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
                description: None,
            },
            node_schemas: HashMap::new(),
            edge_schemas: HashMap::new(),
            nodes: vec![],
            edges: vec![],
        };
        let report = validate(&doc);
        assert!(report.errors.is_empty());
        assert!(report
            .warnings
            .iter()
            .any(|w| w.message.contains("no nodes")));
    }

    #[test]
    fn test_standard_type_checks() {
        assert!(is_standard_node_type("procedure"));
        assert!(is_standard_node_type("table"));
        assert!(!is_standard_node_type("dubbo_service"));
        assert!(is_standard_edge_type("direct"));
        assert!(!is_standard_edge_type("custom_invokes"));
    }
}

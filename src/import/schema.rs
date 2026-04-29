use std::collections::HashMap;

use crate::import::format::{CgefDocument, CgefEdgeSchema, CgefNodeSchema};

#[derive(Debug, thiserror::Error)]
pub enum SchemaError {
    #[error("node '{node_id}' missing required key field '{field}' for type '{type_name}'")]
    MissingKeyField {
        node_id: String,
        type_name: String,
        field: String,
    },
}

pub struct SchemaRegistry {
    node_schemas: HashMap<String, CgefNodeSchema>,
    edge_schemas: HashMap<String, CgefEdgeSchema>,
}

impl SchemaRegistry {
    pub fn from_document(doc: &CgefDocument) -> Self {
        Self {
            node_schemas: doc.node_schemas.clone(),
            edge_schemas: doc.edge_schemas.clone(),
        }
    }

    pub fn get_node_schema(&self, type_name: &str) -> Option<&CgefNodeSchema> {
        self.node_schemas.get(type_name)
    }

    pub fn get_edge_schema(&self, type_name: &str) -> Option<&CgefEdgeSchema> {
        self.edge_schemas.get(type_name)
    }

    pub fn validate_custom_node_keys(
        &self,
        node_id: &str,
        type_name: &str,
        key: &serde_json::Value,
    ) -> Result<(), SchemaError> {
        let Some(schema) = self.node_schemas.get(type_name) else {
            return Ok(());
        };
        let key_obj = match key.as_object() {
            Some(obj) => obj,
            None => return Ok(()),
        };
        for field in &schema.key_fields {
            if !key_obj.contains_key(field) {
                return Err(SchemaError::MissingKeyField {
                    node_id: node_id.to_string(),
                    type_name: type_name.to_string(),
                    field: field.clone(),
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::format::*;

    fn doc_with_schemas() -> CgefDocument {
        let mut node_schemas = HashMap::new();
        node_schemas.insert(
            "dubbo_service".to_string(),
            CgefNodeSchema {
                display_name: Some("Dubbo Service".to_string()),
                key_fields: vec!["interface".to_string(), "version".to_string()],
                properties: HashMap::new(),
            },
        );
        CgefDocument {
            format_version: 1,
            metadata: CgefMetadata {
                source: "test".to_string(),
                generated_at: "2026-01-01T00:00:00Z".to_string(),
                description: None,
            },
            node_schemas,
            edge_schemas: HashMap::new(),
            nodes: vec![],
            edges: vec![],
        }
    }

    #[test]
    fn test_get_node_schema() {
        let doc = doc_with_schemas();
        let registry = SchemaRegistry::from_document(&doc);
        let schema = registry.get_node_schema("dubbo_service").unwrap();
        assert_eq!(schema.key_fields.len(), 2);
    }

    #[test]
    fn test_missing_schema_returns_none() {
        let doc = doc_with_schemas();
        let registry = SchemaRegistry::from_document(&doc);
        assert!(registry.get_node_schema("nonexistent").is_none());
    }

    #[test]
    fn test_validate_keys_ok() {
        let doc = doc_with_schemas();
        let registry = SchemaRegistry::from_document(&doc);
        let key = serde_json::json!({"interface": "com.example.Svc", "version": "1.0"});
        assert!(registry
            .validate_custom_node_keys("n1", "dubbo_service", &key)
            .is_ok());
    }

    #[test]
    fn test_validate_keys_missing_field() {
        let doc = doc_with_schemas();
        let registry = SchemaRegistry::from_document(&doc);
        let key = serde_json::json!({"interface": "com.example.Svc"});
        let result = registry.validate_custom_node_keys("n1", "dubbo_service", &key);
        assert!(result.is_err());
    }
}

use crate::graph::key::NodeKey;
use crate::graph::node_type_tag;
use crate::graph::CodeGraph;
use crate::graph::Node;
use crate::parser::fingerprint::FileRecord;
use petgraph::graph::NodeIndex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Pre-computed lightweight summary of a graph node for fast listing/filtering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSummary {
    pub id: usize,
    pub key: String,
    pub key_lower: String,
    pub type_tag: String,
    pub in_degree: usize,
    pub out_degree: usize,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphStore {
    pub version: u32,
    pub project_name: String,
    pub created_at: u64,
    pub updated_at: u64,

    graph: CodeGraph,
    node_key_index: HashMap<NodeKey, NodeIndex>,
    node_summaries: Vec<NodeSummary>,

    file_nodes: HashMap<PathBuf, Vec<NodeKey>>,
    file_edges: HashMap<PathBuf, Vec<(NodeKey, NodeKey)>>,
    reverse_deps: HashMap<PathBuf, HashSet<PathBuf>>,

    manifest: HashMap<PathBuf, FileRecord>,
}

#[allow(dead_code)]
impl GraphStore {
    pub fn new(project_name: &str) -> Self {
        let now = timestamp_ms();
        Self {
            version: 4,
            project_name: project_name.to_string(),
            created_at: now,
            updated_at: now,
            graph: CodeGraph::new(),
            node_key_index: HashMap::new(),
            node_summaries: Vec::new(),
            file_nodes: HashMap::new(),
            file_edges: HashMap::new(),
            reverse_deps: HashMap::new(),
            manifest: HashMap::new(),
        }
    }

    pub fn from_graph(project_name: &str, graph: CodeGraph) -> Self {
        let now = timestamp_ms();

        let node_key_index: HashMap<NodeKey, NodeIndex> = graph
            .node_indices()
            .map(|idx| (NodeKey::from_node(&graph[idx]), idx))
            .collect();

        let mut node_summaries: Vec<NodeSummary> =
            Vec::with_capacity(graph.node_count());
        for idx in graph.node_indices() {
            let key = NodeKey::from_node(&graph[idx]);
            let key_str = key.to_string();
            let in_deg = graph
                .neighbors_directed(idx, petgraph::Direction::Incoming)
                .count();
            let out_deg = graph
                .neighbors_directed(idx, petgraph::Direction::Outgoing)
                .count();
            node_summaries.push(NodeSummary {
                id: idx.index(),
                key: key_str.clone(),
                key_lower: key_str.to_lowercase(),
                type_tag: node_type_tag(&graph[idx]).to_string(),
                in_degree: in_deg,
                out_degree: out_deg,
            });
        }

        let mut file_nodes: HashMap<PathBuf, Vec<NodeKey>> = HashMap::new();
        for idx in graph.node_indices() {
            let key = NodeKey::from_node(&graph[idx]);
            if let Some(file) = node_source_file(&graph[idx]) {
                file_nodes.entry(file).or_default().push(key);
            }
        }

        let mut file_edges: HashMap<PathBuf, Vec<(NodeKey, NodeKey)>> = HashMap::new();
        let mut reverse_deps: HashMap<PathBuf, HashSet<PathBuf>> = HashMap::new();

        for edge_idx in graph.edge_indices() {
            let (src, dst) = graph.edge_endpoints(edge_idx).unwrap();
            let src_key = NodeKey::from_node(&graph[src]);
            let dst_key = NodeKey::from_node(&graph[dst]);

            if let Some(src_file) = node_source_file(&graph[src]) {
                file_edges
                    .entry(src_file.clone())
                    .or_default()
                    .push((src_key.clone(), dst_key.clone()));

                if let Some(dst_file) = node_source_file(&graph[dst]) {
                    if dst_file != src_file {
                        reverse_deps.entry(dst_file).or_default().insert(src_file);
                    }
                }
            }
        }

        Self {
            version: 4,
            project_name: project_name.to_string(),
            created_at: now,
            updated_at: now,
            graph,
            node_key_index,
            node_summaries,
            file_nodes,
            file_edges,
            reverse_deps,
            manifest: HashMap::new(),
        }
    }

    pub fn graph(&self) -> &CodeGraph {
        &self.graph
    }

    pub fn node_summaries(&self) -> &[NodeSummary] {
        &self.node_summaries
    }

    pub fn node_key_index(&self) -> &HashMap<NodeKey, NodeIndex> {
        &self.node_key_index
    }

    pub fn manifest(&self) -> &HashMap<PathBuf, FileRecord> {
        &self.manifest
    }

    pub fn file_nodes(&self) -> &HashMap<PathBuf, Vec<NodeKey>> {
        &self.file_nodes
    }

    pub fn file_edges(&self) -> &HashMap<PathBuf, Vec<(NodeKey, NodeKey)>> {
        &self.file_edges
    }

    pub fn reverse_deps(&self) -> &HashMap<PathBuf, HashSet<PathBuf>> {
        &self.reverse_deps
    }

    pub fn stats(&self) -> StoreStats {
        let mut s = StoreStats::default();
        for idx in self.graph.node_indices() {
            match &self.graph[idx] {
                Node::Procedure { .. } => s.procedures += 1,
                Node::Function { .. } => s.functions += 1,
                Node::Unresolved { .. } => s.unresolved += 1,
                Node::MappedStatement { .. } => s.mappers += 1,
                Node::JavaSql { .. } => s.java_sql += 1,
                Node::JavaMethod { .. } => s.java_methods += 1,
                Node::JavaClass { .. } => s.java_classes += 1,
                Node::Table { .. } => s.tables += 1,
                Node::View { .. } => s.views += 1,
                Node::Package { .. } => s.packages += 1,
                Node::Trigger { .. } => s.triggers += 1,
                Node::Type { .. } => s.types += 1,
                Node::Sequence { .. } => s.sequences += 1,
                Node::Index { .. } => s.indexes += 1,
                Node::MaterializedView { .. } => s.materialized_views += 1,
                Node::Synonym { .. } => s.synonyms += 1,
                Node::Event { .. } => s.events += 1,
                Node::Custom { .. } => s.custom_nodes += 1,
            }
        }
        s.edges = self.graph.edge_count();
        s.files = self.manifest.len();
        s
    }

    pub fn update_manifest(&mut self, records: Vec<FileRecord>) {
        for record in records {
            self.manifest.insert(record.path.clone(), record);
        }
        self.touch();
    }

    pub fn remove_manifest_entries(&mut self, paths: &[PathBuf]) {
        for path in paths {
            self.manifest.remove(path);
        }
        self.touch();
    }

    pub fn save_bincode(&self, path: &Path) -> crate::error::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| crate::error::CodeWebError::FileRead {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
        let bytes =
            bincode::serialize(self).map_err(|e| crate::error::CodeWebError::ExportError {
                message: format!("bincode serialize: {}", e),
            })?;
        std::fs::write(path, bytes).map_err(|e| crate::error::CodeWebError::FileRead {
            path: path.to_path_buf(),
            source: e,
        })?;
        Ok(())
    }

    pub fn load_bincode(path: &Path) -> crate::error::Result<Self> {
        let bytes = std::fs::read(path).map_err(|e| crate::error::CodeWebError::FileRead {
            path: path.to_path_buf(),
            source: e,
        })?;
        let store: Self =
            bincode::deserialize(&bytes).map_err(|e| crate::error::CodeWebError::ExportError {
                message: format!("bincode deserialize: {} ({} bytes)", e, bytes.len()),
            })?;
        if store.version != 4 {
            return Err(crate::error::CodeWebError::ExportError {
                message: format!(
                    "unsupported cache version {}, expected 4 — run `codeweb analyze` to regenerate",
                    store.version
                ),
            });
        }
        Ok(store)
    }

    pub fn save_json(&self, path: &Path) -> crate::error::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| crate::error::CodeWebError::FileRead {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| {
            crate::error::CodeWebError::ExportError {
                message: format!("json serialize: {}", e),
            }
        })?;
        std::fs::write(path, json).map_err(|e| crate::error::CodeWebError::FileRead {
            path: path.to_path_buf(),
            source: e,
        })?;
        Ok(())
    }

    pub fn load_json(path: &Path) -> crate::error::Result<Self> {
        let json =
            std::fs::read_to_string(path).map_err(|e| crate::error::CodeWebError::FileRead {
                path: path.to_path_buf(),
                source: e,
            })?;
        let store: Self =
            serde_json::from_str(&json).map_err(|e| crate::error::CodeWebError::ExportError {
                message: format!("json deserialize: {}", e),
            })?;
        if store.version != 4 {
            return Err(crate::error::CodeWebError::ExportError {
                message: format!(
                    "unsupported cache version {}, expected 4 — run `codeweb analyze` to regenerate",
                    store.version
                ),
            });
        }
        Ok(store)
    }

    fn touch(&mut self) {
        self.updated_at = timestamp_ms();
    }

    /// Merge multiple stores into one, deduplicating shared nodes by NodeKey.
    /// Edges pointing to the same semantic entity are consolidated.
    pub fn merge(stores: Vec<Self>, merged_name: &str) -> Self {
        let mut merged = GraphStore::new(merged_name);

        for store in &stores {
            let mut idx_map: HashMap<NodeIndex, NodeIndex> = HashMap::new();

            for old_idx in store.graph.node_indices() {
                let key = NodeKey::from_node(&store.graph[old_idx]);
                let new_idx = merged
                    .node_key_index
                    .entry(key.clone())
                    .or_insert_with(|| merged.graph.add_node(store.graph[old_idx].clone()));
                idx_map.insert(old_idx, *new_idx);
            }

            let mut seen_edges: HashSet<(NodeKey, NodeKey, String)> = HashSet::new();
            let mut table_access_merge_map: HashMap<
                (NodeKey, NodeKey),
                petgraph::graph::EdgeIndex,
            > = HashMap::new();
            for old_edge_idx in store.graph.edge_indices() {
                let (src, dst) = store.graph.edge_endpoints(old_edge_idx).unwrap();
                let src_key = NodeKey::from_node(&store.graph[src]);
                let dst_key = NodeKey::from_node(&store.graph[dst]);
                let edge_type = edge_type_tag(&store.graph[old_edge_idx]);

                let dedup_key = (src_key.clone(), dst_key.clone(), edge_type.clone());
                if !seen_edges.insert(dedup_key) {
                    continue;
                }

                let new_src = idx_map[&src];
                let new_dst = idx_map[&dst];
                let new_edge =
                    merged
                        .graph
                        .add_edge(new_src, new_dst, store.graph[old_edge_idx].clone());

                if edge_type == "table_access" {
                    table_access_merge_map.insert((src_key, dst_key), new_edge);
                }
            }

            Self::merge_duplicate_table_access_edges(&mut merged.graph);

            for (file, keys) in &store.file_nodes {
                let entry = merged.file_nodes.entry(file.clone()).or_default();
                for key in keys {
                    if !entry.contains(key) {
                        entry.push(key.clone());
                    }
                }
            }

            for (file, edges) in &store.file_edges {
                let entry = merged.file_edges.entry(file.clone()).or_default();
                for edge in edges {
                    if !entry.contains(edge) {
                        entry.push(edge.clone());
                    }
                }
            }

            for (file, records) in &store.manifest {
                merged.manifest.insert(file.clone(), records.clone());
            }
        }

        merged.rebuild_reverse_deps();
        merged.touch();
        merged
    }

    fn rebuild_reverse_deps(&mut self) {
        self.reverse_deps.clear();

        let node_to_file: HashMap<NodeKey, PathBuf> = self
            .file_nodes
            .iter()
            .flat_map(|(f, keys)| keys.iter().map(|k| (k.clone(), f.clone())))
            .collect();

        for (src_file, edges) in &self.file_edges {
            for (_, dst_key) in edges {
                if let Some(dst_file) = node_to_file.get(dst_key) {
                    if dst_file != src_file {
                        self.reverse_deps
                            .entry(dst_file.clone())
                            .or_default()
                            .insert(src_file.clone());
                    }
                }
            }
        }
    }

    fn merge_duplicate_table_access_edges(graph: &mut crate::graph::CodeGraph) {
        use std::collections::HashMap;
        let mut merge_targets: HashMap<
            (
                petgraph::graph::NodeIndex,
                petgraph::graph::NodeIndex,
                crate::graph::DataFlowKind,
            ),
            Vec<petgraph::graph::EdgeIndex>,
        > = HashMap::new();
        for edge_idx in graph.edge_indices() {
            if let crate::graph::Edge::TableAccess { flow_kind, .. } = &graph[edge_idx] {
                let (src, dst) = graph.edge_endpoints(edge_idx).unwrap();
                merge_targets
                    .entry((src, dst, *flow_kind))
                    .or_default()
                    .push(edge_idx);
            }
        }
        let mut edges_to_remove = Vec::new();
        for (_, mut edge_indices) in merge_targets {
            if edge_indices.len() <= 1 {
                continue;
            }
            let keep = edge_indices.remove(0);
            let (mut merged_modes, mut merged_kinds) =
                if let crate::graph::Edge::TableAccess {
                    modes, write_kinds, ..
                } = &graph[keep]
                {
                    (*modes, write_kinds.clone())
                } else {
                    continue;
                };
            for &remove_idx in &edge_indices {
                if let crate::graph::Edge::TableAccess {
                    modes, write_kinds, ..
                } = &graph[remove_idx]
                {
                    merged_modes |= *modes;
                    for wk in write_kinds {
                        merged_kinds.insert(*wk);
                    }
                }
            }
            if let crate::graph::Edge::TableAccess {
                modes, write_kinds, ..
            } = &mut graph[keep]
            {
                *modes = merged_modes;
                *write_kinds = merged_kinds;
            }
            edges_to_remove.extend(edge_indices);
        }
        for idx in edges_to_remove {
            graph.remove_edge(idx);
        }
    }
}

fn node_source_file(node: &Node) -> Option<PathBuf> {
    match node {
        Node::Procedure { location, .. } => Some(location.file.to_path_buf()),
        Node::Function { location, .. } => Some(location.file.to_path_buf()),
        Node::MappedStatement { xml_file, .. } => Some(xml_file.clone()),
        Node::JavaSql { java_file, .. } => Some(java_file.clone()),
        Node::JavaMethod { file, .. } => Some(file.clone()),
        Node::JavaClass { file, .. } => Some(file.clone()),
        Node::Table { location, .. } => location.as_ref().map(|l| l.file.to_path_buf()),
        Node::View { location, .. } => location.as_ref().map(|l| l.file.to_path_buf()),
        Node::Unresolved { .. } => None,
        Node::Package { location, .. } => Some(location.file.to_path_buf()),
        Node::Trigger { location, .. } => Some(location.file.to_path_buf()),
        Node::Type { location, .. } => Some(location.file.to_path_buf()),
        Node::Sequence { location, .. } => Some(location.file.to_path_buf()),
        Node::Index { location, .. } => Some(location.file.to_path_buf()),
        Node::MaterializedView { location, .. } => Some(location.file.to_path_buf()),
        Node::Synonym { location, .. } => Some(location.file.to_path_buf()),
        Node::Event { location, .. } => Some(location.file.to_path_buf()),
        Node::Custom { location, .. } => location.as_ref().map(|l| l.file.to_path_buf()),
    }
}

#[allow(dead_code)]
fn timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn edge_type_tag(edge: &crate::graph::Edge) -> String {
    match edge {
        crate::graph::Edge::DirectCall { scope, .. } => match scope {
            crate::graph::CallScope::IntraPackage => "intra_call",
            crate::graph::CallScope::CrossPackage => "cross_call",
            crate::graph::CallScope::External => "direct",
        },
        crate::graph::Edge::DynamicCall { .. } => "dynamic",
        crate::graph::Edge::CallsProcedure { .. } => "calls_procedure",
        crate::graph::Edge::InvokesMapper { .. } => "invokes_mapper",
        crate::graph::Edge::CallsJava { .. } => "calls_java",
        crate::graph::Edge::ContainsMethod => "contains_method",
        crate::graph::Edge::Extends { .. } => "extends",
        crate::graph::Edge::Implements { .. } => "implements",
        crate::graph::Edge::TableAccess { .. } => "table_access",
        crate::graph::Edge::DependsOn { .. } => "depends_on",
        crate::graph::Edge::ContainsRoutine => "contains_routine",
        crate::graph::Edge::TriggersRoutine { .. } => "triggers_routine",
        crate::graph::Edge::ReferencesType { .. } => "references_type",
        crate::graph::Edge::UsesSequence { .. } => "uses_sequence",
        crate::graph::Edge::IndexesTable { .. } => "indexes_table",
        crate::graph::Edge::AliasesObject { .. } => "aliases_object",
        crate::graph::Edge::CustomEdge { type_name, .. } => {
            return format!("custom:{}", type_name);
        }
    }
    .to_string()
}

#[allow(dead_code)]
#[derive(Debug, Default, Serialize)]
pub struct StoreStats {
    pub procedures: usize,
    pub functions: usize,
    pub unresolved: usize,
    pub mappers: usize,
    pub java_sql: usize,
    pub java_methods: usize,
    pub java_classes: usize,
    pub tables: usize,
    pub views: usize,
    pub packages: usize,
    pub triggers: usize,
    pub types: usize,
    pub sequences: usize,
    pub indexes: usize,
    pub materialized_views: usize,
    pub synonyms: usize,
    pub events: usize,
    pub custom_nodes: usize,
    pub custom_edges: usize,
    pub edges: usize,
    pub files: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_bincode_roundtrip_edge_only() {
        let mut graph = CodeGraph::new();
        let proc = crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("pkg".to_string()),
                package: None,
                name: "do_work".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: crate::graph::SourceLocation {
                file: std::sync::Arc::new(std::path::PathBuf::from("a.sql")),
                line: 1,
            },
            partial: false,
        };
        let table = crate::graph::Node::Table {
            schema: Some("public".to_string()),
            name: "orders".to_string(),
            location: None,
            columns: vec![],
            partition_by: None,
            distribute_by: None,
            tablespace: None,
            temporary: false,
            unlogged: false,
            ddl_source: None,
        };
        let proc_idx = graph.add_node(proc);
        let table_idx = graph.add_node(table);
        graph.add_edge(
            proc_idx,
            table_idx,
            crate::graph::Edge::TableAccess {
                flow_kind: crate::graph::DataFlowKind::DmlAccess,
                modes: crate::graph::AccessMode::Read,
                write_kinds: std::collections::HashSet::new(),
                location: crate::graph::SourceLocation {
                    file: std::sync::Arc::new(std::path::PathBuf::from("a.sql")),
                    line: 5,
                },
            },
        );

        let bytes = bincode::serialize(&graph).unwrap();
        let back: CodeGraph = bincode::deserialize(&bytes).unwrap();
        assert_eq!(back.node_count(), 2);
        assert_eq!(back.edge_count(), 1);
    }

    #[test]
    fn test_bincode_roundtrip_via_file() {
        let dir = TempDir::new().unwrap();

        let mut graph = CodeGraph::new();
        let proc = crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("pkg".to_string()),
                package: None,
                name: "do_work".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: crate::graph::SourceLocation {
                file: std::sync::Arc::new(std::path::PathBuf::from("a.sql")),
                line: 1,
            },
            partial: false,
        };
        let table = crate::graph::Node::Table {
            schema: Some("public".to_string()),
            name: "orders".to_string(),
            location: None,
            columns: vec![],
            partition_by: None,
            distribute_by: None,
            tablespace: None,
            temporary: false,
            unlogged: false,
            ddl_source: None,
        };
        let proc_idx = graph.add_node(proc);
        let table_idx = graph.add_node(table);
        let write_kinds = std::collections::HashSet::new();
        graph.add_edge(
            proc_idx,
            table_idx,
            crate::graph::Edge::TableAccess {
                flow_kind: crate::graph::DataFlowKind::DmlAccess,
                modes: crate::graph::AccessMode::Read,
                write_kinds,
                location: crate::graph::SourceLocation {
                    file: std::sync::Arc::new(std::path::PathBuf::from("a.sql")),
                    line: 5,
                },
            },
        );

        let store = GraphStore::from_graph("test", graph);
        let path = dir.path().join("test.bincode");
        store.save_bincode(&path).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        eprintln!("File size: {} bytes", bytes.len());

        let loaded = GraphStore::load_bincode(&path).unwrap();
        assert_eq!(loaded.graph().node_count(), 2);
        assert_eq!(loaded.graph().edge_count(), 1);
    }

    #[test]
    fn test_bincode_roundtrip_with_custom_node() {
        let mut graph = CodeGraph::new();
        let mut key_fields = std::collections::BTreeMap::new();
        key_fields.insert("interface".to_string(), "com.example.Svc".to_string());
        let mut props = std::collections::BTreeMap::new();
        props.insert("version".to_string(), serde_json::json!("2.0"));
        let node = crate::graph::Node::Custom {
            type_name: "dubbo_service".to_string(),
            label: "com.example.Svc".to_string(),
            key_fields,
            properties: crate::graph::JsonMap(props),
            location: None,
        };
        graph.add_node(node);

        let store = GraphStore::from_graph("test", graph);
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.bincode");
        store.save_bincode(&path).unwrap();

        let loaded = GraphStore::load_bincode(&path).unwrap();
        assert_eq!(loaded.graph().node_count(), 1);
    }

    #[test]
    fn test_bincode_roundtrip_table_with_partition_and_distribute() {
        // Regression test: PartitionInfo/DistributeInfo used #[serde(tag = "...")]
        // which requires deserialize_any — bincode does not support that.
        let mut graph = CodeGraph::new();
        let table = crate::graph::Node::Table {
            schema: Some("public".to_string()),
            name: "orders".to_string(),
            location: None,
            columns: vec![
                crate::graph::ColumnSummary {
                    name: "id".to_string(),
                    data_type: "BIGINT".to_string(),
                    nullable: false,
                    is_primary_key: true,
                    default_value: None,
                    comment: None,
                },
                crate::graph::ColumnSummary {
                    name: "created_at".to_string(),
                    data_type: "TIMESTAMP".to_string(),
                    nullable: false,
                    is_primary_key: false,
                    default_value: Some("now()".to_string()),
                    comment: Some("creation time".to_string()),
                },
            ],
            partition_by: Some(crate::graph::PartitionInfo::Range {
                columns: vec!["created_at".to_string()],
                partitions: vec!["p_2024".to_string(), "p_2025".to_string()],
            }),
            distribute_by: Some(crate::graph::DistributeInfo::Hash {
                columns: vec!["user_id".to_string()],
            }),
            tablespace: Some("pg_default".to_string()),
            temporary: false,
            unlogged: false,
            ddl_source: None,
        };
        graph.add_node(table);

        let store = GraphStore::from_graph("partition-test", graph);
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("partition.bincode");
        store.save_bincode(&path).unwrap();

        let loaded = GraphStore::load_bincode(&path).unwrap();
        assert_eq!(loaded.graph().node_count(), 1);

        let raw_nodes = loaded.graph().raw_nodes();
        let table_node = &raw_nodes[0].weight;
        match table_node {
            crate::graph::Node::Table {
                name,
                columns,
                partition_by,
                distribute_by,
                ..
            } => {
                assert_eq!(name, "orders");
                assert_eq!(columns.len(), 2);
                assert!(partition_by.is_some());
                assert!(distribute_by.is_some());
            }
            other => panic!("Expected Table node, got {:?}", other),
        }
    }

    #[test]
    fn test_old_cache_version_rejected() {
        let dir = TempDir::new().unwrap();
        let json_path = dir.path().join("test.json");

        let graph = CodeGraph::new();
        let store = GraphStore::from_graph("test", graph);
        store.save_json(&json_path).unwrap();

        let loaded = GraphStore::load_json(&json_path);
        assert!(loaded.is_ok(), "Current version should load fine");

        let json_str = std::fs::read_to_string(&json_path).unwrap();
        let mut json_val: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        json_val["version"] = serde_json::Value::from(3u64);
        std::fs::write(&json_path, serde_json::to_string(&json_val).unwrap()).unwrap();

        let result = GraphStore::load_json(&json_path);
        assert!(result.is_err(), "Version 3 cache should be rejected");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("unsupported cache version"),
            "Error should mention version: {}",
            err_msg
        );
    }
}

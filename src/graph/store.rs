use crate::graph::key::NodeKey;
use crate::graph::CodeGraph;
use crate::graph::Node;
use crate::parser::fingerprint::FileRecord;
use petgraph::graph::NodeIndex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[allow(dead_code)]
#[derive(Serialize, Deserialize)]
pub struct GraphStore {
    pub version: u32,
    pub project_name: String,
    pub created_at: u64,
    pub updated_at: u64,

    graph: CodeGraph,
    node_key_index: HashMap<NodeKey, NodeIndex>,

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
            version: 1,
            project_name: project_name.to_string(),
            created_at: now,
            updated_at: now,
            graph: CodeGraph::new(),
            node_key_index: HashMap::new(),
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
            version: 1,
            project_name: project_name.to_string(),
            created_at: now,
            updated_at: now,
            graph,
            node_key_index,
            file_nodes,
            file_edges,
            reverse_deps,
            manifest: HashMap::new(),
        }
    }

    pub fn graph(&self) -> &CodeGraph {
        &self.graph
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
                Node::Unresolved { .. } => s.unresolved += 1,
                Node::MappedStatement { .. } => s.mappers += 1,
                Node::JavaSql { .. } => s.java_sql += 1,
                Node::JavaMethod { .. } => s.java_methods += 1,
                Node::JavaClass { .. } => s.java_classes += 1,
                Node::Table { .. } => s.tables += 1,
                Node::View { .. } => s.views += 1,
                Node::Package { .. } | Node::Trigger { .. } => {}
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
        bincode::deserialize(&bytes).map_err(|e| crate::error::CodeWebError::ExportError {
            message: format!("bincode deserialize: {}", e),
        })
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
        serde_json::from_str(&json).map_err(|e| crate::error::CodeWebError::ExportError {
            message: format!("json deserialize: {}", e),
        })
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
            for old_edge_idx in store.graph.edge_indices() {
                let (src, dst) = store.graph.edge_endpoints(old_edge_idx).unwrap();
                let src_key = NodeKey::from_node(&store.graph[src]);
                let dst_key = NodeKey::from_node(&store.graph[dst]);
                let edge_type = edge_type_tag(&store.graph[old_edge_idx]);

                let dedup_key = (src_key.clone(), dst_key.clone(), edge_type);
                if seen_edges.insert(dedup_key) {
                    let new_src = idx_map[&src];
                    let new_dst = idx_map[&dst];
                    merged
                        .graph
                        .add_edge(new_src, new_dst, store.graph[old_edge_idx].clone());
                }
            }

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
}

fn node_source_file(node: &Node) -> Option<PathBuf> {
    match node {
        Node::Procedure { location, .. } => Some(location.file.clone()),
        Node::MappedStatement { xml_file, .. } => Some(xml_file.clone()),
        Node::JavaSql { java_file, .. } => Some(java_file.clone()),
        Node::JavaMethod { file, .. } => Some(file.clone()),
        Node::JavaClass { file, .. } => Some(file.clone()),
        Node::Table { .. } | Node::View { .. } | Node::Unresolved { .. } => None,
        Node::Package { location, .. } => Some(location.file.clone()),
        Node::Trigger { location, .. } => Some(location.file.clone()),
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
        crate::graph::Edge::DirectCall { .. } => "direct",
        crate::graph::Edge::DynamicCall { .. } => "dynamic",
        crate::graph::Edge::CallsProcedure { .. } => "calls_procedure",
        crate::graph::Edge::InvokesMapper { .. } => "invokes_mapper",
        crate::graph::Edge::CallsJava { .. } => "calls_java",
        crate::graph::Edge::ContainsMethod => "contains_method",
        crate::graph::Edge::Extends { .. } => "extends",
        crate::graph::Edge::Implements { .. } => "implements",
        crate::graph::Edge::ReferencesTable { .. } => "references_table",
        crate::graph::Edge::ContainsRoutine => "contains_routine",
        crate::graph::Edge::TriggersRoutine { .. } => "triggers_routine",
    }
    .to_string()
}

#[allow(dead_code)]
#[derive(Debug, Default)]
pub struct StoreStats {
    pub procedures: usize,
    pub unresolved: usize,
    pub mappers: usize,
    pub java_sql: usize,
    pub java_methods: usize,
    pub java_classes: usize,
    pub tables: usize,
    pub views: usize,
    pub edges: usize,
    pub files: usize,
}

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

    /// Index: type tag → list of NodeIndex (e.g., "proc" → [idx1, idx2, ...])
    type_tag_index: HashMap<String, Vec<NodeIndex>>,
    /// Index: lowercase name → list of (NodeIndex, display_key) for prefix/substring search
    name_index: Vec<(String, NodeIndex)>,
    /// Index: schema name → list of NodeIndex
    schema_index: HashMap<String, Vec<NodeIndex>>,
    /// Index: EdgeCategory → list of EdgeIndex for fast edge-type filtering
    edge_category_index: HashMap<String, Vec<petgraph::graph::EdgeIndex>>,
}

#[allow(dead_code)]
impl GraphStore {
    pub fn new(project_name: &str) -> Self {
        let now = timestamp_ms();
        Self {
            version: 5,
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
            type_tag_index: HashMap::new(),
            name_index: Vec::new(),
            schema_index: HashMap::new(),
            edge_category_index: HashMap::new(),
        }
    }

    pub fn from_graph(project_name: &str, graph: CodeGraph) -> Self {
        let now = timestamp_ms();

        let node_key_index: HashMap<NodeKey, NodeIndex> = graph
            .node_indices()
            .map(|idx| (NodeKey::from_node(&graph[idx]), idx))
            .collect();

        let mut node_summaries: Vec<NodeSummary> = Vec::with_capacity(graph.node_count());
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

        // Build type_tag_index
        let mut type_tag_index: HashMap<String, Vec<NodeIndex>> = HashMap::new();
        for idx in graph.node_indices() {
            let tag = node_type_tag(&graph[idx]).to_string();
            type_tag_index.entry(tag).or_default().push(idx);
        }

        // Build name_index (sorted by lowercase key for binary search)
        let mut name_index: Vec<(String, NodeIndex)> = graph
            .node_indices()
            .map(|idx| {
                let key = NodeKey::from_node(&graph[idx]);
                (key.to_string().to_lowercase(), idx)
            })
            .collect();
        name_index.sort_by(|a, b| a.0.cmp(&b.0));

        // Build schema_index
        let mut schema_index: HashMap<String, Vec<NodeIndex>> = HashMap::new();
        for idx in graph.node_indices() {
            if let Some(schema) = extract_schema(&graph[idx]) {
                schema_index
                    .entry(schema.to_lowercase())
                    .or_default()
                    .push(idx);
            }
        }

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

        let mut edge_category_index: HashMap<String, Vec<petgraph::graph::EdgeIndex>> =
            HashMap::new();
        for edge_idx in graph.edge_indices() {
            let cat = graph[edge_idx].category();
            let key = match cat {
                crate::graph::EdgeCategory::Call => "call",
                crate::graph::EdgeCategory::Composition => "composition",
                crate::graph::EdgeCategory::DataFlow => "dataflow",
                crate::graph::EdgeCategory::Reference => "reference",
                crate::graph::EdgeCategory::Inheritance => "inheritance",
            };
            edge_category_index
                .entry(key.to_string())
                .or_default()
                .push(edge_idx);
        }

        Self {
            version: 5,
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
            type_tag_index,
            name_index,
            schema_index,
            edge_category_index,
        }
    }

    pub fn graph(&self) -> &CodeGraph {
        &self.graph
    }

    pub fn ensure_consistency(&mut self) {
        let expected = self.graph.node_count();
        if self.node_summaries.len() != expected {
            eprintln!(
                "store: stale indexes (node_summaries {}/{}), rebuilding...",
                self.node_summaries.len(),
                expected,
            );
            self.rebuild_secondary_indexes();
        }
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

    pub fn nodes_by_type(&self, type_tag: &str) -> &[NodeIndex] {
        self.type_tag_index
            .get(type_tag)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn name_index(&self) -> &[(String, NodeIndex)] {
        &self.name_index
    }

    pub fn schema_index(&self) -> &HashMap<String, Vec<NodeIndex>> {
        &self.schema_index
    }

    pub fn edges_by_category(&self, category: &str) -> &[petgraph::graph::EdgeIndex] {
        self.edge_category_index.get(category).map_or(&[], |v| v)
    }

    /// Search nodes by SQL text content (substring match, case-insensitive).
    /// Checks MappedStatement.sql and JavaSql.sql fields.
    /// Returns Vec of (NodeIndex, display_key).
    pub fn search_by_sql(&self, query: &str) -> Vec<(NodeIndex, String)> {
        let prepared = PreparedQuery::new(query);
        let mut results = Vec::new();
        for idx in self.graph.node_indices() {
            match &self.graph[idx] {
                Node::MappedStatement {
                    sql: Some(sql_text),
                    namespace,
                    statement_id,
                    ..
                } => {
                    if prepared.matches(sql_text) {
                        results.push((idx, format!("mapper:{}.{}", namespace, statement_id)));
                    }
                }
                Node::JavaSql {
                    sql: Some(sql_text),
                    class_name,
                    method_name,
                    ..
                } => {
                    if prepared.matches(sql_text) {
                        let ctx = match (class_name, method_name) {
                            (Some(c), Some(m)) => format!("{}.{}", c, m),
                            (Some(c), None) => c.clone(),
                            (None, Some(m)) => m.clone(),
                            (None, None) => "?".to_string(),
                        };
                        results.push((idx, format!("javasql:{}", ctx)));
                    }
                }
                _ => {}
            }
        }
        results
    }
}

/// Query normalized and pre-computed once, then reused for every node comparison.
struct PreparedQuery {
    normalized: String,
    has_wildcard: bool,
    segments: Vec<String>,
}

impl PreparedQuery {
    fn new(query: &str) -> Self {
        let lower = query.to_lowercase();
        let normalized = normalize_for_matching(&lower);
        let has_wildcard = normalized.contains('?');
        let segments: Vec<String> = if has_wildcard {
            normalized
                .split('?')
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        } else {
            Vec::new()
        };
        Self {
            normalized,
            has_wildcard,
            segments,
        }
    }

    fn matches(&self, sql_text: &str) -> bool {
        let sql_lower = normalize_for_matching(&sql_text.to_lowercase());

        if sql_lower.contains(&self.normalized) {
            return true;
        }

        let sql_has_wc = sql_lower.contains('?');

        if !sql_has_wc && !self.has_wildcard {
            return false;
        }

        if self.has_wildcard && find_query_segments_in_sql(&sql_lower, &self.segments) {
            return true;
        }

        if sql_has_wc && find_sql_segments_in_query(&sql_lower, &self.normalized, self.has_wildcard) {
            return true;
        }

        false
    }
}

/// Collapse consecutive whitespace into single spaces, replace ogsql-parser internal
/// placeholder markers (`__XML_PARAM_*__`, `__XML_RAW_*__`) with `?`, then remove spaces
/// around SQL operators, parentheses, and commas so that formatting differences don't
/// prevent a match (e.g. `user_id = ?` vs `user_id=?`, `TO_CHAR( x , y )` vs `TO_CHAR(x,y)`).
fn normalize_for_matching(s: &str) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = replace_xml_placeholders(&s);
    // Comparison operators (longest first to avoid partial matches)
    s.replace(" >= ", ">=")
        .replace(" <= ", "<=")
        .replace(" <> ", "<>")
        .replace(" != ", "!=")
        .replace(" = ", "=")
        .replace(" > ", ">")
        .replace(" < ", "<")
        .replace(" - ", "-")
        .replace(" + ", "+")
        .replace(" * ", "*")
        // Parentheses and commas
        .replace("( ", "(")
        .replace(" )", ")")
        .replace(" ,", ",")
        .replace(", ", ",")
}

/// Replace ogsql-parser internal placeholder markers with `?` for search matching.
/// Handles both `__XML_PARAM_*__` (parameter placeholders from `#{}`) and
/// `__XML_RAW_*__` (text-substitution placeholders from `${}`), including
/// variants with embedded type hints like `__XML_RAW_STRING_col__`.
/// Input is expected to be already lowercased.
fn replace_xml_placeholders(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut pos = 0;

    while pos < s.len() {
        let remaining = &s[pos..];

        let (prefix, prefix_len) = if remaining.starts_with("__xml_param_") {
            ("__xml_param_", 12)
        } else if remaining.starts_with("__xml_raw_") {
            ("__xml_raw_", 10)
        } else {
            let c = remaining.chars().next().unwrap();
            result.push(c);
            pos += c.len_utf8();
            continue;
        };

        let after_prefix = &s[pos + prefix_len..];
        if let Some(end) = after_prefix.find("__") {
            result.push('?');
            pos += prefix_len + end + 2;
        } else {
            result.push_str(prefix);
            pos += prefix_len;
        }
    }

    result
}

/// Check if `sql_text` (lowercased) matches `query_lower` where `?` in **either** side
/// acts as a wildcard matching any non-empty sequence of characters.
///
/// Matching strategy (tried in order, first success wins):
/// 1. Direct substring match after normalization.
/// 2. Query has `?` → split query on `?`, verify each segment appears in order in SQL.
/// 3. SQL has `?` → split SQL on `?`, verify at least 2 consecutive concrete segments
///    appear in order in the query (the `?` gaps absorb any characters).
///
/// Both sides are normalized before comparison: whitespace collapsed, `__XML_PARAM_*__`
/// and `__XML_RAW_*__` replaced with `?`, spaces around operators removed.
#[cfg(test)]
fn sql_text_matches(sql_text: &str, query_lower: &str) -> bool {
    let prepared = PreparedQuery::new(query_lower);
    prepared.matches(sql_text)
}

/// Verify each pre-split query segment appears in order in `sql`.
fn find_query_segments_in_sql(sql: &str, segments: &[String]) -> bool {
    if segments.is_empty() {
        return false;
    }
    let mut pos = 0;
    for part in segments {
        match sql[pos..].find(part.as_str()) {
            Some(p) => pos += p + part.len(),
            None => return false,
        }
    }
    true
}

/// Split `sql` on `?`, check if at least 2 consecutive concrete segments
/// appear in order in `query` (the `?` gaps between SQL segments absorb
/// any characters in the query).
///
/// When `query_has_wildcard` is true, the match is only accepted if the
/// query tail after the last matched position (excluding leading `?`) is empty.
/// This prevents queries with extra conditions not present in the SQL from matching.
fn find_sql_segments_in_query(sql: &str, query: &str, query_has_wildcard: bool) -> bool {
    let sql_parts: Vec<&str> = sql.split('?').filter(|s| !s.is_empty()).collect();
    if sql_parts.is_empty() {
        return false;
    }
    if sql_parts.len() == 1 {
        return query.contains(sql_parts[0]);
    }

    for start in 0..sql_parts.len() {
        let mut pos = 0;
        let mut count = 0;

        for part in &sql_parts[start..] {
            match query[pos..].find(*part) {
                Some(p) => {
                    if count > 0 && p == 0 {
                        break;
                    }
                    pos += p + part.len();
                    count += 1;
                }
                None => break,
            }
        }

        if count >= 2 {
            if query_has_wildcard {
                let tail = query[pos..].trim_start_matches('?').trim();
                if tail.is_empty() {
                    return true;
                }
            } else {
                return true;
            }
        }
    }

    false
}

impl GraphStore {
    /// Search nodes by name using the sorted name_index.
    /// Returns Vec of (NodeIndex, display_key) ranked by MatchRank (Exact > WordBoundary > Substring).
    pub fn search_nodes(&self, query: &str) -> Vec<(NodeIndex, String)> {
        use crate::graph::traverse::MatchRank;
        let lower = query.to_lowercase();
        let mut results: Vec<(NodeIndex, String, MatchRank)> = Vec::new();

        for (key_lower, idx) in &self.name_index {
            if !key_lower.contains(&lower) {
                continue;
            }
            let display = crate::graph::key::NodeKey::from_node(&self.graph[*idx]).to_string();
            if let Some(rank) = MatchRank::classify(&lower, key_lower) {
                results.push((*idx, display, rank));
            }
        }

        results.sort_by(|a, b| a.2.cmp(&b.2).then_with(|| a.1.cmp(&b.1)));
        results
            .into_iter()
            .map(|(idx, display, _)| (idx, display))
            .collect()
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
        if store.version != 5 {
            return Err(crate::error::CodeWebError::ExportError {
                message: format!(
                    "unsupported cache version {}, expected 5 — run `codeweb analyze` to regenerate",
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
        if store.version != 5 {
            return Err(crate::error::CodeWebError::ExportError {
                message: format!(
                    "unsupported cache version {}, expected 5 — run `codeweb analyze` to regenerate",
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
    ///
    /// Uses two-phase matching for Procedure/Function nodes:
    ///   Phase 1 — exact NodeKey match (schema + package + name)
    ///   Phase 2 — relaxed match (package + name, ignoring schema)
    /// This handles the case where SQL analysis produces `proc:BIGFUND.PKG_FOO.sp`
    /// but CGEF import produces `proc:pkg_foo.sp` (no schema).
    pub fn merge(stores: Vec<Self>, merged_name: &str) -> Self {
        let mut merged = GraphStore::new(merged_name);

        for store in &stores {
            let mut idx_map: HashMap<NodeIndex, NodeIndex> = HashMap::new();

            // Build a reverse-relaxed index: maps relaxed(key) → existing index
            // for nodes that have schema. Allows matching incoming schema-less
            // nodes against existing schema-qualified nodes.
            let relaxed_reverse: HashMap<NodeKey, NodeIndex> = merged
                .node_key_index
                .iter()
                .filter_map(|(key, &idx)| key.relaxed().map(|rk| (rk, idx)))
                .collect();

            for old_idx in store.graph.node_indices() {
                let key = NodeKey::from_node(&store.graph[old_idx]);

                // Phase 1: exact NodeKey match
                if let Some(&existing) = merged.node_key_index.get(&key) {
                    idx_map.insert(old_idx, existing);
                    continue;
                }

                // Phase 2a: incoming has schema → try relaxed (drop schema)
                if let Some(ref rk) = key.relaxed() {
                    if let Some(&existing) = merged.node_key_index.get(rk) {
                        idx_map.insert(old_idx, existing);
                        continue;
                    }
                }

                // Phase 2b: incoming has no schema → check if a schema-qualified
                // version already exists via the reverse-relaxed index
                if let Some(&existing) = relaxed_reverse.get(&key) {
                    idx_map.insert(old_idx, existing);
                    continue;
                }

                // No match — add as new node
                let new_idx = merged.graph.add_node(store.graph[old_idx].clone());
                merged.node_key_index.insert(key.clone(), new_idx);
                idx_map.insert(old_idx, new_idx);
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
        merged.rebuild_secondary_indexes();
        merged.touch();
        merged
    }

    fn rebuild_secondary_indexes(&mut self) {
        self.type_tag_index.clear();
        self.name_index.clear();
        self.schema_index.clear();
        self.edge_category_index.clear();
        self.node_summaries.clear();

        for idx in self.graph.node_indices() {
            let tag = node_type_tag(&self.graph[idx]).to_string();
            self.type_tag_index
                .entry(tag.clone())
                .or_default()
                .push(idx);

            let key = NodeKey::from_node(&self.graph[idx]);
            let key_str = key.to_string();
            let key_lower = key_str.to_lowercase();
            self.name_index.push((key_lower.clone(), idx));

            if let Some(schema) = extract_schema(&self.graph[idx]) {
                self.schema_index
                    .entry(schema.to_lowercase())
                    .or_default()
                    .push(idx);
            }

            let in_deg = self
                .graph
                .neighbors_directed(idx, petgraph::Direction::Incoming)
                .count();
            let out_deg = self
                .graph
                .neighbors_directed(idx, petgraph::Direction::Outgoing)
                .count();
            self.node_summaries.push(NodeSummary {
                id: idx.index(),
                key: key_str,
                key_lower,
                type_tag: tag,
                in_degree: in_deg,
                out_degree: out_deg,
            });
        }
        self.name_index.sort_by(|a, b| a.0.cmp(&b.0));

        use crate::graph::EdgeCategory;
        for edge_idx in self.graph.edge_indices() {
            let cat = self.graph[edge_idx].category();
            let key = match cat {
                EdgeCategory::Call => "call",
                EdgeCategory::Composition => "composition",
                EdgeCategory::DataFlow => "dataflow",
                EdgeCategory::Reference => "reference",
                EdgeCategory::Inheritance => "inheritance",
            };
            self.edge_category_index
                .entry(key.to_string())
                .or_default()
                .push(edge_idx);
        }
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

    #[allow(dead_code)]
    pub fn dead_routines(&self) -> Vec<NodeIndex> {
        self.nodes_by_type("proc")
            .iter()
            .chain(self.nodes_by_type("func").iter())
            .filter(|&&idx| {
                self.graph
                    .neighbors_directed(idx, petgraph::Direction::Incoming)
                    .count()
                    == 0
                    && self
                        .graph
                        .neighbors_directed(idx, petgraph::Direction::Outgoing)
                        .count()
                        == 0
            })
            .copied()
            .collect()
    }

    #[allow(dead_code)]
    pub fn entry_points(&self) -> Vec<NodeIndex> {
        self.graph
            .node_indices()
            .filter(|&idx| {
                self.graph
                    .neighbors_directed(idx, petgraph::Direction::Incoming)
                    .count()
                    == 0
            })
            .collect()
    }

    #[allow(dead_code)]
    pub fn find_cycles(&self) -> Vec<Vec<NodeIndex>> {
        petgraph::algo::kosaraju_scc(&self.graph)
            .into_iter()
            .filter(|scc| scc.len() > 1)
            .collect()
    }

    #[allow(dead_code)]
    pub fn impact(&self, node: NodeIndex, max_depth: Option<usize>) -> Vec<NodeIndex> {
        use crate::graph::query::filter::EdgeFilter;
        use crate::graph::query::traversal::GraphTraversal;

        let mut traversal = GraphTraversal::new(&self.graph, node)
            .incoming()
            .edge_filter(EdgeFilter::new());

        if let Some(depth) = max_depth {
            traversal = traversal.max_depth(depth);
        }

        traversal.collect_nodes()
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

pub(crate) fn extract_schema(node: &Node) -> Option<&str> {
    match node {
        Node::Procedure { id, .. } | Node::Function { id, .. } => id.schema.as_deref(),
        Node::Table { schema, .. } | Node::View { schema, .. } => schema.as_deref(),
        Node::Package { schema, .. } => schema.as_deref(),
        Node::Type { schema, .. } => schema.as_deref(),
        Node::Sequence { schema, .. } => schema.as_deref(),
        Node::MaterializedView { schema, .. } => schema.as_deref(),
        Node::Synonym { schema, .. } => schema.as_deref(),
        _ => None,
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
            columns: Box::new(vec![]),
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
                column_analysis: None,
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
            columns: Box::new(vec![]),
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
                column_analysis: None,
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
            type_name: Box::new("dubbo_service".to_string()),
            label: Box::new("com.example.Svc".to_string()),
            key_fields: Box::new(key_fields),
            properties: Box::new(crate::graph::JsonMap(props)),
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
            columns: Box::new(vec![
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
            ]),
            partition_by: Some(Box::new(crate::graph::PartitionInfo::Range {
                columns: vec!["created_at".to_string()],
                partitions: vec!["p_2024".to_string(), "p_2025".to_string()],
            })),
            distribute_by: Some(Box::new(crate::graph::DistributeInfo::Hash {
                columns: vec!["user_id".to_string()],
            })),
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

    #[test]
    fn type_tag_index_returns_correct_nodes() {
        let mut graph = CodeGraph::new();
        let file = std::sync::Arc::new(std::path::PathBuf::from("a.sql"));
        let loc = crate::graph::SourceLocation { file, line: 1 };

        for i in 0..3 {
            graph.add_node(crate::graph::Node::Procedure {
                id: crate::graph::RoutineId {
                    schema: Some("public".to_string()),
                    package: None,
                    name: format!("proc{}", i),
                    kind: crate::graph::RoutineKind::Procedure,
                },
                location: loc.clone(),
                partial: false,
            });
        }
        for i in 0..2 {
            graph.add_node(crate::graph::Node::Table {
                schema: Some("public".to_string()),
                name: format!("table{}", i),
                location: None,
                columns: Box::new(vec![]),
                partition_by: None,
                distribute_by: None,
                tablespace: None,
                temporary: false,
                unlogged: false,
                ddl_source: None,
            });
        }
        graph.add_node(crate::graph::Node::Function {
            id: crate::graph::RoutineId {
                schema: Some("public".to_string()),
                package: None,
                name: "my_func".to_string(),
                kind: crate::graph::RoutineKind::Function,
            },
            location: loc.clone(),
            partial: false,
        });

        let store = GraphStore::from_graph("test", graph);
        assert_eq!(store.nodes_by_type("proc").len(), 3);
        assert_eq!(store.nodes_by_type("table").len(), 2);
        assert_eq!(store.nodes_by_type("func").len(), 1);
        assert_eq!(store.nodes_by_type("view").len(), 0);
    }

    #[test]
    fn name_index_is_sorted_and_complete() {
        let mut graph = CodeGraph::new();
        let file = std::sync::Arc::new(std::path::PathBuf::from("a.sql"));
        let loc = crate::graph::SourceLocation { file, line: 1 };

        graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("public".to_string()),
                package: None,
                name: "zebra".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
        });
        graph.add_node(crate::graph::Node::Table {
            schema: Some("public".to_string()),
            name: "apple".to_string(),
            location: None,
            columns: Box::new(vec![]),
            partition_by: None,
            distribute_by: None,
            tablespace: None,
            temporary: false,
            unlogged: false,
            ddl_source: None,
        });
        graph.add_node(crate::graph::Node::Function {
            id: crate::graph::RoutineId {
                schema: Some("public".to_string()),
                package: None,
                name: "mango".to_string(),
                kind: crate::graph::RoutineKind::Function,
            },
            location: loc.clone(),
            partial: false,
        });

        let store = GraphStore::from_graph("test", graph);
        let name_index = store.name_index();
        assert_eq!(name_index.len(), 3);
        for i in 1..name_index.len() {
            assert!(
                name_index[i - 1].0 <= name_index[i].0,
                "name_index should be sorted"
            );
        }
    }

    #[test]
    fn schema_index_groups_by_schema() {
        let mut graph = CodeGraph::new();
        let file = std::sync::Arc::new(std::path::PathBuf::from("a.sql"));
        let loc = crate::graph::SourceLocation { file, line: 1 };

        graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("schema_a".to_string()),
                package: None,
                name: "p1".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
        });
        graph.add_node(crate::graph::Node::Table {
            schema: Some("schema_a".to_string()),
            name: "t1".to_string(),
            location: None,
            columns: Box::new(vec![]),
            partition_by: None,
            distribute_by: None,
            tablespace: None,
            temporary: false,
            unlogged: false,
            ddl_source: None,
        });
        graph.add_node(crate::graph::Node::Function {
            id: crate::graph::RoutineId {
                schema: Some("schema_b".to_string()),
                package: None,
                name: "f1".to_string(),
                kind: crate::graph::RoutineKind::Function,
            },
            location: loc.clone(),
            partial: false,
        });
        graph.add_node(crate::graph::Node::View {
            schema: Some("schema_b".to_string()),
            name: "v1".to_string(),
            location: None,
        });
        graph.add_node(crate::graph::Node::Trigger {
            name: "trig1".to_string(),
            table: vec!["t1".to_string()],
            location: loc.clone(),
        });

        let store = GraphStore::from_graph("test", graph);
        let schema_index = store.schema_index();
        assert_eq!(schema_index.get("schema_a").map(|v| v.len()), Some(2));
        assert_eq!(schema_index.get("schema_b").map(|v| v.len()), Some(2));
        assert_eq!(schema_index.get("schema_c").map(|v| v.len()), None);
    }

    #[test]
    fn search_nodes_finds_exact_match() {
        let mut graph = CodeGraph::new();
        let file = std::sync::Arc::new(std::path::PathBuf::from("a.sql"));
        let loc = crate::graph::SourceLocation { file, line: 1 };

        graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("public".to_string()),
                package: None,
                name: "do_work".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
        });
        graph.add_node(crate::graph::Node::Table {
            schema: Some("public".to_string()),
            name: "orders".to_string(),
            location: None,
            columns: Box::new(vec![]),
            partition_by: None,
            distribute_by: None,
            tablespace: None,
            temporary: false,
            unlogged: false,
            ddl_source: None,
        });

        let store = GraphStore::from_graph("test", graph);
        let results = store.search_nodes("do_work");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1, "proc:public.do_work");
    }

    #[test]
    fn search_nodes_finds_substring_match() {
        let mut graph = CodeGraph::new();
        let file = std::sync::Arc::new(std::path::PathBuf::from("a.sql"));
        let loc = crate::graph::SourceLocation { file, line: 1 };

        graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("public".to_string()),
                package: None,
                name: "calculate_total".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
        });
        graph.add_node(crate::graph::Node::Function {
            id: crate::graph::RoutineId {
                schema: Some("public".to_string()),
                package: None,
                name: "get_total_amount".to_string(),
                kind: crate::graph::RoutineKind::Function,
            },
            location: loc.clone(),
            partial: false,
        });

        let store = GraphStore::from_graph("test", graph);
        let results = store.search_nodes("total");
        assert_eq!(results.len(), 2);
        assert!(results[0].1.contains("total"));
    }

    #[test]
    fn search_nodes_returns_empty_for_no_match() {
        let mut graph = CodeGraph::new();
        let file = std::sync::Arc::new(std::path::PathBuf::from("a.sql"));
        let loc = crate::graph::SourceLocation { file, line: 1 };

        graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("public".to_string()),
                package: None,
                name: "do_work".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
        });

        let store = GraphStore::from_graph("test", graph);
        let results = store.search_nodes("nonexistent");
        assert!(results.is_empty());
    }

    #[test]
    fn edge_category_index_groups_correctly() {
        let mut graph = CodeGraph::new();
        let file = std::sync::Arc::new(std::path::PathBuf::from("a.sql"));
        let loc = crate::graph::SourceLocation { file, line: 1 };

        let proc_a = graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("public".to_string()),
                package: None,
                name: "proc_a".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
        });
        let proc_b = graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("public".to_string()),
                package: None,
                name: "proc_b".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
        });
        let table = graph.add_node(crate::graph::Node::Table {
            schema: Some("public".to_string()),
            name: "orders".to_string(),
            location: None,
            columns: Box::new(vec![]),
            partition_by: None,
            distribute_by: None,
            tablespace: None,
            temporary: false,
            unlogged: false,
            ddl_source: None,
        });

        graph.add_edge(
            proc_a,
            proc_b,
            crate::graph::Edge::DirectCall {
                scope: crate::graph::CallScope::External,
                location: loc.clone(),
            },
        );
        graph.add_edge(
            proc_a,
            table,
            crate::graph::Edge::TableAccess {
                flow_kind: crate::graph::DataFlowKind::DmlAccess,
                modes: crate::graph::AccessMode::Read,
                write_kinds: std::collections::HashSet::new(),
                location: loc.clone(),
                column_analysis: None,
            },
        );
        graph.add_edge(
            proc_a,
            table,
            crate::graph::Edge::TriggersRoutine {
                location: loc.clone(),
            },
        );

        let store = GraphStore::from_graph("test", graph);

        let call_edges = store.edges_by_category("call");
        assert_eq!(call_edges.len(), 1);
        assert!(matches!(
            store.graph()[call_edges[0]],
            crate::graph::Edge::DirectCall { .. }
        ));

        let dataflow_edges = store.edges_by_category("dataflow");
        assert_eq!(dataflow_edges.len(), 1);
        assert!(matches!(
            store.graph()[dataflow_edges[0]],
            crate::graph::Edge::TableAccess { .. }
        ));

        let reference_edges = store.edges_by_category("reference");
        assert_eq!(reference_edges.len(), 1);
        assert!(matches!(
            store.graph()[reference_edges[0]],
            crate::graph::Edge::TriggersRoutine { .. }
        ));

        let composition_edges = store.edges_by_category("composition");
        assert!(composition_edges.is_empty());
    }

    #[test]
    fn dead_routines_finds_unreferenced_procs() {
        let mut graph = CodeGraph::new();
        let file = std::sync::Arc::new(std::path::PathBuf::from("a.sql"));
        let called = graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: None,
                package: None,
                name: "called".into(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: crate::graph::SourceLocation {
                file: file.clone(),
                line: 1,
            },
            partial: false,
        });
        let orphan = graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: None,
                package: None,
                name: "orphan".into(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: crate::graph::SourceLocation {
                file: file.clone(),
                line: 2,
            },
            partial: false,
        });
        let caller = graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: None,
                package: None,
                name: "caller".into(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: crate::graph::SourceLocation {
                file: file.clone(),
                line: 3,
            },
            partial: false,
        });
        graph.add_edge(
            caller,
            called,
            crate::graph::Edge::DirectCall {
                scope: crate::graph::CallScope::IntraPackage,
                location: crate::graph::SourceLocation {
                    file: file.clone(),
                    line: 3,
                },
            },
        );

        let store = GraphStore::from_graph("test", graph);
        let dead = store.dead_routines();
        assert!(dead.contains(&orphan));
        assert!(!dead.contains(&called));
        assert!(
            !dead.contains(&caller),
            "caller has no incoming edges but it calls something"
        );
    }

    #[test]
    fn find_cycles_detects_mutual_calls() {
        let mut graph = CodeGraph::new();
        let file = std::sync::Arc::new(std::path::PathBuf::from("a.sql"));
        let a = graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: None,
                package: None,
                name: "a".into(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: crate::graph::SourceLocation {
                file: file.clone(),
                line: 1,
            },
            partial: false,
        });
        let b = graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: None,
                package: None,
                name: "b".into(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: crate::graph::SourceLocation {
                file: file.clone(),
                line: 2,
            },
            partial: false,
        });
        graph.add_edge(
            a,
            b,
            crate::graph::Edge::DirectCall {
                scope: crate::graph::CallScope::IntraPackage,
                location: crate::graph::SourceLocation {
                    file: file.clone(),
                    line: 1,
                },
            },
        );
        graph.add_edge(
            b,
            a,
            crate::graph::Edge::DirectCall {
                scope: crate::graph::CallScope::IntraPackage,
                location: crate::graph::SourceLocation {
                    file: file.clone(),
                    line: 2,
                },
            },
        );

        let store = GraphStore::from_graph("test", graph);
        let cycles = store.find_cycles();
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].len(), 2);
    }

    #[test]
    fn impact_traces_backward() {
        let mut graph = CodeGraph::new();
        let file = std::sync::Arc::new(std::path::PathBuf::from("a.sql"));
        let target = graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: None,
                package: None,
                name: "target".into(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: crate::graph::SourceLocation {
                file: file.clone(),
                line: 1,
            },
            partial: false,
        });
        let caller = graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: None,
                package: None,
                name: "caller".into(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: crate::graph::SourceLocation {
                file: file.clone(),
                line: 2,
            },
            partial: false,
        });
        let grandcaller = graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: None,
                package: None,
                name: "grandcaller".into(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: crate::graph::SourceLocation {
                file: file.clone(),
                line: 3,
            },
            partial: false,
        });
        graph.add_edge(
            caller,
            target,
            crate::graph::Edge::DirectCall {
                scope: crate::graph::CallScope::IntraPackage,
                location: crate::graph::SourceLocation {
                    file: file.clone(),
                    line: 2,
                },
            },
        );
        graph.add_edge(
            grandcaller,
            caller,
            crate::graph::Edge::DirectCall {
                scope: crate::graph::CallScope::IntraPackage,
                location: crate::graph::SourceLocation {
                    file: file.clone(),
                    line: 3,
                },
            },
        );

        let store = GraphStore::from_graph("test", graph);
        let impacted = store.impact(target, None);
        assert_eq!(impacted.len(), 2);
        assert!(impacted.contains(&caller));
        assert!(impacted.contains(&grandcaller));
    }

    fn make_proc(schema: Option<&str>, package: Option<&str>, name: &str) -> crate::graph::Node {
        crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: schema.map(String::from),
                package: package.map(String::from),
                name: name.to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: crate::graph::SourceLocation {
                file: std::sync::Arc::new(std::path::PathBuf::from("test.sql")),
                line: 1,
            },
            partial: false,
        }
    }

    #[test]
    fn merge_relaxed_match_schema_vs_no_schema() {
        // Store A: SQL analysis produces procedures WITH schema
        let mut graph_a = CodeGraph::new();
        let p1 = graph_a.add_node(make_proc(
            Some("BIGFUND"),
            Some("PKG_IMPORT_EXCEL"),
            "PROC_IMPORT_EXCEL",
        ));
        let p2 = graph_a.add_node(make_proc(
            Some("BIGFUND"),
            Some("PKG_SQS_CASH_MANAGE"),
            "PROC_UPDATE_TRAN_STATUS",
        ));
        graph_a.add_edge(
            p1,
            p2,
            crate::graph::Edge::DirectCall {
                scope: crate::graph::CallScope::CrossPackage,
                location: crate::graph::SourceLocation {
                    file: std::sync::Arc::new(std::path::PathBuf::from("a.sql")),
                    line: 10,
                },
            },
        );
        let store_a = GraphStore::from_graph("sql-analysis", graph_a);

        // Store B: CGEF import produces procedures WITHOUT schema
        let mut graph_b = CodeGraph::new();
        let p3 = graph_b.add_node(make_proc(
            None,
            Some("pkg_import_excel"),
            "proc_import_excel",
        ));
        let table = graph_b.add_node(crate::graph::Node::Table {
            schema: Some("public".to_string()),
            name: "t_orders".to_string(),
            location: None,
            columns: Box::new(vec![]),
            partition_by: None,
            distribute_by: None,
            tablespace: None,
            temporary: false,
            unlogged: false,
            ddl_source: None,
        });
        graph_b.add_edge(
            p3,
            table,
            crate::graph::Edge::TableAccess {
                flow_kind: crate::graph::DataFlowKind::DmlAccess,
                modes: crate::graph::AccessMode::Write,
                write_kinds: std::collections::HashSet::new(),
                location: crate::graph::SourceLocation {
                    file: std::sync::Arc::new(std::path::PathBuf::from("lineage/excel")),
                    line: 0,
                },
                column_analysis: None,
            },
        );
        let store_b = GraphStore::from_graph("cgef-import", graph_b);

        let merged = GraphStore::merge(vec![store_a, store_b], "combined");

        // PROC_IMPORT_EXCEL should have been merged into one node (schema-relaxed match)
        assert_eq!(
            merged.graph().node_count(),
            3,
            "expected 3 nodes (2 procs + 1 table), got {}",
            merged.graph().node_count()
        );
        assert_eq!(
            merged.graph().edge_count(),
            2,
            "expected 2 edges (call + table_access), got {}",
            merged.graph().edge_count()
        );

        // Verify the table access edge points from the merged procedure to the table
        let proc_nodes: Vec<_> = merged
            .graph()
            .node_indices()
            .filter(|i| matches!(merged.graph()[*i], crate::graph::Node::Procedure { .. }))
            .collect();
        let import_excel_nodes: Vec<_> = proc_nodes
            .iter()
            .filter(|&&i| {
                let key = crate::graph::key::NodeKey::from_node(&merged.graph()[i]);
                key.to_string().contains("import_excel")
            })
            .collect();
        assert_eq!(
            import_excel_nodes.len(),
            1,
            "PROC_IMPORT_EXCEL should be a single merged node"
        );
    }

    #[test]
    fn merge_case_insensitive_procedure_keys() {
        let mut graph_a = CodeGraph::new();
        graph_a.add_node(make_proc(Some("BIGFUND"), Some("PKG_FOO"), "DO_WORK"));
        let store_a = GraphStore::from_graph("a", graph_a);

        let mut graph_b = CodeGraph::new();
        graph_b.add_node(make_proc(Some("bigfund"), Some("pkg_foo"), "do_work"));
        let store_b = GraphStore::from_graph("b", graph_b);

        let merged = GraphStore::merge(vec![store_a, store_b], "combined");
        assert_eq!(
            merged.graph().node_count(),
            1,
            "same procedure with different case should deduplicate to 1 node"
        );
    }

    #[test]
    fn merge_no_schema_matches_schema_qualified() {
        let mut graph_a = CodeGraph::new();
        graph_a.add_node(make_proc(
            Some("bigfund"),
            Some("pkg_import_excel"),
            "proc_import_excel",
        ));
        let store_a = GraphStore::from_graph("a", graph_a);

        let mut graph_b = CodeGraph::new();
        graph_b.add_node(make_proc(
            None,
            Some("pkg_import_excel"),
            "proc_import_excel",
        ));
        let store_b = GraphStore::from_graph("b", graph_b);

        let merged = GraphStore::merge(vec![store_a, store_b], "combined");
        assert_eq!(
            merged.graph().node_count(),
            1,
            "no-schema procedure should match schema-qualified via relaxed key"
        );
    }

    #[test]
    fn merge_different_procedures_stay_separate() {
        let mut graph_a = CodeGraph::new();
        graph_a.add_node(make_proc(Some("bigfund"), Some("pkg_foo"), "proc_a"));
        let store_a = GraphStore::from_graph("a", graph_a);

        let mut graph_b = CodeGraph::new();
        graph_b.add_node(make_proc(None, Some("pkg_bar"), "proc_b"));
        let store_b = GraphStore::from_graph("b", graph_b);

        let merged = GraphStore::merge(vec![store_a, store_b], "combined");
        assert_eq!(
            merged.graph().node_count(),
            2,
            "different procedures should remain separate"
        );
    }

    #[test]
    fn merge_populates_node_summaries() {
        let file = std::sync::Arc::new(std::path::PathBuf::from("a.sql"));
        let loc = crate::graph::SourceLocation { file, line: 1 };

        let mut graph_a = CodeGraph::new();
        let proc_a = graph_a.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("s1".into()),
                package: None,
                name: "proc_a".into(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
        });
        let table = graph_a.add_node(crate::graph::Node::Table {
            schema: Some("public".into()),
            name: "orders".into(),
            location: None,
            columns: Box::new(vec![]),
            partition_by: None,
            distribute_by: None,
            tablespace: None,
            temporary: false,
            unlogged: false,
            ddl_source: None,
        });
        graph_a.add_edge(
            proc_a,
            table,
            crate::graph::Edge::TableAccess {
                flow_kind: crate::graph::DataFlowKind::DmlAccess,
                modes: crate::graph::AccessMode::Read,
                write_kinds: std::collections::HashSet::new(),
                location: loc.clone(),
                column_analysis: None,
            },
        );
        let store_a = GraphStore::from_graph("a", graph_a);

        let mut graph_b = CodeGraph::new();
        graph_b.add_node(crate::graph::Node::Function {
            id: crate::graph::RoutineId {
                schema: Some("s2".into()),
                package: None,
                name: "func_b".into(),
                kind: crate::graph::RoutineKind::Function,
            },
            location: loc,
            partial: false,
        });
        let store_b = GraphStore::from_graph("b", graph_b);

        let merged = GraphStore::merge(vec![store_a, store_b], "combined");

        assert_eq!(merged.graph().node_count(), 3);

        let summaries = merged.node_summaries();
        assert_eq!(summaries.len(), 3, "merge must rebuild node_summaries");

        let proc_summary = summaries.iter().find(|s| s.key.contains("proc_a")).unwrap();
        assert_eq!(proc_summary.type_tag, "proc");
        assert_eq!(proc_summary.out_degree, 1);
        assert_eq!(proc_summary.in_degree, 0);

        let table_summary = summaries.iter().find(|s| s.key.contains("orders")).unwrap();
        assert_eq!(table_summary.type_tag, "table");
        assert_eq!(table_summary.in_degree, 1);
        assert_eq!(table_summary.out_degree, 0);

        let func_summary = summaries.iter().find(|s| s.key.contains("func_b")).unwrap();
        assert_eq!(func_summary.type_tag, "func");
        assert_eq!(func_summary.in_degree, 0);
        assert_eq!(func_summary.out_degree, 0);

        let stats = merged.stats();
        assert_eq!(stats.procedures, 1);
        assert_eq!(stats.functions, 1);
        assert_eq!(stats.tables, 1);
    }

    #[test]
    fn ensure_consistency_rebuilds_stale_indexes() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_proc(Some("public"), None, "do_work"));
        graph.add_node(make_proc(Some("public"), None, "other"));
        let mut store = GraphStore::from_graph("test", graph);

        assert_eq!(store.node_summaries().len(), 2);
        assert_eq!(store.name_index().len(), 2);

        store.node_summaries.clear();
        store.name_index.clear();
        store.type_tag_index.clear();
        assert!(store.node_summaries().is_empty());

        store.ensure_consistency();

        assert_eq!(store.node_summaries().len(), 2, "node_summaries rebuilt");
        assert_eq!(store.name_index().len(), 2, "name_index rebuilt");
        assert_eq!(store.type_tag_index.get("proc").map(|v| v.len()), Some(2),);
    }

    #[test]
    fn ensure_consistency_noop_when_consistent() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_proc(Some("public"), None, "do_work"));
        let mut store = GraphStore::from_graph("test", graph);

        let summaries_before = store.node_summaries().len();
        store.ensure_consistency();
        assert_eq!(store.node_summaries().len(), summaries_before);
    }

    #[test]
    fn sql_text_matches_multiline() {
        let sql = "DELETE FROM bigfund.dat_batch_task_aspect_log\n        WHERE data_date < TO_CHAR(TRUNC(SYSDATE) - 15, 'YYYYMMDD')";
        let query = "delete from bigfund.dat_batch_task_aspect_log where data_date";
        assert!(
            sql_text_matches(sql, query),
            "should match across line break"
        );
    }

    #[test]
    fn sql_text_matches_multiline_with_wildcard() {
        let sql = "SELECT *\nFROM   my_table\nWHERE  id = ?";
        let query = "select * from my_table where id = ?";
        assert!(
            sql_text_matches(sql, query),
            "should match with normalized whitespace"
        );
    }

    #[test]
    fn sql_text_matches_tabs_and_spaces() {
        let sql = "SELECT\t*\tFROM\tmy_table  WHERE\tx = 1";
        let query = "select * from my_table where x = 1";
        assert!(
            sql_text_matches(sql, query),
            "should normalize tabs and multiple spaces"
        );
    }

    #[test]
    fn sql_text_matches_no_false_positive() {
        let sql = "SELECT FROM_TABLE FROM my_table";
        let query = "from table from my";
        assert!(
            !sql_text_matches(sql, query),
            "should not match unrelated fragments"
        );
    }

    #[test]
    fn sql_text_matches_exact_no_change() {
        let sql = "SELECT * FROM my_table WHERE id = 1";
        let query = "select * from my_table";
        assert!(
            sql_text_matches(sql, query),
            "exact match should still work"
        );
    }

    #[test]
    fn sql_text_matches_carriage_return() {
        let sql = "DELETE FROM table\r\nWHERE id = 1";
        let query = "delete from table where id = 1";
        assert!(
            sql_text_matches(sql, query),
            "should handle CRLF line endings"
        );
    }

    #[test]
    fn sql_text_matches_operator_spacing() {
        let sql =
            "SELECT * FROM t_orders WHERE user_id = __XML_PARAM_userId__ AND status = 'CREATED'";
        let query = "select * from t_orders where user_id=?";
        assert!(
            sql_text_matches(sql, query),
            "should match despite different spacing around ="
        );
    }

    #[test]
    fn sql_text_matches_operator_no_space_query() {
        let sql = "SELECT * FROM t_orders WHERE user_id = ? AND status = 'CREATED'";
        let query = "select   * from   t_orders where user_id=?";
        assert!(
            sql_text_matches(sql, query),
            "should match with extra spaces in query and no space around ="
        );
    }

    #[test]
    fn sql_text_matches_greater_than_operator() {
        let sql = "SELECT * FROM logs WHERE created_at >= ? AND type = 'ERROR'";
        let query = "select * from logs where created_at>=?";
        assert!(
            sql_text_matches(sql, query),
            "should match >= operator with different spacing"
        );
    }

    #[test]
    fn sql_text_matches_paren_spacing() {
        let sql = "SELECT TO_CHAR( TRUNC(SYSDATE) - 15 , 'YYYYMMDD' ) FROM dual";
        let query = "select to_char(trunc(sysdate)-?,'yyyymmdd') from dual";
        assert!(
            sql_text_matches(sql, query),
            "should match despite different paren/comma spacing"
        );
    }

    #[test]
    fn sql_text_matches_nested_paren_spacing() {
        let sql = "DELETE FROM t WHERE dt < TO_CHAR( TRUNC( SYSDATE ) - ? , ? )";
        let query = "delete from t where dt < to_char(trunc(sysdate)-?,?)";
        assert!(
            sql_text_matches(sql, query),
            "should match nested function calls with extra spaces around parens"
        );
    }

    #[test]
    fn sql_text_matches_comma_list_spacing() {
        let sql = "SELECT MAX( id ) , MIN( name ) FROM t";
        let query = "select max(id),min(name) from t";
        assert!(
            sql_text_matches(sql, query),
            "should match comma-separated list with different spacing"
        );
    }

    #[test]
    fn sql_text_matches_xml_raw_placeholder_as_wildcard() {
        let sql = "UPDATE __XML_RAW_tableName__ t SET t.req_host_ip = __XML_PARAM_hostIp__, t.file_type = __XML_PARAM_fileType__ WHERE t.data_date = __XML_PARAM_dataDate__";
        let query = "update ? t set t.req_host_ip = ?";
        assert!(
            sql_text_matches(sql, query),
            "__XML_RAW__ and __XML_PARAM__ should be normalized to ? so query ? wildcard matches"
        );
    }

    #[test]
    fn sql_text_matches_xml_raw_with_query_wildcard() {
        let sql = "UPDATE __XML_RAW_tableName__ t SET t.status = '1'";
        let query = "update ? t set t.status='1'";
        assert!(
            sql_text_matches(sql, query),
            "? wildcard in query should match __XML_RAW__ placeholder in SQL"
        );
    }

    #[test]
    fn sql_text_matches_xml_param_chain() {
        let sql = "SELECT * FROM orders WHERE user_id = __XML_PARAM_userId__ AND status = __XML_PARAM_status__";
        let query = "select * from orders where user_id=? and status=?";
        assert!(
            sql_text_matches(sql, query),
            "should match multiple __XML_PARAM__ placeholders with ? wildcards"
        );
    }

    #[test]
    fn sql_text_matches_xml_raw_with_type_hint() {
        let sql = "SELECT __XML_RAW_STRING_column__ FROM users";
        let query = "select ? from users";
        assert!(
            sql_text_matches(sql, query),
            "__XML_RAW_STRING_*__ should be normalized to ? so query ? wildcard matches"
        );
    }

    #[test]
    fn sql_text_matches_xml_raw_generic_pattern_matches_concrete() {
        let sql = "SELECT __XML_RAW_tableName__ FROM users";
        let query = "select orders from users";
        assert!(
            sql_text_matches(sql, query),
            "generic ${{tableName}} pattern should match concrete table name via bidirectional wildcard"
        );
    }

    #[test]
    fn sql_text_matches_xml_raw_concrete_value_in_query() {
        let sql = "UPDATE __XML_RAW_tableName__ t SET t.req_host_ip = __XML_PARAM_hostIp__, t.file_type = __XML_PARAM_fileType__ WHERE t.data_date = __XML_PARAM_dataDate__";
        let query = "update dat_mdb_text t set t.req_host_ip = ?";
        assert!(
            sql_text_matches(sql, query),
            "concrete table name in query should match __XML_RAW__ via bidirectional wildcard"
        );
    }

    #[test]
    fn sql_text_matches_xml_raw_concrete_without_query_wildcard() {
        let sql = "UPDATE __XML_RAW_tableName__ t SET t.status = '1'";
        let query = "update dat_mdb_text t set t.status='1'";
        assert!(
            sql_text_matches(sql, query),
            "concrete table name should match __XML_RAW__ even without query ? wildcard"
        );
    }

    #[test]
    fn sql_text_matches_fully_dynamic_sql_is_excluded() {
        let sql = "__XML_RAW_I_am_Free_SQL__";
        let query = "select * from a where user_id=?";
        assert!(
            !sql_text_matches(sql, query),
            "fully dynamic SQL (normalizes to all ?) must not match specific queries"
        );
    }

    #[test]
    fn sql_text_matches_fully_dynamic_sql_no_match_without_query_wildcard() {
        let sql = "__XML_RAW_anything__";
        let query = "select * from orders";
        assert!(
            !sql_text_matches(sql, query),
            "fully dynamic SQL must not match even without query wildcard"
        );
    }

    #[test]
    fn sql_text_matches_query_with_wildcard_subset_ok() {
        let sql = "SELECT * FROM __XML_RAW_tableName__ WHERE user_id = __XML_PARAM_userId__ AND status = 'CREATED'";
        let query = "select * from a where user_id=?";
        assert!(
            sql_text_matches(sql, query),
            "query ending with ? should match SQL with extra conditions absorbed by the wildcard"
        );
    }

    #[test]
    fn sql_text_matches_query_with_extra_condition_rejected() {
        let sql = "SELECT * FROM __XML_RAW_tableName__ WHERE user_id = __XML_PARAM_userId__ AND status = 'CREATED'";
        let query = "select * from a where user_id=? and q=t";
        assert!(
            !sql_text_matches(sql, query),
            "query with extra conditions not in SQL must not match"
        );
    }

    #[test]
    fn sql_text_matches_query_with_multiple_extra_conditions_rejected() {
        let sql = "SELECT * FROM __XML_RAW_tableName__ WHERE user_id = __XML_PARAM_userId__ AND status = 'CREATED'";
        let query = "select * from a where user_id=? and q=t and ttt is true";
        assert!(
            !sql_text_matches(sql, query),
            "query with multiple extra conditions not in SQL must not match"
        );
    }

    #[test]
    fn sql_text_matches_query_no_wildcard_value_in_tail_ok() {
        let sql = "SELECT * FROM __XML_RAW_tableName__ WHERE user_id = __XML_PARAM_userId__ AND status = 'CREATED'";
        let query = "select * from orders where user_id=123";
        assert!(
            sql_text_matches(sql, query),
            "query without ? should match even if tail has parameter value for SQL ?"
        );
    }

    #[test]
    fn test_bincode_roundtrip_with_non_utf8_path() {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            use std::sync::Arc;

            let raw = std::ffi::OsStr::from_bytes(b"/some/\xff\xfe/path.sql");
            let raw_path = std::path::PathBuf::from(raw);
            let sanitized = crate::parser::scanner::sanitize_path(&raw_path);
            assert!(sanitized.to_str().is_some(), "sanitized should be UTF-8");

            let mut graph = CodeGraph::new();
            let proc = crate::graph::Node::Procedure {
                id: crate::graph::RoutineId {
                    schema: None,
                    package: None,
                    name: "test_proc".to_string(),
                    kind: crate::graph::RoutineKind::Procedure,
                },
                location: crate::graph::SourceLocation {
                    file: Arc::new(sanitized),
                    line: 1,
                },
                partial: false,
            };
            graph.add_node(proc);

            let store = GraphStore::from_graph("test", graph);
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("test.bincode");

            store
                .save_bincode(&path)
                .expect("bincode serialize should succeed with sanitized path");

            let loaded = GraphStore::load_bincode(&path).unwrap();
            assert_eq!(loaded.graph().node_count(), 1);
        }
    }
}

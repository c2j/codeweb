use crate::graph::key::NodeKey;
use crate::graph::node_type_tag;
use crate::graph::CodeGraph;
use crate::graph::Node;
use crate::parser::fingerprint::FileRecord;
use crate::sql_match;
use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Magic bytes prefixing every `store.bincode` file. Lets `load_bincode`
/// validate the file and check the format version *before* attempting bincode
/// deserialization (which fails opaquely on cross-version layout drift).
/// See issue #110.
const STORE_MAGIC: [u8; 9] = *b"CWEBSTORE";

/// GraphStore on-disk format version. Bump when the serialized struct layout
/// changes. Validated in the file header (post-header era files) and again in
/// `GraphStore.version` after deserialize (legacy files + belt-and-suspenders).
const STORE_VERSION: u32 = 6;

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
    /// Index: normalized SQL fingerprint → list of (NodeIndex, display_key)
    /// Built from MappedStatement and JavaSql nodes for O(1) lookup.
    #[serde(default)]
    sql_fingerprint_index: HashMap<String, Vec<(NodeIndex, String)>>,
    /// Index: lock clause kind key → list of (NodeIndex, display_key)
    /// Built from ogsql-parser AST for O(1) lookup of FOR UPDATE / FOR SHARE etc.
    #[serde(default)]
    lock_clause_index: HashMap<String, Vec<(NodeIndex, String)>>,
}

#[allow(dead_code)]
impl GraphStore {
    pub fn new(project_name: &str) -> Self {
        let now = timestamp_ms();
        Self {
            version: STORE_VERSION,
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
            sql_fingerprint_index: HashMap::new(),
            lock_clause_index: HashMap::new(),
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

        let mut sql_fingerprint_index: HashMap<String, Vec<(NodeIndex, String)>> = HashMap::new();
        for idx in graph.node_indices() {
            match &graph[idx] {
                Node::MappedStatement {
                    sql: Some(sql_text),
                    namespace,
                    statement_id,
                    ..
                } => {
                    let fp = sql_match::sql_fingerprint(sql_text);
                    let display_key = format!("mapper:{}.{}", namespace, statement_id);
                    sql_fingerprint_index
                        .entry(fp)
                        .or_default()
                        .push((idx, display_key));
                }
                Node::JavaSql {
                    sql: Some(sql_text),
                    class_name,
                    method_name,
                    line,
                    ..
                } => {
                    let fp = sql_match::sql_fingerprint(sql_text);
                    let ctx = match (class_name, method_name) {
                        (Some(c), Some(m)) => format!("{}.{}", c, m),
                        (Some(c), None) => c.clone(),
                        (None, Some(m)) => m.clone(),
                        (None, None) => "?".to_string(),
                    };
                    let display_key = format!("javasql:{}:{}", ctx, line);
                    sql_fingerprint_index
                        .entry(fp)
                        .or_default()
                        .push((idx, display_key));
                }
                Node::Procedure { id, body_sql, .. } => {
                    for sql in body_sql {
                        let fp = sql_match::sql_fingerprint(&sql.sql_text);
                        let display_key = format!("proc:{}", id);
                        sql_fingerprint_index
                            .entry(fp)
                            .or_default()
                            .push((idx, display_key));
                    }
                }
                Node::Function { id, body_sql, .. } => {
                    for sql in body_sql {
                        let fp = sql_match::sql_fingerprint(&sql.sql_text);
                        let display_key = format!("func:{}", id);
                        sql_fingerprint_index
                            .entry(fp)
                            .or_default()
                            .push((idx, display_key));
                    }
                }
                _ => {}
            }
        }

        let mut lock_clause_index: HashMap<String, Vec<(NodeIndex, String)>> = HashMap::new();
        for idx in graph.node_indices() {
            let (display_key, sql_texts) = match &graph[idx] {
                Node::MappedStatement {
                    sql: Some(sql_text),
                    namespace,
                    statement_id,
                    ..
                } => (
                    format!("mapper:{}.{}", namespace, statement_id),
                    vec![sql_text.as_str()],
                ),
                Node::JavaSql {
                    sql: Some(sql_text),
                    class_name,
                    method_name,
                    line,
                    ..
                } => {
                    let ctx = match (class_name, method_name) {
                        (Some(c), Some(m)) => format!("{}.{}", c, m),
                        (Some(c), None) => c.clone(),
                        (None, Some(m)) => m.clone(),
                        (None, None) => "?".to_string(),
                    };
                    (format!("javasql:{}:{}", ctx, line), vec![sql_text.as_str()])
                }
                Node::Procedure { id, body_sql, .. } => (
                    format!("proc:{}", id),
                    body_sql.iter().map(|s| s.sql_text.as_str()).collect(),
                ),
                Node::Function { id, body_sql, .. } => (
                    format!("func:{}", id),
                    body_sql.iter().map(|s| s.sql_text.as_str()).collect(),
                ),
                _ => continue,
            };
            for sql in sql_texts {
                if let Some(kind) = sql_match::detect_lock_clause_in_sql(sql) {
                    lock_clause_index
                        .entry(kind.index_key().to_string())
                        .or_default()
                        .push((idx, display_key.clone()));
                }
            }
        }

        Self {
            version: STORE_VERSION,
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
            sql_fingerprint_index,
            lock_clause_index,
        }
    }

    pub fn graph(&self) -> &CodeGraph {
        &self.graph
    }

    #[allow(dead_code)]
    pub fn partition(
        &self,
        config: &crate::graph::cluster::ClusterConfig,
    ) -> crate::graph::cluster::PartitionReport {
        crate::graph::cluster::partition(&self.graph, config)
    }

    pub fn ensure_consistency(&mut self) {
        let expected = self.graph.node_count();
        let needs_rebuild = self.node_summaries.len() != expected
            || self.name_index.len() != expected
            || self.type_tag_index.values().map(|v| v.len()).sum::<usize>() != expected;

        if needs_rebuild {
            eprintln!(
                "store: stale indexes (node_summaries {}/{}, name_index {}, type_tag_index total {}), rebuilding...",
                self.node_summaries.len(),
                expected,
                self.name_index.len(),
                self.type_tag_index.values().map(|v| v.len()).sum::<usize>(),
            );
            self.rebuild_secondary_indexes();
        }
    }

    pub fn ensure_consistency_with_progress(&mut self) {
        let expected = self.graph.node_count();
        let needs_rebuild = self.node_summaries.len() != expected
            || self.name_index.len() != expected
            || self.type_tag_index.values().map(|v| v.len()).sum::<usize>() != expected;

        if !needs_rebuild {
            return;
        }

        let pb = indicatif::ProgressBar::new(expected as u64);
        pb.set_style(
            indicatif::ProgressStyle::with_template(
                "  {spinner} Rebuilding indexes {bar:40.cyan/blue} {pos}/{len} {msg}",
            )
            .unwrap()
            .progress_chars("━━╾─"),
        );
        pb.set_message("node indexes...");

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

            pb.inc(1);
        }

        pb.set_message("sorting...");
        self.name_index.sort_by(|a, b| a.0.cmp(&b.0));

        pb.set_style(
            indicatif::ProgressStyle::with_template("  {spinner} Rebuilding edge indexes...")
                .unwrap(),
        );
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

        pb.set_message("fingerprint index...");
        self.sql_fingerprint_index.clear();
        for idx in self.graph.node_indices() {
            match &self.graph[idx] {
                Node::MappedStatement {
                    sql: Some(sql_text),
                    namespace,
                    statement_id,
                    ..
                } => {
                    let fp = sql_match::sql_fingerprint(sql_text);
                    let display_key = format!("mapper:{}.{}", namespace, statement_id);
                    self.sql_fingerprint_index
                        .entry(fp)
                        .or_default()
                        .push((idx, display_key));
                }
                Node::JavaSql {
                    sql: Some(sql_text),
                    class_name,
                    method_name,
                    line,
                    ..
                } => {
                    let fp = sql_match::sql_fingerprint(sql_text);
                    let ctx = match (class_name, method_name) {
                        (Some(c), Some(m)) => format!("{}.{}", c, m),
                        (Some(c), None) => c.clone(),
                        (None, Some(m)) => m.clone(),
                        (None, None) => "?".to_string(),
                    };
                    let display_key = format!("javasql:{}:{}", ctx, line);
                    self.sql_fingerprint_index
                        .entry(fp)
                        .or_default()
                        .push((idx, display_key));
                }
                _ => {}
            }
        }

        pb.set_message("lock clause index...");
        self.lock_clause_index.clear();
        for idx in self.graph.node_indices() {
            let (display_key, sql_texts) = match &self.graph[idx] {
                Node::MappedStatement {
                    sql: Some(sql_text),
                    namespace,
                    statement_id,
                    ..
                } => (
                    format!("mapper:{}.{}", namespace, statement_id),
                    vec![sql_text.as_str()],
                ),
                Node::JavaSql {
                    sql: Some(sql_text),
                    class_name,
                    method_name,
                    line,
                    ..
                } => {
                    let ctx = match (class_name, method_name) {
                        (Some(c), Some(m)) => format!("{}.{}", c, m),
                        (Some(c), None) => c.clone(),
                        (None, Some(m)) => m.clone(),
                        (None, None) => "?".to_string(),
                    };
                    (format!("javasql:{}:{}", ctx, line), vec![sql_text.as_str()])
                }
                Node::Procedure { id, body_sql, .. } => (
                    format!("proc:{}", id),
                    body_sql.iter().map(|s| s.sql_text.as_str()).collect(),
                ),
                Node::Function { id, body_sql, .. } => (
                    format!("func:{}", id),
                    body_sql.iter().map(|s| s.sql_text.as_str()).collect(),
                ),
                _ => continue,
            };
            for sql in sql_texts {
                if let Some(kind) = sql_match::detect_lock_clause_in_sql(sql) {
                    self.lock_clause_index
                        .entry(kind.index_key().to_string())
                        .or_default()
                        .push((idx, display_key.clone()));
                }
            }
        }

        pb.finish_with_message(format!(
            "Indexes rebuilt ({} nodes, {} edges)",
            expected,
            self.graph.edge_count()
        ));
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
                Node::BuiltinFunction { .. } => s.builtin_functions += 1,
                Node::Custom { .. } => s.custom_nodes += 1,
                #[cfg(feature = "jsp")]
                Node::JspPage { .. } => s.jsp_pages += 1,
                #[cfg(feature = "jsp")]
                Node::JspSql { .. } => s.jsp_sql += 1,
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

    pub fn sql_fingerprint_index(&self) -> &HashMap<String, Vec<(NodeIndex, String)>> {
        &self.sql_fingerprint_index
    }

    pub fn lock_clause_index(&self) -> &HashMap<String, Vec<(NodeIndex, String)>> {
        &self.lock_clause_index
    }

    /// Enrich the SQL fingerprint index with expanded dynamic SQL variants.
    /// For each mapper node that has dynamic elements, expands all possible SQL variants
    /// and adds their fingerprints to the index.
    pub fn enrich_fingerprint_index_with_variants(
        &mut self,
        variant_map: &HashMap<String, Vec<String>>,
    ) {
        for (mapper_key, variant_sqls) in variant_map {
            let node_idx = match self.node_key_index.get(&NodeKey::Mapper {
                namespace: mapper_key
                    .rsplit_once('.')
                    .map(|(ns, _)| ns.to_string())
                    .unwrap_or_default(),
                statement_id: mapper_key
                    .rsplit_once('.')
                    .map(|(_, id)| id.to_string())
                    .unwrap_or_default(),
            }) {
                Some(idx) => *idx,
                None => continue,
            };
            let display_key = format!("mapper:{}", mapper_key);
            for variant_sql in variant_sqls {
                let fp = sql_match::sql_fingerprint(variant_sql);
                self.sql_fingerprint_index
                    .entry(fp)
                    .or_default()
                    .push((node_idx, display_key.clone()));
            }
        }
    }

    /// Search nodes by SQL text content (substring match, case-insensitive).
    /// Checks MappedStatement.sql, JavaSql.sql, Procedure.body_sql, Function.body_sql.
    /// Returns Vec of (NodeIndex, display_key, relevance_score) sorted by score descending,
    /// then by node type prefix and display key as tiebreakers.
    pub fn search_by_sql(&self, query: &str) -> Vec<(NodeIndex, String, f64)> {
        let normalized = sql_match::normalize_for_matching(&query.to_lowercase());
        let fp = blake3::hash(normalized.as_bytes()).to_hex().to_string();
        if let Some(hits) = self.sql_fingerprint_index.get(&fp) {
            if !hits.is_empty() {
                let mut results: Vec<(NodeIndex, String, f64)> = hits
                    .iter()
                    .map(|(idx, key)| (*idx, key.clone(), 1.0))
                    .collect();
                sql_match::sort_scored_results(&mut results);
                return results;
            }
        }

        if let Some(kind) = sql_match::classify_lock_clause_query(query) {
            let key = kind.index_key().to_string();
            if let Some(hits) = self.lock_clause_index.get(&key) {
                if !hits.is_empty() {
                    let mut results: Vec<(NodeIndex, String, f64)> = hits
                        .iter()
                        .map(|(idx, display_key)| (*idx, display_key.clone(), 0.9))
                        .collect();
                    sql_match::sort_scored_results(&mut results);
                    return results;
                }
            }
        }

        let prepared = sql_match::PreparedQuery::new(query);
        let mut results: Vec<(NodeIndex, String, f64)> = Vec::new();
        for idx in self.graph.node_indices() {
            match &self.graph[idx] {
                Node::MappedStatement {
                    sql: Some(sql_text),
                    namespace,
                    statement_id,
                    ..
                } => {
                    if prepared.matches(sql_text) {
                        let score = prepared.score(sql_text);
                        results.push((
                            idx,
                            format!("mapper:{}.{}", namespace, statement_id),
                            score,
                        ));
                    }
                }
                Node::JavaSql {
                    sql: Some(sql_text),
                    class_name,
                    method_name,
                    line,
                    ..
                } if prepared.matches(sql_text) => {
                    let ctx = match (class_name, method_name) {
                        (Some(c), Some(m)) => format!("{}.{}", c, m),
                        (Some(c), None) => c.clone(),
                        (None, Some(m)) => m.clone(),
                        (None, None) => "?".to_string(),
                    };
                    let score = prepared.score(sql_text);
                    results.push((idx, format!("javasql:{}:{}", ctx, line), score));
                }
                Node::Procedure { id, body_sql, .. } => {
                    let mut best: Option<f64> = None;
                    for sql in body_sql {
                        if prepared.matches(&sql.sql_text) {
                            let s = prepared.score(&sql.sql_text);
                            best = Some(best.map_or(s, |b| b.max(s)));
                        }
                    }
                    if let Some(score) = best {
                        results.push((idx, format!("proc:{}", id), score));
                    }
                }
                Node::Function { id, body_sql, .. } => {
                    let mut best: Option<f64> = None;
                    for sql in body_sql {
                        if prepared.matches(&sql.sql_text) {
                            let s = prepared.score(&sql.sql_text);
                            best = Some(best.map_or(s, |b| b.max(s)));
                        }
                    }
                    if let Some(score) = best {
                        results.push((idx, format!("func:{}", id), score));
                    }
                }
                _ => {}
            }
        }
        sql_match::sort_scored_results(&mut results);
        results
    }
}

// SQL matching functions moved to crate::sql_match

/// Returns true if the query looks like a plain text search (no wildcards
/// or regex metacharacters), making it safe for binary search optimization.
fn is_simple_query(lower: &str) -> bool {
    !lower.contains('*') && !lower.contains('?') && !lower.contains('\\')
}

impl GraphStore {
    /// Search nodes by name using the sorted name_index.
    /// Returns Vec of (NodeIndex, display_key) ranked by MatchRank (Exact > WordBoundary > Substring).
    /// If `limit` is Some(n), returns at most n results after ranking.
    pub fn search_nodes(&self, query: &str) -> Vec<(NodeIndex, String)> {
        self.search_nodes_limit(query, None)
    }

    /// Search nodes with optional result limit.
    pub fn search_nodes_limit(
        &self,
        query: &str,
        limit: Option<usize>,
    ) -> Vec<(NodeIndex, String)> {
        use crate::graph::traverse::MatchRank;
        let lower = query.to_lowercase();
        let mut results: Vec<(NodeIndex, String, MatchRank)> = Vec::new();
        let mut seen: HashSet<NodeIndex> = HashSet::new();

        // Fast path: binary search for exact and prefix matches.
        // The name_index is sorted by lowercase key, so we can use
        // binary search for O(log n) lookup on exact/prefix queries.
        // Always followed by the full linear scan — fast path can miss
        // substring matches that don't start with the query prefix.
        if is_simple_query(&lower) {
            // Try exact match first
            let exact_pos = self
                .name_index
                .binary_search_by(|(k, _)| k.as_str().cmp(&lower));
            if let Ok(pos) = exact_pos {
                // Found exact match — collect all adjacent entries with
                // the same key (multiple nodes can share the same
                // lowercase key, e.g. same name in different schemas).
                let mut i = pos;
                while i > 0 && self.name_index[i - 1].0 == lower {
                    i -= 1;
                }
                while i < self.name_index.len() && self.name_index[i].0 == lower {
                    let idx = self.name_index[i].1;
                    seen.insert(idx);
                    let display =
                        crate::graph::key::NodeKey::from_node(&self.graph[idx]).to_string();
                    results.push((idx, display, MatchRank::Exact));
                    i += 1;
                }
            } else {
                // No exact match — try prefix match via partition_point.
                let start = self
                    .name_index
                    .partition_point(|(k, _)| k.as_str() < lower.as_str());
                let mut i = start;
                while i < self.name_index.len() && self.name_index[i].0.starts_with(lower.as_str())
                {
                    let idx = self.name_index[i].1;
                    if seen.insert(idx) {
                        let display =
                            crate::graph::key::NodeKey::from_node(&self.graph[idx]).to_string();
                        if let Some(rank) = MatchRank::classify(&lower, &self.name_index[i].0) {
                            results.push((idx, display, rank));
                        }
                    }
                    i += 1;
                }
            }
        }

        // Full linear scan: catches substring matches the fast path
        // missed (keys that contain the query but don't start with it).
        for (key_lower, idx) in &self.name_index {
            if seen.contains(idx) {
                continue;
            }
            if !key_lower.contains(&lower) {
                continue;
            }
            let display = crate::graph::key::NodeKey::from_node(&self.graph[*idx]).to_string();
            if let Some(rank) = MatchRank::classify(&lower, key_lower) {
                results.push((*idx, display, rank));
            }
        }

        // 2. JavaSql semantic key fallback
        //
        // When query is "javasql:ClassName.methodName", the name_index won't
        // match (it stores "javasql:/path/file:line" via NodeKey Display).
        // Search JavaSql nodes by their (class_name, method_name) derived
        // display key — the same format used by search_by_sql().
        //
        // This bridges the gap between search results (which show
        // "javasql:ClassName.method:line") and CLI commands like detail/trace
        // (which use search_nodes() for lookup).
        if let Some(stripped) = lower.strip_prefix("javasql:") {
            let semantic_query = stripped.trim();
            // Strip trailing :line for semantic matching, but keep it for
            // disambiguation when multiple nodes share the same class+method.
            let line_filter: Option<usize> = semantic_query
                .rsplit_once(':')
                .and_then(|(_, line_str)| line_str.parse::<usize>().ok());
            let class_method_query = match line_filter {
                Some(_) => semantic_query
                    .rsplit_once(':')
                    .map(|(prefix, _)| prefix)
                    .unwrap_or(semantic_query),
                None => semantic_query,
            };
            if !class_method_query.is_empty() {
                for idx in self.nodes_by_type("sql") {
                    if results.iter().any(|(i, _, _)| i == idx) {
                        continue;
                    }
                    let semantic_candidate: Option<String> = match &self.graph[*idx] {
                        Node::JavaSql {
                            class_name: Some(c),
                            method_name: Some(m),
                            ..
                        } => Some(format!("{}.{}", c, m).to_lowercase()),
                        Node::JavaSql {
                            class_name: Some(c),
                            method_name: None,
                            ..
                        } => Some(c.to_lowercase()),
                        Node::JavaSql {
                            class_name: None,
                            method_name: Some(m),
                            ..
                        } => Some(m.to_lowercase()),
                        _ => None,
                    };
                    if let Some(candidate) = semantic_candidate {
                        if let Some(rank) = MatchRank::classify(class_method_query, &candidate) {
                            let display = crate::graph::key::NodeKey::from_node(&self.graph[*idx])
                                .to_string();
                            results.push((*idx, display, rank));
                        }
                    }
                }
            }
        }

        // 3. JspPage display_name fallback
        //
        // When query is "jsp:legacy/customer-detail.jsp", the name_index won't
        // match (it stores "jsp:/absolute/path/to/file.jsp" via NodeKey Display).
        // Search JspPage nodes by their display_name.
        #[cfg(feature = "jsp")]
        if let Some(stripped) = lower.strip_prefix("jsp:") {
            let semantic_query = stripped.trim();
            if !semantic_query.is_empty() {
                for idx in self.nodes_by_type("jsp") {
                    if results.iter().any(|(i, _, _)| i == idx) {
                        continue;
                    }
                    if let Node::JspPage {
                        ref display_name, ..
                    } = self.graph[*idx]
                    {
                        let candidate = display_name.to_lowercase();
                        if let Some(rank) = MatchRank::classify(semantic_query, &candidate) {
                            let display = NodeKey::from_node(&self.graph[*idx]).to_string();
                            results.push((*idx, display, rank));
                        }
                    }
                }
            }
        }

        // 4. JspSql display_name fallback
        //
        // When query is "jspsql:jsp/legacy/page.jsp:44", the name_index won't
        // match (it stores "jspsql:/absolute/path:line:hash").
        // Search JspSql nodes by shortened file path + line.
        #[cfg(feature = "jsp")]
        if let Some(stripped) = lower.strip_prefix("jspsql:") {
            let semantic_query = stripped.trim();
            if !semantic_query.is_empty() {
                for idx in self.nodes_by_type("jspsql") {
                    if results.iter().any(|(i, _, _)| i == idx) {
                        continue;
                    }
                    if let Node::JspSql { ref file, line, .. } = self.graph[*idx] {
                        let short = crate::parser::jsp_preprocessor::compute_display_name(file);
                        let candidate = format!("{}:{}", short.to_lowercase(), line);
                        if let Some(rank) = MatchRank::classify(semantic_query, &candidate) {
                            let display = NodeKey::from_node(&self.graph[*idx]).to_string();
                            results.push((*idx, display, rank));
                        }
                    }
                }
            }
        }

        results.sort_by(|a, b| {
            a.2.cmp(&b.2)
                .then_with(|| {
                    let deg_a = self.graph.neighbors_undirected(a.0).count();
                    let deg_b = self.graph.neighbors_undirected(b.0).count();
                    deg_b.cmp(&deg_a)
                })
                .then_with(|| a.1.cmp(&b.1))
        });
        let iter = results.into_iter().map(|(idx, display, _)| (idx, display));
        match limit {
            Some(n) => iter.take(n).collect(),
            None => iter.collect(),
        }
    }

    /// Search nodes with explicit match mode.
    ///
    /// - `Exact`: binary search in name_index for exact lowercase key.
    /// - `Regex`: compile query as regex, linear scan matching keys.
    /// - `Substring`: delegates to `search_nodes()`.
    pub fn search_nodes_with_mode(
        &self,
        query: &str,
        mode: crate::graph::search::MatchMode,
    ) -> Vec<(NodeIndex, String)> {
        self.search_nodes_with_mode_limit(query, mode, None)
    }

    /// Search nodes with explicit match mode and optional result limit.
    pub fn search_nodes_with_mode_limit(
        &self,
        query: &str,
        mode: crate::graph::search::MatchMode,
        limit: Option<usize>,
    ) -> Vec<(NodeIndex, String)> {
        match mode {
            crate::graph::search::MatchMode::Substring => self.search_nodes_limit(query, limit),
            crate::graph::search::MatchMode::Exact => {
                let lower = query.to_lowercase();
                let exact_pos = self
                    .name_index
                    .binary_search_by(|(k, _)| k.as_str().cmp(&lower));
                match exact_pos {
                    Ok(pos) => {
                        let mut results = Vec::new();
                        let mut i = pos;
                        while i > 0 && self.name_index[i - 1].0 == lower {
                            i -= 1;
                        }
                        while i < self.name_index.len() && self.name_index[i].0 == lower {
                            let idx = self.name_index[i].1;
                            let display =
                                crate::graph::key::NodeKey::from_node(&self.graph[idx]).to_string();
                            results.push((idx, display));
                            i += 1;
                        }
                        results
                    }
                    Err(_) => Vec::new(),
                }
            }
            crate::graph::search::MatchMode::Regex => {
                let re = match regex::Regex::new(query) {
                    Ok(r) => r,
                    Err(_) => return Vec::new(),
                };
                let mut results: Vec<(NodeIndex, String)> = Vec::new();
                for (key_lower, idx) in &self.name_index {
                    if re.is_match(key_lower) {
                        let display =
                            crate::graph::key::NodeKey::from_node(&self.graph[*idx]).to_string();
                        results.push((*idx, display));
                    }
                }
                results
            }
        }
    }

    /// Resolve a single node name, handling ambiguity.
    ///
    /// Returns `Single` on unique match, `Multiple` when `all_matches` is true
    /// and multiple hits exist, or `Empty` when `fail_on_multiple` is true for
    /// ambiguous queries. Prints diagnostics to stderr when multiple matches
    /// are found.
    pub fn resolve_single_node(
        &self,
        name: &str,
        mode: crate::graph::search::MatchMode,
        all_matches: bool,
        fail_on_multiple: bool,
    ) -> crate::graph::search::ResolveResult {
        use crate::graph::search::ResolveResult;
        let matches = self.search_nodes_with_mode(name, mode);

        if matches.is_empty() {
            return ResolveResult::Empty;
        }

        if matches.len() == 1 {
            return ResolveResult::Single(matches[0].0, matches[0].1.clone());
        }

        eprintln!("Multiple matches found:");
        for (i, (_, display)) in matches.iter().enumerate() {
            eprintln!("  {}: {}", i + 1, display);
        }

        if all_matches {
            ResolveResult::Multiple(matches)
        } else if fail_on_multiple {
            eprintln!("Ambiguous match. Use --exact or refine the query.");
            ResolveResult::Ambiguous
        } else {
            eprintln!("Using first match: {}", matches[0].1);
            ResolveResult::Single(matches[0].0, matches[0].1.clone())
        }
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
        // Prepend the magic + version header so load_bincode can validate the file
        // and diagnose version mismatches BEFORE bincode deserialization (which
        // fails opaquely on cross-version layout drift). See issue #110.
        let mut bytes: Vec<u8> = Vec::with_capacity(13 + 1024);
        bytes.extend_from_slice(&STORE_MAGIC);
        bytes.extend_from_slice(&STORE_VERSION.to_le_bytes());
        bincode::serialize_into(&mut bytes, self).map_err(|e| {
            crate::error::CodeWebError::ExportError {
                message: format!("bincode serialize: {}", e),
            }
        })?;
        std::fs::write(path, bytes).map_err(|e| crate::error::CodeWebError::FileRead {
            path: path.to_path_buf(),
            source: e,
        })?;
        Self::save_manifest_sidecar(path, &self.manifest)?;
        Ok(())
    }

    fn save_manifest_sidecar(
        store_path: &Path,
        manifest: &HashMap<PathBuf, FileRecord>,
    ) -> crate::error::Result<()> {
        let manifest_path = Self::manifest_sidecar_path(store_path);
        let bytes =
            bincode::serialize(manifest).map_err(|e| crate::error::CodeWebError::ExportError {
                message: format!("manifest bincode serialize: {}", e),
            })?;
        std::fs::write(&manifest_path, bytes).map_err(|e| {
            crate::error::CodeWebError::FileRead {
                path: manifest_path,
                source: e,
            }
        })?;
        Ok(())
    }

    pub fn load_manifest_sidecar(
        store_path: &Path,
    ) -> crate::error::Result<HashMap<PathBuf, FileRecord>> {
        let manifest_path = Self::manifest_sidecar_path(store_path);
        if !manifest_path.exists() {
            return Ok(HashMap::new());
        }
        let bytes =
            std::fs::read(&manifest_path).map_err(|e| crate::error::CodeWebError::FileRead {
                path: manifest_path.clone(),
                source: e,
            })?;
        bincode::deserialize(&bytes).map_err(|e| crate::error::CodeWebError::ExportError {
            message: format!("manifest bincode deserialize: {}", e),
        })
    }

    fn manifest_sidecar_path(store_path: &Path) -> PathBuf {
        let stem = store_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "store".to_string());
        store_path.with_file_name(format!("{stem}.manifest"))
    }

    pub fn load_bincode(path: &Path) -> crate::error::Result<Self> {
        let bytes = std::fs::read(path).map_err(|e| crate::error::CodeWebError::FileRead {
            path: path.to_path_buf(),
            source: e,
        })?;

        // Probe for the magic + version header (post-issue-#110 files).
        // If present, validate version up front so a cross-version file yields an
        // actionable message instead of a cryptic bincode deserialize failure.
        // If absent, fall back to legacy whole-buffer deserialization (pre-header
        // stores from v0.8.x and earlier). See issue #110.
        let payload: &[u8] = if bytes.len() >= 13 && bytes[..9] == STORE_MAGIC {
            let stored_ver = u32::from_le_bytes([bytes[9], bytes[10], bytes[11], bytes[12]]);
            if stored_ver != STORE_VERSION {
                return Err(crate::error::CodeWebError::ExportError {
                    message: format!(
                        "unsupported cache version {}, expected {} — run `codeweb analyze` to regenerate",
                        stored_ver, STORE_VERSION
                    ),
                });
            }
            &bytes[13..]
        } else {
            // Legacy headerless file — best-effort whole-buffer deserialize.
            // The post-deserialize version check below still applies.
            &bytes[..]
        };

        let store_result: Result<Self, _> = bincode::deserialize(payload);
        let store: Self = match store_result {
            Ok(s) => s,
            Err(e) => {
                // If deserialization failed and the file looks like it has a header
                // (13+ bytes) but the magic didn't match, give a friendly error.
                if bytes.len() >= 13 && bytes[..9] != STORE_MAGIC {
                    return Err(crate::error::CodeWebError::ExportError {
                        message: format!(
                            "not a codeweb store (bad magic: expected CWEBSTORE, got {:?})",
                            &bytes[..9]
                        ),
                    });
                }
                return Err(crate::error::CodeWebError::ExportError {
                    message: format!("bincode deserialize: {} ({} bytes)", e, bytes.len()),
                });
            }
        };

        // Belt-and-suspenders: catches legacy files whose struct.version field was
        // set to a mismatched value, and corrupted/garbage files that happened to
        // deserialize to *something*.
        if store.version != STORE_VERSION {
            return Err(crate::error::CodeWebError::ExportError {
                message: format!(
                    "unsupported cache version {}, expected {} — run `codeweb analyze` to regenerate",
                    store.version, STORE_VERSION
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
        Self::save_manifest_sidecar(path, &self.manifest)?;
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
        if store.version != STORE_VERSION {
            return Err(crate::error::CodeWebError::ExportError {
                message: format!(
                    "unsupported cache version {}, expected {} — run `codeweb analyze` to regenerate",
                    store.version, STORE_VERSION
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
        self.lock_clause_index.clear();

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

        self.sql_fingerprint_index.clear();
        for idx in self.graph.node_indices() {
            match &self.graph[idx] {
                Node::MappedStatement {
                    sql: Some(sql_text),
                    namespace,
                    statement_id,
                    ..
                } => {
                    let fp = sql_match::sql_fingerprint(sql_text);
                    let display_key = format!("mapper:{}.{}", namespace, statement_id);
                    self.sql_fingerprint_index
                        .entry(fp)
                        .or_default()
                        .push((idx, display_key));
                }
                Node::JavaSql {
                    sql: Some(sql_text),
                    class_name,
                    method_name,
                    line,
                    ..
                } => {
                    let fp = sql_match::sql_fingerprint(sql_text);
                    let ctx = match (class_name, method_name) {
                        (Some(c), Some(m)) => format!("{}.{}", c, m),
                        (Some(c), None) => c.clone(),
                        (None, Some(m)) => m.clone(),
                        (None, None) => "?".to_string(),
                    };
                    let display_key = format!("javasql:{}:{}", ctx, line);
                    self.sql_fingerprint_index
                        .entry(fp)
                        .or_default()
                        .push((idx, display_key));
                }
                _ => {}
            }
        }

        self.lock_clause_index.clear();
        for idx in self.graph.node_indices() {
            let (display_key, sql_texts) = match &self.graph[idx] {
                Node::MappedStatement {
                    sql: Some(sql_text),
                    namespace,
                    statement_id,
                    ..
                } => (
                    format!("mapper:{}.{}", namespace, statement_id),
                    vec![sql_text.as_str()],
                ),
                Node::JavaSql {
                    sql: Some(sql_text),
                    class_name,
                    method_name,
                    line,
                    ..
                } => {
                    let ctx = match (class_name, method_name) {
                        (Some(c), Some(m)) => format!("{}.{}", c, m),
                        (Some(c), None) => c.clone(),
                        (None, Some(m)) => m.clone(),
                        (None, None) => "?".to_string(),
                    };
                    (format!("javasql:{}:{}", ctx, line), vec![sql_text.as_str()])
                }
                Node::Procedure { id, body_sql, .. } => (
                    format!("proc:{}", id),
                    body_sql.iter().map(|s| s.sql_text.as_str()).collect(),
                ),
                Node::Function { id, body_sql, .. } => (
                    format!("func:{}", id),
                    body_sql.iter().map(|s| s.sql_text.as_str()).collect(),
                ),
                _ => continue,
            };
            for sql in sql_texts {
                if let Some(kind) = sql_match::detect_lock_clause_in_sql(sql) {
                    self.lock_clause_index
                        .entry(kind.index_key().to_string())
                        .or_default()
                        .push((idx, display_key.clone()));
                }
            }
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

pub fn node_source_file(node: &Node) -> Option<PathBuf> {
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
        Node::BuiltinFunction { location, .. } => Some(location.file.to_path_buf()),
        Node::Custom { location, .. } => location.as_ref().map(|l| l.file.to_path_buf()),
        #[cfg(feature = "jsp")]
        Node::JspPage { path, .. } => Some(path.clone()),
        #[cfg(feature = "jsp")]
        Node::JspSql { file, .. } => Some(file.clone()),
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
        crate::graph::Edge::UsesBuiltinFunction { .. } => "uses_builtin_function",
        crate::graph::Edge::ContainsMethod => "contains_method",
        #[cfg(feature = "jsp")]
        crate::graph::Edge::ContainsSql => "contains_sql",
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
fn pick_richer_node(a: &Node, idx_a: NodeIndex, b: &Node, idx_b: NodeIndex) -> NodeIndex {
    match (a, b) {
        (Node::Procedure { partial: false, .. }, Node::Procedure { partial: true, .. }) => idx_a,
        (Node::Procedure { partial: true, .. }, Node::Procedure { partial: false, .. }) => idx_b,
        (Node::Function { partial: false, .. }, Node::Function { partial: true, .. }) => idx_a,
        (Node::Function { partial: true, .. }, Node::Function { partial: false, .. }) => idx_b,
        (
            Node::Table {
                location: Some(_), ..
            },
            Node::Table { location: None, .. },
        ) => idx_a,
        (
            Node::Table { location: None, .. },
            Node::Table {
                location: Some(_), ..
            },
        ) => idx_b,
        _ => idx_a,
    }
}

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
    pub builtin_functions: usize,
    pub custom_nodes: usize,
    pub custom_edges: usize,
    pub edges: usize,
    pub files: usize,
    #[cfg(feature = "jsp")]
    pub jsp_pages: usize,
    #[cfg(feature = "jsp")]
    pub jsp_sql: usize,
}

#[derive(Debug, Default, Serialize)]
pub struct DedupReport {
    pub nodes_before: usize,
    pub nodes_after: usize,
    pub nodes_removed: usize,
    pub edges_before: usize,
    pub edges_after: usize,
    pub edges_removed: usize,
    pub unresolved_resolved: usize,
}

impl GraphStore {
    /// Rebuild all indexes from scratch after structural graph changes.
    /// Covers: node_key_index, file_nodes, file_edges, secondary indexes, reverse deps.
    fn rebuild_all_indexes(&mut self) {
        self.node_key_index.clear();
        for idx in self.graph.node_indices() {
            let key = NodeKey::from_node(&self.graph[idx]);
            self.node_key_index.insert(key, idx);
        }

        self.file_nodes.clear();
        for idx in self.graph.node_indices() {
            let key = NodeKey::from_node(&self.graph[idx]);
            if let Some(file) = node_source_file(&self.graph[idx]) {
                self.file_nodes.entry(file).or_default().push(key);
            }
        }

        self.file_edges.clear();
        for edge_idx in self.graph.edge_indices() {
            let (src, dst) = self.graph.edge_endpoints(edge_idx).unwrap();
            let src_key = NodeKey::from_node(&self.graph[src]);
            let dst_key = NodeKey::from_node(&self.graph[dst]);
            if let Some(src_file) = node_source_file(&self.graph[src]) {
                self.file_edges
                    .entry(src_file.clone())
                    .or_default()
                    .push((src_key, dst_key));
            }
        }

        self.rebuild_secondary_indexes();
        self.rebuild_reverse_deps();
    }

    /// Deduplicate nodes by [`NodeKey`], rewire edges, deduplicate edges, and rebuild all indexes.
    ///
    /// # Phases
    ///
    /// 1. **Node dedup** — groups nodes by [`NodeKey`]. When duplicates exist, uses
    ///    [`pick_richer_node`] to decide which to keep, then rewires all edges from the
    ///    removed node to the canonical node.
    /// 2. **Edge dedup** — for each `(src, dst, edge_type)` group, keeps one edge.
    ///    [`Edge::TableAccess`] edges have their `modes` / `write_kinds` merged.
    /// 3. **(implicit)** — [`NodeKey::Unresolved`] dedup is already handled by Phase 1.
    /// 4. **Rebuild indexes** — all secondary indexes, file maps, and reverse deps are
    ///    reconstructed from scratch.
    pub fn dedup(&mut self) -> DedupReport {
        let nodes_before = self.graph.node_count();
        let edges_before = self.graph.edge_count();

        let nodes_removed;
        let edges_removed;
        let unresolved_resolved;

        {
            let all_indices: Vec<NodeIndex> = self.graph.node_indices().collect();

            let mut canonical: HashMap<NodeKey, NodeIndex> = HashMap::new();
            for &idx in &all_indices {
                let key = NodeKey::from_node(&self.graph[idx]);
                canonical.entry(key).or_insert(idx);
            }

            let mut to_remove: Vec<NodeIndex> = Vec::new();
            let mut removed_to_canonical: HashMap<NodeIndex, NodeIndex> = HashMap::new();
            let mut unresolved_count = 0usize;

            for &idx in &all_indices {
                let key = NodeKey::from_node(&self.graph[idx]);
                let c_idx = canonical[&key];
                if idx == c_idx {
                    continue;
                }

                let keep = pick_richer_node(&self.graph[c_idx], c_idx, &self.graph[idx], idx);

                if keep == idx {
                    canonical.insert(key, idx);
                    to_remove.push(c_idx);
                    removed_to_canonical.insert(c_idx, idx);
                    if matches!(&self.graph[c_idx], Node::Unresolved { .. }) {
                        unresolved_count += 1;
                    }
                } else {
                    to_remove.push(idx);
                    removed_to_canonical.insert(idx, c_idx);
                    if matches!(&self.graph[idx], Node::Unresolved { .. }) {
                        unresolved_count += 1;
                    }
                }
            }

            let mut final_mapping: HashMap<NodeIndex, NodeIndex> = HashMap::new();
            for &remove in &to_remove {
                let mut canon = remove;
                while let Some(&next) = removed_to_canonical.get(&canon) {
                    canon = next;
                }
                final_mapping.insert(remove, canon);
            }

            #[allow(clippy::type_complexity)]
            let mut rewires: Vec<(NodeIndex, NodeIndex, crate::graph::Edge)> = Vec::new();

            for (&remove, &canon) in &final_mapping {
                for edge_ref in self
                    .graph
                    .edges_directed(remove, petgraph::Direction::Incoming)
                {
                    let src = edge_ref.source();
                    let final_src = final_mapping.get(&src).copied().unwrap_or(src);
                    if final_src != canon {
                        rewires.push((final_src, canon, edge_ref.weight().clone()));
                    }
                }
                for edge_ref in self
                    .graph
                    .edges_directed(remove, petgraph::Direction::Outgoing)
                {
                    let dst = edge_ref.target();
                    let final_dst = final_mapping.get(&dst).copied().unwrap_or(dst);
                    if canon != final_dst {
                        rewires.push((canon, final_dst, edge_ref.weight().clone()));
                    }
                }
            }

            for (src, dst, weight) in &rewires {
                self.graph.add_edge(*src, *dst, weight.clone());
            }

            nodes_removed = to_remove.len();
            unresolved_resolved = unresolved_count;

            let mut sorted_remove: Vec<NodeIndex> = to_remove;
            sorted_remove.sort();
            sorted_remove.dedup();
            sorted_remove.sort_by(|a, b| b.cmp(a));
            for idx in sorted_remove {
                self.graph.remove_node(idx);
            }
        }

        {
            let all_edges: Vec<petgraph::graph::EdgeIndex> = self.graph.edge_indices().collect();
            let mut edge_groups: HashMap<
                (NodeIndex, NodeIndex, String),
                Vec<petgraph::graph::EdgeIndex>,
            > = HashMap::new();
            for &edge_idx in &all_edges {
                let (src, dst) = self.graph.edge_endpoints(edge_idx).unwrap();
                let tag = edge_type_tag(&self.graph[edge_idx]);
                edge_groups
                    .entry((src, dst, tag))
                    .or_default()
                    .push(edge_idx);
            }

            // Phase A: collect all edge data and TableAccess merge info
            // BEFORE any removal. petgraph remove_edge uses swap_remove,
            // so interleaving access with removal can panic when a cached
            // EdgeIndex equals the swapped-out last slot.
            let mut to_remove: Vec<petgraph::graph::EdgeIndex> = Vec::new();
            for ((_src, _dst, tag), mut group) in edge_groups {
                if group.len() <= 1 {
                    continue;
                }
                let keep = group.remove(0);
                if tag == "table_access" {
                    // Merge modes/write_kinds from all remove edges into keep.
                    let mut merged_modes: Option<crate::graph::AccessMode> = None;
                    let mut merged_kinds: Vec<crate::graph::WriteKind> = Vec::new();
                    for &remove_idx in &group {
                        if let crate::graph::Edge::TableAccess {
                            modes, write_kinds, ..
                        } = &self.graph[remove_idx]
                        {
                            merged_modes = Some(merged_modes.map_or(*modes, |m| m | *modes));
                            merged_kinds.extend(write_kinds.iter().copied());
                        }
                    }
                    if let Some(modes) = merged_modes {
                        if let crate::graph::Edge::TableAccess {
                            modes: keep_modes,
                            write_kinds: keep_kinds,
                            ..
                        } = &mut self.graph[keep]
                        {
                            *keep_modes |= modes;
                            keep_kinds.extend(merged_kinds);
                        }
                    }
                }
                to_remove.extend(group.iter().copied());
            }

            // Phase B: remove edges in descending index order.
            // Higher indices removed first → lower pending indices stay valid.
            to_remove.sort_unstable();
            to_remove.dedup();
            edges_removed = to_remove.len();
            for idx in to_remove.into_iter().rev() {
                self.graph.remove_edge(idx);
            }
        }

        self.rebuild_all_indexes();
        self.updated_at = timestamp_ms();

        DedupReport {
            nodes_before,
            nodes_after: self.graph.node_count(),
            nodes_removed,
            edges_before,
            edges_after: self.graph.edge_count(),
            edges_removed,
            unresolved_resolved,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn sql_text_matches(sql_text: &str, query_lower: &str) -> bool {
        let prepared = crate::sql_match::PreparedQuery::new(query_lower);
        prepared.matches(sql_text)
    }

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
            body_sql: Vec::new(),
        };
        let table = crate::graph::Node::Table {
            schema: Some("public".to_string()),
            name: "orders".to_string(),
            explicit: false,
            system: false,
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
            body_sql: Vec::new(),
        };
        let table = crate::graph::Node::Table {
            schema: Some("public".to_string()),
            name: "orders".to_string(),
            explicit: false,
            system: false,
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
            explicit: false,
            system: false,
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
    fn save_bincode_writes_magic_version_header() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("with_header.bincode");
        let store = GraphStore::new("probe-test");
        store.save_bincode(&path).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        assert!(
            bytes.len() >= 13,
            "file must have at least a 13-byte header"
        );
        assert_eq!(&bytes[..9], b"CWEBSTORE", "magic header must be CWEBSTORE");
        let stored_ver = u32::from_le_bytes(bytes[9..13].try_into().unwrap());
        assert_eq!(
            stored_ver, STORE_VERSION,
            "header version must match constant"
        );
    }

    #[test]
    fn load_bincode_round_trips_with_header() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("roundtrip.bincode");
        let store = GraphStore::from_graph("roundtrip", CodeGraph::new());
        store.save_bincode(&path).unwrap();
        let loaded = GraphStore::load_bincode(&path);
        assert!(
            loaded.is_ok(),
            "round-trip should succeed: {:?}",
            loaded.err()
        );
        assert_eq!(loaded.unwrap().version, STORE_VERSION);
    }

    #[test]
    fn load_bincode_falls_back_for_legacy_headerless_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("legacy.bincode");
        // Simulate a pre-header (legacy) file: raw bincode payload, no magic prefix.
        let store = GraphStore::from_graph("legacy", CodeGraph::new());
        let raw = bincode::serialize(&store).unwrap();
        std::fs::write(&path, &raw).unwrap();

        let loaded = GraphStore::load_bincode(&path);
        assert!(
            loaded.is_ok(),
            "legacy headerless file must still load: {:?}",
            loaded.err()
        );
        assert_eq!(loaded.unwrap().project_name, "legacy");
    }

    #[test]
    fn load_bincode_rejects_wrong_magic_with_friendly_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("not_a_store.bincode");
        // 13+ bytes starting with something other than the magic.
        let mut bytes = b"NOPESTORE".to_vec();
        bytes.extend_from_slice(&STORE_VERSION.to_le_bytes());
        bytes.extend_from_slice(b"garbage payload that is not bincode");
        std::fs::write(&path, &bytes).unwrap();

        let result = GraphStore::load_bincode(&path);
        assert!(result.is_err(), "wrong magic must error");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not a codeweb store") || err_msg.contains("bad magic"),
            "wrong-magic error should name the problem: {}",
            err_msg
        );
    }

    #[test]
    fn load_bincode_rejects_header_version_mismatch_with_friendly_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("wrong_ver.bincode");
        // Header with correct magic but a future version number, followed by junk payload.
        let mut bytes = b"CWEBSTORE".to_vec();
        let future_ver: u32 = 99;
        bytes.extend_from_slice(&future_ver.to_le_bytes());
        bytes.extend_from_slice(b"payload does not matter, version check fires first");
        std::fs::write(&path, &bytes).unwrap();

        let result = GraphStore::load_bincode(&path);
        assert!(result.is_err(), "version mismatch must error");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("unsupported cache version"),
            "version-mismatch error should mention version: {}",
            err_msg
        );
        assert!(
            err_msg.contains("99"),
            "error should report the found version (99): {}",
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
                body_sql: Vec::new(),
            });
        }
        for i in 0..2 {
            graph.add_node(crate::graph::Node::Table {
                schema: Some("public".to_string()),
                name: format!("table{}", i),
                explicit: false,
                system: false,
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
            body_sql: Vec::new(),
        });

        let store = GraphStore::from_graph("test", graph);
        assert_eq!(store.nodes_by_type("proc").len(), 3);
        assert_eq!(store.nodes_by_type("table*").len(), 2);
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
            body_sql: Vec::new(),
        });
        graph.add_node(crate::graph::Node::Table {
            schema: Some("public".to_string()),
            name: "apple".to_string(),
            explicit: false,
            system: false,
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
            body_sql: Vec::new(),
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
            body_sql: Vec::new(),
        });
        graph.add_node(crate::graph::Node::Table {
            schema: Some("schema_a".to_string()),
            name: "t1".to_string(),
            explicit: false,
            system: false,
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
            body_sql: Vec::new(),
        });
        graph.add_node(crate::graph::Node::View {
            schema: Some("schema_b".to_string()),
            name: "v1".to_string(),
            explicit: false,
            system: false,
            location: None,
            columns: Box::new(vec![]),
            ddl_source: None,
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
            body_sql: Vec::new(),
        });
        graph.add_node(crate::graph::Node::Table {
            schema: Some("public".to_string()),
            name: "orders".to_string(),
            explicit: false,
            system: false,
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
            body_sql: Vec::new(),
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
            body_sql: Vec::new(),
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
            body_sql: Vec::new(),
        });

        let store = GraphStore::from_graph("test", graph);
        let results = store.search_nodes("nonexistent");
        assert!(results.is_empty());
    }

    #[test]
    fn search_nodes_prefers_higher_degree_on_same_rank() {
        let mut graph = CodeGraph::new();
        let file = std::sync::Arc::new(std::path::PathBuf::from("a.sql"));
        let loc = crate::graph::SourceLocation {
            file: file.clone(),
            line: 1,
        };

        let func_a = graph.add_node(crate::graph::Node::Function {
            id: crate::graph::RoutineId {
                schema: Some("public".to_string()),
                package: None,
                name: "do_work".to_string(),
                kind: crate::graph::RoutineKind::Function,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });
        let proc_a = graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("public".to_string()),
                package: None,
                name: "do_work".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });
        let proc_b = graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("public".to_string()),
                package: None,
                name: "other".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });
        // Give proc_a a call edge → more connections → should rank higher
        graph.add_edge(
            proc_a,
            proc_b,
            crate::graph::Edge::DirectCall {
                scope: crate::graph::CallScope::CrossPackage,
                location: loc,
            },
        );

        let store = GraphStore::from_graph("test", graph);
        let results = store.search_nodes("do_work");

        assert_eq!(results.len(), 2, "both func and proc match");
        // proc with more connections should come first
        assert!(
            results[0].1.starts_with("proc:"),
            "higher-degree node (proc) should be first, got: {}",
            results[0].1
        );
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
            body_sql: Vec::new(),
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
            body_sql: Vec::new(),
        });
        let table = graph.add_node(crate::graph::Node::Table {
            schema: Some("public".to_string()),
            name: "orders".to_string(),
            explicit: false,
            system: false,
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
            body_sql: Vec::new(),
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
            body_sql: Vec::new(),
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
            body_sql: Vec::new(),
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
            body_sql: Vec::new(),
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
            body_sql: Vec::new(),
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
            body_sql: Vec::new(),
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
            body_sql: Vec::new(),
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
            body_sql: Vec::new(),
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
            body_sql: Vec::new(),
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
            explicit: false,
            system: false,
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
            body_sql: Vec::new(),
        });
        let table = graph_a.add_node(crate::graph::Node::Table {
            schema: Some("public".into()),
            name: "orders".into(),
            explicit: false,
            system: false,
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
            body_sql: Vec::new(),
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
        assert_eq!(table_summary.type_tag, "table*");
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

    // --- Statement type gate tests (E series) ---

    #[test]
    fn sql_text_matches_update_query_rejects_select_sql() {
        let sql = "SELECT ROW_NUMBER() OVER (ORDER BY __XML_RAW_sortColumnName__) AS DATA_ORDER, __XML_RAW_columnNameSql__ FROM __XML_RAW_tableName__ WHERE __XML_RAW_tradeDateName__ = __XML_PARAM_tradeDate__ and __XML_RAW_fundCodeName__ = __XML_PARAM_fundCode__";
        let query = "UPDATE DAT_MDB_TEXT t SET t.req_host_ip = ?, t.file_type = ?, t.interface_type = ? WHERE t.data_date = ? AND t.req_file_name = ?";
        assert!(
            !sql_text_matches(sql, query),
            "UPDATE query must not match SELECT SQL"
        );
    }

    #[test]
    fn sql_text_matches_update_query_rejects_select_sql_short() {
        let sql = "SELECT * FROM __XML_RAW_table__ WHERE __XML_RAW_cond__ = __XML_PARAM_val__";
        let query = "update orders set status = ? where id = ?";
        assert!(
            !sql_text_matches(sql, query),
            "UPDATE query must not match SELECT SQL even with short templates"
        );
    }

    #[test]
    fn sql_text_matches_update_query_rejects_select_dynamic() {
        let sql = "SELECT __XML_RAW_col__ FROM __XML_RAW_table__ WHERE __XML_RAW_cond__ = __XML_PARAM_val__";
        let query = "update ? set ?=? where ?=?";
        assert!(
            !sql_text_matches(sql, query),
            "UPDATE query must not match fully dynamic SELECT template"
        );
    }

    // --- Same table, different operation (F series) ---

    #[test]
    fn sql_text_matches_select_vs_delete_same_table() {
        let sql = "SELECT * FROM orders WHERE id = ?";
        let query = "delete from orders where id = ?";
        assert!(
            !sql_text_matches(sql, query),
            "DELETE query must not match SELECT SQL even on same table"
        );
    }

    #[test]
    fn sql_text_matches_insert_vs_update_same_table() {
        let sql = "INSERT INTO users (id, name) VALUES (?, ?)";
        let query = "update users set name = ? where id = ?";
        assert!(
            !sql_text_matches(sql, query),
            "UPDATE query must not match INSERT SQL on same table"
        );
    }

    #[test]
    fn sql_text_matches_select_vs_update_different_table() {
        let sql = "SELECT a, b FROM table_x WHERE c = ? AND d = ?";
        let query = "update table_y set e = ? where f = ? and g = ?";
        assert!(
            !sql_text_matches(sql, query),
            "UPDATE query must not match SELECT SQL on different tables"
        );
    }

    // --- K5 series: short segment patterns ---

    #[test]
    fn sql_text_matches_select_two_placeholders() {
        let sql = "SELECT ?, ?";
        let query = "select 1, 2";
        assert!(
            sql_text_matches(sql, query),
            "SELECT ?,? should match select 1, 2"
        );
    }

    #[test]
    fn sql_text_matches_select_three_placeholders() {
        let sql = "SELECT ?, ?, ?";
        let query = "select 1, 2, 3";
        assert!(
            sql_text_matches(sql, query),
            "SELECT ?,?,? should match select 1, 2, 3"
        );
    }

    #[test]
    fn sql_text_matches_values_placeholders() {
        let sql = "INSERT INTO t VALUES (?, ?, ?)";
        let query = "insert into t values (1, 2, 3)";
        assert!(
            sql_text_matches(sql, query),
            "VALUES(?,?,?) should match concrete values"
        );
    }

    #[test]
    fn sql_text_matches_single_placeholder_segment() {
        let sql = "SELECT * FROM ?";
        let query = "select * from orders";
        assert!(
            sql_text_matches(sql, query),
            "SELECT * FROM ? should match concrete table"
        );
    }

    // --- Correct UPDATE-to-UPDATE match (D series) ---

    #[test]
    fn sql_text_matches_update_template_to_concrete() {
        let sql = "UPDATE __XML_RAW_tableName__ t SET t.req_host_ip = __XML_PARAM_hostIp__, t.file_type = __XML_PARAM_fileType__ WHERE t.data_date = __XML_PARAM_dataDate__";
        let query =
            "UPDATE DAT_MDB_TEXT t SET t.req_host_ip = ?, t.file_type = ? WHERE t.data_date = ?";
        assert!(
            sql_text_matches(sql, query),
            "UPDATE template should match UPDATE query with same structure"
        );
    }

    #[test]
    fn sql_text_matches_update_template_partial() {
        let sql = "UPDATE __XML_RAW_tableName__ t SET t.req_host_ip = __XML_PARAM_hostIp__, t.file_type = __XML_PARAM_fileType__ WHERE t.data_date = __XML_PARAM_dataDate__";
        let query = "update dat_mdb_text t set t.req_host_ip = ?";
        assert!(
            sql_text_matches(sql, query),
            "partial UPDATE query should still match UPDATE template"
        );
    }

    // --- Extra conditions rejected (H series) ---

    #[test]
    fn sql_text_matches_query_extra_condition_rejected() {
        let sql = "SELECT * FROM __XML_RAW_tableName__ WHERE user_id = __XML_PARAM_userId__";
        let query = "select * from a where user_id=? and q=t";
        assert!(
            !sql_text_matches(sql, query),
            "query with extra conditions not in SQL must not match"
        );
    }

    // --- WITH CTE compatibility (K1) ---

    #[test]
    fn sql_text_matches_with_cte_select_body() {
        let sql = "WITH cte AS (SELECT 1) SELECT * FROM cte";
        let query = "select * from cte";
        assert!(
            sql_text_matches(sql, query),
            "WITH...SELECT should be compatible with SELECT query"
        );
    }

    #[test]
    fn sql_text_matches_with_cte_vs_update_rejected() {
        let sql = "WITH cte AS (SELECT 1) SELECT * FROM cte";
        let query = "update cte set x = 1";
        assert!(
            !sql_text_matches(sql, query),
            "WITH...SELECT must not match UPDATE query"
        );
    }

    // --- MERGE statement ---

    #[test]
    fn sql_text_matches_merge_to_merge() {
        let sql = "MERGE INTO target t USING source s ON (t.id = s.id) WHEN MATCHED THEN UPDATE SET t.val = s.val";
        let query = "merge into target t using source s on (t.id = s.id)";
        assert!(
            sql_text_matches(sql, query),
            "MERGE query should match MERGE SQL"
        );
    }

    // --- Completely unrelated SQL (J series) ---

    #[test]
    fn sql_text_matches_unrelated_sql_rejected() {
        let sql = "SELECT id, name FROM users WHERE status = 'ACTIVE'";
        let query = "update orders set total = 100 where order_id = 5";
        assert!(
            !sql_text_matches(sql, query),
            "completely unrelated SQL must not match"
        );
    }

    #[test]
    fn sql_text_matches_different_columns_rejected() {
        let sql = "SELECT user_id FROM orders WHERE user_id = ?";
        let query = "select user_name from orders where user_name = ?";
        assert!(
            !sql_text_matches(sql, query),
            "different column names must not match"
        );
    }

    // --- Table name gate tests ---

    #[test]
    fn sql_text_matches_update_different_concrete_table_rejected() {
        let sql = "UPDATE dat_ftp_text t SET t.req_status = __XML_PARAM_status__, t.req_file_content = __XML_PARAM_content__ WHERE t.seq_no = __XML_PARAM_seqNo__ AND t.data_date = __XML_PARAM_dataDate__ AND t.req_file_name = __XML_PARAM_fileName__";
        let query = "UPDATE DAT_MDB_TEXT t SET t.req_host_ip = ?, t.file_type = ? WHERE t.data_date = ? AND t.req_file_name = ?";
        assert!(
            !sql_text_matches(sql, query),
            "UPDATE on different concrete table must not match"
        );
    }

    #[test]
    fn sql_text_matches_select_different_concrete_view_rejected() {
        let sql = "SELECT * FROM V_ACCTBALBOOK WHERE fund_code = __XML_PARAM_fundCode__ AND accountdate = __XML_PARAM_date__";
        let query = "SELECT t.fund_code FROM V_JK_RCS_ACCTBALBOOK t WHERE accountdate = ?";
        assert!(
            !sql_text_matches(sql, query),
            "SELECT from different concrete view must not match"
        );
    }

    #[test]
    fn sql_text_matches_select_unrelated_table_rejected() {
        let sql = "SELECT DISTINCT (t.e2) e2, t.e3 FROM tmp_trd_pre_acctdata t WHERE fund_code = __XML_PARAM_fundCode__ AND accountdate = __XML_PARAM_date__";
        let query = "SELECT t.fund_code FROM V_JK_RCS_ACCTBALBOOK t WHERE accountdate = ?";
        assert!(
            !sql_text_matches(sql, query),
            "SELECT from completely unrelated table must not match"
        );
    }

    #[test]
    fn sql_text_matches_dynamic_table_template_accepts_concrete() {
        let sql = "UPDATE __XML_RAW_tableName__ t SET t.status = __XML_PARAM_status__ WHERE t.id = __XML_PARAM_id__";
        let query = "update orders t set t.status = ? where t.id = ?";
        assert!(
            sql_text_matches(sql, query),
            "dynamic table template (?) must accept any concrete table"
        );
    }

    #[test]
    fn sql_text_matches_dynamic_from_template_accepts_concrete() {
        let sql = "SELECT * FROM __XML_RAW_tableName__ WHERE id = __XML_PARAM_id__";
        let query = "select * from users where id = ?";
        assert!(
            sql_text_matches(sql, query),
            "dynamic FROM template (?) must accept any concrete table"
        );
    }

    #[test]
    fn sql_text_matches_same_concrete_table_different_first_set_col_rejected() {
        let sql = "UPDATE orders SET status = __XML_PARAM_status__ WHERE id = __XML_PARAM_id__";
        let query = "update orders set name = ? where id = ?";
        assert!(
            !sql_text_matches(sql, query),
            "different first SET column should not match"
        );
    }

    #[test]
    fn sql_text_matches_case2_full_scenario() {
        let sql = "SELECT ROW_NUMBER() OVER (ORDER BY __XML_RAW_sortColumnName__) AS DATA_ORDER, __XML_RAW_columnNameSql__ FROM __XML_RAW_tableName__ WHERE __XML_RAW_tradeDateName__ = __XML_PARAM_tradeDate__";
        let query = "SELECT t.FUND_CODE || ? || t.ACCOUNTDATE || ? FROM V_JK_RCS_ACCTBALBOOK t WHERE ACCOUNTDATE = ?";
        assert!(
            !sql_text_matches(sql, query),
            "fully dynamic SELECT template must not match specific SELECT on different view"
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
                body_sql: Vec::new(),
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

    // --- SQL search test helpers ---

    fn make_mapper_node(
        namespace: &str,
        statement_id: &str,
        sql: Option<&str>,
    ) -> crate::graph::Node {
        crate::graph::Node::MappedStatement {
            namespace: namespace.to_string(),
            statement_id: statement_id.to_string(),
            kind: "select".to_string(),
            xml_file: std::path::PathBuf::from("test.xml"),
            line: 1,
            sql: sql.map(String::from),
        }
    }

    fn make_javasql_node(
        class: Option<&str>,
        method: Option<&str>,
        sql: Option<&str>,
    ) -> crate::graph::Node {
        crate::graph::Node::JavaSql {
            class_name: class.map(String::from),
            method_name: method.map(String::from),
            extraction_method: "annotation".to_string(),
            java_file: std::path::PathBuf::from("Test.java"),
            line: 1,
            sql: sql.map(String::from),
        }
    }

    fn make_view_node(name: &str, schema: Option<&str>) -> crate::graph::Node {
        crate::graph::Node::View {
            schema: schema.map(String::from),
            name: name.to_string(),
            explicit: false,
            system: false,
            location: None,
            columns: Box::new(vec![]),
            ddl_source: None,
        }
    }

    // --- search_by_sql: exact and substring matching ---

    #[test]
    fn search_by_sql_finds_mapper_with_exact_sql() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "com.example.UserDao",
            "findById",
            Some("SELECT * FROM users WHERE id = ?"),
        ));
        graph.add_node(make_mapper_node(
            "com.example.OrderDao",
            "findAll",
            Some("SELECT * FROM orders"),
        ));
        let store = GraphStore::from_graph("test", graph);

        let results = store.search_by_sql("select * from users where id = ?");
        assert_eq!(results.len(), 1);
        assert!(results[0].1.contains("UserDao"));
        assert!(results[0].1.contains("findById"));
    }

    #[test]
    fn search_by_sql_finds_mapper_with_substring() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "com.example.UserDao",
            "findById",
            Some("SELECT id, name, email FROM users WHERE id = ? AND status = 'ACTIVE'"),
        ));
        let store = GraphStore::from_graph("test", graph);

        let results = store.search_by_sql("from users where id = ?");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_by_sql_finds_javasql_node() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_javasql_node(
            Some("UserRepository"),
            Some("findByName"),
            Some("SELECT * FROM users WHERE name = ?"),
        ));
        let store = GraphStore::from_graph("test", graph);

        let results = store.search_by_sql("from users where name");
        assert_eq!(results.len(), 1);
        assert!(results[0].1.contains("UserRepository"));
        assert!(results[0].1.contains("findByName"));
    }

    #[test]
    fn search_by_sql_returns_empty_for_no_match() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "com.example.UserDao",
            "findById",
            Some("SELECT * FROM users WHERE id = ?"),
        ));
        let store = GraphStore::from_graph("test", graph);

        let results = store.search_by_sql("delete from orders where id = ?");
        assert!(results.is_empty());
    }

    #[test]
    fn search_by_sql_skips_nodes_without_sql() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node("com.example.Dao", "noop", None));
        graph.add_node(make_mapper_node(
            "com.example.Dao",
            "selectOne",
            Some("SELECT 1 FROM dual"),
        ));
        let store = GraphStore::from_graph("test", graph);

        let results = store.search_by_sql("select 1");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_by_sql_case_insensitive() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "com.example.Dao",
            "find",
            Some("select * from users where id = ?"),
        ));
        let store = GraphStore::from_graph("test", graph);

        assert_eq!(store.search_by_sql("SELECT * FROM USERS").len(), 1);
        assert_eq!(store.search_by_sql("select * from users").len(), 1);
        assert_eq!(store.search_by_sql("Select * From Users").len(), 1);
    }

    #[test]
    fn search_by_sql_multiple_matches() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "com.example.UserDao",
            "findActive",
            Some("SELECT * FROM users WHERE status = 'ACTIVE'"),
        ));
        graph.add_node(make_mapper_node(
            "com.example.AdminDao",
            "findActiveAdmins",
            Some("SELECT * FROM users WHERE status = 'ACTIVE' AND role = 'ADMIN'"),
        ));
        graph.add_node(make_mapper_node(
            "com.example.OrderDao",
            "findAll",
            Some("SELECT * FROM orders"),
        ));
        let store = GraphStore::from_graph("test", graph);

        let results = store.search_by_sql("from users where status = 'active'");
        assert_eq!(results.len(), 2);
    }

    // --- search_by_sql: keyword gate ---

    #[test]
    fn search_by_sql_rejects_select_vs_update() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao",
            "selectUsers",
            Some("SELECT * FROM users WHERE id = ?"),
        ));
        let store = GraphStore::from_graph("test", graph);

        assert!(
            store
                .search_by_sql("update users set name = ? where id = ?")
                .is_empty(),
            "UPDATE query must not match SELECT SQL"
        );
    }

    #[test]
    fn search_by_sql_rejects_insert_vs_delete() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao",
            "insertOrder",
            Some("INSERT INTO orders (id, name) VALUES (?, ?)"),
        ));
        let store = GraphStore::from_graph("test", graph);

        assert!(
            store
                .search_by_sql("delete from orders where id = ?")
                .is_empty(),
            "DELETE query must not match INSERT SQL"
        );
    }

    #[test]
    fn search_by_sql_select_compatible_with_with() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao",
            "cteQuery",
            Some("WITH cte AS (SELECT 1) SELECT * FROM cte"),
        ));
        let store = GraphStore::from_graph("test", graph);

        assert_eq!(
            store.search_by_sql("select * from cte").len(),
            1,
            "SELECT query should match WITH...SELECT SQL"
        );
    }

    #[test]
    fn search_by_sql_merge_matches_merge() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao", "mergeData",
            Some("MERGE INTO target t USING src s ON (t.id = s.id) WHEN MATCHED THEN UPDATE SET t.val = s.val"),
        ));
        let store = GraphStore::from_graph("test", graph);

        assert_eq!(
            store
                .search_by_sql("merge into target t using src s on (t.id = s.id)")
                .len(),
            1,
            "MERGE query should match MERGE SQL"
        );
    }

    // --- search_by_sql: wildcard and XML placeholder ---

    #[test]
    fn search_by_sql_query_wildcard_matches_concrete_sql() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao",
            "findById",
            Some("SELECT * FROM users WHERE id = 123"),
        ));
        let store = GraphStore::from_graph("test", graph);

        assert_eq!(
            store
                .search_by_sql("select * from users where id = ?")
                .len(),
            1
        );
    }

    #[test]
    fn search_by_sql_xml_param_placeholder_matches_wildcard_query() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao", "find",
            Some("SELECT * FROM users WHERE id = __XML_PARAM_userId__ AND status = __XML_PARAM_status__"),
        ));
        let store = GraphStore::from_graph("test", graph);

        assert_eq!(
            store
                .search_by_sql("select * from users where id=? and status=?")
                .len(),
            1
        );
    }

    #[test]
    fn search_by_sql_xml_raw_placeholder_matches_concrete() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao",
            "dynamicUpdate",
            Some("UPDATE __XML_RAW_tableName__ t SET t.status = '1'"),
        ));
        let store = GraphStore::from_graph("test", graph);

        assert_eq!(
            store
                .search_by_sql("update orders t set t.status='1'")
                .len(),
            1,
            "concrete table name should match __XML_RAW__ placeholder"
        );
    }

    #[test]
    fn search_by_sql_fully_dynamic_sql_rejected() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao",
            "freeSql",
            Some("__XML_RAW_I_am_Free_SQL__"),
        ));
        let store = GraphStore::from_graph("test", graph);

        assert!(
            store
                .search_by_sql("select * from users where id = ?")
                .is_empty(),
            "fully dynamic SQL must not match specific queries"
        );
    }

    #[test]
    fn search_by_sql_query_extra_condition_with_two_wildcards_matches() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao",
            "find",
            Some("SELECT * FROM users WHERE id = __XML_PARAM_id__"),
        ));
        let store = GraphStore::from_graph("test", graph);

        // Current behavior: query "id=? and name=?" splits into segments
        // ["id=", " and name="] — both appear in the normalized SQL,
        // so this matches. This is a known over-matching case that P0
        // token normalization should fix.
        let results = store.search_by_sql("select * from users where id=? and name=?");
        assert_eq!(
            results.len(),
            1,
            "current behavior: multi-wildcard query matches even with extra conditions"
        );
    }

    #[test]
    fn search_by_sql_operator_spacing_normalized() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao",
            "find",
            Some("SELECT * FROM t_orders WHERE user_id = __XML_PARAM_id__ AND status = 'CREATED'"),
        ));
        let store = GraphStore::from_graph("test", graph);

        assert_eq!(
            store
                .search_by_sql("select * from t_orders where user_id=?")
                .len(),
            1,
            "different spacing around = should still match"
        );
    }

    // --- search_by_sql: table name gate ---

    #[test]
    fn search_by_sql_different_concrete_table_rejected() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao",
            "updateA",
            Some("UPDATE table_a SET x = 1 WHERE id = ?"),
        ));
        let store = GraphStore::from_graph("test", graph);

        assert!(
            store
                .search_by_sql("update table_b set x = 1 where id = ?")
                .is_empty(),
            "UPDATE on different concrete table must not match"
        );
    }

    #[test]
    fn search_by_sql_dynamic_table_accepts_any_concrete() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao", "dynamicUpdate",
            Some("UPDATE __XML_RAW_tableName__ SET status = __XML_PARAM_s__ WHERE id = __XML_PARAM_id__"),
        ));
        let store = GraphStore::from_graph("test", graph);

        assert_eq!(
            store
                .search_by_sql("update orders set status = ? where id = ?")
                .len(),
            1,
            "dynamic table template must accept any concrete table"
        );
    }

    #[test]
    fn search_by_sql_select_different_table_rejected() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao",
            "selectA",
            Some("SELECT * FROM table_a WHERE x = ?"),
        ));
        let store = GraphStore::from_graph("test", graph);

        assert!(
            store
                .search_by_sql("select * from table_b where x = ?")
                .is_empty(),
            "SELECT from different table must not match"
        );
    }

    #[test]
    fn search_by_sql_different_set_column_rejected() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao",
            "updateStatus",
            Some("UPDATE orders SET status = __XML_PARAM_s__ WHERE id = __XML_PARAM_id__"),
        ));
        let store = GraphStore::from_graph("test", graph);

        assert!(
            store
                .search_by_sql("update orders set name = ? where id = ?")
                .is_empty(),
            "different first SET column must not match"
        );
    }

    // --- search_by_sql: normalization edge cases ---

    #[test]
    fn search_by_sql_multiline_normalized() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao", "delete",
            Some("DELETE FROM bigfund.dat_log\n        WHERE data_date < TO_CHAR(TRUNC(SYSDATE) - 15, 'YYYYMMDD')"),
        ));
        let store = GraphStore::from_graph("test", graph);

        assert_eq!(
            store
                .search_by_sql("delete from bigfund.dat_log where data_date")
                .len(),
            1
        );
    }

    #[test]
    fn search_by_sql_crlf_normalized() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao",
            "delete",
            Some("DELETE FROM table\r\nWHERE id = 1"),
        ));
        let store = GraphStore::from_graph("test", graph);

        assert_eq!(
            store.search_by_sql("delete from table where id = 1").len(),
            1
        );
    }

    #[test]
    fn search_by_sql_paren_and_comma_spacing() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao",
            "select",
            Some("SELECT TO_CHAR( TRUNC(SYSDATE) - 15 , 'YYYYMMDD' ) FROM dual"),
        ));
        let store = GraphStore::from_graph("test", graph);

        assert_eq!(
            store
                .search_by_sql("select to_char(trunc(sysdate)-?,'yyyymmdd') from dual")
                .len(),
            1
        );
    }

    #[test]
    fn search_by_sql_xml_raw_with_type_hint() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao",
            "select",
            Some("SELECT __XML_RAW_STRING_column__ FROM users"),
        ));
        let store = GraphStore::from_graph("test", graph);

        assert_eq!(store.search_by_sql("select ? from users").len(), 1);
    }

    // --- search_by_sql: "for update" clause ---

    #[test]
    fn search_by_sql_for_update_matches_mapper() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "com.example.OrderDao",
            "lockById",
            Some("SELECT * FROM orders WHERE id = ? FOR UPDATE"),
        ));
        let store = GraphStore::from_graph("test", graph);
        let results = store.search_by_sql("for update");
        assert_eq!(results.len(), 1);
        assert!(results[0].1.contains("lockById"));
    }

    #[test]
    fn search_by_sql_for_update_matches_javasql() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_javasql_node(
            Some("OrderRepository"),
            Some("lockById"),
            Some("SELECT * FROM orders WHERE id = ? FOR UPDATE"),
        ));
        let store = GraphStore::from_graph("test", graph);
        let results = store.search_by_sql("for update");
        assert_eq!(results.len(), 1);
        assert!(results[0].1.contains("lockById"));
    }

    #[test]
    fn search_by_sql_for_update_matches_procedure_body_sql() {
        let mut graph = CodeGraph::new();
        graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: None,
                package: None,
                name: "lock_order".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: crate::graph::SourceLocation {
                file: Arc::new(PathBuf::from("test.sql")),
                line: 1,
            },
            partial: false,
            body_sql: vec![crate::graph::ProcedureBodySql {
                sql_text: "SELECT * FROM t_orders WHERE id = p_id FOR UPDATE".to_string(),
                kind: "SELECT".to_string(),
                line: Some(5),
            }],
        });
        let store = GraphStore::from_graph("test", graph);
        let results = store.search_by_sql("for update");
        assert_eq!(results.len(), 1);
        assert!(results[0].1.contains("lock_order"));
    }

    #[test]
    fn search_by_sql_for_update_case_insensitive() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao",
            "lock",
            Some("SELECT * FROM t FOR UPDATE"),
        ));
        let store = GraphStore::from_graph("test", graph);
        assert_eq!(store.search_by_sql("for update").len(), 1);
        assert_eq!(store.search_by_sql("FOR UPDATE").len(), 1);
        assert_eq!(store.search_by_sql("For Update").len(), 1);
    }

    #[test]
    fn search_by_sql_for_update_extra_whitespace_in_sql() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao",
            "lock",
            Some("SELECT * FROM t WHERE id = ? FOR   UPDATE"),
        ));
        let store = GraphStore::from_graph("test", graph);
        assert_eq!(store.search_by_sql("for update").len(), 1);
    }

    #[test]
    fn search_by_sql_for_update_not_matched_without_clause() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao",
            "selectAll",
            Some("SELECT id, name FROM orders WHERE status = 'ACTIVE'"),
        ));
        let store = GraphStore::from_graph("test", graph);
        assert!(
            store.search_by_sql("for update").is_empty(),
            "SQL without FOR UPDATE clause must not match"
        );
    }

    #[test]
    fn search_by_sql_for_update_in_string_literal_ignored() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao",
            "selectByStatus",
            Some("SELECT * FROM orders WHERE status = 'for update'"),
        ));
        let store = GraphStore::from_graph("test", graph);
        assert!(
            store.search_by_sql("for update").is_empty(),
            "'for update' inside a string literal must be normalized away"
        );
    }

    #[test]
    fn search_by_sql_for_update_in_comment_ignored() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao",
            "selectAll",
            Some("-- for update\nSELECT id, name FROM orders"),
        ));
        let store = GraphStore::from_graph("test", graph);
        assert!(
            store.search_by_sql("for update").is_empty(),
            "'for update' inside a line comment must be stripped before matching"
        );
    }

    #[test]
    fn search_by_sql_for_update_matches_nowait_variant() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao",
            "lockNowait",
            Some("SELECT * FROM orders WHERE id = ? FOR UPDATE NOWAIT"),
        ));
        let store = GraphStore::from_graph("test", graph);
        let results = store.search_by_sql("for update");
        assert_eq!(results.len(), 1);
        assert!(results[0].1.contains("lockNowait"));
    }

    #[test]
    fn search_by_sql_for_update_nowait_full_phrase() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao",
            "lockNowait",
            Some("SELECT * FROM orders WHERE id = ? FOR UPDATE NOWAIT"),
        ));
        let store = GraphStore::from_graph("test", graph);
        assert_eq!(store.search_by_sql("for update nowait").len(), 1);
    }

    #[test]
    fn search_by_sql_for_update_skip_then_wait_clause() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao",
            "lockSkip",
            Some("SELECT * FROM orders WHERE id = ? FOR UPDATE SKIP LOCKED"),
        ));
        let store = GraphStore::from_graph("test", graph);
        let results = store.search_by_sql("for update");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_by_sql_for_update_not_confused_by_update_dml() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao",
            "updateStatus",
            Some("UPDATE orders SET status = ? WHERE id = ?"),
        ));
        let store = GraphStore::from_graph("test", graph);
        assert!(
            store.search_by_sql("for update").is_empty(),
            "UPDATE DML without FOR UPDATE clause must not match"
        );
    }

    // --- lock clause index (build-time ogsql-parser AST) ---

    #[test]
    fn lock_clause_index_contains_mapper_with_for_update() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao",
            "lockById",
            Some("SELECT * FROM orders WHERE id = ? FOR UPDATE"),
        ));
        let store = GraphStore::from_graph("test", graph);
        let index = store.lock_clause_index();
        assert!(index.contains_key("for_update"));
        assert_eq!(index["for_update"].len(), 1);
        assert!(index["for_update"][0].1.contains("lockById"));
    }

    #[test]
    fn lock_clause_index_for_share_separate_from_for_update() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao",
            "lockShare",
            Some("SELECT * FROM orders WHERE id = ? FOR SHARE"),
        ));
        let store = GraphStore::from_graph("test", graph);
        let index = store.lock_clause_index();
        assert!(index.contains_key("for_share"));
        assert!(!index.contains_key("for_update"));
    }

    #[test]
    fn lock_clause_index_empty_for_regular_select() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao",
            "findAll",
            Some("SELECT id, name FROM orders WHERE status = 'ACTIVE'"),
        ));
        let store = GraphStore::from_graph("test", graph);
        assert!(store.lock_clause_index().is_empty());
    }

    #[test]
    fn search_by_sql_for_update_uses_lock_clause_fast_path() {
        // Fast path returns score 0.9 (not 1.0 from fingerprint exact match).
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao",
            "lock",
            Some("SELECT * FROM t FOR UPDATE"),
        ));
        let store = GraphStore::from_graph("test", graph);
        let results = store.search_by_sql("for update");
        assert_eq!(results.len(), 1);
        assert!((results[0].2 - 0.9).abs() < 0.001);
    }

    #[test]
    fn lock_clause_index_roundtrip_through_bincode() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao",
            "lock",
            Some("SELECT * FROM t FOR UPDATE"),
        ));
        let store = GraphStore::from_graph("test", graph);
        assert_eq!(store.lock_clause_index()["for_update"].len(), 1);

        let tmp = tempfile::NamedTempFile::new().unwrap();
        store.save_bincode(tmp.path()).unwrap();
        let loaded = GraphStore::load_bincode(tmp.path()).unwrap();
        assert_eq!(loaded.lock_clause_index()["for_update"].len(), 1);
    }

    // =======================================================================
    // RED tests (search-sql-v2) — expected future behavior
    // Activate with: cargo test --features search-sql-v2
    // =======================================================================

    // ── Dedup tests ───────────────────────────────────────────────────────────

    #[test]
    fn dedup_removes_duplicate_procedures() {
        let mut graph = CodeGraph::new();
        let loc = crate::graph::SourceLocation {
            file: std::sync::Arc::new(std::path::PathBuf::from("a.sql")),
            line: 1,
        };

        // Two procedures with same NodeKey.
        let _p1 = graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("public".to_string()),
                package: None,
                name: "do_work".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });
        let _p2 = graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("public".to_string()),
                package: None,
                name: "do_work".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });

        // A third node that calls p2 (the duplicate).
        let caller = graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("public".to_string()),
                package: None,
                name: "caller".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });

        // Add edge from caller to p2 (index 1, the duplicate).
        graph.add_edge(
            caller,
            graph.node_indices().nth(1).unwrap(),
            crate::graph::Edge::DirectCall {
                scope: crate::graph::CallScope::IntraPackage,
                location: loc.clone(),
            },
        );

        let mut store = GraphStore::from_graph("test", graph);
        assert_eq!(store.graph().node_count(), 3);

        let report = store.dedup();

        assert_eq!(
            report.nodes_removed, 1,
            "one duplicate procedure should be removed"
        );
        assert_eq!(
            store.graph().node_count(),
            2,
            "should have 2 nodes after dedup (caller + surviving proc)"
        );

        // The caller should still have an edge to the surviving procedure.
        let proc_indices: Vec<_> = store
            .graph()
            .node_indices()
            .filter(|i| {
                matches!(&store.graph()[*i], crate::graph::Node::Procedure { .. }) && {
                    let key = crate::graph::key::NodeKey::from_node(&store.graph()[*i]);
                    key.to_string().contains("do_work")
                }
            })
            .collect();
        assert_eq!(proc_indices.len(), 1, "only one do_work procedure survives");

        let caller_indices: Vec<_> = store
            .graph()
            .node_indices()
            .filter(|i| {
                matches!(&store.graph()[*i], crate::graph::Node::Procedure { .. }) && {
                    let key = crate::graph::key::NodeKey::from_node(&store.graph()[*i]);
                    key.to_string().contains("caller")
                }
            })
            .collect();
        assert_eq!(caller_indices.len(), 1, "caller should survive");

        let has_edge = store.graph().edge_indices().any(|e| {
            let (src, dst) = store.graph().edge_endpoints(e).unwrap();
            src == caller_indices[0] && dst == proc_indices[0]
        });
        assert!(
            has_edge,
            "caller should have edge rewired to surviving procedure"
        );
    }

    #[test]
    fn dedup_prefers_non_partial() {
        let mut graph = CodeGraph::new();
        let loc = crate::graph::SourceLocation {
            file: std::sync::Arc::new(std::path::PathBuf::from("a.sql")),
            line: 1,
        };

        // Partial procedure.
        let _partial = graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: None,
                package: None,
                name: "calc".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: true,
            body_sql: Vec::new(),
        });
        // Non-partial procedure with same key.
        let _full = graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: None,
                package: None,
                name: "calc".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });

        let mut store = GraphStore::from_graph("test", graph);
        let report = store.dedup();

        assert_eq!(report.nodes_removed, 1);
        assert_eq!(store.graph().node_count(), 1);

        // The surviving node should be non-partial.
        let surviving = &store.graph()[NodeIndex::new(0)];
        match surviving {
            crate::graph::Node::Procedure { partial, .. } => {
                assert!(!partial, "non-partial procedure should survive");
            }
            _ => panic!("expected Procedure node"),
        }
    }

    #[test]
    fn dedup_merges_table_nodes() {
        let mut graph = CodeGraph::new();

        // Implicit table (no location).
        graph.add_node(crate::graph::Node::Table {
            schema: Some("public".to_string()),
            name: "orders".to_string(),
            explicit: false,
            system: false,
            location: None,
            columns: Box::new(vec![]),
            partition_by: None,
            distribute_by: None,
            tablespace: None,
            temporary: false,
            unlogged: false,
            ddl_source: None,
        });
        // DDL table (with location).
        graph.add_node(crate::graph::Node::Table {
            schema: Some("public".to_string()),
            name: "orders".to_string(),
            explicit: true,
            system: false,
            location: Some(crate::graph::SourceLocation {
                file: std::sync::Arc::new(std::path::PathBuf::from("ddl.sql")),
                line: 10,
            }),
            columns: Box::new(vec![]),
            partition_by: None,
            distribute_by: None,
            tablespace: None,
            temporary: false,
            unlogged: false,
            ddl_source: None,
        });

        let mut store = GraphStore::from_graph("test", graph);
        let report = store.dedup();

        assert_eq!(report.nodes_removed, 1);
        assert_eq!(store.graph().node_count(), 1);

        // The surviving node should have a location (DDL table wins).
        let surviving = &store.graph()[NodeIndex::new(0)];
        match surviving {
            crate::graph::Node::Table { location, .. } => {
                assert!(location.is_some(), "DDL table with location should survive");
            }
            _ => panic!("expected Table node"),
        }
    }

    #[test]
    fn dedup_deduplicates_edges() {
        let mut graph = CodeGraph::new();
        let loc = crate::graph::SourceLocation {
            file: std::sync::Arc::new(std::path::PathBuf::from("a.sql")),
            line: 1,
        };

        let pa = graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: None,
                package: None,
                name: "a".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });
        let pb = graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: None,
                package: None,
                name: "b".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });

        // Two identical DirectCall edges.
        graph.add_edge(
            pa,
            pb,
            crate::graph::Edge::DirectCall {
                scope: crate::graph::CallScope::IntraPackage,
                location: loc.clone(),
            },
        );
        graph.add_edge(
            pa,
            pb,
            crate::graph::Edge::DirectCall {
                scope: crate::graph::CallScope::IntraPackage,
                location: loc.clone(),
            },
        );

        let mut store = GraphStore::from_graph("test", graph);
        assert_eq!(store.graph().edge_count(), 2);

        let report = store.dedup();

        assert_eq!(report.edges_removed, 1);
        assert_eq!(store.graph().edge_count(), 1);
    }

    #[test]
    fn dedup_clean_graph_noop() {
        let mut graph = CodeGraph::new();
        let loc = crate::graph::SourceLocation {
            file: std::sync::Arc::new(std::path::PathBuf::from("a.sql")),
            line: 1,
        };

        graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: None,
                package: None,
                name: "a".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });
        graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: None,
                package: None,
                name: "b".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });

        let mut store = GraphStore::from_graph("test", graph);
        let nodes_before = store.graph().node_count();
        let edges_before = store.graph().edge_count();

        let report = store.dedup();

        assert_eq!(report.nodes_removed, 0);
        assert_eq!(report.edges_removed, 0);
        assert_eq!(report.nodes_after, nodes_before);
        assert_eq!(report.edges_after, edges_before);
    }

    #[test]
    fn dedup_report_counts() {
        let mut graph = CodeGraph::new();
        let loc = crate::graph::SourceLocation {
            file: std::sync::Arc::new(std::path::PathBuf::from("a.sql")),
            line: 1,
        };

        // Three procs: two identical (dup), one unique.
        graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: None,
                package: None,
                name: "dup".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });
        graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: None,
                package: None,
                name: "dup".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });
        graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: None,
                package: None,
                name: "unique".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });

        let mut store = GraphStore::from_graph("test", graph);
        let report = store.dedup();

        assert_eq!(report.nodes_before, 3);
        assert_eq!(report.nodes_after, 2);
        assert_eq!(report.nodes_removed, 1);
        assert_eq!(report.unresolved_resolved, 0);
    }

    #[test]
    fn dedup_unresolved_nodes_deduped_by_key() {
        let mut graph = CodeGraph::new();
        let loc = crate::graph::SourceLocation {
            file: std::sync::Arc::new(std::path::PathBuf::from("a.sql")),
            line: 1,
        };

        // Two Procs that call the same Unresolved target.
        let p1 = graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: None,
                package: None,
                name: "p1".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });
        let p2 = graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: None,
                package: None,
                name: "p2".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });

        // Two identical Unresolved nodes (same raw_expr + context).
        let u1 = graph.add_node(crate::graph::Node::Unresolved {
            raw_expr: Box::new("some_func".to_string()),
            context: Box::new("test.sql".to_string()),
        });
        let u2 = graph.add_node(crate::graph::Node::Unresolved {
            raw_expr: Box::new("some_func".to_string()),
            context: Box::new("test.sql".to_string()),
        });

        graph.add_edge(
            p1,
            u1,
            crate::graph::Edge::DynamicCall {
                raw_expr: "some_func".to_string(),
                location: loc.clone(),
            },
        );
        graph.add_edge(
            p2,
            u2,
            crate::graph::Edge::DynamicCall {
                raw_expr: "some_func".to_string(),
                location: loc.clone(),
            },
        );

        let mut store = GraphStore::from_graph("test", graph);
        assert_eq!(store.graph().node_count(), 4);

        let report = store.dedup();

        // One Unresolved node should be removed.
        assert_eq!(report.nodes_removed, 1);
        assert_eq!(report.unresolved_resolved, 1);
        assert_eq!(store.graph().node_count(), 3);

        // Both procs should now have edges to the single surviving Unresolved node.
        let unresolved_indices: Vec<_> = store
            .graph()
            .node_indices()
            .filter(|i| matches!(&store.graph()[*i], crate::graph::Node::Unresolved { .. }))
            .collect();
        assert_eq!(unresolved_indices.len(), 1);

        let surviving_u = unresolved_indices[0];
        let p1_has_edge = store.graph().edge_indices().any(|e| {
            let (src, dst) = store.graph().edge_endpoints(e).unwrap();
            src == p1 && dst == surviving_u
        });
        let p2_has_edge = store.graph().edge_indices().any(|e| {
            let (src, dst) = store.graph().edge_endpoints(e).unwrap();
            src == p2 && dst == surviving_u
        });
        assert!(
            p1_has_edge,
            "p1 should have edge to surviving Unresolved node"
        );
        assert!(
            p2_has_edge,
            "p2 should have edge to surviving Unresolved node"
        );
    }

    // ═══════════════════════════════════════════════════════════════
    // Regression: search_nodes correctness (issue #116)
    // These must pass identically before AND after binary search opt.
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn search_nodes_exact_match_returns_correct_node() {
        let mut graph = CodeGraph::new();
        let file = Arc::new(PathBuf::from("a.sql"));
        let loc = crate::graph::SourceLocation {
            file: file.clone(),
            line: 1,
        };

        let pkg_idx = graph.add_node(crate::graph::Node::Package {
            schema: Some("public".to_string()),
            name: "pkg_a".to_string(),
            location: loc.clone(),
        });
        let proc1_idx = graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("public".to_string()),
                package: Some("pkg_a".to_string()),
                name: "proc_alpha".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });
        let _proc2_idx = graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("public".to_string()),
                package: Some("pkg_a".to_string()),
                name: "proc_alpha_beta".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });
        graph.add_edge(pkg_idx, proc1_idx, crate::graph::Edge::ContainsRoutine);
        graph.add_edge(pkg_idx, _proc2_idx, crate::graph::Edge::ContainsRoutine);

        let store = GraphStore::from_graph("test", graph);

        // Exact match on "proc_alpha" should return BOTH proc_alpha and
        // proc_alpha_beta (substring), with exact match ranked first.
        let results = store.search_nodes("proc_alpha");
        assert_eq!(
            results.len(),
            2,
            "should match both proc_alpha and proc_alpha_beta"
        );
        // Exact match (proc_alpha) comes before substring match (proc_alpha_beta)
        assert!(
            results[0].1.contains("proc_alpha"),
            "exact match should be first, got: {}",
            results[0].1
        );
    }

    #[test]
    fn search_nodes_case_insensitive() {
        let mut graph = CodeGraph::new();
        let file = Arc::new(PathBuf::from("a.sql"));
        let loc = crate::graph::SourceLocation { file, line: 1 };

        graph.add_node(crate::graph::Node::Function {
            id: crate::graph::RoutineId {
                schema: Some("public".to_string()),
                package: None,
                name: "func_gamma".to_string(),
                kind: crate::graph::RoutineKind::Function,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });

        let store = GraphStore::from_graph("test", graph);

        // Case-insensitive search should still find the node
        let results = store.search_nodes("FUNC_GAMMA");
        assert_eq!(results.len(), 1);
        assert!(results[0].1.contains("func_gamma"));

        let results = store.search_nodes("Gamma");
        assert_eq!(results.len(), 1);
        assert!(results[0].1.contains("func_gamma"));
    }

    #[test]
    fn search_nodes_package_substring_matches_package_procedures() {
        let mut graph = CodeGraph::new();
        let file = Arc::new(PathBuf::from("a.sql"));
        let loc = crate::graph::SourceLocation { file, line: 1 };

        graph.add_node(crate::graph::Node::Package {
            schema: Some("public".to_string()),
            name: "pkg_b".to_string(),
            location: loc.clone(),
        });
        graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("public".to_string()),
                package: Some("pkg_b".to_string()),
                name: "proc_one".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });

        let store = GraphStore::from_graph("test", graph);

        // Searching for "pkg_b" should match package and its procedure
        let results = store.search_nodes("pkg_b");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn search_nodes_returns_empty_for_no_match_regress() {
        let mut graph = CodeGraph::new();
        let file = Arc::new(PathBuf::from("a.sql"));
        let loc = crate::graph::SourceLocation { file, line: 1 };

        graph.add_node(crate::graph::Node::Table {
            schema: Some("public".to_string()),
            name: "orders".to_string(),
            explicit: false,
            system: false,
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
        let results = store.search_nodes("nonexistent_table");
        assert!(results.is_empty());
    }

    #[test]
    fn search_nodes_multi_word_match_returns_all() {
        // Regression: ensure that when multiple unrelated nodes match
        // the same substring, ALL are returned (not just first).
        let mut graph = CodeGraph::new();
        let file = Arc::new(PathBuf::from("a.sql"));
        let loc = crate::graph::SourceLocation { file, line: 1 };

        for name in &["handler_init", "handler_process", "handler_cleanup"] {
            graph.add_node(crate::graph::Node::Procedure {
                id: crate::graph::RoutineId {
                    schema: Some("public".to_string()),
                    package: None,
                    name: name.to_string(),
                    kind: crate::graph::RoutineKind::Procedure,
                },
                location: loc.clone(),
                partial: false,
                body_sql: Vec::new(),
            });
        }

        let store = GraphStore::from_graph("test", graph);
        let results = store.search_nodes("handler");
        assert_eq!(results.len(), 3, "all 3 handler_* nodes should match");
    }

    #[test]
    fn search_nodes_returns_prefix_and_substring_matches_together() {
        let mut graph = CodeGraph::new();
        let file = Arc::new(PathBuf::from("a.sql"));
        let loc = crate::graph::SourceLocation { file, line: 1 };

        graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("public".to_string()),
                package: None,
                name: "proc_a".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });
        graph.add_node(crate::graph::Node::Table {
            schema: Some("public".to_string()),
            name: "proc_log".to_string(),
            explicit: false,
            system: false,
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

        let results = store.search_nodes("proc");
        assert_eq!(
            results.len(),
            2,
            "both proc:public.proc_a (prefix match) and table:public.proc_log (substring \
             match) must be returned; fast path must not suppress substring hits"
        );
    }

    #[test]
    fn search_nodes_with_mode_exact_only_returns_exact_key_match() {
        let mut graph = CodeGraph::new();
        let file = Arc::new(PathBuf::from("a.sql"));
        let loc = crate::graph::SourceLocation { file, line: 1 };

        graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("public".to_string()),
                package: None,
                name: "create_order".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });
        graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("public".to_string()),
                package: None,
                name: "create_order_v2".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });

        let store = GraphStore::from_graph("test", graph);

        let results = store.search_nodes_with_mode(
            "proc:public.create_order",
            crate::graph::search::MatchMode::Exact,
        );
        assert_eq!(
            results.len(),
            1,
            "exact mode should only return exact key match"
        );
        assert!(results[0].1.ends_with("create_order"));
    }

    #[test]
    fn resolve_single_node_returns_single_on_unique_match() {
        let mut graph = CodeGraph::new();
        let file = Arc::new(PathBuf::from("a.sql"));
        let loc = crate::graph::SourceLocation { file, line: 1 };

        graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("public".to_string()),
                package: None,
                name: "unique_proc".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });

        let store = GraphStore::from_graph("test", graph);
        let result = store.resolve_single_node(
            "unique_proc",
            crate::graph::search::MatchMode::Substring,
            false,
            false,
        );

        match result {
            crate::graph::search::ResolveResult::Single(_idx, name) => {
                assert!(name.contains("unique_proc"));
            }
            other => panic!("expected Single, got {:?}", other),
        }
    }

    #[test]
    fn resolve_single_node_returns_empty_on_no_match() {
        let store = GraphStore::from_graph("test", CodeGraph::new());
        let result = store.resolve_single_node(
            "nonexistent",
            crate::graph::search::MatchMode::Substring,
            false,
            false,
        );
        assert!(
            matches!(result, crate::graph::search::ResolveResult::Empty),
            "expected Empty, got {:?}",
            result
        );
    }

    #[test]
    fn resolve_single_node_fail_on_multiple_returns_ambiguous() {
        let mut graph = CodeGraph::new();
        let file = Arc::new(PathBuf::from("a.sql"));
        let loc = crate::graph::SourceLocation { file, line: 1 };

        for name in &["do_work", "do_work_v2"] {
            graph.add_node(crate::graph::Node::Procedure {
                id: crate::graph::RoutineId {
                    schema: Some("public".to_string()),
                    package: None,
                    name: name.to_string(),
                    kind: crate::graph::RoutineKind::Procedure,
                },
                location: loc.clone(),
                partial: false,
                body_sql: Vec::new(),
            });
        }

        let store = GraphStore::from_graph("test", graph);
        let result = store.resolve_single_node(
            "do_work",
            crate::graph::search::MatchMode::Substring,
            false,
            true,
        );
        assert!(
            matches!(result, crate::graph::search::ResolveResult::Ambiguous),
            "fail_on_multiple should return Ambiguous when ambiguous, got {:?}",
            result
        );
    }

    #[test]
    fn resolve_single_node_all_matches_returns_multiple() {
        let mut graph = CodeGraph::new();
        let file = Arc::new(PathBuf::from("a.sql"));
        let loc = crate::graph::SourceLocation { file, line: 1 };

        for name in &["handler_a", "handler_b", "handler_c"] {
            graph.add_node(crate::graph::Node::Procedure {
                id: crate::graph::RoutineId {
                    schema: Some("public".to_string()),
                    package: None,
                    name: name.to_string(),
                    kind: crate::graph::RoutineKind::Procedure,
                },
                location: loc.clone(),
                partial: false,
                body_sql: Vec::new(),
            });
        }

        let store = GraphStore::from_graph("test", graph);
        let result = store.resolve_single_node(
            "handler",
            crate::graph::search::MatchMode::Substring,
            true,
            false,
        );

        match result {
            crate::graph::search::ResolveResult::Multiple(results) => {
                assert_eq!(results.len(), 3);
            }
            other => panic!("expected Multiple, got {:?}", other),
        }
    }

    #[test]
    fn summarize_tables_aggregates_child_proc_table_access() {
        use std::collections::{BTreeMap, HashSet};

        let file = Arc::new(PathBuf::from("test.sql"));
        let loc = crate::graph::SourceLocation {
            file: file.clone(),
            line: 1,
        };

        let mut graph = CodeGraph::new();
        let pkg = graph.add_node(crate::graph::Node::Package {
            schema: Some("public".to_string()),
            name: "pkg_test".to_string(),
            location: loc.clone(),
        });

        let proc1 = graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("public".to_string()),
                package: Some("pkg_test".to_string()),
                name: "create_order".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });
        let proc2 = graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("public".to_string()),
                package: Some("pkg_test".to_string()),
                name: "update_order".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });
        let proc3 = graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("public".to_string()),
                package: Some("pkg_test".to_string()),
                name: "read_customer".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });

        graph.add_edge(pkg, proc1, crate::graph::Edge::ContainsRoutine);
        graph.add_edge(pkg, proc2, crate::graph::Edge::ContainsRoutine);
        graph.add_edge(pkg, proc3, crate::graph::Edge::ContainsRoutine);

        let orders = graph.add_node(crate::graph::Node::Table {
            schema: Some("public".to_string()),
            name: "orders".to_string(),
            explicit: true,
            system: false,
            location: None,
            columns: Box::new(vec![]),
            partition_by: None,
            distribute_by: None,
            tablespace: None,
            temporary: false,
            unlogged: false,
            ddl_source: None,
        });
        let customers = graph.add_node(crate::graph::Node::Table {
            schema: Some("public".to_string()),
            name: "customers".to_string(),
            explicit: true,
            system: false,
            location: None,
            columns: Box::new(vec![]),
            partition_by: None,
            distribute_by: None,
            tablespace: None,
            temporary: false,
            unlogged: false,
            ddl_source: None,
        });
        let audit_log = graph.add_node(crate::graph::Node::Table {
            schema: Some("public".to_string()),
            name: "audit_log".to_string(),
            explicit: true,
            system: false,
            location: None,
            columns: Box::new(vec![]),
            partition_by: None,
            distribute_by: None,
            tablespace: None,
            temporary: false,
            unlogged: false,
            ddl_source: None,
        });

        let mut wk = HashSet::new();
        wk.insert(crate::graph::WriteKind::Insert);
        graph.add_edge(
            proc1,
            orders,
            crate::graph::Edge::TableAccess {
                flow_kind: crate::graph::DataFlowKind::DmlAccess,
                modes: crate::graph::AccessMode::Write,
                write_kinds: wk.clone(),
                location: loc.clone(),
                column_analysis: None,
            },
        );
        graph.add_edge(
            proc1,
            audit_log,
            crate::graph::Edge::TableAccess {
                flow_kind: crate::graph::DataFlowKind::DmlAccess,
                modes: crate::graph::AccessMode::Write,
                write_kinds: wk,
                location: loc.clone(),
                column_analysis: None,
            },
        );

        let mut wk2 = HashSet::new();
        wk2.insert(crate::graph::WriteKind::Update);
        graph.add_edge(
            proc2,
            orders,
            crate::graph::Edge::TableAccess {
                flow_kind: crate::graph::DataFlowKind::DmlAccess,
                modes: crate::graph::AccessMode::Write,
                write_kinds: wk2.clone(),
                location: loc.clone(),
                column_analysis: None,
            },
        );
        graph.add_edge(
            proc2,
            audit_log,
            crate::graph::Edge::TableAccess {
                flow_kind: crate::graph::DataFlowKind::DmlAccess,
                modes: crate::graph::AccessMode::Write,
                write_kinds: wk2,
                location: loc.clone(),
                column_analysis: None,
            },
        );

        graph.add_edge(
            proc3,
            customers,
            crate::graph::Edge::TableAccess {
                flow_kind: crate::graph::DataFlowKind::DmlAccess,
                modes: crate::graph::AccessMode::Read,
                write_kinds: HashSet::new(),
                location: loc.clone(),
                column_analysis: None,
            },
        );

        let store = GraphStore::from_graph("test", graph);
        let graph = store.graph();

        let mut summary: BTreeMap<
            String,
            (crate::graph::AccessMode, HashSet<crate::graph::WriteKind>),
        > = BTreeMap::new();
        for edge_ref in graph.edges_directed(pkg, petgraph::Direction::Outgoing) {
            if !matches!(edge_ref.weight(), crate::graph::Edge::ContainsRoutine) {
                continue;
            }
            let child = edge_ref.target();
            for ta_ref in graph.edges_directed(child, petgraph::Direction::Outgoing) {
                if let crate::graph::Edge::TableAccess {
                    flow_kind,
                    modes,
                    write_kinds,
                    ..
                } = ta_ref.weight()
                {
                    if *flow_kind != crate::graph::DataFlowKind::DmlAccess {
                        continue;
                    }
                    let dst = ta_ref.target();
                    if let crate::graph::Node::Table { name, .. } = &graph[dst] {
                        let entry = summary
                            .entry(name.clone())
                            .or_insert_with(|| (crate::graph::AccessMode::empty(), HashSet::new()));
                        entry.0 |= *modes;
                        for wk in write_kinds {
                            entry.1.insert(*wk);
                        }
                    }
                }
            }
        }

        assert_eq!(summary.len(), 3);
        // orders: Insert from proc1 + Update from proc2
        let o = summary.get("orders").unwrap();
        assert!(o.0.contains(crate::graph::AccessMode::Write));
        assert!(
            o.1.contains(&crate::graph::WriteKind::Insert)
                && o.1.contains(&crate::graph::WriteKind::Update)
        );
        // customers: Read only
        let c = summary.get("customers").unwrap();
        assert!(c.0.contains(crate::graph::AccessMode::Read));
        assert!(!c.0.contains(crate::graph::AccessMode::Write));
        // audit_log: Insert from two procs
        let a = summary.get("audit_log").unwrap();
        assert!(a.0.contains(crate::graph::AccessMode::Write));
    }

    #[test]
    fn subgraph_filter_by_name_reduces_node_count() {
        let file = Arc::new(PathBuf::from("test.sql"));
        let loc = crate::graph::SourceLocation { file, line: 1 };

        let mut graph = CodeGraph::new();
        let proc_a = graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("public".to_string()),
                package: None,
                name: "target_proc".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });
        let proc_b = graph.add_node(crate::graph::Node::Procedure {
            id: crate::graph::RoutineId {
                schema: Some("public".to_string()),
                package: None,
                name: "other_proc".to_string(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc.clone(),
            partial: false,
            body_sql: Vec::new(),
        });
        let table = graph.add_node(crate::graph::Node::Table {
            schema: Some("public".to_string()),
            name: "orders".to_string(),
            explicit: true,
            system: false,
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
            table,
            crate::graph::Edge::TableAccess {
                flow_kind: crate::graph::DataFlowKind::DmlAccess,
                modes: crate::graph::AccessMode::Write,
                write_kinds: HashSet::new(),
                location: loc.clone(),
                column_analysis: None,
            },
        );
        graph.add_edge(
            proc_b,
            proc_a,
            crate::graph::Edge::DirectCall {
                scope: crate::graph::CallScope::CrossPackage,
                location: loc,
            },
        );

        let store = GraphStore::from_graph("test", graph);

        // Filter by "target_proc" → should include target_proc, orders
        // (direct neighbor), and proc_b (calls target_proc).
        let matches = store.search_nodes("target_proc");
        assert_eq!(matches.len(), 1);

        let mut selected: HashSet<NodeIndex> = HashSet::new();
        let graph = store.graph();
        for (idx, _) in &matches {
            selected.insert(*idx);
            for n in graph.neighbors_directed(*idx, petgraph::Direction::Outgoing) {
                selected.insert(n);
            }
            for n in graph.neighbors_directed(*idx, petgraph::Direction::Incoming) {
                selected.insert(n);
            }
        }
        // target_proc + orders (outgoing) + proc_b (incoming via DirectCall)
        assert_eq!(selected.len(), 3);

        // Edges between selected nodes: TableAccess + DirectCall
        let edge_count: usize = graph
            .edge_indices()
            .filter(|e| {
                let (s, d) = graph.edge_endpoints(*e).unwrap();
                selected.contains(&s) && selected.contains(&d)
            })
            .count();
        assert_eq!(edge_count, 2);
    }

    #[test]
    fn search_nodes_limit_respects_limit() {
        let file = Arc::new(PathBuf::from("a.sql"));
        let loc = crate::graph::SourceLocation { file, line: 1 };
        let mut graph = CodeGraph::new();
        for i in 0..10 {
            graph.add_node(crate::graph::Node::Procedure {
                id: crate::graph::RoutineId {
                    schema: Some("public".to_string()),
                    package: None,
                    name: format!("proc_{}", i),
                    kind: crate::graph::RoutineKind::Procedure,
                },
                location: loc.clone(),
                partial: false,
                body_sql: Vec::new(),
            });
        }
        let store = GraphStore::from_graph("test", graph);

        let all = store.search_nodes("proc_");
        assert_eq!(all.len(), 10, "should find all 10");

        let limited = store.search_nodes_limit("proc_", Some(3));
        assert_eq!(limited.len(), 3, "limit 3 should return 3 results");
    }

    #[cfg(feature = "search-sql-v2")]
    mod search_sql_v2 {
        use super::*;
        use std::path::PathBuf;
        use std::sync::Arc;

        // --- P0: Token normalization ---

        #[test]
        fn sql_comments_stripped_before_matching() {
            assert!(
                sql_text_matches(
                    "SELECT * FROM users -- get all users\nWHERE id = 1",
                    "select * from users where id = ?",
                ),
                "SQL line comments should be stripped before matching"
            );
        }

        #[test]
        fn sql_block_comments_stripped() {
            assert!(
                sql_text_matches(
                    "SELECT /* comment */ * FROM users WHERE id = 1",
                    "select * from users where id = ?",
                ),
                "Block comments should be stripped before matching"
            );
        }

        #[test]
        fn string_literals_unified_to_wildcard() {
            assert!(
                sql_text_matches(
                    "SELECT * FROM users WHERE status = 'ACTIVE'",
                    "select * from users where status = ?",
                ),
                "String literals should be normalized to ? for matching"
            );
        }

        #[test]
        fn number_literals_unified_to_wildcard() {
            assert!(
                sql_text_matches(
                    "SELECT * FROM users WHERE age > 18",
                    "select * from users where age > ?",
                ),
                "Number literals should be normalized to ? for matching"
            );
        }

        #[test]
        fn trailing_semicolon_ignored() {
            assert!(
                sql_text_matches("SELECT * FROM users;", "select * from users"),
                "Trailing semicolons should not prevent matching"
            );
        }

        #[test]
        fn where_one_equals_one_removed() {
            assert!(
                sql_text_matches(
                    "SELECT * FROM users WHERE 1=1 AND id = ?",
                    "select * from users where id = ?",
                ),
                "WHERE 1=1 pattern should be stripped for matching"
            );
        }

        // --- P0: Scoring and ranking (search_by_sql_scored) ---

        #[test]
        #[ignore = "placeholder for future search_by_sql_scored API"]
        fn search_by_sql_returns_scored_results() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_mapper_node(
                "dao",
                "exactMatch",
                Some("SELECT * FROM users WHERE id = ?"),
            ));
            graph.add_node(make_mapper_node(
                "dao",
                "partialMatch",
                Some("SELECT * FROM users WHERE id = ? AND name = ?"),
            ));
            graph.add_node(make_mapper_node(
                "dao",
                "differentTable",
                Some("SELECT * FROM orders WHERE id = ?"),
            ));
            let store = GraphStore::from_graph("test", graph);

            // search_by_sql_scored should return scored results
            // Placeholder: use existing search_by_sql until scored version exists
            let results = store.search_by_sql("select * from users where id = ?");
            assert_eq!(results.len(), 2, "should match 2 SQL nodes (users table)");

            // When search_by_sql_scored is implemented:
            // exact match should score higher than partial
            // exact.score should be >= 0.8
        }

        #[test]
        fn search_by_sql_exact_match_is_first_result() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_mapper_node(
                "dao",
                "q",
                Some("SELECT id, name FROM users WHERE status = 'ACTIVE'"),
            ));
            let store = GraphStore::from_graph("test", graph);

            let results = store.search_by_sql("select id, name from users where status = 'active'");
            assert_eq!(results.len(), 1);
            // When scored: score should be >= 0.95, match_method should be "exact"
        }

        #[test]
        #[ignore = "placeholder for future search_by_sql_scored API"]
        fn search_by_sql_results_sorted_by_relevance() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_mapper_node(
                "dao",
                "exact",
                Some("SELECT * FROM users WHERE id = ?"),
            ));
            graph.add_node(make_mapper_node(
                "dao",
                "similar",
                Some("SELECT * FROM users WHERE id = ? AND status = ?"),
            ));
            graph.add_node(make_mapper_node(
                "dao",
                "vague",
                Some("SELECT id FROM users"),
            ));
            let store = GraphStore::from_graph("test", graph);

            let results = store.search_by_sql("select * from users where id = ?");
            assert!(results.len() >= 2);
            // When scored: results should be sorted by score descending
        }

        // --- P1: Jaccard fallback ---

        #[test]
        fn similar_sql_matched_by_token_similarity() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_mapper_node(
                "dao",
                "findUsers",
                Some("SELECT name, email, id FROM users WHERE status = 'ACTIVE'"),
            ));
            let store = GraphStore::from_graph("test", graph);

            // Currently fails: column order differs, substring match misses
            let results =
                store.search_by_sql("select id, name, email from users where status = 'active'");
            assert_eq!(
                results.len(),
                1,
                "column-reordered SQL should match via token similarity"
            );
        }

        #[test]
        fn slightly_different_sql_matched() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_mapper_node(
                "dao",
                "find",
                Some("SELECT id, name FROM users WHERE status = 'ACTIVE' AND dept = 'IT'"),
            ));
            let store = GraphStore::from_graph("test", graph);

            // Currently passes via substring match already
            let results = store.search_by_sql("select id, name from users where status = 'active'");
            assert_eq!(results.len(), 1, "SQL with extra condition should match");
        }

        #[test]
        fn dissimilar_sql_not_matched() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_mapper_node(
                "dao",
                "insert",
                Some("INSERT INTO orders (id, total) VALUES (?, ?)"),
            ));
            let store = GraphStore::from_graph("test", graph);

            let results = store.search_by_sql("select * from users where id = ?");
            assert!(results.is_empty(), "dissimilar SQL should not match");
        }

        // --- P1: Cross-type search ---

        #[test]
        fn search_by_sql_does_not_search_views() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_view_node("v_active_users", Some("public")));
            let store = GraphStore::from_graph("test", graph);

            let results = store.search_by_sql("from users where status");
            assert!(
                results.is_empty(),
                "search_by_sql currently does not search View nodes"
            );
        }

        #[test]
        fn search_by_sql_covers_mapper_and_javasql() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_mapper_node("dao", "find", Some("SELECT * FROM users")));
            graph.add_node(make_javasql_node(
                Some("Svc"),
                Some("query"),
                Some("SELECT * FROM orders"),
            ));
            let store = GraphStore::from_graph("test", graph);

            assert_eq!(store.search_by_sql("select * from users").len(), 1);
            assert_eq!(store.search_by_sql("select * from orders").len(), 1);
        }

        #[test]
        fn search_by_sql_finds_procedure_body_sql() {
            let mut graph = CodeGraph::new();
            graph.add_node(crate::graph::Node::Procedure {
                id: crate::graph::RoutineId {
                    schema: None,
                    package: None,
                    name: "get_users".to_string(),
                    kind: crate::graph::RoutineKind::Procedure,
                },
                location: crate::graph::SourceLocation {
                    file: Arc::new(PathBuf::from("test.sql")),
                    line: 1,
                },
                partial: false,
                body_sql: vec![crate::graph::ProcedureBodySql {
                    sql_text: "SELECT * FROM t_users WHERE status = 'ACTIVE'".to_string(),
                    kind: "SELECT".to_string(),
                    line: Some(3),
                }],
            });
            let store = GraphStore::from_graph("test", graph);
            let results = store.search_by_sql("select * from t_users where status");
            assert_eq!(results.len(), 1);
            assert!(results[0].1.contains("get_users"));
        }

        #[test]
        fn search_by_sql_finds_function_body_sql() {
            let mut graph = CodeGraph::new();
            graph.add_node(crate::graph::Node::Function {
                id: crate::graph::RoutineId {
                    schema: None,
                    package: None,
                    name: "count_orders".to_string(),
                    kind: crate::graph::RoutineKind::Function,
                },
                location: crate::graph::SourceLocation {
                    file: Arc::new(PathBuf::from("test.sql")),
                    line: 1,
                },
                partial: false,
                body_sql: vec![crate::graph::ProcedureBodySql {
                    sql_text: "SELECT COUNT(*) FROM t_orders".to_string(),
                    kind: "SELECT".to_string(),
                    line: Some(3),
                }],
            });
            let store = GraphStore::from_graph("test", graph);
            let results = store.search_by_sql("select count(*) from t_orders");
            assert_eq!(results.len(), 1);
            assert!(results[0].1.contains("count_orders"));
        }

        #[test]
        fn search_by_sql_proc_multiple_sqls_match_one() {
            let mut graph = CodeGraph::new();
            graph.add_node(crate::graph::Node::Procedure {
                id: crate::graph::RoutineId {
                    schema: None,
                    package: None,
                    name: "process".to_string(),
                    kind: crate::graph::RoutineKind::Procedure,
                },
                location: crate::graph::SourceLocation {
                    file: Arc::new(PathBuf::from("test.sql")),
                    line: 1,
                },
                partial: false,
                body_sql: vec![
                    crate::graph::ProcedureBodySql {
                        sql_text: "UPDATE t_orders SET status = 'DONE'".to_string(),
                        kind: "UPDATE".to_string(),
                        line: Some(3),
                    },
                    crate::graph::ProcedureBodySql {
                        sql_text: "INSERT INTO t_log(msg) VALUES('done')".to_string(),
                        kind: "INSERT".to_string(),
                        line: Some(4),
                    },
                ],
            });
            let store = GraphStore::from_graph("test", graph);
            let results = store.search_by_sql("insert into t_log");
            assert_eq!(results.len(), 1);
        }

        #[test]
        fn search_by_sql_proc_rejects_unrelated() {
            let mut graph = CodeGraph::new();
            graph.add_node(crate::graph::Node::Procedure {
                id: crate::graph::RoutineId {
                    schema: None,
                    package: None,
                    name: "reader".to_string(),
                    kind: crate::graph::RoutineKind::Procedure,
                },
                location: crate::graph::SourceLocation {
                    file: Arc::new(PathBuf::from("test.sql")),
                    line: 1,
                },
                partial: false,
                body_sql: vec![crate::graph::ProcedureBodySql {
                    sql_text: "SELECT * FROM t_users".to_string(),
                    kind: "SELECT".to_string(),
                    line: Some(3),
                }],
            });
            let store = GraphStore::from_graph("test", graph);
            let results = store.search_by_sql("insert into t_orders");
            assert!(results.is_empty());
        }

        // --- SQL fingerprint index ---

        #[test]
        fn fingerprint_index_built_for_mapper_sql() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_mapper_node(
                "dao",
                "findById",
                Some("SELECT * FROM users WHERE id = __XML_PARAM_id__"),
            ));
            let store = GraphStore::from_graph("test", graph);

            // The fingerprint index should have at least one entry
            assert!(!store.sql_fingerprint_index().is_empty());
        }

        #[test]
        fn fingerprint_index_built_for_javasql() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_javasql_node(
                Some("Svc"),
                Some("query"),
                Some("SELECT * FROM orders WHERE status = 'ACTIVE'"),
            ));
            let store = GraphStore::from_graph("test", graph);

            assert!(!store.sql_fingerprint_index().is_empty());
        }

        #[test]
        fn fingerprint_fast_path_hits_exact_match() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_mapper_node(
                "dao",
                "findById",
                Some("SELECT * FROM users WHERE id = __XML_PARAM_id__"),
            ));
            let store = GraphStore::from_graph("test", graph);

            // Searching by SQL should find it via fingerprint fast-path
            let results = store.search_by_sql("SELECT * FROM users WHERE id = ?");
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].1, "mapper:dao.findById");
        }

        #[test]
        fn fingerprint_miss_falls_back_to_matching() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_mapper_node(
                "dao",
                "findById",
                Some("SELECT * FROM users WHERE id = __XML_PARAM_id__ AND status = 'ACTIVE'"),
            ));
            let store = GraphStore::from_graph("test", graph);

            // Partial query won't match fingerprint, but should match via fallback
            let results = store.search_by_sql("from users where id");
            assert_eq!(results.len(), 1);
        }

        #[test]
        fn fingerprint_index_empty_for_graph_without_sql() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_mapper_node("dao", "findById", None));
            let store = GraphStore::from_graph("test", graph);

            assert!(store.sql_fingerprint_index().is_empty());
        }

        #[test]
        fn enrich_fingerprint_index_adds_variant_sqls() {
            let mut graph = CodeGraph::new();
            let idx = graph.add_node(make_mapper_node(
                "dao",
                "dynamicQuery",
                Some("SELECT * FROM users WHERE 1=1"),
            ));
            let mut store = GraphStore::from_graph("test", graph);

            // Before enrichment, only the flat_sql fingerprint exists
            let before_count = store.sql_fingerprint_index().len();

            // Enrich with a variant SQL
            let mut variant_map = std::collections::HashMap::new();
            variant_map.insert(
                "dao.dynamicQuery".to_string(),
                vec![
                    "SELECT * FROM users WHERE 1=1 AND status = ?".to_string(),
                    "SELECT * FROM users WHERE 1=1 AND name = ?".to_string(),
                ],
            );
            store.enrich_fingerprint_index_with_variants(&variant_map);

            // After enrichment, more fingerprints should exist
            let after_count = store.sql_fingerprint_index().len();
            assert!(
                after_count > before_count,
                "expected more fingerprints after enrichment, got {} before, {} after",
                before_count,
                after_count
            );
        }

        #[test]
        fn enriched_variant_searchable_via_fast_path() {
            let mut graph = CodeGraph::new();
            let idx = graph.add_node(make_mapper_node(
                "dao",
                "search",
                Some("SELECT * FROM users WHERE 1=1"),
            ));
            let mut store = GraphStore::from_graph("test", graph);

            // Enrich with variant SQL
            let mut variant_map = std::collections::HashMap::new();
            variant_map.insert(
                "dao.search".to_string(),
                vec!["SELECT * FROM users WHERE 1=1 AND status = __XML_PARAM_status__".to_string()],
            );
            store.enrich_fingerprint_index_with_variants(&variant_map);

            // The variant should be findable via search_by_sql (fingerprint fast-path)
            let results = store.search_by_sql("SELECT * FROM users WHERE 1=1 AND status = ?");
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].1, "mapper:dao.search");
        }

        // --- SQL fingerprint index: backward compatibility ---

        #[test]
        fn fingerprint_bincode_roundtrip_preserves_index() {
            let dir = TempDir::new().unwrap();
            let mut graph = CodeGraph::new();
            graph.add_node(make_mapper_node(
                "dao",
                "findById",
                Some("SELECT * FROM users WHERE id = __XML_PARAM_id__"),
            ));
            let store = GraphStore::from_graph("test", graph);

            assert!(!store.sql_fingerprint_index().is_empty());
            let fp_count = store.sql_fingerprint_index().len();

            let path = dir.path().join("test.bincode");
            store.save_bincode(&path).unwrap();
            let loaded = GraphStore::load_bincode(&path).unwrap();

            assert_eq!(loaded.sql_fingerprint_index().len(), fp_count);
            // Fast-path should still work after round-trip
            let results = loaded.search_by_sql("SELECT * FROM users WHERE id = ?");
            assert_eq!(results.len(), 1);
        }

        #[test]
        fn fingerprint_backward_compat_empty_index_fallback() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_mapper_node(
                "dao",
                "findById",
                Some("SELECT * FROM users WHERE id = __XML_PARAM_id__"),
            ));
            let mut store = GraphStore::from_graph("test", graph);

            // Simulate old cache: manually clear the fingerprint index
            store.sql_fingerprint_index.clear();

            // search_by_sql should still work via fallback (PreparedQuery)
            let results = store.search_by_sql("SELECT * FROM users WHERE id = ?");
            assert_eq!(results.len(), 1);
        }

        // --- SQL fingerprint index: index rebuild ---

        #[test]
        fn fingerprint_index_rebuilt_by_ensure_consistency() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_mapper_node(
                "dao",
                "find",
                Some("SELECT * FROM users WHERE status = 'ACTIVE'"),
            ));
            let mut store = GraphStore::from_graph("test", graph);

            assert!(!store.sql_fingerprint_index().is_empty());
            let fp_before = store.sql_fingerprint_index().clone();

            // Clear core secondary indexes AND fingerprint index
            store.node_summaries.clear();
            store.name_index.clear();
            store.type_tag_index.clear();
            store.sql_fingerprint_index.clear();

            assert!(store.sql_fingerprint_index().is_empty());

            // ensure_consistency rebuilds all indexes including fingerprint index
            store.ensure_consistency();

            // Core indexes are rebuilt
            assert!(!store.node_summaries.is_empty());
            assert!(!store.name_index.is_empty());

            // Fingerprint index is also rebuilt
            assert!(!store.sql_fingerprint_index().is_empty());
            assert_eq!(store.sql_fingerprint_index().len(), fp_before.len());
        }

        // --- SQL fingerprint index: multi-hit scenarios ---

        #[test]
        fn fingerprint_multiple_mappers_same_sql() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_mapper_node(
                "dao",
                "findActive",
                Some("SELECT * FROM users WHERE status = 'ACTIVE'"),
            ));
            graph.add_node(make_mapper_node(
                "dao",
                "findActiveCopy",
                Some("SELECT * FROM users WHERE status = 'ACTIVE'"),
            ));
            let store = GraphStore::from_graph("test", graph);

            // Same SQL → same fingerprint → both in same bucket
            let results = store.search_by_sql("SELECT * FROM users WHERE status = ?");
            assert_eq!(
                results.len(),
                2,
                "both mappers with identical SQL should be found"
            );

            let keys: Vec<&str> = results.iter().map(|(_, k, _)| k.as_str()).collect();
            assert!(keys.contains(&"mapper:dao.findActive"));
            assert!(keys.contains(&"mapper:dao.findActiveCopy"));
        }

        #[test]
        fn fingerprint_mixed_mapper_and_javasql() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_mapper_node(
                "dao",
                "findUser",
                Some("SELECT * FROM users WHERE id = __XML_PARAM_id__"),
            ));
            graph.add_node(make_javasql_node(
                Some("UserService"),
                Some("getOrders"),
                Some("SELECT * FROM orders WHERE user_id = ?"),
            ));
            let store = GraphStore::from_graph("test", graph);

            assert_eq!(store.sql_fingerprint_index().len(), 2);
            assert_eq!(
                store
                    .search_by_sql("SELECT * FROM users WHERE id = ?")
                    .len(),
                1
            );
            assert_eq!(
                store
                    .search_by_sql("SELECT * FROM orders WHERE user_id = ?")
                    .len(),
                1
            );
        }

        #[test]
        fn fingerprint_no_results_for_unrelated_query() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_mapper_node(
                "dao",
                "findUser",
                Some("SELECT * FROM users WHERE id = __XML_PARAM_id__"),
            ));
            let store = GraphStore::from_graph("test", graph);

            let results = store.search_by_sql("DELETE FROM products WHERE category = ?");
            assert!(results.is_empty());
        }

        // --- SQL fingerprint index: enrich edge cases ---

        #[test]
        fn enrich_empty_variant_map_no_crash() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_mapper_node("dao", "find", Some("SELECT 1")));
            let mut store = GraphStore::from_graph("test", graph);

            let empty_map: HashMap<String, Vec<String>> = HashMap::new();
            store.enrich_fingerprint_index_with_variants(&empty_map);

            // No crash, fingerprint count unchanged
            assert_eq!(store.sql_fingerprint_index().len(), 1);
        }

        #[test]
        fn enrich_skips_non_mapper_node_index() {
            let mut graph = CodeGraph::new();
            // Add a procedure node (not a mapper)
            let proc_idx = graph.add_node(make_proc(Some("public"), None, "do_work"));
            let mut store = GraphStore::from_graph("test", graph);

            let before_count = store.sql_fingerprint_index().len();

            // Try to enrich with a non-mapper NodeIndex
            let mut variant_map = HashMap::new();
            variant_map.insert("nonexistent.key".to_string(), vec!["SELECT 1".to_string()]);
            store.enrich_fingerprint_index_with_variants(&variant_map);

            // Should be skipped — no new entries
            assert_eq!(store.sql_fingerprint_index().len(), before_count);
        }

        #[test]
        fn enrich_variant_sql_same_as_primary_adds_to_bucket() {
            let mut graph = CodeGraph::new();
            let idx = graph.add_node(make_mapper_node(
                "dao",
                "search",
                Some("SELECT * FROM users"),
            ));
            let mut store = GraphStore::from_graph("test", graph);

            // Primary SQL already fingerprinted
            let results_before = store.search_by_sql("SELECT * FROM users");
            assert_eq!(results_before.len(), 1);

            // Enrich with the SAME SQL as a variant
            let mut variant_map = HashMap::new();
            variant_map.insert(
                "dao.search".to_string(),
                vec!["SELECT * FROM users".to_string()],
            );
            store.enrich_fingerprint_index_with_variants(&variant_map);

            // Same fingerprint, but now 2 entries in the bucket
            let results_after = store.search_by_sql("SELECT * FROM users");
            assert_eq!(results_after.len(), 2, "duplicate variant adds to bucket");
        }

        #[test]
        fn enrich_multiple_variants_for_same_node() {
            let mut graph = CodeGraph::new();
            let idx = graph.add_node(make_mapper_node(
                "dao",
                "dynamicSearch",
                Some("SELECT * FROM users WHERE 1=1"),
            ));
            let mut store = GraphStore::from_graph("test", graph);

            let mut variant_map = HashMap::new();
            variant_map.insert(
                "dao.dynamicSearch".to_string(),
                vec![
                    "SELECT * FROM users WHERE 1=1 AND status = ?".to_string(),
                    "SELECT * FROM users WHERE 1=1 AND role = ?".to_string(),
                    "SELECT * FROM users WHERE 1=1 AND dept = ?".to_string(),
                ],
            );
            store.enrich_fingerprint_index_with_variants(&variant_map);

            // All 3 variants + 1 primary = 4 findable SQLs
            assert_eq!(
                store
                    .search_by_sql("SELECT * FROM users WHERE 1=1 AND status = ?")
                    .len(),
                1
            );
            assert_eq!(
                store
                    .search_by_sql("SELECT * FROM users WHERE 1=1 AND role = ?")
                    .len(),
                1
            );
            assert_eq!(
                store
                    .search_by_sql("SELECT * FROM users WHERE 1=1 AND dept = ?")
                    .len(),
                1
            );
        }

        // --- SQL fingerprint index: SQL complexity ---

        #[test]
        fn fingerprint_multi_table_join() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_mapper_node(
                "mapper",
                "getUserWithOrders",
                Some(
                    "SELECT u.id, u.name, o.total \
                     FROM users u \
                     JOIN orders o ON u.id = o.user_id \
                     WHERE u.status = __XML_PARAM_status__ \
                     AND o.created > __XML_PARAM_date__",
                ),
            ));
            let store = GraphStore::from_graph("test", graph);

            let results = store.search_by_sql(
                "SELECT u.id, u.name, o.total FROM users u JOIN orders o ON u.id = o.user_id WHERE u.status = ? AND o.created > ?"
            );
            assert_eq!(results.len(), 1);
        }

        #[test]
        fn fingerprint_subquery() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_mapper_node(
                "mapper",
                "findRecent",
                Some(
                    "SELECT * FROM orders WHERE user_id IN \
                     (SELECT id FROM users WHERE status = __XML_PARAM_status__)",
                ),
            ));
            let store = GraphStore::from_graph("test", graph);

            let results = store.search_by_sql(
                "SELECT * FROM orders WHERE user_id IN (SELECT id FROM users WHERE status = ?)",
            );
            assert_eq!(results.len(), 1);
        }

        #[test]
        fn fingerprint_case_when_expression() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_mapper_node(
                "mapper",
                "categorize",
                Some(
                    "SELECT id, \
                     CASE WHEN score >= 90 THEN 'A' \
                          WHEN score >= 80 THEN 'B' \
                          ELSE 'C' END AS grade \
                     FROM students \
                     WHERE class_id = __XML_PARAM_classId__",
                ),
            ));
            let store = GraphStore::from_graph("test", graph);

            let results = store.search_by_sql(
                "SELECT id, CASE WHEN score >= ? THEN ? WHEN score >= ? THEN ? ELSE ? END AS grade FROM students WHERE class_id = ?"
            );
            assert_eq!(results.len(), 1);
        }

        #[test]
        fn fingerprint_group_by_having() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_mapper_node(
                "mapper",
                "salesReport",
                Some(
                    "SELECT region, SUM(amount) as total \
                     FROM sales \
                     WHERE year = __XML_PARAM_year__ \
                     GROUP BY region \
                     HAVING SUM(amount) > __XML_PARAM_threshold__",
                ),
            ));
            let store = GraphStore::from_graph("test", graph);

            let results = store.search_by_sql(
                "SELECT region, SUM(amount) as total FROM sales WHERE year = ? GROUP BY region HAVING SUM(amount) > ?"
            );
            assert_eq!(results.len(), 1);
        }

        #[test]
        fn fingerprint_update_with_set() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_mapper_node(
                "mapper",
                "updateStatus",
                Some(
                    "UPDATE users \
                     SET status = __XML_PARAM_status__, updated_at = __XML_PARAM_now__ \
                     WHERE id = __XML_PARAM_id__",
                ),
            ));
            let store = GraphStore::from_graph("test", graph);

            let results =
                store.search_by_sql("UPDATE users SET status = ?, updated_at = ? WHERE id = ?");
            assert_eq!(results.len(), 1);
        }

        #[test]
        fn fingerprint_delete_with_conditions() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_mapper_node(
                "mapper",
                "cleanOld",
                Some(
                    "DELETE FROM logs \
                     WHERE created < __XML_PARAM_cutoff__ \
                     AND level = __XML_PARAM_level__",
                ),
            ));
            let store = GraphStore::from_graph("test", graph);

            let results = store.search_by_sql("DELETE FROM logs WHERE created < ? AND level = ?");
            assert_eq!(results.len(), 1);
        }

        #[test]
        fn fingerprint_insert_with_values() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_mapper_node(
                "mapper",
                "insertUser",
                Some(
                    "INSERT INTO users (name, email, status) \
                     VALUES (__XML_PARAM_name__, __XML_PARAM_email__, 'ACTIVE')",
                ),
            ));
            let store = GraphStore::from_graph("test", graph);

            let results =
                store.search_by_sql("INSERT INTO users (name, email, status) VALUES (?, ?, ?)");
            assert_eq!(results.len(), 1);
        }

        #[test]
        fn fingerprint_xml_raw_placeholder() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_mapper_node(
                "mapper",
                "dynamicSort",
                Some(
                    "SELECT * FROM users \
                     WHERE status = __XML_PARAM_status__ \
                     ORDER BY __XML_RAW_sortColumn__",
                ),
            ));
            let store = GraphStore::from_graph("test", graph);

            // __XML_RAW_*__ is also replaced with ? in normalize_for_matching
            let results = store.search_by_sql("SELECT * FROM users WHERE status = ? ORDER BY ?");
            assert_eq!(results.len(), 1);
        }

        #[test]
        fn fingerprint_where_one_equals_one_stripped() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_mapper_node(
                "mapper",
                "search",
                Some("SELECT * FROM users WHERE 1=1 AND status = __XML_PARAM_status__"),
            ));
            let store = GraphStore::from_graph("test", graph);

            // normalize_for_matching strips "WHERE 1=1", so the fingerprint
            // should match a query without it
            let results = store.search_by_sql("SELECT * FROM users WHERE status = ?");
            assert_eq!(results.len(), 1);
        }

        #[test]
        fn fingerprint_comment_stripped() {
            let mut graph = CodeGraph::new();
            graph.add_node(make_mapper_node(
                "mapper",
                "findActive",
                Some("SELECT * FROM users /* active only */ WHERE status = 'ACTIVE'"),
            ));
            let store = GraphStore::from_graph("test", graph);

            // Comments are stripped in normalize_for_matching
            let results = store.search_by_sql("SELECT * FROM users WHERE status = ?");
            assert_eq!(results.len(), 1);
        }

        // --- SQL fingerprint index: integration (builder + store) ---

        #[test]
        fn fingerprint_integration_builder_structured_variants() {
            // Simulate what project/mod.rs does: build graph, create store, enrich
            let mut graph = CodeGraph::new();
            let mut _mapper_index: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();

            // Add a static mapper
            let static_idx = graph.add_node(make_mapper_node(
                "dao",
                "findById",
                Some("SELECT * FROM users WHERE id = __XML_PARAM_id__"),
            ));
            _mapper_index.insert("dao.findById".to_string(), static_idx);

            // Add a dynamic mapper (simulating what builder creates)
            let dynamic_idx = graph.add_node(make_mapper_node(
                "dao",
                "search",
                Some("SELECT * FROM users WHERE 1=1"),
            ));
            _mapper_index.insert("dao.search".to_string(), dynamic_idx);

            // Build store (primary fingerprints built here)
            let mut store = GraphStore::from_graph("test", graph);
            assert_eq!(store.sql_fingerprint_index().len(), 2);

            // Simulate variant expansion result from builder
            let mut variant_map = HashMap::new();
            variant_map.insert(
                "dao.search".to_string(),
                vec![
                    "SELECT * FROM users WHERE 1=1 AND status = __XML_PARAM_status__".to_string(),
                    "SELECT * FROM users WHERE 1=1 AND role = __XML_PARAM_role__".to_string(),
                    "SELECT * FROM users WHERE 1=1 AND dept = __XML_PARAM_dept__ AND status = __XML_PARAM_status__".to_string(),
                ],
            );
            store.enrich_fingerprint_index_with_variants(&variant_map);

            // Static mapper: found via primary fingerprint
            assert_eq!(
                store
                    .search_by_sql("SELECT * FROM users WHERE id = ?")
                    .len(),
                1
            );

            // Dynamic variants: found via enriched fingerprints
            assert_eq!(
                store
                    .search_by_sql("SELECT * FROM users WHERE 1=1 AND status = ?")
                    .len(),
                1
            );
            assert_eq!(
                store
                    .search_by_sql("SELECT * FROM users WHERE 1=1 AND role = ?")
                    .len(),
                1
            );
            assert_eq!(
                store
                    .search_by_sql("SELECT * FROM users WHERE 1=1 AND dept = ? AND status = ?")
                    .len(),
                1
            );

            // Original flat_sql still findable
            assert_eq!(
                store.search_by_sql("SELECT * FROM users WHERE 1=1").len(),
                1
            );
        }

        #[test]
        fn fingerprint_integration_roundtrip_with_variants() {
            let dir = TempDir::new().unwrap();

            let mut graph = CodeGraph::new();
            let idx = graph.add_node(make_mapper_node(
                "dao",
                "search",
                Some("SELECT * FROM users WHERE 1=1"),
            ));
            let mut store = GraphStore::from_graph("test", graph);

            let mut variant_map = HashMap::new();
            variant_map.insert(
                "dao.search".to_string(),
                vec!["SELECT * FROM users WHERE 1=1 AND status = __XML_PARAM_status__".to_string()],
            );
            store.enrich_fingerprint_index_with_variants(&variant_map);

            // Save + reload
            let path = dir.path().join("test.bincode");
            store.save_bincode(&path).unwrap();
            let loaded = GraphStore::load_bincode(&path).unwrap();

            // Both primary and variant fingerprints survive
            assert_eq!(
                loaded.search_by_sql("SELECT * FROM users WHERE 1=1").len(),
                1
            );
            assert_eq!(
                loaded
                    .search_by_sql("SELECT * FROM users WHERE 1=1 AND status = ?")
                    .len(),
                1
            );
        }
    }
}

# Incremental Graph Store + Project + TUI Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement incremental graph building with Project as first-class concept, persistent GraphStore, merge support, and TUI mode.

**Architecture:** Project (codeweb.toml) owns a GraphStore (persisted binary/JSON). GraphStore wraps petgraph::Graph with stable NodeKey indexing, file→node mapping, reverse dependency tracking, and parsed result cache. CLI and TUI share the same library core. Feature flags gate CLI (clap) and TUI (ratatui) independently.

**Tech Stack:** Rust, petgraph, serde+bincode+toml, blake3, clap, ratatui+crossterm

**Prerequisite:** Current codebase at Phase 3 complete (18/18 tests passing, clippy clean).

---

## Dependency Graph

```
Task 1 (NodeKey) ──────────┐
Task 2 (Serde on types) ───┼──► Task 4 (GraphStore) ──► Task 6 (Project) ──► Task 8 (Incremental)
Task 3 (blake3 + scanner) ─┘       │                        │                  │
                                    ▼                        ▼                  ▼
                            Task 5 (Builder refactor)    Task 7 (CLI init)   Task 9 (Merge)
                                    │                        │                  │
                                    ▼                        ▼                  ▼
                                 (all above) ──────► Task 10 (CLI refactor) ──► Task 11 (TUI)
```

Tasks 1, 2, 3 are independent and can run in parallel.
Tasks 4, 5 depend on Task 1 (and Task 4 also on 2, 3).
Task 6 depends on 4, 5.
Tasks 8, 9 depend on 6.
Task 10 depends on 6, 7.
Task 11 depends on 10.

---

## Phase A: Foundation (parallel)

### Task 1: Add NodeKey + Node::key()

**Files:**
- Create: `src/graph/key.rs`
- Modify: `src/graph/mod.rs` (add `pub mod key;` and `use`)

**Step 1: Create `src/graph/key.rs`**

```rust
use serde::{Deserialize, Serialize};
use std::fmt;

/// Stable node identity independent of petgraph's NodeIndex.
/// Two nodes with the same NodeKey represent the same semantic entity
/// regardless of which GraphStore they belong to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeKey {
    Procedure {
        schema: Option<String>,
        name: String,
    },
    Mapper {
        namespace: String,
        statement_id: String,
    },
    JavaMethod {
        fqn: String,
    },
    JavaClass {
        fqn: String,
    },
    Table {
        schema: Option<String>,
        name: String,
    },
    View {
        schema: Option<String>,
        name: String,
    },
    JavaSql {
        file: String,
        line: usize,
    },
    Unresolved {
        raw_expr: String,
        context: String,
    },
}

impl fmt::Display for NodeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NodeKey::Procedure { schema, name } => {
                match schema {
                    Some(s) => write!(f, "proc:{}.{}", s, name),
                    None => write!(f, "proc:{}", name),
                }
            }
            NodeKey::Mapper { namespace, statement_id } => {
                write!(f, "mapper:{}.{}", namespace, statement_id)
            }
            NodeKey::JavaMethod { fqn } => write!(f, "method:{}", fqn),
            NodeKey::JavaClass { fqn } => write!(f, "class:{}", fqn),
            NodeKey::Table { schema, name } => match schema {
                Some(s) => write!(f, "table:{}.{}", s, name),
                None => write!(f, "table:{}", name),
            },
            NodeKey::View { schema, name } => match schema {
                Some(s) => write!(f, "view:{}.{}", s, name),
                None => write!(f, "view:{}", name),
            },
            NodeKey::JavaSql { file, line } => write!(f, "javasql:{}:{}", file, line),
            NodeKey::Unresolved { raw_expr, context } => {
                write!(f, "unresolved:{} (in {})", raw_expr, context)
            }
        }
    }
}

/// Extract NodeKey from a Node variant.
/// This is the single source of truth for node identity.
impl NodeKey {
    pub fn from_node(node: &super::Node) -> Self {
        match node {
            super::Node::Procedure { id, .. } => NodeKey::Procedure {
                schema: id.schema.clone(),
                name: id.name.clone(),
            },
            super::Node::MappedStatement {
                namespace,
                statement_id,
                ..
            } => NodeKey::Mapper {
                namespace: namespace.clone(),
                statement_id: statement_id.clone(),
            },
            super::Node::JavaMethod { fqn, .. } => NodeKey::JavaMethod { fqn: fqn.clone() },
            super::Node::JavaClass { fqn, .. } => NodeKey::JavaClass { fqn: fqn.clone() },
            super::Node::Table { schema, name } => NodeKey::Table {
                schema: schema.clone(),
                name: name.clone(),
            },
            super::Node::View { schema, name } => NodeKey::View {
                schema: schema.clone(),
                name: name.clone(),
            },
            super::Node::JavaSql { java_file, line, .. } => NodeKey::JavaSql {
                file: java_file.to_string_lossy().to_string(),
                line: *line,
            },
            super::Node::Unresolved { raw_expr, context } => NodeKey::Unresolved {
                raw_expr: raw_expr.clone(),
                context: context.clone(),
            },
        }
    }
}
```

**Step 2: Add `pub mod key;` to `src/graph/mod.rs`**

After line 1 (`pub mod builder;`), add:
```rust
pub mod key;
```

**Step 3: Verify**

Run: `cargo build`
Expected: Compiles without errors.

Run: `cargo test`
Expected: All 18 existing tests still pass.

**Step 4: Commit**

```bash
git add src/graph/key.rs src/graph/mod.rs
git commit -m "feat: add NodeKey — stable node identity for incremental graph building"
```

---

### Task 2: Add Serialize/Deserialize to Java types

**Files:**
- Modify: `src/parser/java_method.rs`

**Step 1: Add serde derives**

Add `use serde::{Deserialize, Serialize};` to imports.

Add `#[derive(Debug, Clone, Serialize, Deserialize)]` to:
- `JavaClassInfo` (line 21)
- `MethodCallInfo` (line 33)
- `JavaMethodInfo` (line 40)
- `JavaParseResult` (line 50)

Also need to add `serde` to the `Serialize`/`Deserialize` imports.

**Step 2: Verify**

Run: `cargo build`
Expected: Compiles. All tests pass.

**Step 3: Commit**

```bash
git add src/parser/java_method.rs
git commit -m "feat: add Serialize/Deserialize to Java parse result types"
```

---

### Task 3: Add blake3 + FileRecord + hash computation

**Files:**
- Modify: `Cargo.toml` (add blake3, globset, toml, bincode)
- Create: `src/parser/fingerprint.rs`
- Modify: `src/parser/mod.rs` (add `pub mod fingerprint;`)
- Modify: `src/parser/scanner.rs` (add hash computation to ScannedFiles)

**Step 1: Add dependencies to Cargo.toml**

```toml
blake3 = "1"
globset = "0.4"
toml = "0.8"
bincode = "1"
```

Add these after the existing dependencies.

**Step 2: Create `src/parser/fingerprint.rs`**

```rust
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// File type classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileType {
    Sql,
    Java,
    Xml,
}

/// File fingerprint for change detection.
/// Two FileRecords with the same content_hash represent identical file contents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
    pub path: PathBuf,
    pub content_hash: String,
    pub mtime_ns: u128,
    pub size: u64,
    pub file_type: FileType,
    pub parse_ok: bool,
    pub node_count: usize,
}

impl FileRecord {
    /// Compute a FileRecord for the given path.
    /// Returns None if the file cannot be read.
    pub fn compute(path: &std::path::Path, file_type: FileType) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        let mtime_ns = metadata.modified().ok()?
            .duration_since(std::time::UNIX_EPOCH).ok()?
            .as_nanos();
        let size = metadata.len();

        let bytes = std::fs::read(path).ok()?;
        let content_hash = blake3::hash(&bytes).to_hex().to_string();

        Some(FileRecord {
            path: path.to_path_buf(),
            content_hash,
            mtime_ns,
            size,
            file_type,
            parse_ok: false, // set after parsing
            node_count: 0,   // set after graph building
        })
    }

    /// Quick check: if mtime hasn't changed, content definitely hasn't changed.
    /// Use this to skip hash computation for unchanged files.
    pub fn mtime_matches(&self, path: &std::path::Path) -> bool {
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH))
            .map(|d| d.as_nanos() == self.mtime_ns)
            .unwrap_or(false)
    }
}

/// Classification of file changes since last analysis.
#[derive(Debug, Default)]
pub struct FileChangeSet {
    pub unchanged: Vec<PathBuf>,
    pub modified: Vec<PathBuf>,
    pub added: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
}

/// Compare current files against a manifest to determine changes.
pub fn compute_changes(
    current_files: &[(PathBuf, FileType)],
    manifest: &std::collections::HashMap<PathBuf, FileRecord>,
) -> FileChangeSet {
    let mut changes = FileChangeSet::default();
    let current_set: std::collections::HashSet<_> =
        current_files.iter().map(|(p, _)| p.clone()).collect();

    // Check current files against manifest
    for (path, file_type) in current_files {
        match manifest.get(path) {
            None => {
                changes.added.push(path.clone());
            }
            Some(record) => {
                // Fast path: mtime unchanged → definitely unchanged
                if record.mtime_matches(path) {
                    changes.unchanged.push(path.clone());
                } else {
                    // Slow path: compute hash
                    if let Some(current) = FileRecord::compute(path, *file_type) {
                        if current.content_hash == record.content_hash {
                            // mtime changed but content didn't (touch)
                            changes.unchanged.push(path.clone());
                        } else {
                            changes.modified.push(path.clone());
                        }
                    } else {
                        // Can't read file — treat as modified
                        changes.modified.push(path.clone());
                    }
                }
            }
        }
    }

    // Find deleted files
    for path in manifest.keys() {
        if !current_set.contains(path) {
            changes.deleted.push(path.clone());
        }
    }

    changes
}
```

**Step 3: Add `pub mod fingerprint;` to `src/parser/mod.rs`**

**Step 4: Modify `src/parser/scanner.rs`**

Add `fingerprint` info to the return type. Create a new struct:

```rust
use crate::parser::fingerprint::FileType;

/// Extended scan result with file type classification.
pub struct ScannedFile {
    pub path: PathBuf,
    pub file_type: FileType,
}

#[derive(Debug)]
pub struct ScannedFiles {
    pub files: Vec<ScannedFile>,          // NEW: unified list
    pub sql_files: Vec<PathBuf>,           // kept for backward compat
    pub java_files: Vec<PathBuf>,          // kept for backward compat
    pub xml_files: Vec<PathBuf>,           // kept for backward compat
}
```

In `scan_directory`, also populate the `files` vec with `ScannedFile` entries.

Keep the existing `sql_files`, `java_files`, `xml_files` fields for backward compatibility with existing code.

**Step 5: Verify**

Run: `cargo build`
Run: `cargo test`
Expected: All 18 tests pass.

**Step 6: Commit**

```bash
git add Cargo.toml src/parser/fingerprint.rs src/parser/mod.rs src/parser/scanner.rs
git commit -m "feat: add file fingerprinting with blake3 hashes for change detection"
```

---

## Phase B: GraphStore + Builder Refactor

### Task 4: Create GraphStore struct with persistence

**Files:**
- Create: `src/graph/store.rs`
- Modify: `src/graph/mod.rs` (add `pub mod store;`)
- Modify: `src/error.rs` (add store-related errors)
- Modify: `Cargo.toml` (add `dirs` dependency for default store path, if needed)

**Step 1: Create `src/graph/store.rs`**

This is the largest new file. Key structures:

```rust
use crate::graph::key::NodeKey;
use crate::graph::{CodeGraph, Node};
use crate::parser::fingerprint::FileRecord;
use petgraph::graph::NodeIndex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Persistent graph store supporting incremental updates and merge.
#[derive(Serialize, Deserialize)]
pub struct GraphStore {
    /// Format version for future migration.
    pub version: u32,
    /// Project name this store belongs to.
    pub project_name: String,
    /// Creation timestamp (unix epoch ms).
    pub created_at: u64,
    /// Last update timestamp.
    pub updated_at: u64,

    // ─── Graph ───
    graph: CodeGraph,
    /// Stable node identity → petgraph NodeIndex.
    node_key_index: HashMap<NodeKey, NodeIndex>,

    // ─── File mapping ───
    /// File → list of NodeKeys produced by that file.
    file_nodes: HashMap<PathBuf, Vec<NodeKey>>,
    /// File → list of (source_key, target_key) edges produced by that file.
    file_edges: HashMap<PathBuf, Vec<(NodeKey, NodeKey)>>,

    // ─── Reverse dependency ───
    /// target_file → set of source_files that have edges targeting target_file's nodes.
    reverse_deps: HashMap<PathBuf, HashSet<PathBuf>>,

    // ─── File fingerprint ───
    manifest: HashMap<PathBuf, FileRecord>,

    // ─── Parse cache ─── (placeholder, will be populated in Phase D)
    #[serde(default)]
    parse_cache_version: u32, // bump when ParsedCache format changes
}

impl GraphStore {
    /// Create a new empty store for a project.
    pub fn new(project_name: &str) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
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
            parse_cache_version: 0,
        }
    }

    /// Create from a fully-built CodeGraph (used by GraphBuilder).
    pub fn from_graph(
        project_name: &str,
        graph: CodeGraph,
        node_key_index: HashMap<NodeKey, NodeIndex>,
        file_nodes: HashMap<PathBuf, Vec<NodeKey>>,
        file_edges: HashMap<PathBuf, Vec<(NodeKey, NodeIndex)>>,
    ) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        // Convert file_edges from NodeIndex targets to NodeKey targets
        let file_edges_keyed: HashMap<PathBuf, Vec<(NodeKey, NodeKey)>> = file_edges
            .into_iter()
            .map(|(file, edges)| {
                let keyed: Vec<(NodeKey, NodeKey)> = edges
                    .into_iter()
                    .filter_map(|(src_key, dst_idx)| {
                        let dst_key = graph.node_indices()
                            .find(|&idx| idx == dst_idx)
                            .and_then(|idx| {
                                let key = NodeKey::from_node(&graph[idx]);
                                // verify the dst_idx maps back
                                node_key_index.get(&key).map(|_| key)
                            });
                        dst_key.map(|dk| (src_key, dk))
                    })
                    .collect();
                (file, keyed)
            })
            .collect();

        // Compute reverse_deps from file_edges
        let mut reverse_deps: HashMap<PathBuf, HashSet<PathBuf>> = HashMap::new();
        // We need to know which file each target node belongs to
        // Build a reverse map: NodeKey → source_file
        let mut node_to_file: HashMap<NodeKey, PathBuf> = HashMap::new();
        for (file, keys) in &file_nodes {
            for key in keys {
                node_to_file.insert(key.clone(), file.clone());
            }
        }
        for (src_file, edges) in &file_edges_keyed {
            for (_, dst_key) in edges {
                if let Some(dst_file) = node_to_file.get(dst_key) {
                    if dst_file != src_file {
                        reverse_deps
                            .entry(dst_file.clone())
                            .or_default()
                            .insert(src_file.clone());
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
            file_edges: file_edges_keyed,
            reverse_deps,
            manifest: HashMap::new(),
            parse_cache_version: 0,
        }
    }

    // ─── Accessors ───

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
            }
        }
        s.edges = self.graph.edge_count();
        s.files = self.manifest.len();
        s
    }

    // ─── Persistence ───

    /// Save to bincode format.
    pub fn save_bincode(&self, path: &Path) -> crate::error::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| crate::error::CodeWebError::FileRead {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
        let bytes = bincode::serialize(self).map_err(|e| crate::error::CodeWebError::ExportError {
            message: format!("bincode serialize: {}", e),
        })?;
        std::fs::write(path, bytes).map_err(|e| crate::error::CodeWebError::FileRead {
            path: path.to_path_buf(),
            source: e,
        })?;
        Ok(())
    }

    /// Load from bincode format.
    pub fn load_bincode(path: &Path) -> crate::error::Result<Self> {
        let bytes = std::fs::read(path).map_err(|e| crate::error::CodeWebError::FileRead {
            path: path.to_path_buf(),
            source: e,
        })?;
        bincode::deserialize(&bytes).map_err(|e| crate::error::CodeWebError::ExportError {
            message: format!("bincode deserialize: {}", e),
        })
    }

    /// Save to JSON format (human-readable).
    pub fn save_json(&self, path: &Path) -> crate::error::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| crate::error::CodeWebError::FileRead {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| crate::error::CodeWebError::ExportError {
                message: format!("json serialize: {}", e),
            })?;
        std::fs::write(path, json).map_err(|e| crate::error::CodeWebError::FileRead {
            path: path.to_path_buf(),
            source: e,
        })?;
        Ok(())
    }

    /// Load from JSON format.
    pub fn load_json(path: &Path) -> crate::error::Result<Self> {
        let json = std::fs::read_to_string(path).map_err(|e| crate::error::CodeWebError::FileRead {
            path: path.to_path_buf(),
            source: e,
        })?;
        serde_json::from_str(&json).map_err(|e| crate::error::CodeWebError::ExportError {
            message: format!("json deserialize: {}", e),
        })
    }

    // ─── Manifest management ───

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

    fn touch(&mut self) {
        self.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
    }
}

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
```

**Step 2: Add `pub mod store;` to `src/graph/mod.rs`**

**Step 3: Add `bincode` dependency to Cargo.toml** (done in Task 3)

**Step 4: Verify**

Run: `cargo build`
Expected: Compiles.

**Step 5: Commit**

```bash
git add src/graph/store.rs src/graph/mod.rs
git commit -m "feat: add GraphStore — persistent graph storage with NodeKey indexing"
```

---

### Task 5: Refactor GraphBuilder to produce GraphStore metadata

**Files:**
- Modify: `src/graph/builder.rs`

**Goal:** `build_all()` returns `GraphStore` instead of raw `CodeGraph`. It populates `file_nodes`, `file_edges`, and `node_key_index` during construction.

**Step 1: Add a new method `build_store()` alongside existing `build_all()`**

Keep `build_all()` working for backward compatibility during transition. Add `build_store()` that wraps it.

```rust
use crate::graph::key::NodeKey;
use crate::graph::store::GraphStore;

impl GraphBuilder {
    /// Build a GraphStore from all parsed files.
    /// This is the new primary API that produces full store metadata.
    pub fn build_store(&self, all: &AllParsedFiles, project_name: &str) -> GraphStore {
        // Use existing build_all to get the graph + indices
        // But we need to refactor to capture file_nodes and file_edges
        
        let mut graph = CodeGraph::new();
        let mut proc_index: HashMap<ProcedureId, petgraph::graph::NodeIndex> = HashMap::new();
        let mut mapper_index: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();
        let mut table_index: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();
        let mut file_nodes: HashMap<PathBuf, Vec<NodeKey>> = HashMap::new();
        let mut file_edges: HashMap<PathBuf, Vec<(NodeKey, petgraph::graph::NodeIndex)>> = HashMap::new();

        Self::create_procedure_nodes_tracked(&all.sql_files, &mut graph, &mut proc_index, &mut file_nodes);
        let edges = Self::collect_call_edges(&all.sql_files);
        Self::create_edges_tracked(&edges, &mut graph, &mut proc_index, &mut file_edges);
        Self::add_table_refs_from_sql_tracked(&all.sql_files, &mut graph, &proc_index, &mut table_index, &mut file_nodes, &mut file_edges);
        
        Self::add_ibatis_nodes_tracked(
            &all.ibatis_files, &mut graph, &mut proc_index, &mut mapper_index, &mut table_index,
            &mut file_nodes, &mut file_edges,
        );
        // ... (continue for all add_* methods, adding _tracked versions)

        // Build node_key_index from graph
        let node_key_index: HashMap<NodeKey, petgraph::graph::NodeIndex> = graph
            .node_indices()
            .map(|idx| (NodeKey::from_node(&graph[idx]), idx))
            .collect();

        GraphStore::from_graph(
            project_name,
            graph,
            node_key_index,
            file_nodes,
            file_edges,
        )
    }
}
```

The _tracked versions of each method are identical to the current ones, but also push NodeKey entries into `file_nodes` and edge tuples into `file_edges` as they create nodes and edges.

**IMPORTANT:** Keep the existing `build()` and `build_all()` methods untouched so all 18 tests continue to pass. `build_store()` is a new method.

**Step 2: Verify**

Run: `cargo test`
Expected: All 18 tests pass (they use `build_all`, not `build_store`).

**Step 3: Write new tests for `build_store()`**

In `tests/integration_test.rs`, add:

```rust
#[test]
fn test_build_store_produces_node_key_index() {
    let all = parser::load_all_files(
        Path::new("lib/codeweb-e2e-demo"),
    ).unwrap();
    let builder = graph::builder::GraphBuilder::new();
    let store = builder.build_store(&all, "test-project");
    
    // Node key index should have entries for all nodes
    assert!(!store.node_key_index().is_empty());
    assert_eq!(store.graph().node_count(), store.node_key_index().len());
    
    // File nodes should track which files produced which nodes
    assert!(!store.file_nodes().is_empty());
}
```

**Step 4: Verify**

Run: `cargo test`
Expected: 19 tests pass.

**Step 5: Commit**

```bash
git add src/graph/builder.rs tests/integration_test.rs
git commit -m "feat: add build_store() to GraphBuilder producing full GraphStore metadata"
```

---

## Phase C: Project + Config

### Task 6: Create Project module with codeweb.toml parsing

**Files:**
- Create: `src/project/mod.rs`
- Create: `src/project/config.rs`
- Modify: `src/main.rs` (add `mod project;`)
- Modify: `src/lib.rs` or add if not exists

**Step 1: Create `src/project/config.rs`**

```rust
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub struct ProjectConfig {
    pub project: ProjectMeta,
    #[serde(default)]
    pub analysis: AnalysisConfig,
    #[serde(default)]
    pub store: StoreConfig,
}

#[derive(Debug, Deserialize)]
pub struct ProjectMeta {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct AnalysisConfig {
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub encoding: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct StoreConfig {
    #[serde(default = "default_store_path")]
    pub path: String,
    #[serde(default = "default_store_format")]
    pub format: StoreFormat,
}

fn default_store_path() -> String {
    ".codeweb/store.bincode".to_string()
}

fn default_store_format() -> StoreFormat {
    StoreFormat::Bincode
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StoreFormat {
    Bincode,
    Json,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            path: default_store_path(),
            format: default_store_format(),
        }
    }
}

impl ProjectConfig {
    pub fn load(toml_content: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(toml_content)
    }

    pub fn default_template(name: &str) -> String {
        format!(
            r#"[project]
name = "{}"

[analysis]
paths = ["."]

[store]
path = ".codeweb/store.bincode"
format = "bincode"
"#,
            name
        )
    }
}
```

**Step 2: Create `src/project/mod.rs`**

```rust
pub mod config;

use config::ProjectConfig;
use crate::error::{CodeWebError, Result};
use crate::graph::store::GraphStore;
use crate::graph::builder::GraphBuilder;
use crate::parser;
use crate::parser::fingerprint::{FileRecord, FileType, compute_changes, FileChangeSet};
use std::path::{Path, PathBuf};

const CODEWEB_TOML: &str = "codeweb.toml";

pub struct Project {
    root: PathBuf,
    config: ProjectConfig,
    store: Option<GraphStore>,
}

/// Result of an incremental analysis run.
#[derive(Debug)]
pub struct AnalyzeReport {
    pub files_scanned: usize,
    pub files_changed: usize,
    pub files_added: usize,
    pub files_deleted: usize,
    pub files_unchanged: usize,
    pub nodes_before: usize,
    pub nodes_after: usize,
    pub edges_before: usize,
    pub edges_after: usize,
    pub is_full_build: bool,
    pub elapsed_ms: u64,
}

impl Project {
    /// Find a project by searching for codeweb.toml starting from `dir`
    /// and walking up to parent directories.
    pub fn find(dir: &Path) -> Result<Self> {
        let mut current = if dir.is_absolute() {
            dir.to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_default().join(dir)
        };

        loop {
            let toml_path = current.join(CODEWEB_TOML);
            if toml_path.exists() {
                return Self::load_from(&toml_path);
            }
            match current.parent() {
                Some(parent) => current = parent.to_path_buf(),
                None => break,
            }
        }

        Err(CodeWebError::ProjectNotFound {
            search_from: dir.to_path_buf(),
        })
    }

    /// Initialize a new project in the given directory.
    pub fn init(dir: &Path, name: &str) -> Result<Self> {
        let dir = if dir.is_absolute() {
            dir.to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_default().join(dir)
        };

        let toml_path = dir.join(CODEWEB_TOML);
        if toml_path.exists() {
            return Err(CodeWebError::ProjectAlreadyExists {
                path: toml_path,
            });
        }

        let content = ProjectConfig::default_template(name);
        std::fs::write(&toml_path, &content).map_err(|e| CodeWebError::FileRead {
            path: toml_path.clone(),
            source: e,
        })?;

        // Create .codeweb directory
        let codeweb_dir = dir.join(".codeweb");
        std::fs::create_dir_all(&codeweb_dir).map_err(|e| CodeWebError::FileRead {
            path: codeweb_dir,
            source: e,
        })?;

        Self::load_from(&toml_path)
    }

    fn load_from(toml_path: &Path) -> Result<Self> {
        let root = toml_path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let content = std::fs::read_to_string(toml_path).map_err(|e| CodeWebError::FileRead {
            path: toml_path.to_path_buf(),
            source: e,
        })?;
        let config = ProjectConfig::load(&content).map_err(|e| CodeWebError::ConfigError {
            message: e.to_string(),
        })?;

        Ok(Project {
            root,
            config,
            store: None,
        })
    }

    /// Load the existing GraphStore from disk, if it exists.
    pub fn load_store(&mut self) -> Result<()> {
        let store_path = self.store_path();
        if !store_path.exists() {
            self.store = None;
            return Ok(());
        }

        self.store = match self.config.store.format {
            config::StoreFormat::Bincode => Some(GraphStore::load_bincode(&store_path)?),
            config::StoreFormat::Json => Some(GraphStore::load_json(&store_path)?),
        };
        Ok(())
    }

    /// Perform analysis (full or incremental).
    pub fn analyze(&mut self) -> Result<AnalyzeReport> {
        let start = std::time::Instant::now();

        // Load existing store if not already loaded
        if self.store.is_none() {
            let _ = self.load_store(); // ignore error — first run has no store
        }

        let nodes_before = self.store.as_ref().map(|s| s.graph().node_count()).unwrap_or(0);
        let edges_before = self.store.as_ref().map(|s| s.graph().edge_count()).unwrap_or(0);

        // Determine input paths
        let input_paths: Vec<PathBuf> = if self.config.analysis.paths.is_empty() {
            vec![self.root.clone()]
        } else {
            self.config.analysis.paths.iter()
                .map(|p| self.root.join(p))
                .collect()
        };

        // For now: full build (incremental will be added in Task 8)
        let mut all_files = parser::AllParsedFiles {
            sql_files: Vec::new(),
            java_files: Vec::new(),
            ibatis_files: Vec::new(),
            java_method_results: Vec::new(),
        };

        for input in &input_paths {
            let loaded = parser::load_all_files(input)?;
            all_files.sql_files.extend(loaded.sql_files);
            all_files.java_files.extend(loaded.java_files);
            all_files.ibatis_files.extend(loaded.ibatis_files);
            all_files.java_method_results.extend(loaded.java_method_results);
        }

        let files_scanned = all_files.sql_files.len()
            + all_files.java_files.len()
            + all_files.ibatis_files.len();

        let builder = GraphBuilder::new();
        let new_store = builder.build_store(&all_files, &self.config.project.name);

        let nodes_after = new_store.graph().node_count();
        let edges_after = new_store.graph().edge_count();

        // Save
        self.save_store(&new_store)?;
        self.store = Some(new_store);

        let elapsed_ms = start.elapsed().as_millis() as u64;

        Ok(AnalyzeReport {
            files_scanned,
            files_changed: files_scanned,
            files_added: 0,
            files_deleted: 0,
            files_unchanged: 0,
            nodes_before,
            nodes_after,
            edges_before,
            edges_after,
            is_full_build: true,
            elapsed_ms,
        })
    }

    /// Get a reference to the current store.
    pub fn store(&self) -> Option<&GraphStore> {
        self.store.as_ref()
    }

    /// Project root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Project name.
    pub fn name(&self) -> &str {
        &self.config.project.name
    }

    fn store_path(&self) -> PathBuf {
        self.root.join(&self.config.store.path)
    }

    fn save_store(&self, store: &GraphStore) -> Result<()> {
        let path = self.store_path();
        match self.config.store.format {
            config::StoreFormat::Bincode => store.save_bincode(&path)?,
            config::StoreFormat::Json => store.save_json(&path)?,
        }
        Ok(())
    }
}
```

**Step 3: Add error variants to `src/error.rs`**

```rust
#[error("project not found (searched from {search_from})")]
ProjectNotFound { search_from: PathBuf },

#[error("project already exists at {path}")]
ProjectAlreadyExists { path: PathBuf },

#[error("config error: {message}")]
ConfigError { message: String },
```

**Step 4: Add `mod project;` to `src/main.rs`**

**Step 5: Write tests**

```rust
#[test]
fn test_project_init_and_analyze() {
    let tmp = tempfile::tempdir().unwrap();
    // Copy demo files
    // Init project
    // Analyze
    // Verify store was created
}
```

**Step 6: Verify**

Run: `cargo test`
Expected: All tests pass.

**Step 7: Commit**

```bash
git add src/project/ src/error.rs src/main.rs
git commit -m "feat: add Project module with codeweb.toml config and analyze"
```

---

## Phase D: Incremental + Merge

### Task 7: Implement incremental update algorithm

**Files:**
- Modify: `src/project/mod.rs` (enhance `analyze()` with incremental logic)

This task replaces the "always full build" in `Project::analyze()` with proper incremental:
1. Load existing store
2. Scan + fingerprint
3. Compute changes
4. Remove affected nodes/edges
5. Re-parse changed files
6. Rebuild subgraph
7. Upgrade unresolved

**Detailed code omitted for brevity — see design discussion above.**

**Verification:** Test that modifying one file only re-parses that file, and nodes from unchanged files keep their NodeIndex.

---

### Task 8: Implement merge algorithm

**Files:**
- Add method to `src/graph/store.rs`

```rust
pub fn merge(stores: Vec<GraphStore>) -> GraphStore { ... }
```

**Verification:** Test merging two demo project stores, verify shared nodes are deduplicated.

---

## Phase E: CLI Refactor

### Task 9: Refactor CLI with subcommands

**Files:**
- Modify: `src/main.rs`

New clap structure:
```rust
#[derive(Parser)]
#[command(name = "codeweb")]
enum Cli {
    /// Initialize a new codeweb project
    Init { name: String, #[arg(short, long, default_value = ".")] dir: PathBuf },
    /// Analyze project (full or incremental)
    Analyze { ... },
    /// Show changes since last analysis
    Diff { ... },
    /// Export graph to various formats
    Export { ... },
    /// Merge multiple projects
    Merge { ... },
    /// Launch TUI mode
    Tui { ... },
}
```

Keep backward compatibility: if no subcommand, fall back to old behavior.

---

## Phase F: TUI

### Task 10: Add TUI feature flag + dependencies

**Files:**
- Modify: `Cargo.toml`

```toml
[features]
default = ["cli", "tui"]
cli = ["clap"]
tui = ["ratatui", "crossterm"]

[dependencies]
ratatui = { version = "0.29", optional = true }
crossterm = { version = "0.28", optional = true }
```

### Task 11: Implement TUI screens

**Files:**
- Create: `src/tui/mod.rs`
- Create: `src/tui/app.rs`
- Create: `src/tui/event.rs`
- Create: `src/tui/ui/mod.rs`
- Create: `src/tui/ui/dashboard.rs`
- Create: `src/tui/ui/files.rs`
- Create: `src/tui/ui/graph_browser.rs`
- Create: `src/tui/ui/diff_view.rs`
- Create: `src/tui/ui/merge_view.rs`

Each screen is a self-contained rendering module. The `App` struct manages screen routing and state.

**This is the largest task by LOC but each screen is independent.**

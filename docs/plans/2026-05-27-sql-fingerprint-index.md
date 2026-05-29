# SQL Fingerprint Index Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add offline SQL fingerprint index to cobweb's GraphStore, leveraging ogsql-parser's new `expand_variants()` API to generate fingerprints for all possible SQL variants from dynamic iBatis/MyBatis mapper XML.

**Architecture:** Three-layer change: (1) update ogsql-parser dependency and wire `parse_mapper_bytes_structured()` into the ibatis loader, (2) add `SqlFingerprintIndex` to `GraphStore` as a `HashMap<String, Vec<(NodeIndex, String)>>` built during `from_graph()` / `rebuild_secondary_indexes()`, (3) modify `search_by_sql()` to fast-path via fingerprint lookup before falling back to existing `PreparedQuery::matches()`.

**Tech Stack:** Rust, ogsql-parser `StructuredStatement::expand_variants()`, blake3 hashing (already in deps), existing `normalize_for_matching()` pipeline.

---

## Task 1: Update ogsql-parser dependency to latest main

**Files:**
- Modify: `Cargo.toml`

**Step 1: Update the rev to latest main (088eb4d6)**

```toml
ogsql-parser = { git = "https://github.com/c2j/ogsql-parser", rev = "088eb4d6", features = ["ibatis", "java"] }
```

**Step 2: Build to verify compilation**

Run: `cargo build`
Expected: Compiles successfully (may need minor API adjustments if any breaking changes from rev `388aeaae` → `088eb4d6`)

**Step 3: Run existing tests**

Run: `cargo test`
Expected: All existing tests pass

**Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: update ogsql-parser to 088eb4d6 (structured dynamic SQL AST)"
```

---

## Task 2: Wire structured parsing into ibatis_loader

**Files:**
- Modify: `src/parser/ibatis_loader.rs`

**Step 1: Add `StructuredMapper` parsing alongside existing `ParsedMapper`**

Add a new struct and loader function that calls `parse_mapper_bytes_structured_with_path()`:

```rust
use ogsql_parser::ibatis::{StructuredMapper, parse_mapper_bytes_structured_with_path};

pub struct IbatisStructuredFile {
    pub path: PathBuf,
    pub result: StructuredMapper,
    pub content_hash: String,
}

pub fn load_ibatis_structured_files_from_paths(paths: &[PathBuf]) -> Vec<IbatisStructuredFile> {
    use rayon::prelude::*;
    paths
        .par_iter()
        .filter_map(|path| {
            load_ibatis_structured_file(path)
                .ok()
                .map(|(result, hash)| IbatisStructuredFile {
                    path: path.clone(),
                    result,
                    content_hash: hash,
                })
        })
        .collect()
}

fn load_ibatis_structured_file(path: &Path) -> Result<(StructuredMapper, String), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read error: {}", e))?;
    let content_hash = blake3::hash(&bytes).to_hex().to_string();
    let file_path = path.to_string_lossy().to_string();
    let result = parse_mapper_bytes_structured_with_path(&bytes, Some(&file_path));

    if result.namespace.is_empty() && result.statements.is_empty() && result.errors.is_empty() {
        crate::parse_log::info(&file_path, "skipped: not a mapper file");
        return Err("not a mapper file".to_string());
    }

    if !result.errors.is_empty() {
        for err in &result.errors {
            crate::parse_log::warn(&file_path, &err.to_string());
        }
    }

    crate::parse_log::info(
        &file_path,
        &format!(
            "namespace={}, {} statements (structured)",
            result.namespace,
            result.statements.len()
        ),
    );

    Ok((result, content_hash))
}
```

**Step 2: Build and verify**

Run: `cargo build`
Expected: Compiles successfully

**Step 3: Commit**

```bash
git add src/parser/ibatis_loader.rs
git commit -m "feat: add structured ibatis loader using parse_mapper_bytes_structured"
```

---

## Task 3: Add `SqlFingerprintIndex` to `GraphStore`

**Files:**
- Modify: `src/graph/store.rs`

**Step 1: Add the fingerprint index field to `GraphStore` struct**

After the existing `edge_category_index` field (around line 47), add:

```rust
    /// Index: normalized SQL fingerprint → list of (NodeIndex, display_key)
    /// Built from expanded dynamic SQL variants for O(1) lookup.
    #[serde(default)]
    sql_fingerprint_index: HashMap<String, Vec<(NodeIndex, String)>>,
```

The `#[serde(default)]` attribute ensures backward compatibility: old cache files load with an empty `HashMap`.

**Step 2: Initialize in `new()` constructor**

Add to the `Self { ... }` block in `new()`:

```rust
sql_fingerprint_index: HashMap::new(),
```

**Step 3: Add accessor method**

```rust
    pub fn sql_fingerprint_index(&self) -> &HashMap<String, Vec<(NodeIndex, String)>> {
        &self.sql_fingerprint_index
    }
```

**Step 4: Build to verify struct compiles**

Run: `cargo build`
Expected: Compile errors for `from_graph()` and `rebuild_secondary_indexes()` missing the field — that's expected, fix in next steps.

**Step 5: Commit**

```bash
git add src/graph/store.rs
git commit -m "feat: add SqlFingerprintIndex field to GraphStore"
```

---

## Task 4: Build fingerprint index in `from_graph()` and `rebuild_secondary_indexes()`

**Files:**
- Modify: `src/graph/store.rs`

**Step 1: Add fingerprint extraction helper function**

Add a private helper near the other helper functions (after `normalize_for_matching`):

```rust
/// Generate a fingerprint from SQL text: normalize then hash.
fn sql_fingerprint(sql: &str) -> String {
    let normalized = normalize_for_matching(&sql.to_lowercase());
    blake3::hash(normalized.as_bytes()).to_hex().to_string()
}
```

**Step 2: Build fingerprint index in `from_graph()`**

After the `edge_category_index` building loop (around line 174), add:

```rust
        // Build sql_fingerprint_index
        let mut sql_fingerprint_index: HashMap<String, Vec<(NodeIndex, String)>> = HashMap::new();
        for idx in graph.node_indices() {
            match &graph[idx] {
                Node::MappedStatement {
                    sql: Some(sql_text),
                    namespace,
                    statement_id,
                    ..
                } => {
                    let fp = sql_fingerprint(sql_text);
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
                    ..
                } => {
                    let fp = sql_fingerprint(sql_text);
                    let ctx = match (class_name, method_name) {
                        (Some(c), Some(m)) => format!("{}.{}", c, m),
                        (Some(c), None) => c.clone(),
                        (None, Some(m)) => m.clone(),
                        (None, None) => "?".to_string(),
                    };
                    let display_key = format!("javasql:{}", ctx);
                    sql_fingerprint_index
                        .entry(fp)
                        .or_default()
                        .push((idx, display_key));
                }
                _ => {}
            }
        }
```

And include it in the `Self { ... }` return:

```rust
sql_fingerprint_index,
```

**Step 3: Add fingerprint rebuild to `rebuild_secondary_indexes()`**

At the end of `ensure_consistency_with_progress()` (after the edge_category_index rebuild, around line 303), add:

```rust
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
                    let fp = sql_fingerprint(sql_text);
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
                    ..
                } => {
                    let fp = sql_fingerprint(sql_text);
                    let ctx = match (class_name, method_name) {
                        (Some(c), Some(m)) => format!("{}.{}", c, m),
                        (Some(c), None) => c.clone(),
                        (None, Some(m)) => m.clone(),
                        (None, None) => "?".to_string(),
                    };
                    let display_key = format!("javasql:{}", ctx);
                    self.sql_fingerprint_index
                        .entry(fp)
                        .or_default()
                        .push((idx, display_key));
                }
                _ => {}
            }
        }
```

**Step 4: Build and run tests**

Run: `cargo test`
Expected: All existing tests pass (fingerprint index is additive, no behavior changes yet)

**Step 5: Commit**

```bash
git add src/graph/store.rs
git commit -m "feat: build SqlFingerprintIndex in from_graph and rebuild"
```

---

## Task 5: Fast-path `search_by_sql()` with fingerprint lookup

**Files:**
- Modify: `src/graph/store.rs`

**Step 1: Add fingerprint fast-path to `search_by_sql()`**

At the top of `search_by_sql()` (line 388), add fingerprint lookup before the O(n) scan:

```rust
    pub fn search_by_sql(&self, query: &str) -> Vec<(NodeIndex, String)> {
        // Fast path: O(1) fingerprint lookup
        let normalized = normalize_for_matching(&query.to_lowercase());
        let fp = blake3::hash(normalized.as_bytes()).to_hex().to_string();
        if let Some(hits) = self.sql_fingerprint_index.get(&fp) {
            if !hits.is_empty() {
                return hits.clone();
            }
        }

        // Slow path: existing PreparedQuery matching
        let prepared = PreparedQuery::new(query);
        // ... existing code unchanged ...
```

**Step 2: Run all existing tests**

Run: `cargo test --lib -- graph::store::tests`
Expected: All tests pass — fingerprint lookup should produce same or better results

**Step 3: Commit**

```bash
git add src/graph/store.rs
git commit -m "feat: add fingerprint fast-path to search_by_sql"
```

---

## Task 6: Wire dynamic variant expansion into graph builder

**Files:**
- Modify: `src/graph/builder.rs`

**Step 1: Expand dynamic SQL variants and generate fingerprint entries**

In the `add_ibatis_nodes_from_parsed()` method, after creating the `MappedStatement` node (around line 1500), add variant expansion for dynamic statements. This requires the builder to also receive structured ibatis data.

Add a new method `add_ibatis_structured_nodes()` that:
1. For each `StructuredStatement` with `has_dynamic_elements == true`:
   a. Call `stmt.expand_variants(&ExpandConfig { generate_parse_results: true, ..Default::default() })`
   b. For each `ExpandedVariant`, store the SQL as a fingerprint entry
   c. Extract CALL targets and table accesses from `variant.parse_result`
2. For static statements, fall through to existing `to_parsed_statement()` path

**Implementation approach:**

In `builder.rs`, modify `add_ibatis_nodes_from_parsed_with_source_paths()` to accept optional `structured_files`:

```rust
pub(crate) fn add_ibatis_nodes_from_parsed_with_source_paths(
    ibatis_files: &[crate::parser::ibatis_loader::IbatisParsedFile],
    structured_files: Option<&[crate::parser::ibatis_loader::IbatisStructuredFile]>,
    graph: &mut CodeGraph,
    // ... existing params
)
```

For each structured statement with dynamic elements, expand variants and:
- Store all variant SQLs in the `MappedStatement.sql` field (concatenated with separator, or store only `flat_sql` for display while fingerprints are computed separately)
- For each variant's `parse_result`, extract CALL targets and table accesses
- The fingerprint index is built later in `from_graph()` from the stored `sql` field

**Important design note:** The `MappedStatement.sql` field stores a single String. For dynamic SQL with multiple variants, we store the `flat_sql` (most complete variant) for display purposes. The fingerprint index is built from the graph nodes at `from_graph()` time. For dynamic SQL fingerprinting, we need to either:
- (A) Store variant SQLs as additional data in the graph node, OR
- (B) Re-expand at `from_graph()` time using the structured data

**Recommended approach (B):** Store the structured data alongside the graph during construction, then expand at `from_graph()` time. This avoids changing the `Node` enum.

**Step 2: Build and test**

Run: `cargo build && cargo test`
Expected: Compiles, all tests pass

**Step 3: Commit**

```bash
git add src/graph/builder.rs
git commit -m "feat: wire dynamic variant expansion into graph builder"
```

---

## Task 7: Add tests for fingerprint index

**Files:**
- Modify: `src/graph/store.rs` (tests module)

**Step 1: Add fingerprint-specific tests**

```rust
    #[test]
    fn fingerprint_index_built_for_mapper_sql() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao", "findById",
            Some("SELECT * FROM users WHERE id = __XML_PARAM_id__"),
        ));
        let store = GraphStore::from_graph("test", graph);

        // The fingerprint index should have at least one entry
        assert!(!store.sql_fingerprint_index.is_empty());

        // Searching by SQL should find it via fingerprint
        let results = store.search_by_sql("SELECT * FROM users WHERE id = ?");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn fingerprint_miss_falls_back_to_matching() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao", "findById",
            Some("SELECT * FROM users WHERE id = __XML_PARAM_id__ AND status = 'ACTIVE'"),
        ));
        let store = GraphStore::from_graph("test", graph);

        // Partial query won't match fingerprint, but should match via fallback
        let results = store.search_by_sql("from users where id");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn fingerprint_index_empty_for_old_cache() {
        // Simulate loading from old cache: fingerprint index is empty
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node("dao", "find", Some("SELECT 1")));
        let mut store = GraphStore::from_graph("test", graph);

        // Manually clear the index to simulate old cache
        store.sql_fingerprint_index.clear();

        // search_by_sql should still work via fallback
        let results = store.search_by_sql("select 1");
        assert_eq!(results.len(), 1);
    }
```

**Step 2: Run tests**

Run: `cargo test --lib -- graph::store::tests`
Expected: All new tests pass

**Step 3: Commit**

```bash
git add src/graph/store.rs
git commit -m "test: add SqlFingerprintIndex tests"
```

---

## Task 8: Run full verification suite

**Step 1: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: No warnings

**Step 2: Run format check**

Run: `cargo fmt -- --check`
Expected: No changes needed

**Step 3: Run all tests including serve feature**

Run: `cargo test && cargo test --features serve`
Expected: All pass

**Step 4: Final commit if any fixes needed**

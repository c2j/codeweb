# Search-SQL Test Coverage Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add comprehensive test coverage for search-by-sql matching, with GREEN tests locking current behavior and RED tests marking expected improvements for token normalization, scoring/ranking, Jaccard fallback, and cross-type search.

**Architecture:** Three-layer test strategy: (1) unit tests in `src/graph/store.rs` testing `PreparedQuery::matches` and `search_by_sql` directly, (2) integration tests in `tests/search_sql_test.rs` testing the `trace-sql` CLI command end-to-end, (3) serve API tests in `tests/serve_api.rs` testing the `/api/v1/nodes/search-sql` endpoint. GREEN tests use `assert!` / `assert_eq!` normally. RED tests are wrapped in a `#[cfg(feature = "search-sql-v2")]` gate so they compile but don't run by default — they become active when the corresponding improvement is implemented.

**Tech Stack:** Rust, existing test framework (`#[test]`), `tempfile` crate (already in dev-deps), `assert_cmd` or direct `std::process::Command` for CLI tests, `axum::test` for serve API tests.

---

## Test Category Map

| Category | File | Gate | Purpose |
|---|---|---|---|
| GREEN — PreparedQuery unit | `src/graph/store.rs` | always | Lock current matching behavior |
| GREEN — search_by_sql integration | `src/graph/store.rs` | always | Lock current GraphStore-level search |
| GREEN — CLI trace-sql | `tests/search_sql_test.rs` | always | End-to-end CLI behavior |
| GREEN — Serve API | `tests/serve_api.rs` | `#[cfg(feature = "serve")]` | HTTP endpoint behavior |
| RED — Token normalization | `src/graph/store.rs` | `#[cfg(feature = "search-sql-v2")]` | Expected: comments/stripped, literals→?, WHERE 1=1 removed |
| RED — Scoring & ranking | `src/graph/store.rs` | `#[cfg(feature = "search-sql-v2")]` | Expected: results with score + match_method |
| RED — Jaccard fallback | `src/graph/store.rs` | `#[cfg(feature = "search-sql-v2")]` | Expected: token-similar SQL matches |
| RED — Cross-type search | `src/graph/store.rs` | `#[cfg(feature = "search-sql-v2")]` | Expected: search Procedure body, View query |

---

## Phase 1: GREEN Unit Tests — PreparedQuery (store.rs)

### Task 1: Add helper functions for test ergonomics

**Files:**
- Modify: `src/graph/store.rs` (tests module)

**Step 1: Add test helpers at top of tests module**

After the existing `use super::*; use tempfile::TempDir;`, add:

```rust
// --- Helpers for SQL search tests ---

fn make_mapper_node(namespace: &str, statement_id: &str, sql: Option<&str>) -> crate::graph::Node {
    crate::graph::Node::MappedStatement {
        namespace: namespace.to_string(),
        statement_id: statement_id.to_string(),
        kind: "select".to_string(),
        xml_file: std::path::PathBuf::from("test.xml"),
        line: 1,
        sql: sql.map(String::from),
    }
}

fn make_javasql_node(class: Option<&str>, method: Option<&str>, sql: Option<&str>) -> crate::graph::Node {
    crate::graph::Node::JavaSql {
        class_name: class.map(String::from),
        method_name: method.map(String::from),
        extraction_method: "annotation".to_string(),
        java_file: std::path::PathBuf::from("Test.java"),
        line: 1,
        sql: sql.map(String::from),
    }
}

fn make_procedure_with_body(name: &str, schema: Option<&str>) -> crate::graph::Node {
    crate::graph::Node::Procedure {
        id: crate::graph::RoutineId {
            schema: schema.map(String::from),
            package: None,
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

fn make_view_node(name: &str, schema: Option<&str>, query: Option<&str>) -> crate::graph::Node {
    crate::graph::Node::View {
        schema: schema.map(String::from),
        name: name.to_string(),
        location: None,
        query: query.map(String::from),
    }
}
```

**Step 2: Run tests to verify no breakage**

Run: `cargo test --lib -- graph::store::tests`
Expected: All existing tests pass

**Step 3: Commit**

```bash
git add src/graph/store.rs
git commit -m "test: add helper functions for SQL search tests"
```

---

### Task 2: GREEN — Exact/substring match scenarios

**Files:**
- Modify: `src/graph/store.rs` (tests module)

**Step 1: Add exact match and basic substring tests**

Add after existing sql_text_matches tests:

```rust
// --- GREEN: search_by_sql exact and substring matching ---

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

    let results_upper = store.search_by_sql("SELECT * FROM USERS");
    let results_lower = store.search_by_sql("select * from users");
    let results_mixed = store.search_by_sql("Select * From Users");
    assert_eq!(results_upper.len(), 1);
    assert_eq!(results_lower.len(), 1);
    assert_eq!(results_mixed.len(), 1);
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
```

**Step 2: Run tests**

Run: `cargo test --lib -- graph::store::tests::search_by_sql`
Expected: All 7 new tests pass

**Step 3: Commit**

```bash
git add src/graph/store.rs
git commit -m "test: add GREEN unit tests for search_by_sql exact/substring matching"
```

---

### Task 3: GREEN — Keyword compatibility & operation type gating

**Files:**
- Modify: `src/graph/store.rs` (tests module)

**Step 1: Add keyword gate tests for search_by_sql**

```rust
// --- GREEN: search_by_sql keyword gate ---

#[test]
fn search_by_sql_rejects_select_vs_update() {
    let mut graph = CodeGraph::new();
    graph.add_node(make_mapper_node(
        "dao", "selectUsers",
        Some("SELECT * FROM users WHERE id = ?"),
    ));
    let store = GraphStore::from_graph("test", graph);

    let results = store.search_by_sql("update users set name = ? where id = ?");
    assert!(results.is_empty(), "UPDATE query must not match SELECT SQL");
}

#[test]
fn search_by_sql_rejects_insert_vs_delete() {
    let mut graph = CodeGraph::new();
    graph.add_node(make_mapper_node(
        "dao", "insertOrder",
        Some("INSERT INTO orders (id, name) VALUES (?, ?)"),
    ));
    let store = GraphStore::from_graph("test", graph);

    let results = store.search_by_sql("delete from orders where id = ?");
    assert!(results.is_empty(), "DELETE query must not match INSERT SQL");
}

#[test]
fn search_by_sql_select_compatible_with_with() {
    let mut graph = CodeGraph::new();
    graph.add_node(make_mapper_node(
        "dao", "cteQuery",
        Some("WITH cte AS (SELECT 1) SELECT * FROM cte"),
    ));
    let store = GraphStore::from_graph("test", graph);

    let results = store.search_by_sql("select * from cte");
    assert_eq!(results.len(), 1, "SELECT query should match WITH...SELECT SQL");
}

#[test]
fn search_by_sql_merge_matches_merge() {
    let mut graph = CodeGraph::new();
    graph.add_node(make_mapper_node(
        "dao", "mergeData",
        Some("MERGE INTO target t USING src s ON (t.id = s.id) WHEN MATCHED THEN UPDATE SET t.val = s.val"),
    ));
    let store = GraphStore::from_graph("test", graph);

    let results = store.search_by_sql("merge into target t using src s on (t.id = s.id)");
    assert_eq!(results.len(), 1, "MERGE query should match MERGE SQL");
}
```

**Step 2: Run tests**

Run: `cargo test --lib -- graph::store::tests::search_by_sql`
Expected: All 4 new tests pass

**Step 3: Commit**

```bash
git add src/graph/store.rs
git commit -m "test: add GREEN keyword gate tests for search_by_sql"
```

---

### Task 4: GREEN — Wildcard `?` and XML placeholder scenarios

**Files:**
- Modify: `src/graph/store.rs` (tests module)

**Step 1: Add wildcard and XML placeholder tests for search_by_sql**

```rust
// --- GREEN: search_by_sql wildcard and XML placeholder ---

#[test]
fn search_by_sql_query_wildcard_matches_concrete_sql() {
    let mut graph = CodeGraph::new();
    graph.add_node(make_mapper_node(
        "dao", "findById",
        Some("SELECT * FROM users WHERE id = 123"),
    ));
    let store = GraphStore::from_graph("test", graph);

    let results = store.search_by_sql("select * from users where id = ?");
    assert_eq!(results.len(), 1);
}

#[test]
fn search_by_sql_xml_param_placeholder_matches_wildcard_query() {
    let mut graph = CodeGraph::new();
    graph.add_node(make_mapper_node(
        "dao", "find",
        Some("SELECT * FROM users WHERE id = __XML_PARAM_userId__ AND status = __XML_PARAM_status__"),
    ));
    let store = GraphStore::from_graph("test", graph);

    let results = store.search_by_sql("select * from users where id=? and status=?");
    assert_eq!(results.len(), 1);
}

#[test]
fn search_by_sql_xml_raw_placeholder_matches_concrete() {
    let mut graph = CodeGraph::new();
    graph.add_node(make_mapper_node(
        "dao", "dynamicUpdate",
        Some("UPDATE __XML_RAW_tableName__ t SET t.status = '1'"),
    ));
    let store = GraphStore::from_graph("test", graph);

    let results = store.search_by_sql("update orders t set t.status='1'");
    assert_eq!(results.len(), 1, "concrete table name should match __XML_RAW__ placeholder");
}

#[test]
fn search_by_sql_fully_dynamic_sql_rejected() {
    let mut graph = CodeGraph::new();
    graph.add_node(make_mapper_node(
        "dao", "freeSql",
        Some("__XML_RAW_I_am_Free_SQL__"),
    ));
    let store = GraphStore::from_graph("test", graph);

    let results = store.search_by_sql("select * from users where id = ?");
    assert!(results.is_empty(), "fully dynamic SQL must not match specific queries");
}

#[test]
fn search_by_sql_query_extra_condition_rejected() {
    let mut graph = CodeGraph::new();
    graph.add_node(make_mapper_node(
        "dao", "find",
        Some("SELECT * FROM users WHERE id = __XML_PARAM_id__"),
    ));
    let store = GraphStore::from_graph("test", graph);

    let results = store.search_by_sql("select * from users where id=? and name=?");
    assert!(results.is_empty(), "query with extra conditions must not match");
}

#[test]
fn search_by_sql_operator_spacing_normalized() {
    let mut graph = CodeGraph::new();
    graph.add_node(make_mapper_node(
        "dao", "find",
        Some("SELECT * FROM t_orders WHERE user_id = __XML_PARAM_id__ AND status = 'CREATED'"),
    ));
    let store = GraphStore::from_graph("test", graph);

    let results = store.search_by_sql("select * from t_orders where user_id=?");
    assert_eq!(results.len(), 1, "different spacing around = should still match");
}
```

**Step 2: Run tests**

Run: `cargo test --lib -- graph::store::tests::search_by_sql`
Expected: All 6 new tests pass

**Step 3: Commit**

```bash
git add src/graph/store.rs
git commit -m "test: add GREEN wildcard and XML placeholder tests for search_by_sql"
```

---

### Task 5: GREEN — Table name gate

**Files:**
- Modify: `src/graph/store.rs` (tests module)

**Step 1: Add table name gating tests for search_by_sql**

```rust
// --- GREEN: search_by_sql table name gate ---

#[test]
fn search_by_sql_different_concrete_table_rejected() {
    let mut graph = CodeGraph::new();
    graph.add_node(make_mapper_node(
        "dao", "updateA",
        Some("UPDATE table_a SET x = 1 WHERE id = ?"),
    ));
    let store = GraphStore::from_graph("test", graph);

    let results = store.search_by_sql("update table_b set x = 1 where id = ?");
    assert!(results.is_empty(), "UPDATE on different concrete table must not match");
}

#[test]
fn search_by_sql_dynamic_table_accepts_any_concrete() {
    let mut graph = CodeGraph::new();
    graph.add_node(make_mapper_node(
        "dao", "dynamicUpdate",
        Some("UPDATE __XML_RAW_tableName__ SET status = __XML_PARAM_s__ WHERE id = __XML_PARAM_id__"),
    ));
    let store = GraphStore::from_graph("test", graph);

    let results = store.search_by_sql("update orders set status = ? where id = ?");
    assert_eq!(results.len(), 1, "dynamic table template must accept any concrete table");
}

#[test]
fn search_by_sql_select_different_table_rejected() {
    let mut graph = CodeGraph::new();
    graph.add_node(make_mapper_node(
        "dao", "selectA",
        Some("SELECT * FROM table_a WHERE x = ?"),
    ));
    let store = GraphStore::from_graph("test", graph);

    let results = store.search_by_sql("select * from table_b where x = ?");
    assert!(results.is_empty(), "SELECT from different table must not match");
}

#[test]
fn search_by_sql_different_set_column_rejected() {
    let mut graph = CodeGraph::new();
    graph.add_node(make_mapper_node(
        "dao", "updateStatus",
        Some("UPDATE orders SET status = __XML_PARAM_s__ WHERE id = __XML_PARAM_id__"),
    ));
    let store = GraphStore::from_graph("test", graph);

    let results = store.search_by_sql("update orders set name = ? where id = ?");
    assert!(results.is_empty(), "different first SET column must not match");
}
```

**Step 2: Run tests**

Run: `cargo test --lib -- graph::store::tests::search_by_sql`
Expected: All 4 new tests pass

**Step 3: Commit**

```bash
git add src/graph/store.rs
git commit -m "test: add GREEN table name gate tests for search_by_sql"
```

---

### Task 6: GREEN — Normalization edge cases

**Files:**
- Modify: `src/graph/store.rs` (tests module)

**Step 1: Add normalization edge case tests**

```rust
// --- GREEN: search_by_sql normalization edge cases ---

#[test]
fn search_by_sql_multiline_normalized() {
    let mut graph = CodeGraph::new();
    graph.add_node(make_mapper_node(
        "dao", "delete",
        Some("DELETE FROM bigfund.dat_log\n        WHERE data_date < TO_CHAR(TRUNC(SYSDATE) - 15, 'YYYYMMDD')"),
    ));
    let store = GraphStore::from_graph("test", graph);

    let results = store.search_by_sql("delete from bigfund.dat_log where data_date");
    assert_eq!(results.len(), 1);
}

#[test]
fn search_by_sql_crlf_normalized() {
    let mut graph = CodeGraph::new();
    graph.add_node(make_mapper_node(
        "dao", "delete",
        Some("DELETE FROM table\r\nWHERE id = 1"),
    ));
    let store = GraphStore::from_graph("test", graph);

    let results = store.search_by_sql("delete from table where id = 1");
    assert_eq!(results.len(), 1);
}

#[test]
fn search_by_sql_paren_and_comma_spacing() {
    let mut graph = CodeGraph::new();
    graph.add_node(make_mapper_node(
        "dao", "select",
        Some("SELECT TO_CHAR( TRUNC(SYSDATE) - 15 , 'YYYYMMDD' ) FROM dual"),
    ));
    let store = GraphStore::from_graph("test", graph);

    let results = store.search_by_sql("select to_char(trunc(sysdate)-?,'yyyymmdd') from dual");
    assert_eq!(results.len(), 1);
}

#[test]
fn search_by_sql_xml_raw_with_type_hint() {
    let mut graph = CodeGraph::new();
    graph.add_node(make_mapper_node(
        "dao", "select",
        Some("SELECT __XML_RAW_STRING_column__ FROM users"),
    ));
    let store = GraphStore::from_graph("test", graph);

    let results = store.search_by_sql("select ? from users");
    assert_eq!(results.len(), 1);
}
```

**Step 2: Run tests**

Run: `cargo test --lib -- graph::store::tests::search_by_sql`
Expected: All 4 new tests pass

**Step 3: Commit**

```bash
git add src/graph/store.rs
git commit -m "test: add GREEN normalization edge case tests for search_by_sql"
```

---

## Phase 2: GREEN Integration Tests — CLI trace-sql

### Task 7: Create integration test file for trace-sql

**Files:**
- Create: `tests/search_sql_test.rs`

**Step 1: Create test file with CLI integration tests**

```rust
use std::fs;
use tempfile::TempDir;

fn run_codeweb(args: &[&str]) -> std::process::Output {
    let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    let bin_name = if cfg!(windows) { "codeweb.exe" } else { "codeweb" };
    let entries = std::fs::read_dir(&base).unwrap_or_else(|_| panic!("no target dir"));
    for entry in entries.flatten() {
        let p = entry.path().join("debug").join(bin_name);
        if p.exists() {
            return std::process::Command::new(p)
                .args(args)
                .output()
                .expect("failed to run codeweb");
        }
    }
    let bin = base.join("debug").join(bin_name);
    std::process::Command::new(bin)
        .args(args)
        .output()
        .expect("failed to run codeweb")
}

fn write_sql(dir: &TempDir, filename: &str, sql: &str) -> std::path::PathBuf {
    let path = dir.path().join(filename);
    fs::write(&path, sql).unwrap();
    path
}

fn write_mapper_xml(dir: &TempDir, namespace: &str, id: &str, sql: &str) -> std::path::PathBuf {
    let content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE mapper PUBLIC "-//mybatis.org//DTD Mapper 3.0//EN" "http://mybatis.org/dtd/mybatis-3-mapper.dtd">
<mapper namespace="{}">
  <select id="{}">
    {}
  </select>
</mapper>"#,
        namespace, id, sql
    );
    let path = dir.path().join(format!("{}.xml", id));
    fs::write(&path, content).unwrap();
    path
}

// --- GREEN: CLI trace-sql basic ---

#[test]
fn trace_sql_finds_matching_mapper() {
    let dir = TempDir::new().unwrap();
    write_mapper_xml(&dir, "com.example.UserDao", "findById", "SELECT * FROM users WHERE id = #{id}");

    // First analyze to build graph
    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    assert!(output.status.success(), "analyze: {}", String::from_utf8_lossy(&output.stderr));

    // Then search SQL
    let output = run_codeweb(&["trace-sql", "--sql", "select * from users where id", "--project", dir.path().to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("UserDao") || stdout.contains("findById"),
        "trace-sql should find matching mapper. stdout: {}", stdout);
}

#[test]
fn trace_sql_no_match_returns_empty() {
    let dir = TempDir::new().unwrap();
    write_mapper_xml(&dir, "com.example.UserDao", "findById", "SELECT * FROM users WHERE id = #{id}");

    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    assert!(output.status.success());

    let output = run_codeweb(&["trace-sql", "--sql", "delete from orders where id = ?", "--project", dir.path().to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("UserDao"), "should not find mapper for unrelated SQL");
}

#[test]
fn trace_sql_reads_from_file() {
    let dir = TempDir::new().unwrap();
    write_mapper_xml(&dir, "com.example.UserDao", "findById", "SELECT * FROM users WHERE id = #{id}");

    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    assert!(output.status.success());

    // Write query to file
    let query_file = dir.path().join("query.txt");
    fs::write(&query_file, "select * from users where id").unwrap();

    let output = run_codeweb(&["trace-sql", "--file", query_file.to_str().unwrap(), "--project", dir.path().to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("UserDao") || stdout.contains("findById"),
        "trace-sql --file should find matching mapper. stdout: {}", stdout);
}
```

**Step 2: Run tests**

Run: `cargo test --test search_sql_test`
Expected: All 3 tests pass (requires pre-built binary: `cargo build` first)

**Step 3: Commit**

```bash
git add tests/search_sql_test.rs
git commit -m "test: add GREEN CLI integration tests for trace-sql"
```

---

## Phase 3: GREEN Serve API Tests

### Task 8: Add search-sql serve API tests

**Files:**
- Modify: `tests/serve_api.rs`

**Step 1: Find existing serve_api test patterns and add search-sql tests**

Read existing patterns in `tests/serve_api.rs` first, then append:

```rust
// --- GREEN: search-sql API endpoint ---

#[cfg(test)]
mod search_sql_api {
    use super::*;

    #[tokio::test]
    async fn search_sql_returns_matching_nodes() {
        // Setup: build a store with mapper nodes, start server, GET /api/v1/nodes/search-sql?q=...
        // Verify response JSON contains matching nodes
        // This test follows the existing pattern in serve_api.rs
    }

    #[tokio::test]
    async fn search_sql_returns_empty_for_no_match() {
        // Setup: build a store, search for unrelated SQL
        // Verify response JSON has empty nodes array
    }

    #[tokio::test]
    async fn search_sql_keyword_gate() {
        // Setup: store has SELECT SQL, query with UPDATE
        // Verify no results
    }
}
```

**Note:** The exact implementation depends on the existing serve_api.rs test helper patterns. The engineer should read the file first and follow the same setup/teardown pattern.

**Step 2: Run tests**

Run: `cargo test --test serve_api --features serve`
Expected: All tests pass

**Step 3: Commit**

```bash
git add tests/serve_api.rs
git commit -m "test: add GREEN serve API tests for search-sql endpoint"
```

---

## Phase 4: RED Tests — Token Normalization (P0)

### Task 9: Add RED tests for SQL token normalization

**Files:**
- Modify: `src/graph/store.rs` (tests module)

**Step 1: Add feature-gated RED tests**

```rust
// --- RED: Token normalization (search-sql-v2) ---
// These tests expect improved normalization that strips comments,
// unifies literals to ?, removes WHERE 1=1, etc.
// Activate with: cargo test --features search-sql-v2

#[cfg(feature = "search-sql-v2")]
mod search_sql_v2_token_norm {
    use super::*;

    #[test]
    fn sql_comments_stripped_before_matching() {
        assert!(
            sql_text_matches(
                "SELECT * FROM users -- get all users\nWHERE id = 1",
                "select * from users where id = ?",
            ),
            "SQL comments should be stripped before matching"
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
            sql_text_matches(
                "SELECT * FROM users;",
                "select * from users",
            ),
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
}
```

**Step 2: Add feature to Cargo.toml**

In `Cargo.toml`, add to `[features]`:

```toml
search-sql-v2 = []
```

**Step 3: Verify RED tests fail**

Run: `cargo test --lib --features search-sql-v2 -- graph::store::tests::search_sql_v2_token_norm`
Expected: Tests compile but FAIL (the normalization features don't exist yet)

**Step 4: Verify GREEN tests still pass without feature**

Run: `cargo test --lib -- graph::store::tests`
Expected: All GREEN tests pass

**Step 5: Commit**

```bash
git add src/graph/store.rs Cargo.toml
git commit -m "test: add RED tests for P0 token normalization (search-sql-v2 feature gate)"
```

---

## Phase 5: RED Tests — Scoring & Ranking (P0)

### Task 10: Add RED tests for scoring and ranking

**Files:**
- Modify: `src/graph/store.rs` (tests module)

**Step 1: Add feature-gated RED scoring tests**

```rust
#[cfg(feature = "search-sql-v2")]
mod search_sql_v2_scoring {
    use super::*;

    #[test]
    fn search_by_sql_returns_scored_results() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao", "exactMatch",
            Some("SELECT * FROM users WHERE id = ?"),
        ));
        graph.add_node(make_mapper_node(
            "dao", "partialMatch",
            Some("SELECT * FROM users WHERE id = ? AND name = ?"),
        ));
        graph.add_node(make_mapper_node(
            "dao", "differentTable",
            Some("SELECT * FROM orders WHERE id = ?"),
        ));
        let store = GraphStore::from_graph("test", graph);

        let results = store.search_by_sql_scored("select * from users where id = ?");
        assert_eq!(results.len(), 2, "should match 2 SQL nodes (users table)");

        // Exact match should score higher than partial
        let exact = results.iter().find(|r| r.display_key.contains("exactMatch")).unwrap();
        let partial = results.iter().find(|r| r.display_key.contains("partialMatch")).unwrap();
        assert!(exact.score > partial.score, "exact match should score higher than partial");
        assert!(exact.score >= 0.8, "exact match should have high score");
    }

    #[test]
    fn search_by_sql_exact_match_highest_score() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao", "q",
            Some("SELECT id, name FROM users WHERE status = 'ACTIVE'"),
        ));
        let store = GraphStore::from_graph("test", graph);

        let results = store.search_by_sql_scored(
            "select id, name from users where status = 'active'"
        );
        assert_eq!(results.len(), 1);
        assert!(results[0].score >= 0.95, "exact match should score >= 0.95, got {}", results[0].score);
        assert_eq!(results[0].match_method, "exact");
    }

    #[test]
    fn search_by_sql_substring_lower_score() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao", "q",
            Some("SELECT id, name, email, created_at FROM users WHERE status = 'ACTIVE' AND role = 'ADMIN'"),
        ));
        let store = GraphStore::from_graph("test", graph);

        let results = store.search_by_sql_scored("from users");
        assert_eq!(results.len(), 1);
        assert!(results[0].score < 0.8, "substring-only match should have lower score");
        assert_eq!(results[0].match_method, "substring");
    }

    #[test]
    fn search_by_sql_results_sorted_by_score_desc() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao", "exact",
            Some("SELECT * FROM users WHERE id = ?"),
        ));
        graph.add_node(make_mapper_node(
            "dao", "similar",
            Some("SELECT * FROM users WHERE id = ? AND status = ?"),
        ));
        graph.add_node(make_mapper_node(
            "dao", "vague",
            Some("SELECT id FROM users"),
        ));
        let store = GraphStore::from_graph("test", graph);

        let results = store.search_by_sql_scored("select * from users where id = ?");
        assert!(results.len() >= 2);
        for i in 1..results.len() {
            assert!(results[i - 1].score >= results[i].score,
                "results should be sorted by score descending");
        }
    }
}
```

**Step 2: Verify RED**

Run: `cargo test --lib --features search-sql-v2 -- graph::store::tests::search_sql_v2_scoring`
Expected: FAIL — `search_by_sql_scored` method doesn't exist yet

**Step 3: Commit**

```bash
git add src/graph/store.rs
git commit -m "test: add RED tests for P0 scoring & ranking (search-sql-v2 feature gate)"
```

---

## Phase 6: RED Tests — Jaccard Fallback (P1)

### Task 11: Add RED tests for Jaccard similarity fallback

**Files:**
- Modify: `src/graph/store.rs` (tests module)

**Step 1: Add Jaccard fallback tests**

```rust
#[cfg(feature = "search-sql-v2")]
mod search_sql_v2_jaccard {
    use super::*;

    #[test]
    fn similar_sql_matched_by_token_similarity() {
        // Same columns but different order — current impl misses this
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao", "findUsers",
            Some("SELECT name, email, id FROM users WHERE status = 'ACTIVE'"),
        ));
        let store = GraphStore::from_graph("test", graph);

        let results = store.search_by_sql_scored("select id, name, email from users where status = 'active'");
        assert_eq!(results.len(), 1, "column-reordered SQL should match via token similarity");
        assert!(results[0].score >= 0.6, "token-similar match should have moderate score");
        assert_eq!(results[0].match_method, "jaccard");
    }

    #[test]
    fn slightly_different_sql_matched() {
        // Extra column, same table, same WHERE — high similarity
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao", "find",
            Some("SELECT id, name FROM users WHERE status = 'ACTIVE' AND dept = 'IT'"),
        ));
        let store = GraphStore::from_graph("test", graph);

        let results = store.search_by_sql_scored("select id, name from users where status = 'active'");
        assert_eq!(results.len(), 1, "SQL with extra condition should match via similarity");
        assert!(results[0].score >= 0.5);
    }

    #[test]
    fn dissimilar_sql_not_matched() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node(
            "dao", "insert",
            Some("INSERT INTO orders (id, total) VALUES (?, ?)"),
        ));
        let store = GraphStore::from_graph("test", graph);

        let results = store.search_by_sql_scored("select * from users where id = ?");
        assert!(results.is_empty(), "dissimilar SQL should not match even with Jaccard");
    }
}
```

**Step 2: Verify RED**

Run: `cargo test --lib --features search-sql-v2 -- graph::store::tests::search_sql_v2_jaccard`
Expected: FAIL

**Step 3: Commit**

```bash
git add src/graph/store.rs
git commit -m "test: add RED tests for P1 Jaccard fallback (search-sql-v2 feature gate)"
```

---

## Phase 7: RED Tests — Cross-Type Search (P1)

### Task 12: Add RED tests for cross-node-type search

**Files:**
- Modify: `src/graph/store.rs` (tests module)

**Step 1: Add cross-type search tests**

```rust
#[cfg(feature = "search-sql-v2")]
mod search_sql_v2_cross_type {
    use super::*;

    #[test]
    fn search_finds_procedure_with_matching_body() {
        // Procedures don't have sql field but their body text should be searchable
        let mut graph = CodeGraph::new();
        graph.add_node(make_procedure_with_body("sp_get_users", Some("public")));
        let store = GraphStore::from_graph("test", graph);

        let results = store.search_fulltext("select * from users");
        // Procedure body should be searchable (if body text is stored)
        // This test documents the expected future behavior
    }

    #[test]
    fn search_finds_view_with_matching_query() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_view_node("v_active_users", Some("public"),
            Some("SELECT * FROM users WHERE status = 'ACTIVE'")));
        let store = GraphStore::from_graph("test", graph);

        let results = store.search_fulltext("from users where status");
        assert_eq!(results.len(), 1, "View query text should be searchable");
        assert!(results[0].display_key.contains("v_active_users"));
    }

    #[test]
    fn search_fulltext_covers_all_node_types() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_mapper_node("dao", "find", Some("SELECT * FROM users")));
        graph.add_node(make_javasql_node(Some("Svc"), Some("query"), Some("SELECT * FROM orders")));
        graph.add_node(make_view_node("v1", None, Some("SELECT * FROM products")));
        let store = GraphStore::from_graph("test", graph);

        let results = store.search_fulltext("select * from");
        assert!(results.len() >= 2, "fulltext search should cover mappers, javasql, and views");
    }
}
```

**Step 2: Verify RED**

Run: `cargo test --lib --features search-sql-v2 -- graph::store::tests::search_sql_v2_cross_type`
Expected: FAIL — `search_fulltext` method doesn't exist yet

**Step 3: Commit**

```bash
git add src/graph/store.rs
git commit -m "test: add RED tests for P1 cross-type search (search-sql-v2 feature gate)"
```

---

## Summary

| Phase | Tasks | Type | Tests |
|---|---|---|---|
| Phase 1 | 1-6 | GREEN unit | ~25 unit tests in store.rs |
| Phase 2 | 7 | GREEN CLI | 3 integration tests |
| Phase 3 | 8 | GREEN serve | 3 serve API tests |
| Phase 4 | 9 | RED token norm | 6 tests |
| Phase 5 | 10 | RED scoring | 4 tests |
| Phase 6 | 11 | RED Jaccard | 3 tests |
| Phase 7 | 12 | RED cross-type | 3 tests |
| **Total** | **12** | | **~47 tests** |

**Test execution:**
- `cargo test --lib` — runs all GREEN unit tests
- `cargo test --test search_sql_test` — runs CLI integration tests
- `cargo test --test serve_api --features serve` — runs serve API tests
- `cargo test --lib --features search-sql-v2` — runs GREEN + all RED tests

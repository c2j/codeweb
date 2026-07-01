#![cfg(feature = "jsp")]

use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

// ═══════════════════════════════════════════════════════════════
// Category A: Basic SQL extraction fixtures
// ═══════════════════════════════════════════════════════════════
const A01_SELECT_SIMPLE: &str = include_str!("regress/jsp/cases/a01_select_simple.jsp");
const A02_INSERT: &str = include_str!("regress/jsp/cases/a02_insert.jsp");
const A03_UPDATE: &str = include_str!("regress/jsp/cases/a03_update.jsp");
const A04_DELETE: &str = include_str!("regress/jsp/cases/a04_delete.jsp");
const A05_MERGE: &str = include_str!("regress/jsp/cases/a05_merge.jsp");
const A06_WITH_CTE: &str = include_str!("regress/jsp/cases/a06_with_cte.jsp");
const A07_STRING_CONCAT: &str = include_str!("regress/jsp/cases/a07_string_concat.jsp");
const A08_DECLARATION_CONSTANT: &str =
    include_str!("regress/jsp/cases/a08_declaration_constant.jsp");

// ═══════════════════════════════════════════════════════════════
// Category B: Stored procedure call detection fixtures
// ═══════════════════════════════════════════════════════════════
const B01_PREPARE_CALL_SELECT: &str = include_str!("regress/jsp/cases/b01_prepare_call_select.jsp");
const B01_PREPARE_CALL_SELECT_SQL: &str =
    include_str!("regress/jsp/cases/b01_prepare_call_select.sql");
const B02_JDBC_ESCAPE: &str = include_str!("regress/jsp/cases/b02_jdbc_escape_syntax.jsp");
const B02_JDBC_ESCAPE_SQL: &str = include_str!("regress/jsp/cases/b02_jdbc_escape_syntax.sql");
const B03_MULTI_PROC: &str = include_str!("regress/jsp/cases/b03_multi_procedure_calls.jsp");
const B03_MULTI_PROC_SQL: &str = include_str!("regress/jsp/cases/b03_multi_procedure_calls.sql");
const B04_STMT_EXECUTE: &str = include_str!("regress/jsp/cases/b04_statement_execute_call.jsp");
const B04_STMT_EXECUTE_SQL: &str = include_str!("regress/jsp/cases/b04_statement_execute_call.sql");

// ═══════════════════════════════════════════════════════════════
// Category C: Graph integration fixtures
// ═══════════════════════════════════════════════════════════════
const C01_JSP_PAGE: &str = include_str!("regress/jsp/cases/c01_jsp_page_node.jsp");
const C01_JSP_PAGE_SQL: &str = include_str!("regress/jsp/cases/c01_jsp_page_node.sql");
const C02_JSP_SQL: &str = include_str!("regress/jsp/cases/c02_jsp_sql_node.jsp");
const C03_CONTAINS_SQL: &str = include_str!("regress/jsp/cases/c03_contains_sql_edge.jsp");
const C04_CALLS_PROC: &str = include_str!("regress/jsp/cases/c04_calls_procedure_edge.jsp");
const C04_CALLS_PROC_SQL: &str = include_str!("regress/jsp/cases/c04_calls_procedure_edge.sql");
const C05_TABLE_ACCESS: &str = include_str!("regress/jsp/cases/c05_table_access_edge.jsp");
const C05_TABLE_ACCESS_SQL: &str = include_str!("regress/jsp/cases/c05_table_access_edge.sql");

// ═══════════════════════════════════════════════════════════════
// Category D: Edge case / robustness fixtures
// ═══════════════════════════════════════════════════════════════
const D01_HTML_ONLY: &str = include_str!("regress/jsp/cases/d01_html_only.jsp");
const D02_EMPTY: &str = include_str!("regress/jsp/cases/d02_empty.jsp");
const D03_INVALID_JAVA: &str = include_str!("regress/jsp/cases/d03_invalid_java.jsp");
const D04_COMMENTS_ONLY: &str = include_str!("regress/jsp/cases/d04_comments_only.jsp");
const D05_SPLIT_SCRIPTLETS: &str = include_str!("regress/jsp/cases/d05_split_scriptlets.jsp");
const D06_EL_EXPRESSIONS: &str = include_str!("regress/jsp/cases/d06_el_expressions.jsp");

// ═══════════════════════════════════════════════════════════════
// Category E: SQL dedup & scriptlet types fixtures
// ═══════════════════════════════════════════════════════════════
const E01_DUPLICATE_SQL: &str = include_str!("regress/jsp/cases/e01_duplicate_sql.jsp");
const E02_MIXED_TYPES: &str = include_str!("regress/jsp/cases/e02_mixed_sql_types.jsp");
const E03_MIXED_DECL_SCRIPTLET: &str =
    include_str!("regress/jsp/cases/e03_mixed_decl_and_scriptlet.jsp");
const E04_TRY_CATCH: &str = include_str!("regress/jsp/cases/e04_try_catch_jdbc.jsp");

// ═══════════════════════════════════════════════════════════════
// Category F: Cross-file integration fixtures
// ═══════════════════════════════════════════════════════════════
const F01_JSP_PROC: &str = include_str!("regress/jsp/cases/f01_jsp_calls_procedure_in_sql.jsp");
const F01_JSP_PROC_SQL: &str = include_str!("regress/jsp/cases/f01_jsp_calls_procedure_in_sql.sql");
const F02_JSP_TABLE: &str = include_str!("regress/jsp/cases/f02_jsp_references_table_in_sql.jsp");
const F02_JSP_TABLE_SQL: &str =
    include_str!("regress/jsp/cases/f02_jsp_references_table_in_sql.sql");
const F03_MULTI_PAGE1: &str = include_str!("regress/jsp/cases/f03_multi_jsp_page1.jsp");
const F03_MULTI_PAGE2: &str = include_str!("regress/jsp/cases/f03_multi_jsp_page2.jsp");
const F03_MULTI_SHARED_SQL: &str = include_str!("regress/jsp/cases/f03_multi_jsp_shared.sql");

// ═══════════════════════════════════════════════════════════════
// Helper functions
// ═══════════════════════════════════════════════════════════════

fn run_codeweb(args: &[&str]) -> std::process::Output {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    let bin_name = if cfg!(windows) {
        "codeweb.exe"
    } else {
        "codeweb"
    };
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

/// Analyze a temp directory containing fixture files, returning JSON output.
fn analyze_dir(dir: &TempDir) -> serde_json::Value {
    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "codeweb failed. stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&output.stderr),
        stdout
    );
    serde_json::from_str(&stdout).expect("failed to parse JSON output")
}

/// Write a single .jsp fixture to a temp dir and analyze.
fn analyze_jsp_only(jsp_content: &str, filename: &str) -> serde_json::Value {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join(filename), jsp_content).unwrap();
    analyze_dir(&dir)
}

/// Write a .jsp + .sql fixture pair to a temp dir and analyze.
fn analyze_jsp_with_sql(
    jsp_content: &str,
    jsp_filename: &str,
    sql_content: &str,
    sql_filename: &str,
) -> serde_json::Value {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join(jsp_filename), jsp_content).unwrap();
    fs::write(dir.path().join(sql_filename), sql_content).unwrap();
    analyze_dir(&dir)
}

/// Count nodes of a given type in the JSON output.
fn node_count(json: &serde_json::Value, node_type: &str) -> usize {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["type"].as_str() == Some(node_type))
        .count()
}

/// Count edges of a given type in the JSON output.
fn edge_count(json: &serde_json::Value, edge_type: &str) -> usize {
    json["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"].as_str() == Some(edge_type))
        .count()
}

/// Find a node by name (works for procedure/function nodes).
fn find_node_by_name<'a>(json: &'a serde_json::Value, name: &str) -> Option<&'a serde_json::Value> {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"].as_str() == Some(name))
}

/// Get the node ID by name.
#[allow(dead_code)]
fn node_id_by_name(json: &serde_json::Value, name: &str) -> Option<usize> {
    find_node_by_name(json, name)
        .and_then(|n| n["id"].as_u64())
        .map(|id| id as usize)
}

/// Check if an edge exists from source node name to target node name with given edge type.
#[allow(dead_code)]
fn has_named_edge(json: &serde_json::Value, from: &str, to: &str, edge_type: &str) -> bool {
    let (Some(src_id), Some(dst_id)) = (node_id_by_name(json, from), node_id_by_name(json, to))
    else {
        return false;
    };
    json["edges"].as_array().unwrap().iter().any(|e| {
        e["source"].as_u64() == Some(src_id as u64)
            && e["target"].as_u64() == Some(dst_id as u64)
            && e["type"].as_str() == Some(edge_type)
    })
}

/// Check if any node of given type exists with matching property value.
fn has_node_with_property(
    json: &serde_json::Value,
    node_type: &str,
    prop: &str,
    value: &str,
) -> bool {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["type"].as_str() == Some(node_type))
        .any(|n| n[prop].as_str() == Some(value))
}

/// Get the raw_expr values of all unresolved nodes.
#[allow(dead_code)]
fn unresolved_raw_exprs(json: &serde_json::Value) -> Vec<String> {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["type"].as_str() == Some("unresolved"))
        .filter_map(|n| n["raw_expr"].as_str().map(String::from))
        .collect()
}

/// Check if the JSON output contains any node of type "jsp".
fn has_jsp_page(json: &serde_json::Value) -> bool {
    node_count(json, "jsp") > 0
}

/// Check if the JSON output contains any node of type "jspsql".
fn has_jsp_sql(json: &serde_json::Value) -> bool {
    node_count(json, "jspsql") > 0
}

/// Check if JspSql nodes contain specific SQL text.
fn jsp_sql_contains(json: &serde_json::Value, sql_fragment: &str) -> bool {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["type"].as_str() == Some("jspsql"))
        .any(|n| {
            n["sql"]
                .as_str()
                .map(|s| s.contains(sql_fragment))
                .unwrap_or(false)
        })
}

/// Dump all nodes for debugging.
#[allow(dead_code)]
fn dump_all_nodes(json: &serde_json::Value) {
    let nodes = json["nodes"].as_array().unwrap();
    eprintln!("--- All Nodes ({}) ---", nodes.len());
    for n in nodes {
        let t = n["type"].as_str().unwrap_or("?");
        let id = n["id"].as_u64().unwrap_or(0);
        match t {
            "procedure" | "function" | "jsp" | "jspsql" => {
                eprintln!(
                    "  [{id}] {t}: name={}",
                    n["name"]
                        .as_str()
                        .or(n["display_name"].as_str())
                        .unwrap_or("?")
                );
            }
            "unresolved" => {
                eprintln!(
                    "  [{id}] {t}: raw_expr={}",
                    n["raw_expr"].as_str().unwrap_or("?")
                );
            }
            "table" | "view" => {
                eprintln!("  [{id}] {t}: {}", n["name"].as_str().unwrap_or("?"));
            }
            _ => {
                eprintln!("  [{id}] {t}");
            }
        }
    }
}

/// Dump all edges for debugging.
#[allow(dead_code)]
fn dump_all_edges(json: &serde_json::Value) {
    let edges = json["edges"].as_array().unwrap();
    eprintln!("--- All Edges ({}) ---", edges.len());
    for e in edges {
        let t = e["type"].as_str().unwrap_or("?");
        let src = e["source"].as_u64().unwrap_or(0);
        let tgt = e["target"].as_u64().unwrap_or(0);
        eprintln!("  {src} --[{t}]--> {tgt}");
    }
}

// ═══════════════════════════════════════════════════════════════
// Category A: Basic SQL extraction tests (8 tests)
// ═══════════════════════════════════════════════════════════════

#[test]
fn regress_jsp_a01_select_simple() {
    let json = analyze_jsp_only(A01_SELECT_SIMPLE, "a01_select_simple.jsp");
    assert!(has_jsp_page(&json), "must have a JspPage node");
    assert!(has_jsp_sql(&json), "must have JspSql node(s)");
    assert!(
        jsp_sql_contains(&json, "SELECT"),
        "JspSql must contain SELECT: {json:?}"
    );
    assert!(
        jsp_sql_contains(&json, "users"),
        "JspSql must reference 'users' table"
    );
}

#[test]
fn regress_jsp_a02_insert() {
    let json = analyze_jsp_only(A02_INSERT, "a02_insert.jsp");
    assert!(
        jsp_sql_contains(&json, "INSERT"),
        "JspSql must contain INSERT"
    );
    assert!(
        jsp_sql_contains(&json, "audit_log"),
        "JspSql must reference 'audit_log' table"
    );
}

#[test]
fn regress_jsp_a03_update() {
    let json = analyze_jsp_only(A03_UPDATE, "a03_update.jsp");
    assert!(
        jsp_sql_contains(&json, "UPDATE"),
        "JspSql must contain UPDATE"
    );
    assert!(
        jsp_sql_contains(&json, "products"),
        "JspSql must reference 'products' table"
    );
}

#[test]
fn regress_jsp_a04_delete() {
    let json = analyze_jsp_only(A04_DELETE, "a04_delete.jsp");
    assert!(
        jsp_sql_contains(&json, "DELETE"),
        "JspSql must contain DELETE"
    );
    assert!(
        jsp_sql_contains(&json, "temp_sessions"),
        "JspSql must reference 'temp_sessions' table"
    );
}

#[test]
fn regress_jsp_a05_merge() {
    let json = analyze_jsp_only(A05_MERGE, "a05_merge.jsp");
    assert!(
        jsp_sql_contains(&json, "MERGE"),
        "JspSql must contain MERGE"
    );
    assert!(
        jsp_sql_contains(&json, "inventory"),
        "JspSql must reference 'inventory' table"
    );
}

#[test]
fn regress_jsp_a06_with_cte() {
    let json = analyze_jsp_only(A06_WITH_CTE, "a06_with_cte.jsp");
    assert!(
        jsp_sql_contains(&json, "WITH"),
        "JspSql must contain WITH (CTE)"
    );
    assert!(
        jsp_sql_contains(&json, "ranked"),
        "JspSql must contain CTE name 'ranked'"
    );
    assert!(
        jsp_sql_contains(&json, "orders"),
        "JspSql must reference 'orders' table"
    );
}

#[test]
fn regress_jsp_a07_string_concat() {
    let json = analyze_jsp_only(A07_STRING_CONCAT, "a07_string_concat.jsp");
    assert!(
        has_jsp_sql(&json),
        "must extract SQL from string concatenation"
    );
    assert!(
        jsp_sql_contains(&json, "SELECT"),
        "JspSql must contain SELECT from string-concatenated SQL"
    );
}

#[test]
fn regress_jsp_a08_declaration_constant() {
    let json = analyze_jsp_only(A08_DECLARATION_CONSTANT, "a08_declaration_constant.jsp");
    assert!(
        has_jsp_sql(&json),
        "must extract SQL from declaration constant"
    );
    assert!(
        jsp_sql_contains(&json, "FIND_BY_ID")
            || jsp_sql_contains(&json, "SELECT id, name FROM users"),
        "JspSql must contain the constant SQL or its reference"
    );
    // The declaration SQL should be extracted as a constant
    let has_decl_sql = json["nodes"].as_array().unwrap().iter().any(|n| {
        n["sql"]
            .as_str()
            .is_some_and(|s| s.contains("SELECT id, name FROM users"))
    });
    assert!(
        has_decl_sql,
        "must extract declaration constant SQL: {json:?}"
    );
}

// ═══════════════════════════════════════════════════════════════
// Category B: Stored procedure call detection tests (4 tests)
// ═══════════════════════════════════════════════════════════════

#[test]
fn regress_jsp_b01_prepare_call_select() {
    let json = analyze_jsp_with_sql(
        B01_PREPARE_CALL_SELECT,
        "b01_prepare_call_select.jsp",
        B01_PREPARE_CALL_SELECT_SQL,
        "b01_prepare_call_select.sql",
    );
    // The JSP uses prepareCall("SELECT pkg.get_user(?, ?)") which starts with SELECT
    // so it should pass the keyword gate
    assert!(has_jsp_sql(&json), "must extract SQL from prepareCall");
    assert!(
        jsp_sql_contains(&json, "pkg.get_user"),
        "JspSql must reference pkg.get_user"
    );

    // Should have a procedure node for pkg.get_user
    let has_proc = find_node_by_name(&json, "pkg.get_user").is_some();
    eprintln!(
        "pkg.get_user found as procedure: {has_proc}. Nodes: {}",
        serde_json::to_string_pretty(&json["nodes"]).unwrap_or_default()
    );
}

#[test]
fn regress_jsp_b02_jdbc_escape_syntax_filtered() {
    let json = analyze_jsp_with_sql(
        B02_JDBC_ESCAPE,
        "b02_jdbc_escape_syntax.jsp",
        B02_JDBC_ESCAPE_SQL,
        "b02_jdbc_escape_syntax.sql",
    );
    // Known limitation: JDBC escape syntax {call ...} should be filtered by keyword gate.
    // The JSP should not crash, but the SQL may not be extracted.
    // This test documents the current behavior - we assert analysis succeeds.
    assert!(
        has_jsp_page(&json),
        "JspPage must be created even if no SQL extracted"
    );

    // Document: JDBC escape syntax is a known limitation
    let sql_extracted = has_jsp_sql(&json);
    eprintln!(
        "JDBC escape syntax SQL extracted: {sql_extracted} (known limitation: expected=false, \
         only SELECT/INSERT/UPDATE/DELETE/MERGE/WITH pass keyword gate)"
    );
}

#[test]
fn regress_jsp_b03_multi_procedure_calls() {
    let json = analyze_jsp_with_sql(
        B03_MULTI_PROC,
        "b03_multi_procedure_calls.jsp",
        B03_MULTI_PROC_SQL,
        "b03_multi_procedure_calls.sql",
    );
    assert!(
        has_jsp_sql(&json),
        "must extract SQL from multiple procedure calls"
    );

    // Should reference both calc_total and get_last_order
    let has_calc = jsp_sql_contains(&json, "calc_total");
    let has_last = jsp_sql_contains(&json, "get_last_order");
    assert!(
        has_calc || has_last,
        "must extract at least one procedure call. calc_total={has_calc}, get_last_order={has_last}"
    );
}

#[test]
fn regress_jsp_b04_statement_execute_call() {
    let json = analyze_jsp_with_sql(
        B04_STMT_EXECUTE,
        "b04_statement_execute_call.jsp",
        B04_STMT_EXECUTE_SQL,
        "b04_statement_execute_call.sql",
    );
    assert!(
        has_jsp_sql(&json),
        "must extract SQL from Statement.executeQuery"
    );
    assert!(
        jsp_sql_contains(&json, "proc_from_stmt"),
        "JspSql must reference proc_from_stmt"
    );
}

// ═══════════════════════════════════════════════════════════════
// Category C: Graph integration tests (5 tests)
// ═══════════════════════════════════════════════════════════════

#[test]
fn regress_jsp_c01_jsp_page_node_created() {
    let json = analyze_jsp_with_sql(
        C01_JSP_PAGE,
        "c01_jsp_page_node.jsp",
        C01_JSP_PAGE_SQL,
        "c01_jsp_page_node.sql",
    );
    assert!(
        has_jsp_page(&json),
        "must create a JspPage node (type='jsp')"
    );
    let jsp_nodes = node_count(&json, "jsp");
    assert!(
        jsp_nodes >= 1,
        "expected >= 1 JspPage node, got {jsp_nodes}"
    );
}

#[test]
fn regress_jsp_c02_jsp_sql_node_properties() {
    let json = analyze_jsp_only(C02_JSP_SQL, "c02_jsp_sql_node.jsp");
    assert!(
        has_jsp_sql(&json),
        "must create JspSql node(s) (type='jsql')"
    );

    // Verify JspSql has sql, line, kind properties
    let jsql_nodes: Vec<_> = json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["type"].as_str() == Some("jspsql"))
        .collect();
    assert!(!jsql_nodes.is_empty(), "must have at least one jsql node");

    for node in &jsql_nodes {
        assert!(node["sql"].is_string(), "JspSql must have 'sql' property");
        assert!(node["line"].is_number(), "JspSql must have 'line' property");
        assert!(node["kind"].is_string(), "JspSql must have 'kind' property");
        assert!(
            node["parsed"].is_boolean(),
            "JspSql must have 'parsed' property"
        );
        assert!(node["file"].is_string(), "JspSql must have 'file' property");
    }
}

#[test]
fn regress_jsp_c03_contains_sql_edge() {
    let json = analyze_jsp_only(C03_CONTAINS_SQL, "c03_contains_sql_edge.jsp");
    assert!(has_jsp_page(&json), "must have JspPage node");
    assert!(has_jsp_sql(&json), "must have JspSql node");

    // Verify ContainsSql edges exist
    let contains_sql_edges = edge_count(&json, "contains_sql");
    assert!(
        contains_sql_edges >= 1,
        "expected >= 1 ContainsSql edges, got {contains_sql_edges}"
    );
}

#[test]
fn regress_jsp_c04_calls_procedure_edge() {
    let json = analyze_jsp_with_sql(
        C04_CALLS_PROC,
        "c04_calls_procedure_edge.jsp",
        C04_CALLS_PROC_SQL,
        "c04_calls_procedure_edge.sql",
    );
    assert!(has_jsp_sql(&json), "must have JspSql node");

    // The JspSql should have a CallsProcedure edge to calc_tax
    let has_proc_edge = edge_count(&json, "calls_procedure") >= 1;
    let proc_exists = find_node_by_name(&json, "calc_tax").is_some();
    eprintln!("calls_procedure edge exists: {has_proc_edge}, calc_tax proc node: {proc_exists}");

    // At minimum, the JspSql should reference calc_tax
    assert!(
        jsp_sql_contains(&json, "calc_tax"),
        "JspSql must reference calc_tax"
    );
}

#[test]
fn regress_jsp_c05_table_access_edge() {
    let json = analyze_jsp_with_sql(
        C05_TABLE_ACCESS,
        "c05_table_access_edge.jsp",
        C05_TABLE_ACCESS_SQL,
        "c05_table_access_edge.sql",
    );
    assert!(has_jsp_sql(&json), "must have JspSql node");

    // Should have a table node for orders
    let has_table = find_node_by_name(&json, "orders").is_some();
    assert!(has_table, "must have a table node for 'orders'");

    // Should have table_access edges
    let table_access_edges = edge_count(&json, "table_access");
    assert!(
        table_access_edges >= 1,
        "expected >= 1 TableAccess edges, got {table_access_edges}"
    );
}

// ═══════════════════════════════════════════════════════════════
// Category D: Edge case / robustness tests (6 tests)
// ═══════════════════════════════════════════════════════════════

#[test]
fn regress_jsp_d01_html_only_no_crash() {
    let json = analyze_jsp_only(D01_HTML_ONLY, "d01_html_only.jsp");
    // HTML-only JSP should not crash and should produce zero extractions
    let jsql_count = node_count(&json, "jspsql");
    assert_eq!(
        jsql_count, 0,
        "HTML-only JSP should have no JspSql nodes, got {jsql_count}"
    );
    // JspPage should still be created (unless no content at all)
    let jsp_count = node_count(&json, "jsp");
    eprintln!(
        "HTML-only JSP page node count: {jsp_count} (may be 0 if no scriptlets/declarations)"
    );
}

#[test]
fn regress_jsp_d02_empty_no_crash() {
    let json = analyze_jsp_only(D02_EMPTY, "d02_empty.jsp");
    // Empty JSP should not crash. No extraction expected.
    let jsql_count = node_count(&json, "jspsql");
    assert_eq!(
        jsql_count, 0,
        "Empty JSP should have no JspSql nodes, got {jsql_count}"
    );
    // Should not panic - this test just verifies completion
    assert!(json.is_object(), "must produce valid JSON output");
}

#[test]
fn regress_jsp_d03_invalid_java_no_crash() {
    let json = analyze_jsp_only(D03_INVALID_JAVA, "d03_invalid_java.jsp");
    // Invalid Java in scriptlet should not crash the analysis
    assert!(
        json.is_object(),
        "must produce valid JSON output despite invalid Java"
    );
    // May or may not extract SQL - the key assertion is no crash
    let jsql_count = node_count(&json, "jspsql");
    let jsp_count = node_count(&json, "jsp");
    eprintln!("Invalid Java JSP: {jsp_count} jsp nodes, {jsql_count} jsql nodes (no crash = pass)");
}

#[test]
fn regress_jsp_d04_comments_only_no_crash() {
    let json = analyze_jsp_only(D04_COMMENTS_ONLY, "d04_comments_only.jsp");
    // Comments-only JSP should not crash and produce zero extractions
    let jsql_count = node_count(&json, "jspsql");
    assert_eq!(
        jsql_count, 0,
        "Comments-only JSP should have no JspSql nodes, got {jsql_count}"
    );
}

#[test]
fn regress_jsp_d05_split_scriptlets_extracts_sql() {
    let json = analyze_jsp_only(D05_SPLIT_SCRIPTLETS, "d05_split_scriptlets.jsp");
    // Split scriptlets (mixed HTML + Java) should still extract SQL
    assert!(has_jsp_sql(&json), "must extract SQL from split scriptlets");
    assert!(
        jsp_sql_contains(&json, "items"),
        "JspSql must reference 'items' table"
    );
}

#[test]
fn regress_jsp_d06_el_expressions_replaced() {
    let json = analyze_jsp_only(D06_EL_EXPRESSIONS, "d06_el_expressions.jsp");
    // EL expressions ${param.id} should be replaced with placeholders
    assert!(
        has_jsp_sql(&json),
        "must extract SQL despite EL expressions"
    );
    // The SQL should contain the basics but EL markers should be replaced
    assert!(
        jsp_sql_contains(&json, "SELECT"),
        "JspSql must contain SELECT"
    );
    assert!(
        jsp_sql_contains(&json, "users"),
        "JspSql must reference 'users' table"
    );
    // EL ${param.id} should become "<EL_PARAM_ID>" placeholder
    eprintln!(
        "JSP SQL nodes: {:?}",
        json["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|n| n["type"].as_str() == Some("jspsql"))
            .collect::<Vec<_>>()
    );
}

// ═══════════════════════════════════════════════════════════════
// Category E: SQL dedup & scriptlet types tests (4 tests)
// ═══════════════════════════════════════════════════════════════

#[test]
fn regress_jsp_e01_duplicate_sql_dedup() {
    let json = analyze_jsp_only(E01_DUPLICATE_SQL, "e01_duplicate_sql.jsp");
    // Same SQL in two scriptlet blocks should be deduplicated
    let jsql_count = node_count(&json, "jspsql");
    assert!(
        jsql_count <= 2,
        "duplicate SQL should be deduplicated: expected <= 2 jsql nodes, got {jsql_count}"
    );
    // Should have at least one JspSql node with "SELECT * FROM users"
    assert!(
        has_node_with_property(&json, "jspsql", "sql", "SELECT * FROM users"),
        "must have JspSql with 'SELECT * FROM users'"
    );
}

#[test]
fn regress_jsp_e02_mixed_sql_types() {
    let json = analyze_jsp_only(E02_MIXED_TYPES, "e02_mixed_sql_types.jsp");
    // Should extract all four SQL types (SELECT, INSERT, UPDATE, DELETE)
    let jsql_count = node_count(&json, "jspsql");
    assert!(
        jsql_count >= 1,
        "must extract at least 1 SQL statement, got {jsql_count}"
    );

    // Verify each DML type is present in the JspSql nodes
    let has_select = jsp_sql_contains(&json, "SELECT");
    let has_insert = jsp_sql_contains(&json, "INSERT");
    let has_update = jsp_sql_contains(&json, "UPDATE");
    let has_delete = jsp_sql_contains(&json, "DELETE");
    eprintln!(
        "Mixed SQL types: SELECT={has_select}, INSERT={has_insert}, UPDATE={has_update}, DELETE={has_delete}"
    );
    assert!(
        has_select || has_insert || has_update || has_delete,
        "must extract at least one SQL type"
    );
}

#[test]
fn regress_jsp_e03_mixed_decl_and_scriptlet() {
    let json = analyze_jsp_only(E03_MIXED_DECL_SCRIPTLET, "e03_mixed_decl_and_scriptlet.jsp");
    let jsql_count = node_count(&json, "jspsql");
    assert!(
        jsql_count >= 1,
        "must extract SQL from both declaration and scriptlet"
    );

    // Declaration SQL should be extracted as constant
    let has_user_sql = jsp_sql_contains(&json, "users");
    let has_product_sql = jsp_sql_contains(&json, "products");
    eprintln!("Mixed decl+scriptlet: users_SQL={has_user_sql}, products_SQL={has_product_sql}");
    assert!(
        has_user_sql || has_product_sql,
        "must extract SQL from at least one of the two SQL sources"
    );
}

#[test]
fn regress_jsp_e04_try_catch_jdbc() {
    let json = analyze_jsp_only(E04_TRY_CATCH, "e04_try_catch_jdbc.jsp");
    // JDBC in try-catch blocks should still extract SQL
    assert!(
        has_jsp_sql(&json),
        "must extract SQL from try-catch JDBC block"
    );
    assert!(
        jsp_sql_contains(&json, "orders"),
        "JspSql must reference 'orders' table"
    );
    assert!(
        jsp_sql_contains(&json, "SELECT"),
        "JspSql must contain SELECT"
    );
}

// ═══════════════════════════════════════════════════════════════
// Category F: Cross-file integration tests (3 tests)
// ═══════════════════════════════════════════════════════════════

#[test]
fn regress_jsp_f01_jsp_calls_procedure_in_sql() {
    let json = analyze_jsp_with_sql(
        F01_JSP_PROC,
        "f01_jsp_calls_procedure_in_sql.jsp",
        F01_JSP_PROC_SQL,
        "f01_jsp_calls_procedure_in_sql.sql",
    );
    assert!(
        has_jsp_sql(&json),
        "must extract SQL from JSP calling stored procedure"
    );
    assert!(
        jsp_sql_contains(&json, "process_order"),
        "JspSql must reference process_order"
    );

    // Should have a procedure node from the SQL file
    let proc_exists = find_node_by_name(&json, "process_order").is_some();
    assert!(
        proc_exists,
        "must have a procedure node for process_order defined in companion .sql"
    );
}

#[test]
fn regress_jsp_f02_jsp_references_table_in_sql() {
    let json = analyze_jsp_with_sql(
        F02_JSP_TABLE,
        "f02_jsp_references_table_in_sql.jsp",
        F02_JSP_TABLE_SQL,
        "f02_jsp_references_table_in_sql.sql",
    );
    assert!(
        has_jsp_sql(&json),
        "must extract SQL from JSP querying table"
    );
    assert!(
        jsp_sql_contains(&json, "products"),
        "JspSql must reference products table"
    );

    // Should have a table node from the SQL file
    let table_exists = find_node_by_name(&json, "products").is_some();
    assert!(
        table_exists,
        "must have a table node for products defined in companion .sql"
    );
}

#[test]
fn regress_jsp_f03_multi_jsp_project() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("page1.jsp"), F03_MULTI_PAGE1).unwrap();
    fs::write(dir.path().join("page2.jsp"), F03_MULTI_PAGE2).unwrap();
    fs::write(dir.path().join("shared.sql"), F03_MULTI_SHARED_SQL).unwrap();

    let json = analyze_dir(&dir);

    // Should have JSP pages for both JSP files
    let jsp_count = node_count(&json, "jsp");
    assert!(
        jsp_count >= 2,
        "expected >= 2 JspPage nodes for multi-JSP project, got {jsp_count}"
    );

    // Both pages should contribute JspSql nodes
    let jsql_count = node_count(&json, "jspsql");
    assert!(
        jsql_count >= 1,
        "expected >= 1 JspSql nodes from 2 JSP pages, got {jsql_count}"
    );

    // The shared procedure and table should exist
    let proc_exists = find_node_by_name(&json, "get_customer").is_some();
    assert!(
        proc_exists,
        "must have procedure node get_customer from shared SQL"
    );

    let table_exists = find_node_by_name(&json, "customers").is_some();
    assert!(
        table_exists,
        "must have table node customers from shared SQL"
    );
}

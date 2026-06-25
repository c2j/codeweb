use std::fs;
use tempfile::TempDir;

const EXPR_ASSIGNMENT: &str = include_str!("regress/function_call_edges/cases/expr_assignment.sql");
const PERFORM_CALL: &str = include_str!("regress/function_call_edges/cases/perform_call.sql");
const SELECT_TARGET_AND_WHERE: &str =
    include_str!("regress/function_call_edges/cases/select_target_and_where.sql");
const SCHEMA_MISMATCH: &str = include_str!("regress/function_call_edges/cases/schema_mismatch.sql");
const WHERE_SUBQUERY: &str = include_str!("regress/function_call_edges/cases/where_subquery.sql");
const BUILTIN_NOT_CAPTURED: &str =
    include_str!("regress/function_call_edges/cases/builtin_not_captured.sql");
const DBE_XMLDOM_BUILTIN: &str =
    include_str!("regress/function_call_edges/cases/dbe_xmldom_builtin.sql");

fn run_codeweb(args: &[&str]) -> std::process::Output {
    let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
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

fn analyze_json(sql: &str) -> serde_json::Value {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("test.sql"), sql).unwrap();
    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).expect("failed to parse JSON output")
}

fn node_id_by_name(json: &serde_json::Value, name: &str) -> Option<usize> {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"].as_str() == Some(name))
        .and_then(|n| n["id"].as_u64())
        .map(|id| id as usize)
}

fn has_direct_edge(json: &serde_json::Value, source: &str, target: &str) -> bool {
    let (Some(src_id), Some(dst_id)) =
        (node_id_by_name(json, source), node_id_by_name(json, target))
    else {
        return false;
    };
    json["edges"].as_array().unwrap().iter().any(|e| {
        e["source"].as_u64() == Some(src_id as u64)
            && e["target"].as_u64() == Some(dst_id as u64)
            && e["type"] == "direct"
    })
}

fn has_any_direct_or_dynamic_edge(json: &serde_json::Value) -> bool {
    json["edges"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["type"] == "direct" || e["type"] == "dynamic")
}

fn unresolved_nodes(json: &serde_json::Value) -> Vec<&serde_json::Value> {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["type"].as_str() == Some("unresolved"))
        .collect()
}

#[test]
fn regress_expr_assignment() {
    let json = analyze_json(EXPR_ASSIGNMENT);
    assert!(
        has_direct_edge(&json, "process_order", "calc_total"),
        "Expected DirectCall edge: process_order -> calc_total"
    );
}

#[test]
fn regress_perform_call() {
    let json = analyze_json(PERFORM_CALL);
    assert!(
        has_direct_edge(&json, "foo", "bar"),
        "Expected DirectCall edge: foo -> bar"
    );
}

#[test]
fn regress_select_target_and_where() {
    let json = analyze_json(SELECT_TARGET_AND_WHERE);
    assert!(
        has_direct_edge(&json, "report_users", "format_name"),
        "Expected DirectCall edge: report_users -> format_name (SELECT target)"
    );
    assert!(
        has_direct_edge(&json, "report_users", "get_priority"),
        "Expected DirectCall edge: report_users -> get_priority (WHERE clause)"
    );
}

#[test]
fn regress_schema_mismatch() {
    let json = analyze_json(SCHEMA_MISMATCH);
    assert!(
        has_direct_edge(&json, "process_order", "calc_total"),
        "Expected DirectCall edge: process_order -> calc_total (unqualified call to biz.calc_total)"
    );
}

#[test]
fn regress_where_subquery() {
    let json = analyze_json(WHERE_SUBQUERY);
    assert!(
        has_direct_edge(&json, "find_high_value_orders", "get_threshold"),
        "Expected DirectCall edge: find_high_value_orders -> get_threshold (inside WHERE subquery)"
    );
}

#[test]
fn regress_builtin_not_captured() {
    let json = analyze_json(BUILTIN_NOT_CAPTURED);
    assert!(
        !has_any_direct_or_dynamic_edge(&json),
        "Built-in COUNT must NOT create any call edges"
    );
}

#[test]
fn regress_dbe_xmldom_builtin_not_unresolved() {
    let json = analyze_json(DBE_XMLDOM_BUILTIN);
    let unresolved = unresolved_nodes(&json);
    assert!(
        unresolved.is_empty(),
        "dbe_xmldom.* are GaussDB built-in system calls and must NOT spawn Unresolved nodes; \
         found: {:?}",
        unresolved
    );
}

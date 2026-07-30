use std::fs;
use tempfile::TempDir;

const MULTI_SCHEMA_SAME_TABLE: &str =
    include_str!("regress/issue_120_multi_schema_table_access/cases/multi_schema_same_table.sql");

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
        "codeweb analyze failed\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).expect("failed to parse JSON output")
}

fn node_id_by_name_schema(json: &serde_json::Value, name: &str, schema: &str) -> Option<usize> {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"].as_str() == Some(name) && n["schema"].as_str() == Some(schema))
        .and_then(|n| n["id"].as_u64())
        .map(|id| id as usize)
}

fn has_table_access_edge(
    json: &serde_json::Value,
    src_name: &str,
    src_schema: &str,
    dst_name: &str,
    dst_schema: &str,
) -> bool {
    let (Some(src_id), Some(dst_id)) = (
        node_id_by_name_schema(json, src_name, src_schema),
        node_id_by_name_schema(json, dst_name, dst_schema),
    ) else {
        return false;
    };
    json["edges"].as_array().unwrap().iter().any(|e| {
        e["source"].as_u64() == Some(src_id as u64)
            && e["target"].as_u64() == Some(dst_id as u64)
            && e["type"].as_str() == Some("table_access")
    })
}

fn table_node_count(json: &serde_json::Value, name: &str) -> usize {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["name"].as_str() == Some(name) && n["type"].as_str() == Some("table"))
        .count()
}

#[test]
fn regress_multi_schema_same_table_qualified_and_bare() {
    let json = analyze_json(MULTI_SCHEMA_SAME_TABLE);

    let table_count = table_node_count(&json, "tab1");
    assert_eq!(
        table_count, 2,
        "Expected 2 table nodes named 'tab1' (one per schema), got {}",
        table_count
    );

    assert!(
        node_id_by_name_schema(&json, "tab1", "schema_a").is_some(),
        "table schema_a.tab1 not found in graph"
    );
    assert!(
        node_id_by_name_schema(&json, "tab1", "schema_b").is_some(),
        "table schema_b.tab1 not found in graph"
    );

    assert!(
        node_id_by_name_schema(&json, "proc_a", "schema_a").is_some(),
        "procedure schema_a.proc_a not found in graph"
    );
    assert!(
        node_id_by_name_schema(&json, "proc_b", "schema_b").is_some(),
        "procedure schema_b.proc_b not found in graph"
    );

    assert!(
        has_table_access_edge(&json, "proc_a", "schema_a", "tab1", "schema_a"),
        "Issue #120: expected TableAccess edge schema_a.proc_a → schema_a.tab1 \
         (qualified reference should resolve correctly)"
    );

    assert!(
        has_table_access_edge(&json, "proc_b", "schema_b", "tab1", "schema_b"),
        "Issue #120: expected TableAccess edge schema_b.proc_b → schema_b.tab1 \
         (bare reference `tab1` should resolve to proc_b's owner schema schema_b), \
         but got wrong target. Bug: bare-name alias collision in table_index."
    );

    assert!(
        !has_table_access_edge(&json, "proc_b", "schema_b", "tab1", "schema_a"),
        "Issue #120: incorrect cross-schema edge schema_b.proc_b → schema_a.tab1. \
         schema_b.proc_b should NOT reach schema_a.tab1."
    );

    let edge_count: usize = json["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"].as_str() == Some("table_access"))
        .count();
    assert_eq!(
        edge_count, 2,
        "Expected exactly 2 TableAccess edges (one per procedure), got {}",
        edge_count
    );
}

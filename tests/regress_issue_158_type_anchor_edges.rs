//! Regression for #158: `%TYPE`/`%ROWTYPE` schema anchors must produce
//! `AnchorsOn` edges alongside (not instead of) normal `TableAccess` edges,
//! and must never fire for `cursor%ROWTYPE` (issue #147/#142 guard).
//!
//! Simplified, parseable equivalent of the real-world issue #158 sample: a
//! function whose `RETURN` clause, a local `RESULT` variable, and another
//! local variable all anchor to the same DML-read table
//! (`par_sys_purchase`), plus a second variable anchored to a table that is
//! never referenced in DML (`dat_trd_repurchase`) — this table must get an
//! inferred `table*` node with only an `AnchorsOn` edge, no `TableAccess`.

use std::fs;
use tempfile::TempDir;

const ANCHOR_EDGES: &str =
    include_str!("regress/issue_158_type_anchor_edges/cases/anchor_edges.sql");

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

fn node_id_by_name(json: &serde_json::Value, name: &str) -> Option<usize> {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"].as_str().map(|s| s.eq_ignore_ascii_case(name)) == Some(true))
        .and_then(|n| n["id"].as_u64())
        .map(|id| id as usize)
}

fn edges_between(json: &serde_json::Value, source: &str, target: &str) -> Vec<serde_json::Value> {
    let (Some(src_id), Some(dst_id)) =
        (node_id_by_name(json, source), node_id_by_name(json, target))
    else {
        return vec![];
    };
    json["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| {
            e["source"].as_u64() == Some(src_id as u64)
                && e["target"].as_u64() == Some(dst_id as u64)
        })
        .cloned()
        .collect()
}

fn node_by_name(json: &serde_json::Value, name: &str) -> serde_json::Value {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"].as_str().map(|s| s.eq_ignore_ascii_case(name)) == Some(true))
        .cloned()
        .unwrap_or_else(|| panic!("node '{name}' not found in graph"))
}

/// Both `TableAccess` (from the `SELECT ... INTO` DML read) and `AnchorsOn`
/// (from the `%TYPE` signature/variable anchors) must exist between the
/// function and `par_sys_purchase` — coexisting, not merged into one edge.
#[test]
fn issue_158_dml_and_anchor_edges_coexist_on_read_table() {
    let json = analyze_json(ANCHOR_EDGES);

    let edges = edges_between(&json, "fnc_get_purchase_js_days", "par_sys_purchase");
    let table_access_count = edges
        .iter()
        .filter(|e| e["type"].as_str() == Some("table_access"))
        .count();
    let anchor_count = edges
        .iter()
        .filter(|e| e["type"].as_str() == Some("anchors_on"))
        .count();

    assert!(
        table_access_count >= 1,
        "expected at least 1 TableAccess edge f -> par_sys_purchase, got edges: {edges:?}"
    );
    assert!(
        anchor_count >= 1,
        "expected at least 1 AnchorsOn edge f -> par_sys_purchase, got edges: {edges:?}"
    );

    let has_read = edges.iter().any(|e| {
        e["type"].as_str() == Some("table_access")
            && e["modes"]
                .as_array()
                .map(|m| m.iter().any(|v| v.as_str() == Some("read")))
                .unwrap_or(false)
    });
    assert!(
        has_read,
        "TableAccess edge to par_sys_purchase must carry Read mode, got edges: {edges:?}"
    );
}

/// `dat_trd_repurchase` is referenced only through a `%TYPE` variable
/// declaration, never in DML — it must become an inferred (`explicit:
/// false`) table node reachable only via `AnchorsOn`, with zero
/// `TableAccess` edges.
#[test]
fn issue_158_type_only_reference_produces_inferred_table_with_anchor_only() {
    let json = analyze_json(ANCHOR_EDGES);

    let table_node = node_by_name(&json, "dat_trd_repurchase");
    assert_eq!(table_node["type"].as_str(), Some("table"));
    // `explicit` is only serialized when `true` (skip_serializing_if =
    // is_false in json.rs) — a missing field means explicit=false, i.e.
    // this table has no DDL and was inferred from the %TYPE anchor.
    assert!(
        !table_node["explicit"].as_bool().unwrap_or(false),
        "dat_trd_repurchase has no DDL — must be inferred (explicit=false), got {table_node:?}"
    );

    let edges = edges_between(&json, "fnc_get_purchase_js_days", "dat_trd_repurchase");
    assert!(
        !edges.is_empty(),
        "expected at least 1 AnchorsOn edge f -> dat_trd_repurchase"
    );
    assert!(
        edges
            .iter()
            .all(|e| e["type"].as_str() == Some("anchors_on")),
        "dat_trd_repurchase must only be reached via AnchorsOn edges (no DML), got: {edges:?}"
    );
}

/// No `AnchorsOn` edge anywhere in the graph may originate from a
/// `cursor%ROWTYPE` declaration — this fixture has no cursors at all, so
/// this is a straightforward absence check guarding against a regression
/// that would anchor every `%ROWTYPE` unconditionally.
#[test]
fn issue_158_no_cursor_rowtype_anchor_edges_present() {
    let json = analyze_json(ANCHOR_EDGES);

    let anchor_edges: Vec<_> = json["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"].as_str() == Some("anchors_on"))
        .collect();
    assert!(
        !anchor_edges.is_empty(),
        "sanity check: fixture should produce at least one AnchorsOn edge"
    );

    for edge in &anchor_edges {
        assert_ne!(
            edge["kind"].as_str(),
            Some("percent_row_type"),
            "this fixture declares no cursors — no percent_row_type anchor should exist, got {edge:?}"
        );
    }
}

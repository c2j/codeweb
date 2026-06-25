use std::fs;
use tempfile::TempDir;

const COLLECTION_INDEX_NOT_CAPTURED: &str =
    include_str!("regress/local_var_not_call_edges/cases/collection_index_not_captured.sql");
const COLLECTION_INDEX_WITH_REAL_CALL: &str =
    include_str!("regress/local_var_not_call_edges/cases/collection_index_with_real_call.sql");
const PARAM_SHADOWS_PROCEDURE: &str =
    include_str!("regress/local_var_not_call_edges/cases/param_shadows_procedure.sql");
const SCOPE_RESET_ACROSS_PROCEDURES: &str =
    include_str!("regress/local_var_not_call_edges/cases/scope_reset_across_procedures.sql");
const TYPE_CONSTRUCTOR_NOT_CAPTURED: &str =
    include_str!("regress/local_var_not_call_edges/cases/type_constructor_not_captured.sql");

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

/// Count nodes of type "unresolved". Each false-positive call edge creates one.
fn count_unresolved_nodes(json: &serde_json::Value) -> usize {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["type"].as_str() == Some("unresolved"))
        .count()
}

#[test]
fn regress_collection_index_not_captured() {
    let json = analyze_json(COLLECTION_INDEX_NOT_CAPTURED);
    assert!(
        !has_any_direct_or_dynamic_edge(&json),
        "PL/SQL collection index access v_date(i)/v_fund(i) must NOT create any call edges"
    );
}

#[test]
fn regress_collection_index_with_real_call() {
    let json = analyze_json(COLLECTION_INDEX_WITH_REAL_CALL);
    assert!(
        has_direct_edge(&json, "clean_proc", "compute_score"),
        "Expected DirectCall edge: clean_proc -> compute_score (real function call)"
    );
    let unresolved = count_unresolved_nodes(&json);
    assert_eq!(
        unresolved, 0,
        "Collection variable v_scores must NOT spawn an Unresolved node; found {unresolved}"
    );
}

#[test]
fn regress_param_shadows_procedure() {
    let json = analyze_json(PARAM_SHADOWS_PROCEDURE);
    assert!(
        has_direct_edge(&json, "batch_check", "real_target"),
        "Expected DirectCall edge: batch_check -> real_target (real function call)"
    );
    let unresolved = count_unresolved_nodes(&json);
    assert_eq!(
        unresolved, 0,
        "Parameter p_ids must NOT spawn an Unresolved node; found {unresolved}"
    );
}

#[test]
fn regress_scope_reset_across_procedures() {
    let json = analyze_json(SCOPE_RESET_ACROSS_PROCEDURES);
    assert!(
        has_direct_edge(&json, "proc_b", "v_date"),
        "proc_b must have a DirectCall edge to the v_date function (scope reset working)"
    );
    assert!(
        !has_direct_edge(&json, "proc_a", "v_date"),
        "proc_a's local collection variable v_date must NOT produce a call edge to the v_date function"
    );
}

#[test]
fn regress_type_constructor_not_captured() {
    let json = analyze_json(TYPE_CONSTRUCTOR_NOT_CAPTURED);
    let unresolved = count_unresolved_nodes(&json);
    assert_eq!(
        unresolved, 0,
        "TYPE constructor account_record_table(), member method obj_account_record.equals(...), \
         and collection index aaa1(i)/aaa2(i) must NOT spawn Unresolved nodes; found {unresolved}"
    );
}

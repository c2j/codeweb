use std::fs;
use tempfile::TempDir;

const AMBIGUOUS_BARE_NAME: &str =
    include_str!("regress/func_schema_resolution/cases/ambiguous_bare_name.sql");
const AMBIGUOUS_STANDALONE_AND_PKG: &str =
    include_str!("regress/func_schema_resolution/cases/ambiguous_standalone_and_pkg.sql");
const CALLER_SCHEMA_DISAMBIGUATION: &str =
    include_str!("regress/func_schema_resolution/cases/caller_schema_disambiguation.sql");
const MULTI_SCHEMA_SAME_UTIL: &str =
    include_str!("regress/func_schema_resolution/cases/multi_schema_same_util.sql");

fn run_codeweb(args: &[&str]) -> std::process::Output {
    let bin = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("debug")
        .join(if cfg!(windows) {
            "codeweb.exe"
        } else {
            "codeweb"
        });
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

fn has_direct_edge_to_any_named(json: &serde_json::Value, source: &str, target_name: &str) -> bool {
    let Some(src_id) = node_id_by_name(json, source) else {
        return false;
    };
    let target_ids: Vec<u64> = json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["name"].as_str() == Some(target_name))
        .filter_map(|n| n["id"].as_u64())
        .collect();
    json["edges"].as_array().unwrap().iter().any(|e| {
        e["source"].as_u64() == Some(src_id as u64)
            && target_ids.contains(&(e["target"].as_u64().unwrap_or(0)))
            && e["type"] == "direct"
    })
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
fn regress_ambiguous_bare_name_two_schemas() {
    let json = analyze_json(AMBIGUOUS_BARE_NAME);
    let unresolved = unresolved_nodes(&json);
    assert!(
        unresolved.is_empty(),
        "Bug #1+#2: unqualified call to ambiguous 'util_func' (defined in app_a + app_b) \
         should resolve via caller-schema or fallback, but got Unresolved nodes: {:?}",
        unresolved
            .iter()
            .map(|n| n["raw_expr"].as_str().unwrap_or("?"))
            .collect::<Vec<_>>()
    );
    assert!(
        has_direct_edge(&json, "caller_proc", "util_func"),
        "Expected DirectCall edge: caller_proc -> util_func"
    );
}

#[test]
fn regress_ambiguous_standalone_and_package() {
    let json = analyze_json(AMBIGUOUS_STANDALONE_AND_PKG);
    let unresolved = unresolved_nodes(&json);
    assert!(
        unresolved.is_empty(),
        "Bug #1+#2+#6: unqualified call to ambiguous 'helper' (standalone biz.helper + \
         package member util_pkg.helper) should resolve, but got Unresolved nodes: {:?}",
        unresolved
            .iter()
            .map(|n| n["raw_expr"].as_str().unwrap_or("?"))
            .collect::<Vec<_>>()
    );
    assert!(
        has_direct_edge_to_any_named(&json, "do_work", "helper"),
        "Expected DirectCall edge: do_work -> helper"
    );
}

#[test]
fn regress_caller_schema_disambiguation() {
    let json = analyze_json(CALLER_SCHEMA_DISAMBIGUATION);
    let unresolved = unresolved_nodes(&json);
    assert!(
        unresolved.is_empty(),
        "Bug #2: caller s1.run_it calls bare 'compute' which exists in s1 + s2; \
         should resolve to s1.compute via caller-schema context, but got Unresolved nodes: {:?}",
        unresolved
            .iter()
            .map(|n| n["raw_expr"].as_str().unwrap_or("?"))
            .collect::<Vec<_>>()
    );
    assert!(
        has_direct_edge(&json, "run_it", "compute"),
        "Expected DirectCall edge: run_it -> compute (resolved to caller's schema s1)"
    );
}

#[test]
fn regress_multi_schema_same_util() {
    let json = analyze_json(MULTI_SCHEMA_SAME_UTIL);
    let unresolved = unresolved_nodes(&json);
    assert!(
        unresolved.is_empty(),
        "Bug #1+#2 (systemic): three schemas (mod_a/b/c) define format_date; \
         unqualified call should resolve via caller-schema context or fallback, \
         but got Unresolved nodes: {:?}",
        unresolved
            .iter()
            .map(|n| n["raw_expr"].as_str().unwrap_or("?"))
            .collect::<Vec<_>>()
    );
    assert!(
        has_direct_edge(&json, "batch_run", "format_date"),
        "Expected DirectCall edge: batch_run -> format_date"
    );
}

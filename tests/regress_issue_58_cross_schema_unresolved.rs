use std::fs;
use tempfile::TempDir;

const CROSS_SCHEMA_SHARED_UNRESOLVED: &str = include_str!(
    "regress/issue_58_cross_schema_unresolved/cases/cross_schema_shared_unresolved.sql"
);

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

fn node_id_by_name_schema(json: &serde_json::Value, name: &str, schema: &str) -> Option<usize> {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"].as_str() == Some(name) && n["schema"].as_str() == Some(schema))
        .and_then(|n| n["id"].as_u64())
        .map(|id| id as usize)
}

fn has_edge(
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
            && e["type"] == "direct"
    })
}

#[test]
fn regress_cross_schema_shared_unresolved_correct_edges() {
    let json = analyze_json(CROSS_SCHEMA_SHARED_UNRESOLVED);

    // Both compute nodes must exist in the graph
    assert!(
        node_id_by_name_schema(&json, "compute", "s1").is_some(),
        "Issue #58: compute function in schema s1 not found in graph"
    );
    assert!(
        node_id_by_name_schema(&json, "compute", "s2").is_some(),
        "Issue #58: compute function in schema s2 not found in graph"
    );

    // s1.proc_a → s1.compute: per-edge resolution resolves based on proc_a's schema
    assert!(
        has_edge(&json, "proc_a", "s1", "compute", "s1"),
        "Issue #58: expected edge from s1.proc_a → s1.compute (caller schema match)"
    );

    // s2.proc_b → s2.compute: per-edge resolution resolves based on proc_b's schema
    assert!(
        has_edge(&json, "proc_b", "s2", "compute", "s2"),
        "Issue #58: expected edge from s2.proc_b → s2.compute (caller schema match)"
    );

    // s1.proc_a must NOT have an edge to s2.compute (cross-schema)
    assert!(
        !has_edge(&json, "proc_a", "s1", "compute", "s2"),
        "Issue #58: incorrect cross-schema edge from s1.proc_a → s2.compute"
    );
}

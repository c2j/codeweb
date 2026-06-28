use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

fn codeweb_bin() -> PathBuf {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    let bin_name = if cfg!(windows) {
        "codeweb.exe"
    } else {
        "codeweb"
    };
    let entries = fs::read_dir(&base).unwrap_or_else(|_| panic!("no target dir"));
    for entry in entries.flatten() {
        let p = entry.path().join("debug").join(bin_name);
        if p.exists() {
            return p;
        }
    }
    base.join("debug").join(bin_name)
}

fn run_in_dir(dir: &TempDir, args: &[&str]) -> std::process::Output {
    std::process::Command::new(codeweb_bin())
        .args(args)
        .current_dir(dir.path())
        .output()
        .expect("failed to run codeweb")
}

fn write_sql(dir: &TempDir, filename: &str, sql: &str) {
    fs::write(dir.path().join(filename), sql).unwrap();
}

fn setup_project() -> TempDir {
    let dir = TempDir::new().unwrap();

    write_sql(
        &dir,
        "proc_a.sql",
        "CREATE OR REPLACE PROCEDURE proc_a() AS\nBEGIN\n    CALL proc_b();\nEND;\n/\n",
    );

    write_sql(
        &dir,
        "proc_b.sql",
        "CREATE OR REPLACE PROCEDURE proc_b() AS\nBEGIN\n    NULL;\nEND;\n/\n",
    );

    let output = run_in_dir(&dir, &["init", "test-project", "-d", "."]);
    assert!(
        output.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    dir
}

#[test]
fn test_impact_json_schema() {
    let dir = setup_project();

    let output = run_in_dir(
        &dir,
        &["impact", "--file", "proc_a.sql", "--format", "json"],
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("must be valid JSON");

    assert_eq!(json["schema_version"], 2);
    assert!(
        json["file"].as_str().unwrap().ends_with("proc_a.sql"),
        "file field: {:?}",
        json["file"]
    );
    // --file 路径下 node 字段应为 null 或不存在
    assert!(
        json.get("node").map(|v| v.is_null()).unwrap_or(true),
        "node should be null or absent for --file: {:?}",
        json.get("node")
    );
    assert!(json["upstream"].is_array());
    assert!(json["downstream"].is_array());

    let downstream = json["downstream"].as_array().unwrap();
    assert!(
        downstream
            .iter()
            .any(|e| e["symbol"].as_str().unwrap().contains("proc_b")),
        "downstream should contain proc_b: {:?}",
        downstream
    );
}

#[test]
fn test_impact_file_not_in_graph() {
    let dir = setup_project();

    let output = run_in_dir(
        &dir,
        &["impact", "--file", "nonexistent.sql", "--format", "json"],
    );

    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("must be valid JSON");

    assert_eq!(json["schema_version"], 2);
    assert!(json["upstream"].as_array().unwrap().is_empty());
    assert!(json["downstream"].as_array().unwrap().is_empty());

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("not found in graph"),
        "stderr should warn: {}",
        stderr
    );
}

#[test]
fn test_impact_text_format() {
    let dir = setup_project();

    let output = run_in_dir(
        &dir,
        &["impact", "--file", "proc_b.sql", "--format", "text"],
    );

    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("UPSTREAM"));
    assert!(
        stdout.contains("proc_a"),
        "upstream should contain proc_a: {}",
        stdout
    );
}

#[test]
fn test_impact_upstream_direction() {
    let dir = setup_project();

    let output = run_in_dir(
        &dir,
        &["impact", "--file", "proc_b.sql", "--format", "json"],
    );
    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("must be valid JSON");

    let upstream = json["upstream"].as_array().unwrap();
    assert!(
        upstream
            .iter()
            .any(|e| e["symbol"].as_str().unwrap().contains("proc_a")),
        "upstream of proc_b should contain proc_a: {:?}",
        upstream
    );

    let downstream = json["downstream"].as_array().unwrap();
    assert!(
        downstream.is_empty(),
        "proc_b has no downstream: {:?}",
        downstream
    );
}

// ────────────────────────────────────────────────────────────────────
// --node 入口测试
// ────────────────────────────────────────────────────────────────────

#[test]
fn test_impact_node_json_schema() {
    let dir = setup_project();

    // proc_a 调用 proc_b → 对 proc_a 查 node:
    //   upstream 为空(没人调用 proc_a)
    //   downstream 含 proc_b
    let output = run_in_dir(&dir, &["impact", "--node", "proc_a", "--format", "json"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("must be valid JSON");

    assert_eq!(json["schema_version"], 2);
    assert!(
        json["node"].as_str().unwrap().contains("proc_a"),
        "node field should contain proc_a: {:?}",
        json["node"]
    );
    // file 字段在 --node 路径下应不存在(skip_serializing_if)或为 null
    assert!(
        json.get("file").map(|v| v.is_null()).unwrap_or(true),
        "file should be null or absent for --node: {:?}",
        json.get("file")
    );

    let downstream = json["downstream"].as_array().unwrap();
    assert!(
        downstream
            .iter()
            .any(|e| e["symbol"].as_str().unwrap().contains("proc_b")),
        "downstream should contain proc_b: {:?}",
        downstream
    );

    let upstream = json["upstream"].as_array().unwrap();
    assert!(
        upstream.is_empty(),
        "proc_a has no upstream: {:?}",
        upstream
    );
}

#[test]
fn test_impact_node_upstream_direction() {
    let dir = setup_project();

    // 对 proc_b 查 node:被 proc_a 调用 → upstream 含 proc_a,downstream 为空
    let output = run_in_dir(&dir, &["impact", "--node", "proc_b", "--format", "json"]);
    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("must be valid JSON");

    let upstream = json["upstream"].as_array().unwrap();
    assert!(
        upstream
            .iter()
            .any(|e| e["symbol"].as_str().unwrap().contains("proc_a")),
        "upstream of proc_b should contain proc_a: {:?}",
        upstream
    );

    let downstream = json["downstream"].as_array().unwrap();
    assert!(
        downstream.is_empty(),
        "proc_b has no downstream: {:?}",
        downstream
    );
}

#[test]
fn test_impact_node_not_found() {
    let dir = setup_project();

    let output = run_in_dir(
        &dir,
        &["impact", "--node", "does_not_exist", "--format", "json"],
    );

    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("must be valid JSON");

    assert_eq!(json["schema_version"], 2);
    assert!(json["upstream"].as_array().unwrap().is_empty());
    assert!(json["downstream"].as_array().unwrap().is_empty());

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("No nodes matching"),
        "stderr should warn about missing node: {}",
        stderr
    );
}

#[test]
fn test_impact_node_text_format() {
    let dir = setup_project();

    let output = run_in_dir(&dir, &["impact", "--node", "proc_b", "--format", "text"]);
    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("Node:"),
        "text output should show 'Node:' header: {}",
        stdout
    );
    assert!(
        stdout.contains("proc_a"),
        "upstream should contain proc_a: {}",
        stdout
    );
}

#[test]
fn test_impact_mutual_exclusion_error() {
    let dir = setup_project();

    // 同时给 --file 和 --node → 退出码 2
    let output = run_in_dir(
        &dir,
        &["impact", "--file", "proc_a.sql", "--node", "proc_a"],
    );
    assert!(
        !output.status.success(),
        "should fail when both --file and --node given"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("mutually exclusive"),
        "stderr should mention mutual exclusion: {}",
        stderr
    );
}

#[test]
fn test_impact_neither_flag_error() {
    let dir = setup_project();

    // 都不给 → 退出码 2
    let output = run_in_dir(&dir, &["impact"]);
    assert!(
        !output.status.success(),
        "should fail when neither --file nor --node given"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("exactly one of"),
        "stderr should mention required flag: {}",
        stderr
    );
}

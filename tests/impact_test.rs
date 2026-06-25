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

    assert_eq!(json["schema_version"], 1);
    assert!(
        json["file"].as_str().unwrap().ends_with("proc_a.sql"),
        "file field: {:?}",
        json["file"]
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

    assert_eq!(json["schema_version"], 1);
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

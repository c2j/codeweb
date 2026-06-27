use std::fs;
use tempfile::TempDir;

fn codeweb_bin() -> std::path::PathBuf {
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
            return p;
        }
    }
    base.join("debug").join(bin_name)
}

fn run(args: &[&str]) -> std::process::Output {
    let bin = codeweb_bin();
    std::process::Command::new(&bin)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to run {}: {}", bin.display(), e))
}

#[test]
fn analyze_command_builtin_dedup_across_files() {
    let tmp = TempDir::new().unwrap();

    let sql_dir = tmp.path().join("sql");
    fs::create_dir_all(&sql_dir).unwrap();
    fs::write(sql_dir.join("a.sql"), "SELECT ascii('x') FROM dual;\n").unwrap();
    fs::write(sql_dir.join("b.sql"), "SELECT ascii('y') FROM dual;\n").unwrap();
    fs::write(
        sql_dir.join("c.sql"),
        "CREATE OR REPLACE PROCEDURE p1 AS v INT; BEGIN v := ascii('z'); END;\n/\n",
    )
    .unwrap();
    fs::write(
        sql_dir.join("d.sql"),
        "CREATE OR REPLACE PROCEDURE p2 AS v1 INT; v2 VARCHAR2(100);\nBEGIN v1 := ascii('w'); v2 := substr('hello', 1, 3); END;\n/\n",
    )
    .unwrap();

    let proj_dir = tmp.path().join("test-proj");
    let sql_dir_abs = fs::canonicalize(&sql_dir).unwrap();

    let output = run(&[
        "init", "test-proj",
        "-d", sql_dir_abs.to_str().unwrap(),
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("=== init stderr ===\n{}", stderr);
    if !output.status.success() {
        // `init` creates the project dir in CWD, and the repo root already has a codeweb.toml.
        // Retry with CWD = tmp dir.
        let output_retry = std::process::Command::new(codeweb_bin())
            .args(&["init", "test-proj", "-d", sql_dir_abs.to_str().unwrap()])
            .current_dir(tmp.path())
            .output()
            .expect("init failed");
        let stderr_retry = String::from_utf8_lossy(&output_retry.stderr);
        eprintln!("=== init retry stderr ===\n{}", stderr_retry);
        assert!(output_retry.status.success(), "init retry failed: {}", stderr_retry);
    }

    let output2 = run(&[
        "export", "--format", "json", "--project",
        proj_dir.to_str().unwrap(),
    ]);
    let stderr2 = String::from_utf8_lossy(&output2.stderr);
    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    eprintln!("=== export stderr ===\n{}", stderr2);
    assert!(output2.status.success(), "export failed: {}", stderr2);

    let json: serde_json::Value = serde_json::from_str(&stdout2).expect("invalid JSON");

    let mut builtin_names: Vec<String> = Vec::new();
    for node in json["nodes"].as_array().unwrap() {
        if node["type"].as_str() == Some("builtin_function") {
            builtin_names.push(node["name"].as_str().unwrap_or("?").to_string());
        }
    }

    eprintln!("Builtin nodes after analyze+export: {:?}", builtin_names);

    let ascii_count = builtin_names.iter().filter(|n| n.to_lowercase() == "ascii").count();
    assert_eq!(ascii_count, 1, "Expected 1 'ascii' BuiltinFunction after analyze, found {}", ascii_count);
    assert!(builtin_names.iter().any(|n| n.to_lowercase() == "substr"), "Expected 'substr' to be present");
}

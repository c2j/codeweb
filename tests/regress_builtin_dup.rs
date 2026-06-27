use std::fs;
use tempfile::TempDir;

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

/// Count BuiltinFunction nodes in JSON output (tagged with `"type":"builtin_function"`)
fn count_builtins(json: &serde_json::Value) -> Vec<(String, String)> {
    let mut builtins = Vec::new();
    for node in json["nodes"].as_array().unwrap() {
        if node["type"].as_str() == Some("builtin_function") {
            builtins.push((
                node["name"].as_str().unwrap_or("?").to_string(),
                node["domain"].as_str().unwrap_or("?").to_string(),
            ));
        }
    }
    builtins
}

/// Count occurrences of each builtin name
fn name_counts(builtins: &[(String, String)]) -> std::collections::HashMap<String, usize> {
    let mut counts = std::collections::HashMap::new();
    for (name, _domain) in builtins {
        *counts.entry(name.to_lowercase()).or_default() += 1;
    }
    counts
}

#[test]
fn multiple_files_same_builtin_produces_single_node() {
    let dir = TempDir::new().unwrap();

    fs::write(dir.path().join("a.sql"), "SELECT ascii('x') FROM dual;\n").unwrap();
    fs::write(dir.path().join("b.sql"), "SELECT ascii('y') FROM dual;\n").unwrap();

    let output = run_codeweb(&[
        dir.path().to_str().unwrap(),
        "--format",
        "json",
        "--sql-only",
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "codeweb failed. stderr:\n{}",
        stderr
    );

    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    let builtins = count_builtins(&json);

    let counts = name_counts(&builtins);
    let ascii_count = counts.get("ascii").copied().unwrap_or(0);
    assert_eq!(
        ascii_count, 1,
        "Expected 1 'ascii' BuiltinFunction node, found {}. stderr:\n{:>80}",
        ascii_count, stderr
    );
}

#[test]
fn standalone_and_procedure_body_calls_same_builtin_produces_single_node() {
    let dir = TempDir::new().unwrap();

    fs::write(
        dir.path().join("standalone.sql"),
        "SELECT ascii('x') FROM dual;\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("proc.sql"),
        "CREATE OR REPLACE PROCEDURE test_ascii AS v INT; BEGIN v := ascii('y'); END;\n/\n",
    )
    .unwrap();

    let output = run_codeweb(&[
        dir.path().to_str().unwrap(),
        "--format",
        "json",
        "--sql-only",
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "codeweb failed. stderr:\n{}", stderr);

    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    let builtins = count_builtins(&json);

    let counts = name_counts(&builtins);
    let ascii_count = counts.get("ascii").copied().unwrap_or(0);
    assert_eq!(
        ascii_count, 1,
        "Expected 1 'ascii' BuiltinFunction node (standalone + proc body), found {}. stderr:\n{:>80}",
        ascii_count, stderr
    );
}

#[test]
fn four_files_with_ascii_and_substr_should_not_duplicate() {
    let dir = TempDir::new().unwrap();

    fs::write(dir.path().join("a.sql"), "SELECT ascii('x') FROM dual;\n").unwrap();
    fs::write(dir.path().join("b.sql"), "SELECT ascii('y') FROM dual;\n").unwrap();
    fs::write(
        dir.path().join("c.sql"),
        "CREATE OR REPLACE PROCEDURE p1 AS v INT; BEGIN v := ascii('z'); END;\n/\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("d.sql"),
        "CREATE OR REPLACE PROCEDURE p2 AS v1 INT; v2 VARCHAR2(100); BEGIN v1 := ascii('w'); v2 := substr('hello', 1, 3); END;\n/\n",
    )
    .unwrap();

    let output = run_codeweb(&[
        dir.path().to_str().unwrap(),
        "--format",
        "json",
        "--sql-only",
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "codeweb failed. stderr:\n{}", stderr);

    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    let builtins = count_builtins(&json);

    eprintln!("All BuiltinFunction nodes: {:#?}", builtins);

    let counts = name_counts(&builtins);
    let ascii_count = counts.get("ascii").copied().unwrap_or(0);
    let substr_count = counts.get("substr").copied().unwrap_or(0);

    assert_eq!(
        ascii_count, 1,
        "Expected 1 'ascii' node across 4 files, found {}. stderr:\n{:>80}",
        ascii_count, stderr
    );
    assert_eq!(substr_count, 1, "Expected 1 'substr' node, found {}", substr_count);

    let total_builtins = builtins.len();
    assert_eq!(
        total_builtins, 2,
        "Expected only 2 BuiltinFunction nodes (ascii + substr), found {}. stderr:\n{:>80}",
        total_builtins, stderr
    );
}

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

fn setup_multi_file_project() -> TempDir {
    let dir = TempDir::new().unwrap();

    fs::write(
        dir.path().join("proc_a.sql"),
        "CREATE OR REPLACE PROCEDURE proc_a() AS\nBEGIN\n    CALL proc_b();\nEND;\n/\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("proc_b.sql"),
        "CREATE OR REPLACE PROCEDURE proc_b() AS\nBEGIN\n    CALL proc_c();\nEND;\n/\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("proc_c.sql"),
        "CREATE OR REPLACE PROCEDURE proc_c() AS\nBEGIN\n    NULL;\nEND;\n/\n",
    )
    .unwrap();

    let output = std::process::Command::new(codeweb_bin())
        .args(&["init", "test-project", "-d", "."])
        .current_dir(dir.path())
        .output()
        .expect("failed to run codeweb init");
    assert!(
        output.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    dir
}

fn run_in_dir(dir: &TempDir, args: &[&str]) -> std::process::Output {
    std::process::Command::new(codeweb_bin())
        .args(args)
        .current_dir(dir.path())
        .output()
        .expect("failed to run codeweb")
}

#[test]
#[ignore = "issue #85: impact command should support multiple --file args (batch mode)"]
fn regress_impact_multiple_file_args_rejected() {
    let dir = setup_multi_file_project();

    let output = run_in_dir(
        &dir,
        &[
            "impact",
            "--file",
            "proc_a.sql",
            "--file",
            "proc_b.sql",
            "--format",
            "json",
        ],
    );

    let stderr = String::from_utf8_lossy(&output.stderr);

    let is_broken = !output.status.success()
        || (output.status.success()
            && !stderr.is_empty()
            && stderr.contains("provided more than once"));

    assert!(
        is_broken,
        "Issue #85: expected --file twice to fail or warn. \
         Exit code: {:?}, stderr: '{}'. \
         Current behavior: clap Option<PathBuf> rejects multiple --file args.",
        output.status.code(),
        stderr.trim()
    );
}

#[test]
fn regress_impact_single_file_workaround() {
    let dir = setup_multi_file_project();

    let output_a = run_in_dir(
        &dir,
        &["impact", "--file", "proc_a.sql", "--format", "json"],
    );
    assert!(
        output_a.status.success(),
        "impact --file proc_a.sql failed: {}",
        String::from_utf8_lossy(&output_a.stderr)
    );

    let a_json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output_a.stdout))
            .expect("proc_a result must be valid JSON");
    assert_eq!(a_json["schema_version"], 2);
    assert!(
        a_json["downstream"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["symbol"].as_str().unwrap().contains("proc_b")),
        "proc_a downstream should contain proc_b"
    );

    let output_b = run_in_dir(
        &dir,
        &["impact", "--file", "proc_b.sql", "--format", "json"],
    );
    assert!(output_b.status.success());

    let b_json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output_b.stdout))
            .expect("proc_b result must be valid JSON");
    assert_eq!(b_json["schema_version"], 2);
    assert!(
        !b_json["upstream"].as_array().unwrap().is_empty(),
        "proc_b upstream should contain proc_a"
    );
    assert!(
        !b_json["downstream"].as_array().unwrap().is_empty(),
        "proc_b downstream should contain proc_c"
    );

    let output_c = run_in_dir(
        &dir,
        &["impact", "--file", "proc_c.sql", "--format", "json"],
    );
    assert!(output_c.status.success());

    let c_json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output_c.stdout))
            .expect("proc_c result must be valid JSON");
    assert!(
        c_json["upstream"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["symbol"].as_str().unwrap().contains("proc_b")),
        "proc_c upstream should contain proc_b"
    );
}

#[test]
#[ignore = "issue #85: after fix, batch impact should return array of results in single invocation"]
fn regress_impact_batch_mode_expected_behavior() {
    let dir = setup_multi_file_project();

    let output = run_in_dir(
        &dir,
        &[
            "impact",
            "--file",
            "proc_a.sql",
            "--file",
            "proc_b.sql",
            "--file",
            "proc_c.sql",
            "--format",
            "json",
        ],
    );

    assert!(
        output.status.success(),
        "Issue #85: batch impact should succeed. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        serde_json::from_str(&stdout).expect("batch impact result must be valid JSON");

    assert!(
        json.is_array(),
        "Issue #85: batch impact should return a JSON array, got: {}",
        json
    );

    let results = json.as_array().unwrap();
    assert_eq!(
        results.len(),
        3,
        "Issue #85: batch impact should return 3 results for 3 files, got {}",
        results.len()
    );
    for (i, result) in results.iter().enumerate() {
        assert_eq!(
            result["schema_version"], 2,
            "Issue #85: result[{}] should have schema_version=2",
            i
        );
        assert!(
            result["file"].is_string(),
            "Issue #85: result[{}] should have a 'file' field",
            i
        );
        assert!(
            result["upstream"].is_array(),
            "Issue #85: result[{}] upstream should be an array",
            i
        );
        assert!(
            result["downstream"].is_array(),
            "Issue #85: result[{}] downstream should be an array",
            i
        );
    }
}

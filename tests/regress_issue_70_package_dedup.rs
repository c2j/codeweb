use std::fs;
use tempfile::TempDir;

const SPEC_STALE: &str = include_str!("regress/issue_70_package_dedup/cases/01_spec_stale.sql");
const SPEC_RICH: &str = include_str!("regress/issue_70_package_dedup/cases/02_spec_rich.sql");

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

fn analyze_json(sql_files: &[(&str, &str)]) -> serde_json::Value {
    let dir = TempDir::new().unwrap();
    for (filename, content) in sql_files {
        fs::write(dir.path().join(filename), content).unwrap();
    }
    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).expect("failed to parse JSON output")
}

#[test]
#[ignore = "issue #70: package spec dedup is first-wins, should be last-wins per CREATE OR REPLACE semantics"]
fn regress_duplicate_package_spec_last_wins() {
    let json = analyze_json(&[
        ("01_spec_stale.sql", SPEC_STALE),
        ("02_spec_rich.sql", SPEC_RICH),
    ]);

    // Find all Package nodes named pkg_x
    let pkg_x_nodes: Vec<&serde_json::Value> = json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["type"].as_str() == Some("package") && n["name"].as_str() == Some("pkg_x"))
        .collect();

    // Exactly one Package node should survive dedup
    assert_eq!(
        pkg_x_nodes.len(),
        1,
        "Issue #70: expected exactly 1 Package node for pkg_x after dedup, found {}. \
         Nodes: {:?}",
        pkg_x_nodes.len(),
        pkg_x_nodes
            .iter()
            .map(|n| format!("file={:?}", n["file"]))
            .collect::<Vec<_>>()
    );

    // The surviving node should be from the richer file (02_spec_rich.sql, last-wins)
    let surviving = pkg_x_nodes[0];
    let file_path = surviving["file"].as_str().unwrap_or("");
    assert!(
        file_path.ends_with("02_spec_rich.sql"),
        "Issue #70: expected surviving Package node for pkg_x to come from \
         02_spec_rich.sql (last-wins per CREATE OR REPLACE semantics), \
         but got file={:?}. \
         Bug: first-wins dedup picks 01_spec_stale.sql instead.",
        file_path
    );
}

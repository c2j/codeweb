use std::fs;
use tempfile::TempDir;

const SPEC_STALE: &str = include_str!("regress/issue_70_package_dedup/cases/01_spec_stale.sql");
const SPEC_RICH: &str = include_str!("regress/issue_70_package_dedup/cases/02_spec_rich.sql");
const BODY: &str = include_str!("regress/issue_70_package_dedup/cases/03_body.sql");

fn nodes_of_type<'a>(json: &'a serde_json::Value, type_name: &str) -> Vec<&'a serde_json::Value> {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["type"].as_str() == Some(type_name))
        .collect()
}

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
fn regress_duplicate_package_spec_node_last_wins() {
    let json = analyze_json(&[
        ("01_spec_stale.sql", SPEC_STALE),
        ("02_spec_rich.sql", SPEC_RICH),
        ("03_body.sql", BODY),
    ]);

    let pkg_x_nodes: Vec<&serde_json::Value> = json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["type"].as_str() == Some("package") && n["name"].as_str() == Some("pkg_x"))
        .collect();

    assert_eq!(
        pkg_x_nodes.len(),
        1,
        "Issue #70: expected exactly 1 Package node for pkg_x after dedup, found {}",
        pkg_x_nodes.len()
    );

    let surviving = pkg_x_nodes[0];
    let file_path = surviving["file"].as_str().unwrap_or("");
    assert!(
        file_path.ends_with("02_spec_rich.sql"),
        "Issue #70: expected surviving Package node for pkg_x to come from \
         02_spec_rich.sql (last-wins per CREATE OR REPLACE semantics), but got file={:?}. \
         Bug: package_index first-wins dedup picks 01_spec_stale.sql instead.",
        file_path
    );
}

#[test]
fn regress_duplicate_package_spec_body_inherits_rich_spec_symbols() {
    let json = analyze_json(&[
        ("01_spec_stale.sql", SPEC_STALE),
        ("02_spec_rich.sql", SPEC_RICH),
        ("03_body.sql", BODY),
    ]);

    // The body's do_work uses the TYPE vchar_array, declared ONLY in the rich
    // spec (02). When the rich spec wins (last-wins), vchar_array is registered
    // as a type and the constructor call vchar_array() is NOT misread as a call.
    // Bug: spec_items_by_pkg first-wins feeds the stale spec's items to the body,
    // so vchar_array is unknown → vchar_array() is misread as a procedure call →
    // a spurious Unresolved node {raw_expr: "vchar_array"} appears.
    let spurious: Vec<&serde_json::Value> = nodes_of_type(&json, "unresolved")
        .into_iter()
        .filter(|n| n["raw_expr"].as_str() == Some("vchar_array"))
        .collect();

    assert!(
        spurious.is_empty(),
        "Issue #70: body inherited symbols from the stale spec (first-wins), so \
         vchar_array was unknown to the body's extraction scope and got misread as \
         a call. Found spurious Unresolved node(s): {:?}. \
         Bug: spec_items_by_pkg first-wins dedup feeds the stale spec's items to \
         the body instead of the rich spec's.",
        spurious
            .iter()
            .map(|n| format!("raw_expr={:?}", n["raw_expr"]))
            .collect::<Vec<_>>()
    );
}

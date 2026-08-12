//! Table-level lineage traversal (#115).
//!
//! `codeweb lineage <table> --direction upstream|downstream` answers "who writes this
//! table" / "where does this table flow to" in one step.

use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn codeweb_bin() -> std::path::PathBuf {
    let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    let bin_name = if cfg!(windows) {
        "codeweb.exe"
    } else {
        "codeweb"
    };
    if let Ok(entries) = std::fs::read_dir(&base) {
        for entry in entries.flatten() {
            let p = entry.path().join("debug").join(bin_name);
            if p.exists() {
                return p;
            }
        }
    }
    base.join("debug").join(bin_name)
}

fn run_codeweb_in(cwd: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(codeweb_bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("failed to run codeweb")
}

/// Lay out a project directory with `src/t.sql`, then `init` it (which also runs the
/// first full analysis). Returns the project root.
fn project_with_sql(dir: &TempDir, sql: &str) -> std::path::PathBuf {
    let root = dir.path().to_path_buf();
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("t.sql"), sql).unwrap();

    let out = run_codeweb_in(
        &root,
        &["init", "lineage-test", "--dir", src.to_str().unwrap()],
    );
    assert!(
        out.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    root
}

fn lineage(root: &Path, target: &str, direction: &str, format: &str) -> String {
    let out = run_codeweb_in(
        root,
        &[
            "lineage",
            target,
            "--direction",
            direction,
            "--format",
            format,
            "-p",
            root.to_str().unwrap(),
        ],
    );
    assert!(
        out.status.success(),
        "lineage failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// Three-stage pipeline: source_tbl -> mid_tbl -> final_tbl.
const PIPELINE_SQL: &str = r#"
CREATE TABLE source_tbl(id NUMBER, amount NUMBER);
CREATE TABLE mid_tbl(id NUMBER, total NUMBER);
CREATE TABLE final_tbl(id NUMBER, result NUMBER);

CREATE PROCEDURE prc_step1 AS BEGIN
  INSERT INTO mid_tbl(id, total)
  SELECT id, SUM(amount) FROM source_tbl GROUP BY id;
END;

CREATE PROCEDURE prc_step2 AS BEGIN
  INSERT INTO final_tbl(id, result)
  SELECT id, total FROM mid_tbl;
END;
"#;

#[test]
fn upstream_walks_back_through_writing_routines() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, PIPELINE_SQL);

    let out = lineage(&root, "final_tbl", "upstream", "tree");

    // final_tbl is written by prc_step2, which reads mid_tbl; mid_tbl is written by
    // prc_step1, which reads source_tbl.
    assert!(out.contains("final_tbl"), "missing root node:\n{out}");
    assert!(
        out.contains("prc_step2"),
        "missing writer of final_tbl:\n{out}"
    );
    assert!(
        out.contains("mid_tbl"),
        "missing 1-hop upstream table:\n{out}"
    );
    assert!(
        out.contains("prc_step1"),
        "missing writer of mid_tbl:\n{out}"
    );
    assert!(
        out.contains("source_tbl"),
        "missing 2-hop upstream table:\n{out}"
    );
}

#[test]
fn downstream_walks_forward_through_reading_routines() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, PIPELINE_SQL);

    let out = lineage(&root, "source_tbl", "downstream", "tree");

    assert!(out.contains("source_tbl"), "missing root node:\n{out}");
    assert!(
        out.contains("prc_step1"),
        "missing reader of source_tbl:\n{out}"
    );
    assert!(
        out.contains("mid_tbl"),
        "missing 1-hop downstream table:\n{out}"
    );
    assert!(
        out.contains("final_tbl"),
        "missing 2-hop downstream table:\n{out}"
    );
}

#[test]
fn upstream_and_downstream_label_the_direction_differently() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, PIPELINE_SQL);

    assert!(
        lineage(&root, "final_tbl", "upstream", "tree").contains("written by"),
        "upstream should label steps as writers"
    );
    assert!(
        lineage(&root, "source_tbl", "downstream", "tree").contains("read by"),
        "downstream should label steps as readers"
    );
}

#[test]
fn json_output_is_a_nested_node_via_children_tree() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, PIPELINE_SQL);

    let out = lineage(&root, "final_tbl", "upstream", "json");
    let json: serde_json::Value =
        serde_json::from_str(&out).expect("lineage --format json must emit valid JSON");

    assert!(json["node"].as_str().unwrap().contains("final_tbl"));
    assert!(json["via"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v.as_str().unwrap_or("").contains("prc_step2")));

    let child = &json["children"][0];
    assert!(child["node"].as_str().unwrap().contains("mid_tbl"));
    assert!(child["children"][0]["node"]
        .as_str()
        .unwrap()
        .contains("source_tbl"));
}

/// A view's base tables are upstream of it; the view is downstream of its base tables.
/// Getting this backwards was the first bug found against the real exam codebase.
#[test]
fn view_dependencies_point_from_base_table_to_view() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE base_tbl(id NUMBER, raw NUMBER);
CREATE VIEW v_derived AS SELECT id, raw * 2 AS doubled FROM base_tbl;
"#,
    );

    let up = lineage(&root, "v_derived", "upstream", "tree");
    assert!(
        up.contains("base_tbl"),
        "a view's base table is upstream of it:\n{up}"
    );

    let down = lineage(&root, "base_tbl", "downstream", "tree");
    assert!(
        down.contains("v_derived"),
        "a view that selects from a table is downstream of it:\n{down}"
    );
}

/// The traversal must not label a node as its own step (a view reaching its own base
/// tables would otherwise print `v_x [written by v_x]`).
#[test]
fn a_node_is_never_listed_as_its_own_step() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE base_tbl(id NUMBER);
CREATE VIEW v_derived AS SELECT id FROM base_tbl;
"#,
    );

    let out = lineage(&root, "v_derived", "upstream", "tree");
    for line in out.lines() {
        if let Some((node, label)) = line.split_once("  [") {
            let node = node.trim();
            assert!(!label.contains(node), "node listed as its own step: {line}");
        }
    }
}

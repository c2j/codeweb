//! Issue #154: lineage target syntax.
//! - Node key `table:schema.table` must use table-level lineage.
//! - Bare `schema.table` falls back to table-level with a transparent note.
//! - Existing `table.column` / `schema.table.column` behavior remains unchanged.

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

fn project_with_sql(dir: &TempDir, sql: &str) -> std::path::PathBuf {
    let root = dir.path().to_path_buf();
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("t.sql"), sql).unwrap();

    let out = run_codeweb_in(
        &root,
        &["init", "issue-154", "--dir", src.to_str().unwrap()],
    );
    assert!(
        out.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    root
}

const FIXTURE_SQL: &str = r#"
CREATE SCHEMA bigfund;
CREATE TABLE bigfund.mid_yjqs_detail(id NUMBER, amt NUMBER);
CREATE TABLE bigfund.out_detail(id NUMBER, amt NUMBER);
CREATE PROCEDURE bigfund.prc_load AS BEGIN
    UPDATE bigfund.mid_yjqs_detail SET amt = 0;
    INSERT INTO bigfund.out_detail SELECT id, amt FROM bigfund.mid_yjqs_detail;
END;
"#;

#[test]
fn should_treat_nodekey_target_as_table_level_lineage() {
    let tmp = TempDir::new().unwrap();
    let root = project_with_sql(&tmp, FIXTURE_SQL);
    let out = run_codeweb_in(
        &root,
        &[
            "lineage",
            "table:bigfund.mid_yjqs_detail",
            "-p",
            root.to_str().unwrap(),
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "lineage failed: {stderr}");
    assert!(
        stdout.contains("prc_load"),
        "expected table-level lineage mentioning the writer proc, got:\n{stdout}"
    );
    assert!(
        !stderr.contains("No column lineage"),
        "must not hit the column-level branch, stderr:\n{stderr}"
    );
}

#[test]
fn should_fall_back_to_table_level_for_bare_schema_qualified_target() {
    let tmp = TempDir::new().unwrap();
    let root = project_with_sql(&tmp, FIXTURE_SQL);
    let out = run_codeweb_in(
        &root,
        &[
            "lineage",
            "bigfund.mid_yjqs_detail",
            "-p",
            root.to_str().unwrap(),
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "lineage failed: {stderr}");
    assert!(
        stdout.contains("prc_load"),
        "bare schema.table should fall back to table-level lineage, got:\n{stdout}"
    );
    assert!(
        stderr.contains("interpreting") || stderr.contains("treating"),
        "fallback must emit a transparent note, stderr:\n{stderr}"
    );
}

#[test]
fn should_keep_column_level_for_existing_table_and_column() {
    let tmp = TempDir::new().unwrap();
    let root = project_with_sql(&tmp, FIXTURE_SQL);
    let out = run_codeweb_in(
        &root,
        &[
            "lineage",
            "bigfund.mid_yjqs_detail.amt",
            "-p",
            root.to_str().unwrap(),
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(
        stdout.contains("bigfund.mid_yjqs_detail.amt"),
        "column-level root line expected, got:\n{stdout}"
    );
}

#[test]
fn should_resolve_column_query_with_differently_cased_schema() {
    let tmp = TempDir::new().unwrap();
    let root = project_with_sql(&tmp, FIXTURE_SQL);
    let out = run_codeweb_in(
        &root,
        &[
            "lineage",
            "Bigfund.mid_yjqs_detail.amt",
            "-p",
            root.to_str().unwrap(),
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("interpreting"),
        "schema casing must not trigger table fallback, stderr:\n{stderr}"
    );
    assert!(
        stdout.contains("Bigfund.mid_yjqs_detail.amt"),
        "column-level root line expected, got:\n{stdout}"
    );
}

#[test]
fn should_keep_no_column_lineage_hint_when_table_exists_but_column_unknown() {
    let tmp = TempDir::new().unwrap();
    let root = project_with_sql(&tmp, FIXTURE_SQL);
    let out = run_codeweb_in(
        &root,
        &[
            "lineage",
            "bigfund.mid_yjqs_detail.nonexistent",
            "-p",
            root.to_str().unwrap(),
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success());
    assert!(stdout.contains("bigfund.mid_yjqs_detail.nonexistent"));
    assert!(
        stderr.contains("No column lineage"),
        "column branch must stay when the table resolves, stderr:\n{stderr}"
    );
}

#[test]
fn should_report_clean_error_for_unknown_nodekey_target() {
    let tmp = TempDir::new().unwrap();
    let root = project_with_sql(&tmp, FIXTURE_SQL);
    let out = run_codeweb_in(
        &root,
        &[
            "lineage",
            "table:bigfund.missing_table",
            "-p",
            root.to_str().unwrap(),
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("No column lineage"),
        "node-key miss must not produce column-branch noise, stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("No table found matching"),
        "expected table-resolution error, stderr:\n{stderr}"
    );
}

#[test]
fn should_say_ambiguous_when_table_half_is_ambiguous() {
    let tmp = TempDir::new().unwrap();
    let root = project_with_sql(
        &tmp,
        r#"
CREATE SCHEMA bigfund;
CREATE SCHEMA archive;
CREATE TABLE bigfund.mid_yjqs_detail(id NUMBER);
CREATE TABLE archive.mid_yjqs_detail(id NUMBER);
"#,
    );
    let out = run_codeweb_in(
        &root,
        &[
            "lineage",
            "mid_yjqs_detail.nonexistent_col",
            "-p",
            root.to_str().unwrap(),
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("ambiguous"),
        "ambiguous table half must be identified accurately, stderr:\n{stderr}"
    );
}

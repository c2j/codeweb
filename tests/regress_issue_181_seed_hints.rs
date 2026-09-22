//! Issue #181: `codeweb columns --format seed-hints` — the machine-readable
//! seed-data entry point. `columns --format json` already lists tables, hard
//! filters and joins, but a data generator also needs, in one document: the
//! routine signature, per-table read/write operations, cross-table equalities
//! that are expressions rather than plain column pairs, and the `operation_no`
//! enumeration with what triggers each value.
//!
//! Scope reminders from the issue: codeweb does not generate SQL and never
//! connects to a database; `seed-hints` must not become the default `columns`
//! format.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

fn codeweb_bin() -> PathBuf {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
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

fn run_codeweb_in(cwd: &Path, args: &[&str]) -> Output {
    Command::new(codeweb_bin())
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("failed to run codeweb")
}

fn project_with_sql(dir: &TempDir, sql: &str) -> PathBuf {
    let root = dir.path().to_path_buf();
    let src = root.join("sql");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("t.sql"), sql).unwrap();

    let out = run_codeweb_in(&root, &["init", "issue-181", "-d", "sql"]);
    assert!(
        out.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    root
}

fn seed_hints(root: &Path, procedure: &str) -> serde_json::Value {
    let out = run_codeweb_in(
        root,
        &[
            "columns",
            "--procedure",
            procedure,
            "--format",
            "seed-hints",
            "-p",
            ".",
        ],
    );
    assert!(
        out.status.success(),
        "columns --format seed-hints failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("seed-hints must be JSON")
}

/// One procedure touching four tables with four different operations, plus a
/// literal hard filter. The shape mirrors what a seed generator needs first:
/// which tables must have rows, and what the routine does to each.
///
/// `ops` deliberately keeps the graph's own write-kind labels rather than a
/// reduced read/write/insert/update/delete set: `insert` (hand-craftable rows)
/// and `insert_select` (rows that must come out of another query) mean different
/// things to a generator, and they are the same labels `codeweb detail` prints.
const SEED_DEMO_SQL: &str = r#"
CREATE TABLE src_orders(order_id VARCHAR(20), kind VARCHAR(10), amount NUMBER);
CREATE TABLE out_orders(order_id VARCHAR(20), kind VARCHAR(10));
CREATE TABLE arch_orders(order_id VARCHAR(20), kind VARCHAR(10));
CREATE TABLE stat_orders(order_id VARCHAR(20), total NUMBER);

CREATE PROCEDURE prc_seed_demo AS
BEGIN
  INSERT INTO out_orders(order_id, kind) VALUES ('A', '0509');

  INSERT INTO arch_orders(order_id, kind)
  SELECT o.order_id, o.kind FROM src_orders o WHERE o.kind = '0509';

  UPDATE stat_orders SET total = 1 WHERE order_id = 'X';

  DELETE FROM stat_orders WHERE order_id = 'Y';
END;
"#;

fn table_entry<'a>(hints: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    hints["tables"]
        .as_array()
        .expect("tables array")
        .iter()
        .find(|t| t["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("table {name} missing from {:#?}", hints["tables"]))
}

fn ops_of(hints: &serde_json::Value, name: &str) -> Vec<String> {
    table_entry(hints, name)["ops"]
        .as_array()
        .expect("ops array")
        .iter()
        .map(|v| v.as_str().expect("ops entries are strings").to_string())
        .collect()
}

#[test]
fn seed_hints_lists_every_table_with_its_operations() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, SEED_DEMO_SQL);

    let hints = seed_hints(&root, "prc_seed_demo");

    assert_eq!(hints["schema_version"], 1);
    assert_eq!(hints["procedure"], "prc_seed_demo");
    assert!(hints["package"].is_null());

    assert_eq!(
        ops_of(&hints, "src_orders"),
        vec!["read"],
        "a table only read must report read"
    );
    assert_eq!(
        ops_of(&hints, "out_orders"),
        vec!["insert"],
        "a literal INSERT target must report insert"
    );
    assert_eq!(
        ops_of(&hints, "arch_orders"),
        vec!["insert_select"],
        "an INSERT ... SELECT target keeps the graph's own label, so a generator \
         knows the rows must come from a query"
    );
    assert_eq!(
        ops_of(&hints, "stat_orders"),
        vec!["delete", "update"],
        "both operations on the same table must be reported, sorted"
    );
}

#[test]
fn seed_hints_carries_the_hard_filters_that_shape_the_rows() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, SEED_DEMO_SQL);

    let hints = seed_hints(&root, "prc_seed_demo");

    let filters = hints["hard_filters"]
        .as_array()
        .expect("hard_filters array");
    let kind = filters
        .iter()
        .find(|f| f["column"] == "kind")
        .unwrap_or_else(|| panic!("kind filter missing from {filters:#?}"));
    assert_eq!(kind["value"]["String"], "0509");
}

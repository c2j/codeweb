//! Column-level lineage (#136).
//!
//! `codeweb lineage <table>.<column> --direction upstream|downstream` traces one column's
//! data flow through the routines that write and read it.

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
        &["init", "col-lineage", "--dir", src.to_str().unwrap()],
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

/// Two stages, so a column can be traced across more than one hop:
///   raw_trade.qty, .price -> mid_trade.gross -> fund_report.net, .total_gross
const PIPELINE_SQL: &str = r#"
CREATE TABLE raw_trade(id NUMBER, qty NUMBER, price NUMBER, fee NUMBER);
CREATE TABLE mid_trade(id NUMBER, gross NUMBER, fee NUMBER);
CREATE TABLE fund_report(id NUMBER, net NUMBER, total_gross NUMBER);

CREATE PROCEDURE prc_stage1 AS BEGIN
  INSERT INTO mid_trade(id, gross, fee)
  SELECT id, qty * price, fee FROM raw_trade;
END;

CREATE PROCEDURE prc_stage2 AS BEGIN
  INSERT INTO fund_report(id, net, total_gross)
  SELECT id, gross - fee, SUM(gross) FROM mid_trade GROUP BY id;
END;
"#;

#[test]
fn upstream_reaches_the_original_source_columns() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, PIPELINE_SQL);

    let out = lineage(&root, "fund_report.net", "upstream", "tree");

    // net = gross - fee, and gross = qty * price one stage earlier.
    assert!(
        out.contains("mid_trade.gross"),
        "1-hop source missing:\n{out}"
    );
    assert!(
        out.contains("mid_trade.fee"),
        "1-hop source missing:\n{out}"
    );
    assert!(
        out.contains("raw_trade.qty"),
        "2-hop source missing:\n{out}"
    );
    assert!(
        out.contains("raw_trade.price"),
        "2-hop source missing:\n{out}"
    );
    assert!(
        out.contains("prc_stage2"),
        "writing routine missing:\n{out}"
    );
    assert!(
        out.contains("prc_stage1"),
        "upstream routine missing:\n{out}"
    );
}

#[test]
fn downstream_reaches_every_column_the_value_ends_up_in() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, PIPELINE_SQL);

    let out = lineage(&root, "raw_trade.qty", "downstream", "tree");

    assert!(
        out.contains("mid_trade.gross"),
        "1-hop target missing:\n{out}"
    );
    assert!(
        out.contains("fund_report.net"),
        "2-hop target missing:\n{out}"
    );
    assert!(
        out.contains("fund_report.total_gross"),
        "2-hop aggregate target missing:\n{out}"
    );
}

#[test]
fn direction_is_visible_in_the_arrow() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, PIPELINE_SQL);

    assert!(
        lineage(&root, "fund_report.net", "upstream", "tree").contains('←'),
        "upstream should point back at its sources"
    );
    assert!(
        lineage(&root, "raw_trade.qty", "downstream", "tree").contains('→'),
        "downstream should point forward at its targets"
    );
}

#[test]
fn aggregate_and_derived_kinds_are_labelled() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, PIPELINE_SQL);

    let agg = lineage(&root, "fund_report.total_gross", "upstream", "tree");
    assert!(agg.contains("[SUM]"), "aggregate not labelled:\n{agg}");

    let derived = lineage(&root, "fund_report.net", "upstream", "tree");
    assert!(
        derived.contains("[derived]"),
        "computed column not labelled:\n{derived}"
    );
    assert!(
        derived.contains("gross - fee"),
        "expression text missing:\n{derived}"
    );
}

/// A column copied without transformation should be reported as a direct copy, and carry
/// no expression text — there is no computation to explain.
#[test]
fn a_plain_copy_is_direct_with_no_expression() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE src_tbl(a NUMBER);
CREATE TABLE dst_tbl(b NUMBER);
CREATE PROCEDURE prc_copy AS BEGIN
  INSERT INTO dst_tbl(b) SELECT a FROM src_tbl;
END;
"#,
    );

    let json: serde_json::Value =
        serde_json::from_str(&lineage(&root, "dst_tbl.b", "upstream", "json")).unwrap();

    assert_eq!(json["table"], "dst_tbl");
    assert_eq!(json["column"], "b");
    let step = &json["steps"][0];
    assert_eq!(step["source"], "src_tbl.a");
    assert_eq!(step["kind"], "direct");
    assert!(step["expression"].is_null(), "got {step:#?}");
}

#[test]
fn json_output_nests_each_hop_under_next() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, PIPELINE_SQL);

    let json: serde_json::Value =
        serde_json::from_str(&lineage(&root, "fund_report.net", "upstream", "json")).unwrap();

    let first = &json["steps"][0];
    assert_eq!(first["source"], "mid_trade.gross");
    let nested = &first["next"];
    assert_eq!(nested["table"], "mid_trade");
    assert_eq!(nested["column"], "gross");
    let deeper: Vec<&str> = nested["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["source"].as_str().unwrap())
        .collect();
    assert!(deeper.contains(&"raw_trade.qty"), "got {deeper:?}");
    assert!(deeper.contains(&"raw_trade.price"), "got {deeper:?}");
}

/// A constant column has no upstream column, and saying so is more useful than saying
/// nothing.
#[test]
fn a_constant_column_reports_its_literal() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE flags(id NUMBER, is_voucher VARCHAR2(1));
CREATE TABLE src_tbl(id NUMBER);
CREATE PROCEDURE prc_flag AS BEGIN
  INSERT INTO flags(id, is_voucher) SELECT id, '0' FROM src_tbl;
END;
"#,
    );

    let out = lineage(&root, "flags.is_voucher", "upstream", "tree");
    assert!(out.contains("literal '0'"), "got:\n{out}");
}

/// A PL/SQL variable terminates the walk rather than being silently dropped, so the gap
/// is visible. Resolving these back to columns is the remaining piece of #136.
#[test]
fn an_unresolved_variable_is_reported_not_dropped() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE t_amount(id NUMBER, amount NUMBER, total NUMBER);
CREATE PROCEDURE prc_var AS
  v_sum NUMBER;
BEGIN
  SELECT SUM(amount) INTO v_sum FROM t_amount;
  UPDATE t_amount SET total = nvl(v_sum, 0);
END;
"#,
    );

    let out = lineage(&root, "t_amount.total", "upstream", "tree");
    assert!(
        out.contains("variable v_sum"),
        "the variable hop should be visible:\n{out}"
    );
}

/// A self-referencing UPDATE would otherwise recurse forever through the same column.
#[test]
fn a_self_referencing_update_terminates() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE running(id NUMBER, total NUMBER, delta NUMBER);
CREATE PROCEDURE prc_accumulate AS BEGIN
  UPDATE running SET total = total + delta;
END;
"#,
    );

    let out = lineage(&root, "running.total", "upstream", "tree");
    assert!(out.contains("running.delta"), "got:\n{out}");
    assert!(
        out.lines().count() < 30,
        "self-reference should not recurse without bound, got {} lines:\n{out}",
        out.lines().count()
    );
}

/// Each union branch feeds the same target column, so all of them must appear.
#[test]
fn every_union_branch_shows_up_as_a_source() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE t1(a NUMBER);
CREATE TABLE t2(b NUMBER);
CREATE TABLE t3(c NUMBER);
CREATE TABLE dst(v NUMBER);
CREATE PROCEDURE prc_union AS BEGIN
  INSERT INTO dst(v)
  SELECT a FROM t1 UNION ALL SELECT b FROM t2 UNION ALL SELECT c FROM t3;
END;
"#,
    );

    let out = lineage(&root, "dst.v", "upstream", "tree");
    for expected in ["t1.a", "t2.b", "t3.c"] {
        assert!(out.contains(expected), "{expected} missing:\n{out}");
    }
}

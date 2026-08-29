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

/// A cursor's SELECT feeds its FETCH variables, which feed `INSERT ... VALUES` — the
/// whole chain must resolve back to the cursor's source columns.
#[test]
fn cursor_fetch_insert_values_resolves_to_source_columns() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE src_cursor(id NUMBER, amount NUMBER);
CREATE TABLE dst_cursor(id NUMBER, total NUMBER);
CREATE PROCEDURE prc_cursor AS
  CURSOR c IS SELECT id, amount FROM src_cursor;
  v_id NUMBER;
  v_amount NUMBER;
BEGIN
  OPEN c;
  LOOP
    FETCH c INTO v_id, v_amount;
    EXIT WHEN c%NOTFOUND;
    INSERT INTO dst_cursor(id, total) VALUES (v_id, v_amount);
  END LOOP;
  CLOSE c;
END;
"#,
    );

    let out = lineage(&root, "dst_cursor.total", "upstream", "tree");
    assert!(
        out.contains("src_cursor.amount"),
        "cursor source column missing:\n{out}"
    );
}

/// #142: a scalar subquery in the INSERT..SELECT target list must resolve to the
/// subquery's source column, not report "No column lineage".
#[test]
fn scalar_subquery_in_insert_select_target_resolves() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE t_src(id NUMBER, amt NUMBER);
CREATE TABLE t_ref(id NUMBER, code VARCHAR2(10));
CREATE TABLE t_out(id NUMBER, code VARCHAR2(10));
CREATE PROCEDURE p_copy_subquery AS BEGIN
  INSERT INTO t_out (id, code)
  SELECT s.id, (SELECT r.code FROM t_ref r WHERE r.id = s.id) FROM t_src s;
END;
"#,
    );
    let out = lineage(&root, "t_out.code", "upstream", "tree");
    assert!(
        !out.contains("No column lineage"),
        "scalar subquery target must resolve:\n{out}"
    );
    assert!(
        out.contains("t_ref.code"),
        "subquery source column missing:\n{out}"
    );
}

/// #142: a table-anchored %ROWTYPE record (`r t_src%ROWTYPE`) written via
/// `VALUES (r.id, r.amt)` must resolve to t_src columns, not "?.id".
#[test]
fn table_rowtype_record_insert_values_resolves_to_table() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE t_src(id NUMBER, amt NUMBER);
CREATE TABLE t_dst(id NUMBER, amt NUMBER);
CREATE PROCEDURE p_table_rowtype AS
  r t_src%ROWTYPE;
  CURSOR cur IS SELECT id, amt FROM t_src;
BEGIN
  OPEN cur;
  LOOP
    FETCH cur INTO r;
    EXIT WHEN cur%NOTFOUND;
    INSERT INTO t_dst (id, amt) VALUES (r.id, r.amt);
  END LOOP;
  CLOSE cur;
END;
"#,
    );
    let out = lineage(&root, "t_dst.id", "upstream", "tree");
    assert!(
        out.contains("t_src.id"),
        "table-anchored record field must resolve:\n{out}"
    );
    assert!(
        !out.contains("?.id"),
        "table-anchored record field must not stay unattributed:\n{out}"
    );
}

/// #142: `SELECT *` cursor + `%ROWTYPE` record fields must resolve to the
/// cursor's table (columns attributed under the field names), not "?.id".
#[test]
fn star_cursor_rowtype_record_resolves_to_cursor_table() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE t_src(id NUMBER, amt NUMBER);
CREATE TABLE t_dst(id NUMBER, amt NUMBER);
CREATE PROCEDURE p_star_cursor AS
  CURSOR cur IS SELECT * FROM t_src;
  r cur%ROWTYPE;
BEGIN
  OPEN cur;
  LOOP
    FETCH cur INTO r;
    EXIT WHEN cur%NOTFOUND;
    INSERT INTO t_dst (id, amt) VALUES (r.id, r.amt);
  END LOOP;
  CLOSE cur;
END;
"#,
    );
    let out = lineage(&root, "t_dst.id", "upstream", "tree");
    assert!(
        out.contains("t_src.id"),
        "star-cursor record field must resolve:\n{out}"
    );
    assert!(
        !out.contains("?.id"),
        "star-cursor record field must not stay unattributed:\n{out}"
    );
}

/// Regression: a cursor declared with `SELECT *` resolves to zero source columns, so a
/// later `FETCH` used to panic in `resolve_cursor_flows` — `bool::then_some` evaluates
/// its argument eagerly, indexing `&cols[0]` on the empty list
/// ("index out of bounds: the len is 0 but the index is 0"). The chain must survive and
/// the cursor's source table must still flow upstream.
#[test]
fn star_cursor_fetch_does_not_panic_on_empty_sources() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE src_star(id NUMBER, amount NUMBER);
CREATE TABLE dst_star(id NUMBER, total NUMBER);
CREATE PROCEDURE prc_star AS
  CURSOR c IS SELECT * FROM src_star;
  v_id NUMBER;
  v_amount NUMBER;
BEGIN
  OPEN c;
  LOOP
    FETCH c INTO v_id, v_amount;
    EXIT WHEN c%NOTFOUND;
    INSERT INTO dst_star(id, total) VALUES (v_id, v_amount);
  END LOOP;
  CLOSE c;
END;
"#,
    );

    let out = run_codeweb_in(&root, &["analyze"]);
    assert!(
        out.status.success(),
        "analyze must not panic on a SELECT * cursor:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let tbl = lineage(&root, "dst_star", "upstream", "tree");
    assert!(
        tbl.contains("table:src_star"),
        "cursor source table must be upstream:\n{tbl}"
    );
}

/// A view's body is the definition of its columns, so column lineage must resolve
/// through the view to its base table.
#[test]
fn view_column_lineage_resolves_through_the_view() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE base_v(a NUMBER, b NUMBER);
CREATE VIEW v_dbl AS SELECT a * 2 AS x, b AS y FROM base_v;
CREATE TABLE dst_view(x NUMBER, y NUMBER);
CREATE PROCEDURE prc_view AS BEGIN
  INSERT INTO dst_view(x, y) SELECT x, y FROM v_dbl;
END;
"#,
    );

    let out = lineage(&root, "dst_view.x", "upstream", "tree");
    assert!(out.contains("base_v.a"), "view base column missing:\n{out}");
    assert!(
        out.contains("a * 2"),
        "view expression text missing:\n{out}"
    );
}

/// A MERGE statement's WHEN MATCHED UPDATE and WHEN NOT MATCHED INSERT clauses both
/// produce column mappings.
#[test]
fn merge_produces_column_mappings() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE m_src(id NUMBER, val NUMBER);
CREATE TABLE m_tgt(id NUMBER, val NUMBER);
CREATE PROCEDURE prc_merge AS BEGIN
  MERGE INTO m_tgt t
  USING m_src s ON (t.id = s.id)
  WHEN MATCHED THEN UPDATE SET val = s.val
  WHEN NOT MATCHED THEN INSERT (id, val) VALUES (s.id, s.val);
END;
"#,
    );

    let out = lineage(&root, "m_tgt.val", "upstream", "tree");
    assert!(
        out.contains("m_src.val"),
        "merge source column missing:\n{out}"
    );
}

/// `UPDATE ... FROM` source columns must be attributed to the FROM table, not the
/// UPDATE target table.
#[test]
fn update_from_attributes_source_columns() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE u_tgt(id NUMBER, v NUMBER);
CREATE TABLE u_src(id NUMBER, v NUMBER);
CREATE PROCEDURE prc_upd AS BEGIN
  UPDATE u_tgt t SET v = s.v FROM u_src s WHERE t.id = s.id;
END;
"#,
    );

    let out = lineage(&root, "u_tgt.v", "upstream", "tree");
    assert!(
        out.contains("u_src.v"),
        "UPDATE ... FROM source column missing:\n{out}"
    );
}

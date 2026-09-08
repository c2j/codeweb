//! #165 (P0): `codeweb columns --procedure <name> --format json` — per-procedure
//! aggregated `ColumnAnalysis` export.

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
        &["init", "col-analysis", "--dir", src.to_str().unwrap()],
    );
    assert!(
        out.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    root
}

/// A procedure with two `INSERT` statements writing the same output table
/// (`mid_yjqs_detail`) from the same two source tables (`s1_src`, `par_fund_partner`),
/// joined the same way but each guarded by a different `WHERE` filter — one a plain
/// literal comparison (`scdm = '001'`), the other a #169 whitelisted-transform
/// comparison (`substr(stock_kind, 1, 2) = '05'`). Both statements' `TableAccess` edges
/// carry the same join condition, so aggregation must report it once, not twice.
const CURSOR_AND_JOIN_SQL: &str = r#"
CREATE TABLE mid_yjqs_detail(security_id VARCHAR(20), partner_no VARCHAR(20));
CREATE TABLE par_fund_partner(fund_code VARCHAR(20), partner_no VARCHAR(20));
CREATE TABLE s1_src(fund_code VARCHAR(20), scdm VARCHAR(10), stock_kind VARCHAR(10));

CREATE PROCEDURE prc_trd_hz_byfund AS
BEGIN
  INSERT INTO mid_yjqs_detail(security_id, partner_no)
  SELECT c.fund_code, f.partner_no
  FROM s1_src c, par_fund_partner f
  WHERE f.fund_code = c.fund_code AND c.scdm = '001';

  INSERT INTO mid_yjqs_detail(security_id, partner_no)
  SELECT c.fund_code, f.partner_no
  FROM s1_src c, par_fund_partner f
  WHERE f.fund_code = c.fund_code AND substr(c.stock_kind,1,2) = '05';
END;
"#;

fn columns_json(root: &Path, extra_args: &[&str]) -> std::process::Output {
    let mut args = vec!["columns", "-p"];
    let root_str = root.to_str().unwrap();
    args.push(root_str);
    args.extend_from_slice(extra_args);
    run_codeweb_in(root, &args)
}

#[test]
fn columns_json_lists_hard_filters_and_joins_without_duplicates() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, CURSOR_AND_JOIN_SQL);

    let out = columns_json(&root, &["--procedure", "prc_trd_hz_byfund"]);
    assert!(
        out.status.success(),
        "columns failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();

    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["procedure"], "prc_trd_hz_byfund");
    assert!(json["package"].is_null());

    let hard_filters = json["hard_filters"].as_array().expect("hard_filters array");
    assert_eq!(
        hard_filters.len(),
        2,
        "expected exactly 2 distinct hard filters (scdm + substr), got: {hard_filters:#?}"
    );

    let scdm_count = hard_filters
        .iter()
        .filter(|f| f["column"] == "scdm" && f["value"]["String"] == "001")
        .count();
    assert_eq!(
        scdm_count, 1,
        "scdm='001' filter must appear exactly once, got {scdm_count} in {hard_filters:#?}"
    );

    let substr_filter = hard_filters
        .iter()
        .find(|f| f["column"] == "stock_kind")
        .expect("stock_kind filter present");
    assert_eq!(substr_filter["value"]["String"], "05");
    assert_eq!(substr_filter["transform"]["fn"], "substr");
    assert_eq!(substr_filter["transform"]["args"][0]["Integer"], 1);
    assert_eq!(substr_filter["transform"]["args"][1]["Integer"], 2);

    let join_conditions = json["join_conditions"]
        .as_array()
        .expect("join_conditions array");
    assert_eq!(
        join_conditions.len(),
        1,
        "the same join from both statements must dedup to one row, got: {join_conditions:#?}"
    );
    let jc = &join_conditions[0];
    let tables: Vec<&str> = vec![
        jc["left_table"].as_str().unwrap(),
        jc["right_table"].as_str().unwrap(),
    ];
    assert!(
        tables.contains(&"par_fund_partner") && tables.contains(&"s1_src"),
        "join should be between par_fund_partner and s1_src, got: {jc:#?}"
    );
    assert_eq!(jc["left_column"], "fund_code");
    assert_eq!(jc["right_column"], "fund_code");

    let read_tables: Vec<&str> = json["read_tables"]
        .as_array()
        .expect("read_tables array")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        read_tables.contains(&"s1_src") && read_tables.contains(&"par_fund_partner"),
        "read_tables should include both source tables, got: {read_tables:?}"
    );
}

#[test]
fn columns_json_table_filter_narrows_output() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, CURSOR_AND_JOIN_SQL);

    let out = columns_json(
        &root,
        &[
            "--procedure",
            "prc_trd_hz_byfund",
            "--table",
            "par_fund_partner",
        ],
    );
    assert!(
        out.status.success(),
        "columns failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();

    let tables: Vec<&str> = json["tables"]
        .as_array()
        .expect("tables array")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        tables,
        vec!["par_fund_partner"],
        "--table should narrow tables[] to the one requested table"
    );

    // The table's own statement-level constraints (the join to its partner table)
    // survive the narrowing — only `tables`/`read_tables` narrow, not join/filter rows.
    let join_conditions = json["join_conditions"]
        .as_array()
        .expect("join_conditions array");
    assert_eq!(
        join_conditions.len(),
        1,
        "join constraint touching the filtered table should still be reported"
    );
}

#[test]
fn columns_unknown_procedure_errors_cleanly() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, CURSOR_AND_JOIN_SQL);

    let out = columns_json(&root, &["--procedure", "nonexistent_proc_xyz"]);
    assert!(
        !out.status.success(),
        "unknown procedure should exit non-zero"
    );
    assert!(
        out.stdout.is_empty(),
        "unknown procedure must not print JSON to stdout, got: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        !out.stderr.is_empty(),
        "unknown procedure should print an error to stderr"
    );
}

#[test]
fn columns_ambiguous_substring_fails_explicitly() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE PROCEDURE prc_order AS BEGIN NULL; END;
CREATE PROCEDURE prc_order_header AS BEGIN NULL; END;
"#,
    );

    let out = columns_json(&root, &["--procedure", "prc_order"]);

    assert!(!out.status.success(), "ambiguous query should fail");
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .to_lowercase()
            .contains("ambiguous"),
        "stderr should explain ambiguity: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// #168: a `%ROWTYPE` record fetched from a cursor over a "detail" table
/// (`mid_yjqs_detail`), referenced in a `SELECT ... INTO` WHERE clause against a
/// dimension table (`par_sys_purchase`), must surface as a cross-table
/// `JoinCondition` tagged `"RecordField"` in `columns --format json`.
const STEP3_DIM_TABLE_JOIN_SQL: &str = r#"
CREATE TABLE mid_yjqs_detail(security_id VARCHAR(20), fund_code VARCHAR(20));
CREATE TABLE par_sys_purchase(security_id VARCHAR(20), purchase_days NUMBER);

CREATE OR REPLACE PROCEDURE prc_step3_dim_join AS
  CURSOR c_get_data IS SELECT security_id, fund_code FROM mid_yjqs_detail;
  r_get_purchase c_get_data%ROWTYPE;
  v_purchase_days NUMBER;
BEGIN
  OPEN c_get_data;
  FETCH c_get_data INTO r_get_purchase;
  SELECT t.purchase_days INTO v_purchase_days FROM par_sys_purchase t
    WHERE t.security_id = r_get_purchase.security_id;
  CLOSE c_get_data;
END;
"#;

#[test]
fn columns_json_reports_record_field_cross_table_join() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, STEP3_DIM_TABLE_JOIN_SQL);

    let out = columns_json(&root, &["--procedure", "prc_step3_dim_join"]);
    assert!(
        out.status.success(),
        "columns failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();

    let join_conditions = json["join_conditions"]
        .as_array()
        .expect("join_conditions array");
    let jc = join_conditions
        .iter()
        .find(|jc| jc["source"] == "RecordField")
        .unwrap_or_else(|| panic!("no RecordField join in {join_conditions:#?}"));

    let tables: Vec<&str> = vec![
        jc["left_table"].as_str().unwrap(),
        jc["right_table"].as_str().unwrap(),
    ];
    assert!(
        tables.contains(&"par_sys_purchase") && tables.contains(&"mid_yjqs_detail"),
        "join should cross par_sys_purchase and mid_yjqs_detail, got: {jc:#?}"
    );
    assert_eq!(jc["left_column"], "security_id");
    assert_eq!(jc["right_column"], "security_id");
}

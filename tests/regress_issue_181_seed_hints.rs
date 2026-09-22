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

fn seed_hints_with(root: &Path, procedure: &str, discriminators: &[&str]) -> serde_json::Value {
    let mut args = vec![
        "columns",
        "--procedure",
        procedure,
        "--format",
        "seed-hints",
        "-p",
        ".",
    ];
    for d in discriminators {
        args.push("--discriminator");
        args.push(d);
    }
    let out = run_codeweb_in(root, &args);
    assert!(
        out.status.success(),
        "columns --format seed-hints failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("seed-hints must be JSON")
}

/// The issue's own case: `operation_no` is a configured discriminator column.
fn seed_hints(root: &Path, procedure: &str) -> serde_json::Value {
    seed_hints_with(root, procedure, &["operation_no"])
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
    assert_eq!(
        hints["kind"], "predicate_inventory",
        "the document must self-describe as the predicate inventory"
    );
    assert!(
        hints["caveat"]
            .as_str()
            .unwrap_or_default()
            .contains("not a specification"),
        "the caveat must state the necessary-not-sufficient contract, got {}",
        hints["caveat"]
    );
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
    assert_eq!(
        kind["confidence"], "high",
        "a filter attributed to a table is high confidence, got {kind}"
    );
    assert_eq!(kind["table"], "src_orders");
    let provenance = &kind["provenance"];
    assert!(
        provenance["file"]
            .as_str()
            .unwrap_or_default()
            .ends_with(".sql"),
        "provenance must name the source file, got {provenance}"
    );
    assert!(
        provenance["line"].as_u64().unwrap_or(0) > 0,
        "provenance must carry a 1-based line, got {provenance}"
    );
}

/// A procedure whose signature the seed generator has to satisfy (the issue
/// names `p_i_date` specifically: the date parameter is what a caller has to
/// pick before any row can be seeded).
const SEED_PARAM_SQL: &str = r#"
CREATE TABLE out_orders(order_id VARCHAR(20), kind VARCHAR(10));

CREATE PROCEDURE prc_seed_params(
  p_i_date VARCHAR2,
  p_i_bs   VARCHAR2,
  p_o_cnt  OUT NUMBER
) AS
BEGIN
  INSERT INTO out_orders(order_id, kind) VALUES (p_i_date, p_i_bs);
END;
"#;

#[test]
fn seed_hints_reports_the_declared_signature() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, SEED_PARAM_SQL);

    let hints = seed_hints(&root, "prc_seed_params");

    let routines = hints["routines"]
        .as_array()
        .unwrap_or_else(|| panic!("routines array missing from {hints:#?}"));
    assert_eq!(
        routines.len(),
        1,
        "one procedure, one signature: {routines:#?}"
    );
    assert_eq!(routines[0]["routine"], "prc_seed_params");

    let params = routines[0]["parameters"]
        .as_array()
        .unwrap_or_else(|| panic!("parameters array missing from {routines:#?}"));
    let names: Vec<&str> = params.iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        vec!["p_i_date", "p_i_bs", "p_o_cnt"],
        "parameters must keep signature order, got {params:#?}"
    );
    // The parser normalizes keyword types to lowercase; the hint reports what the
    // AST holds rather than re-casing it.
    assert_eq!(params[0]["data_type"], "varchar2");
    assert!(
        params[0]["mode"].is_null(),
        "a parameter with no mode keyword has no mode, got {}",
        params[0]["mode"]
    );
    assert_eq!(params[2]["mode"], "OUT");
    assert_eq!(params[2]["data_type"], "number");
    assert!(params[0]["default_value"].is_null());
}

/// Package mode must keep one signature per sub-routine, each named, rather than
/// flat-merging every parameter into one synthetic signature. Two routines that
/// both declare `p_i_date` would otherwise be indistinguishable.
const SEED_PACKAGE_SQL: &str = r#"
CREATE TABLE out_orders(order_id VARCHAR(20), kind VARCHAR(10));

CREATE PACKAGE pkg_seed AS
  PROCEDURE prc_a(p_i_date VARCHAR2);
  PROCEDURE prc_b(p_i_date VARCHAR2, p_i_kind VARCHAR2);
END pkg_seed;
/

CREATE PACKAGE BODY pkg_seed AS
  PROCEDURE prc_a(p_i_date VARCHAR2) AS
  BEGIN
    INSERT INTO out_orders(order_id, kind) VALUES (p_i_date, 'A');
  END;

  PROCEDURE prc_b(p_i_date VARCHAR2, p_i_kind VARCHAR2) AS
  BEGIN
    INSERT INTO out_orders(order_id, kind) VALUES (p_i_date, p_i_kind);
  END;
END pkg_seed;
"#;

#[test]
fn seed_hints_keeps_one_signature_per_routine_in_package_mode() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, SEED_PACKAGE_SQL);

    let out = run_codeweb_in(
        &root,
        &[
            "columns",
            "--package",
            "pkg_seed",
            "--format",
            "seed-hints",
            "-p",
            ".",
        ],
    );
    assert!(
        out.status.success(),
        "package seed-hints failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hints: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .expect("seed-hints must be JSON");

    let routines = hints["routines"]
        .as_array()
        .unwrap_or_else(|| panic!("routines array missing from {hints:#?}"));
    let names: Vec<&str> = routines
        .iter()
        .map(|r| r["routine"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec!["prc_a", "prc_b"],
        "each sub-routine keeps its own named signature, sorted, got {routines:#?}"
    );
    // prc_a declares one parameter, prc_b two: they must not be merged.
    assert_eq!(routines[0]["parameters"].as_array().unwrap().len(), 1);
    assert_eq!(routines[1]["parameters"].as_array().unwrap().len(), 2);
}

/// The two cross-table equalities the issue calls out by name: a function-wrapped
/// key (`substr(c.trade_no, -3) = r.check_type`) and an arithmetic one
/// (`abs(c.vol * 1000) = abs(r.cjsl)`). Neither is a plain `column = column`
/// pair, so neither shows up in `join_conditions`, yet a seed generator has to
/// satisfy both.
const SEED_EQUALITY_SQL: &str = r#"
CREATE TABLE zgh_temp(trade_no VARCHAR(30), vol NUMBER);
CREATE TABLE out_trd(check_type VARCHAR(10), cjsl NUMBER);

CREATE PROCEDURE prc_seed_eq AS
BEGIN
  INSERT INTO out_trd(check_type, cjsl)
  SELECT substr(c.trade_no, -3), abs(c.vol * 1000)
  FROM zgh_temp c, out_trd r
  WHERE substr(c.trade_no, decode(sign(length(c.trade_no) - 3), -1, 0, -3)) = r.check_type
    AND abs(c.vol * 1000) = abs(r.cjsl);
END;
"#;

fn equality_mentioning<'a>(hints: &'a serde_json::Value, needle: &str) -> &'a serde_json::Value {
    hints["cross_table_equalities"]
        .as_array()
        .expect("cross_table_equalities array")
        .iter()
        .find(|e| {
            let left = e["left"]["expression"].as_str().unwrap_or_default();
            let right = e["right"]["expression"].as_str().unwrap_or_default();
            left.contains(needle) || right.contains(needle)
        })
        .unwrap_or_else(|| panic!("no equality mentioning {needle} in {:#?}", hints))
}

#[test]
fn seed_hints_reports_cross_table_equalities_that_are_expressions() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, SEED_EQUALITY_SQL);

    let hints = seed_hints(&root, "prc_seed_eq");

    let trade = equality_mentioning(&hints, "trade_no");
    assert_eq!(trade["left"]["table"], "zgh_temp");
    assert_eq!(trade["left"]["column"], "trade_no");
    assert!(
        trade["left"]["expression"]
            .as_str()
            .unwrap()
            .starts_with("substr("),
        "the wrapper must be preserved, got {trade}"
    );
    assert_eq!(trade["right"]["table"], "out_trd");
    assert_eq!(trade["right"]["column"], "check_type");
    assert_eq!(
        trade["confidence"], "high",
        "a resolved cross-table equality is high confidence, got {trade}"
    );
    assert!(
        trade["provenance"]["line"].as_u64().unwrap_or(0) > 0,
        "an equality must carry the statement's line, got {trade}"
    );

    let vol = equality_mentioning(&hints, "vol");
    assert_eq!(vol["left"]["table"], "zgh_temp");
    assert_eq!(vol["left"]["column"], "vol");
    assert!(
        vol["left"]["expression"].as_str().unwrap().contains("1000"),
        "the arithmetic wrapper must be preserved, got {vol}"
    );
    assert_eq!(vol["right"]["table"], "out_trd");
    assert_eq!(vol["right"]["column"], "cjsl");
}

/// The acceptance shape resolves one side through a `%ROWTYPE` record
/// (`r_bond_repurchase.check_type`), not a plain table alias. Lock that path: an
/// alias-only fixture would still pass if record-field resolution broke.
const SEED_EQUALITY_RECORD_SQL: &str = r#"
CREATE TABLE zgh_temp(trade_no VARCHAR(30), vol NUMBER);
CREATE TABLE dat_fund_cjqs(check_type VARCHAR(10), cjsl NUMBER);

CREATE PROCEDURE prc_seed_eq_rec AS
  CURSOR c_cur IS
    SELECT d.check_type AS check_type, d.cjsl AS cjsl FROM dat_fund_cjqs d;
  r_rec c_cur%ROWTYPE;
BEGIN
  OPEN c_cur;
  FETCH c_cur INTO r_rec;
  INSERT INTO zgh_temp(trade_no, vol)
  SELECT c.trade_no, c.vol FROM zgh_temp c
  WHERE substr(c.trade_no, decode(sign(length(c.trade_no) - 3), -1, 0, -3)) = r_rec.check_type
    AND abs(c.vol * 1000) = abs(r_rec.cjsl);
  CLOSE c_cur;
END;
"#;

#[test]
fn seed_hints_resolves_a_record_field_equality_side() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, SEED_EQUALITY_RECORD_SQL);

    let hints = seed_hints(&root, "prc_seed_eq_rec");

    let trade = equality_mentioning(&hints, "trade_no");
    assert_eq!(trade["left"]["table"], "zgh_temp");
    assert_eq!(trade["left"]["column"], "trade_no");
    assert_eq!(
        trade["right"]["table"], "dat_fund_cjqs",
        "the record field must resolve to its cursor's source table, got {trade}"
    );
    assert_eq!(trade["right"]["column"], "check_type");
}

/// A plain `column = column` pair is already a join condition; it must not be
/// duplicated into the expression list.
#[test]
fn seed_hints_does_not_duplicate_plain_join_columns_as_equalities() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE a_tbl(k VARCHAR(10), v NUMBER);
CREATE TABLE b_tbl(k VARCHAR(10), v NUMBER);

CREATE PROCEDURE prc_seed_plain AS
BEGIN
  INSERT INTO b_tbl(k, v)
  SELECT a.k, a.v FROM a_tbl a, b_tbl b WHERE a.k = b.k;
END;
"#,
    );

    let hints = seed_hints(&root, "prc_seed_plain");

    assert_eq!(
        hints["cross_table_equalities"].as_array().unwrap().len(),
        0,
        "a plain column = column pair belongs to join_conditions, got {:#?}",
        hints["cross_table_equalities"]
    );
}

/// `operation_no` is the discriminator the issue's migration hinged on: a seed
/// generator has to pick a value that walks the branch it wants. The values come
/// from two places — a `DECODE` mapping (the cursor's enum) and a PL branch
/// condition — and the second one carries the trigger.
const SEED_OPERATION_NO_SQL: &str = r#"
CREATE TABLE src_op(operation_no VARCHAR(20), amount NUMBER);
CREATE TABLE out_op(bs VARCHAR(20));

CREATE PROCEDURE prc_seed_opno AS
  CURSOR c_cur IS
    SELECT t.operation_no AS operation_no FROM src_op t;
  r_rec c_cur%ROWTYPE;
BEGIN
  INSERT INTO out_op(bs)
  SELECT DECODE(operation_no, '0112004001', 'A', '0111004001', 'B', 'Z') FROM src_op;

  OPEN c_cur;
  FETCH c_cur INTO r_rec;
  IF r_rec.operation_no = '0110999001' THEN
    INSERT INTO out_op(bs) VALUES ('hit');
  END IF;
  CLOSE c_cur;
END;
"#;

fn discriminator_entry<'a>(
    hints: &'a serde_json::Value,
    value: &str,
) -> Option<&'a serde_json::Value> {
    hints["discriminator_values"]
        .as_array()
        .expect("discriminator_values array")
        .iter()
        .find(|v| v["value"].as_str() == Some(value))
}

/// A `%ROWTYPE` field whose cursor *projects* it under a different name must
/// still be matched by the field name — and a sibling clause on another column,
/// or a `<>` comparison, must not be attributed to the discriminator.
const SEED_DISCRIMINATOR_RENAME_SQL: &str = r#"
CREATE TABLE src_op(bs VARCHAR(20), stock_kind VARCHAR(10));
CREATE TABLE out_op(x VARCHAR(20));

CREATE PROCEDURE prc_seed_rename AS
  CURSOR c_cur IS
    SELECT t.bs AS operation_no, t.stock_kind AS stock_kind FROM src_op t;
  r_rec c_cur%ROWTYPE;
BEGIN
  OPEN c_cur;
  FETCH c_cur INTO r_rec;
  IF r_rec.operation_no <> 'BAD' AND r_rec.stock_kind = 'OK' THEN
    INSERT INTO out_op(x) VALUES ('a');
  END IF;
  IF r_rec.operation_no = '0110999001' THEN
    INSERT INTO out_op(x) VALUES ('b');
  END IF;
  CLOSE c_cur;
END;
"#;

#[test]
fn seed_hints_matches_the_record_field_name_not_the_projected_column() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, SEED_DISCRIMINATOR_RENAME_SQL);

    let hints = seed_hints(&root, "prc_seed_rename");
    let values: Vec<&str> = hints["discriminator_values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["value"].as_str().unwrap())
        .collect();

    assert!(
        values.contains(&"0110999001"),
        "the field name must match even though the cursor projects it as `bs`, got {values:?}"
    );
    assert!(
        !values.contains(&"OK"),
        "a sibling clause on another column must not be attributed to the discriminator, got {values:?}"
    );
    assert!(
        !values.contains(&"BAD"),
        "a `<>` value must not be enumerated, got {values:?}"
    );

    let entry = discriminator_entry(&hints, "0110999001").unwrap();
    assert!(
        entry["provenance"]["line"].as_u64().unwrap_or(0) > 0,
        "the matched branch must carry a real line, got {entry}"
    );
}

#[test]
fn seed_hints_lists_discriminator_values_with_their_provenance() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, SEED_OPERATION_NO_SQL);

    let hints = seed_hints(&root, "prc_seed_opno");
    let values: Vec<&str> = hints["discriminator_values"]
        .as_array()
        .expect("discriminator_values array")
        .iter()
        .map(|v| v["value"].as_str().unwrap())
        .collect();

    for expected in ["0112004001", "0111004001", "0110999001"] {
        assert!(
            values.contains(&expected),
            "operation_no value {expected} missing from {values:?}"
        );
    }

    let decoded = discriminator_entry(&hints, "0112004001").unwrap();
    assert_eq!(decoded["column"], "operation_no");
    assert_eq!(
        decoded["source"], "cursor_decode",
        "a DECODE mapping value must say so, got {decoded}"
    );
    assert_eq!(
        decoded["confidence"], "medium",
        "a DECODE key is one of a set, so medium confidence, got {decoded}"
    );

    let branch = discriminator_entry(&hints, "0110999001").unwrap();
    assert_eq!(branch["column"], "operation_no");
    assert_eq!(
        branch["source"], "branch_condition",
        "an IF condition value must say so, got {branch}"
    );
    assert_eq!(
        branch["confidence"], "high",
        "an `=` branch condition names exactly one value, got {branch}"
    );
    let trigger = branch["trigger"]
        .as_str()
        .unwrap_or_else(|| panic!("the branch condition must carry its trigger: {branch}"));
    assert!(
        trigger.contains("operation_no") && trigger.contains("0110999001"),
        "the trigger must name the condition, got {trigger}"
    );
    assert!(
        branch["provenance"]["line"].as_u64().unwrap_or(0) > 0,
        "a branch condition carries its own line, got {branch}"
    );
}

/// An `IN` branch condition is a set of values, so the trigger must render as
/// SQL (`col IN ('a', 'b')`) rather than leaking the AST Debug form.
#[test]
fn seed_hints_renders_in_conditions_as_sql() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE src_op(operation_no VARCHAR(20));
CREATE TABLE out_op(bs VARCHAR(20));

CREATE PROCEDURE prc_seed_in AS
  CURSOR c_cur IS
    SELECT t.operation_no AS operation_no FROM src_op t;
  r_rec c_cur%ROWTYPE;
BEGIN
  OPEN c_cur;
  FETCH c_cur INTO r_rec;
  IF r_rec.operation_no IN ('0110004001', '0111004001') THEN
    INSERT INTO out_op(bs) VALUES ('hit');
  END IF;
  CLOSE c_cur;
END;
"#,
    );

    let hints = seed_hints(&root, "prc_seed_in");
    let branch = discriminator_entry(&hints, "0110004001").unwrap();
    assert_eq!(
        branch["confidence"], "medium",
        "an IN condition selects a set, so medium confidence, got {branch}"
    );
    let trigger = branch["trigger"].as_str().unwrap();
    assert!(
        trigger.contains("IN (") && trigger.contains("'0110004001'"),
        "the trigger must render as SQL, got {trigger}"
    );
    assert!(
        !trigger.contains("InList {"),
        "the trigger must not leak the AST Debug form, got {trigger}"
    );
}

/// Discriminator columns are opt-in: with none configured, the document still
/// reports tables/filters/equalities but enumerates no discriminator values.
#[test]
fn seed_hints_enumerates_no_discriminator_values_without_configuration() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, SEED_OPERATION_NO_SQL);

    let hints = seed_hints_with(&root, "prc_seed_opno", &[]);

    assert_eq!(
        hints["discriminator_values"].as_array().unwrap().len(),
        0,
        "codeweb ships no built-in discriminator column, got {:#?}",
        hints["discriminator_values"]
    );
    // The rest of the inventory is unaffected.
    assert!(!hints["tables"].as_array().unwrap().is_empty());
}

/// The discriminator is a column *name*, not a hard-coded `operation_no`: any
/// configured column is enumerated, and each value names its column.
#[test]
fn seed_hints_enumerates_any_configured_discriminator_column() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE src_cfg(biz_type VARCHAR(10), operation_no VARCHAR(20));
CREATE TABLE out_cfg(bs VARCHAR(10));

CREATE PROCEDURE prc_seed_cfg AS
  CURSOR c_cur IS
    SELECT t.biz_type AS biz_type, t.operation_no AS operation_no FROM src_cfg t;
  r_rec c_cur%ROWTYPE;
BEGIN
  OPEN c_cur;
  FETCH c_cur INTO r_rec;
  IF r_rec.biz_type = 'A1' THEN
    INSERT INTO out_cfg(bs) VALUES ('x');
  END IF;
  IF r_rec.operation_no = '0110999001' THEN
    INSERT INTO out_cfg(bs) VALUES ('y');
  END IF;
  CLOSE c_cur;
END;
"#,
    );

    let hints = seed_hints_with(&root, "prc_seed_cfg", &["biz_type"]);

    let values: Vec<(&str, &str)> = hints["discriminator_values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| (v["column"].as_str().unwrap(), v["value"].as_str().unwrap()))
        .collect();
    assert!(
        values.contains(&("biz_type", "A1")),
        "the configured column must be enumerated, got {values:?}"
    );
    assert!(
        !values.iter().any(|(c, _)| *c == "operation_no"),
        "an unconfigured column must not be enumerated, got {values:?}"
    );
}

/// Two overloads with bodies: the routine node keeps the *first* declaration
/// (`or_insert_with`), so the reported parameters must come from that same
/// declaration rather than the last one seen.
const SEED_OVERLOAD_SQL: &str = r#"
CREATE TABLE out_orders(order_id VARCHAR(20), kind VARCHAR(10));

CREATE PACKAGE pkg_over AS
  PROCEDURE prc_p(a NUMBER);
  PROCEDURE prc_p(a NUMBER, b NUMBER);
END pkg_over;
/

CREATE PACKAGE BODY pkg_over AS
  PROCEDURE prc_p(a NUMBER) AS
  BEGIN
    INSERT INTO out_orders(order_id, kind) VALUES ('a', 'A');
  END;

  PROCEDURE prc_p(a NUMBER, b NUMBER) AS
  BEGIN
    INSERT INTO out_orders(order_id, kind) VALUES ('b', 'B');
  END;
END pkg_over;
"#;

#[test]
fn seed_hints_signature_matches_the_surviving_overload() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, SEED_OVERLOAD_SQL);

    let hints = seed_hints(&root, "prc_p");

    let routines = hints["routines"].as_array().unwrap();
    assert_eq!(routines.len(), 1, "one node, one signature: {routines:#?}");
    let params = routines[0]["parameters"].as_array().unwrap();
    assert_eq!(
        params.len(),
        1,
        "the first declaration (one parameter) owns the node, so its signature \
         must be the one reported, got {params:#?}"
    );
    assert_eq!(params[0]["name"], "a");
}

/// Two aliases over the *same* physical table are a self-join, not a same-table
/// reference: the equality must survive. Dropping it (because both sides resolve
/// to the same table name) would lose a real constraint.
#[test]
fn seed_hints_keeps_a_self_join_equality() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE pair_tbl(k VARCHAR(10), v NUMBER);

CREATE PROCEDURE prc_self_join AS
BEGIN
  INSERT INTO pair_tbl(k, v)
  SELECT a.k, a.v FROM pair_tbl a, pair_tbl b
  WHERE abs(a.v * 10) = abs(b.v);
END;
"#,
    );

    let hints = seed_hints(&root, "prc_self_join");
    let eq = equality_mentioning(&hints, "10");
    assert_eq!(eq["left"]["table"], "pair_tbl");
    assert_eq!(eq["right"]["table"], "pair_tbl");
}

/// A schema-qualified `schema.table.column` has no alias: the table name must be
/// taken from the segment before the column rather than dropping the equality.
#[test]
fn seed_hints_keeps_a_schema_qualified_equality() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE s1.a_tbl(k VARCHAR(10), v NUMBER);
CREATE TABLE b_tbl(k VARCHAR(10), v NUMBER);

CREATE PROCEDURE prc_schema_eq AS
BEGIN
  INSERT INTO b_tbl(k, v)
  SELECT s1.a_tbl.k, s1.a_tbl.v FROM s1.a_tbl, b_tbl
  WHERE abs(s1.a_tbl.v * 10) = abs(b_tbl.v);
END;
"#,
    );

    let hints = seed_hints(&root, "prc_schema_eq");
    let eq = equality_mentioning(&hints, "10");
    assert_eq!(eq["left"]["table"], "a_tbl");
    assert_eq!(eq["right"]["table"], "b_tbl");
}

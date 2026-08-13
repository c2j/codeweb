//! Set operations (UNION / INTERSECT / EXCEPT) must contribute every branch's tables
//! to the graph, not just the first one.
//!
//! `TableAccessExtractor::visit_select` returns `SkipChildren` on every path, so the
//! visitor framework never reaches `SelectStatement::set_operation`. The extractor now
//! walks those branches itself.

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
        &["init", "union-test", "--dir", src.to_str().unwrap()],
    );
    assert!(
        out.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    root
}

fn detail(root: &Path, node: &str) -> String {
    let out = run_codeweb_in(root, &["detail", node, "-p", root.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "detail failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn view_over_two_way_union_depends_on_both_branches() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE ta(id NUMBER);
CREATE TABLE tb(id NUMBER);
CREATE VIEW v_two AS SELECT id FROM ta UNION ALL SELECT id FROM tb;
"#,
    );

    let out = detail(&root, "v_two");
    assert!(out.contains("table:ta"), "first branch missing:\n{out}");
    assert!(out.contains("table:tb"), "second branch missing:\n{out}");
}

#[test]
fn insert_select_over_two_way_union_reads_both_branches() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE ta(id NUMBER);
CREATE TABLE tb(id NUMBER);
CREATE TABLE dst(id NUMBER);
CREATE PROCEDURE prc_union AS BEGIN
  INSERT INTO dst(id) SELECT id FROM ta UNION ALL SELECT id FROM tb;
END;
"#,
    );

    let out = detail(&root, "prc_union");
    assert!(out.contains("table:ta"), "first branch not read:\n{out}");
    assert!(out.contains("table:tb"), "second branch not read:\n{out}");
    assert!(out.contains("table:dst"), "insert target missing:\n{out}");
}

/// Regression for the pair of bugs that hid each other: codeweb's extractor returned
/// `SkipChildren` before reaching `set_operation`, and `ogsql-parser` overwrote
/// `stmt.set_operation` each loop iteration instead of appending to the chain tail
/// (c2j/ogsql-parser#318, fixed by PR #317). With only one fixed, an N-branch union
/// still lost branches.
#[test]
fn view_over_four_way_union_depends_on_all_branches() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE ta(id NUMBER);
CREATE TABLE tb(id NUMBER);
CREATE TABLE tc(id NUMBER);
CREATE TABLE td(id NUMBER);
CREATE VIEW v_four AS
  SELECT id FROM ta
  UNION ALL SELECT id FROM tb
  UNION ALL SELECT id FROM tc
  UNION ALL SELECT id FROM td;
"#,
    );

    let out = detail(&root, "v_four");
    for t in ["table:ta", "table:tb", "table:tc", "table:td"] {
        assert!(
            out.contains(t),
            "{t} missing from view dependencies:\n{out}"
        );
    }
}

/// Eight branches with per-branch column renaming, mirroring `V_PAR_BOND` in the
/// clearing-split codebase that first exposed the branch loss.
#[test]
fn view_over_eight_way_union_depends_on_all_branches() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE par_sys_bond(security_id VARCHAR2(10), bond_kind VARCHAR2(4));
CREATE TABLE par_sys_asset_security(security_id VARCHAR2(10), bond_kind VARCHAR2(4));
CREATE TABLE par_sys_entrust(security_id VARCHAR2(10), entrust_type VARCHAR2(4));
CREATE TABLE par_sys_bond_right(security_id VARCHAR2(10), debt_loan_type VARCHAR2(4));
CREATE TABLE par_sys_financial_product(security_id VARCHAR2(10), financial_kind VARCHAR2(4));
CREATE TABLE par_sys_annuity_fund(security_id VARCHAR2(10), fund_type VARCHAR2(4));
CREATE TABLE par_sys_assurance(security_id VARCHAR2(10), security_kind VARCHAR2(4));
CREATE TABLE par_sys_debt_loan(security_id VARCHAR2(10), debt_loan_type VARCHAR2(4));

CREATE VIEW v_par_bond AS
  SELECT security_id, bond_kind FROM par_sys_bond
  UNION ALL SELECT security_id, bond_kind AS bond_kind FROM par_sys_asset_security
  UNION ALL SELECT security_id, entrust_type AS bond_kind FROM par_sys_entrust
  UNION ALL SELECT security_id, debt_loan_type AS bond_kind FROM par_sys_bond_right
  UNION ALL SELECT security_id, financial_kind AS bond_kind FROM par_sys_financial_product
  UNION ALL SELECT security_id, fund_type AS bond_kind FROM par_sys_annuity_fund
  UNION ALL SELECT security_id, security_kind AS bond_kind FROM par_sys_assurance
  UNION ALL SELECT security_id, debt_loan_type AS bond_kind FROM par_sys_debt_loan;
"#,
    );

    let out = detail(&root, "v_par_bond");
    for t in [
        "table:par_sys_bond",
        "table:par_sys_asset_security",
        "table:par_sys_entrust",
        "table:par_sys_bond_right",
        "table:par_sys_financial_product",
        "table:par_sys_annuity_fund",
        "table:par_sys_assurance",
        "table:par_sys_debt_loan",
    ] {
        assert!(
            out.contains(t),
            "{t} missing from view dependencies:\n{out}"
        );
    }
}

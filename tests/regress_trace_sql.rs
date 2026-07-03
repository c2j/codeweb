use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

fn codeweb_bin() -> PathBuf {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    let bin_name = if cfg!(windows) {
        "codeweb.exe"
    } else {
        "codeweb"
    };
    let entries = fs::read_dir(&base).unwrap_or_else(|_| panic!("no target dir"));
    for entry in entries.flatten() {
        let p = entry.path().join("debug").join(bin_name);
        if p.exists() {
            return p;
        }
    }
    base.join("debug").join(bin_name)
}

fn run_in_dir(dir: &TempDir, args: &[&str]) -> std::process::Output {
    std::process::Command::new(codeweb_bin())
        .args(args)
        .current_dir(dir.path())
        .output()
        .expect("failed to run codeweb")
}

fn write_xml(dir: &TempDir, filename: &str, namespace: &str, tag: &str, id: &str, sql: &str) {
    let content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE mapper PUBLIC "-//mybatis.org//DTD Mapper 3.0//EN" "http://mybatis.org/dtd/mybatis-3-mapper.dtd">
<mapper namespace="{}">
  <{} id="{}">
    {}
  </{}>
</mapper>"#,
        namespace, tag, id, sql, tag
    );
    fs::write(dir.path().join(filename), content).unwrap();
}

fn write_sql(dir: &TempDir, filename: &str, sql: &str) {
    fs::write(dir.path().join(filename), sql).unwrap();
}

fn init_project(dir: &TempDir) {
    let output = run_in_dir(dir, &["init", "test", "-d", "."]);
    assert!(
        output.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn trace_sql(dir: &TempDir, query: &str) -> String {
    let output = run_in_dir(dir, &["trace-sql", "--project", ".", query]);
    assert!(
        output.status.success(),
        "trace-sql failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn assert_contains(stdout: &str, expected: &str, case: &str) {
    assert!(
        stdout.contains(expected),
        "[{}] expected output to contain '{}'\nactual:\n{}",
        case,
        expected,
        stdout
    );
}

fn assert_not_contains(stdout: &str, unexpected: &str, case: &str) {
    assert!(
        !stdout.contains(unexpected),
        "[{}] expected output NOT to contain '{}'\nactual:\n{}",
        case,
        unexpected,
        stdout
    );
}

// ── tests ──

#[test]
fn regress_trace_sql_c01_exact_fingerprint() {
    let dir = TempDir::new().unwrap();
    write_xml(
        &dir,
        "UserDao.xml",
        "com.example.UserDao",
        "select",
        "findById",
        "SELECT * FROM users WHERE id = #{id}",
    );
    init_project(&dir);
    let stdout = trace_sql(&dir, "select * from users where id = ?");
    assert_contains(&stdout, "findById", "c01");
    assert_contains(&stdout, "100%", "c01");
}

#[test]
fn regress_trace_sql_c02_wildcard() {
    let dir = TempDir::new().unwrap();
    write_xml(
        &dir,
        "OrderDao.xml",
        "com.example.OrderDao",
        "select",
        "findByStatus",
        "SELECT * FROM orders WHERE status = #{status} AND amount > #{minAmount}",
    );
    init_project(&dir);
    let stdout = trace_sql(&dir, "select * from orders where status = ? and amount > ?");
    assert_contains(&stdout, "findByStatus", "c02");
}

#[test]
fn regress_trace_sql_c03_dml_keyword() {
    let dir = TempDir::new().unwrap();
    write_xml(
        &dir,
        "DaoA.xml",
        "com.example.ReportDao",
        "select",
        "findReports",
        "SELECT id, name FROM reports WHERE type = #{type}",
    );
    write_xml(
        &dir,
        "DaoB.xml",
        "com.example.ReportDao2",
        "insert",
        "insertReport",
        "INSERT INTO reports (name, type) VALUES (#{name}, #{type})",
    );
    init_project(&dir);
    let stdout = trace_sql(&dir, "insert into reports (name, type) values (?, ?)");
    assert_contains(&stdout, "insertReport", "c03");
    assert_not_contains(&stdout, "findReports", "c03");
}

#[test]
fn regress_trace_sql_c04_jaccard() {
    let dir = TempDir::new().unwrap();
    write_xml(
        &dir,
        "AccountDao.xml",
        "com.example.AccountDao",
        "update",
        "updateBalance",
        "UPDATE accounts SET balance = #{amount} WHERE id = #{id}",
    );
    init_project(&dir);
    let stdout = trace_sql(&dir, "update accounts set balance = 100 where id = 1");
    assert_contains(&stdout, "updateBalance", "c04");
}

#[test]
fn regress_trace_sql_c05_proc_body() {
    let dir = TempDir::new().unwrap();
    write_sql(&dir, "procs.sql",
        "CREATE OR REPLACE PROCEDURE get_active_users()\nAS BEGIN\n    SELECT * FROM t_users WHERE status = 'ACTIVE';\nEND;\n/\n");
    init_project(&dir);
    let stdout = trace_sql(&dir, "select * from t_users where status");
    assert_contains(&stdout, "get_active_users", "c05");
}

#[test]
fn regress_trace_sql_c06_scoring() {
    let dir = TempDir::new().unwrap();
    write_xml(
        &dir,
        "Multi1.xml",
        "com.example.MultiDao",
        "select",
        "findUser",
        "SELECT * FROM users WHERE id = #{id}",
    );
    write_xml(
        &dir,
        "Multi2.xml",
        "com.example.MultiDao2",
        "select",
        "findUserByName",
        "SELECT * FROM users WHERE name LIKE #{name}",
    );
    write_xml(
        &dir,
        "Multi3.xml",
        "com.example.MultiDao3",
        "insert",
        "insertUser",
        "INSERT INTO users (name, email) VALUES (#{name}, #{email})",
    );
    init_project(&dir);
    let stdout = trace_sql(&dir, "select * from users");
    assert_contains(&stdout, "findUser", "c06");
    assert_contains(&stdout, "findUserByName", "c06");
    assert_not_contains(&stdout, "insertUser", "c06");
}

#[test]
fn regress_trace_sql_c07_no_match_type() {
    let dir = TempDir::new().unwrap();
    write_xml(
        &dir,
        "SelectOnly.xml",
        "com.example.SelectOnlyDao",
        "select",
        "getAll",
        "SELECT * FROM items WHERE active = 1",
    );
    init_project(&dir);
    let stdout = trace_sql(&dir, "insert into items (name) values (?)");
    assert_not_contains(&stdout, "SelectOnlyDao", "c07");
}

#[test]
fn regress_trace_sql_c08_case_insens() {
    let dir = TempDir::new().unwrap();
    write_xml(
        &dir,
        "CaseDao.xml",
        "com.example.CaseDao",
        "select",
        "getAll",
        "SELECT * FROM Users WHERE Status = 'ACTIVE'",
    );
    init_project(&dir);
    let stdout = trace_sql(&dir, "select * from users where status");
    assert_contains(&stdout, "getAll", "c08");
}

// ── Regress fixtures (tests/regress/trace_sql/cases/) ──

const SQL_MULTI_PROC_DML: &str = include_str!("regress/trace_sql/cases/01_multi_proc_dml.sql");
const SQL_PROC_WITH_CALLERS: &str =
    include_str!("regress/trace_sql/cases/02_proc_with_callers.sql");
const SQL_MULTI_MATCH_SCORING: &str =
    include_str!("regress/trace_sql/cases/03_multi_match_scoring.sql");
const SQL_COMPLEX_QUERY: &str = include_str!("regress/trace_sql/cases/04_complex_query.sql");

fn init_and_trace(dir: &TempDir, query: &str) -> String {
    let output = run_in_dir(dir, &["init", "test", "-d", "."]);
    assert!(
        output.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = run_in_dir(dir, &["trace-sql", "--project", ".", query]);
    assert!(
        output.status.success(),
        "trace-sql failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

// ── c09: DML keyword filtering — INSERT only matches INSERT ──

#[test]
fn regress_trace_sql_c09_insert_matches_only_insert() {
    let dir = TempDir::new().unwrap();
    write_sql(&dir, "dml_procs.sql", SQL_MULTI_PROC_DML);
    let stdout = init_and_trace(&dir, "insert into t_orders (id, amount");

    assert_contains(&stdout, "proc_insert_order", "c09");
    assert_contains(&stdout, "t_orders", "c09");
    assert_not_contains(&stdout, "proc_update_order_status", "c09");
    assert_not_contains(&stdout, "proc_delete_expired_orders", "c09");
}

// ── c10: DML keyword filtering — UPDATE only matches UPDATE ──

#[test]
fn regress_trace_sql_c10_update_matches_only_update() {
    let dir = TempDir::new().unwrap();
    write_sql(&dir, "dml_procs.sql", SQL_MULTI_PROC_DML);
    let stdout = init_and_trace(&dir, "update t_orders set status");

    assert_contains(&stdout, "proc_update_order_status", "c10");
    assert_not_contains(&stdout, "proc_insert_order", "c10");
    assert_not_contains(&stdout, "proc_delete_expired_orders", "c10");
}

// ── c11: DML keyword filtering — DELETE only matches DELETE ──

#[test]
fn regress_trace_sql_c11_delete_matches_only_delete() {
    let dir = TempDir::new().unwrap();
    write_sql(&dir, "dml_procs.sql", SQL_MULTI_PROC_DML);
    let stdout = init_and_trace(&dir, "delete from t_orders where");

    assert_contains(&stdout, "proc_delete_expired_orders", "c11");
    assert_not_contains(&stdout, "proc_insert_order", "c11");
    assert_not_contains(&stdout, "proc_update_order_status", "c11");
}

// ── c12: Procedure body SQL match with callers ──

#[test]
fn regress_trace_sql_c12_proc_body_with_callers() {
    let dir = TempDir::new().unwrap();
    write_sql(&dir, "all_procs.sql", SQL_MULTI_PROC_DML);
    // proc_delete_expired_orders CALLed by proc_daily_cleanup
    write_sql(&dir, "procs_with_callers.sql", SQL_PROC_WITH_CALLERS);
    let stdout = init_and_trace(&dir, "delete from t_orders where status = 'EXPIRED'");

    assert_contains(&stdout, "proc_delete_expired_orders", "c12");
    assert_contains(&stdout, "proc_daily_cleanup", "c12");
}

// ── c13: Scoring order — exact match (100%) > substring match (95%) ──

#[test]
fn regress_trace_sql_c13_scoring_exact_beats_partial() {
    let dir = TempDir::new().unwrap();
    write_xml(
        &dir,
        "ItemDaoA.xml",
        "com.example.ItemDao",
        "select",
        "findByCategory",
        "SELECT * FROM t_items WHERE category = #{cat}",
    );
    write_xml(
        &dir,
        "ItemDaoB.xml",
        "com.example.ItemDaoV2",
        "select",
        "findByCategoryAndPrice",
        "SELECT * FROM t_items WHERE category = #{cat} AND price > #{minPrice}",
    );
    init_project(&dir);
    // Query without "= ?" to bypass fingerprint index, forcing PreparedQuery path
    // where both mappers match as substrings
    let stdout = trace_sql(&dir, "select * from t_items where category");

    let pos1 = stdout.find("findByCategory");
    let pos2 = stdout.find("findByCategoryAndPrice");
    assert!(pos1.is_some(), "c13: should find findByCategory");
    assert!(pos2.is_some(), "c13: should find findByCategoryAndPrice");

    // Both match as substring (95%), order is alphabetical within same score
    if let (Some(p1), Some(p2)) = (pos1, pos2) {
        assert!(
            p1 < p2,
            "c13: findByCategory should appear before findByCategoryAndPrice. p1={} p2={}\nstdout:\n{}",
            p1, p2, stdout
        );
    }
}

// ── c14: UPDATE DML should NOT match SELECT procedures ──

#[test]
fn regress_trace_sql_c14_update_not_match_select() {
    let dir = TempDir::new().unwrap();
    write_sql(&dir, "user_procs.sql", SQL_MULTI_MATCH_SCORING);
    let stdout = init_and_trace(&dir, "update t_users set status");

    assert_contains(&stdout, "proc_deactivate_user", "c14");
    assert_not_contains(&stdout, "proc_get_active_users", "c14");
    assert_not_contains(&stdout, "proc_get_users_by_dept", "c14");
}

// ── c15: Complex multi-table query with JOIN ──

#[test]
fn regress_trace_sql_c15_complex_join_query() {
    let dir = TempDir::new().unwrap();
    write_sql(&dir, "report.sql", SQL_COMPLEX_QUERY);
    let stdout = init_and_trace(&dir, "join t_customers c on");

    assert_contains(&stdout, "proc_generate_monthly_report", "c15");
}

// ── c16: No match for unrelated SQL ──

#[test]
fn regress_trace_sql_c16_no_match_unrelated_sql() {
    let dir = TempDir::new().unwrap();
    write_sql(&dir, "dml_procs.sql", SQL_MULTI_PROC_DML);
    let stdout = init_and_trace(&dir, "select * from some_other_table where x = 1");

    assert!(
        stdout.contains("No matching SQL found")
            || (!stdout.contains("proc_insert_order")
                && !stdout.contains("proc_update_order_status")
                && !stdout.contains("proc_delete_expired_orders")),
        "c16: unrelated SQL should not match any procedure. stdout:\n{}",
        stdout
    );
}

// ── c17: Full output snapshot for display verification ──

#[test]
fn regress_trace_sql_c17_display_snapshot() {
    let dir = TempDir::new().unwrap();
    write_sql(&dir, "dml_procs.sql", SQL_MULTI_PROC_DML);
    write_sql(&dir, "procs_with_callers.sql", SQL_PROC_WITH_CALLERS);
    let stdout = init_and_trace(&dir, "insert into t_order_audit (order_id, action");

    assert_contains(&stdout, "proc_process_order", "c17-target");
    assert_contains(&stdout, "t_order_audit", "c17-sql");
    assert_contains(&stdout, "proc_batch_process_orders", "c17-caller");
    assert_not_contains(&stdout, "proc_delete_expired_orders", "c17-unrelated");

    // Print full output so we can see the display format
    eprintln!(
        "=== c17: trace-sql display snapshot ===\n{}\n=== end snapshot ===",
        stdout
    );
}

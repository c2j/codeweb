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

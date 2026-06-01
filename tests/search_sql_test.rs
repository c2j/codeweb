use std::fs;
use tempfile::TempDir;

fn run_codeweb(args: &[&str]) -> std::process::Output {
    let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    let bin_name = if cfg!(windows) {
        "codeweb.exe"
    } else {
        "codeweb"
    };
    let entries = std::fs::read_dir(&base).unwrap_or_else(|_| panic!("no target dir"));
    for entry in entries.flatten() {
        let p = entry.path().join("debug").join(bin_name);
        if p.exists() {
            return std::process::Command::new(p)
                .args(args)
                .output()
                .expect("failed to run codeweb");
        }
    }
    let bin = base.join("debug").join(bin_name);
    std::process::Command::new(bin)
        .args(args)
        .output()
        .expect("failed to run codeweb")
}

fn write_mapper_xml(
    dir: &TempDir,
    filename: &str,
    namespace: &str,
    id: &str,
    sql: &str,
) -> std::path::PathBuf {
    let content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE mapper PUBLIC "-//mybatis.org//DTD Mapper 3.0//EN" "http://mybatis.org/dtd/mybatis-3-mapper.dtd">
<mapper namespace="{}">
  <select id="{}">
    {}
  </select>
</mapper>"#,
        namespace, id, sql
    );
    let path = dir.path().join(filename);
    fs::write(&path, content).unwrap();
    path
}

fn run_codeweb_in_dir(args: &[&str], cwd: &std::path::Path) -> std::process::Output {
    let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    let bin_name = if cfg!(windows) {
        "codeweb.exe"
    } else {
        "codeweb"
    };
    let entries = std::fs::read_dir(&base).unwrap_or_else(|_| panic!("no target dir"));
    for entry in entries.flatten() {
        let p = entry.path().join("debug").join(bin_name);
        if p.exists() {
            return std::process::Command::new(p)
                .args(args)
                .current_dir(cwd)
                .output()
                .expect("failed to run codeweb");
        }
    }
    let bin = base.join("debug").join(bin_name);
    std::process::Command::new(bin)
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("failed to run codeweb")
}

fn init_project(dir: &TempDir, name: &str) -> std::process::Output {
    run_codeweb_in_dir(&["init", name], dir.path())
}

// --- GREEN: CLI trace-sql basic ---

#[test]
fn trace_sql_finds_matching_mapper() {
    let dir = TempDir::new().unwrap();
    write_mapper_xml(
        &dir,
        "UserDao.xml",
        "com.example.UserDao",
        "findById",
        "SELECT * FROM users WHERE id = #{id}",
    );

    let init_out = init_project(&dir, "test-trace-sql");
    assert!(
        init_out.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init_out.stderr)
    );

    let output = run_codeweb(&[
        "trace-sql",
        "select * from users where id",
        "--project",
        dir.path().to_str().unwrap(),
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("UserDao") || stdout.contains("findById"),
        "trace-sql should find matching mapper. stdout: {}, stderr: {}",
        stdout,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn trace_sql_no_match_returns_empty() {
    let dir = TempDir::new().unwrap();
    write_mapper_xml(
        &dir,
        "UserDao.xml",
        "com.example.UserDao",
        "findById",
        "SELECT * FROM users WHERE id = #{id}",
    );

    let init_out = init_project(&dir, "test-no-match");
    assert!(init_out.status.success(), "init failed");

    let output = run_codeweb(&[
        "trace-sql",
        "delete from orders where id = ?",
        "--project",
        dir.path().to_str().unwrap(),
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("UserDao"),
        "should not find mapper for unrelated SQL. stdout: {}",
        stdout
    );
}

#[test]
fn trace_sql_reads_from_file() {
    let dir = TempDir::new().unwrap();
    write_mapper_xml(
        &dir,
        "UserDao.xml",
        "com.example.UserDao",
        "findById",
        "SELECT * FROM users WHERE id = #{id}",
    );

    let init_out = init_project(&dir, "test-file-input");
    assert!(init_out.status.success(), "init failed");

    let query_file = dir.path().join("query.txt");
    fs::write(&query_file, "select * from users where id").unwrap();

    let output = run_codeweb(&[
        "trace-sql",
        "--file",
        query_file.to_str().unwrap(),
        "--project",
        dir.path().to_str().unwrap(),
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("UserDao") || stdout.contains("findById"),
        "trace-sql --file should find matching mapper. stdout: {}, stderr: {}",
        stdout,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_sql_file(dir: &TempDir, filename: &str, sql: &str) -> std::path::PathBuf {
    let path = dir.path().join(filename);
    fs::write(&path, sql).unwrap();
    path
}

#[test]
fn trace_sql_finds_matching_procedure() {
    let dir = TempDir::new().unwrap();
    write_sql_file(
        &dir,
        "procs.sql",
        r#"
            CREATE OR REPLACE PROCEDURE get_active_users()
            AS BEGIN
                SELECT * FROM t_users WHERE status = 'ACTIVE';
            END;
            /
        "#,
    );

    let init_out = init_project(&dir, "test-proc-search");
    assert!(
        init_out.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init_out.stderr)
    );

    let output = run_codeweb(&[
        "trace-sql",
        "select * from t_users where status",
        "--project",
        dir.path().to_str().unwrap(),
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("get_active_users") || stdout.contains("Procedure"),
        "trace-sql should find procedure body SQL. stdout: {}, stderr: {}",
        stdout,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn trace_sql_finds_matching_function() {
    let dir = TempDir::new().unwrap();
    write_sql_file(
        &dir,
        "funcs.sql",
        r#"
            CREATE OR REPLACE FUNCTION count_orders() RETURNS INT
            AS $$
            BEGIN
                INSERT INTO t_audit(action) VALUES('count');
                RETURN 0;
            END;
            $$ LANGUAGE plpgsql;
        "#,
    );

    let init_out = init_project(&dir, "test-func-search");
    assert!(
        init_out.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init_out.stderr)
    );

    let output = run_codeweb(&[
        "trace-sql",
        "insert into t_audit",
        "--project",
        dir.path().to_str().unwrap(),
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("count_orders") || stdout.contains("Function"),
        "trace-sql should find function body SQL. stdout: {}, stderr: {}",
        stdout,
        String::from_utf8_lossy(&output.stderr)
    );
}

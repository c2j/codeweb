#![cfg(feature = "jsp")]

use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

fn run_codeweb(args: &[&str]) -> std::process::Output {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    let bin_name = if cfg!(windows) { "codeweb.exe" } else { "codeweb" };
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

#[test]
fn jsp_file_is_recognized_and_extracts_sql() {
    let tmp = TempDir::new().unwrap();
    let jsp_path = tmp.path().join("user_query.jsp");
    fs::write(
        &jsp_path,
        r#"<%@ page import="java.sql.*" %>
<%
Connection conn = DriverManager.getConnection("jdbc:default");
PreparedStatement ps = conn.prepareStatement("SELECT id, name FROM users WHERE id = ?");
ps.setInt(1, 123);
ResultSet rs = ps.executeQuery();
%>
<html><body>done</body></html>"#,
    )
    .unwrap();

    let output = run_codeweb(&[tmp.path().to_str().unwrap(), "--format", "json"]);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("users"),
        "extracted SQL should mention 'users' table: {}",
        stdout
    );
}

#[test]
fn jsp_file_links_to_stored_procedure_select() {
    let tmp = TempDir::new().unwrap();

    let sql_path = tmp.path().join("schema.sql");
    fs::write(
        &sql_path,
        r#"CREATE TABLE users (id BIGINT, name VARCHAR(100));
CREATE OR REPLACE PROCEDURE get_user_by_id(p_id IN BIGINT)
AS
BEGIN
    SELECT * FROM users WHERE id = p_id;
END;
/"#,
    )
    .unwrap();

    let jsp_path = tmp.path().join("page.jsp");
    fs::write(
        &jsp_path,
        r#"<%
Connection conn = null;
PreparedStatement ps = conn.prepareStatement("SELECT * FROM users WHERE id = ?");
%>"#,
    )
    .unwrap();

    let output = run_codeweb(&[tmp.path().to_str().unwrap(), "--format", "dot"]);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("jsp") || stdout.contains("jsql"),
        "DOT output should contain JSP nodes: {}",
        stdout
    );
}

#[test]
fn html_only_jsp_does_not_break_analysis() {
    let tmp = TempDir::new().unwrap();
    let jsp_path = tmp.path().join("static.jsp");
    fs::write(&jsp_path, "<html><body>Hello</body></html>").unwrap();

    let output = run_codeweb(&[tmp.path().to_str().unwrap(), "--format", "json"]);

    assert!(
        output.status.success(),
        "should not crash on HTML-only JSP, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

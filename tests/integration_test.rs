use std::fs;
use tempfile::TempDir;

fn run_codeweb(args: &[&str]) -> std::process::Output {
    let bin = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("debug")
        .join("codeweb");
    std::process::Command::new(bin)
        .args(args)
        .output()
        .expect("failed to run codeweb")
}

fn write_sql(dir: &TempDir, filename: &str, sql: &str) -> std::path::PathBuf {
    let path = dir.path().join(filename);
    fs::write(&path, sql).unwrap();
    path
}

#[test]
fn test_dot_output_single_procedure_call() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        "CREATE PROCEDURE hello() AS $$ BEGIN world(); END; $$;",
    );

    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "dot"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("digraph"));
    assert!(stdout.contains("hello"));
    assert!(stdout.contains("world"));
}

#[test]
fn test_json_output() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        "CREATE PROCEDURE hello() AS $$ BEGIN world(); END; $$;",
    );

    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(parsed["nodes"].is_array());
    assert!(parsed["edges"].is_array());
    let nodes = parsed["nodes"].as_array().unwrap();
    assert!(nodes.len() >= 2);
}

#[test]
fn test_mermaid_output() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        "CREATE PROCEDURE hello() AS $$ BEGIN world(); END; $$;",
    );

    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "mermaid"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("graph LR"));
    assert!(stdout.contains("hello"));
}

#[test]
fn test_call_statement() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        "CREATE PROCEDURE caller() AS $$ BEGIN callee(1, 2); END; $$;",
    );

    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "dot"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("caller"));
    assert!(stdout.contains("callee"));
}

#[test]
fn test_function_call_graph() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        r#"
            CREATE FUNCTION foo() RETURNS INTEGER AS $$
            BEGIN
                PERFORM bar();
                RETURN 1;
            END;
            $$ LANGUAGE plpgsql;
        "#,
    );

    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "dot"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("foo"));
}

#[test]
fn test_output_to_file() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        "CREATE PROCEDURE hello() AS $$ BEGIN world(); END; $$;",
    );
    let outfile = dir.path().join("output.dot");

    let output = run_codeweb(&[
        dir.path().to_str().unwrap(),
        "--format",
        "dot",
        "--output",
        outfile.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let content = fs::read_to_string(&outfile).unwrap();
    assert!(content.contains("digraph"));
}

#[test]
fn test_no_sql_files() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("readme.txt"), "not sql").unwrap();

    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "dot"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no SQL files"));
}

#[test]
fn test_schema_qualified_names() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        "CREATE PROCEDURE public.init() AS $$ BEGIN public.setup(); END; $$;",
    );

    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "dot"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("public.init"));
    assert!(stdout.contains("public.setup"));
}

fn write_java(dir: &TempDir, filename: &str, java: &str) -> std::path::PathBuf {
    let path = dir.path().join(filename);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, java).unwrap();
    path
}

fn write_xml(dir: &TempDir, filename: &str, xml: &str) -> std::path::PathBuf {
    let path = dir.path().join(filename);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, xml).unwrap();
    path
}

#[test]
fn test_java_method_to_mapper_bridge() {
    let dir = TempDir::new().unwrap();

    write_xml(
        &dir,
        "UserDao.xml",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE mapper PUBLIC "-//mybatis.org//DTD Mapper 3.0//EN" "http://mybatis.org/dtd/mybatis-3-mapper.dtd">
<mapper namespace="com.example.dao.UserDao">
    <select id="findById" resultType="User">
        SELECT * FROM users WHERE id = #{id}
    </select>
</mapper>"#,
    );

    write_java(
        &dir,
        "UserService.java",
        r#"package com.example.service;
import com.example.dao.UserDao;
public class UserService {
    private UserDao userDao;
    public Object getUser(Long id) {
        return userDao.findById(id);
    }
}"#,
    );

    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let nodes = parsed["nodes"].as_array().unwrap();
    assert!(
        nodes.len() >= 3,
        "Expected at least 3 nodes, got {}",
        nodes.len()
    );

    let has_java_method = nodes.iter().any(|n| n["type"] == "java_method");
    assert!(has_java_method, "Expected a java_method node");

    let edges = parsed["edges"].as_array().unwrap();
    let has_bridge_edge = edges.iter().any(|e| e["type"] == "invokes_mapper");
    assert!(
        has_bridge_edge,
        "Expected an invokes_mapper edge bridging Java method to mapper"
    );
}

#[test]
fn test_sqlsession_bridge() {
    let dir = TempDir::new().unwrap();

    write_xml(
        &dir,
        "UserMapper.xml",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE mapper PUBLIC "-//mybatis.org//DTD Mapper 3.0//EN" "http://mybatis.org/dtd/mybatis-3-mapper.dtd">
<mapper namespace="com.example.dao.UserMapper">
    <select id="findAll" resultType="User">
        SELECT * FROM users
    </select>
</mapper>"#,
    );

    write_java(
        &dir,
        "UserRepo.java",
        r#"package com.example.repo;
public class UserRepo {
    public Object getUsers() {
        return sqlSession.selectList("com.example.dao.UserMapper.findAll");
    }
}"#,
    );

    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let edges = parsed["edges"].as_array().unwrap();
    let has_bridge = edges.iter().any(|e| e["type"] == "invokes_mapper");
    assert!(has_bridge, "Expected sqlSession -> mapper bridge edge");
}

#[test]
fn test_java_method_to_method_call() {
    let dir = TempDir::new().unwrap();

    write_java(
        &dir,
        "Service.java",
        r#"package com.example;
public class Service {
    public void doWork() {
        helper();
    }
    public void helper() {}
}"#,
    );

    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let edges = parsed["edges"].as_array().unwrap();
    let has_java_call = edges.iter().any(|e| e["type"] == "calls_java");
    assert!(has_java_call, "Expected calls_java edge between methods");
}

#[test]
fn test_java_extends_implements() {
    let dir = TempDir::new().unwrap();

    write_java(
        &dir,
        "Base.java",
        "package com.example;\npublic class Base {}",
    );

    write_java(
        &dir,
        "Iface.java",
        "package com.example;\npublic interface Iface {}",
    );

    write_java(
        &dir,
        "Child.java",
        "package com.example;\npublic class Child extends Base implements Iface {}",
    );

    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let edges = parsed["edges"].as_array().unwrap();

    let has_extends = edges.iter().any(|e| e["type"] == "extends");
    let has_implements = edges.iter().any(|e| e["type"] == "implements");
    assert!(has_extends, "Expected extends edge");
    assert!(has_implements, "Expected implements edge");
}

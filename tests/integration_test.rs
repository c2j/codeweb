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

#[test]
fn test_e2e_full_chain() {
    let demo_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("lib")
        .join("codeweb-e2e-demo");

    let output = run_codeweb(&[demo_dir.to_str().unwrap(), "--format", "json"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let nodes = parsed["nodes"].as_array().unwrap();
    let edges = parsed["edges"].as_array().unwrap();

    let node_types: std::collections::HashSet<&str> =
        nodes.iter().filter_map(|n| n["type"].as_str()).collect();
    let edge_types: std::collections::HashSet<&str> =
        edges.iter().filter_map(|e| e["type"].as_str()).collect();

    assert!(
        node_types.contains("procedure"),
        "Expected procedure nodes (SQL stored procs)"
    );
    assert!(
        node_types.contains("function"),
        "Expected function nodes (SQL functions)"
    );
    assert!(
        node_types.contains("mapped_statement"),
        "Expected mapped_statement nodes (MyBatis XML)"
    );
    assert!(
        node_types.contains("java_class"),
        "Expected java_class nodes"
    );
    assert!(
        node_types.contains("java_method"),
        "Expected java_method nodes"
    );
    assert!(
        node_types.contains("table"),
        "Expected table nodes (from SQL DML references)"
    );

    assert!(
        edge_types.contains("calls_procedure"),
        "Expected calls_procedure edges: mapper → stored proc"
    );
    assert!(
        edge_types.contains("invokes_mapper"),
        "Expected invokes_mapper edges: Java method → mapper"
    );
    assert!(
        edge_types.contains("calls_java"),
        "Expected calls_java edges: Java method → Java method"
    );
    assert!(
        edge_types.contains("extends"),
        "Expected extends edge: ReportService → BaseService"
    );
    assert!(
        edge_types.contains("contains_method"),
        "Expected contains_method edges: class → method"
    );
    assert!(
        edge_types.contains("table_access"),
        "Expected table_access edges: procedure/mapper → table"
    );

    let proc_names: Vec<&str> = nodes
        .iter()
        .filter(|n| n["type"] == "procedure")
        .filter_map(|n| n["name"].as_str())
        .collect();
    assert!(
        proc_names.contains(&"create_user"),
        "Expected pkg_user_mgmt.create_user procedure"
    );
    assert!(
        proc_names.contains(&"deactivate_user"),
        "Expected pkg_user_mgmt.deactivate_user procedure"
    );

    let func_names: Vec<&str> = nodes
        .iter()
        .filter(|n| n["type"] == "function")
        .filter_map(|n| n["name"].as_str())
        .collect();
    assert!(
        func_names.contains(&"send_event"),
        "Expected pkg_notify.send_event function"
    );

    let table_names: Vec<&str> = nodes
        .iter()
        .filter(|n| n["type"] == "table")
        .filter_map(|n| n["name"].as_str())
        .collect();
    assert!(
        table_names.contains(&"t_users"),
        "Expected t_users table node"
    );
    assert!(
        table_names.contains(&"t_orders"),
        "Expected t_orders table node"
    );

    let table_access_edges: Vec<&serde_json::Value> = edges
        .iter()
        .filter(|e| e["type"] == "table_access")
        .collect();
    assert!(
        table_access_edges.len() >= 5,
        "Expected >= 5 table_access edges, got {}",
        table_access_edges.len()
    );

    let calls_proc_edges: Vec<&serde_json::Value> = edges
        .iter()
        .filter(|e| e["type"] == "calls_procedure")
        .collect();
    assert!(
        calls_proc_edges.len() >= 3,
        "Expected >= 3 calls_procedure edges, got {}",
        calls_proc_edges.len()
    );

    let mapper_to_proc: Vec<(usize, usize)> = calls_proc_edges
        .iter()
        .filter_map(|e| {
            let src = e["source"].as_u64()? as usize;
            let tgt = e["target"].as_u64()? as usize;
            let src_type = nodes.get(src)?["type"].as_str()?;
            let tgt_type = nodes.get(tgt)?["type"].as_str()?;
            if src_type == "mapped_statement" && (tgt_type == "procedure" || tgt_type == "function")
            {
                Some((src, tgt))
            } else {
                None
            }
        })
        .collect();
    assert!(
        !mapper_to_proc.is_empty(),
        "Expected at least one mapped_statement → procedure/function chain"
    );

    let invokes_mapper_edges: Vec<&serde_json::Value> = edges
        .iter()
        .filter(|e| e["type"] == "invokes_mapper")
        .collect();
    assert!(
        invokes_mapper_edges.len() >= 5,
        "Expected >= 5 invokes_mapper edges, got {}",
        invokes_mapper_edges.len()
    );

    let java_to_mapper: Vec<(usize, usize)> = invokes_mapper_edges
        .iter()
        .filter_map(|e| {
            let src = e["source"].as_u64()? as usize;
            let tgt = e["target"].as_u64()? as usize;
            let src_type = nodes.get(src)?["type"].as_str()?;
            let tgt_type = nodes.get(tgt)?["type"].as_str()?;
            if src_type == "java_method" && tgt_type == "mapped_statement" {
                Some((src, tgt))
            } else {
                None
            }
        })
        .collect();
    assert!(
        !java_to_mapper.is_empty(),
        "Expected at least one java_method → mapped_statement chain"
    );
}

#[test]
fn test_package_body_in_call_graph() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "pkg.sql",
        r#"
        CREATE OR REPLACE PACKAGE BODY pkg_api AS
            PROCEDURE do_work(p_id INT) IS
            BEGIN
                helper.validate(p_id);
            END;
        END pkg_api;
    "#,
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

    let has_package = nodes.iter().any(|n| n["type"] == "package");
    assert!(
        has_package,
        "Expected a package node, got nodes: {:?}",
        nodes
    );

    let has_package_proc = nodes
        .iter()
        .any(|n| n["type"] == "procedure" && n["name"] == "do_work");
    assert!(has_package_proc, "Expected a procedure node for do_work");
}

#[test]
fn test_trigger_in_call_graph() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "trigger.sql",
        r#"
        CREATE OR REPLACE FUNCTION trg_func() RETURNS TRIGGER AS $$
        BEGIN
            INSERT INTO t_log(action) VALUES('FIRED');
            RETURN NEW;
        END;
        $$ LANGUAGE plpgsql;

        CREATE TRIGGER trg_after_insert
        AFTER INSERT ON t_users
        FOR EACH ROW EXECUTE PROCEDURE trg_func();
    "#,
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

    let has_trigger = nodes.iter().any(|n| n["type"] == "trigger");
    assert!(has_trigger, "Expected a trigger node");

    let has_function = nodes
        .iter()
        .any(|n| n["type"] == "function" && n["name"] == "trg_func");
    assert!(has_function, "Expected trg_func as a function node");

    let edges = parsed["edges"].as_array().unwrap();
    let has_triggers_routine = edges.iter().any(|e| e["type"] == "triggers_routine");
    assert!(has_triggers_routine, "Expected a triggers_routine edge");
}

#[test]
fn test_view_table_references() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "view.sql",
        r#"
        CREATE VIEW v_active_users AS
        SELECT u.id, u.name
        FROM t_users u
        WHERE u.status = 'ACTIVE';
    "#,
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

    let has_view = nodes
        .iter()
        .any(|n| n["type"] == "view" && n["name"] == "v_active_users");
    assert!(has_view, "Expected a view node named v_active_users");
}

#[test]
fn test_package_cross_call_resolution() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "mixed.sql",
        r#"
        CREATE OR REPLACE PROCEDURE caller_proc() AS $$
        BEGIN
            pkg_api.do_work(42);
        END;
        $$;

        CREATE OR REPLACE PACKAGE BODY pkg_api AS
            PROCEDURE do_work(p_id INT) IS
            BEGIN
                helper.validate(p_id);
            END;
        END pkg_api;
    "#,
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

    let caller_idx = nodes.iter().position(|n| n["name"] == "caller_proc");
    let dowork_idx = nodes.iter().position(|n| n["name"] == "do_work");
    assert!(caller_idx.is_some(), "Expected a node for caller_proc");
    assert!(dowork_idx.is_some(), "Expected a node for do_work");

    let edges = parsed["edges"].as_array().unwrap();
    let has_call_edge = edges.iter().any(|e| {
        e["type"] == "direct"
            && e["source"] == serde_json::Value::from(caller_idx.unwrap() as u64)
            && e["target"] == serde_json::Value::from(dowork_idx.unwrap() as u64)
    });
    assert!(
        has_call_edge,
        "Expected calls_procedure edge from caller_proc to do_work"
    );
}

#[test]
fn test_package_spec_no_ghost_procedure_nodes() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "pkg_spec.sql",
        r#"
        CREATE OR REPLACE PACKAGE pkg_example IS
            PROCEDURE do_work(p_id INT);
            FUNCTION get_name(p_id INT) RETURN VARCHAR2;
        END pkg_example;
    "#,
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

    let has_package = nodes.iter().any(|n| n["type"] == "package");
    assert!(
        has_package,
        "Expected a package node, got nodes: {:?}",
        nodes
    );

    let procedure_nodes: Vec<_> = nodes.iter().filter(|n| n["type"] == "procedure").collect();
    assert!(
        procedure_nodes.is_empty(),
        "Spec-only items should NOT create procedure nodes, but found: {:?}",
        procedure_nodes
    );
}

#[test]
fn test_package_spec_and_body_only_body_produces_nodes() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "pkg_full.sql",
        r#"
        CREATE OR REPLACE PACKAGE pkg_full IS
            PROCEDURE do_work(p_id INT);
            FUNCTION get_name(p_id INT) RETURN VARCHAR2;
        END pkg_full;

        CREATE OR REPLACE PACKAGE BODY pkg_full AS
            PROCEDURE do_work(p_id INT) IS
            BEGIN
                INSERT INTO t_log VALUES(p_id);
            END;
        END pkg_full;
    "#,
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

    let procedure_names: Vec<_> = nodes
        .iter()
        .filter(|n| n["type"] == "procedure")
        .filter_map(|n| n["name"].as_str())
        .collect();
    assert_eq!(
        procedure_names,
        vec!["do_work"],
        "Only body procedures should appear, not spec-only declarations"
    );

    let function_names: Vec<_> = nodes
        .iter()
        .filter(|n| {
            n["type"] == "procedure"
                && nodes
                    .iter()
                    .any(|m| m["type"] == "procedure" && m["name"] == n["name"])
        })
        .filter_map(|n| n["name"].as_str())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    assert!(
        !function_names.contains(&"get_name"),
        "get_name has no body implementation — should not appear as a node"
    );
}

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

#[test]
fn test_procedure_table_write_access() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        r#"
        CREATE PROCEDURE insert_user(p_name VARCHAR) AS $$
        BEGIN
            INSERT INTO t_users(name) VALUES(p_name);
        END;
        $$;
    "#,
    );

    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let edges = parsed["edges"].as_array().unwrap();

    let write_edges: Vec<_> = edges
        .iter()
        .filter(|e| e["type"] == "table_access")
        .filter(|e| {
            let modes = e["modes"].as_array().unwrap();
            modes.iter().any(|m| m == "write")
        })
        .collect();
    assert!(!write_edges.is_empty(), "Expected write table_access edge");

    let has_insert_kind = write_edges.iter().any(|e| {
        let wk = e["write_kinds"].as_array().unwrap();
        wk.iter().any(|k| k == "insert")
    });
    assert!(has_insert_kind, "Expected insert write_kind");
}

#[test]
fn test_procedure_table_read_access() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        r#"
        CREATE PROCEDURE get_user(p_id INT) AS $$
        BEGIN
            SELECT * FROM t_users WHERE id = p_id;
        END;
        $$;
    "#,
    );

    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let edges = parsed["edges"].as_array().unwrap();

    let read_edges: Vec<_> = edges
        .iter()
        .filter(|e| e["type"] == "table_access")
        .filter(|e| {
            let modes = e["modes"].as_array().unwrap();
            modes.iter().any(|m| m == "read")
        })
        .collect();
    assert!(!read_edges.is_empty(), "Expected read table_access edge");
}

#[test]
fn test_insert_select_read_write() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        r#"
        CREATE PROCEDURE copy_users() AS $$
        BEGIN
            INSERT INTO t_archive SELECT * FROM t_users;
        END;
        $$;
    "#,
    );

    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let edges = parsed["edges"].as_array().unwrap();
    let nodes = parsed["nodes"].as_array().unwrap();

    let table_access_edges: Vec<_> = edges
        .iter()
        .filter(|e| e["type"] == "table_access")
        .collect();
    assert!(
        table_access_edges.len() >= 2,
        "Expected >= 2 table_access edges (t_archive + t_users), got {}",
        table_access_edges.len()
    );

    // t_archive should be write (insert_select)
    let archive_idx = nodes
        .iter()
        .position(|n| n["type"] == "table" && n["name"] == "t_archive");
    let archive_edge = table_access_edges.iter().find(|e| {
        archive_idx.map_or(false, |idx| {
            e["target"] == serde_json::Value::from(idx as u64)
        })
    });
    assert!(archive_edge.is_some(), "Expected edge to t_archive");
    let modes = archive_edge.unwrap()["modes"].as_array().unwrap();
    assert!(
        modes.iter().any(|m| m == "write"),
        "t_archive should have write mode"
    );
    let wk = archive_edge.unwrap()["write_kinds"].as_array().unwrap();
    assert!(
        wk.iter().any(|k| k == "insert_select"),
        "t_archive should have insert_select write_kind"
    );

    // t_users should be read
    let users_idx = nodes
        .iter()
        .position(|n| n["type"] == "table" && n["name"] == "t_users");
    let users_edge = table_access_edges.iter().find(|e| {
        users_idx.map_or(false, |idx| {
            e["target"] == serde_json::Value::from(idx as u64)
        })
    });
    assert!(users_edge.is_some(), "Expected edge to t_users");
    let modes = users_edge.unwrap()["modes"].as_array().unwrap();
    assert!(
        modes.iter().any(|m| m == "read"),
        "t_users should have read mode"
    );
}

#[test]
fn test_view_reads_from_table() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        r#"
        CREATE VIEW v_users AS
        SELECT id, name FROM t_users WHERE status = 'ACTIVE';
    "#,
    );

    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let edges = parsed["edges"].as_array().unwrap();

    let view_to_table: Vec<_> = edges.iter().filter(|e| e["type"] == "depends_on").collect();
    assert!(
        !view_to_table.is_empty(),
        "Expected depends_on edge from view to table"
    );
}

#[test]
fn test_package_procedure_table_access() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        r#"
        CREATE OR REPLACE PACKAGE BODY pkg_ops AS
            PROCEDURE update_status(p_id INT, p_status VARCHAR) IS
            BEGIN
                UPDATE t_orders SET status = p_status WHERE id = p_id;
            END;
        END pkg_ops;
    "#,
    );

    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let edges = parsed["edges"].as_array().unwrap();

    let write_edges: Vec<_> = edges
        .iter()
        .filter(|e| e["type"] == "table_access")
        .filter(|e| {
            let modes = e["modes"].as_array().unwrap();
            modes.iter().any(|m| m == "write")
        })
        .collect();
    assert!(
        !write_edges.is_empty(),
        "Expected write table_access from package procedure"
    );

    let has_update_kind = write_edges.iter().any(|e| {
        let wk = e["write_kinds"].as_array().unwrap();
        wk.iter().any(|k| k == "update")
    });
    assert!(has_update_kind, "Expected update write_kind");
}

#[test]
fn test_create_type_node() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        r#"
        CREATE TYPE address_t AS (
            street VARCHAR(200),
            city   VARCHAR(100)
        );
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

    let type_nodes: Vec<_> = nodes.iter().filter(|n| n["type"] == "type").collect();
    assert_eq!(type_nodes.len(), 1, "Expected 1 type node");
    assert_eq!(type_nodes[0]["name"], "address_t");
    assert_eq!(type_nodes[0]["type_kind"], "composite");
}

#[test]
fn test_create_enum_type_node() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        r#"
        CREATE TYPE status_t AS ENUM ('ACTIVE', 'INACTIVE');
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

    let type_nodes: Vec<_> = nodes.iter().filter(|n| n["type"] == "type").collect();
    assert_eq!(type_nodes.len(), 1, "Expected 1 type node");
    assert_eq!(type_nodes[0]["name"], "status_t");
    assert_eq!(type_nodes[0]["type_kind"], "enum");
}

#[test]
fn test_create_sequence_node() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        r#"
        CREATE SEQUENCE user_id_seq START WITH 1 INCREMENT BY 1;
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

    let seq_nodes: Vec<_> = nodes.iter().filter(|n| n["type"] == "sequence").collect();
    assert_eq!(seq_nodes.len(), 1, "Expected 1 sequence node");
    assert_eq!(seq_nodes[0]["name"], "user_id_seq");
}

#[test]
fn test_create_index_node_and_edge() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        r#"
        CREATE UNIQUE INDEX idx_users_email ON t_users(email);
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
    let edges = parsed["edges"].as_array().unwrap();

    let index_nodes: Vec<_> = nodes.iter().filter(|n| n["type"] == "index").collect();
    assert_eq!(index_nodes.len(), 1, "Expected 1 index node");
    assert_eq!(index_nodes[0]["name"], "idx_users_email");
    assert_eq!(index_nodes[0]["table_name"], "t_users");
    assert_eq!(index_nodes[0]["unique"], true);

    let indexes_edges: Vec<_> = edges
        .iter()
        .filter(|e| e["type"] == "indexes_table")
        .collect();
    assert_eq!(indexes_edges.len(), 1, "Expected 1 indexes_table edge");
}

#[test]
fn test_create_materialized_view_node() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        r#"
        CREATE MATERIALIZED VIEW mv_summary AS
        SELECT user_id, COUNT(*) as cnt FROM t_orders GROUP BY user_id
        WITH DATA;
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
    let edges = parsed["edges"].as_array().unwrap();

    let mview_nodes: Vec<_> = nodes
        .iter()
        .filter(|n| n["type"] == "materialized_view")
        .collect();
    assert_eq!(mview_nodes.len(), 1, "Expected 1 materialized_view node");
    assert_eq!(mview_nodes[0]["name"], "mv_summary");

    let depends_on: Vec<_> = edges.iter().filter(|e| e["type"] == "depends_on").collect();
    assert!(
        !depends_on.is_empty(),
        "Expected depends_on edge from materialized view"
    );
}

#[test]
fn test_create_synonym_node_and_edge() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        r#"
        CREATE OR REPLACE PROCEDURE remote_pkg.do_work(p_id INT) AS $$
        BEGIN
            NULL;
        END;
        $$;

        CREATE SYNONYM my_work FOR remote_pkg.do_work;
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
    let edges = parsed["edges"].as_array().unwrap();

    let syn_nodes: Vec<_> = nodes.iter().filter(|n| n["type"] == "synonym").collect();
    assert_eq!(syn_nodes.len(), 1, "Expected 1 synonym node");
    assert_eq!(syn_nodes[0]["name"], "my_work");

    let alias_edges: Vec<_> = edges
        .iter()
        .filter(|e| e["type"] == "aliases_object")
        .collect();
    assert_eq!(alias_edges.len(), 1, "Expected 1 aliases_object edge");
}

#[test]
fn test_create_event_node() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        r#"
        CREATE EVENT evt_cleanup ON SCHEDULE EVERY 1 DAY DO CALL cleanup_proc();
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

    let event_nodes: Vec<_> = nodes.iter().filter(|n| n["type"] == "event").collect();
    assert_eq!(event_nodes.len(), 1, "Expected 1 event node");
    assert_eq!(event_nodes[0]["name"], "evt_cleanup");
}

#[test]
fn test_type_reference_edge() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        r#"
        CREATE TYPE address_t AS (
            street VARCHAR(200),
            city   VARCHAR(100)
        );

        CREATE OR REPLACE PROCEDURE print_address(
            p_addr address_t
        ) AS $$
        BEGIN
            RAISE NOTICE 'City: %', p_addr.city;
        END;
        $$;
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
    let edges = parsed["edges"].as_array().unwrap();
    let nodes = parsed["nodes"].as_array().unwrap();

    let type_ref_edges: Vec<_> = edges
        .iter()
        .filter(|e| e["type"] == "references_type")
        .collect();
    assert_eq!(
        type_ref_edges.len(),
        1,
        "Expected 1 references_type edge from procedure to type"
    );

    let proc_idx = nodes
        .iter()
        .position(|n| n["type"] == "procedure" && n["name"] == "print_address")
        .expect("should find print_address procedure");
    let type_idx = nodes
        .iter()
        .position(|n| n["type"] == "type" && n["name"] == "address_t")
        .expect("should find address_t type");

    assert_eq!(
        type_ref_edges[0]["source"],
        serde_json::Value::from(proc_idx as u64)
    );
    assert_eq!(
        type_ref_edges[0]["target"],
        serde_json::Value::from(type_idx as u64)
    );
}

#[test]
fn test_sequence_usage_edge_nextval() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        r#"
        CREATE SEQUENCE user_id_seq;

        CREATE OR REPLACE FUNCTION next_user_id() RETURNS BIGINT AS $$
        BEGIN
            RETURN nextval('user_id_seq');
        END;
        $$ LANGUAGE plpgsql;
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
    let edges = parsed["edges"].as_array().unwrap();

    let seq_edges: Vec<_> = edges
        .iter()
        .filter(|e| e["type"] == "uses_sequence")
        .collect();
    assert_eq!(
        seq_edges.len(),
        1,
        "Expected 1 uses_sequence edge from function to sequence"
    );
}

#[test]
fn test_sequence_usage_edge_dot_nextval() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        r#"
        CREATE SEQUENCE order_id_seq;

        CREATE OR REPLACE PROCEDURE create_order() AS $$
        BEGIN
            INSERT INTO t_orders(id) VALUES(order_id_seq.NEXTVAL);
        END;
        $$;
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
    let edges = parsed["edges"].as_array().unwrap();

    let seq_edges: Vec<_> = edges
        .iter()
        .filter(|e| e["type"] == "uses_sequence")
        .collect();
    assert!(
        !seq_edges.is_empty(),
        "Expected at least 1 uses_sequence edge for seq.NEXTVAL usage"
    );
}

#[test]
fn test_e2e_demo_all_object_types() {
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

    let node_types: std::collections::HashSet<String> = nodes
        .iter()
        .filter_map(|n| n["type"].as_str().map(String::from))
        .collect();

    assert!(
        node_types.contains("type"),
        "Expected type nodes from types_and_sequences.sql"
    );
    assert!(
        node_types.contains("sequence"),
        "Expected sequence nodes from types_and_sequences.sql"
    );
    assert!(
        node_types.contains("index"),
        "Expected index nodes from types_and_sequences.sql"
    );
    assert!(
        node_types.contains("materialized_view"),
        "Expected materialized_view nodes"
    );
    assert!(node_types.contains("synonym"), "Expected synonym nodes");
    assert!(node_types.contains("event"), "Expected event nodes");

    let edge_types: std::collections::HashSet<String> = edges
        .iter()
        .filter_map(|e| e["type"].as_str().map(String::from))
        .collect();

    assert!(
        edge_types.contains("references_type"),
        "Expected references_type edges"
    );
    assert!(
        edge_types.contains("uses_sequence"),
        "Expected uses_sequence edges"
    );
    assert!(
        edge_types.contains("indexes_table"),
        "Expected indexes_table edges"
    );
    assert!(
        edge_types.contains("aliases_object"),
        "Expected aliases_object edges"
    );
}

#[test]
fn test_intra_package_call_scope() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        r#"
        CREATE OR REPLACE PACKAGE BODY pkg_api AS
            PROCEDURE do_work(p_id INT) IS
            BEGIN
                pkg_api.helper(p_id);
            END;
            PROCEDURE helper(p_id INT) IS
            BEGIN
                NULL;
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
    let edges = parsed["edges"].as_array().unwrap();

    let direct_edges: Vec<_> = edges
        .iter()
        .filter(|e| e["type"] == "direct" || e["type"] == "intra_call")
        .collect();
    assert!(
        !direct_edges.is_empty(),
        "Expected at least one DirectCall edge, got: {:?}",
        edges
    );

    let scope = direct_edges[0].get("scope").and_then(|v| v.as_str());
    assert_eq!(
        scope,
        Some("intra"),
        "Same-package call should have scope 'intra', got: {:?}",
        direct_edges[0]
    );
}

#[test]
fn test_cross_package_call_scope() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        r#"
        CREATE OR REPLACE PACKAGE BODY pkg_api AS
            PROCEDURE do_work(p_id INT) IS
            BEGIN
                pkg_utils.format_date(SYSDATE);
            END;
        END pkg_api;

        CREATE OR REPLACE PACKAGE BODY pkg_utils AS
            FUNCTION format_date(d DATE) RETURN VARCHAR2 IS
            BEGIN
                RETURN TO_CHAR(d);
            END;
        END pkg_utils;
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
    let edges = parsed["edges"].as_array().unwrap();

    let cross_edges: Vec<_> = edges
        .iter()
        .filter(|e| e["type"] == "cross_call" || e["scope"] == "cross")
        .collect();
    assert!(
        !cross_edges.is_empty(),
        "Expected at least one cross-package DirectCall edge, got: {:?}",
        edges
    );
}

#[test]
fn test_external_call_scope() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        r#"
        CREATE PROCEDURE caller_proc() AS $$
        BEGIN
            callee_proc();
        END;
        $$;

        CREATE PROCEDURE callee_proc() AS $$
        BEGIN
            NULL;
        END;
        $$;
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
    let edges = parsed["edges"].as_array().unwrap();

    let external_edges: Vec<_> = edges
        .iter()
        .filter(|e| e["type"] == "direct" && e.get("scope").map_or(true, |s| s == "external"))
        .collect();
    assert!(
        !external_edges.is_empty(),
        "Expected at least one external DirectCall edge, got: {:?}",
        edges
    );
}

#[test]
fn test_view_depends_on_table_edge() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        r#"
        CREATE TABLE t_data (id INT, val VARCHAR(50));
        CREATE VIEW v_summary AS SELECT id FROM t_data;
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
    let edges = parsed["edges"].as_array().unwrap();

    let depends_on: Vec<_> = edges.iter().filter(|e| e["type"] == "depends_on").collect();
    assert!(
        !depends_on.is_empty(),
        "Expected depends_on edge from view to table"
    );

    let table_access: Vec<_> = edges
        .iter()
        .filter(|e| e["type"] == "table_access")
        .collect();
    assert!(
        table_access.is_empty(),
        "View should NOT produce table_access edges, only depends_on"
    );
}

#[test]
fn test_procedure_table_access_has_flow_kind() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        r#"
        CREATE TABLE t_orders (id INT, amount NUMERIC);
        CREATE PROCEDURE sp_read_orders() AS $$
        BEGIN
            SELECT * FROM t_orders;
        END;
        $$;
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
    let edges = parsed["edges"].as_array().unwrap();

    let table_access: Vec<_> = edges
        .iter()
        .filter(|e| e["type"] == "table_access")
        .collect();
    assert!(!table_access.is_empty(), "Expected table_access edge");

    let flow_kind = table_access[0].get("flow_kind").and_then(|v| v.as_str());
    assert_eq!(
        flow_kind,
        Some("dml"),
        "Procedure table access should have flow_kind 'dml'"
    );
}

#[test]
fn test_cgef_roundtrip_with_scope() {
    let dir = TempDir::new().unwrap();
    let cgef_json = r#"{
        "format_version": 1,
        "metadata": { "source": "roundtrip-test", "generated_at": "2026-04-29T00:00:00Z" },
        "nodes": [
            { "id": "p1", "type": "procedure", "key": {"schema": "pkg_api", "package": "pkg_api", "name": "do_work"}, "location": {"file": "pkg_api.sql", "line": 5} },
            { "id": "p2", "type": "procedure", "key": {"schema": "pkg_api", "package": "pkg_api", "name": "helper"}, "location": {"file": "pkg_api.sql", "line": 10} }
        ],
        "edges": [
            { "source": "p1", "target": "p2", "type": "direct", "location": {"file": "pkg_api.sql", "line": 7}, "properties": {"scope": "intra"} }
        ]
    }"#;
    let cgef_path = dir.path().join("scope_test.json");
    fs::write(&cgef_path, cgef_json).unwrap();

    let store_path = dir.path().join("imported.bincode");
    let import_output = run_codeweb(&[
        "import",
        "--file",
        cgef_path.to_str().unwrap(),
        "--output",
        store_path.to_str().unwrap(),
    ]);
    assert!(
        import_output.status.success(),
        "import stderr: {}",
        String::from_utf8_lossy(&import_output.stderr)
    );

    let stderr = String::from_utf8_lossy(&import_output.stderr);
    assert!(stderr.contains("1 edges"), "Expected 1 edge: {}", stderr);
}

#[test]
fn test_cgef_roundtrip_with_depends_on() {
    let dir = TempDir::new().unwrap();
    let cgef_json = r#"{
        "format_version": 1,
        "metadata": { "source": "roundtrip-test", "generated_at": "2026-04-29T00:00:00Z" },
        "nodes": [
            { "id": "v1", "type": "view", "key": {"schema": "public", "name": "v_active"}, "location": {"file": "views.sql", "line": 1} },
            { "id": "t1", "type": "table", "key": {"schema": "public", "name": "t_users"} }
        ],
        "edges": [
            { "source": "v1", "target": "t1", "type": "depends_on", "location": {"file": "views.sql", "line": 1} }
        ]
    }"#;
    let cgef_path = dir.path().join("depends_test.json");
    fs::write(&cgef_path, cgef_json).unwrap();

    let store_path = dir.path().join("imported.bincode");
    let import_output = run_codeweb(&[
        "import",
        "--file",
        cgef_path.to_str().unwrap(),
        "--output",
        store_path.to_str().unwrap(),
    ]);
    assert!(
        import_output.status.success(),
        "import stderr: {}",
        String::from_utf8_lossy(&import_output.stderr)
    );

    let stderr = String::from_utf8_lossy(&import_output.stderr);
    assert!(
        stderr.contains("1 edges"),
        "Expected 1 depends_on edge: {}",
        stderr
    );
    assert!(
        stderr.contains("2 nodes"),
        "Expected 2 nodes (view + table): {}",
        stderr
    );
}

#[test]
fn test_intra_package_bare_name_resolves() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        r#"
        CREATE OR REPLACE PACKAGE BODY pkg_batch AS
            PROCEDURE submit_entry(p_id INT) IS
            BEGIN
                add_job(p_id);
            END;
            PROCEDURE add_job(p_id INT) IS
            BEGIN
                NULL;
            END;
        END pkg_batch;
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
    let edges = parsed["edges"].as_array().unwrap();

    let unresolved_count = nodes.iter().filter(|n| n["kind"] == "unresolved").count();
    assert_eq!(
        unresolved_count, 0,
        "Expected zero unresolved nodes, but found one — bare name 'add_job' should resolve within the same package"
    );

    let submit_idx = nodes
        .iter()
        .position(|n| n["name"] == "submit_entry")
        .expect("expected submit_entry node");
    let addjob_idx = nodes
        .iter()
        .position(|n| n["name"] == "add_job")
        .expect("expected add_job node");

    let has_edge = edges.iter().any(|e| {
        e["type"] != "contains"
            && e["source"] == serde_json::Value::from(submit_idx as u64)
            && e["target"] == serde_json::Value::from(addjob_idx as u64)
    });
    assert!(
        has_edge,
        "Expected a call edge from submit_entry to add_job (resolved via caller-context)"
    );
}

/// Regression: ogsql-parser #70 — CDATA with XML entities (&gt;=, &lt;=)
/// caused infinite loop in parse_mapper_bytes_with_path.
#[test]
fn test_mapper_cdata_with_xml_entities_no_hang() {
    let dir = TempDir::new().unwrap();

    write_xml(
        &dir,
        "TestMapper.xml",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE mapper PUBLIC "-//mybatis.org//DTD Mapper 3.0//EN" "http://mybatis.org/dtd/mybatis-3-mapper.dtd">
<mapper namespace="test">
    <select id="queryRange" parameterType="map" resultType="map">
        <![CDATA[
            SELECT t.model_need "modelNeed"
            FROM dat_inst_oper_type_mode t
            WHERE t.operation_no = #{vOperationNo}
            AND t.inure_begin_date >= #{date}
            AND t.inure_end_date <= #{date}
        ]]>
    </select>
</mapper>"#,
    );

    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let nodes = json["nodes"].as_array().unwrap();

    assert!(
        nodes.len() >= 2,
        "Expected >= 2 nodes (mapper + table), got {}",
        nodes.len()
    );
}

use std::collections::HashSet;
use std::fs;
use tempfile::TempDir;

const PKG_NAME_CASING_MISMATCH: &str =
    include_str!("regress/package_casing_mismatch/cases/pkg_name_casing_mismatch.sql");
const PROC_NAME_CASING_MISMATCH: &str =
    include_str!("regress/package_casing_mismatch/cases/proc_name_casing_mismatch.sql");
const PKG_CALL_EDGE_CASING: &str =
    include_str!("regress/package_casing_mismatch/cases/pkg_call_edge_casing.sql");
const PROCEDURE_CASING_MISMATCH: &str =
    include_str!("regress/package_casing_mismatch/cases/procedure_casing_mismatch.sql");
const TYPE_CASING_MISMATCH: &str =
    include_str!("regress/package_casing_mismatch/cases/type_casing_mismatch.sql");
const SEQUENCE_CASING_MISMATCH: &str =
    include_str!("regress/package_casing_mismatch/cases/sequence_casing_mismatch.sql");

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

fn analyze_json(sql: &str) -> serde_json::Value {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("test.sql"), sql).unwrap();
    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).expect("failed to parse JSON output")
}

fn node_id_by_name(json: &serde_json::Value, name: &str) -> Option<usize> {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"].as_str() == Some(name))
        .and_then(|n| n["id"].as_u64())
        .map(|id| id as usize)
}

fn has_direct_edge(json: &serde_json::Value, source: &str, target: &str) -> bool {
    let (Some(src_id), Some(dst_id)) =
        (node_id_by_name(json, source), node_id_by_name(json, target))
    else {
        return false;
    };
    json["edges"].as_array().unwrap().iter().any(|e| {
        e["source"].as_u64() == Some(src_id as u64)
            && e["target"].as_u64() == Some(dst_id as u64)
            && e["type"] == "direct"
    })
}

/// Count nodes of a specific type (e.g. "package", "procedure", "function", "unresolved").
fn count_nodes_by_type(json: &serde_json::Value, node_type: &str) -> usize {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["type"].as_str() == Some(node_type))
        .count()
}

/// Count nodes matching both name AND type.
fn count_nodes_by_name_and_type(json: &serde_json::Value, name: &str, node_type: &str) -> usize {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["name"].as_str() == Some(name) && n["type"].as_str() == Some(node_type))
        .count()
}

/// Find partial nodes of type "procedure" or "function" that are partial.
fn partial_routine_nodes(json: &serde_json::Value) -> Vec<&serde_json::Value> {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| {
            let t = n["type"].as_str();
            (t == Some("procedure") || t == Some("function"))
                && n["partial"].as_bool() == Some(true)
        })
        .collect()
}

/// Find orphan nodes — nodes with zero incident edges (degree 0).
fn orphan_nodes(json: &serde_json::Value) -> Vec<&serde_json::Value> {
    let nodes = json["nodes"].as_array().unwrap();
    let edges = json["edges"].as_array().unwrap();
    let connected_ids: HashSet<usize> = edges
        .iter()
        .flat_map(|e| {
            let src = e["source"].as_u64().map(|id| id as usize);
            let dst = e["target"].as_u64().map(|id| id as usize);
            src.into_iter().chain(dst)
        })
        .collect();
    nodes
        .iter()
        .filter(|n| {
            let id = n["id"].as_u64().map(|id| id as usize);
            !connected_ids.contains(&id.unwrap_or(usize::MAX))
        })
        .collect()
}

// ── Test 1: Package name casing mismatch ──────────────────────

#[test]
fn regress_pkg_name_casing_mismatch() {
    let json = analyze_json(PKG_NAME_CASING_MISMATCH);

    // Bug: head `my_pkg` and body `MY_PKG` produce TWO package nodes.
    // After fix: exactly 1 package node.
    let pkg_count = count_nodes_by_type(&json, "package");
    assert_eq!(
        pkg_count, 1,
        "Package head 'my_pkg' and body 'MY_PKG' with mismatched casing must merge into \
         1 Package node, but found {pkg_count}. This is the casing mismatch bug."
    );

    // The single procedure node should exist and NOT be partial.
    let proc_count = count_nodes_by_name_and_type(&json, "proc_a", "procedure");
    assert_eq!(
        proc_count, 1,
        "Expected exactly 1 Procedure node 'proc_a', found {proc_count}. \
         Casing mismatch causes head-declared proc to be a separate partial node."
    );

    let partials = partial_routine_nodes(&json);
    assert!(
        partials.is_empty(),
        "Expected zero partial (orphan) routine nodes, but found: {:?}",
        partials
            .iter()
            .map(|n| (n["name"].as_str(), n["type"].as_str()))
            .collect::<Vec<_>>()
    );

    // No Unresolved nodes should be spawned.
    let unresolved = count_nodes_by_type(&json, "unresolved");
    assert_eq!(
        unresolved, 0,
        "Casing mismatch should not produce Unresolved nodes; found {unresolved}"
    );

    // No orphan nodes (degree 0).
    let orphans = orphan_nodes(&json);
    assert!(
        orphans.is_empty(),
        "Casing mismatch must not produce orphan nodes (degree 0); found: {:?}",
        orphans
            .iter()
            .map(|n| (n["name"].as_str(), n["type"].as_str()))
            .collect::<Vec<_>>()
    );
}

// ── Test 2: Procedure name casing mismatch inside package ─────

#[test]
fn regress_proc_name_casing_mismatch() {
    let json = analyze_json(PROC_NAME_CASING_MISMATCH);

    // Same package name casing, only procedure name casing differs.
    let pkg_count = count_nodes_by_type(&json, "package");
    assert_eq!(
        pkg_count, 1,
        "Expected exactly 1 Package node, found {pkg_count}"
    );

    // Head `proc_a` and body `Proc_A` should merge into 1 Procedure node.
    let proc_count = count_nodes_by_name_and_type(&json, "proc_a", "procedure");
    assert_eq!(
        proc_count, 1,
        "Expected exactly 1 Procedure node 'proc_a' (head proc_a + body Proc_A should merge), \
         found {proc_count}. Case-insensitive merge failure: head declaration and body \
         implementation create separate nodes."
    );

    // No partial nodes.
    let partials = partial_routine_nodes(&json);
    assert!(
        partials.is_empty(),
        "Procedure name casing mismatch must not produce partial nodes; found: {:?}",
        partials
            .iter()
            .map(|n| (n["name"].as_str(), n["type"].as_str()))
            .collect::<Vec<_>>()
    );
}

// ── Test 3: Casing mismatch + call edge ───────────────────────

#[test]
fn regress_pkg_call_edge_casing() {
    let json = analyze_json(PKG_CALL_EDGE_CASING);

    // After fix: 1 Package node.
    let pkg_count = count_nodes_by_type(&json, "package");
    assert_eq!(
        pkg_count, 1,
        "Expected exactly 1 Package node for head 'my_pkg' + body 'MY_PKG', found {pkg_count}"
    );

    // Function `helper` should exist and resolve correctly.
    let func_count = count_nodes_by_name_and_type(&json, "helper", "function");
    assert_eq!(
        func_count, 1,
        "Expected exactly 1 Function node 'helper' (head 'helper' + body 'Helper' should merge), \
         found {func_count}"
    );

    // No partial nodes.
    let partials = partial_routine_nodes(&json);
    assert!(
        partials.is_empty(),
        "Call edge test must not produce partial routine nodes; found: {:?}",
        partials
            .iter()
            .map(|n| (n["name"].as_str(), n["type"].as_str()))
            .collect::<Vec<_>>()
    );

    // DirectCall edge from caller_proc to helper should exist.
    assert!(
        has_direct_edge(&json, "caller_proc", "helper"),
        "Expected DirectCall edge: caller_proc -> helper. \
         Casing mismatch between head (helper) and body (Helper) may cause call resolution \
         to fail, suppressing the edge."
    );

    // No Unresolved nodes.
    let unresolved = count_nodes_by_type(&json, "unresolved");
    assert_eq!(
        unresolved, 0,
        "Call edge test must not produce Unresolved nodes; found {unresolved}"
    );

    // No orphan nodes.
    let orphans = orphan_nodes(&json);
    assert!(
        orphans.is_empty(),
        "Call edge test must not produce orphan nodes; found: {:?}",
        orphans
            .iter()
            .map(|n| (n["name"].as_str(), n["type"].as_str()))
            .collect::<Vec<_>>()
    );
}

// ── Test 4: Standalone Procedure name casing mismatch ─────────

#[test]
fn regress_procedure_casing_mismatch() {
    let json = analyze_json(PROCEDURE_CASING_MISMATCH);

    // Two CREATE PROCEDURE with my_test_proc / MY_TEST_PROC should merge into 1 node.
    let proc_count = count_nodes_by_name_and_type(&json, "my_test_proc", "procedure");
    assert_eq!(
        proc_count, 1,
        "Standalone procedure 'my_test_proc' and 'MY_TEST_PROC' must merge into 1 node, \
         found {proc_count}. builder.rs proc_index uses raw-cased RoutineId keys."
    );

    // No procedure nodes with different casing.
    let proc_count_upper = count_nodes_by_name_and_type(&json, "MY_TEST_PROC", "procedure");
    assert_eq!(
        proc_count_upper, 0,
        "No node should exist with the uppercased name; found {proc_count_upper}. \
         Both unquoted identifiers must fold to lowercase."
    );

    // No orphan nodes.
    let orphans = orphan_nodes(&json);
    assert!(
        orphans.is_empty(),
        "Procedure casing mismatch must not produce orphan nodes; found: {:?}",
        orphans
            .iter()
            .map(|n| (n["name"].as_str(), n["type"].as_str()))
            .collect::<Vec<_>>()
    );
}

// ── Test 5: Type name casing mismatch ─────────────────────────

#[test]
fn regress_type_casing_mismatch() {
    let json = analyze_json(TYPE_CASING_MISMATCH);

    // Two CREATE TYPE with my_test_type / MY_TEST_TYPE should merge into 1 node.
    let type_count = count_nodes_by_name_and_type(&json, "my_test_type", "type");
    assert_eq!(
        type_count, 1,
        "TYPE 'my_test_type' and 'MY_TEST_TYPE' must merge into 1 node, \
         found {type_count}. builder.rs type_index uses raw-case keys."
    );

    let type_count_upper = count_nodes_by_name_and_type(&json, "MY_TEST_TYPE", "type");
    assert_eq!(
        type_count_upper, 0,
        "No node should exist with the uppercased name; found {type_count_upper}."
    );

    // No orphan nodes.
    let orphans = orphan_nodes(&json);
    assert!(
        orphans.is_empty(),
        "Type casing mismatch must not produce orphan nodes; found: {:?}",
        orphans
            .iter()
            .map(|n| (n["name"].as_str(), n["type"].as_str()))
            .collect::<Vec<_>>()
    );
}

// ── Test 6: Sequence name casing mismatch ─────────────────────

#[test]
fn regress_sequence_casing_mismatch() {
    let json = analyze_json(SEQUENCE_CASING_MISMATCH);

    // Two CREATE SEQUENCE with my_test_seq / MY_TEST_SEQ should merge into 1 node.
    let seq_count = count_nodes_by_name_and_type(&json, "my_test_seq", "sequence");
    assert_eq!(
        seq_count, 1,
        "SEQUENCE 'my_test_seq' and 'MY_TEST_SEQ' must merge into 1 node, \
         found {seq_count}. builder.rs sequence_index uses raw-case keys."
    );

    let seq_count_upper = count_nodes_by_name_and_type(&json, "MY_TEST_SEQ", "sequence");
    assert_eq!(
        seq_count_upper, 0,
        "No node should exist with the uppercased name; found {seq_count_upper}."
    );

    // No orphan nodes.
    let orphans = orphan_nodes(&json);
    assert!(
        orphans.is_empty(),
        "Sequence casing mismatch must not produce orphan nodes; found: {:?}",
        orphans
            .iter()
            .map(|n| (n["name"].as_str(), n["type"].as_str()))
            .collect::<Vec<_>>()
    );
}

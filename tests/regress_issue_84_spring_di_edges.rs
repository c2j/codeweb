use std::fs;
use tempfile::TempDir;

const SERVICE_JAVA: &str =
    include_str!("regress/issue_84_spring_di_edges/cases/01_film_service.java");
const CONTROLLER_CDI_JAVA: &str =
    include_str!("regress/issue_84_spring_di_edges/cases/02_film_controller_cdi.java");
const CONTROLLER_FDI_JAVA: &str =
    include_str!("regress/issue_84_spring_di_edges/cases/03_film_controller_fdi.java");

fn codeweb_bin() -> std::path::PathBuf {
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
            return p;
        }
    }
    base.join("debug").join(bin_name)
}

fn analyze_java_files(files: &[(&str, &str)]) -> serde_json::Value {
    let dir = TempDir::new().unwrap();
    for (filename, content) in files {
        fs::write(dir.path().join(filename), content).unwrap();
    }
    let output = std::process::Command::new(codeweb_bin())
        .args(&[dir.path().to_str().unwrap(), "--format", "json"])
        .output()
        .expect("failed to run codeweb");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).expect("failed to parse JSON")
}

fn collect_node_ids_by_fqn(json: &serde_json::Value) -> Vec<(String, usize)> {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| {
            let fqn = n["fqn"].as_str().or(n["name"].as_str())?;
            let id = n["id"].as_u64()?;
            Some((fqn.to_string(), id as usize))
        })
        .collect()
}

fn fqn_matches_class(fqn: &str, class_name: &str) -> bool {
    fqn == class_name
        || fqn.ends_with(&format!(".{}", class_name))
        || fqn.contains(&format!(".{}.", class_name))
}

fn has_edge_between_classes(
    json: &serde_json::Value,
    node_ids: &[(String, usize)],
    class_a_fqn: &str,
    class_b_fqn: &str,
) -> bool {
    let a_ids: Vec<usize> = node_ids
        .iter()
        .filter(|(fqn, _)| fqn_matches_class(fqn, class_a_fqn))
        .map(|(_, id)| *id)
        .collect();
    let b_ids: Vec<usize> = node_ids
        .iter()
        .filter(|(fqn, _)| fqn_matches_class(fqn, class_b_fqn))
        .map(|(_, id)| *id)
        .collect();

    if a_ids.is_empty() || b_ids.is_empty() {
        return false;
    }

    json["edges"].as_array().unwrap().iter().any(|e| {
        let src = e["source"].as_u64().unwrap() as usize;
        let tgt = e["target"].as_u64().unwrap() as usize;
        (a_ids.contains(&src) && b_ids.contains(&tgt))
            || (a_ids.contains(&tgt) && b_ids.contains(&src))
    })
}

fn node_ids_by_type(json: &serde_json::Value, node_type: &str) -> Vec<usize> {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["type"].as_str() == Some(node_type))
        .map(|n| n["id"].as_u64().unwrap() as usize)
        .collect()
}

#[test]
#[ignore = "issue #84: Spring @Autowired/@Inject DI edges not captured — field declarations and constructor params are invisible to the call graph"]
fn regress_di_constructor_injection_creates_edge() {
    let json = analyze_java_files(&[
        ("FilmService.java", SERVICE_JAVA),
        ("FilmControllerCdi.java", CONTROLLER_CDI_JAVA),
    ]);

    let class_ids = node_ids_by_type(&json, "java_class");
    assert!(
        class_ids.len() >= 2,
        "Issue #84: expected at least 2 java_class nodes, got {}. \
         Nodes: {:?}",
        class_ids.len(),
        json["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|n| n["type"] == "java_class")
            .map(|n| n["fqn"].as_str().unwrap_or("?"))
            .collect::<Vec<_>>()
    );

    let method_ids = node_ids_by_type(&json, "java_method");
    assert!(
        !method_ids.is_empty(),
        "Issue #84: expected java_method nodes, got none"
    );

    let node_ids = collect_node_ids_by_fqn(&json);
    let has_di_edge = has_edge_between_classes(
        &json,
        &node_ids,
        "FilmControllerCdi",
        "FilmService",
    );
    assert!(
        has_di_edge,
        "Issue #84: expected edge from FilmControllerCdi to FilmService (constructor DI). \
         Current behavior: constructor parameters are not inspected by tree-sitter parser, \
         so the @Autowired FilmService dependency creates zero edges. \
         After fix: DI injection should create a calls_java or injects edge."
    );
}

#[test]
#[ignore = "issue #84: Spring @Autowired/@Inject DI edges not captured — field declarations and constructor params are invisible to the call graph"]
fn regress_di_field_injection_creates_edge() {
    let json = analyze_java_files(&[
        ("FilmService.java", SERVICE_JAVA),
        ("FilmControllerFdi.java", CONTROLLER_FDI_JAVA),
    ]);

    let class_ids = node_ids_by_type(&json, "java_class");
    assert!(
        class_ids.len() >= 2,
        "Issue #84: expected at least 2 java_class nodes, got {}",
        class_ids.len()
    );

    let node_ids = collect_node_ids_by_fqn(&json);
    let has_di_edge = has_edge_between_classes(
        &json,
        &node_ids,
        "FilmControllerFdi",
        "FilmService",
    );
    assert!(
        has_di_edge,
        "Issue #84: expected edge from FilmControllerFdi to FilmService (field DI). \
         Current behavior: field declarations are not inspected by tree-sitter parser, \
         so the @Autowired private FilmService field creates zero edges. \
         After fix: DI injection should create a calls_java or injects edge."
    );
}

#[test]
fn regress_same_class_method_call_creates_edge() {
    let plain_controller = r#"package com.example.controller;
import com.example.service.FilmService;
public class PlainController {
    public void work() {
        helper();
    }
    public void helper() {}
}"#;

    let json = analyze_java_files(&[
        ("FilmService.java", SERVICE_JAVA),
        ("PlainController.java", plain_controller),
    ]);

    let node_ids = collect_node_ids_by_fqn(&json);
    let has_call_edge = has_edge_between_classes(
        &json,
        &node_ids,
        "PlainController",
        "PlainController",
    );
    assert!(
        has_call_edge,
        "Baseline: PlainController.work() directly calls helper() \
         via MethodInvocation within the same class. A calls_java edge must exist. \
         This verifies the parser infrastructure works for direct method calls, \
         so failures in the DI regression tests are specifically due to DI edges \
         being missing, not due to the parser being broken."
    );
}

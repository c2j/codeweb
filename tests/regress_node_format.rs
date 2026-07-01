#![cfg(feature = "jsp")]

use std::path::PathBuf;
use tempfile::TempDir;

// ═══════════════════════════════════════════════════════════════
// Format-snapshot regression: verify exact JSON keys per node type.
// When format unification changes the JSON schema, these tests MUST
// be updated to reflect intentional changes (not accidental breakage).
// ═══════════════════════════════════════════════════════════════

fn run_codeweb(args: &[&str]) -> std::process::Output {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
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

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("regress")
        .join("node_format")
        .join("cases")
}

fn analyze_fixtures() -> serde_json::Value {
    let dir = TempDir::new().unwrap();
    for entry in std::fs::read_dir(fixture_dir()).unwrap() {
        let entry = entry.unwrap();
        let src = entry.path();
        let dst = dir.path().join(entry.file_name());
        std::fs::copy(&src, &dst).unwrap();
    }
    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "codeweb failed. stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&output.stderr),
        stdout
    );
    serde_json::from_str(&stdout).expect("failed to parse JSON")
}

fn nodes_of_type<'a>(json: &'a serde_json::Value, typ: &str) -> Vec<&'a serde_json::Value> {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["type"].as_str() == Some(typ))
        .collect()
}

/// Assert the node has exactly the expected set of keys (no more, no less).
fn assert_keys(node: &serde_json::Value, node_type: &str, expected: &[&str]) {
    let actual_keys: std::collections::BTreeSet<&str> = node
        .as_object()
        .unwrap()
        .keys()
        .map(|k| k.as_str())
        .collect();
    let expected_keys: std::collections::BTreeSet<&str> = expected.iter().copied().collect();

    let extras: Vec<_> = actual_keys.difference(&expected_keys).collect();
    let missing: Vec<_> = expected_keys.difference(&actual_keys).collect();

    if !extras.is_empty() || !missing.is_empty() {
        panic!(
            "Node type '{node_type}' key mismatch:\n  expected: {expected:?}\n  got:      {actual_keys:?}\n  extras:   {extras:?}\n  missing:  {missing:?}\n  node:     {node}"
        );
    }
}

// ═══════════════════════════════════════════════════════════════
// Format snapshot tests
// ═══════════════════════════════════════════════════════════════

#[test]
fn format_jsp_page_keys() {
    let json = analyze_fixtures();
    let nodes = nodes_of_type(&json, "jsp");
    assert!(!nodes.is_empty());
    for node in &nodes {
        assert_keys(
            node,
            "jsp",
            &["id", "type", "display_name", "file", "line", "url_pattern"],
        );
    }
}

#[test]
fn format_jsql_keys() {
    let json = analyze_fixtures();
    let nodes = nodes_of_type(&json, "jspsql");
    assert!(!nodes.is_empty());
    for node in &nodes {
        assert_keys(
            node,
            "jspsql",
            &["id", "type", "sql", "file", "line", "kind", "parsed"],
        );
        assert!(node["line"].is_number());
        assert!(node["parsed"].is_boolean());
        assert!(matches!(
            node["kind"].as_str(),
            Some("scriptlet" | "declaration" | "jstl_query" | "jstl_update")
        ));
    }
}

#[test]
fn format_java_class_keys() {
    let json = analyze_fixtures();
    let classes = nodes_of_type(&json, "java_class");
    assert!(!classes.is_empty(), "must have at least one JavaClass node");

    for node in &classes {
        assert_keys(
            node,
            "java_class",
            &["id", "type", "fqn", "name", "package", "file", "line"],
        );
    }
}

#[test]
fn format_java_method_keys() {
    let json = analyze_fixtures();
    let methods = nodes_of_type(&json, "java_method");
    assert!(
        !methods.is_empty(),
        "must have at least one JavaMethod node"
    );

    for node in &methods {
        assert_keys(
            node,
            "java_method",
            &[
                "id",
                "type",
                "fqn",
                "class_fqn",
                "name",
                "signature",
                "file",
                "line",
            ],
        );
    }
}

#[test]
fn format_java_sql_keys() {
    let json = analyze_fixtures();
    let java_sqls = nodes_of_type(&json, "java_sql");
    assert!(!java_sqls.is_empty(), "must have at least one JavaSql node");

    for node in &java_sqls {
        assert_keys(
            node,
            "java_sql",
            &[
                "id",
                "type",
                "class_name",
                "method_name",
                "extraction_method",
                "sql",
                "file",
                "line",
            ],
        );
        assert!(matches!(
            node["extraction_method"].as_str(),
            Some("constant" | "annotation" | "method_call")
        ));
    }
}

#[test]
fn format_mapped_statement_keys() {
    let json = analyze_fixtures();
    let mappers = nodes_of_type(&json, "mapped_statement");
    assert!(
        !mappers.is_empty(),
        "must have at least one MappedStatement node"
    );

    for node in &mappers {
        assert_keys(
            node,
            "mapped_statement",
            &[
                "id",
                "type",
                "namespace",
                "statement_id",
                "kind",
                "sql",
                "file",
                "line",
            ],
        );
        assert!(matches!(
            node["kind"].as_str(),
            Some("select" | "insert" | "update" | "delete")
        ));
    }
}

#[test]
fn format_procedure_keys() {
    let json = analyze_fixtures();
    let procs = nodes_of_type(&json, "procedure");
    assert!(!procs.is_empty(), "must have at least one Procedure node");

    for node in &procs {
        assert_keys(
            node,
            "procedure",
            &["id", "type", "name", "schema", "file", "line"],
        );
    }
}

#[test]
fn format_table_keys() {
    let json = analyze_fixtures();
    let tables = nodes_of_type(&json, "table");
    assert!(!tables.is_empty(), "must have at least one Table node");

    for node in &tables {
        assert_keys(
            node,
            "table",
            &[
                "id", "type", "name", "schema", "file", "line", "explicit", "columns",
            ],
        );
    }
}

// ═══════════════════════════════════════════════════════════════
// Cross-type comparison: JspPage vs JavaClass vs MappedStatement
// ═══════════════════════════════════════════════════════════════

#[test]
fn format_container_nodes_identifier_comparison() {
    let json = analyze_fixtures();

    let jsp_pages = nodes_of_type(&json, "jsp");
    let classes = nodes_of_type(&json, "java_class");
    let mappers = nodes_of_type(&json, "mapped_statement");

    for node in &jsp_pages {
        assert!(node.get("display_name").is_some());
        assert!(node.get("name").is_none());
        assert!(node.get("line").is_some(), "JspPage now has 'line' field");
    }

    for node in &classes {
        assert!(node.get("name").is_some());
        assert!(node.get("fqn").is_some());
        assert!(node.get("display_name").is_none());
        assert!(node.get("line").is_some());
    }

    for node in &mappers {
        assert!(node.get("namespace").is_some());
        assert!(node.get("statement_id").is_some());
        assert!(node.get("name").is_none());
        assert!(node.get("line").is_some());
    }
}

#[test]
fn format_sql_carrier_nodes_classification_comparison() {
    let json = analyze_fixtures();
    let jsqls = nodes_of_type(&json, "jspsql");
    let java_sqls = nodes_of_type(&json, "java_sql");

    for node in &jsqls {
        assert!(
            node.get("kind").is_some(),
            "JspSql uses 'kind' for classification"
        );
        assert!(
            node.get("extraction_method").is_none(),
            "JspSql does NOT use 'extraction_method'"
        );
    }

    for node in &java_sqls {
        assert!(
            node.get("extraction_method").is_some(),
            "JavaSql uses 'extraction_method' for classification"
        );
        assert!(node.get("kind").is_none(), "JavaSql does NOT use 'kind'");
    }
}

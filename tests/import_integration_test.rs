use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

fn codeweb_bin() -> PathBuf {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    // When --target is specified, cargo places artifacts under target/<triple>/
    let bin_name = if cfg!(windows) { "codeweb.exe" } else { "codeweb" };
    let entries = std::fs::read_dir(&base).unwrap_or_else(|_| panic!("no target dir"));
    for entry in entries.flatten() {
        let p = entry.path().join("debug").join(bin_name);
        if p.exists() {
            return p;
        }
    }
    base.join("debug").join(bin_name)
}

fn run(args: &[&str]) -> std::process::Output {
    std::process::Command::new(codeweb_bin())
        .args(args)
        .output()
        .expect("failed to run codeweb")
}

fn enterprise_cgef_json() -> &'static str {
    r#"{
        "format_version": 1,
        "metadata": { "source": "enterprise-scanner", "generated_at": "2026-04-15T10:00:00Z" },
        "node_schemas": {
            "dubbo_service": { "display_name": "Dubbo RPC Service", "key_fields": ["interface"] },
            "kafka_topic": { "display_name": "Kafka Topic", "key_fields": ["topic"] }
        },
        "edge_schemas": {
            "dubbo_invokes": { "display_name": "Dubbo RPC Invocation" },
            "publishes_to": { "display_name": "Publishes to Kafka" }
        },
        "nodes": [
            { "id": "proc1", "type": "procedure", "key": {"schema": "pkg_order", "name": "create_order"}, "location": {"file": "sql/pkg_order.sql", "line": 10} },
            { "id": "proc2", "type": "procedure", "key": {"schema": "pkg_order", "name": "process_payment"}, "location": {"file": "sql/pkg_order.sql", "line": 50} },
            { "id": "tbl1", "type": "table", "key": {"schema": "public", "name": "orders"} },
            { "id": "svc1", "type": "dubbo_service", "key": {"interface": "com.example.PaymentService"}, "properties": {"version": "2.0"} },
            { "id": "topic1", "type": "kafka_topic", "key": {"topic": "order-events"} }
        ],
        "edges": [
            { "source": "proc1", "target": "proc2", "type": "direct", "location": {"file": "sql/pkg_order.sql", "line": 15} },
            { "source": "proc1", "target": "tbl1", "type": "table_access", "location": {"file": "sql/pkg_order.sql", "line": 12}, "properties": {"modes": ["read", "write"], "write_kinds": ["insert"]} },
            { "source": "proc2", "target": "svc1", "type": "dubbo_invokes", "properties": {"timeout": 5000} },
            { "source": "proc1", "target": "topic1", "type": "publishes_to", "properties": {"key_serializer": "StringSerializer"} }
        ]
    }"#
}

fn standard_cgef_json() -> &'static str {
    r#"{
        "format_version": 1,
        "metadata": { "source": "codeweb-analyze", "generated_at": "2026-04-15T10:00:00Z" },
        "nodes": [
            { "id": "sp_a", "type": "procedure", "key": {"schema": "pkg_order", "name": "create_order"}, "location": {"file": "sql/a.sql", "line": 1} },
            { "id": "sp_b", "type": "procedure", "key": {"schema": "pkg_order", "name": "validate_order"}, "location": {"file": "sql/b.sql", "line": 1} }
        ],
        "edges": [
            { "source": "sp_a", "target": "sp_b", "type": "direct", "location": {"file": "sql/a.sql", "line": 5} }
        ]
    }"#
}

#[test]
fn test_import_enterprise_cgef() {
    let dir = TempDir::new().unwrap();
    let cgef_path = dir.path().join("enterprise.cgef.json");
    let output_path = dir.path().join("imported.bincode");
    fs::write(&cgef_path, enterprise_cgef_json()).unwrap();

    let out = run(&[
        "import",
        "--file",
        cgef_path.to_str().unwrap(),
        "--output",
        output_path.to_str().unwrap(),
        "--name",
        "enterprise-graph",
    ]);

    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        panic!("import failed: {}", stderr);
    }
    assert!(output_path.exists());
    assert!(stderr.contains("5 nodes"), "Expected 5 nodes: {}", stderr);
    assert!(stderr.contains("4 edges"), "Expected 4 edges: {}", stderr);
    assert!(stderr.contains("2 custom"), "{}", stderr);
}

#[test]
fn test_import_standard_cgef() {
    let dir = TempDir::new().unwrap();
    let cgef_path = dir.path().join("standard.cgef.json");
    let output_path = dir.path().join("standard.bincode");
    fs::write(&cgef_path, standard_cgef_json()).unwrap();

    let out = run(&[
        "import",
        "--file",
        cgef_path.to_str().unwrap(),
        "--output",
        output_path.to_str().unwrap(),
    ]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "import failed: {}", stderr);
    assert!(output_path.exists());
    assert!(stderr.contains("2 nodes"), "{}", stderr);
    assert!(stderr.contains("1 edges"), "{}", stderr);
}

#[test]
fn test_import_with_prefix() {
    let dir = TempDir::new().unwrap();
    let cgef_path = dir.path().join("enterprise.cgef.json");
    let output_path = dir.path().join("prefixed.bincode");
    fs::write(&cgef_path, enterprise_cgef_json()).unwrap();

    let out = run(&[
        "import",
        "--file",
        cgef_path.to_str().unwrap(),
        "--output",
        output_path.to_str().unwrap(),
        "--prefix",
        "/enterprise/module-a",
    ]);
    assert!(
        out.status.success(),
        "import with prefix failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(output_path.exists());
}

#[test]
fn test_import_invalid_version() {
    let dir = TempDir::new().unwrap();
    let bad_json = r#"{ "format_version": 99, "metadata": {"source":"t","generated_at":"2026-01-01T00:00:00Z"}, "nodes": [{"id":"n1","type":"procedure","key":{"name":"a"}}], "edges": [] }"#;
    let cgef_path = dir.path().join("bad.cgef.json");
    let output_path = dir.path().join("bad.bincode");
    fs::write(&cgef_path, bad_json).unwrap();

    let out = run(&[
        "import",
        "--file",
        cgef_path.to_str().unwrap(),
        "--output",
        output_path.to_str().unwrap(),
    ]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("error"), "Should print error: {}", stderr);
}

#[test]
fn test_merge_standard_stores() {
    let dir = TempDir::new().unwrap();

    let json_a = r#"{
        "format_version": 1,
        "metadata": {"source":"a","generated_at":"2026-01-01T00:00:00Z"},
        "nodes": [
            { "id": "n1", "type": "procedure", "key": {"schema": "pkg", "name": "do_work"}, "location": {"file": "a.sql", "line": 1} },
            { "id": "n2", "type": "table", "key": {"schema": "public", "name": "orders"} }
        ],
        "edges": [
            { "source": "n1", "target": "n2", "type": "table_access", "location": {"file": "a.sql", "line": 5}, "properties": {"modes": ["read"]} }
        ]
    }"#;

    let json_b = r#"{
        "format_version": 1,
        "metadata": {"source":"b","generated_at":"2026-01-01T00:00:00Z"},
        "nodes": [
            { "id": "n1", "type": "procedure", "key": {"schema": "pkg", "name": "do_work"}, "location": {"file": "a.sql", "line": 1} },
            { "id": "n3", "type": "procedure", "key": {"schema": "pkg", "name": "validate"}, "location": {"file": "b.sql", "line": 1} }
        ],
        "edges": [
            { "source": "n3", "target": "n1", "type": "direct", "location": {"file": "b.sql", "line": 3} }
        ]
    }"#;

    let a_cgef = dir.path().join("a.cgef.json");
    let b_cgef = dir.path().join("b.cgef.json");
    let a_store = dir.path().join("a.bincode");
    let b_store = dir.path().join("b.bincode");
    fs::write(&a_cgef, json_a).unwrap();
    fs::write(&b_cgef, json_b).unwrap();

    let out_a = run(&[
        "import",
        "--file",
        a_cgef.to_str().unwrap(),
        "--output",
        a_store.to_str().unwrap(),
    ]);
    assert!(
        out_a.status.success(),
        "{}",
        String::from_utf8_lossy(&out_a.stderr)
    );
    let out_b = run(&[
        "import",
        "--file",
        b_cgef.to_str().unwrap(),
        "--output",
        b_store.to_str().unwrap(),
    ]);
    assert!(
        out_b.status.success(),
        "{}",
        String::from_utf8_lossy(&out_b.stderr)
    );

    let merged_path = dir.path().join("merged.bincode");
    let out_m = run(&[
        "merge",
        a_store.to_str().unwrap(),
        b_store.to_str().unwrap(),
        "--output",
        merged_path.to_str().unwrap(),
        "--name",
        "combined",
    ]);
    let stderr = String::from_utf8_lossy(&out_m.stderr);
    assert!(out_m.status.success(), "merge failed: {}", stderr);
    assert!(merged_path.exists());
    assert!(stderr.contains("Merged"), "{}", stderr);
    assert!(stderr.contains("3 nodes"), "do_work deduped: {}", stderr);
    assert!(stderr.contains("2 edges"), "{}", stderr);
}

#[test]
fn test_import_nonexistent_file() {
    let out = run(&[
        "import",
        "--file",
        "/nonexistent/file.cgef.json",
        "--output",
        "/tmp/out.bincode",
    ]);
    assert!(!out.status.success());
}

#[test]
fn test_import_malformed_json() {
    let dir = TempDir::new().unwrap();
    let cgef_path = dir.path().join("bad.cgef.json");
    let output_path = dir.path().join("bad.bincode");
    fs::write(&cgef_path, "not valid json{{{").unwrap();

    let out = run(&[
        "import",
        "--file",
        cgef_path.to_str().unwrap(),
        "--output",
        output_path.to_str().unwrap(),
    ]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("error"), "{}", stderr);
}

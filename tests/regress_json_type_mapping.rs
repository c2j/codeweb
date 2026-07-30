use std::path::PathBuf;
use tempfile::TempDir;

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
        .join("fixtures")
        .join("type_mapping_regress")
}

fn analyze_json() -> serde_json::Value {
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

#[test]
fn json_export_has_type_mapping() {
    let json = analyze_json();
    let meta = &json["_meta"];
    assert!(
        meta.is_object(),
        "JSON output must have _meta object; got: {}",
        meta
    );

    let mapping = &meta["type_mapping"];
    assert!(
        mapping.is_object(),
        "_meta.type_mapping must be an object; got: {}",
        mapping
    );

    let expected: Vec<(&str, &str)> = vec![
        ("proc", "procedure"),
        ("pkg", "package"),
        ("func", "function"),
        ("table", "table"),
        ("mapper", "mapped_statement"),
        ("sql", "java_sql"),
        ("method", "java_method"),
        ("class", "java_class"),
        ("mview", "materialized_view"),
        ("builtin", "builtin_function"),
        ("trigger", "trigger"),
        ("type", "type"),
        ("seq", "sequence"),
        ("index", "index"),
        ("synonym", "synonym"),
        ("event", "event"),
        ("unres", "unresolved"),
    ];

    for (cli_tag, json_type) in &expected {
        let actual = mapping[cli_tag].as_str();
        assert_eq!(
            actual,
            Some(*json_type),
            "type_mapping['{}'] expected '{}', got {:?}",
            cli_tag,
            json_type,
            actual
        );
    }
}

#[test]
fn json_export_existing_type_fields_unchanged() {
    let json = analyze_json();
    let nodes = json["nodes"].as_array().unwrap();

    let type_values: Vec<&str> = nodes.iter().filter_map(|n| n["type"].as_str()).collect();

    assert!(!type_values.is_empty(), "no nodes in JSON output");

    for tv in &type_values {
        assert!(
            [
                "procedure",
                "function",
                "table",
                "package",
                "sequence",
                "type",
                "trigger",
                "builtin_function",
                "unresolved"
            ]
            .contains(tv),
            "unexpected node type '{}' in JSON output",
            tv
        );
    }

    for expected in &["procedure", "package", "table"] {
        assert!(
            type_values.contains(expected),
            "must have '{}' type in output",
            expected
        );
    }
}

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

#[test]
fn ndjson_export_each_line_is_valid_json() {
    let dir = TempDir::new().unwrap();
    for entry in std::fs::read_dir(fixture_dir()).unwrap() {
        let entry = entry.unwrap();
        let src = entry.path();
        let dst = dir.path().join(entry.file_name());
        std::fs::copy(&src, &dst).unwrap();
    }
    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "ndjson"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "codeweb ndjson export failed. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!stdout.is_empty(), "NDJSON output must not be empty");

    let lines: Vec<&str> = stdout.lines().collect();
    assert!(!lines.is_empty(), "NDJSON must have at least one line");

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        assert!(!trimmed.is_empty(), "NDJSON line {} is empty", i + 1);
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(trimmed);
        assert!(
            parsed.is_ok(),
            "NDJSON line {} is not valid JSON: '{}'",
            i + 1,
            trimmed
        );
    }
}

#[test]
fn ndjson_export_contains_nodes_and_edges() {
    let dir = TempDir::new().unwrap();
    for entry in std::fs::read_dir(fixture_dir()).unwrap() {
        let entry = entry.unwrap();
        let src = entry.path();
        let dst = dir.path().join(entry.file_name());
        std::fs::copy(&src, &dst).unwrap();
    }
    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "ndjson"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());

    let mut node_count = 0usize;
    let mut edge_count = 0usize;

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let obj: serde_json::Value = serde_json::from_str(trimmed).expect("valid JSON line");
        match obj["record"].as_str() {
            Some("node") => node_count += 1,
            Some("edge") => edge_count += 1,
            other => panic!("unexpected record type: {:?}", other),
        }
    }

    assert!(node_count > 0, "NDJSON must contain node records");
    assert!(edge_count > 0, "NDJSON must contain edge records");
}

#[test]
fn ndjson_node_records_have_type_tag_and_type_fields() {
    let dir = TempDir::new().unwrap();
    for entry in std::fs::read_dir(fixture_dir()).unwrap() {
        let entry = entry.unwrap();
        let src = entry.path();
        let dst = dir.path().join(entry.file_name());
        std::fs::copy(&src, &dst).unwrap();
    }
    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "ndjson"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());

    let mut found_tag = false;
    let mut found_type = false;

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let obj: serde_json::Value = serde_json::from_str(trimmed).unwrap();
        if obj["record"].as_str() == Some("node") {
            if obj.get("type_tag").is_some() {
                found_tag = true;
            }
            if obj.get("type").is_some() {
                found_type = true;
            }
        }
    }

    assert!(
        found_tag,
        "NDJSON node records must have 'type_tag' field (CLI short name)"
    );
    assert!(
        found_type,
        "NDJSON node records must have 'type' field (JSON long name)"
    );
}

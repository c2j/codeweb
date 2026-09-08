//! Regression for #159: sequence references without CREATE SEQUENCE must remain
//! visible through persisted project analysis and incremental re-analysis.

use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn codeweb_bin() -> PathBuf {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    let bin_name = if cfg!(windows) {
        "codeweb.exe"
    } else {
        "codeweb"
    };
    let entries = fs::read_dir(&base).unwrap_or_else(|_| panic!("no target dir"));
    for entry in entries.flatten() {
        let path = entry.path().join("debug").join(bin_name);
        if path.exists() {
            return path;
        }
    }
    base.join("debug").join(bin_name)
}

fn run_codeweb_in(cwd: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(codeweb_bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("failed to run codeweb")
}

fn export_json(project: &Path) -> serde_json::Value {
    let output = run_codeweb_in(
        project,
        &[
            "export",
            "--format",
            "json",
            "-p",
            project.to_str().unwrap(),
        ],
    );
    assert!(
        output.status.success(),
        "export failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("export should produce JSON")
}

fn assert_one_inferred_sequence_edge(json: &serde_json::Value) {
    let nodes = json["nodes"].as_array().unwrap();
    let sequences: Vec<_> = nodes
        .iter()
        .filter(|node| {
            node["type"].as_str() == Some("sequence")
                && node["name"].as_str() == Some("seq_batch_payment")
        })
        .collect();
    assert_eq!(sequences.len(), 1, "sequence must not be duplicated");
    assert_eq!(
        sequences[0].get("explicit"),
        None,
        "inferred sequence omits explicit (false is skipped, matching table/view JSON)"
    );
    assert_eq!(
        sequences[0].get("file"),
        None,
        "inferred sequence omits file, matching table/view JSON"
    );
    assert_eq!(sequences[0].get("line"), None);

    let sequence_id = sequences[0]["id"].as_u64().unwrap();
    let uses_sequence_edges: Vec<_> = json["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|edge| {
            edge["type"].as_str() == Some("uses_sequence")
                && edge["target"].as_u64() == Some(sequence_id)
        })
        .collect();
    assert_eq!(
        uses_sequence_edges.len(),
        1,
        "UsesSequence edge must not be duplicated"
    );
}

#[test]
fn inferred_sequence_survives_store_and_incremental_analyze_without_duplicates() {
    let temp = TempDir::new().unwrap();
    let sql_dir = temp.path().join("sql");
    fs::create_dir_all(&sql_dir).unwrap();
    fs::write(
        sql_dir.join("p.sql"),
        r#"CREATE PROCEDURE p_pay() AS $$
DECLARE v_seq BIGINT;
BEGIN
  SELECT seq_batch_payment.nextval INTO v_seq FROM sys_dummy;
END;
$$ LANGUAGE plpgsql;
"#,
    )
    .unwrap();

    let sql_dir = fs::canonicalize(sql_dir).unwrap();
    let init = run_codeweb_in(
        temp.path(),
        &["init", "issue159", "-d", sql_dir.to_str().unwrap()],
    );
    assert!(
        init.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let store_path = temp.path().join(".codeweb/store.bincode");
    assert!(store_path.exists(), "init should persist the graph store");
    assert_one_inferred_sequence_edge(&export_json(temp.path()));

    let detail = run_codeweb_in(
        temp.path(),
        &["detail", "p_pay", "-p", temp.path().to_str().unwrap()],
    );
    assert!(
        detail.status.success(),
        "detail failed: {}",
        String::from_utf8_lossy(&detail.stderr)
    );
    let detail_stdout = String::from_utf8_lossy(&detail.stdout);
    assert!(detail_stdout.contains("seq:seq_batch_payment"));
    assert!(detail_stdout.contains("[uses_seq]"));

    let mut old_store = fs::read(&store_path).unwrap();
    old_store[9..13].copy_from_slice(&8u32.to_le_bytes());
    fs::write(&store_path, old_store).unwrap();

    let analyze = run_codeweb_in(
        temp.path(),
        &["analyze", "-p", temp.path().to_str().unwrap()],
    );
    assert!(
        analyze.status.success(),
        "incremental analyze failed: {}",
        String::from_utf8_lossy(&analyze.stderr)
    );
    let rebuilt_store = fs::read(&store_path).unwrap();
    assert_eq!(&rebuilt_store[..9], b"CWEBSTORE");
    assert_eq!(
        u32::from_le_bytes(rebuilt_store[9..13].try_into().unwrap()),
        12
    );
    assert_one_inferred_sequence_edge(&export_json(temp.path()));
}

//! Regression for #143/#144: inline TRUNCATE in a procedure body and
//! `EXECUTE IMMEDIATE 'TRUNCATE …'` literals must both emit TableAccess edges.
//!
//! `#148` re-parses `PlStatement::Sql` via `parse_with_text()`, so inline DDL
//! should already be reachable. This file locks that in before lock-level work.

use std::fs;
use tempfile::TempDir;

fn run_codeweb(args: &[&str]) -> std::process::Output {
    let bin = codeweb_bin();
    std::process::Command::new(bin)
        .args(args)
        .output()
        .expect("failed to run codeweb")
}

fn run_in_dir(dir: &TempDir, args: &[&str]) -> std::process::Output {
    std::process::Command::new(codeweb_bin())
        .args(args)
        .current_dir(dir.path())
        .output()
        .expect("failed to run codeweb")
}

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

fn analyze_json(sql: &str) -> serde_json::Value {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("test.sql"), sql).unwrap();
    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    assert!(
        output.status.success(),
        "codeweb analyze failed\nstderr: {}",
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

fn has_truncate_table_access(json: &serde_json::Value, source: &str, target: &str) -> bool {
    let (Some(src_id), Some(dst_id)) =
        (node_id_by_name(json, source), node_id_by_name(json, target))
    else {
        return false;
    };
    json["edges"].as_array().unwrap().iter().any(|e| {
        e["source"].as_u64() == Some(src_id as u64)
            && e["target"].as_u64() == Some(dst_id as u64)
            && e["type"].as_str() == Some("table_access")
            && e["write_kinds"]
                .as_array()
                .map(|a| a.iter().any(|s| s.as_str() == Some("truncate")))
                .unwrap_or(false)
            && e["modes"]
                .as_array()
                .map(|a| {
                    a.iter().any(|s| {
                        s.as_str() == Some("truncate") || s.as_str() == Some("access_exclusive")
                    })
                })
                .unwrap_or(false)
    })
}

#[test]
fn inline_truncate_in_procedure_body_emits_table_access() {
    let json = analyze_json(
        r#"
        CREATE TABLE t_log (id int);
        CREATE OR REPLACE PROCEDURE p_clean IS
        BEGIN
            TRUNCATE TABLE t_log;
        END;
        /
        "#,
    );
    assert!(
        has_truncate_table_access(&json, "p_clean", "t_log"),
        "expected p_clean → t_log truncate TableAccess, edges: {:?}",
        json["edges"]
    );
}

#[test]
fn execute_immediate_truncate_literal_still_works() {
    let json = analyze_json(
        r#"
        CREATE TABLE t_log (id int);
        CREATE OR REPLACE PROCEDURE p_clean2 IS
        BEGIN
            EXECUTE IMMEDIATE 'TRUNCATE TABLE t_log';
        END;
        /
        "#,
    );
    assert!(
        has_truncate_table_access(&json, "p_clean2", "t_log"),
        "EXECUTE IMMEDIATE truncate path regressed, edges: {:?}",
        json["edges"]
    );
}

#[test]
fn conflicts_reports_truncate_vs_select_not_two_inserts() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("t.sql"),
        r#"
        CREATE TABLE t_log (id int);
        CREATE OR REPLACE PROCEDURE p_clean IS
        BEGIN
            TRUNCATE TABLE t_log;
        END;
        /
        CREATE OR REPLACE PROCEDURE p_read IS
            v int;
        BEGIN
            SELECT COUNT(*) INTO v FROM t_log;
        END;
        /
        CREATE OR REPLACE PROCEDURE p_ins1 IS
        BEGIN
            INSERT INTO t_log VALUES (1);
        END;
        /
        CREATE OR REPLACE PROCEDURE p_ins2 IS
        BEGIN
            INSERT INTO t_log VALUES (2);
        END;
        /
        "#,
    )
    .unwrap();
    let init = run_in_dir(&dir, &["init", "t", "-d", "."]);
    assert!(
        init.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let output = run_in_dir(&dir, &["conflicts", "--format", "json"]);
    assert!(
        output.status.success(),
        "conflicts failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    assert_eq!(json["schema_version"], 1);
    let conflicts = json["conflicts"].as_array().unwrap();
    let has_high = conflicts.iter().any(|c| {
        c["severity"].as_str() == Some("high") && c["table"].as_str() == Some("t_log") && {
            let a = c["proc_a"].as_str().unwrap_or("");
            let b = c["proc_b"].as_str().unwrap_or("");
            (a.contains("p_clean") && b.contains("p_read"))
                || (a.contains("p_read") && b.contains("p_clean"))
        }
    });
    assert!(
        has_high,
        "expected HIGH p_clean vs p_read on t_log, got: {conflicts:?}"
    );
    let has_insert_pair = conflicts.iter().any(|c| {
        let a = c["proc_a"].as_str().unwrap_or("");
        let b = c["proc_b"].as_str().unwrap_or("");
        (a.contains("p_ins1") && b.contains("p_ins2"))
            || (a.contains("p_ins2") && b.contains("p_ins1"))
    });
    assert!(
        !has_insert_pair,
        "two INSERTs must not be reported: {conflicts:?}"
    );

    let medium = run_in_dir(
        &dir,
        &["conflicts", "--format", "json", "--severity", "medium"],
    );
    assert!(medium.status.success());
    let med_json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&medium.stdout)).unwrap();
    let med = med_json["conflicts"].as_array().unwrap();
    let has_insert_pair_med = med.iter().any(|c| {
        let a = c["proc_a"].as_str().unwrap_or("");
        let b = c["proc_b"].as_str().unwrap_or("");
        (a.contains("p_ins1") && b.contains("p_ins2"))
            || (a.contains("p_ins2") && b.contains("p_ins1"))
    });
    assert!(
        !has_insert_pair_med,
        "two INSERTs must not be reported at medium: {med:?}"
    );
}

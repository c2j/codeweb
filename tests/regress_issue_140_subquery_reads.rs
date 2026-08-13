//! Regression for #140: table references inside subqueries must produce read
//! edges regardless of the outer statement kind (SELECT / INSERT), not just
//! UPDATE / DELETE.
//!
//! Before the fix, `visit_select`/`visit_insert` returned SkipChildren without
//! walking the expression-bearing fields, so `IN (SELECT…)`, `NOT EXISTS`,
//! SELECT-list scalar subqueries, and cursor-query subqueries were silently
//! dropped — the referenced tables became degree-0 orphans and `lineage
//! --direction upstream` missed real input tables.

use std::fs;
use std::path::Path;
use tempfile::TempDir;

const SUBQUERY_READS: &str =
    include_str!("regress/issue_140_subquery_reads/cases/subquery_reads.sql");

fn run_codeweb(args: &[&str]) -> std::process::Output {
    let bin = codeweb_bin();
    std::process::Command::new(bin)
        .args(args)
        .output()
        .expect("failed to run codeweb")
}

fn run_codeweb_in(cwd: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(codeweb_bin())
        .args(args)
        .current_dir(cwd)
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

fn has_table_access_edge(json: &serde_json::Value, source: &str, target: &str) -> bool {
    let (Some(src_id), Some(dst_id)) =
        (node_id_by_name(json, source), node_id_by_name(json, target))
    else {
        return false;
    };
    json["edges"].as_array().unwrap().iter().any(|e| {
        e["source"].as_u64() == Some(src_id as u64)
            && e["target"].as_u64() == Some(dst_id as u64)
            && e["type"].as_str() == Some("table_access")
    })
}

/// Subquery tables previously missed (degree-0 orphans) must now be read edges.
#[test]
fn regress_issue_140_subquery_reads_are_extracted() {
    let json = analyze_json(SUBQUERY_READS);

    assert!(
        has_table_access_edge(&json, "prc_repro", "t_parent"),
        "#140: t_parent (IN subquery of SELECT INTO) must be a read edge"
    );
    assert!(
        has_table_access_edge(&json, "prc_repro", "t_excl"),
        "#140: t_excl (NOT EXISTS subquery of INSERT..SELECT) must be a read edge"
    );
    assert!(
        has_table_access_edge(&json, "prc_repro", "t_scalar"),
        "#140: t_scalar (scalar subquery in SELECT list) must be a read edge"
    );
    assert!(
        has_table_access_edge(&json, "prc_repro", "t_cursor"),
        "#140: t_cursor (cursor query) must be a read edge"
    );
    // The UPDATE/EXISTS case already worked before the fix; keep guarding it.
    assert!(
        has_table_access_edge(&json, "prc_repro", "t_audit"),
        "t_audit (EXISTS subquery of UPDATE) must be a read edge"
    );
    // The procedure also writes t_out and reads/writes t_main.
    assert!(
        has_table_access_edge(&json, "prc_repro", "t_out"),
        "t_out must be a write edge"
    );
    assert!(
        has_table_access_edge(&json, "prc_repro", "t_main"),
        "t_main must be a read edge"
    );
}

/// No table node may be created for the CTE, even when it is referenced from
/// inside a subquery (the walk must stay inside the statement's CTE scope).
#[test]
fn regress_issue_140_cte_in_subquery_stays_filtered() {
    let json = analyze_json(SUBQUERY_READS);

    assert!(
        node_id_by_name(&json, "cte_sub").is_none(),
        "CTE 'cte_sub' must not become a table node"
    );
    assert!(
        !has_table_access_edge(&json, "prc_repro", "cte_sub"),
        "prc_repro must not have a table_access edge to CTE 'cte_sub'"
    );
}

/// `codeweb detail` of the procedure must list the previously-missing tables as
/// reads, and none of them may remain degree-0 orphans.
#[test]
fn regress_issue_140_detail_lists_subquery_tables_and_no_orphans() {
    let dir = TempDir::new().unwrap();
    let dir_str = dir.path().to_str().unwrap().to_string();
    fs::write(dir.path().join("test.sql"), SUBQUERY_READS).unwrap();

    // `init` creates the project (with .codeweb/store.bincode) in the cwd.
    let init = run_codeweb_in(dir.path(), &["init", "t140", "--dir", &dir_str]);
    assert!(
        init.status.success(),
        "codeweb init failed\nstderr: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let detail = run_codeweb_in(dir.path(), &["detail", "prc_repro", "-p", &dir_str]);
    assert!(
        detail.status.success(),
        "codeweb detail failed\nstderr: {}",
        String::from_utf8_lossy(&detail.stderr)
    );
    let out = String::from_utf8_lossy(&detail.stdout);
    for table in ["t_parent", "t_excl", "t_scalar", "t_cursor"] {
        assert!(
            out.contains(&format!("table:{table}")),
            "detail should list table:{table}, got:\n{out}"
        );
    }

    let orphan = run_codeweb_in(dir.path(), &["nodes", "--orphan", "-p", &dir_str]);
    assert!(
        orphan.status.success(),
        "codeweb nodes --orphan failed\nstderr: {}",
        String::from_utf8_lossy(&orphan.stderr)
    );
    let orphan_out = String::from_utf8_lossy(&orphan.stdout);
    for table in ["t_parent", "t_excl", "t_scalar", "t_cursor", "t_audit"] {
        assert!(
            !orphan_out.contains(&format!("table:{table}")),
            "table:{table} must not remain an orphan, got:\n{orphan_out}"
        );
    }
}

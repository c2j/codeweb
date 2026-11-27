//! #193: `codeweb extract` — dependency-closure-trimmed SQL definition export,
//! exercised end-to-end through the compiled binary (init → analyze → extract).

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tempfile::TempDir;

/// Committed example fixture (see tests/fixtures/extract/README.md): PKG_A.sql
/// holds procedures a1..a4 (only a1 is extracted by default; a4 is a second
/// root sharing the b1 dependency), FNC_B.sql holds functions b1/b2, and
/// TBL_C1.sql holds two tables. Dependency chain: a1/a4 → b1 → tbl_c1.
const FIXTURE_PKG_A: &str = include_str!("fixtures/extract/sql/PKG_A.sql");
const FIXTURE_FNC_B: &str = include_str!("fixtures/extract/sql/FNC_B.sql");
const FIXTURE_TBL_C1: &str = include_str!("fixtures/extract/sql/TBL_C1.sql");
const FIXTURE_TARGETS: &str = include_str!("fixtures/extract/targets.txt");

fn codeweb_bin() -> PathBuf {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    let bin_name = if cfg!(windows) {
        "codeweb.exe"
    } else {
        "codeweb"
    };
    if let Ok(entries) = fs::read_dir(&base) {
        for entry in entries.flatten() {
            let p = entry.path().join("debug").join(bin_name);
            if p.exists() {
                return p;
            }
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

fn fixture_project() -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let sql = root.join("sql");
    fs::create_dir_all(&sql).unwrap();

    fs::write(sql.join("PKG_A.sql"), FIXTURE_PKG_A).unwrap();
    fs::write(sql.join("FNC_B.sql"), FIXTURE_FNC_B).unwrap();
    fs::write(sql.join("TBL_C1.sql"), FIXTURE_TBL_C1).unwrap();

    let out = run_codeweb_in(&root, &["init", "issue193-demo", "--dir", "sql"]);
    assert!(
        out.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = run_codeweb_in(&root, &["analyze"]);
    assert!(
        out.status.success(),
        "analyze failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    (dir, root)
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Acceptance: a single root produces its full dependency closure as trimmed
/// files — PKG_A.sql contains ONLY a1, FNC_B.sql ONLY b1, TBL_C1.sql ONLY
/// tbl_c1 (byte-exact, terminators included).
#[test]
fn extract_single_root_produces_exact_trimmed_files() {
    let (_d, root) = fixture_project();
    let out = run_codeweb_in(
        &root,
        &["extract", "--node", "a1", "--output", "dependency_sources"],
    );
    assert!(
        out.status.success(),
        "extract failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert_eq!(
        read(&root.join("dependency_sources/sql/PKG_A.sql")),
        "CREATE OR REPLACE PROCEDURE a1 AS\nBEGIN\n    v := b1(1);\nEND;\n/",
        "PKG_A.sql must contain exactly the a1 definition"
    );
    assert_eq!(
        read(&root.join("dependency_sources/sql/FNC_B.sql")),
        "CREATE FUNCTION b1(p INT) RETURNS INTEGER AS $$\nBEGIN\n    INSERT INTO tbl_c1 \
         VALUES (p);\n    RETURN p;\nEND;\n$$;",
        "FNC_B.sql must contain exactly the b1 definition"
    );
    assert_eq!(
        read(&root.join("dependency_sources/sql/TBL_C1.sql")),
        "CREATE TABLE tbl_c1(id INT);",
        "TBL_C1.sql must contain exactly the tbl_c1 DDL"
    );
}

/// Original SQL files keep their exact content and mtime — extract is
/// read-only on the source tree (issue #193 requirement 3).
#[test]
fn extract_preserves_original_files_untouched() {
    let (_d, root) = fixture_project();
    let originals = ["sql/PKG_A.sql", "sql/FNC_B.sql", "sql/TBL_C1.sql"];
    let before: Vec<(String, String, SystemTime)> = originals
        .iter()
        .map(|rel| {
            let meta = fs::metadata(root.join(rel)).unwrap();
            (
                rel.to_string(),
                read(&root.join(rel)),
                meta.modified().unwrap(),
            )
        })
        .collect();

    let out = run_codeweb_in(
        &root,
        &["extract", "--node", "a1", "--output", "dependency_sources"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    for (rel, content, mtime_before) in before {
        assert_eq!(
            read(&root.join(&rel)),
            content,
            "{rel} content must be unchanged"
        );
        let mtime_after = fs::metadata(root.join(&rel)).unwrap().modified().unwrap();
        assert_eq!(
            mtime_before, mtime_after,
            "{rel} mtime must be unchanged (read-only extraction)"
        );
    }
}

/// An existing output directory is refused with a non-zero exit and without
/// touching its content; `--overwrite` clears it and re-extracts.
#[test]
fn extract_refuses_existing_output_and_overwrite_replaces() {
    let (_d, root) = fixture_project();

    // First run creates the output.
    let out = run_codeweb_in(&root, &["extract", "--node", "a1", "--output", "out"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Second run without --overwrite must fail and keep a pre-existing marker.
    fs::write(root.join("out/marker.txt"), "keep").unwrap();
    let out = run_codeweb_in(&root, &["extract", "--node", "a1", "--output", "out"]);
    assert!(
        !out.status.success(),
        "refusal must exit non-zero, stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--overwrite"),
        "refusal message must point at --overwrite: {stderr}"
    );
    assert_eq!(read(&root.join("out/marker.txt")), "keep");

    // --overwrite clears and re-extracts.
    let out = run_codeweb_in(
        &root,
        &["extract", "--node", "a1", "--output", "out", "--overwrite"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!root.join("out/marker.txt").exists(), "stale file cleared");
    assert!(root.join("out/sql/PKG_A.sql").exists(), "re-extracted");
}

/// Multiple roots union their closures; the shared b1 dependency is written
/// exactly once and same-file roots merge in source order (a1 … a4).
#[test]
fn extract_multiple_roots_share_dependency_once() {
    let (_d, root) = fixture_project();
    let out = run_codeweb_in(
        &root,
        &["extract", "--node", "a1", "--node", "a4", "--output", "out"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let pkg = read(&root.join("out/sql/PKG_A.sql"));
    assert_eq!(
        pkg,
        "CREATE OR REPLACE PROCEDURE a1 AS\nBEGIN\n    v := b1(1);\nEND;\n/\nCREATE OR \
         REPLACE PROCEDURE a4 AS\nBEGIN\n    PERFORM b1(7);\nEND;\n/",
        "a1 and a4 must merge in source order, a2/a3 trimmed"
    );
    let fnc = read(&root.join("out/sql/FNC_B.sql"));
    assert_eq!(
        fnc.matches("CREATE FUNCTION b1").count(),
        1,
        "shared dependency must appear exactly once: {fnc:?}"
    );
}

/// `--report` writes the JSON report; unresolved calls and inferred objects
/// appear there (issue #193 requirement 4).
#[test]
fn extract_report_lists_unresolved() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let sql = root.join("sql");
    fs::create_dir_all(&sql).unwrap();
    // caller → mystery_proc (unresolved) and ghost_tbl (inferred: no DDL in
    // the project).
    fs::write(
        sql.join("CALLER.sql"),
        r#"CREATE PROCEDURE caller AS $$
BEGIN
    PERFORM mystery_proc();
    INSERT INTO ghost_tbl SELECT COUNT(*) FROM ghost_tbl;
END;
$$;
"#,
    )
    .unwrap();
    let out = run_codeweb_in(&root, &["init", "unres-demo", "--dir", "sql"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = run_codeweb_in(&root, &["analyze"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = run_codeweb_in(
        &root,
        &[
            "extract",
            // Strict resolution (review on #194): the bare name 'caller' also
            // substring-matches the unresolved `mystery_proc` node, so the
            // full NodeKey is required.
            "--node",
            "proc:caller",
            "--output",
            "out",
            "--report",
            "out/report.json",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let report: Value = serde_json::from_str(&read(&root.join("out/report.json"))).unwrap();
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["direction"], "outgoing");
    let unresolved = report["unresolved"].as_array().unwrap();
    let keys: Vec<&str> = unresolved
        .iter()
        .filter_map(|u| u["key"].as_str())
        .collect();
    let joined = keys.join(" | ");
    assert!(
        joined.contains("mystery_proc"),
        "unresolved call must be listed: {joined}"
    );
    assert!(
        joined.contains("ghost_tbl"),
        "inferred table must be listed: {joined}"
    );
    assert!(joined.contains("count"), "builtin must be listed: {joined}");
    // The extractable root is still exported.
    assert!(
        read(&root.join("out/sql/CALLER.sql")).contains("CREATE PROCEDURE caller"),
        "extractable member exported despite unresolved siblings"
    );
}

/// `--direction incoming` collects callers: extracting b1 backwards yields
/// a1 and a4 definitions plus b1 itself.
#[test]
fn extract_incoming_direction_includes_callers() {
    let (_d, root) = fixture_project();
    let out = run_codeweb_in(
        &root,
        &[
            "extract",
            "--node",
            "b1",
            "--direction",
            "incoming",
            "--output",
            "out",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let pkg = read(&root.join("out/sql/PKG_A.sql"));
    assert!(
        pkg.contains("CREATE OR REPLACE PROCEDURE a1")
            && pkg.contains("CREATE OR REPLACE PROCEDURE a4"),
        "incoming closure must include the callers: {pkg:?}"
    );
    assert!(
        !pkg.contains("a2") && !pkg.contains("a3"),
        "non-callers stay out: {pkg:?}"
    );
    assert!(
        read(&root.join("out/sql/FNC_B.sql")).contains("CREATE FUNCTION b1"),
        "root definition included"
    );
}

/// `--nodes-file`: one root per line, `#` comments and blank lines skipped —
/// same union as repeating `--node`.
#[test]
fn extract_nodes_file_skips_comments_and_blank_lines() {
    let (_d, root) = fixture_project();
    fs::write(root.join("targets.txt"), FIXTURE_TARGETS).unwrap();
    let out = run_codeweb_in(
        &root,
        &["extract", "--nodes-file", "targets.txt", "--output", "out"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let pkg = read(&root.join("out/sql/PKG_A.sql"));
    assert!(
        pkg.contains("PROCEDURE a1") && pkg.contains("PROCEDURE a4"),
        "both file roots extracted: {pkg:?}"
    );
    assert_eq!(
        read(&root.join("out/sql/FNC_B.sql"))
            .matches("CREATE FUNCTION b1")
            .count(),
        1,
        "shared dependency exactly once"
    );
}

/// `--depth 0` restricts the closure to the roots themselves: only the root's
/// file is written.
#[test]
fn extract_depth_zero_limits_to_root() {
    let (_d, root) = fixture_project();
    let out = run_codeweb_in(
        &root,
        &["extract", "--node", "a1", "--depth", "0", "--output", "out"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(root.join("out/sql/PKG_A.sql").exists(), "root file written");
    assert!(
        !root.join("out/sql/FNC_B.sql").exists(),
        "depth 0 must exclude dependencies"
    );
    assert!(
        !root.join("out/sql/TBL_C1.sql").exists(),
        "depth 0 must exclude transitive deps"
    );
}

/// Fix 3 regression (review on #194): real stores can hold source paths
/// containing `..` that re-enter the project root — produced when the
/// analysis path is configured through the parent directory (e.g.
/// `paths = ["../<project>/sql"]`, as happens when analyze runs against the
/// project from a different CWD). The old code flattened EVERY such path to
/// the output root; the fix must canonicalize, map back under the root, and
/// preserve the directory layout.
#[test]
fn extract_maps_dotdot_source_paths_back_under_project_root() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let sql = root.join("sql");
    fs::create_dir_all(&sql).unwrap();
    fs::write(
        sql.join("A.sql"),
        "CREATE PROCEDURE pa AS\nBEGIN\n    NULL;\nEND;\n/\n",
    )
    .unwrap();

    let out = run_codeweb_in(&root, &["init", "dotdot-demo", "--dir", "sql"]);
    assert!(
        out.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Re-point the analysis path through the parent so stored locations
    // contain `../<root-name>/sql/...` — the shape seen in the review.
    let root_name = root.file_name().unwrap().to_string_lossy().into_owned();
    let toml_path = root.join("codeweb.toml");
    let toml = read(&toml_path);
    fs::write(
        &toml_path,
        toml.replace(
            "paths = [\"sql\"]",
            &format!("paths = [\"../{root_name}/sql\"]"),
        ),
    )
    .unwrap();

    let out = run_codeweb_in(&root, &["analyze"]);
    assert!(
        out.status.success(),
        "analyze failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = run_codeweb_in(&root, &["extract", "--node", "pa", "--output", "out"]);
    assert!(
        out.status.success(),
        "extract failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(
        root.join("out/sql/A.sql").exists(),
        "dotdot source path must map back under the root and keep its layout"
    );
    assert!(
        !root.join("out/A.sql").exists(),
        "path must NOT be flattened to the output root"
    );
    assert_eq!(
        read(&root.join("out/sql/A.sql")),
        "CREATE PROCEDURE pa AS\nBEGIN\n    NULL;\nEND;\n/"
    );
}

/// Strict root resolution (review on #194): a name matching several nodes is
/// an error with a non-zero exit — never a silent first match that could
/// export the WRONG definition. `--all-matches` is the explicit opt-in.
#[test]
fn extract_rejects_ambiguous_root_name() {
    let (_d, root) = fixture_project();
    // "a" substring-matches a1..a4 — ambiguous under strict resolution.
    let out = run_codeweb_in(
        &root,
        &[
            "extract",
            "--node",
            "a",
            "--output",
            "out",
            "--report",
            "report.json",
        ],
    );
    assert!(
        !out.status.success(),
        "ambiguous root must exit non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: Value =
        serde_json::from_str(&read(&root.join("report.json"))).expect("report written");
    assert!(
        report["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e.as_str().unwrap().contains("ambiguous")),
        "ambiguity recorded in report errors: {report}"
    );

    // --all-matches is the explicit opt-in that expands every hit.
    let out = run_codeweb_in(
        &root,
        &[
            "extract",
            "--node",
            "a",
            "--all-matches",
            "--output",
            "out2",
        ],
    );
    assert!(
        out.status.success(),
        "--all-matches must succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Regression (review on #194 round 2, blocker): when EVERY root fails to
/// resolve, the early `--report` write must still pass source protection —
/// a report path equal to an analyzed source file is refused and the file
/// is left byte-identical.
#[test]
fn extract_early_report_write_is_source_protected() {
    let (_d, root) = fixture_project();
    let target = root.join("sql/PKG_A.sql");
    let before = read(&target);

    let out = run_codeweb_in(
        &root,
        &[
            "extract",
            "--node",
            "no_such_object_anywhere",
            "--output",
            "out",
            "--report",
            "sql/PKG_A.sql",
        ],
    );
    assert!(
        !out.status.success(),
        "report on a source file must exit non-zero even when roots fail: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(
        read(&target),
        before,
        "analyzed source must not be overwritten by the early report"
    );
}

//! Display collapse of parallel callee rows. Storage stays one edge per
//! column; `detail` prints one row per neighbor. Anchor labels stay the
//! single letter `T`.

use std::fs;
use std::path::Path;
use tempfile::TempDir;

const ANCHOR_EDGES: &str =
    include_str!("regress/issue_158_type_anchor_edges/cases/anchor_edges.sql");

const FOUR_LOG_COLUMNS: &str = r#"
CREATE OR REPLACE PROCEDURE prc_log_anchors IS
    v_proc_name db_log.proc_name%TYPE;
    v_step_no db_log.step_no%TYPE;
    v_sql_txt db_log.sql_txt%TYPE;
    v_sql_param db_log.sql_param%TYPE;
BEGIN
    v_proc_name := 'x';
END;
/
"#;

fn run_in_dir(dir: &Path, args: &[&str]) -> std::process::Output {
    let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    let bin_name = if cfg!(windows) {
        "codeweb.exe"
    } else {
        "codeweb"
    };
    let bin = std::fs::read_dir(&base)
        .unwrap_or_else(|_| panic!("no target dir"))
        .flatten()
        .map(|entry| entry.path().join("debug").join(bin_name))
        .find(|p| p.exists())
        .unwrap_or_else(|| base.join("debug").join(bin_name));
    std::process::Command::new(bin)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run codeweb")
}

fn init_project(dir: &TempDir, name: &str, sql: &str) -> std::path::PathBuf {
    let root = dir.path().to_path_buf();
    fs::write(root.join("t.sql"), sql).unwrap();
    let out = run_in_dir(&root, &["init", name, "-d", "."]);
    assert!(
        out.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    root
}

fn detail(root: &Path, name: &str) -> String {
    let out = run_in_dir(root, &["detail", name]);
    assert!(
        out.status.success(),
        "detail failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn callees_section(stdout: &str) -> &str {
    stdout
        .split("── CALLEES ──")
        .nth(1)
        .unwrap_or_else(|| panic!("no CALLEES section:\n{stdout}"))
}

fn lines_containing<'a>(section: &'a str, needle: &str) -> Vec<&'a str> {
    section
        .lines()
        .filter(|line| line.to_lowercase().contains(&needle.to_lowercase()))
        .collect()
}

#[test]
fn should_print_db_log_once_with_bare_t() {
    let dir = TempDir::new().unwrap();
    let root = init_project(&dir, "db-log-collapse", FOUR_LOG_COLUMNS);
    let stdout = detail(&root, "prc_log_anchors");
    let rows = lines_containing(callees_section(&stdout), "db_log");
    assert_eq!(
        rows.len(),
        1,
        "db_log must appear once in CALLEES:\n{stdout}"
    );
    assert!(
        rows[0].contains("[T]") && !rows[0].contains("[T:"),
        "the single row keeps a bare T and does not list columns: {}",
        rows[0]
    );
}

#[test]
fn should_collapse_issue_158_fixture_callees_to_one_row_each() {
    let dir = TempDir::new().unwrap();
    let root = init_project(&dir, "issue-158-collapse", ANCHOR_EDGES);
    let stdout = detail(&root, "fnc_get_purchase_js_days");
    let section = callees_section(&stdout);

    let purchase = lines_containing(section, "par_sys_purchase");
    assert_eq!(
        purchase.len(),
        1,
        "par_sys_purchase is TableAccess plus two surviving anchors and must \
         collapse from 3 rows to 1:\n{stdout}"
    );
    assert!(
        purchase[0].contains("[R,T]"),
        "same-column anchors stay a bare T: {}",
        purchase[0]
    );

    let repurchase = lines_containing(section, "dat_trd_repurchase");
    assert_eq!(
        repurchase.len(),
        1,
        "dat_trd_repurchase has one AnchorsOn and must stay one row:\n{stdout}"
    );
    assert!(
        repurchase[0].contains("[T]") && !repurchase[0].contains("[R"),
        "anchor-only table keeps a bare T: {}",
        repurchase[0]
    );
}

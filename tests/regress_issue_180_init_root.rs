//! Issue #180: `codeweb init` must be able to anchor the project at an explicit
//! directory instead of scattering `codeweb.toml` + `.codeweb/` into the cwd.
//!
//! The real report was
//! `codeweb init baseline -d /path/to/基线代码` executed from another repo:
//! `codeweb.toml` + `.codeweb/` landed in the cwd, so every later command without
//! `-p` scanned the wrong tree.
//!
//! `-d` keeps its documented meaning (analysis source dirs, project rooted at the
//! cwd) because several existing tests and the README/user-guide depend on it;
//! `--root <dir>` is the explicit way to say "the project lives over there".

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

fn codeweb_bin() -> PathBuf {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    let bin_name = if cfg!(windows) {
        "codeweb.exe"
    } else {
        "codeweb"
    };
    if let Ok(entries) = std::fs::read_dir(&base) {
        for entry in entries.flatten() {
            let p = entry.path().join("debug").join(bin_name);
            if p.exists() {
                return p;
            }
        }
    }
    base.join("debug").join(bin_name)
}

fn run_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(codeweb_bin())
        .current_dir(dir)
        .args(args)
        .output()
        .expect("failed to run codeweb")
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

#[test]
fn init_with_root_writes_project_into_that_dir() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("cwd");
    let target = tmp.path().join("baseline");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&target).unwrap();

    let out = run_in(
        &cwd,
        &["init", "baseline", "--root", target.to_str().unwrap()],
    );
    assert!(out.status.success(), "init failed: {}", stderr_of(&out));

    assert!(
        target.join("codeweb.toml").exists(),
        "codeweb.toml must be written into --root"
    );
    assert!(
        target.join(".codeweb").is_dir(),
        ".codeweb must be written into --root"
    );
    assert!(
        !cwd.join("codeweb.toml").exists(),
        "init must not write codeweb.toml into the cwd when --root names another directory"
    );
    assert!(
        !cwd.join(".codeweb").exists(),
        "init must not write .codeweb into the cwd when --root names another directory"
    );
}

/// Regression guard for the documented `-d` contract: with no `--root`, the
/// project stays in the cwd and `-d` only registers analysis paths. Several
/// existing suites (`regress_columns`, `regress_column_lineage`, issue #140/#154/
/// #159 regressions) init this way and then drive the project from the parent.
#[test]
fn init_without_root_keeps_the_project_in_cwd() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("proj");
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("a.sql"), "SELECT 1;").unwrap();

    let out = run_in(&cwd, &["init", "app", "-d", src.to_str().unwrap()]);
    assert!(out.status.success(), "init failed: {}", stderr_of(&out));

    assert!(
        cwd.join("codeweb.toml").exists(),
        "without --root the project must stay in the cwd"
    );
    assert!(
        !src.join("codeweb.toml").exists(),
        "-d must not silently become the project root"
    );
    let toml = std::fs::read_to_string(cwd.join("codeweb.toml")).unwrap();
    assert!(
        toml.contains("a.sql") || toml.contains("src"),
        "the -d dir must be registered as an analysis path, got:\n{toml}"
    );
}

#[test]
fn init_root_refuses_populated_target_without_force() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("cwd");
    let target = tmp.path().join("baseline");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("existing.sql"), "SELECT 1;").unwrap();

    let refused = run_in(&cwd, &["init", "t", "--root", target.to_str().unwrap()]);
    assert!(
        !refused.status.success(),
        "a populated --root without codeweb.toml must be refused"
    );
    let stderr = stderr_of(&refused);
    assert!(
        stderr.contains("--force"),
        "the refusal must name the escape hatch, got: {stderr}"
    );
    assert!(
        !target.join("codeweb.toml").exists(),
        "the refusal must not leave a codeweb.toml behind"
    );

    let forced = run_in(
        &cwd,
        &["init", "t", "--root", target.to_str().unwrap(), "--force"],
    );
    assert!(
        forced.status.success(),
        "--force must be accepted: {}",
        stderr_of(&forced)
    );
    assert!(target.join("codeweb.toml").exists());
}

#[test]
fn init_root_resolves_dir_paths_inside_the_root() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("cwd");
    let target = tmp.path().join("baseline");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(target.join("sql")).unwrap();
    std::fs::write(target.join("sql").join("a.sql"), "SELECT 1;").unwrap();

    let out = run_in(
        &cwd,
        &[
            "init",
            "p",
            "--root",
            target.to_str().unwrap(),
            "-d",
            "sql",
            "--force",
        ],
    );
    assert!(out.status.success(), "init failed: {}", stderr_of(&out));

    let toml = std::fs::read_to_string(target.join("codeweb.toml")).unwrap();
    assert!(
        toml.contains("sql"),
        "with --root, -d is resolved inside the root, got:\n{toml}"
    );
    // The point of resolving `-d` inside the root is that analysis actually sees
    // the files; asserting only on the toml text would pass even if the path
    // resolved somewhere empty.
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains("1 files"),
        "the -d directory inside --root must be analyzed, got: {stderr}"
    );
    assert!(
        !cwd.join("codeweb.toml").exists(),
        "the cwd must stay untouched"
    );
}

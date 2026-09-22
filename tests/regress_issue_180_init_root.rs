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

/// Issue #180's amended contract (2026-09-22) keeps `-d` as "repeatable analysis
/// directories" and roots the project at the cwd when `--root` is omitted. That
/// includes `-d` pointing *outside* the cwd: it must still work, even when the
/// cwd already holds other things, because refusing it would be a different
/// safety rule the issue explicitly leaves out. The reported accident is fixed
/// by naming the root (`--root <基线目录> --force`), not by refusing.
#[test]
fn init_without_root_keeps_an_outside_dir_as_an_analysis_path() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("workspace");
    let elsewhere = tmp.path().join("baseline");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&elsewhere).unwrap();
    // Populated cwd: the amended contract says that alone is not a reason to refuse.
    std::fs::write(cwd.join("unrelated.txt"), "not a codeweb project").unwrap();
    std::fs::write(elsewhere.join("a.sql"), "SELECT 1;").unwrap();

    let out = run_in(&cwd, &["init", "app", "-d", elsewhere.to_str().unwrap()]);
    assert!(out.status.success(), "init failed: {}", stderr_of(&out));

    assert!(
        cwd.join("codeweb.toml").exists(),
        "without --root the project belongs to the cwd"
    );
    assert!(
        !elsewhere.join("codeweb.toml").exists(),
        "-d must not silently become the project root"
    );
    assert!(
        !elsewhere.join(".codeweb").exists(),
        "-d must not receive the store either"
    );
    let toml = std::fs::read_to_string(cwd.join("codeweb.toml")).unwrap();
    assert!(
        toml.contains("baseline"),
        "the outside dir must be registered as an analysis path, got:\n{toml}"
    );
}

/// `--root` pointing at a directory that does not exist yet creates it (amended
/// acceptance: "`/abs/project` 为空或不存在时成功").
#[test]
fn init_root_creates_a_missing_target_directory() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let target = tmp.path().join("not-yet-created");

    let out = run_in(&cwd, &["init", "fresh", "--root", target.to_str().unwrap()]);
    assert!(out.status.success(), "init failed: {}", stderr_of(&out));

    assert!(target.join("codeweb.toml").exists());
    assert!(target.join(".codeweb").is_dir());
    assert!(!cwd.join("codeweb.toml").exists());
}

/// An existing project is not something `--force` overrides: `--root` must fail
/// with or without it, and the existing `codeweb.toml` must not be rewritten
/// (amended acceptance). The repair path for a stale store is `analyze -p`.
#[test]
fn init_root_refuses_an_existing_project_even_with_force() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let target = tmp.path().join("baseline");
    std::fs::create_dir_all(&target).unwrap();
    let existing = "# hand-written\n[project]\nname = \"kept\"\n";
    std::fs::write(target.join("codeweb.toml"), existing).unwrap();

    for force in [false, true] {
        let mut args = vec!["init", "t", "--root", target.to_str().unwrap()];
        if force {
            args.push("--force");
        }
        let out = run_in(&cwd, &args);
        assert!(
            !out.status.success(),
            "an existing project must be refused (force={force}), stdout: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert_eq!(
            std::fs::read_to_string(target.join("codeweb.toml")).unwrap(),
            existing,
            "the existing codeweb.toml must not be rewritten (force={force})"
        );
    }
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

/// `--root` must accept a relative path (resolved against the cwd), and the
/// project must be usable afterwards through the plain directory path.
#[test]
fn init_root_accepts_a_relative_path_and_stays_usable() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("cwd");
    let sibling = tmp.path().join("sibling");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&sibling).unwrap();

    let out = run_in(&cwd, &["init", "rel", "--root", "../sibling"]);
    assert!(out.status.success(), "got: {}", stderr_of(&out));
    assert!(
        sibling.join("codeweb.toml").exists(),
        "codeweb.toml must land in ../sibling"
    );
    assert!(
        sibling.join(".codeweb").is_dir(),
        ".codeweb must land there too"
    );
    assert!(
        !cwd.join("codeweb.toml").exists(),
        "the cwd must stay untouched"
    );

    // The usual follow-up: address the project by its plain directory path.
    let stats = run_in(&cwd, &["stats", "-p", sibling.to_str().unwrap()]);
    assert!(
        stats.status.success(),
        "the project created via a relative --root must be usable: {}",
        stderr_of(&stats)
    );
}

/// `--root` is a statement about the target, so the guard keys on the flag:
/// `--root .` inside a populated cwd is checked too, while omitting `--root`
/// stays byte-identical to the historical behaviour.
#[test]
fn init_root_dot_in_populated_cwd_is_checked() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("proj");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::write(cwd.join("existing.sql"), "SELECT 1;").unwrap();

    let refused = run_in(&cwd, &["init", "t", "--root", "."]);
    assert!(
        !refused.status.success(),
        "an explicit --root must be checked even when it is the cwd"
    );
    assert!(
        stderr_of(&refused).contains("--force"),
        "got: {}",
        stderr_of(&refused)
    );

    // Omitting --root keeps the legacy behaviour: no check, project in the cwd.
    let legacy = run_in(&cwd, &["init", "t", "-d", "."]);
    assert!(
        legacy.status.success(),
        "without --root the legacy path must stay unchanged: {}",
        stderr_of(&legacy)
    );
    assert!(cwd.join("codeweb.toml").exists());
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

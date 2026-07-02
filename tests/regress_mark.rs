use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

// ── fixture data ──

const CASE01_SQL: &str = include_str!("regress/mark/cases/case01_simple_table/schema.sql");
const CASE01_INPUT: &str = include_str!("regress/mark/cases/case01_simple_table/input.csv");
const CASE01_EXPECTED: &str = include_str!("regress/mark/cases/case01_simple_table/expected.csv");

const CASE02_SQL: &str = include_str!("regress/mark/cases/case02_proc_chain/schema.sql");
const CASE02_INPUT: &str = include_str!("regress/mark/cases/case02_proc_chain/input.csv");
const CASE02_EXPECTED: &str = include_str!("regress/mark/cases/case02_proc_chain/expected.csv");

const CASE03_SQL: &str = include_str!("regress/mark/cases/case03_table_deep_chain/schema.sql");
const CASE03_INPUT: &str = include_str!("regress/mark/cases/case03_table_deep_chain/input.csv");
const CASE03_EXPECTED: &str =
    include_str!("regress/mark/cases/case03_table_deep_chain/expected.csv");

const CASE04_SQL: &str = include_str!("regress/mark/cases/case04_function/schema.sql");
const CASE04_INPUT: &str = include_str!("regress/mark/cases/case04_function/input.csv");
const CASE04_EXPECTED: &str = include_str!("regress/mark/cases/case04_function/expected.csv");

const CASE05_SQL: &str = include_str!("regress/mark/cases/case05_case_insensitivity/schema.sql");
const CASE05_INPUT: &str = include_str!("regress/mark/cases/case05_case_insensitivity/input.csv");
const CASE05_EXPECTED: &str =
    include_str!("regress/mark/cases/case05_case_insensitivity/expected.csv");

const CASE06_SQL: &str = include_str!("regress/mark/cases/case06_no_match/schema.sql");
const CASE06_INPUT: &str = include_str!("regress/mark/cases/case06_no_match/input.csv");
const CASE06_EXPECTED: &str = include_str!("regress/mark/cases/case06_no_match/expected.csv");

const CASE07_SQL: &str = include_str!("regress/mark/cases/case07_mixed_package/schema.sql");
const CASE07_INPUT: &str = include_str!("regress/mark/cases/case07_mixed_package/input.csv");
const CASE07_EXPECTED: &str = include_str!("regress/mark/cases/case07_mixed_package/expected.csv");

// ── helpers ──

fn codeweb_bin() -> PathBuf {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    let bin_name = if cfg!(windows) {
        "codeweb.exe"
    } else {
        "codeweb"
    };
    let entries = fs::read_dir(&base).unwrap_or_else(|_| panic!("no target dir"));
    for entry in entries.flatten() {
        let p = entry.path().join("debug").join(bin_name);
        if p.exists() {
            return p;
        }
    }
    base.join("debug").join(bin_name)
}

fn run_in_dir(dir: &TempDir, args: &[&str]) -> std::process::Output {
    std::process::Command::new(codeweb_bin())
        .args(args)
        .current_dir(dir.path())
        .output()
        .expect("failed to run codeweb")
}

fn init_project(dir: &TempDir, sql: &str, csv_input: Option<&str>) {
    fs::write(dir.path().join("schema.sql"), sql).unwrap();
    if let Some(csv) = csv_input {
        fs::write(dir.path().join("input.csv"), csv).unwrap();
    }

    let output = run_in_dir(dir, &["init", "test-project", "-d", "."]);
    assert!(
        output.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_mark(dir: &TempDir, node: &str, csv_file: &str) -> String {
    let output = run_in_dir(
        dir,
        &[
            "mark", "--node", node, "--csv", csv_file, "--output", "out.csv",
        ],
    );
    assert!(
        output.status.success(),
        "mark failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::read_to_string(dir.path().join("out.csv")).unwrap()
}

fn normalize_csv_lines(csv: &str) -> Vec<Vec<String>> {
    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_reader(csv.as_bytes());
    let mut rows = Vec::new();
    for result in reader.records() {
        let record = result.unwrap();
        rows.push(record.iter().map(|s| s.to_string()).collect());
    }
    rows
}

fn assert_csv_eq(actual: &str, expected: &str, case_name: &str) {
    let actual_rows = normalize_csv_lines(actual);
    let expected_rows = normalize_csv_lines(expected);

    assert_eq!(
        actual_rows.len(),
        expected_rows.len(),
        "[{}] row count mismatch: actual {} rows, expected {} rows\nactual:\n{}\nexpected:\n{}",
        case_name,
        actual_rows.len(),
        expected_rows.len(),
        actual,
        expected,
    );

    for (i, (a_row, e_row)) in actual_rows.iter().zip(expected_rows.iter()).enumerate() {
        assert_eq!(
            a_row, e_row,
            "[{}] row {} mismatch\nactual:   {:?}\nexpected: {:?}",
            case_name, i, a_row, e_row,
        );
    }
}

// ── tests ──

#[test]
fn regress_mark_case01_simple_table() {
    let dir = TempDir::new().unwrap();
    init_project(&dir, CASE01_SQL, Some(CASE01_INPUT));

    let actual = run_mark(&dir, "orders", "input.csv");
    assert_csv_eq(&actual, CASE01_EXPECTED, "case01");
}

#[test]
fn regress_mark_case02_proc_chain() {
    let dir = TempDir::new().unwrap();
    init_project(&dir, CASE02_SQL, Some(CASE02_INPUT));

    let actual = run_mark(&dir, "proc_c", "input.csv");
    assert_csv_eq(&actual, CASE02_EXPECTED, "case02");
}

#[test]
fn regress_mark_case03_table_deep_chain() {
    let dir = TempDir::new().unwrap();
    init_project(&dir, CASE03_SQL, Some(CASE03_INPUT));

    let actual = run_mark(&dir, "accounts", "input.csv");
    assert_csv_eq(&actual, CASE03_EXPECTED, "case03");
}

#[test]
fn regress_mark_case04_function() {
    let dir = TempDir::new().unwrap();
    init_project(&dir, CASE04_SQL, Some(CASE04_INPUT));

    let actual = run_mark(&dir, "calc_tax", "input.csv");
    assert_csv_eq(&actual, CASE04_EXPECTED, "case04");
}

#[test]
fn regress_mark_case05_case_insensitivity() {
    let dir = TempDir::new().unwrap();
    init_project(&dir, CASE05_SQL, Some(CASE05_INPUT));

    let actual = run_mark(&dir, "orders", "input.csv");
    assert_csv_eq(&actual, CASE05_EXPECTED, "case05");
}

#[test]
fn regress_mark_case06_no_match() {
    let dir = TempDir::new().unwrap();
    init_project(&dir, CASE06_SQL, Some(CASE06_INPUT));

    // Target a node that is NOT in the graph
    let actual = run_mark(&dir, "nonexistent_table", "input.csv");
    assert_csv_eq(&actual, CASE06_EXPECTED, "case06");
}

#[test]
fn regress_mark_case07_mixed_package() {
    let dir = TempDir::new().unwrap();
    init_project(&dir, CASE07_SQL, Some(CASE07_INPUT));

    let actual = run_mark(&dir, "orders", "input.csv");
    assert_csv_eq(&actual, CASE07_EXPECTED, "case07");
}

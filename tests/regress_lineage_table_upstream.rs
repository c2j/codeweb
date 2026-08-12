use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

fn run_codeweb(args: &[&str]) -> std::process::Output {
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

fn write_sql(dir: &TempDir, filename: &str, sql: &str) -> PathBuf {
    let path = dir.path().join(filename);
    fs::write(&path, sql).unwrap();
    path
}

fn init_and_analyze_project(dir: &TempDir) {
    // First create the .codeweb directory and codeweb.toml
    let codeweb_dir = dir.path().join(".codeweb");
    fs::create_dir_all(&codeweb_dir).unwrap();

    let config_path = codeweb_dir.join("codeweb.toml");
    fs::write(
        &config_path,
        "[project]\nname = \"test-project\"\nversion = \"0.0.1\"\n",
    )
    .unwrap();

    // Use the legacy codeweb API to analyze the directory
    // This will create a .codeweb/store.bincode file
    let output = run_codeweb(&[
        dir.path().to_str().unwrap(),
        "--format",
        "json",
    ]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("legacy analyze stderr: {}", stderr);

    assert!(
        output.status.success(),
        "initial analysis failed: {}",
        stderr
    );
}

#[test]
fn test_lineage_table_upstream_simple() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        r#"
CREATE TABLE jsmx_temp(fund_code VARCHAR2(10), qsje NUMBER);
CREATE TABLE dat_trd_qfii_chinaclear(fund_code VARCHAR2(10), cjje NUMBER);
CREATE PROCEDURE prc_copy AS BEGIN
  INSERT INTO dat_trd_qfii_chinaclear(fund_code, cjje)
  SELECT fund_code, qsje FROM jsmx_temp;
END;
"#,
    );

    init_and_analyze_project(&dir);

    // Run lineage command
    let output = run_codeweb(&[
        "lineage",
        "dat_trd_qfii_chinaclear",
        "--direction",
        "upstream",
        "--project",
        dir.path().to_str().unwrap(),
        "--format",
        "tree",
    ]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("stdout: {}", stdout);
    eprintln!("stderr: {}", stderr);

    assert!(
        output.status.success(),
        "lineage command failed: {}",
        stderr
    );

    // Verify the output contains key table names
    assert!(stdout.contains("dat_trd_qfii_chinaclear") || stdout.contains("DAT_TRD_QFII_CHINACLEAR"));
    assert!(stdout.contains("jsmx_temp") || stdout.contains("JSMX_TEMP"));
}

#[test]
fn test_lineage_table_downstream_simple() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        r#"
CREATE TABLE source_tbl(id NUMBER, amount NUMBER);
CREATE TABLE mid_tbl(id NUMBER, sum_amount NUMBER);
CREATE TABLE dest_tbl(id NUMBER, equity_amount NUMBER);

CREATE PROCEDURE prc_step2 AS BEGIN
  INSERT INTO mid_tbl(id, sum_amount)
  SELECT id, SUM(amount) FROM source_tbl GROUP BY id;
END;

CREATE PROCEDURE prc_step3 AS BEGIN
  INSERT INTO dest_tbl(id, equity_amount)
  SELECT id, sum_amount FROM mid_tbl;
END;
"#,
    );

    init_and_analyze_project(&dir);

    // Run lineage command with downstream
    let output = run_codeweb(&[
        "lineage",
        "source_tbl",
        "--direction",
        "downstream",
        "--project",
        dir.path().to_str().unwrap(),
        "--format",
        "tree",
    ]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("stdout: {}", stdout);
    eprintln!("stderr: {}", stderr);

    assert!(
        output.status.success(),
        "lineage command failed: {}",
        stderr
    );

    // Verify the output contains table names
    assert!(stdout.contains("source_tbl") || stdout.contains("SOURCE_TBL"));
}

#[test]
fn test_lineage_table_json_format() {
    let dir = TempDir::new().unwrap();
    write_sql(
        &dir,
        "test.sql",
        r#"
CREATE TABLE tbl_a(id NUMBER);
CREATE TABLE tbl_b(id NUMBER);
CREATE PROCEDURE prc_ab AS BEGIN
  INSERT INTO tbl_b SELECT * FROM tbl_a;
END;
"#,
    );

    init_and_analyze_project(&dir);

    // Run lineage command with JSON format
    let output = run_codeweb(&[
        "lineage",
        "tbl_b",
        "--direction",
        "upstream",
        "--project",
        dir.path().to_str().unwrap(),
        "--format",
        "json",
    ]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("stdout: {}", stdout);
    eprintln!("stderr: {}", stderr);

    assert!(
        output.status.success(),
        "lineage command failed: {}",
        stderr
    );

    // Verify JSON is valid and contains expected fields
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(&stdout);
    assert!(parsed.is_ok(), "invalid JSON output");
    let json = parsed.unwrap();
    assert!(json["node"].is_string());
    assert!(json["via"].is_array());
    assert!(json["children"].is_array());
}

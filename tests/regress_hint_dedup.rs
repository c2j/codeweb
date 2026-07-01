use std::collections::HashMap;
use std::fs;
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

fn count_builtins(json: &serde_json::Value) -> Vec<(String, String)> {
    let mut builtins = Vec::new();
    for node in json["nodes"].as_array().unwrap() {
        if node["type"].as_str() == Some("builtin_function") {
            builtins.push((
                node["name"].as_str().unwrap_or("?").to_string(),
                node["domain"].as_str().unwrap_or("?").to_string(),
            ));
        }
    }
    builtins
}

fn name_counts(builtins: &[(String, String)]) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for (name, _domain) in builtins {
        *counts.entry(name.to_lowercase()).or_default() += 1;
    }
    counts
}

#[test]
fn hint_set_not_duplicated() {
    let dir = TempDir::new().unwrap();

    fs::write(
        dir.path().join("hint_set.sql"),
        "CREATE OR REPLACE PROCEDURE proc_set_hint() AS $$
        BEGIN
            FOR r IN (SELECT /*+ set(enable_hashjoin off) */ * FROM t1) LOOP
                NULL;
            END LOOP;
        END;
        $$ LANGUAGE plpgsql;\n",
    )
    .unwrap();

    let output = run_codeweb(&[
        dir.path().to_str().unwrap(),
        "--format",
        "json",
        "--sql-only",
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "codeweb failed. stderr:\n{}",
        stderr
    );

    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    let builtins = count_builtins(&json);

    eprintln!("All BuiltinFunction nodes: {:#?}", builtins);

    let counts = name_counts(&builtins);
    let set_count = counts.get("set").copied().unwrap_or(0);
    assert_eq!(
        set_count, 1,
        "Expected 1 'set' BuiltinFunction node, found {}. stderr:\n{:>80}",
        set_count, stderr
    );
}

#[test]
fn hint_with_args_not_duplicated() {
    let dir = TempDir::new().unwrap();

    fs::write(
        dir.path().join("hint_wlm.sql"),
        "CREATE OR REPLACE PROCEDURE proc_wlm_hint() AS $$
        BEGIN
            FOR r IN (SELECT /*+ wlmrule(\"100,500,1\") */ * FROM t1) LOOP
                NULL;
            END LOOP;
        END;
        $$ LANGUAGE plpgsql;\n",
    )
    .unwrap();

    let output = run_codeweb(&[
        dir.path().to_str().unwrap(),
        "--format",
        "json",
        "--sql-only",
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "codeweb failed. stderr:\n{}",
        stderr
    );

    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    let builtins = count_builtins(&json);

    let counts = name_counts(&builtins);
    let wlm_count = counts.get("wlmrule").copied().unwrap_or(0);
    assert_eq!(
        wlm_count, 1,
        "Expected 1 'wlmrule' BuiltinFunction node, found {}",
        wlm_count
    );
}

#[test]
fn hint_not_confused_with_regular_builtin() {
    let dir = TempDir::new().unwrap();

    fs::write(
        dir.path().join("set_conflict.sql"),
        "CREATE OR REPLACE PROCEDURE proc_set_conflict() AS $$
        BEGIN
            FOR r IN (SELECT /*+ set(enable_hashjoin off) */ * FROM t1) LOOP
                NULL;
            END LOOP;
            PERFORM set_config('search_path', 'public', false);
        END;
        $$ LANGUAGE plpgsql;\n",
    )
    .unwrap();

    let output = run_codeweb(&[
        dir.path().to_str().unwrap(),
        "--format",
        "json",
        "--sql-only",
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "codeweb failed. stderr:\n{}",
        stderr
    );

    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    let builtins = count_builtins(&json);

    eprintln!("All BuiltinFunction nodes: {:#?}", builtins);

    // "set" hint should exist once
    let counts = name_counts(&builtins);
    let set_count = counts.get("set").copied().unwrap_or(0);
    assert_eq!(
        set_count, 1,
        "Expected 1 'set' BuiltinFunction node (hint only), found {}. stderr:\n{:>80}",
        set_count, stderr
    );

    // set_config should exist once (separate regular builtin)
    let set_config_count = counts.get("set_config").copied().unwrap_or(0);
    assert_eq!(
        set_config_count, 1,
        "Expected 1 'set_config' BuiltinFunction node, found {}",
        set_config_count
    );
}

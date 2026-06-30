use std::fs;
use tempfile::TempDir;

const CTE_BASIC: &str = include_str!("regress/cte_not_table/cases/cte_basic.sql");

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

fn analyze_json(sql: &str) -> serde_json::Value {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("test.sql"), sql).unwrap();
    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).expect("failed to parse JSON output")
}

fn node_exists_by_name_and_type(json: &serde_json::Value, name: &str, node_type: &str) -> bool {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n["name"].as_str() == Some(name) && n["type"].as_str() == Some(node_type))
}

fn table_node_names(json: &serde_json::Value) -> Vec<String> {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["type"].as_str() == Some("table"))
        .filter_map(|n| n["name"].as_str().map(|s| s.to_string()))
        .collect()
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
            && e["type"] == "table_access"
    })
}

#[test]
fn regress_cte_not_table_node() {
    let json = analyze_json(CTE_BASIC);

    assert!(
        node_exists_by_name_and_type(&json, "orders", "table"),
        "Real table 'orders' must appear as a table node"
    );
    assert!(
        node_exists_by_name_and_type(&json, "customers", "table"),
        "Real table 'customers' must appear as a table node"
    );

    assert!(
        node_exists_by_name_and_type(&json, "process_orders", "procedure"),
        "Procedure 'process_orders' must appear"
    );
    assert!(
        node_exists_by_name_and_type(&json, "process_multiple", "procedure"),
        "Procedure 'process_multiple' must appear"
    );
    assert!(
        node_exists_by_name_and_type(&json, "process_joined", "procedure"),
        "Procedure 'process_joined' must appear"
    );

    let cte_names = ["cte_orders", "cte_customers", "cte_joined"];
    for cte_name in &cte_names {
        assert!(
            !node_exists_by_name_and_type(&json, cte_name, "table"),
            "CTE '{cte_name}' must NOT appear as a table node"
        );
    }

    let tables = table_node_names(&json);
    assert!(
        tables.contains(&"orders".to_string()),
        "Expected 'orders' in table nodes, got: {:?}",
        tables
    );
    assert!(
        tables.contains(&"customers".to_string()),
        "Expected 'customers' in table nodes, got: {:?}",
        tables
    );
    for cte_name in &cte_names {
        assert!(
            !tables.contains(&cte_name.to_string()),
            "CTE '{cte_name}' leaked into table nodes. Table nodes: {:?}",
            tables
        );
    }
}

#[test]
fn regress_cte_table_access_edges_to_real_tables() {
    let json = analyze_json(CTE_BASIC);

    assert!(
        has_table_access_edge(&json, "process_orders", "orders"),
        "process_orders must have table_access edge to orders (table inside CTE body)"
    );
    assert!(
        has_table_access_edge(&json, "process_multiple", "orders"),
        "process_multiple must have table_access edge to orders"
    );
    assert!(
        has_table_access_edge(&json, "process_multiple", "customers"),
        "process_multiple must have table_access edge to customers"
    );
    assert!(
        has_table_access_edge(&json, "process_joined", "orders"),
        "process_joined must have table_access edge to orders"
    );
    assert!(
        has_table_access_edge(&json, "process_joined", "customers"),
        "process_joined must have table_access edge to customers"
    );

    for proc_name in &["process_orders", "process_multiple", "process_joined"] {
        for cte_name in &["cte_orders", "cte_customers", "cte_joined"] {
            assert!(
                !has_table_access_edge(&json, proc_name, cte_name),
                "'{proc_name}' must NOT have table_access edge to CTE '{cte_name}'"
            );
        }
    }
}

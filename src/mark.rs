use std::collections::HashSet;
use std::path::Path;

use petgraph::graph::NodeIndex;
use petgraph::Direction;

use crate::error::Result;
use crate::graph::key::NodeKey;
use crate::graph::store::GraphStore;
use crate::graph::Node;

/// Build the incoming transitive closure of `target` — all upstream nodes reachable
/// via Incoming edges (callers, TableAccess sources, etc.), including `target` itself.
pub fn build_callers_set(store: &GraphStore, target: NodeIndex) -> HashSet<NodeIndex> {
    let graph = store.graph();
    let mut visited = HashSet::new();
    let mut frontier = vec![target];
    visited.insert(target);

    while let Some(node) = frontier.pop() {
        for neighbor in graph.neighbors_directed(node, Direction::Incoming) {
            if visited.insert(neighbor) {
                frontier.push(neighbor);
            }
        }
    }
    visited
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchType {
    Direct,
    Indirect,
    None,
}

#[derive(Debug, Clone)]
pub struct MatchResult {
    pub match_type: MatchType,
    pub matched_by: Option<String>,
}

impl MatchResult {
    pub fn csv_values(&self) -> (String, String) {
        let match_str = match self.match_type {
            MatchType::Direct => "direct".to_string(),
            MatchType::Indirect => "indirect".to_string(),
            MatchType::None => String::new(),
        };
        let by_str = self.matched_by.clone().unwrap_or_default();
        (match_str, by_str)
    }
}

/// Classify a single CSV row's `sql_text` against a target node and its callers set.
///
/// Priority: direct > indirect (name) > indirect (fingerprint).
/// Returns the first match found.
pub fn classify_row(
    sql_text: &str,
    target_idx: NodeIndex,
    target_node: &Node,
    callers: &HashSet<NodeIndex>,
    store: &GraphStore,
) -> MatchResult {
    // --- Direct match ---
    match target_node {
        Node::Table { name, .. } | Node::View { name, .. } => {
            if contains_table_name(sql_text, name) {
                return MatchResult {
                    match_type: MatchType::Direct,
                    matched_by: Some(NodeKey::from_node(target_node).to_string()),
                };
            }
        }
        Node::Procedure { id, .. } | Node::Function { id, .. } => {
            if contains_routine_call(sql_text, &id.name) {
                return MatchResult {
                    match_type: MatchType::Direct,
                    matched_by: Some(NodeKey::from_node(target_node).to_string()),
                };
            }
        }
        _ => {}
    }

    // --- Indirect match: iterate callers set ---
    let graph = store.graph();
    for &caller_idx in callers {
        if caller_idx == target_idx {
            continue;
        }
        let caller_node = &graph[caller_idx];

        // 1) Name match: CSV SQL text contains a CALL/EXECUTE to this caller routine
        if let Some(routine_name) = try_extract_routine_name(caller_node) {
            if contains_routine_call(sql_text, routine_name) {
                return MatchResult {
                    match_type: MatchType::Indirect,
                    matched_by: Some(NodeKey::from_node(caller_node).to_string()),
                };
            }
        }

        // 2) Fingerprint match: CSV SQL text matches caller's body_sql
        if let Some(body_sql) = try_extract_body_sql(caller_node) {
            if sql_text_fingerprint_matches(sql_text, body_sql) {
                return MatchResult {
                    match_type: MatchType::Indirect,
                    matched_by: Some(NodeKey::from_node(caller_node).to_string()),
                };
            }
        }
    }

    MatchResult {
        match_type: MatchType::None,
        matched_by: None,
    }
}

/// Case-insensitive token-level match: ensures `orders` matches `SELECT * FROM orders`
/// but NOT `SELECT * FROM customer_orders`.
fn contains_table_name(sql_text: &str, table_name: &str) -> bool {
    let lower_sql = sql_text.to_lowercase();
    let lower_name = table_name.to_lowercase();
    for token in sql_tokens(&lower_sql) {
        if token == lower_name {
            return true;
        }
    }
    false
}

/// Detect a routine call in SQL text (CALL / EXECUTE / EXEC / SELECT func(...)).
fn contains_routine_call(sql_text: &str, routine_name: &str) -> bool {
    let lower_sql = sql_text.to_lowercase();
    let lower_name = routine_name.to_lowercase();

    // Helper: check if the routine name appears as a bare name or
    // qualified name (token ending with ".name") after a CALL/EXEC/etc.
    let name_matches = |prefix: &str| -> bool {
        // exact: "CALL name" or "CALL name("
        let bare = format!("{}{}", prefix, lower_name);
        if lower_sql.contains(&bare) {
            let after = &lower_sql[lower_sql.find(&bare).unwrap() + bare.len()..];
            if after.is_empty()
                || after.starts_with('(')
                || after.starts_with(' ')
                || after.starts_with(';')
            {
                return true;
            }
        }
        // qualified: "CALL pkg.name" — check if any token ends with ".name"
        let dot_name = format!(".{}", lower_name);
        for token in sql_tokens(&lower_sql) {
            if token.ends_with(&dot_name) {
                // Verify it's preceded by the call prefix
                if let Some(pos) = lower_sql.find(token) {
                    let before = &lower_sql[..pos].trim_end();
                    if before.ends_with(prefix.trim_end()) {
                        return true;
                    }
                }
            }
        }
        false
    };

    if name_matches("call ") {
        return true;
    }
    if name_matches("execute ") || name_matches("exec ") {
        return true;
    }
    if name_matches("perform ") {
        return true;
    }
    // SELECT func_name(...) — only match if func_name is followed by '('
    let with_paren = format!("{}(", lower_name);
    if lower_sql.contains(&with_paren) {
        // Check it's after SELECT/select
        if lower_sql.contains(&format!("select {}", lower_name))
            || lower_sql.contains(&format!("select {}", with_paren))
        {
            return true;
        }
        // Also check qualified: "select pkg.func_name("
        let dot_paren = format!(".{}(", lower_name);
        if lower_sql.contains(&dot_paren) {
            return true;
        }
    }

    false
}

/// Split SQL text into tokens by non-identifier characters.
fn sql_tokens(sql: &str) -> Vec<&str> {
    sql.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| !t.is_empty())
        .collect()
}

/// Try to extract the routine name from a Node (Procedure or Function).
fn try_extract_routine_name(node: &Node) -> Option<&str> {
    match node {
        Node::Procedure { id, .. } | Node::Function { id, .. } => Some(&id.name),
        _ => None,
    }
}

/// Try to extract body_sql from a Node for fingerprint matching.
fn try_extract_body_sql(node: &Node) -> Option<&str> {
    match node {
        Node::Procedure { body_sql, .. } | Node::Function { body_sql, .. } => {
            body_sql.first().map(|bs| bs.sql_text.as_str())
        }
        _ => None,
    }
}

/// Check if `sql_text` matches `body_sql` after normalization.
/// Uses the same normalize_for_matching logic as the store's search_by_sql.
fn sql_text_fingerprint_matches(sql_text: &str, body_sql: &str) -> bool {
    let norm_sql = crate::graph::store::normalize_for_matching(&sql_text.to_lowercase());
    let norm_body = crate::graph::store::normalize_for_matching(&body_sql.to_lowercase());

    // Body SQL is typically short (single DML statement). Check if the
    // normalized body appears as a substring of the normalized CSV SQL.
    if norm_body.len() >= 10 && norm_sql.contains(&norm_body) {
        return true;
    }
    // Also check if the normalized SQL appears within the body (less likely but cheap)
    if norm_sql.len() >= 10 && norm_body.contains(&norm_sql) {
        return true;
    }

    false
}

/// Normalize a CSV header name to a canonical form for matching:
/// lowercase, underscores → spaces, collapse whitespace.
/// "Unique SQL Id" → "unique sql id"
/// "unique_sql_id"   → "unique sql id"
fn normalize_header(h: &str) -> String {
    h.trim()
        .to_lowercase()
        .replace('_', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Entry point: read CSV, classify each row, write annotated CSV.
pub fn process_mark(
    store: &GraphStore,
    node_name: &str,
    csv_path: &Path,
    output_path: Option<&Path>,
) -> Result<()> {
    // 1) Resolve target node
    let matches = store.search_nodes(node_name);
    let target_info: Option<(NodeIndex, Node)> = if matches.is_empty() {
        eprintln!("No nodes matching '{}'", node_name);
        eprintln!(
            "Try `codeweb nodes -s {}` to find available nodes.",
            node_name
        );
        None
    } else {
        if matches.len() > 1 {
            eprintln!("Multiple nodes match '{}':", node_name);
            for (i, (_, name)) in matches.iter().enumerate() {
                eprintln!("  {}: {}", i + 1, name);
            }
            eprintln!("Using first match: {}", matches[0].1);
        }
        let (idx, _) = &matches[0];
        Some((*idx, store.graph()[*idx].clone()))
    };

    // 2) Build callers set (only if target resolved)
    let callers = target_info
        .as_ref()
        .map(|(idx, _)| build_callers_set(store, *idx))
        .unwrap_or_default();

    // 3) Read CSV
    let csv_content =
        std::fs::read_to_string(csv_path).map_err(|e| crate::error::CodeWebError::FileRead {
            path: csv_path.to_path_buf(),
            source: e,
        })?;

    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_reader(csv_content.as_bytes());

    let headers: Vec<String> = reader
        .headers()
        .map_err(|e| crate::error::CodeWebError::ExportError {
            message: format!("CSV header error: {}", e),
        })?
        .iter()
        .map(|s| s.to_string())
        .collect();

    // Validate required columns
    let has_sql_id = headers
        .iter()
        .any(|h| normalize_header(h) == "unique sql id");
    let has_sql_text = headers
        .iter()
        .any(|h| normalize_header(h) == "sql text");
    if !has_sql_id || !has_sql_text {
        return Err(crate::error::CodeWebError::ExportError {
            message: format!(
                "CSV missing required columns. Expected: Unique SQL Id (or unique_sql_id), SQL Text (or sql_text). Found: {}",
                headers.join(", ")
            ),
        });
    }

    // 4) Classify each row
    let mut output_rows: Vec<(Vec<String>, MatchResult)> = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|e| crate::error::CodeWebError::ExportError {
            message: format!("CSV parse error: {}", e),
        })?;
        let fields: Vec<String> = record.iter().map(|s| s.to_string()).collect();

        let sql_text = fields
            .iter()
            .zip(headers.iter())
            .find(|(_, h)| normalize_header(h) == "sql text")
            .map(|(v, _)| v.as_str())
            .unwrap_or("");

        let result = if let Some((target_idx, ref target_node)) = target_info {
            classify_row(sql_text, target_idx, target_node, &callers, store)
        } else {
            MatchResult {
                match_type: MatchType::None,
                matched_by: None,
            }
        };
        output_rows.push((fields, result));
    }

    // 5) Write output
    let writer: Box<dyn std::io::Write> = if let Some(out_path) = output_path {
        let file =
            std::fs::File::create(out_path).map_err(|e| crate::error::CodeWebError::FileRead {
                path: out_path.to_path_buf(),
                source: e,
            })?;
        Box::new(file)
    } else {
        Box::new(std::io::stdout())
    };
    let mut wtr = csv::WriterBuilder::new().from_writer(writer);

    // Write header
    let mut out_headers = headers.clone();
    out_headers.extend_from_slice(&[
        "codeweb_match".to_string(),
        "codeweb_matched_by".to_string(),
    ]);
    wtr.write_record(&out_headers)
        .map_err(|e| crate::error::CodeWebError::ExportError {
            message: format!("CSV write error: {}", e),
        })?;

    // Write rows
    for (fields, result) in &output_rows {
        let (match_str, by_str) = result.csv_values();
        let mut row = fields.clone();
        row.push(match_str);
        row.push(by_str);
        wtr.write_record(&row)
            .map_err(|e| crate::error::CodeWebError::ExportError {
                message: format!("CSV write error: {}", e),
            })?;
    }
    wtr.flush()
        .map_err(|e| crate::error::CodeWebError::ExportError {
            message: format!("CSV flush error: {}", e),
        })?;

    if let Some(out_path) = output_path {
        eprintln!("Output written to {}", out_path.display());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contains_table_name_exact_match() {
        assert!(contains_table_name(
            "SELECT * FROM orders WHERE id = 1",
            "orders"
        ));
    }

    #[test]
    fn test_contains_table_name_case_insensitive() {
        assert!(contains_table_name("SELECT * FROM ORDERS", "orders"));
        assert!(contains_table_name("select * from orders", "ORDERS"));
    }

    #[test]
    fn test_contains_table_name_no_substring_false_positive() {
        // "orders" should NOT match inside "customer_orders"
        assert!(!contains_table_name(
            "SELECT * FROM customer_orders",
            "orders"
        ));
    }

    #[test]
    fn test_contains_table_name_in_insert() {
        assert!(contains_table_name(
            "INSERT INTO orders (id) VALUES (1)",
            "orders"
        ));
    }

    #[test]
    fn test_contains_routine_call_call() {
        assert!(contains_routine_call(
            "CALL update_orders()",
            "update_orders"
        ));
    }

    #[test]
    fn test_contains_routine_call_call_no_parens() {
        assert!(contains_routine_call("CALL update_orders", "update_orders"));
    }

    #[test]
    fn test_contains_routine_call_execute() {
        assert!(contains_routine_call(
            "EXECUTE update_orders(1)",
            "update_orders"
        ));
    }

    #[test]
    fn test_contains_routine_call_exec() {
        assert!(contains_routine_call("EXEC update_orders", "update_orders"));
    }

    #[test]
    fn test_contains_routine_call_select_func() {
        assert!(contains_routine_call("SELECT calc_tax(100)", "calc_tax"));
    }

    #[test]
    fn test_contains_routine_call_no_false_positive_table() {
        // "SELECT orders" without paren should NOT match as routine call
        assert!(!contains_routine_call("SELECT * FROM orders", "orders"));
    }

    #[test]
    fn test_contains_routine_call_case_insensitive() {
        assert!(contains_routine_call(
            "call UPDATE_ORDERS()",
            "update_orders"
        ));
    }

    #[test]
    fn test_contains_routine_call_perform() {
        assert!(contains_routine_call(
            "PERFORM update_orders()",
            "update_orders"
        ));
    }
}

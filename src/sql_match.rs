//! SQL matching utilities shared by trace-sql and mark commands.
//!
//! Provides SQL normalization, wildcard matching, Jaccard similarity,
//! DML keyword classification, and fingerprint hashing.

use std::collections::HashSet;

use petgraph::graph::NodeIndex;

// ── SQL normalization pipeline ──

/// Collapse consecutive whitespace into single spaces, replace ogsql-parser internal
/// placeholder markers (`__XML_PARAM_*__`, `__XML_RAW_*__`) with `?`, then remove spaces
/// around SQL operators, parentheses, and commas so that formatting differences don't
/// prevent a match (e.g. `user_id = ?` vs `user_id=?`, `TO_CHAR( x , y )` vs `TO_CHAR(x,y)`).
fn strip_line_comments(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for line in s.lines() {
        if let Some(pos) = line.find("--") {
            result.push_str(line[..pos].trim_end());
        } else {
            result.push_str(line);
        }
        result.push(' ');
    }
    result.trim_end().to_string()
}

fn strip_block_comments(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut pos = 0;
    while pos < s.len() {
        let remaining = &s[pos..];
        if let Some(start) = remaining.find("/*") {
            result.push_str(&remaining[..start]);
            let after_start = &remaining[start + 2..];
            if let Some(end) = after_start.find("*/") {
                pos += start + 2 + end + 2;
            } else {
                pos = s.len();
            }
        } else {
            result.push_str(remaining);
            break;
        }
    }
    result
}

fn replace_string_literals(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\'' {
            result.push('?');
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' {
                    i += 1;
                    if i < chars.len() && chars[i] == '\'' {
                        i += 1;
                    } else {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

fn replace_number_literals(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            if i > 0 && (chars[i - 1].is_ascii_alphabetic() || chars[i - 1] == '_') {
                result.push(chars[i]);
                i += 1;
                continue;
            }
            result.push('?');
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

fn strip_where_one_equals_one(s: &str) -> String {
    let lower = s.to_lowercase();
    let patterns = ["where 1=1 and ", "where 1 = 1 and "];
    for pat in &patterns {
        if let Some(pos) = lower.find(pat) {
            let prefix = &s[..pos];
            let rest = &s[pos + pat.len()..];
            return format!("{}where {}", prefix, rest.trim_start());
        }
    }
    s.to_string()
}

fn collapse_operator_spaces(s: &str) -> String {
    s.replace(" >= ", ">=")
        .replace(" <= ", "<=")
        .replace(" <> ", "<>")
        .replace(" != ", "!=")
        .replace(" = ", "=")
        .replace(" > ", ">")
        .replace(" < ", "<")
        .replace(" - ", "-")
        .replace(" + ", "+")
        .replace(" * ", "*")
        .replace("( ", "(")
        .replace(" )", ")")
        .replace(" ,", ",")
        .replace(", ", ",")
        .replace(" =", "=")
        .replace("= ", "=")
        .replace(" >", ">")
        .replace("> ", ">")
        .replace(" <", "<")
        .replace("< ", "<")
}

fn replace_xml_placeholders(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut pos = 0;

    while pos < s.len() {
        let remaining = &s[pos..];

        let (prefix, prefix_len) = if remaining.starts_with("__xml_param_") {
            ("__xml_param_", 12)
        } else if remaining.starts_with("__xml_raw_") {
            ("__xml_raw_", 10)
        } else {
            let c = remaining.chars().next().unwrap();
            result.push(c);
            pos += c.len_utf8();
            continue;
        };

        let after_prefix = &s[pos + prefix_len..];
        if let Some(end) = after_prefix.find("__") {
            result.push('?');
            pos += prefix_len + end + 2;
        } else {
            result.push_str(prefix);
            pos += prefix_len;
        }
    }

    result
}

pub(crate) fn normalize_for_matching(s: &str) -> String {
    let s = strip_line_comments(s);
    let s = strip_block_comments(&s);
    let s = strip_where_one_equals_one(&s);
    let s = replace_string_literals(&s);
    let s = replace_number_literals(&s);
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = s.trim_end_matches(';').to_string();
    let s = replace_xml_placeholders(&s);
    collapse_operator_spaces(&s)
}

fn normalize_for_matching_pre_collapse(s: &str) -> String {
    let s = strip_line_comments(s);
    let s = strip_block_comments(&s);
    let s = strip_where_one_equals_one(&s);
    let s = replace_string_literals(&s);
    let s = replace_number_literals(&s);
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn sql_fingerprint(sql: &str) -> String {
    let normalized = normalize_for_matching(&sql.to_lowercase());
    blake3::hash(normalized.as_bytes()).to_hex().to_string()
}

// ── Table extraction ──

pub(crate) fn extract_table_name(normalized: &str) -> Option<&str> {
    if let Some(rest) = normalized.strip_prefix("update ") {
        if let Some(end) = rest.find(" set ") {
            let table_part = &rest[..end];
            return Some(table_part.split_whitespace().next().unwrap_or(""));
        }
    }
    if let Some(rest) = normalized.strip_prefix("delete from ") {
        let table_part = if let Some(end) = rest.find(" where ") {
            &rest[..end]
        } else {
            rest
        };
        return Some(table_part.split_whitespace().next().unwrap_or(""));
    }
    if let Some(rest) = normalized.strip_prefix("insert into ") {
        let table_part = if let Some(end) = rest.find(" values") {
            &rest[..end]
        } else if let Some(end) = rest.find('(') {
            &rest[..end]
        } else if let Some(end) = rest.find(" select") {
            &rest[..end]
        } else {
            rest
        };
        return Some(table_part.split_whitespace().next().unwrap_or(""));
    }
    if let Some(rest) = normalized.strip_prefix("merge into ") {
        if let Some(end) = rest.find(" using ") {
            let table_part = &rest[..end];
            return Some(table_part.split_whitespace().next().unwrap_or(""));
        }
    }
    if let Some(pos) = normalized.find(" from ") {
        let after_from = &normalized[pos + 6..];
        let table_part = if let Some(end) = after_from.find(" where ") {
            &after_from[..end]
        } else if let Some(end) = after_from.find(" group ") {
            &after_from[..end]
        } else if let Some(end) = after_from.find(" order ") {
            &after_from[..end]
        } else if let Some(end) = after_from.find(" having ") {
            &after_from[..end]
        } else if let Some(end) = after_from.find(" limit ") {
            &after_from[..end]
        } else if let Some(end) = after_from.find(" union ") {
            &after_from[..end]
        } else {
            after_from
        };
        return Some(table_part.split_whitespace().next().unwrap_or(""));
    }
    None
}

// ── DML keyword classification ──

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SqlKeyword {
    Select,
    Insert,
    Update,
    Delete,
    Merge,
    With,
    Other,
    Empty,
}

impl SqlKeyword {
    fn extract(normalized: &str) -> Self {
        let first_word = normalized
            .split(|c: char| !c.is_ascii_alphabetic())
            .next()
            .unwrap_or("");
        match first_word {
            "select" => Self::Select,
            "insert" => Self::Insert,
            "update" => Self::Update,
            "delete" => Self::Delete,
            "merge" => Self::Merge,
            "with" => Self::With,
            "" => Self::Empty,
            _ => Self::Other,
        }
    }

    pub(crate) fn is_compatible(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Select | Self::With, Self::Select | Self::With) => true,
            (Self::Other | Self::Empty, _) | (_, Self::Other | Self::Empty) => true,
            _ if self == other => true,
            _ => false,
        }
    }
}

// ── PreparedQuery ──

fn tables_compatible(query_table: &Option<String>, sql_normalized: &str) -> bool {
    let query_table = match query_table {
        Some(t) => t.as_str(),
        None => return true,
    };
    if query_table == "?" {
        return true;
    }
    match extract_table_name(sql_normalized) {
        None | Some("?") => true,
        Some(sql_table) => sql_table == query_table,
    }
}

/// Query normalized and pre-computed once, then reused for every node comparison.
pub(crate) struct PreparedQuery {
    normalized: String,
    has_wildcard: bool,
    segments: Vec<String>,
    keyword: SqlKeyword,
    table: Option<String>,
}

impl PreparedQuery {
    pub(crate) fn new(query: &str) -> Self {
        let lower = query.to_lowercase();
        let normalized = normalize_for_matching(&lower);
        let has_wildcard = normalized.contains('?');
        let segments: Vec<String> = if has_wildcard {
            normalized
                .split('?')
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        } else {
            Vec::new()
        };
        let keyword = SqlKeyword::extract(&normalized);
        let pre_collapse = normalize_for_matching_pre_collapse(&lower);
        let table = extract_table_name(&pre_collapse).map(String::from);
        Self {
            normalized,
            has_wildcard,
            segments,
            keyword,
            table,
        }
    }

    /// Check if `sql_text` matches this prepared query with wildcard support.
    pub(crate) fn matches(&self, sql_text: &str) -> bool {
        let sql_lower = normalize_for_matching(&sql_text.to_lowercase());

        let sql_kw = SqlKeyword::extract(&sql_lower);
        if !self.keyword.is_compatible(&sql_kw) {
            return false;
        }

        if sql_lower.contains(&self.normalized) {
            return true;
        }

        if !tables_compatible(&self.table, &sql_lower) {
            return false;
        }

        let sql_has_wc = sql_lower.contains('?');

        if !sql_has_wc && !self.has_wildcard {
            return false;
        }

        if self.has_wildcard && find_query_segments_in_sql(&sql_lower, &self.segments) {
            return true;
        }

        if sql_has_wc && find_sql_segments_in_query(&sql_lower, &self.normalized, self.has_wildcard)
        {
            return true;
        }

        {
            let sql_pre_collapse = normalize_for_matching_pre_collapse(&sql_text.to_lowercase());
            if tables_compatible(&self.table, &sql_pre_collapse)
                && jaccard_similarity(&self.normalized, &sql_lower) >= 0.8
            {
                return true;
            }
        }

        false
    }

    /// Compute a relevance score in [0.0, 1.0] for SQL text against this query.
    pub(crate) fn score(&self, sql_text: &str) -> f64 {
        let sql_norm = normalize_for_matching(&sql_text.to_lowercase());
        compute_relevance(&self.normalized, &sql_norm, self.keyword, &self.table)
    }
}

// ── Jaccard similarity ──

fn jaccard_similarity(a: &str, b: &str) -> f64 {
    let tokens_a: HashSet<&str> = a
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| !t.is_empty())
        .collect();
    let tokens_b: HashSet<&str> = b
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| !t.is_empty())
        .collect();

    if tokens_a.is_empty() && tokens_b.is_empty() {
        return 1.0;
    }

    let intersection = tokens_a.intersection(&tokens_b).count();
    let union = tokens_a.union(&tokens_b).count();

    intersection as f64 / union as f64
}

// ── Scoring ──

fn extract_type_prefix(display_key: &str) -> &str {
    display_key.split(':').next().unwrap_or(display_key)
}

fn compute_relevance(
    query_norm: &str,
    sql_norm: &str,
    query_kw: SqlKeyword,
    query_table: &Option<String>,
) -> f64 {
    if sql_norm == query_norm {
        return 1.0;
    }
    if sql_norm.contains(query_norm) {
        return 0.95;
    }

    let jaccard = jaccard_similarity(query_norm, sql_norm);

    let kw_bonus = if query_kw.is_compatible(&SqlKeyword::extract(sql_norm)) {
        0.10
    } else {
        0.0
    };

    let table_bonus = if tables_compatible(query_table, sql_norm) {
        0.05
    } else {
        0.0
    };

    (jaccard * 0.85 + kw_bonus + table_bonus).clamp(0.0, 1.0)
}

pub(crate) fn sort_scored_results(results: &mut [(NodeIndex, String, f64)]) {
    results.sort_by(|a, b| {
        b.2.partial_cmp(&a.2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| extract_type_prefix(&a.1).cmp(extract_type_prefix(&b.1)))
            .then_with(|| a.1.cmp(&b.1))
    });
}

// ── Wildcard segment matching ──

const MIN_PART_LEN: usize = 4;
const SOLO_MIN_LEN: usize = 6;

fn find_query_segments_in_sql(sql: &str, segments: &[String]) -> bool {
    if segments.is_empty() {
        return false;
    }
    let mut pos = 0;
    for part in segments {
        match sql[pos..].find(part.as_str()) {
            Some(p) => pos += p + part.len(),
            None => return false,
        }
    }
    true
}

fn try_stripped_part(query: &str, pos: usize, part: &str) -> Option<(usize, usize)> {
    let stripped = strip_sql_segment(part);
    if stripped.len() >= MIN_PART_LEN && stripped.len() < part.len() {
        if let Some(p) = query[pos..].find(stripped) {
            return Some((p, stripped.len()));
        }
    }
    None
}

fn strip_sql_segment(part: &str) -> &str {
    let s = strip_trailing_quoted(part);
    let s = s.trim_end_matches(|c: char| !c.is_ascii_alphabetic() && c != '_' && c != '.');
    if s.len() >= part.len() {
        return part;
    }
    s
}

fn strip_trailing_quoted(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut i = bytes.len();
    while i > 0 {
        match bytes[i - 1] {
            b'\'' => {
                if let Some(open) = s[..i - 1].rfind('\'') {
                    i = open;
                } else {
                    break;
                }
            }
            b' ' | b')' | b',' => {
                i -= 1;
            }
            _ => break,
        }
    }
    if i < s.len() {
        &s[..i]
    } else {
        s
    }
}

fn find_sql_segments_in_query(sql: &str, query: &str, query_has_wildcard: bool) -> bool {
    let sql_parts: Vec<&str> = sql.split('?').filter(|s| !s.is_empty()).collect();
    if sql_parts.is_empty() {
        return false;
    }
    if sql_parts.len() == 1 {
        return query.contains(sql_parts[0]);
    }

    let sig_parts: Vec<&str> = sql_parts
        .into_iter()
        .filter(|p| p.len() >= MIN_PART_LEN)
        .collect();
    if sig_parts.is_empty() {
        return false;
    }
    if sig_parts.len() == 1 {
        let p = sig_parts[0];
        if p.len() >= SOLO_MIN_LEN && query.contains(p) {
            return true;
        }
        let stripped = strip_sql_segment(p);
        return stripped.len() >= SOLO_MIN_LEN
            && stripped.len() < p.len()
            && query.contains(stripped);
    }

    let mut pos = 0;
    let mut count = 0;
    let mut solo_len: usize = 0;

    for part in &sig_parts {
        let matched = match query[pos..].find(*part) {
            Some(p) => Some((p, part.len())),
            None => try_stripped_part(query, pos, part),
        };
        match matched {
            Some((p, len)) => {
                if count > 0 && p == 0 {
                    break;
                }
                pos += p + len;
                solo_len = len;
                count += 1;
            }
            None => break,
        }
    }

    let threshold = if count == 1 {
        solo_len >= SOLO_MIN_LEN
    } else {
        count >= 2
    };
    if threshold {
        if query_has_wildcard {
            let tail = query[pos..].trim_start_matches('?').trim();
            if tail.is_empty() {
                return true;
            }
        } else if count == 1 {
            let tail = query[pos..].trim();
            if tail.is_empty() {
                return true;
            }
        } else {
            return true;
        }
    }

    false
}

// ── test-only helper ──

#[cfg(test)]
fn sql_text_matches(sql_text: &str, query_lower: &str) -> bool {
    let prepared = PreparedQuery::new(query_lower);
    prepared.matches(sql_text)
}

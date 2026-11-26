//! #193: `codeweb extract` — dependency-closure-trimmed SQL definition export.
//!
//! Given one or more graph nodes ("roots"), compute the dependency closure
//! (outgoing by default) and export the **original SQL definition text** of
//! every closure member that has a source definition, trimmed per file.
//!
//! Design (issue #193, compatibility requirement 5): statement boundaries are
//! NOT stored in the graph. `SourceLocation` keeps only the defining
//! statement's start line (1-based, identical line numbering to
//! ogsql-parser's `StatementInfo.start_line`). At extraction time the
//! involved source files are re-parsed once each and the node's
//! `location.line` is matched against a `start_line → span` index built from
//! the fresh parse, so existing stores keep working without a STORE_VERSION
//! bump or re-analysis.

use ogsql_parser::StatementInfo;

/// A statement's source span in 1-based line / 1-based byte-column units,
/// exactly as reported by ogsql-parser's `StatementInfo`. The end position is
/// exclusive: the covered bytes are `[offset(start), offset(end))` of the
/// decoded source text (see [`slice_span`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct LineSpan {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
}

impl LineSpan {
    fn from_info(info: &StatementInfo) -> Self {
        Self {
            start_line: info.start_line,
            start_col: info.start_col,
            end_line: info.end_line,
            end_col: info.end_col,
        }
    }
}

/// Return the span of the statement that **starts** on `line` (1-based).
///
/// Graph nodes record their defining statement's `start_line` (see
/// `graph::builder`), so this is the join key between a stored node and a
/// freshly re-parsed file. `None` when no statement starts on that line
/// (e.g. the source changed since the last `codeweb analyze`).
pub fn span_at_line(stmts: &[StatementInfo], line: usize) -> Option<LineSpan> {
    stmts
        .iter()
        .find(|info| info.start_line == line)
        .map(LineSpan::from_info)
}

/// Slice the exact source text covered by `span` out of the decoded source.
///
/// Line/col units follow ogsql-parser: 1-based lines, 1-based columns counted
/// in bytes on the decoded string; the end position is exclusive. Returns
/// `None` when the span points outside the source (stale store vs. changed
/// file) — callers must not invent boundaries in that case.
pub fn slice_span(source: &str, span: LineSpan) -> Option<String> {
    let mut lines = source.split('\n').collect::<Vec<&str>>();
    if source.ends_with('\n') {
        lines.pop();
    }
    if span.start_line == 0 || span.start_line > lines.len() || span.end_line > lines.len() {
        return None;
    }
    let start_idx = span.start_line - 1;
    let end_idx = span.end_line - 1;

    let byte_col = |line: &str, col: usize| -> Option<usize> {
        let off = col.checked_sub(1)?;
        if off <= line.len() && line.is_char_boundary(off) {
            Some(off)
        } else {
            None
        }
    };

    if start_idx == end_idx {
        let line = lines[start_idx];
        let from = byte_col(line, span.start_col)?;
        let to = byte_col(line, span.end_col)?;
        if from > to {
            return None;
        }
        return Some(line[from..to].to_string());
    }

    let mut out = String::new();
    for (i, idx) in (start_idx..=end_idx).enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let line = lines[idx];
        if idx == start_idx {
            let from = byte_col(line, span.start_col)?;
            out.push_str(&line[from..]);
        } else if idx == end_idx {
            let to = byte_col(line, span.end_col)?;
            out.push_str(&line[..to]);
        } else {
            out.push_str(line.strip_suffix('\r').unwrap_or(line));
        }
    }
    Some(out)
}

/// Merge spans that overlap or coincide (nested package members, duplicate
/// closure entries) so each source byte is emitted at most once. Disjoint
/// spans are kept separate and returned in ascending source order.
pub fn merge_spans(mut spans: Vec<LineSpan>) -> Vec<LineSpan> {
    if spans.len() <= 1 {
        return spans;
    }
    spans.sort();
    let mut merged: Vec<LineSpan> = Vec::with_capacity(spans.len());
    for span in spans {
        match merged.last_mut() {
            // Overlap / containment: `span` starts at or before `last` ends.
            // LineSpans compare (start_line, start_col, end_line, end_col), so
            // `span <= *last` on the end side means containment or touching.
            Some(last) if (span.start_line, span.start_col) <= (last.end_line, last.end_col) => {
                if (span.end_line, span.end_col) > (last.end_line, last.end_col) {
                    last.end_line = span.end_line;
                    last.end_col = span.end_col;
                }
            }
            _ => merged.push(span),
        }
    }
    merged
}

/// Slice several merged spans out of one source, joined with a newline in
/// source order.
pub fn slice_spans(source: &str, spans: &[LineSpan]) -> Option<String> {
    let mut parts = Vec::with_capacity(spans.len());
    for span in spans {
        parts.push(slice_span(source, *span)?);
    }
    Some(parts.join("\n"))
}

/// Dependency closure of one or more root nodes.
///
/// Each root is traversed independently with [`crate::graph::query::traversal`]
/// machinery (same edge semantics as `trace`/`impact`: every edge kind is
/// followed, the visited-set makes cycles terminate), then the per-root
/// results are unioned. Roots themselves are always members (issue #193:
/// "包含所有起点自身定义"); shared dependencies collapse via the set. Returned
/// in ascending [`petgraph::graph::NodeIndex`] order for deterministic output.
pub fn collect_closure(
    graph: &crate::graph::CodeGraph,
    roots: &[petgraph::graph::NodeIndex],
    direction: petgraph::Direction,
    max_depth: Option<usize>,
) -> Vec<petgraph::graph::NodeIndex> {
    use crate::graph::query::traversal::GraphTraversal;
    use std::collections::BTreeSet;

    let mut closure = BTreeSet::new();
    for &root in roots {
        closure.insert(root);
        let mut traversal = GraphTraversal::new(graph, root).direction(direction);
        if let Some(depth) = max_depth {
            traversal = traversal.max_depth(depth);
        }
        for idx in traversal.collect_nodes() {
            closure.insert(idx);
        }
    }
    closure.into_iter().collect()
}

/// How a closure member maps (or fails to map) to an extractable SQL
/// definition. Issue #193 requirement 4: every non-extractable member is
/// reported, never silently dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClosureMemberKind {
    /// Carries a `SourceLocation` into an SQL file — slice the definition.
    Extractable,
    /// Routine declared in a package spec whose body implementation could not
    /// be parsed (`partial: true`, `location.line == 0`).
    PartialRoutine,
    /// No source definition exists in the analyzed SQL sources (inferred
    /// table/view/sequence, built-in function call site).
    NoSourceDefinition,
    /// Definition lives outside SQL sources (XML mapper, Java, JSP, custom).
    NonSqlSource,
    /// Unresolved dynamic reference — no definition anywhere.
    Unresolved,
}

/// Classify one graph node for extraction purposes.
///
/// Note: `BuiltinFunction` nodes DO carry a call-site `SourceLocation` (first
/// call site wins at build time), so a location alone is not proof of a
/// definable statement — the node kind decides.
pub fn classify_member(node: &crate::graph::Node) -> ClosureMemberKind {
    use crate::graph::Node;
    match node {
        Node::Procedure {
            location, partial, ..
        }
        | Node::Function {
            location, partial, ..
        } => {
            if *partial || location.line == 0 {
                ClosureMemberKind::PartialRoutine
            } else {
                ClosureMemberKind::Extractable
            }
        }
        Node::Package { .. }
        | Node::Trigger { .. }
        | Node::Type { .. }
        | Node::Index { .. }
        | Node::MaterializedView { .. }
        | Node::Synonym { .. }
        | Node::Event { .. } => ClosureMemberKind::Extractable,
        Node::Table { location, .. }
        | Node::View { location, .. }
        | Node::Sequence { location, .. } => match location {
            Some(_) => ClosureMemberKind::Extractable,
            None => ClosureMemberKind::NoSourceDefinition,
        },
        Node::BuiltinFunction { .. } => ClosureMemberKind::NoSourceDefinition,
        Node::Unresolved { .. } => ClosureMemberKind::Unresolved,
        Node::MappedStatement { .. }
        | Node::JavaSql { .. }
        | Node::JavaMethod { .. }
        | Node::JavaClass { .. }
        | Node::JspPage { .. }
        | Node::JspSql { .. }
        | Node::Custom { .. } => ClosureMemberKind::NonSqlSource,
    }
}

/// One freshly parsed statement that starts on a given line: its span plus the
/// lowercased name of the object it defines (when the statement kind has one).
#[derive(Debug, Clone)]
pub struct SpanCandidate {
    pub span: LineSpan,
    pub object_name: Option<String>,
}

/// Index a freshly parsed file's statements by `start_line` — the join key
/// stored in graph `SourceLocation.line` values.
pub fn build_span_index(
    stmts: &[StatementInfo],
) -> std::collections::BTreeMap<usize, Vec<SpanCandidate>> {
    let mut index = std::collections::BTreeMap::new();
    for info in stmts {
        index
            .entry(info.start_line)
            .or_insert_with(Vec::new)
            .push(SpanCandidate {
                span: LineSpan::from_info(info),
                object_name: statement_object_name(&info.statement),
            });
    }
    index
}

/// Outcome of matching a stored node location against a fresh span index.
#[derive(Debug, Clone, PartialEq)]
pub enum SpanMatch {
    /// One statement starts on the line (or the object name picks one out of
    /// several).
    Matched(LineSpan),
    /// Several statements start on the line and no object name matched — the
    /// first is returned as a deterministic guess; callers warn.
    GuessedFirst(LineSpan),
    /// No statement starts on this line: the source changed since the last
    /// `codeweb analyze` (stale store). Callers must NOT invent boundaries.
    NotFound,
}

/// Find the span of the statement defining a node whose `location.line` is
/// `line` and whose object name (if any) is `node_name`.
pub fn match_span(
    index: &std::collections::BTreeMap<usize, Vec<SpanCandidate>>,
    line: usize,
    node_name: Option<&str>,
) -> SpanMatch {
    let Some(candidates) = index.get(&line) else {
        return SpanMatch::NotFound;
    };
    if candidates.len() == 1 {
        return SpanMatch::Matched(candidates[0].span);
    }
    if let Some(name) = node_name {
        let by_name: Vec<LineSpan> = candidates
            .iter()
            .filter(|c| c.object_name.as_deref() == Some(name))
            .map(|c| c.span)
            .collect();
        if by_name.len() == 1 {
            return SpanMatch::Matched(by_name[0]);
        }
    }
    SpanMatch::GuessedFirst(candidates[0].span)
}

/// The lowercased object name a node declares (used to disambiguate several
/// statements sharing one start line). `None` when the node kind has no
/// single object name (e.g. unnamed indexes).
pub fn node_object_name(node: &crate::graph::Node) -> Option<String> {
    use crate::graph::Node;
    let lower = |s: &str| s.to_lowercase();
    match node {
        Node::Procedure { id, .. } | Node::Function { id, .. } => Some(lower(&id.name)),
        Node::Package { name, .. }
        | Node::Table { name, .. }
        | Node::View { name, .. }
        | Node::Type { name, .. }
        | Node::Sequence { name, .. }
        | Node::MaterializedView { name, .. }
        | Node::Synonym { name, .. }
        | Node::Event { name, .. } => Some(lower(name)),
        Node::Trigger { name, .. } => Some(lower(name)),
        Node::Index { name, .. } => name.as_deref().map(lower),
        _ => None,
    }
}

/// The lowercased last identifier of the object a statement defines, or `None`
/// for statements without a single defining name.
fn statement_object_name(stmt: &ogsql_parser::ast::Statement) -> Option<String> {
    use ogsql_parser::ast::Statement;
    let last_ident =
        |name: &ogsql_parser::ast::ObjectName| name.last().map(|i| i.to_string().to_lowercase());
    match stmt {
        Statement::CreateTable(t) => last_ident(&t.name),
        Statement::CreateView(v) => last_ident(&v.name),
        Statement::CreateMaterializedView(v) => last_ident(&v.name),
        Statement::CreateSequence(s) => last_ident(&s.name),
        Statement::CreateType(t) => last_ident(&t.name),
        Statement::CreateFunction(f) => last_ident(&f.name),
        Statement::CreateProcedure(p) => last_ident(&p.name),
        Statement::CreatePackage(p) => last_ident(&p.name),
        Statement::CreatePackageBody(p) => last_ident(&p.name),
        Statement::CreateSynonym(s) => last_ident(&s.name),
        Statement::CreateTrigger(t) => Some(t.name.to_lowercase()),
        Statement::CreateEvent(e) => Some(e.name.to_lowercase()),
        Statement::CreateIndex(i) => i.name.as_ref().and_then(|n| last_ident(n)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    fn write_temp_sql(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::with_suffix(".sql").unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    /// Core slicing primitive: the byte range reported by ogsql-parser for a
    /// standalone `CREATE PROCEDURE` must reproduce exactly that procedure's
    /// definition text — not the leading comment, not the following
    /// procedure. The span is inclusive of the statement's `/` terminator
    /// (observed ogsql-parser behavior, locked here) so exported definitions
    /// remain deployable SQL.
    #[test]
    fn should_slice_exact_statement_text_for_standalone_procedure() {
        let source = r#"-- leading comment
CREATE OR REPLACE PROCEDURE a1 AS
BEGIN
    NULL;
END;
/

CREATE OR REPLACE PROCEDURE a2 AS
BEGIN
    NULL;
END;
/
"#;
        let f = write_temp_sql(source);
        let (stmts, text) =
            crate::parser::parse_sql_file_with_source(f.path()).expect("fixture should parse");

        let span = span_at_line(&stmts, 2).expect("procedure a1 starts on line 2");
        let sliced = slice_span(&text, span).expect("span within source bounds");

        assert_eq!(
            sliced, "CREATE OR REPLACE PROCEDURE a1 AS\nBEGIN\n    NULL;\nEND;\n/",
            "sliced text must be exactly the a1 definition (terminator included); \
             span={span:?} sliced={sliced:?}"
        );
    }

    /// Issue #193 requirement 2: boundaries must respect dollar-quoted strings,
    /// comments and nested BEGIN/END — a `;` or `CREATE` keyword inside a
    /// string/comment must never split a definition.
    #[test]
    fn should_slice_through_dollar_quotes_comments_and_nested_blocks() {
        let source = r#"-- header comment
CREATE FUNCTION f1() RETURNS INTEGER AS $$
BEGIN
    -- fake terminator inside dollar quotes: CREATE PROCEDURE decoy();
    RETURN 1;  /* inline comment with END; keyword */
END;
$$ LANGUAGE plpgsql;

CREATE PROCEDURE p1 AS
BEGIN
    IF 1 = 1 THEN
        BEGIN
            NULL; -- nested block: END inside must not close p1
        END;
    END IF;
    v_x := 'string with END; and / inside';
END;
/
"#;
        let f = write_temp_sql(source);
        let (stmts, text) =
            crate::parser::parse_sql_file_with_source(f.path()).expect("fixture should parse");

        // f1 starts on line 2 (after the header comment).
        let f1_span = span_at_line(&stmts, 2).expect("f1 starts on line 2");
        let f1_text = slice_span(&text, f1_span).expect("f1 span in bounds");
        assert!(
            !f1_text.contains("p1"),
            "f1 slice must not bleed into the following procedure: {f1_text:?}"
        );
        assert!(
            f1_text.contains("CREATE PROCEDURE decoy();"),
            "dollar-quoted body must be captured verbatim: {f1_text:?}"
        );
        assert!(
            f1_text.starts_with("CREATE FUNCTION f1()"),
            "slice must start at the definition, not the comment: {f1_text:?}"
        );

        let p1_span = span_at_line(&stmts, 9).expect("p1 starts on line 9");
        let p1_text = slice_span(&text, p1_span).expect("p1 span in bounds");
        assert!(
            p1_text.contains("string with END; and / inside"),
            "string literal content must be captured verbatim: {p1_text:?}"
        );
        assert!(
            p1_text.contains("END;") && p1_text.ends_with("/"),
            "nested BEGIN/END must stay inside the p1 slice: {p1_text:?}"
        );
        assert!(
            !p1_text.contains("f1"),
            "p1 slice must not bleed back into f1: {p1_text:?}"
        );
    }

    /// Identical spans (e.g. a package node and its member routine pointing at
    /// the same defining statement) collapse to one range.
    #[test]
    fn should_merge_identical_spans_into_one() {
        let s = LineSpan {
            start_line: 2,
            start_col: 1,
            end_line: 9,
            end_col: 4,
        };
        let merged = merge_spans(vec![s, s]);
        assert_eq!(merged, vec![s]);
    }

    /// A span fully contained in another (spec-decl vs whole-statement) must
    /// not produce a duplicate or a second range.
    #[test]
    fn should_merge_contained_span_into_its_outer_span() {
        let outer = LineSpan {
            start_line: 1,
            start_col: 1,
            end_line: 20,
            end_col: 10,
        };
        let inner = LineSpan {
            start_line: 5,
            start_col: 3,
            end_line: 12,
            end_col: 7,
        };
        let merged = merge_spans(vec![outer, inner]);
        assert_eq!(merged, vec![outer]);
    }

    /// Overlapping spans merge into their union so shared lines are not
    /// emitted twice.
    #[test]
    fn should_merge_overlapping_spans_into_union() {
        let a = LineSpan {
            start_line: 2,
            start_col: 1,
            end_line: 10,
            end_col: 6,
        };
        let b = LineSpan {
            start_line: 8,
            start_col: 2,
            end_line: 15,
            end_col: 4,
        };
        let merged = merge_spans(vec![a, b]);
        assert_eq!(
            merged,
            vec![LineSpan {
                start_line: 2,
                start_col: 1,
                end_line: 15,
                end_col: 4
            }]
        );
    }

    /// Disjoint definitions stay separate ranges in source order — the gap
    /// between them (unrelated code) is not filled in.
    #[test]
    fn should_keep_disjoint_spans_separate_in_source_order() {
        let first = LineSpan {
            start_line: 10,
            start_col: 1,
            end_line: 12,
            end_col: 3,
        };
        let second = LineSpan {
            start_line: 20,
            start_col: 1,
            end_line: 22,
            end_col: 9,
        };
        let merged = merge_spans(vec![second, first]);
        assert_eq!(merged, vec![first, second]);
    }

    /// Slicing a merged list reproduces each definition in order without
    /// duplication.
    #[test]
    fn should_slice_merged_spans_without_duplication() {
        let source = "A1\nx\n/\n\nA2\ny\n/\n";
        let spans = vec![
            LineSpan {
                start_line: 1,
                start_col: 1,
                end_line: 3,
                end_col: 2,
            },
            LineSpan {
                start_line: 5,
                start_col: 1,
                end_line: 7,
                end_col: 2,
            },
        ];
        let merged = merge_spans(spans);
        let out = slice_spans(source, &merged).expect("spans in bounds");
        // Inter-range gap content (blank lines, unrelated separators) is
        // trimmed; definitions are emitted back-to-back in source order.
        assert_eq!(out, "A1\nx\n/\nA2\ny\n/");
    }

    // ── Closure fixtures ────────────────────────────────────────

    const CALL_CHAIN_SQL: &str = r#"
CREATE TABLE shared_tbl(id INT);

CREATE FUNCTION b1() RETURNS INTEGER AS $$
BEGIN
    INSERT INTO shared_tbl(id) VALUES (1);
    RETURN 1;
END;
$$;

CREATE PROCEDURE root_a AS $$
BEGIN
    PERFORM b1();
END;
$$;

CREATE PROCEDURE root_b AS $$
BEGIN
    PERFORM b1();
END;
$$;

CREATE PROCEDURE unrelated_p AS $$
BEGIN
    NULL;
END;
$$;

CREATE PROCEDURE cyclic_a AS $$
BEGIN
    PERFORM cyclic_b();
END;
$$;

CREATE PROCEDURE cyclic_b AS $$
BEGIN
    PERFORM cyclic_a();
END;
$$;
"#;

    fn graph_with_nodes(
        file_name: &str,
    ) -> (
        crate::graph::CodeGraph,
        std::collections::HashMap<String, petgraph::graph::NodeIndex>,
    ) {
        let stmts = {
            let tokens = ogsql_parser::Tokenizer::new(CALL_CHAIN_SQL)
                .tokenize()
                .unwrap();
            let mut parser = ogsql_parser::Parser::with_source(tokens, CALL_CHAIN_SQL.to_string());
            parser.parse_with_text()
        };
        let parsed = vec![crate::parser::ParsedFile {
            path: PathBuf::from(file_name),
            statements: stmts,
            content_hash: String::new(),
        }];
        let graph = crate::graph::builder::GraphBuilder::new().build(&parsed);
        let mut nodes = std::collections::HashMap::new();
        for idx in graph.node_indices() {
            if let crate::graph::Node::Procedure { id, .. } = &graph[idx] {
                nodes.insert(id.name.clone(), idx);
            }
        }
        (graph, nodes)
    }

    fn closure_keys(
        graph: &crate::graph::CodeGraph,
        roots: &[petgraph::graph::NodeIndex],
        direction: petgraph::Direction,
        max_depth: Option<usize>,
    ) -> Vec<String> {
        collect_closure(graph, roots, direction, max_depth)
            .into_iter()
            .map(|idx| crate::graph::key::NodeKey::from_node(&graph[idx]).to_string())
            .collect()
    }

    /// Outgoing closure includes the root itself plus transitive dependencies
    /// (root_a → b1 → shared_tbl), and nothing else.
    #[test]
    fn should_include_root_and_transitive_deps_in_outgoing_closure() {
        let (graph, nodes) = graph_with_nodes("test.sql");
        let keys = closure_keys(
            &graph,
            &[nodes["root_a"]],
            petgraph::Direction::Outgoing,
            None,
        );
        assert!(
            keys.iter().any(|k| k.contains("root_a")),
            "root itself: {keys:?}"
        );
        assert!(
            keys.iter().any(|k| k.contains("b1")),
            "direct dep: {keys:?}"
        );
        assert!(
            keys.iter().any(|k| k.contains("shared_tbl")),
            "transitive dep: {keys:?}"
        );
        assert!(
            !keys
                .iter()
                .any(|k| k.contains("root_b") || k.contains("unrelated")),
            "non-dependencies must stay out: {keys:?}"
        );
    }

    /// Multiple roots compute the UNION of their closures; a dependency shared
    /// by both roots appears exactly once.
    #[test]
    fn should_union_multiple_roots_and_dedup_shared_deps() {
        let (graph, nodes) = graph_with_nodes("test.sql");
        let keys = closure_keys(
            &graph,
            &[nodes["root_a"], nodes["root_b"]],
            petgraph::Direction::Outgoing,
            None,
        );
        let b1_count = keys.iter().filter(|k| k.contains("b1")).count();
        assert_eq!(b1_count, 1, "shared dep must appear once: {keys:?}");
        assert!(keys.iter().any(|k| k.contains("root_a")));
        assert!(keys.iter().any(|k| k.contains("root_b")));
        assert!(!keys.iter().any(|k| k.contains("unrelated")), "{keys:?}");
    }

    /// A dependency cycle must terminate (no infinite recursion, no
    /// duplicated members) — issue #193 requirement 1.
    #[test]
    fn should_terminate_on_dependency_cycle() {
        let (graph, nodes) = graph_with_nodes("test.sql");
        let keys = closure_keys(
            &graph,
            &[nodes["cyclic_a"]],
            petgraph::Direction::Outgoing,
            None,
        );
        let cyclic_count = keys.iter().filter(|k| k.contains("cyclic")).count();
        assert_eq!(cyclic_count, 2, "cycle members exactly once each: {keys:?}");
    }

    /// `max_depth` bounds the recursion: 0 = roots only, 1 = direct deps.
    #[test]
    fn should_limit_closure_by_depth() {
        let (graph, nodes) = graph_with_nodes("test.sql");
        let depth0 = closure_keys(
            &graph,
            &[nodes["root_a"]],
            petgraph::Direction::Outgoing,
            Some(0),
        );
        assert_eq!(depth0.len(), 1, "depth 0 = root only: {depth0:?}");

        let depth1 = closure_keys(
            &graph,
            &[nodes["root_a"]],
            petgraph::Direction::Outgoing,
            Some(1),
        );
        assert!(
            depth1.iter().any(|k| k.contains("b1")),
            "depth 1 includes direct dep: {depth1:?}"
        );
        assert!(
            !depth1.iter().any(|k| k.contains("shared_tbl")),
            "depth 1 excludes transitive dep: {depth1:?}"
        );
    }

    /// Issue #193 requirement 4: closure members without an extractable SQL
    /// definition must be classified, never silently dropped.
    /// - routines with a real location → Extractable
    /// - partial routines (spec-declared, line 0) → PartialRoutine
    /// - inferred tables / builtins → NoSourceDefinition
    /// - XML/Java/JSP graph nodes → NonSqlSource
    /// - unresolved dynamic calls → Unresolved
    #[test]
    fn should_classify_closure_members_by_extractability() {
        let loc = || crate::graph::SourceLocation {
            file: std::sync::Arc::new(PathBuf::from("src/a.sql")),
            line: 3,
        };
        let cases: Vec<(crate::graph::Node, ClosureMemberKind)> = vec![
            (
                crate::graph::Node::Procedure {
                    id: crate::graph::RoutineId {
                        schema: None,
                        package: None,
                        name: "a1".into(),
                        kind: crate::graph::RoutineKind::Procedure,
                    },
                    location: loc(),
                    partial: false,
                    body_sql: Vec::new(),
                },
                ClosureMemberKind::Extractable,
            ),
            (
                crate::graph::Node::Function {
                    id: crate::graph::RoutineId {
                        schema: None,
                        package: None,
                        name: "b1".into(),
                        kind: crate::graph::RoutineKind::Function,
                    },
                    location: loc(),
                    partial: false,
                    body_sql: Vec::new(),
                },
                ClosureMemberKind::Extractable,
            ),
            (
                crate::graph::Node::Procedure {
                    id: crate::graph::RoutineId {
                        schema: None,
                        package: Some("pkg".into()),
                        name: "declared_only".into(),
                        kind: crate::graph::RoutineKind::Procedure,
                    },
                    location: crate::graph::SourceLocation {
                        file: std::sync::Arc::new(PathBuf::from("src/a.sql")),
                        line: 0,
                    },
                    partial: true,
                    body_sql: Vec::new(),
                },
                ClosureMemberKind::PartialRoutine,
            ),
            (
                crate::graph::Node::Table {
                    schema: None,
                    name: "inferred_tbl".into(),
                    explicit: false,
                    system: false,
                    location: None,
                    columns: Box::new(Vec::new()),
                    partition_by: None,
                    distribute_by: None,
                    tablespace: None,
                    temporary: false,
                    unlogged: false,
                    ddl_source: None,
                },
                ClosureMemberKind::NoSourceDefinition,
            ),
            (
                crate::graph::Node::Table {
                    schema: None,
                    name: "ddl_tbl".into(),
                    explicit: true,
                    system: false,
                    location: Some(loc()),
                    columns: Box::new(Vec::new()),
                    partition_by: None,
                    distribute_by: None,
                    tablespace: None,
                    temporary: false,
                    unlogged: false,
                    ddl_source: None,
                },
                ClosureMemberKind::Extractable,
            ),
            (
                crate::graph::Node::BuiltinFunction {
                    name: "count".into(),
                    category: "aggregate".into(),
                    domain: "core".into(),
                    location: loc(),
                },
                ClosureMemberKind::NoSourceDefinition,
            ),
            (
                crate::graph::Node::Unresolved {
                    raw_expr: Box::new("dynamic_call()".into()),
                    context: Box::new("proc:a1".into()),
                },
                ClosureMemberKind::Unresolved,
            ),
        ];
        for (node, expected) in cases {
            assert_eq!(classify_member(&node), expected, "mis-classified {node:?}");
        }

        // Non-SQL graph node kinds (XML mapper / Java / JSP) can never be
        // sliced from SQL sources.
        let mapper = crate::graph::Node::MappedStatement {
            namespace: "ns".into(),
            statement_id: "stmt".into(),
            kind: "select".into(),
            xml_file: PathBuf::from("m.xml"),
            line: 1,
            sql: None,
        };
        assert_eq!(classify_member(&mapper), ClosureMemberKind::NonSqlSource);
        let java = crate::graph::Node::JavaMethod {
            fqn: "F.m".into(),
            class_fqn: "F".into(),
            name: "m".into(),
            signature: String::new(),
            file: PathBuf::from("F.java"),
            line: 1,
        };
        assert_eq!(classify_member(&java), ClosureMemberKind::NonSqlSource);
    }

    /// A stored node's `location.line` must find its defining statement's
    /// span in a freshly re-parsed file.
    #[test]
    fn should_match_statement_span_by_location_line() {
        let source = "CREATE TABLE t_a(id INT);\n\nCREATE VIEW v_a AS SELECT 1;\n";
        let f = write_temp_sql(source);
        let (stmts, _text) = crate::parser::parse_sql_file_with_source(f.path()).expect("parse");
        let index = build_span_index(&stmts);

        let m = match_span(&index, 3, Some("v_a"));
        assert!(matches!(m, SpanMatch::Matched(_)), "view on line 3: {m:?}");

        // A line no statement starts on (e.g. after the file was edited since
        // the last `codeweb analyze`) must NOT invent a boundary.
        let stale = match_span(&index, 2, Some("whatever"));
        assert!(
            matches!(stale, SpanMatch::NotFound),
            "blank line: {stale:?}"
        );
    }

    /// Two statements sharing one start line are told apart by the node's
    /// object name (single-line `CREATE TABLE a(...); CREATE TABLE b(...);`).
    #[test]
    fn should_disambiguate_same_line_statements_by_object_name() {
        let source = "CREATE TABLE t_x(id INT); CREATE TABLE t_y(id INT);\n";
        let f = write_temp_sql(source);
        let (stmts, text) = crate::parser::parse_sql_file_with_source(f.path()).expect("parse");
        let index = build_span_index(&stmts);

        let m = match_span(&index, 1, Some("t_y"));
        let SpanMatch::Matched(span) = m else {
            panic!("expected name-matched span, got {m:?}");
        };
        let sliced = slice_span(&text, span).expect("in bounds");
        assert!(
            sliced.contains("t_y") && !sliced.contains("t_x"),
            "must slice the t_y statement only: {sliced:?}"
        );

        // Without a usable name hint the first same-line statement is used —
        // deterministically — and the caller is told it is a guess.
        assert!(matches!(
            match_span(&index, 1, None),
            SpanMatch::GuessedFirst(_)
        ));
    }

    /// `node_object_name` feeds the same-line disambiguation: routine ids,
    /// tables/views/sequences, triggers, packages.
    #[test]
    fn should_expose_node_object_name_for_matching() {
        let mk_id = |name: &str| crate::graph::RoutineId {
            schema: Some("s".into()),
            package: None,
            name: name.into(),
            kind: crate::graph::RoutineKind::Procedure,
        };
        let proc = crate::graph::Node::Procedure {
            id: mk_id("A1"),
            location: crate::graph::SourceLocation {
                file: std::sync::Arc::new(PathBuf::from("x.sql")),
                line: 1,
            },
            partial: false,
            body_sql: Vec::new(),
        };
        assert_eq!(node_object_name(&proc).as_deref(), Some("a1"));

        let table = crate::graph::Node::Table {
            schema: None,
            name: "TBL".into(),
            explicit: true,
            system: false,
            location: None,
            columns: Box::new(Vec::new()),
            partition_by: None,
            distribute_by: None,
            tablespace: None,
            temporary: false,
            unlogged: false,
            ddl_source: None,
        };
        assert_eq!(node_object_name(&table).as_deref(), Some("tbl"));

        let trigger = crate::graph::Node::Trigger {
            name: "TRG".into(),
            table: vec!["t".into()],
            location: crate::graph::SourceLocation {
                file: std::sync::Arc::new(PathBuf::from("x.sql")),
                line: 1,
            },
        };
        assert_eq!(node_object_name(&trigger).as_deref(), Some("trg"));
    }
}

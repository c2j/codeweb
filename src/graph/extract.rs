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
}

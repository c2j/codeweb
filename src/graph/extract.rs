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
use std::path::{Path, PathBuf};

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
        Statement::CreateIndex(i) => i.name.as_ref().and_then(last_ident),
        _ => None,
    }
}

/// `--nodes-file` content: one NodeKey or resolvable object name per line.
/// Blank lines and `#` comment lines are skipped; entries are trimmed.
pub fn parse_nodes_file(content: &str) -> Vec<String> {
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// Where a location's file lands inside the output directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputPathMapping {
    /// Path relative to the output root (directory structure preserved).
    pub relative: PathBuf,
    /// True when the location escapes the project root and was flattened to
    /// the output root's file name; callers add a report warning.
    pub escaped: bool,
}

/// Map a node location's file path to its output-relative path.
///
/// Relative locations keep their directory structure (`./` components are
/// dropped). Absolute locations are re-based onto the project root; anything
/// that would land outside the project root (foreign absolute paths, `..`
/// components) is flattened to the output root's file name and flagged.
pub fn output_relative_path(location_file: &Path, project_root: &Path) -> OutputPathMapping {
    let flatten = |p: &Path| OutputPathMapping {
        relative: p
            .file_name()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("unknown.sql")),
        escaped: true,
    };

    if location_file.is_absolute() {
        return match location_file.strip_prefix(project_root) {
            Ok(rel) => OutputPathMapping {
                relative: rel.to_path_buf(),
                escaped: false,
            },
            Err(_) => flatten(location_file),
        };
    }

    let mut normalized = PathBuf::new();
    for component in location_file.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(part) => normalized.push(part),
            _ => return flatten(location_file),
        }
    }
    if normalized.as_os_str().is_empty() {
        return flatten(location_file);
    }
    OutputPathMapping {
        relative: normalized,
        escaped: false,
    }
}

// ── Extraction engine ───────────────────────────────────────────

/// `extract` JSON report schema version (see `columns` for the convention).
pub const EXTRACT_REPORT_SCHEMA_VERSION: u32 = 1;

/// Status values used in the report's `closure_nodes` array.
pub const STATUS_EXTRACTED: &str = "extracted";
pub const STATUS_PARTIAL_ROUTINE: &str = "partial_routine";
pub const STATUS_NO_SOURCE_DEFINITION: &str = "no_source_definition";
pub const STATUS_NON_SQL_SOURCE: &str = "non_sql_source";
pub const STATUS_UNRESOLVED: &str = "unresolved";
pub const STATUS_MISSING_STATEMENT: &str = "missing_statement";

/// Machine-readable result of an extraction run (`--report <path>`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExtractReport {
    pub schema_version: u32,
    /// Display keys of the resolved roots, in the order they were given.
    pub roots: Vec<String>,
    /// "outgoing" or "incoming".
    pub direction: String,
    /// Every closure member with its extraction status.
    pub closure_nodes: Vec<ClosureNodeReport>,
    /// One entry per source file that contributed definitions.
    pub files: Vec<FileExtractionReport>,
    /// Members without an extractable definition (inferred objects, builtins,
    /// unresolved dynamic calls) — issue #193 requirement 4: never silent.
    pub unresolved: Vec<UnresolvedReport>,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ClosureNodeReport {
    pub key: String,
    pub type_tag: String,
    pub status: &'static str,
    /// Source file as stored in the graph, when the node has a location.
    pub source: Option<String>,
    /// Path of the trimmed definition relative to the output directory.
    pub output: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FileExtractionReport {
    pub source: String,
    pub output: String,
    pub ranges: Vec<RangeReport>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RangeReport {
    pub node: String,
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct UnresolvedReport {
    pub key: String,
    pub reason: String,
}

/// What an extraction run produced.
#[derive(Debug)]
pub struct ExtractOutcome {
    pub report: ExtractReport,
    pub files_written: usize,
}

/// Fatal extraction errors — the CLI maps these to exit code 1.
#[derive(Debug)]
pub enum ExtractError {
    /// Target directory exists and `--overwrite` was not given.
    OutputDirExists(PathBuf),
    /// An output file or directory could not be written.
    OutputWrite { path: PathBuf, source: std::io::Error },
}

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtractError::OutputDirExists(path) => write!(
                f,
                "output directory already exists: {} (use --overwrite to replace it)",
                path.display()
            ),
            ExtractError::OutputWrite { path, source } => {
                write!(f, "cannot write {}: {}", path.display(), source)
            }
        }
    }
}

/// One closure member awaiting slicing, grouped by source file.
struct FileMember {
    idx: petgraph::graph::NodeIndex,
    key: String,
    line: usize,
    name: Option<String>,
}

/// Compute the dependency closure of `roots` and write the closure members'
/// original SQL definitions, trimmed per file, into `output_dir`.
///
/// Never mutates the source tree — only reads it. When `output_dir` exists it
/// is refused unless `overwrite` (then cleared and recreated). Members whose
/// stored location no longer matches a statement in the re-parsed file are
/// reported via warnings (stale store), never given invented boundaries.
#[allow(clippy::too_many_arguments)]
pub fn run_extract(
    graph: &crate::graph::CodeGraph,
    project_root: &Path,
    roots: &[petgraph::graph::NodeIndex],
    direction: petgraph::Direction,
    direction_label: &str,
    max_depth: Option<usize>,
    output_dir: &Path,
    overwrite: bool,
) -> Result<ExtractOutcome, ExtractError> {
    // Output directory policy (issue #193 requirement 3): refuse instead of
    // silently merging into an existing tree.
    if output_dir.exists() {
        if !overwrite {
            return Err(ExtractError::OutputDirExists(output_dir.to_path_buf()));
        }
        std::fs::remove_dir_all(output_dir).map_err(|source| ExtractError::OutputWrite {
            path: output_dir.to_path_buf(),
            source,
        })?;
    }
    std::fs::create_dir_all(output_dir).map_err(|source| ExtractError::OutputWrite {
        path: output_dir.to_path_buf(),
        source,
    })?;

    let mut warnings: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut unresolved: Vec<UnresolvedReport> = Vec::new();
    let mut closure_nodes: Vec<ClosureNodeReport> = Vec::new();

    let key_of = |idx: petgraph::graph::NodeIndex| {
        crate::graph::key::NodeKey::from_node(&graph[idx]).to_string()
    };

    let mut report_roots: Vec<String> = Vec::new();
    for &root in roots {
        let key = key_of(root);
        if !report_roots.contains(&key) {
            report_roots.push(key);
        }
    }

    let closure = collect_closure(graph, roots, direction, max_depth);

    // Group extractable members by source file; classify the rest into the
    // report immediately (issue #193 requirement 4).
    let mut by_file: std::collections::BTreeMap<PathBuf, Vec<FileMember>> =
        std::collections::BTreeMap::new();
    for idx in closure {
        let node = &graph[idx];
        let key = key_of(idx);
        let type_tag = crate::graph::node_type_tag(node).to_string();
        let push_unresolved = |unresolved: &mut Vec<UnresolvedReport>,
                               key: String,
                               reason: String,
                               status: &'static str,
                               source: Option<String>| {
            unresolved.push(UnresolvedReport { key: key.clone(), reason });
            closure_nodes.push(ClosureNodeReport {
                key,
                type_tag,
                status,
                source,
                output: None,
            });
        };
        match classify_member(node) {
            ClosureMemberKind::Extractable => {
                let (file, line) = node_location(node).expect("extractable has location");
                by_file.entry((*file).clone()).or_default().push(FileMember {
                    idx,
                    key,
                    line,
                    name: node_object_name(node),
                });
            }
            ClosureMemberKind::PartialRoutine => {
                let (file, _) = node_location(node).expect("partial has location");
                push_unresolved(
                    &mut unresolved,
                    key,
                    "partial routine: package spec declaration whose body implementation \
                     could not be parsed — no standalone definition to extract"
                        .to_string(),
                    STATUS_PARTIAL_ROUTINE,
                    Some(file.to_string_lossy().into_owned()),
                );
            }
            ClosureMemberKind::NoSourceDefinition => {
                push_unresolved(
                    &mut unresolved,
                    key,
                    "no source definition in analyzed SQL sources (inferred from \
                     references, or built-in)"
                        .to_string(),
                    STATUS_NO_SOURCE_DEFINITION,
                    None,
                );
            }
            ClosureMemberKind::NonSqlSource => {
                push_unresolved(
                    &mut unresolved,
                    key,
                    "definition lives outside SQL sources (XML mapper / Java / JSP node); \
                     SQL trimming does not apply"
                        .to_string(),
                    STATUS_NON_SQL_SOURCE,
                    None,
                );
            }
            ClosureMemberKind::Unresolved => {
                push_unresolved(
                    &mut unresolved,
                    key,
                    "unresolved dynamic reference — no definition anywhere".to_string(),
                    STATUS_UNRESOLVED,
                    None,
                );
            }
        }
    }

    // Slice each file's members out of a fresh parse of that file.
    let mut files_report: Vec<FileExtractionReport> = Vec::new();
    let mut files_written: usize = 0;
    for (stored_path, members) in by_file {
        let source_path = if stored_path.is_absolute() {
            stored_path.clone()
        } else {
            project_root.join(&stored_path)
        };
        let file_display = stored_path.to_string_lossy().into_owned();

        let Ok((stmts, source_text)) =
            crate::parser::parse_sql_file_with_source(&source_path)
        else {
            errors.push(format!(
                "cannot re-parse source file '{}' — its definitions were not extracted",
                source_path.display()
            ));
            for member in &members {
                closure_nodes.push(ClosureNodeReport {
                    key: member.key.clone(),
                    type_tag: crate::graph::node_type_tag(&graph[member.idx]).to_string(),
                    status: STATUS_MISSING_STATEMENT,
                    source: Some(file_display.clone()),
                    output: None,
                });
            }
            continue;
        };
        let index = build_span_index(&stmts);

        // Match every member against the fresh index. `matched` keeps
        // (member slot, span); everything else becomes a warning.
        let mut matched: Vec<(usize, LineSpan)> = Vec::new();
        let mut extracted_slots: std::collections::HashSet<usize> =
            std::collections::HashSet::new();
        for (slot, member) in members.iter().enumerate() {
            match match_span(&index, member.line, member.name.as_deref()) {
                SpanMatch::Matched(span) => {
                    matched.push((slot, span));
                    extracted_slots.insert(slot);
                }
                SpanMatch::GuessedFirst(span) => {
                    warnings.push(format!(
                        "'{}': {} statements start at line {} in '{}' — used the first \
                         (provide the full NodeKey if this is wrong)",
                        member.key,
                        index.get(&member.line).map_or(0, Vec::len),
                        member.line,
                        file_display
                    ));
                    matched.push((slot, span));
                    extracted_slots.insert(slot);
                }
                SpanMatch::NotFound => {
                    warnings.push(format!(
                        "'{}': no statement starts at line {} in '{}' — the file changed \
                         since the last `codeweb analyze`; no boundary invented",
                        member.key, member.line, file_display
                    ));
                }
            }
        }

        // Nothing matched (all stale) — do not write an empty file.
        if matched.is_empty() {
            for member in &members {
                closure_nodes.push(ClosureNodeReport {
                    key: member.key.clone(),
                    type_tag: crate::graph::node_type_tag(&graph[member.idx]).to_string(),
                    status: STATUS_MISSING_STATEMENT,
                    source: Some(file_display.clone()),
                    output: None,
                });
            }
            continue;
        }

        let spans: Vec<LineSpan> = matched.iter().map(|(_, s)| *s).collect();
        let merged = merge_spans(spans);
        let Some(trimmed) = slice_spans(&source_text, &merged) else {
            warnings.push(format!(
                "statement spans exceed the bounds of '{}' — the file changed since the \
                 last `codeweb analyze`; nothing written for it",
                file_display
            ));
            for member in members.iter() {
                closure_nodes.push(ClosureNodeReport {
                    key: member.key.clone(),
                    type_tag: crate::graph::node_type_tag(&graph[member.idx]).to_string(),
                    status: STATUS_MISSING_STATEMENT,
                    source: Some(file_display.clone()),
                    output: None,
                });
            }
            continue;
        };

        let mapping = output_relative_path(&stored_path, project_root);
        if mapping.escaped {
            warnings.push(format!(
                "source file '{}' resolves outside the project root — flattened to the \
                 output directory root",
                file_display
            ));
        }
        let out_path = output_dir.join(&mapping.relative);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ExtractError::OutputWrite {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        std::fs::write(&out_path, &trimmed).map_err(|source| ExtractError::OutputWrite {
            path: out_path.clone(),
            source,
        })?;
        files_written += 1;

        // Range report: one entry per member whose span was used, in member
        // (source) order.
        let mut ranges: Vec<RangeReport> = Vec::new();
        for (slot, span) in &matched {
            ranges.push(RangeReport {
                node: members[*slot].key.clone(),
                start_line: span.start_line,
                end_line: span.end_line,
            });
        }
        let out_display = mapping.relative.to_string_lossy().into_owned();
        files_report.push(FileExtractionReport {
            source: file_display.clone(),
            output: out_display.clone(),
            ranges,
        });

        for (slot, member) in members.iter().enumerate() {
            let extracted = extracted_slots.contains(&slot);
            closure_nodes.push(ClosureNodeReport {
                key: member.key.clone(),
                type_tag: crate::graph::node_type_tag(&graph[member.idx]).to_string(),
                status: if extracted {
                    STATUS_EXTRACTED
                } else {
                    STATUS_MISSING_STATEMENT
                },
                source: Some(file_display.clone()),
                output: extracted.then(|| out_display.clone()),
            });
        }
    }

    Ok(ExtractOutcome {
        report: ExtractReport {
            schema_version: EXTRACT_REPORT_SCHEMA_VERSION,
            roots: report_roots,
            direction: direction_label.to_string(),
            closure_nodes,
            files: files_report,
            unresolved,
            warnings,
            errors,
        },
        files_written,
    })
}

/// The stored `SourceLocation` of an extractable node (file, start line).
fn node_location(node: &crate::graph::Node) -> Option<(std::sync::Arc<PathBuf>, usize)> {
    use crate::graph::Node;
    match node {
        Node::Procedure { location, .. }
        | Node::Function { location, .. }
        | Node::Package { location, .. }
        | Node::Trigger { location, .. }
        | Node::Type { location, .. }
        | Node::Index { location, .. }
        | Node::MaterializedView { location, .. }
        | Node::Synonym { location, .. }
        | Node::Event { location, .. } => Some((location.file.clone(), location.line)),
        Node::Table { location, .. } | Node::View { location, .. } => {
            location.as_ref().map(|l| (l.file.clone(), l.line))
        }
        Node::Sequence { location, .. } => {
            location.as_ref().map(|l| (l.file.clone(), l.line))
        }
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

    /// `--nodes-file`: one NodeKey or object name per line; blank lines and
    /// `#` comment lines are skipped; entries are trimmed.
    #[test]
    fn should_parse_nodes_file_skipping_comments_and_blank_lines() {
        let content = "\
# 16 target routines for the migration cut

proc:sales.pkg_a.a1

  pkg_a.a2
# another comment
func:reporting.b1
";
        assert_eq!(
            parse_nodes_file(content),
            vec!["proc:sales.pkg_a.a1", "pkg_a.a2", "func:reporting.b1"]
        );
    }

    /// Output paths preserve the location's directory structure relative to
    /// the project root; `./` prefixes are normalized away.
    #[test]
    fn should_map_location_relative_path_under_project_root() {
        let root = PathBuf::from("/proj");
        let rel = output_relative_path(Path::new("./src/schema/x.sql"), &root);
        assert_eq!(rel.relative, PathBuf::from("src/schema/x.sql"));
        assert!(!rel.escaped);
    }

    /// Locations outside the project root (absolute analyze inputs from other
    /// trees, `..` escapes) flatten to the output root and are flagged so the
    /// caller warns instead of silently surprising the user.
    #[test]
    fn should_flatten_paths_escaping_project_root() {
        let root = PathBuf::from("/proj");
        let outside = output_relative_path(Path::new("/elsewhere/src/x.sql"), &root);
        assert!(outside.escaped, "absolute path outside root must be flagged");
        assert_eq!(outside.relative, PathBuf::from("x.sql"));

        let parent = output_relative_path(Path::new("../shared/x.sql"), &root);
        assert!(parent.escaped, "`..` escape must be flagged");
        assert_eq!(parent.relative, PathBuf::from("x.sql"));
    }

    // ── Engine: end-to-end extraction ───────────────────────────

    /// Full run: root_a's outgoing closure (root_a, b1, shared_tbl) is sliced
    /// out of its source file — same-file non-members (root_b, unrelated_p,
    /// cyclic_*) stay out, members appear once each in source order.
    #[test]
    fn should_extract_closure_definitions_per_file_from_engine() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("app.sql"), CALL_CHAIN_SQL).unwrap();

        // Build the graph the same way an analyze would (relative path).
        let stmts = {
            let tokens =
                ogsql_parser::Tokenizer::new(CALL_CHAIN_SQL).tokenize().unwrap();
            let mut parser =
                ogsql_parser::Parser::with_source(tokens, CALL_CHAIN_SQL.to_string());
            parser.parse_with_text()
        };
        let parsed = vec![crate::parser::ParsedFile {
            path: PathBuf::from("src/app.sql"),
            statements: stmts,
            content_hash: String::new(),
        }];
        let graph = crate::graph::builder::GraphBuilder::new().build(&parsed);
        let root_a = graph
            .node_indices()
            .find(|&idx| matches!(&graph[idx], crate::graph::Node::Procedure { id, .. } if id.name == "root_a"))
            .expect("root_a in graph");

        let out_dir = dir.path().join("out");
        let outcome = run_extract(
            &graph,
            &root,
            &[root_a],
            petgraph::Direction::Outgoing,
            "outgoing",
            None,
            &out_dir,
            false,
        )
        .expect("extraction should succeed");

        let written = std::fs::read_to_string(out_dir.join("src/app.sql"))
            .expect("output file exists");
        assert!(written.contains("CREATE PROCEDURE root_a"), "{written:?}");
        assert!(written.contains("CREATE FUNCTION b1"), "{written:?}");
        assert!(written.contains("CREATE TABLE shared_tbl"), "{written:?}");
        assert!(
            !written.contains("root_b"),
            "non-member must be trimmed: {written:?}"
        );
        assert!(
            !written.contains("unrelated_p"),
            "non-member must be trimmed: {written:?}"
        );
        assert!(
            !written.contains("cyclic_a"),
            "non-member must be trimmed: {written:?}"
        );

        // Report: one source file, three extracted ranges in source order.
        assert_eq!(outcome.report.schema_version, 1);
        assert_eq!(outcome.report.files.len(), 1);
        let file_report = &outcome.report.files[0];
        assert_eq!(file_report.ranges.len(), 3, "{:?}: {outcome:?}", file_report);
        let starts: Vec<usize> =
            file_report.ranges.iter().map(|r| r.start_line).collect();
        let mut sorted = starts.clone();
        sorted.sort();
        assert_eq!(starts, sorted, "ranges must be in source order");
        assert_eq!(outcome.files_written, 1);
    }

    /// Issue #193 acceptance example: a chain across three files
    /// (`a1` → `b1` → `tbl_c1`) produces three output files, each containing
    /// only the closure member — the issue's PKG_A/FNC_B scenario.
    #[test]
    fn should_distribute_closure_across_source_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let src = root.join("sql");
        std::fs::create_dir_all(&src).unwrap();

        let pkg_a = r#"-- PKG_A.sql: a1 needs a2/a3 trimmed away
CREATE OR REPLACE PROCEDURE a1 AS
BEGIN
    v := b1(1);
END;
/

CREATE OR REPLACE PROCEDURE a2 AS
BEGIN
    NULL;
END;
/

CREATE OR REPLACE PROCEDURE a3 AS
BEGIN
    NULL;
END;
/
"#;
        let fnc_b = r#"-- FNC_B.sql
CREATE FUNCTION b1(p INT) RETURNS INTEGER AS $$
BEGIN
    INSERT INTO tbl_c1 VALUES (p);
    RETURN p;
END;
$$;

CREATE FUNCTION b2() RETURNS INTEGER AS $$
BEGIN
    RETURN 2;
END;
$$;
"#;
        let tbl_c = r#"-- TBL_C1.sql
CREATE TABLE tbl_c1(id INT);

CREATE TABLE tbl_other(id INT);
"#;
        std::fs::write(src.join("PKG_A.sql"), pkg_a).unwrap();
        std::fs::write(src.join("FNC_B.sql"), fnc_b).unwrap();
        std::fs::write(src.join("TBL_C1.sql"), tbl_c).unwrap();

        let mk = |text: &str, name: &str| {
            let tokens = ogsql_parser::Tokenizer::new(text).tokenize().unwrap();
            let mut parser = ogsql_parser::Parser::with_source(tokens, text.to_string());
            crate::parser::ParsedFile {
                path: src.join(name),
                statements: parser.parse_with_text(),
                content_hash: String::new(),
            }
        };
        let parsed = vec![
            mk(pkg_a, "PKG_A.sql"),
            mk(fnc_b, "FNC_B.sql"),
            mk(tbl_c, "TBL_C1.sql"),
        ];
        let graph = crate::graph::builder::GraphBuilder::new().build(&parsed);
        let a1 = graph
            .node_indices()
            .find(|&idx| matches!(&graph[idx], crate::graph::Node::Procedure { id, .. } if id.name == "a1"))
            .expect("a1 in graph");

        let out_dir = dir.path().join("out");
        let outcome = run_extract(
            &graph,
            &root,
            &[a1],
            petgraph::Direction::Outgoing,
            "outgoing",
            None,
            &out_dir,
            false,
        )
        .expect("extraction should succeed");

        let pkg_out = std::fs::read_to_string(out_dir.join("sql/PKG_A.sql")).unwrap();
        assert!(pkg_out.contains("PROCEDURE a1"), "{pkg_out:?}");
        assert!(
            !pkg_out.contains("a2") && !pkg_out.contains("a3"),
            "same-file non-members trimmed: {pkg_out:?}"
        );

        let fnc_out = std::fs::read_to_string(out_dir.join("sql/FNC_B.sql")).unwrap();
        assert!(fnc_out.contains("FUNCTION b1"), "{fnc_out:?}");
        assert!(!fnc_out.contains("b2"), "{fnc_out:?}");

        let tbl_out = std::fs::read_to_string(out_dir.join("sql/TBL_C1.sql")).unwrap();
        assert!(tbl_out.contains("CREATE TABLE tbl_c1"), "{tbl_out:?}");
        assert!(
            !tbl_out.contains("tbl_other"),
            "unrelated DDL trimmed: {tbl_out:?}"
        );

        assert_eq!(outcome.files_written, 3);
        assert_eq!(outcome.report.files.len(), 3);
    }

    /// When the source file changed since `codeweb analyze` (stored
    /// `location.line` no longer starts a statement), no boundary is invented:
    /// a warning is recorded, the member reports `missing_statement`, and the
    /// output is not written.
    #[test]
    fn should_not_invent_boundaries_for_stale_source_lines() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();

        let sql = "CREATE PROCEDURE solo AS\nBEGIN\n    NULL;\nEND;\n/\n";
        std::fs::write(src.join("s.sql"), sql).unwrap();

        let tokens = ogsql_parser::Tokenizer::new(sql).tokenize().unwrap();
        let mut parser = ogsql_parser::Parser::with_source(tokens, sql.to_string());
        let parsed = vec![crate::parser::ParsedFile {
            path: PathBuf::from("src/s.sql"),
            statements: parser.parse_with_text(),
            content_hash: String::new(),
        }];
        let graph = crate::graph::builder::GraphBuilder::new().build(&parsed);
        let solo = graph
            .node_indices()
            .find(|&idx| matches!(&graph[idx], crate::graph::Node::Procedure { .. }))
            .expect("solo in graph");

        // The file changes after the graph was built: one comment line
        // inserted at the top shifts every stored line number.
        std::fs::write(src.join("s.sql"), format!("-- moved\n{sql}")).unwrap();

        let out_dir = dir.path().join("out");
        let outcome = run_extract(
            &graph,
            &root,
            &[solo],
            petgraph::Direction::Outgoing,
            "outgoing",
            None,
            &out_dir,
            false,
        )
        .expect("run itself should succeed");

        assert!(
            !out_dir.join("src/s.sql").exists(),
            "no output must be written from stale lines"
        );
        assert!(
            outcome
                .report
                .warnings
                .iter()
                .any(|w| w.contains("no statement starts at line")),
            "stale line must be warned about: {:?}",
            outcome.report.warnings
        );
        assert!(
            outcome
                .report
                .closure_nodes
                .iter()
                .any(|n| n.status == STATUS_MISSING_STATEMENT),
            "member must report missing_statement: {:?}",
            outcome.report.closure_nodes
        );
    }

    /// Unresolved calls, inferred tables and builtins are listed in the
    /// report (issue #193 requirement 4) without aborting extraction of the
    /// extractable members.
    #[test]
    fn should_report_unresolved_and_inferred_members_without_aborting() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();

        let sql = r#"
CREATE PROCEDURE caller AS $$
BEGIN
    PERFORM mystery_proc();
    INSERT INTO ghost_tbl SELECT COUNT(*) FROM ghost_tbl;
END;
$$;

CREATE TABLE ghost_tbl(id INT);
"#;
        // ghost_tbl has NO DDL in this project — remove its statement to make
        // it inferred-only, as if the DDL lives in an unanalyzed file.
        let without_ddl = sql
            .replace("CREATE TABLE ghost_tbl(id INT);\n", "")
            .trim_start()
            .to_string();
        std::fs::write(src.join("c.sql"), &without_ddl).unwrap();

        let tokens = ogsql_parser::Tokenizer::new(&without_ddl).tokenize().unwrap();
        let mut parser = ogsql_parser::Parser::with_source(tokens, without_ddl.clone());
        let parsed = vec![crate::parser::ParsedFile {
            path: PathBuf::from("src/c.sql"),
            statements: parser.parse_with_text(),
            content_hash: String::new(),
        }];
        let graph = crate::graph::builder::GraphBuilder::new().build(&parsed);
        let caller = graph
            .node_indices()
            .find(|&idx| matches!(&graph[idx], crate::graph::Node::Procedure { id, .. } if id.name == "caller"))
            .expect("caller in graph");

        let out_dir = dir.path().join("out");
        let outcome = run_extract(
            &graph,
            &root,
            &[caller],
            petgraph::Direction::Outgoing,
            "outgoing",
            None,
            &out_dir,
            false,
        )
        .expect("extraction should succeed");

        let unresolved_keys = outcome
            .report
            .unresolved
            .iter()
            .map(|u| u.key.clone())
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(
            unresolved_keys.contains("mystery_proc"),
            "unresolved call must be listed: {unresolved_keys}"
        );
        assert!(
            unresolved_keys.contains("ghost_tbl"),
            "inferred table must be listed: {unresolved_keys}"
        );
        assert!(
            unresolved_keys.contains("count"),
            "builtin must be listed: {unresolved_keys}"
        );
        // The extractable root is still exported.
        let written = std::fs::read_to_string(out_dir.join("src/c.sql")).unwrap();
        assert!(written.contains("CREATE PROCEDURE caller"), "{written:?}");
    }

    /// An existing output directory is refused (exit-worthy error) unless
    /// `overwrite` is set — issue #193 requirement 3: no silent merging.
    #[test]
    fn should_refuse_existing_output_dir_without_overwrite() {
        let (graph, root, solo, _dir) = solo_project_fixture();
        let out_dir = root.join("out");
        std::fs::create_dir_all(&out_dir).unwrap();
        let marker = out_dir.join("precious.txt");
        std::fs::write(&marker, "keep me").unwrap();

        let err = run_extract(
            &graph,
            &root,
            &[solo],
            petgraph::Direction::Outgoing,
            "outgoing",
            None,
            &out_dir,
            false,
        )
        .expect_err("must refuse an existing output dir");

        assert!(
            matches!(err, ExtractError::OutputDirExists(_)),
            "expected OutputDirExists, got {err}"
        );
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap(),
            "keep me",
            "existing dir must not be touched"
        );
    }

    /// `overwrite` clears the output directory and re-extracts; stale files
    /// from a previous run do not survive.
    #[test]
    fn should_clear_and_recreate_output_dir_on_overwrite() {
        let (graph, root, solo, dir) = solo_project_fixture();
        let out_dir = root.join("out");
        std::fs::create_dir_all(out_dir.join("stale")).unwrap();
        std::fs::write(out_dir.join("stale/old.sql"), "old").unwrap();

        run_extract(
            &graph,
            &root,
            &[solo],
            petgraph::Direction::Outgoing,
            "outgoing",
            None,
            &out_dir,
            true,
        )
        .expect("overwrite run should succeed");

        assert!(
            !out_dir.join("stale").exists(),
            "stale content must be cleared"
        );
        let written = std::fs::read_to_string(out_dir.join("src/s.sql")).unwrap();
        assert!(written.contains("CREATE PROCEDURE solo"), "{written:?}");
    }

    /// Shared fixture: a one-procedure project whose graph is already built.
    fn solo_project_fixture() -> (
        crate::graph::CodeGraph,
        PathBuf,
        petgraph::graph::NodeIndex,
        tempfile::TempDir,
    ) {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let sql = "CREATE PROCEDURE solo AS\nBEGIN\n    NULL;\nEND;\n/\n";
        std::fs::write(src.join("s.sql"), sql).unwrap();

        let tokens = ogsql_parser::Tokenizer::new(sql).tokenize().unwrap();
        let mut parser = ogsql_parser::Parser::with_source(tokens, sql.to_string());
        let parsed = vec![crate::parser::ParsedFile {
            path: PathBuf::from("src/s.sql"),
            statements: parser.parse_with_text(),
            content_hash: String::new(),
        }];
        let graph = crate::graph::builder::GraphBuilder::new().build(&parsed);
        let solo = graph
            .node_indices()
            .find(|&idx| matches!(&graph[idx], crate::graph::Node::Procedure { .. }))
            .expect("solo in graph");
        (graph, root, solo, dir)
    }
}

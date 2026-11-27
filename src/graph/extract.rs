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
    /// Exactly one statement on the line defines the node's object (or the
    /// node kind carries no object name and a single candidate exists).
    Matched(LineSpan),
    /// No statement starts on this line: the source changed since the last
    /// `codeweb analyze` (stale store). Callers must NOT invent boundaries.
    NotFound,
    /// Statements start on this line but none defines the node's object name
    /// — exporting the first candidate would silently emit the WRONG
    /// definition, which is worse than reporting the member missing.
    NameMismatch,
}

/// Find the span of the statement defining a node whose `location.line` is
/// `line` and whose object name (if any) is `node_name`.
///
/// Strict matching (review on #194): when the node carries an object name,
/// the candidate statement must define that same name — a single candidate
/// with a different name is a `NameMismatch`, never a guess. When the node
/// kind has no object name (`node_name == None`), only a single candidate on
/// the line may match; several candidates are `NotFound`.
pub fn match_span(
    index: &std::collections::BTreeMap<usize, Vec<SpanCandidate>>,
    line: usize,
    node_name: Option<&str>,
) -> SpanMatch {
    let Some(candidates) = index.get(&line) else {
        return SpanMatch::NotFound;
    };
    match node_name {
        Some(name) => {
            let by_name: Vec<LineSpan> = candidates
                .iter()
                .filter(|c| c.object_name.as_deref() == Some(name))
                .map(|c| c.span)
                .collect();
            if by_name.len() == 1 {
                SpanMatch::Matched(by_name[0])
            } else if by_name.is_empty() && candidates.iter().all(|c| c.object_name.is_some()) {
                SpanMatch::NameMismatch
            } else {
                SpanMatch::NotFound
            }
        }
        None => {
            if candidates.len() == 1 {
                SpanMatch::Matched(candidates[0].span)
            } else {
                SpanMatch::NotFound
            }
        }
    }
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

/// Make `path` absolute without touching the filesystem.
fn absolutize(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

/// Lexically resolve `.` and `..` components without touching the
/// filesystem (symlinks are handled later by `canonicalize`).
fn normalize_lexical(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() && !path.has_root() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Canonicalize `path` even when its tail does not exist yet (e.g. an output
/// directory that has not been created): resolve the deepest existing
/// ancestor and re-append the missing components lexically. This resolves
/// `.`/`..`/symlinks for the existing part, which is what makes stored
/// `../<project>/...` analyze paths map back under the project root.
fn canonicalize_lenient(path: &Path) -> PathBuf {
    let normalized = normalize_lexical(path);
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    let mut cur = normalized.clone();
    loop {
        if let Ok(canon) = cur.canonicalize() {
            let mut out = canon;
            for comp in missing.iter().rev() {
                out.push(comp);
            }
            return out;
        }
        match (cur.file_name(), cur.parent()) {
            (Some(name), Some(parent)) => {
                missing.push(name.to_os_string());
                cur = parent.to_path_buf();
            }
            _ => return normalized,
        }
    }
}

/// Map a node location's file path to its output-relative path.
///
/// The stored path is joined onto the project root (when relative) and
/// canonicalized, so `../`-containing paths recorded by an analyze run from a
/// different working directory resolve back under the root and keep their
/// original directory structure. Returns `Err` when the source cannot be
/// mapped safely under the project root — callers must fail explicitly
/// rather than flatten (flattening silently overwrote same-named files from
/// different directories, review on #194).
pub fn output_relative_path(location_file: &Path, project_root: &Path) -> Result<PathBuf, String> {
    let abs = if location_file.is_absolute() {
        location_file.to_path_buf()
    } else {
        absolutize(project_root).join(location_file)
    };
    let canon = canonicalize_lenient(&abs);
    let root_canon = canonicalize_lenient(&absolutize(project_root));
    match canon.strip_prefix(&root_canon) {
        Ok(rel) if !rel.as_os_str().is_empty() => Ok(rel.to_path_buf()),
        _ => Err(format!(
            "source path '{}' (resolved to '{}') is not under project root '{}' — \
             refusing to flatten it into the output root",
            location_file.display(),
            canon.display(),
            root_canon.display()
        )),
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
    OutputWrite {
        path: PathBuf,
        source: std::io::Error,
    },
    /// An output/report target would delete or overwrite project sources —
    /// refused before any filesystem mutation (review on #194).
    ProtectedPath { path: PathBuf, reason: String },
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
            ExtractError::ProtectedPath { path, reason } => {
                write!(f, "refusing unsafe target {}: {}", path.display(), reason)
            }
        }
    }
}

/// Every file that participated in the analysis — all node kinds carrying a
/// source path (SQL `SourceLocation`s, mapper XML, Java, JSP). Source
/// protection must cover the WHOLE graph, not just the extraction closure:
/// `--output <dir> --overwrite` / `--report <file>` pointing at an analyzed
/// file outside the closure used to pass validation and destroy it (review
/// on #194, round 2).
pub fn analyzed_source_paths(graph: &crate::graph::CodeGraph) -> Vec<PathBuf> {
    use crate::graph::Node;
    let mut out: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();
    for idx in graph.node_indices() {
        let node = &graph[idx];
        match node {
            Node::Procedure { location, .. }
            | Node::Function { location, .. }
            | Node::Package { location, .. }
            | Node::Trigger { location, .. }
            | Node::Type { location, .. }
            | Node::Index { location, .. }
            | Node::MaterializedView { location, .. }
            | Node::Synonym { location, .. }
            | Node::Event { location, .. }
            | Node::BuiltinFunction { location, .. } => {
                out.insert(location.file.as_ref().clone());
            }
            Node::Table { location, .. }
            | Node::View { location, .. }
            | Node::Sequence { location, .. }
            | Node::Custom { location, .. } => {
                if let Some(loc) = location {
                    out.insert(loc.file.as_ref().clone());
                }
            }
            Node::MappedStatement { xml_file, .. } => {
                out.insert(xml_file.clone());
            }
            Node::JavaSql { java_file, .. } => {
                out.insert(java_file.clone());
            }
            Node::JavaMethod { file, .. }
            | Node::JavaClass { file, .. }
            | Node::JspSql { file, .. } => {
                out.insert(file.clone());
            }
            Node::JspPage { path, .. } => {
                out.insert(path.clone());
            }
            Node::Unresolved { .. } => {}
        }
    }
    out.into_iter().collect()
}

/// Run-level pre-flight shared by the CLI and [`run_extract`]: refuse
/// output/report targets that would delete or overwrite ANY analyzed
/// source — the whole graph, not just the extraction closure. Called before
/// any write path, including the early report written when no root
/// resolves (review on #194, round 2).
pub fn validate_extract_targets_for_graph(
    graph: &crate::graph::CodeGraph,
    project_root: &Path,
    output_dir: &Path,
    report_path: Option<&Path>,
) -> Result<(), ExtractError> {
    let source_paths: Vec<PathBuf> = analyzed_source_paths(graph)
        .into_iter()
        .map(|p| {
            if p.is_absolute() {
                p
            } else {
                project_root.join(p)
            }
        })
        .collect();
    validate_extract_targets(project_root, output_dir, report_path, &source_paths)
}

/// Refuse any output/report target that would delete or overwrite project
/// sources (review on #194: `--output <srcdir> --overwrite` used to wipe the
/// analyzed SQL files and still exit 0).
///
/// The output directory must not be, contain, or sit on the ancestor chain of
/// the project root or of any analyzed source file; the report path must not
/// equal or contain a source file. All comparisons run on canonicalized
/// absolute paths so `..`/symlink tricks cannot slip through.
fn validate_extract_targets(
    project_root: &Path,
    output_dir: &Path,
    report_path: Option<&Path>,
    source_paths: &[PathBuf],
) -> Result<(), ExtractError> {
    let protected = |path: &Path, reason: String| ExtractError::ProtectedPath {
        path: path.to_path_buf(),
        reason,
    };
    let root_c = canonicalize_lenient(&absolutize(project_root));
    let out_c = canonicalize_lenient(&absolutize(output_dir));
    if out_c == root_c || root_c.starts_with(&out_c) {
        return Err(protected(
            output_dir,
            "output directory is or contains the project root".to_string(),
        ));
    }
    let src_canons: Vec<PathBuf> = source_paths
        .iter()
        .map(|p| canonicalize_lenient(&absolutize(p)))
        .collect();
    for (src, canon) in source_paths.iter().zip(src_canons.iter()) {
        if canon == &out_c || canon.starts_with(&out_c) {
            return Err(protected(
                output_dir,
                format!(
                    "output directory would contain or replace source file '{}'",
                    src.display()
                ),
            ));
        }
    }
    if let Some(report) = report_path {
        let rep_c = canonicalize_lenient(&absolutize(report));
        for (src, canon) in source_paths.iter().zip(src_canons.iter()) {
            if canon == &rep_c || canon.starts_with(&rep_c) {
                return Err(protected(
                    report,
                    format!(
                        "report path would overwrite or contain source file '{}'",
                        src.display()
                    ),
                ));
            }
        }
    }
    Ok(())
}

// ── Package member-level trimming (review on #194, fix 4) ──────
//
// Object-level trimming is the core goal of #193: when the closure hits
// members of package P, the output is a REASSEMBLED
// `CREATE [OR REPLACE] PACKAGE [BODY] P AS|IS … END P;` containing only the
// selected member bodies plus the package-level declarations they
// (transitively) reference — never the whole package verbatim.
//
// Member boundaries come from ogsql-parser's AST (`PackageProcedure` /
// `PackageFunction` carry `start_line`/`end_line`), mapped onto the token
// stream for byte-exact slices. Package-level declarations (variables,
// types, cursors, forward declarations) are split at top-level semicolons
// and kept only when their declared names are referenced — by word-boundary,
// case-insensitive search — inside the kept member bodies and kept
// declarations (transitive closure). This is a documented lexical
// approximation: references hidden in dynamic SQL (`EXECUTE '…'`) cannot be
// seen.

use ogsql_parser::ast::{PackageItem, Statement};
use ogsql_parser::token::{Keyword, Token, TokenWithSpan};

/// Result of trimming one package spec/body statement.
struct PackageTrim {
    /// Reassembled deployable statement text.
    text: String,
    /// Selected member names actually found (and emitted) in the AST items.
    found_members: std::collections::BTreeSet<String>,
    /// Lowercased text of the kept member bodies — lets a sibling spec trim
    /// keep declarations the members reference (spec signatures alone would
    /// miss them).
    member_hay: String,
    /// The statement's start line (for segment ordering / range reports).
    start_line: usize,
    end_line: usize,
}

/// Convert a 1-based line / 1-based byte-column position to a byte offset in
/// `source` (same units as `slice_span`).
fn line_col_to_offset(source: &str, line: usize, col: usize) -> Option<usize> {
    let mut offset = 0usize;
    for (idx, raw) in source.split_inclusive('\n').enumerate() {
        if idx + 1 == line {
            let content = raw.strip_suffix('\n').unwrap_or(raw);
            let content = content.strip_suffix('\r').unwrap_or(content);
            let off = col.checked_sub(1)?;
            if off <= content.len() && content.is_char_boundary(off) {
                return Some(offset + off);
            }
            return None;
        }
        offset += raw.len();
    }
    // Last line without trailing newline is covered by split_inclusive too;
    // a line number past the end is invalid.
    None
}

/// Qualified, lowercased package name as the graph sees it
/// (`schema.pkg` or `pkg`).
fn ast_object_qualified(name: &ogsql_parser::ast::ObjectName) -> String {
    name.iter()
        .map(|i| i.to_string().to_lowercase())
        .collect::<Vec<_>>()
        .join(".")
}

/// Scan forward from a member's `PROCEDURE`/`FUNCTION` keyword token to the
/// token that ends the member. Body members end at the `;` after the closing
/// `END [name]`; spec signatures end at their first depth-0 `;` (`spec_mode`).
/// `BEGIN`/`CASE` push depth, compound `END IF|LOOP|CASE` never pop it.
fn scan_pkg_member_end(
    tokens: &[TokenWithSpan],
    start: usize,
    bound: usize,
    spec_mode: bool,
) -> Option<usize> {
    let mut i = start + 1;
    let (mut begin_d, mut case_d) = (0i32, 0i32);
    while i < bound {
        match &tokens[i].token {
            // Spec signature: `PROCEDURE p(x INT);` — first top-level `;`.
            // (In a body, `;` at depth 0 is a local declaration separator and
            // must NOT end the member.)
            Token::Semicolon if spec_mode && begin_d == 0 && case_d == 0 => return Some(i),
            Token::Keyword(Keyword::CASE) => case_d += 1,
            Token::Keyword(Keyword::BEGIN_P) => begin_d += 1,
            Token::Keyword(Keyword::END_P) => {
                match tokens.get(i + 1).map(|t| &t.token) {
                    // Compound ends close IF/LOOP blocks — no depth change.
                    Some(Token::Keyword(Keyword::IF_P)) | Some(Token::Keyword(Keyword::LOOP)) => {
                        i += 2;
                        continue;
                    }
                    // `END CASE` closes a CASE statement/expression.
                    Some(Token::Keyword(Keyword::CASE)) => {
                        if case_d > 0 {
                            case_d -= 1;
                        }
                        i += 2;
                        continue;
                    }
                    _ => {
                        if case_d > 0 {
                            case_d -= 1;
                        } else if begin_d > 1 {
                            // A nested BEGIN..END block inside the member.
                            begin_d -= 1;
                        } else {
                            // This END closes the member itself: consume the
                            // optional member name and the terminating `;`.
                            let mut j = i + 1;
                            while j < tokens.len()
                                && matches!(
                                    tokens[j].token,
                                    Token::Ident(_) | Token::QuotedIdent(_)
                                )
                            {
                                j += 1;
                            }
                            if j < tokens.len() && tokens[j].token == Token::Semicolon {
                                j += 1;
                            }
                            return Some(j - 1);
                        }
                    }
                }
            }
            Token::Eof => return None,
            _ => {}
        }
        i += 1;
    }
    None
}

/// How a package-level declaration chunk participates in reassembly.
#[derive(Debug)]
enum DeclClass {
    /// Declared names (lowercased); kept iff one is referenced.
    Named(Vec<String>),
    /// Unclassifiable declaration (pragma, quoted name, raw fallback) —
    /// always kept so the reassembled package stays deployable.
    Always,
}

/// Extract the declared name(s) of a package-level declaration chunk from
/// its leading tokens: `name [, name]* TYPE…`, `CURSOR name …`,
/// `TYPE name …`, or a forward `PROCEDURE|FUNCTION name …;` declaration.
fn classify_decl_chunk(tokens: &[TokenWithSpan]) -> DeclClass {
    let idxs: Vec<usize> = tokens
        .iter()
        .enumerate()
        .filter(|(_, t)| !matches!(t.token, Token::Comment(_)))
        .map(|(i, _)| i)
        .collect();
    let Some(&first) = idxs.first() else {
        return DeclClass::Always;
    };
    let ident_name = |i: usize| -> Option<String> {
        match &tokens[i].token {
            Token::Ident(s) => Some(s.to_lowercase()),
            Token::QuotedIdent(_) => None, // quoted — cannot word-match safely
            _ => None,
        }
    };
    match &tokens[first].token {
        Token::Keyword(Keyword::CURSOR) | Token::Keyword(Keyword::TYPE_P) => idxs
            .get(1)
            .and_then(|&i| ident_name(i))
            .map_or(DeclClass::Always, |n| DeclClass::Named(vec![n])),
        Token::Keyword(Keyword::PROCEDURE) | Token::Keyword(Keyword::FUNCTION) => {
            // Forward declaration: `PROCEDURE [schema.]name [(…)] …;`
            let mut names: Vec<String> = Vec::new();
            let mut expect_ident = true;
            for &i in idxs.iter().skip(1) {
                match &tokens[i].token {
                    Token::Ident(s) if expect_ident => {
                        names.push(s.to_lowercase());
                        expect_ident = false;
                    }
                    Token::Dot if !expect_ident => expect_ident = true,
                    Token::LParen | Token::Semicolon => break,
                    _ => break,
                }
            }
            if names.is_empty() {
                DeclClass::Always
            } else {
                DeclClass::Named(vec![names.join(".")])
            }
        }
        Token::Ident(s) => {
            if s.eq_ignore_ascii_case("pragma") {
                return DeclClass::Always;
            }
            // Leading `name [, name]*` before the type/expression.
            let mut names = Vec::new();
            let mut i = 0usize;
            let mut want_ident = true;
            while i < idxs.len() {
                let ti = idxs[i];
                match &tokens[ti].token {
                    Token::Ident(s) if want_ident => {
                        names.push(s.to_lowercase());
                        want_ident = false;
                    }
                    Token::Comma if !want_ident => want_ident = true,
                    _ => break,
                }
                i += 1;
            }
            if names.is_empty() {
                DeclClass::Always
            } else {
                DeclClass::Named(names)
            }
        }
        _ => DeclClass::Always,
    }
}

/// Word-boundary, case-insensitive reference check: does `hay_lower`
/// (already lowercased) contain `name_lower` as a standalone identifier?
/// Identifier characters are alphanumerics, `_` and `$`.
fn text_references_name(hay_lower: &str, name_lower: &str) -> bool {
    if name_lower.is_empty() {
        return false;
    }
    let is_ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    hay_lower.match_indices(name_lower).any(|(i, _)| {
        let before = hay_lower[..i].chars().next_back();
        let after = hay_lower[i + name_lower.len()..].chars().next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

/// Trim one `CREATE [OR REPLACE] PACKAGE [BODY]` statement to the selected
/// member set plus transitively referenced package-level declarations.
///
/// Returns `None` when the statement cannot be safely decomposed (token
/// mapping failure) — the caller falls back to emitting the whole statement
/// rather than producing broken SQL.
fn trim_package_statement(
    source: &str,
    info: &StatementInfo,
    selected: &std::collections::BTreeSet<String>,
    extra_refs: &str,
    warnings: &mut Vec<String>,
) -> Option<PackageTrim> {
    let (items, is_spec) = match &info.statement {
        Statement::CreatePackage(p) => (&p.items, true),
        Statement::CreatePackageBody(p) => (&p.items, false),
        _ => return None,
    };

    let span = LineSpan::from_info(info);
    let b0 = line_col_to_offset(source, span.start_line, span.start_col)?;
    let b1 = line_col_to_offset(source, span.end_line, span.end_col)?;
    if b0 >= b1 || b1 > source.len() {
        return None;
    }

    let tokens = ogsql_parser::Tokenizer::new(source).tokenize().ok()?;
    let stmt_toks: Vec<usize> = tokens
        .iter()
        .enumerate()
        .filter(|(_, t)| t.span.start >= b0 && t.span.end <= b1 && !matches!(t.token, Token::Eof))
        .map(|(i, _)| i)
        .collect();
    let as_pos = stmt_toks.iter().position(|&i| {
        matches!(
            tokens[i].token,
            Token::Keyword(Keyword::AS) | Token::Keyword(Keyword::IS)
        )
    })?;
    let end_pos = stmt_toks
        .iter()
        .rposition(|&i| tokens[i].token == Token::Keyword(Keyword::END_P))?;
    let as_tok = stmt_toks[as_pos];
    let end_tok = stmt_toks[end_pos];

    // Region token indices strictly between `AS|IS` and the final `END`.
    let region: Vec<usize> = stmt_toks
        .iter()
        .copied()
        .filter(|&i| i > as_tok && i < end_tok)
        .collect();

    // AST subprogram items → (name, token range). In a spec every
    // PROCEDURE/FUNCTION item is a signature; in a body only items with a
    // block are members (block-less ones are forward declarations → decls).
    struct AstMember {
        name: String,
        start_line: usize,
    }
    let mut ast_members: Vec<AstMember> = Vec::new();
    for item in items {
        let (name, start_line, has_body) = match item {
            PackageItem::Procedure(p) => (
                ast_object_qualified(&p.name),
                p.start_line,
                p.block.is_some(),
            ),
            PackageItem::Function(f) => (
                ast_object_qualified(&f.name),
                f.start_line,
                f.block.is_some(),
            ),
            _ => continue,
        };
        if is_spec || has_body {
            ast_members.push(AstMember { name, start_line });
        }
    }

    // Map each AST member to its token range, in source order.
    let mut member_ranges: Vec<(usize, usize, String)> = Vec::new();
    let mut rpos = 0usize;
    for m in &ast_members {
        let mut start_tok = None;
        while rpos < region.len() {
            let gi = region[rpos];
            rpos += 1;
            if tokens[gi].location.line == m.start_line
                && matches!(
                    tokens[gi].token,
                    Token::Keyword(Keyword::PROCEDURE) | Token::Keyword(Keyword::FUNCTION)
                )
            {
                start_tok = Some(gi);
                break;
            }
        }
        let Some(st) = start_tok else {
            warnings.push(format!(
                "package member '{}' could not be located in the token stream — \
                 emitting the whole package statement",
                m.name
            ));
            return None;
        };
        // Verify the token after the keyword really is this member's name —
        // guards against matching a nested subprogram's keyword instead.
        let name_after = tokens
            .iter()
            .skip(st + 1)
            .find(|t| !matches!(t.token, Token::Comment(_)))
            .and_then(|t| match &t.token {
                Token::Ident(s) => Some(s.to_lowercase()),
                _ => None,
            });
        let expected_first = m.name.split('.').next().unwrap_or(m.name.as_str());
        if name_after.as_deref() != Some(expected_first) {
            warnings.push(format!(
                "package member '{}' token mapping mismatch — emitting the whole \
                 package statement",
                m.name
            ));
            return None;
        }
        let Some(et) = scan_pkg_member_end(&tokens, st, end_tok, is_spec) else {
            warnings.push(format!(
                "package member '{}' has no detectable end — emitting the whole \
                 package statement",
                m.name
            ));
            return None;
        };
        member_ranges.push((st, et, m.name.clone()));
    }

    // Pieces in source order: kept members + decl chunks in the gaps.
    enum Piece {
        Member {
            s: usize,
            e: usize,
            name: String,
        },
        Decl {
            s: usize,
            e: usize,
            class: DeclClass,
        },
    }
    let mut pieces: Vec<Piece> = Vec::new();

    // Split a global token-index range [lo, hi) into decl chunks at
    // semicolons; a trailing chunk without `;` is kept as unclassifiable.
    fn decl_chunks(
        tokens: &[TokenWithSpan],
        lo: usize,
        hi: usize,
    ) -> Vec<(usize, usize, DeclClass)> {
        let mut out = Vec::new();
        let mut chunk_start: Option<usize> = None;
        for g in lo..hi {
            if chunk_start.is_none() {
                chunk_start = Some(g);
            }
            if tokens[g].token == Token::Semicolon {
                let s = chunk_start.take().unwrap();
                let toks: Vec<TokenWithSpan> = (s..=g).map(|i| tokens[i].clone()).collect();
                out.push((s, g, classify_decl_chunk(&toks)));
            }
        }
        if let Some(s) = chunk_start {
            // Trailing decl without semicolon (shouldn't happen in valid
            // packages) — keep it visible rather than dropping text.
            let toks: Vec<TokenWithSpan> = (s..hi).map(|i| tokens[i].clone()).collect();
            out.push((s, hi - 1, classify_decl_chunk(&toks)));
        }
        out
    }

    let mut cursor_tok = as_tok + 1;
    for &(ms, me, ref name) in &member_ranges {
        // Decl gap before this member: global indices (cursor_tok, ms).
        if ms > cursor_tok {
            for (s, e, class) in decl_chunks(&tokens, cursor_tok, ms) {
                pieces.push(Piece::Decl { s, e, class });
            }
        }
        pieces.push(Piece::Member {
            s: ms,
            e: me,
            name: name.clone(),
        });
        cursor_tok = me + 1;
    }
    if end_tok > cursor_tok {
        for (s, e, class) in decl_chunks(&tokens, cursor_tok, end_tok) {
            pieces.push(Piece::Decl { s, e, class });
        }
    }

    // Select members; compute the referenced-declaration fixpoint.
    let member_text = |s: usize, e: usize| -> String {
        source[tokens[s].span.start..tokens[e].span.end].to_string()
    };
    let mut hay = extra_refs.to_lowercase();
    hay.push('\n');
    let mut member_hay = String::new();
    let mut found_members = std::collections::BTreeSet::new();
    for piece in &pieces {
        if let Piece::Member { s, e, name } = piece {
            if selected.contains(name) {
                found_members.insert(name.clone());
                let text = member_text(*s, *e).to_lowercase();
                hay.push_str(&text);
                hay.push('\n');
                member_hay.push_str(&text);
                member_hay.push('\n');
            }
        }
    }
    // Selected members that the AST does not know → caller reports them.
    let ast_names: std::collections::BTreeSet<String> =
        ast_members.iter().map(|m| m.name.clone()).collect();
    for name in selected {
        if !ast_names.contains(name) {
            warnings.push(format!(
                "package member '{name}' not found in the re-parsed package — \
                 the file changed since `codeweb analyze`"
            ));
        }
    }

    // Decl fixpoint: keep decls whose names are referenced by kept text.
    let mut kept_decl: Vec<bool> = vec![false; pieces.len()];
    // KeepAlways decls are always kept and join the haystack.
    for (i, piece) in pieces.iter().enumerate() {
        if let Piece::Decl { s, e, class } = piece {
            match class {
                DeclClass::Always => {
                    kept_decl[i] = true;
                    hay.push_str(&member_text(*s, *e).to_lowercase());
                    hay.push('\n');
                }
                DeclClass::Named(_) => {}
            }
        }
    }
    loop {
        let mut changed = false;
        for (i, piece) in pieces.iter().enumerate() {
            if kept_decl[i] {
                continue;
            }
            if let Piece::Decl {
                s,
                e,
                class: DeclClass::Named(names),
            } = piece
            {
                if names.iter().any(|n| text_references_name(&hay, n)) {
                    kept_decl[i] = true;
                    hay.push_str(&member_text(*s, *e).to_lowercase());
                    hay.push('\n');
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    // Warn when a kept member references a sibling member that is NOT in the
    // closure — the reassembled package will not deploy standalone.
    let unselected: Vec<&String> = ast_names
        .iter()
        .filter(|n| !selected.contains(*n))
        .collect();
    if !unselected.is_empty() {
        let mut member_hay = String::new();
        for piece in &pieces {
            if let Piece::Member { s, e, name } = piece {
                if selected.contains(name) {
                    member_hay.push_str(&member_text(*s, *e).to_lowercase());
                    member_hay.push('\n');
                }
            }
        }
        for name in unselected {
            if text_references_name(&member_hay, name) {
                warnings.push(format!(
                    "package member '{name}' is referenced by an extracted member but \
                     is not in the closure — the reassembled package may not deploy \
                     standalone"
                ));
            }
        }
    }

    // Assemble: header + kept items in source order + footer.
    let mut out = String::new();
    out.push_str(&source[b0..tokens[as_tok].span.end]);
    out.push('\n');
    let mut emitted = 0usize;
    for (i, piece) in pieces.iter().enumerate() {
        let keep = match piece {
            Piece::Member { name, .. } => selected.contains(name),
            Piece::Decl { .. } => kept_decl[i],
        };
        if !keep {
            continue;
        }
        let (s, e) = match piece {
            Piece::Member { s, e, .. } | Piece::Decl { s, e, .. } => (*s, *e),
        };
        out.push_str(&member_text(s, e));
        out.push('\n');
        emitted += 1;
    }
    out.push_str(&source[tokens[end_tok].span.start..b1]);
    out.push('\n');
    if emitted == 0 && found_members.is_empty() {
        // Nothing selected survived — still emit a minimal deployable wrapper.
    }

    Some(PackageTrim {
        text: out,
        found_members,
        member_hay,
        start_line: span.start_line,
        end_line: span.end_line,
    })
}

/// One closure member awaiting slicing, grouped by source file.
struct FileMember {
    idx: petgraph::graph::NodeIndex,
    key: String,
    line: usize,
    name: Option<String>,
    route: MemberRoute,
}

/// How an extractable closure member maps onto source text.
#[derive(Debug, Clone)]
enum MemberRoute {
    /// Ordinary statement — slice its `StatementInfo` span.
    Regular,
    /// Package member routine — handled by package reassembly.
    PackageMember { pkg: String, member: String },
    /// `Node::Package` — emits the (trimmed) spec or body statement.
    PackageNode { pkg: String },
}

/// Qualified lowercased package key shared by member nodes and the package
/// node (`schema.pkg` or `pkg`).
fn routine_pkg_qualified(schema: &Option<String>, package: &str) -> String {
    match schema {
        Some(s) if !s.is_empty() => format!("{}.{}", s.to_lowercase(), package.to_lowercase()),
        _ => package.to_lowercase(),
    }
}

/// Route an extractable node: package members and package nodes take the
/// reassembly path; everything else slices its own statement.
fn member_route(node: &crate::graph::Node) -> MemberRoute {
    use crate::graph::Node;
    match node {
        Node::Procedure { id, .. } | Node::Function { id, .. } => match &id.package {
            Some(pkg) => MemberRoute::PackageMember {
                pkg: routine_pkg_qualified(&id.schema, pkg),
                member: id.name.to_lowercase(),
            },
            None => MemberRoute::Regular,
        },
        Node::Package { schema, name, .. } => MemberRoute::PackageNode {
            pkg: routine_pkg_qualified(schema, name),
        },
        _ => MemberRoute::Regular,
    }
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
    report_path: Option<&Path>,
) -> Result<ExtractOutcome, ExtractError> {
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
    // Package-qualified → selected member names (lowercased), and packages
    // whose spec/body node itself is in the closure. Drives member-level
    // package reassembly (review on #194, fix 4).
    let mut pkg_members: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
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
            unresolved.push(UnresolvedReport {
                key: key.clone(),
                reason,
            });
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
                let route = member_route(node);
                match &route {
                    MemberRoute::PackageMember { pkg, member } => {
                        pkg_members
                            .entry(pkg.clone())
                            .or_default()
                            .insert(member.clone());
                    }
                    MemberRoute::PackageNode { pkg } => {
                        pkg_members.entry(pkg.clone()).or_default();
                    }
                    MemberRoute::Regular => {}
                }
                by_file
                    .entry((*file).clone())
                    .or_default()
                    .push(FileMember {
                        idx,
                        key,
                        line,
                        name: node_object_name(node),
                        route,
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

    // ── Pre-flight: validate targets and output mappings BEFORE any
    // filesystem mutation. `--overwrite` must never be able to delete or
    // overwrite analyzed sources — ALL of them, not just closure members
    // (review on #194, round 2). ──
    validate_extract_targets_for_graph(graph, project_root, output_dir, report_path)?;

    // Map every source file to its output-relative path up front. Two
    // different stored paths resolving to the same output path is a hard
    // error — silently overwriting would lose a definition (review on #194).
    let mut out_rel: std::collections::HashMap<PathBuf, PathBuf> = std::collections::HashMap::new();
    for stored in by_file.keys() {
        let rel = output_relative_path(stored, project_root).map_err(|reason| {
            ExtractError::ProtectedPath {
                path: stored.clone(),
                reason,
            }
        })?;
        if out_rel.values().any(|r| r == &rel) {
            return Err(ExtractError::ProtectedPath {
                path: stored.clone(),
                reason: format!(
                    "output path '{}' is produced by more than one source file",
                    rel.display()
                ),
            });
        }
        out_rel.insert(stored.clone(), rel);
    }

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

    // pkg → kept member-body text across ALL files, so a spec trim (possibly
    // in a different file than the body) retains declarations the selected
    // members' bodies reference.
    let mut body_hay: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();

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

        let Ok((stmts, source_text)) = crate::parser::parse_sql_file_with_source(&source_path)
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

        // ── Package-routed members: reassemble trimmed PACKAGE [BODY]
        // statements instead of slicing whole statements (fix 4). ──
        // stmt start_line → trim result (dedupes members + package node
        // pointing at the same statement).
        let mut pkg_trims: std::collections::HashMap<usize, PackageTrim> =
            std::collections::HashMap::new();
        // member slot → extracted via package stmt (with its stmt span).
        let mut pkg_slot_span: std::collections::HashMap<usize, (usize, usize)> =
            std::collections::HashMap::new();
        // (stmt start_line, pkg) groups for member bodies, in source order.
        let mut body_groups: std::collections::BTreeMap<usize, (String, Vec<usize>)> =
            std::collections::BTreeMap::new();
        let mut node_items: Vec<(usize, usize, String)> = Vec::new(); // (slot, line, pkg)
        for (slot, member) in members.iter().enumerate() {
            match &member.route {
                MemberRoute::PackageMember { pkg, .. } => {
                    body_groups
                        .entry(member.line)
                        .or_insert_with(|| (pkg.clone(), Vec::new()))
                        .1
                        .push(slot);
                }
                MemberRoute::PackageNode { pkg } => {
                    node_items.push((slot, member.line, pkg.clone()));
                }
                MemberRoute::Regular => {}
            }
        }

        // Find + trim one package statement; `require_body` distinguishes
        // member bodies (must be CreatePackageBody) from package nodes.
        fn trim_pkg_stmt<'a>(
            line: usize,
            pkg: &str,
            require_body: bool,
            extra_refs: &str,
            stmts: &'a [StatementInfo],
            source_text: &str,
            pkg_members: &std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
            pkg_trims: &'a mut std::collections::HashMap<usize, PackageTrim>,
            warnings: &mut Vec<String>,
        ) -> Option<&'a PackageTrim> {
            if pkg_trims.contains_key(&line) {
                return pkg_trims.get(&line);
            }
            let info = stmts.iter().find(|s| s.start_line == line)?;
            let (items_name, is_body) = match &info.statement {
                Statement::CreatePackage(p) => (ast_object_qualified(&p.name), false),
                Statement::CreatePackageBody(p) => (ast_object_qualified(&p.name), true),
                _ => return None,
            };
            if items_name != pkg || (require_body && !is_body) {
                return None;
            }
            let selected = pkg_members.get(pkg).cloned().unwrap_or_default();
            let trim =
                match trim_package_statement(source_text, info, &selected, extra_refs, warnings) {
                    Some(t) => t,
                    None => {
                        // Structural fallback: emit the whole statement rather
                        // than broken SQL (trim_package_statement already warned).
                        let span = LineSpan::from_info(info);
                        let text = slice_span(source_text, span).unwrap_or_default();
                        PackageTrim {
                            text,
                            found_members: selected.clone(),
                            member_hay: String::new(),
                            start_line: span.start_line,
                            end_line: span.end_line,
                        }
                    }
                };
            pkg_trims.insert(line, trim);
            pkg_trims.get(&line)
        }

        for (line, (pkg, slots)) in body_groups {
            match trim_pkg_stmt(
                line,
                &pkg,
                true,
                "",
                &stmts,
                &source_text,
                &pkg_members,
                &mut pkg_trims,
                &mut warnings,
            ) {
                Some(trim) => {
                    body_hay.insert(pkg.clone(), trim.member_hay.clone());
                    for slot in slots {
                        if let MemberRoute::PackageMember { member, .. } = &members[slot].route {
                            if trim.found_members.contains(member) {
                                pkg_slot_span.insert(slot, (trim.start_line, trim.end_line));
                            } else {
                                warnings.push(format!(
                                    "'{}': name_mismatch — package member '{}' not found \
                                     in the re-parsed package body; skipped",
                                    members[slot].key, member
                                ));
                            }
                        }
                    }
                }
                None => {
                    for slot in slots {
                        warnings.push(format!(
                            "'{}': no package body statement for '{}' at line {} in '{}'",
                            members[slot].key, pkg, line, file_display
                        ));
                    }
                }
            }
        }
        for (slot, line, pkg) in node_items {
            // A spec statement keeps declarations referenced by the selected
            // members' bodies — signatures alone cannot see those references.
            let is_spec_stmt = stmts
                .iter()
                .find(|s| s.start_line == line)
                .is_some_and(|s| matches!(s.statement, Statement::CreatePackage(_)));
            let extra = if is_spec_stmt {
                body_hay.get(&pkg).map(String::as_str).unwrap_or("")
            } else {
                ""
            };
            if trim_pkg_stmt(
                line,
                &pkg,
                false,
                extra,
                &stmts,
                &source_text,
                &pkg_members,
                &mut pkg_trims,
                &mut warnings,
            )
            .is_some()
            {
                if let Some(t) = pkg_trims.get(&line) {
                    pkg_slot_span.insert(slot, (t.start_line, t.end_line));
                }
            } else {
                warnings.push(format!(
                    "'{}': no package statement for '{}' at line {} in '{}'",
                    members[slot].key, pkg, line, file_display
                ));
            }
        }

        // ── Regular members: strict span matching ──
        let mut matched: Vec<(usize, LineSpan)> = Vec::new();
        let mut extracted_slots: std::collections::HashSet<usize> =
            std::collections::HashSet::new();
        for (slot, member) in members.iter().enumerate() {
            if !matches!(member.route, MemberRoute::Regular) {
                continue;
            }
            match match_span(&index, member.line, member.name.as_deref()) {
                SpanMatch::Matched(span) => {
                    matched.push((slot, span));
                    extracted_slots.insert(slot);
                }
                SpanMatch::NameMismatch => {
                    warnings.push(format!(
                        "'{}': name_mismatch — statement(s) start at line {} in '{}' but \
                         none defines '{}'; member skipped (a wrong definition is worse \
                         than a missing report)",
                        member.key,
                        member.line,
                        file_display,
                        member.name.as_deref().unwrap_or("?")
                    ));
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

        // Output segments in source order: regular spans + package trims.
        let mut segments: Vec<(usize, String)> = Vec::new();
        if !matched.is_empty() {
            let spans: Vec<LineSpan> = matched.iter().map(|(_, s)| *s).collect();
            for span in merge_spans(spans) {
                match slice_span(&source_text, span) {
                    Some(text) => segments.push((span.start_line, text)),
                    None => {
                        warnings.push(format!(
                            "statement spans exceed the bounds of '{}' — the file changed \
                             since the last `codeweb analyze`; nothing written for it",
                            file_display
                        ));
                        segments.clear();
                        break;
                    }
                }
            }
        }
        for trim in pkg_trims.values() {
            segments.push((trim.start_line, trim.text.clone()));
        }
        segments.sort_by_key(|(line, _)| *line);

        if segments.is_empty() {
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

        let trimmed = segments
            .iter()
            .map(|(_, t)| t.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        let rel = out_rel
            .get(&stored_path)
            .expect("output mapping precomputed for every by_file key")
            .clone();
        let out_path = output_dir.join(&rel);
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

        // Range report: one entry per member whose span/statement was used.
        let mut ranges: Vec<RangeReport> = Vec::new();
        for (slot, span) in &matched {
            ranges.push(RangeReport {
                node: members[*slot].key.clone(),
                start_line: span.start_line,
                end_line: span.end_line,
            });
        }
        for (slot, (sl, el)) in &pkg_slot_span {
            ranges.push(RangeReport {
                node: members[*slot].key.clone(),
                start_line: *sl,
                end_line: *el,
            });
        }
        ranges.sort_by_key(|r| r.start_line);
        let out_display = rel.to_string_lossy().into_owned();
        files_report.push(FileExtractionReport {
            source: file_display.clone(),
            output: out_display.clone(),
            ranges,
        });

        for (slot, member) in members.iter().enumerate() {
            let extracted = extracted_slots.contains(&slot) || pkg_slot_span.contains_key(&slot);
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
        Node::Sequence { location, .. } => location.as_ref().map(|l| (l.file.clone(), l.line)),
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

        // Strict matching (review on #194): without a name hint and several
        // candidates there is NO guess — the member is reported missing.
        assert!(matches!(match_span(&index, 1, None), SpanMatch::NotFound));
        // A name that matches no candidate is a name_mismatch, not a pick.
        assert!(matches!(
            match_span(&index, 1, Some("t_z")),
            SpanMatch::NameMismatch
        ));
    }

    /// A single candidate whose object name differs from the node's must NOT
    /// be exported — the reviewer observed `prc_gen_inst` silently resolving
    /// to `prc_gen_inst_byfund`'s definition (review on #194).
    #[test]
    fn should_reject_single_candidate_with_mismatched_name() {
        let source = "CREATE PROCEDURE prc_gen_inst_byfund AS\nBEGIN\n    NULL;\nEND;\n/\n";
        let f = write_temp_sql(source);
        let (stmts, _text) = crate::parser::parse_sql_file_with_source(f.path()).expect("parse");
        let index = build_span_index(&stmts);

        assert!(matches!(
            match_span(&index, 1, Some("prc_gen_inst")),
            SpanMatch::NameMismatch
        ));
        assert!(matches!(
            match_span(&index, 1, Some("prc_gen_inst_byfund")),
            SpanMatch::Matched(_)
        ));
        // Nameless node kinds may still match a single candidate directly.
        assert!(matches!(match_span(&index, 1, None), SpanMatch::Matched(_)));
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
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().join("proj");
        std::fs::create_dir_all(root.join("src/schema")).unwrap();
        std::fs::write(root.join("src/schema/x.sql"), "SELECT 1;").unwrap();

        let rel = output_relative_path(Path::new("./src/schema/x.sql"), &root)
            .expect("in-root relative path must map");
        assert_eq!(rel, PathBuf::from("src/schema/x.sql"));

        // Absolute stored paths inside the root map the same way.
        let abs = output_relative_path(&root.join("src/schema/x.sql"), &root)
            .expect("in-root absolute path must map");
        assert_eq!(abs, PathBuf::from("src/schema/x.sql"));
    }

    /// Real-store form (review on #194): analyze run from a different working
    /// directory records `../<project>/...` relative paths. They must resolve
    /// back under the project root and keep the directory structure — the old
    /// flatten behavior dumped 564/564 real files into the output root.
    #[test]
    fn should_resolve_dotdot_relative_paths_back_under_project_root() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().join("proj");
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::write(root.join("src/deep/x.sql"), "SELECT 1;").unwrap();

        // Real store form: analyze recorded "../proj/src/deep/x.sql" (the
        // `..` re-enters the project). Joined onto the root and normalized it
        // lands back inside — structure preserved, no flatten.
        let stored = PathBuf::from("../proj/src/deep/x.sql");
        let rel =
            output_relative_path(&stored, &root).expect("`..` path re-entering the root must map");
        assert_eq!(rel, PathBuf::from("src/deep/x.sql"));

        // A `..` path that escapes the root for good is an error, not a flatten.
        assert!(output_relative_path(Path::new("../other/src/x.sql"), &root).is_err());
    }

    /// Locations that cannot be mapped under the project root are a hard
    /// error — never flattened (flattening let same-named files from
    /// different directories silently overwrite each other).
    #[test]
    fn should_reject_unmappable_paths_instead_of_flattening() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().join("proj");
        std::fs::create_dir_all(&root).unwrap();

        assert!(
            output_relative_path(Path::new("/etc/hostname"), &root).is_err(),
            "foreign absolute path must fail"
        );
        assert!(
            output_relative_path(Path::new("../../outside/x.sql"), &root).is_err(),
            "`..` escape must fail"
        );
        assert!(
            output_relative_path(Path::new(""), &root).is_err(),
            "empty location must fail"
        );
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
            let tokens = ogsql_parser::Tokenizer::new(CALL_CHAIN_SQL)
                .tokenize()
                .unwrap();
            let mut parser = ogsql_parser::Parser::with_source(tokens, CALL_CHAIN_SQL.to_string());
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
            None,
        )
        .expect("extraction should succeed");

        let written =
            std::fs::read_to_string(out_dir.join("src/app.sql")).expect("output file exists");
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
        assert_eq!(
            file_report.ranges.len(),
            3,
            "{:?}: {outcome:?}",
            file_report
        );
        let starts: Vec<usize> = file_report.ranges.iter().map(|r| r.start_line).collect();
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
            None,
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
            None,
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

        let tokens = ogsql_parser::Tokenizer::new(&without_ddl)
            .tokenize()
            .unwrap();
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
            None,
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
            None,
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
        let (graph, root, solo, _dir) = solo_project_fixture();
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
            None,
        )
        .expect("overwrite run should succeed");

        assert!(
            !out_dir.join("stale").exists(),
            "stale content must be cleared"
        );
        let written = std::fs::read_to_string(out_dir.join("src/s.sql")).unwrap();
        assert!(written.contains("CREATE PROCEDURE solo"), "{written:?}");
    }

    // ── Review on #194, fix 4: package member-level trimming ────
    //
    // Object-level trimming is the point of #193: a package member root must
    // produce a REASSEMBLED `CREATE PACKAGE [BODY]` containing only the
    // selected members plus the package-level declarations they reference —
    // never the whole package verbatim.

    /// Build a graph + on-disk project from `sql` stored at `stored_path`.
    fn package_project(
        sql: &str,
        stored_path: &str,
    ) -> (crate::graph::CodeGraph, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().unwrap();
        let disk = dir.path().join(stored_path);
        std::fs::create_dir_all(disk.parent().unwrap()).unwrap();
        std::fs::write(&disk, sql).unwrap();

        let tokens = ogsql_parser::Tokenizer::new(sql).tokenize().unwrap();
        let mut parser = ogsql_parser::Parser::with_source(tokens, sql.to_string());
        let parsed = vec![crate::parser::ParsedFile {
            path: PathBuf::from(stored_path),
            statements: parser.parse_with_text(),
            content_hash: String::new(),
        }];
        let graph = crate::graph::builder::GraphBuilder::new().build(&parsed);
        (graph, dir)
    }

    fn find_pkg_member(
        graph: &crate::graph::CodeGraph,
        pkg: &str,
        member: &str,
    ) -> petgraph::graph::NodeIndex {
        graph
            .node_indices()
            .find(|&idx| {
                matches!(&graph[idx], crate::graph::Node::Procedure { id, .. } | crate::graph::Node::Function { id, .. }
                    if id.package.as_deref() == Some(pkg) && id.name == member)
            })
            .unwrap_or_else(|| panic!("{pkg}.{member} in graph"))
    }

    fn find_pkg_node(graph: &crate::graph::CodeGraph, name: &str) -> petgraph::graph::NodeIndex {
        graph
            .node_indices()
            .find(|&idx| matches!(&graph[idx], crate::graph::Node::Package { name: n, .. } if n == name))
            .unwrap_or_else(|| panic!("package {name} in graph"))
    }

    /// p1 selected / p2 not: the output is a reassembled package body with
    /// ONLY p1's body — referenced package vars kept, unreferenced dropped.
    #[test]
    fn should_trim_package_body_to_selected_members_and_referenced_decls() {
        let sql = r#"CREATE OR REPLACE PACKAGE BODY pkg_a AS
    v_count NUMBER;
    v_unused NUMBER;

    PROCEDURE p1 IS
    BEGIN
        v_count := v_count + 1;
    END;

    PROCEDURE p2 IS
    BEGIN
        v_unused := 0;
    END;
END pkg_a;
/"#;
        let (graph, dir) = package_project(sql, "src/pkg.sql");
        let p1 = find_pkg_member(&graph, "pkg_a", "p1");
        let root = dir.path().to_path_buf();

        let out_dir = dir.path().join("out");
        let outcome = run_extract(
            &graph,
            &root,
            &[p1],
            petgraph::Direction::Outgoing,
            "outgoing",
            None,
            &out_dir,
            false,
            None,
        )
        .expect("extraction should succeed");

        let written = std::fs::read_to_string(out_dir.join("src/pkg.sql")).unwrap();
        assert!(
            written.starts_with("CREATE OR REPLACE PACKAGE BODY pkg_a AS"),
            "reassembled deployable wrapper: {written:?}"
        );
        assert!(written.contains("PROCEDURE p1 IS"), "p1 kept: {written:?}");
        assert!(!written.contains("PROCEDURE p2"), "p2 trimmed: {written:?}");
        assert!(
            written.contains("v_count"),
            "referenced package var kept: {written:?}"
        );
        assert!(
            !written.contains("v_unused"),
            "unreferenced package var dropped: {written:?}"
        );
        assert!(
            written.trim_end().ends_with("END pkg_a;\n/"),
            "footer kept: {written:?}"
        );
        assert!(
            outcome
                .report
                .closure_nodes
                .iter()
                .all(|n| n.status != STATUS_MISSING_STATEMENT),
            "member must map to the package statement: {:?}",
            outcome.report.closure_nodes
        );
    }

    /// Package-level declarations form a reference chain: keeping the decl
    /// that a member uses must transitively keep the decl THAT decl uses.
    #[test]
    fn should_keep_transitively_referenced_package_decls() {
        let sql = r#"CREATE OR REPLACE PACKAGE BODY pkg_t AS
    v_base NUMBER := 1;
    v_derived NUMBER;
    v_lonely NUMBER;

    PROCEDURE p1 IS
    BEGIN
        v_derived := v_base * 2;
    END;
END pkg_t;
/"#;
        let (graph, dir) = package_project(sql, "src/pkg.sql");
        let p1 = find_pkg_member(&graph, "pkg_t", "p1");
        let root = dir.path().to_path_buf();

        run_extract(
            &graph,
            &root,
            &[p1],
            petgraph::Direction::Outgoing,
            "outgoing",
            None,
            &dir.path().join("out"),
            false,
            None,
        )
        .expect("extraction should succeed");

        let written = std::fs::read_to_string(dir.path().join("out/src/pkg.sql")).unwrap();
        assert!(
            written.contains("v_derived"),
            "direct ref kept: {written:?}"
        );
        assert!(
            written.contains("v_base"),
            "transitive ref (v_derived uses v_base) kept: {written:?}"
        );
        assert!(
            !written.contains("v_lonely"),
            "unreferenced decl dropped: {written:?}"
        );
    }

    /// A package node with no members in the closure still emits a minimal
    /// deployable wrapper — never broken SQL.
    #[test]
    fn should_emit_minimal_package_wrapper_for_bare_package_node() {
        let sql = r#"CREATE OR REPLACE PACKAGE BODY pkg_m AS
    PROCEDURE p1 IS
    BEGIN
        NULL;
    END;
END pkg_m;
/"#;
        let (graph, dir) = package_project(sql, "src/pkg.sql");
        let pkg = find_pkg_node(&graph, "pkg_m");
        let root = dir.path().to_path_buf();

        // depth 0: only the package node itself, no members.
        run_extract(
            &graph,
            &root,
            &[pkg],
            petgraph::Direction::Outgoing,
            "outgoing",
            Some(0),
            &dir.path().join("out"),
            false,
            None,
        )
        .expect("extraction should succeed");

        let written = std::fs::read_to_string(dir.path().join("out/src/pkg.sql")).unwrap();
        assert!(
            written.contains("CREATE OR REPLACE PACKAGE BODY pkg_m AS")
                && written.contains("END pkg_m;"),
            "minimal deployable wrapper: {written:?}"
        );
        assert!(
            !written.contains("PROCEDURE p1"),
            "no members selected → none emitted: {written:?}"
        );
    }

    /// Spec + body in one file: an incoming-rooted member pulls the package
    /// node into the closure, so BOTH statements are reassembled — the spec
    /// emits only the selected member's signature plus the public
    /// declarations its body references.
    #[test]
    fn should_trim_spec_and_body_for_selected_member() {
        let sql = r#"CREATE OR REPLACE PACKAGE pkg_b AS
    v_pub NUMBER;
    v_spec_only NUMBER;
    PROCEDURE p1(x INT);
    PROCEDURE p2;
END pkg_b;
/

CREATE OR REPLACE PACKAGE BODY pkg_b AS
    v_priv NUMBER;
    PROCEDURE p1(x INT) IS
    BEGIN
        v_pub := x;
        v_priv := v_pub + 1;
    END;

    PROCEDURE p2 IS
    BEGIN
        v_priv := 0;
    END;
END pkg_b;
/"#;
        let (graph, dir) = package_project(sql, "src/pkg.sql");
        let p1 = find_pkg_member(&graph, "pkg_b", "p1");
        let root = dir.path().to_path_buf();

        // incoming: pkg_b node (ContainsRoutine edge) enters the closure.
        run_extract(
            &graph,
            &root,
            &[p1],
            petgraph::Direction::Incoming,
            "incoming",
            None,
            &dir.path().join("out"),
            false,
            None,
        )
        .expect("extraction should succeed");

        let written = std::fs::read_to_string(dir.path().join("out/src/pkg.sql")).unwrap();
        // Spec: only p1's signature + the public var its body uses.
        assert!(
            written.contains("CREATE OR REPLACE PACKAGE pkg_b AS"),
            "spec emitted: {written:?}"
        );
        assert!(
            written.contains("PROCEDURE p1(x INT);"),
            "p1 sig kept: {written:?}"
        );
        assert!(
            !written.contains("PROCEDURE p2;"),
            "p2 sig dropped: {written:?}"
        );
        assert!(written.contains("v_pub"), "public var kept: {written:?}");
        assert!(
            !written.contains("v_spec_only"),
            "unreferenced spec var dropped: {written:?}"
        );
        // Body: only p1's body + its private var.
        assert!(
            written.contains("CREATE OR REPLACE PACKAGE BODY pkg_b AS"),
            "body emitted: {written:?}"
        );
        assert!(
            written.contains("PROCEDURE p1(x INT) IS"),
            "p1 body kept: {written:?}"
        );
        assert!(
            !written.contains("PROCEDURE p2 IS"),
            "p2 body dropped: {written:?}"
        );
        assert!(written.contains("v_priv"), "private var kept: {written:?}");
    }

    // ── Review on #194: output/report path isolation ────────────

    /// Regression (blocker): `--output <srcdir> --overwrite` used to delete
    /// every analyzed SQL file and still exit 0. The run must be refused
    /// BEFORE `remove_dir_all`, leaving sources byte- and mtime-identical.
    #[test]
    fn should_refuse_output_dir_containing_source_files() {
        let (graph, root, solo, _dir) = solo_project_fixture();
        let src_dir = root.join("src");
        let src_file = src_dir.join("s.sql");
        let before = std::fs::read(&src_file).unwrap();
        let mtime_before = std::fs::metadata(&src_file).unwrap().modified().unwrap();

        let err = run_extract(
            &graph,
            &root,
            &[solo],
            petgraph::Direction::Outgoing,
            "outgoing",
            None,
            &src_dir,
            true,
            None,
        )
        .expect_err("output dir containing sources must be refused");

        assert!(
            matches!(err, ExtractError::ProtectedPath { .. }),
            "expected ProtectedPath, got {err}"
        );
        assert_eq!(std::fs::read(&src_file).unwrap(), before);
        assert_eq!(
            std::fs::metadata(&src_file).unwrap().modified().unwrap(),
            mtime_before,
            "source file must be untouched"
        );
    }

    /// The project root itself is never a valid output directory.
    #[test]
    fn should_refuse_output_dir_equal_to_project_root() {
        let (graph, root, solo, _dir) = solo_project_fixture();
        let err = run_extract(
            &graph,
            &root,
            &[solo],
            petgraph::Direction::Outgoing,
            "outgoing",
            None,
            &root,
            true,
            None,
        )
        .expect_err("project root as output must be refused");
        assert!(matches!(err, ExtractError::ProtectedPath { .. }));
        assert!(root.join("src/s.sql").exists());
    }

    /// A report path pointing at a source file must be refused before any
    /// write — the report `fs::write` would otherwise overwrite the SQL.
    #[test]
    fn should_refuse_report_path_equal_to_source_file() {
        let (graph, root, solo, _dir) = solo_project_fixture();
        let src_file = root.join("src/s.sql");
        let before = std::fs::read(&src_file).unwrap();

        let err = run_extract(
            &graph,
            &root,
            &[solo],
            petgraph::Direction::Outgoing,
            "outgoing",
            None,
            &root.join("out"),
            false,
            Some(&src_file),
        )
        .expect_err("report path on a source file must be refused");

        assert!(matches!(err, ExtractError::ProtectedPath { .. }));
        assert_eq!(std::fs::read(&src_file).unwrap(), before);
        assert!(
            !root.join("out").exists(),
            "refusal happens before the output dir is created"
        );
    }

    /// Regression (review on #194 round 2, blocker): source protection must
    /// cover EVERY analyzed file, not just closure members.
    /// `unrelated/keep.sql` was analyzed (its nodes are in the graph) but is
    /// not in `a1`'s closure — `--output unrelated --overwrite` and
    /// `--report unrelated/keep.sql` must both be refused before any write.
    #[test]
    fn should_protect_analyzed_sources_outside_the_closure() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let used = root.join("used");
        let unrelated = root.join("unrelated");
        std::fs::create_dir_all(&used).unwrap();
        std::fs::create_dir_all(&unrelated).unwrap();
        let a_sql = "CREATE PROCEDURE a1 AS\nBEGIN\n    NULL;\nEND;\n/\n";
        let keep_sql = "CREATE PROCEDURE keep_p AS\nBEGIN\n    NULL;\nEND;\n/\n";
        std::fs::write(used.join("a.sql"), a_sql).unwrap();
        std::fs::write(unrelated.join("keep.sql"), keep_sql).unwrap();

        let mk = |text: &str, stored: &str| {
            let tokens = ogsql_parser::Tokenizer::new(text).tokenize().unwrap();
            let mut parser = ogsql_parser::Parser::with_source(tokens, text.to_string());
            crate::parser::ParsedFile {
                path: PathBuf::from(stored),
                statements: parser.parse_with_text(),
                content_hash: String::new(),
            }
        };
        let parsed = vec![mk(a_sql, "used/a.sql"), mk(keep_sql, "unrelated/keep.sql")];
        let graph = crate::graph::builder::GraphBuilder::new().build(&parsed);
        let a1 = graph
            .node_indices()
            .find(|&idx| matches!(&graph[idx], crate::graph::Node::Procedure { id, .. } if id.name == "a1"))
            .expect("a1 in graph");
        let keep_before = std::fs::read(unrelated.join("keep.sql")).unwrap();

        // Output dir containing an analyzed-but-not-in-closure source.
        let err = run_extract(
            &graph,
            &root,
            &[a1],
            petgraph::Direction::Outgoing,
            "outgoing",
            None,
            &unrelated,
            true,
            None,
        )
        .expect_err("output dir containing analyzed sources must be refused");
        assert!(
            matches!(err, ExtractError::ProtectedPath { .. }),
            "expected ProtectedPath, got {err}"
        );
        assert_eq!(
            std::fs::read(unrelated.join("keep.sql")).unwrap(),
            keep_before,
            "keep.sql must survive --overwrite"
        );

        // Report path equal to an analyzed-but-not-in-closure source.
        let err = run_extract(
            &graph,
            &root,
            &[a1],
            petgraph::Direction::Outgoing,
            "outgoing",
            None,
            &root.join("out"),
            false,
            Some(&unrelated.join("keep.sql")),
        )
        .expect_err("report path on an analyzed source must be refused");
        assert!(matches!(err, ExtractError::ProtectedPath { .. }));
        assert_eq!(
            std::fs::read(unrelated.join("keep.sql")).unwrap(),
            keep_before,
            "keep.sql must not be overwritten by the report"
        );
        assert!(
            !root.join("out").exists(),
            "refusal happens before the output dir is created"
        );
    }

    /// Two different stored paths resolving to the same output path must be
    /// a hard error — flatten/mirror collisions silently overwrote files.
    #[test]
    fn should_fail_on_output_path_collisions() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let src = root.join("d");
        std::fs::create_dir_all(&src).unwrap();
        // pa lives at stored path "d/x.sql"; pb at "d/../d/x.sql" — different
        // PathBuf keys that canonicalize to the SAME output path.
        let sql_a = "CREATE PROCEDURE pa AS\nBEGIN\n    PERFORM pb();\nEND;\n/\n";
        let sql_b = "CREATE PROCEDURE pb AS\nBEGIN\n    NULL;\nEND;\n/\n";
        std::fs::write(src.join("x.sql"), sql_a).unwrap();

        let mk = |text: &str, stored: &str| {
            let tokens = ogsql_parser::Tokenizer::new(text).tokenize().unwrap();
            let mut parser = ogsql_parser::Parser::with_source(tokens, text.to_string());
            crate::parser::ParsedFile {
                path: PathBuf::from(stored),
                statements: parser.parse_with_text(),
                content_hash: String::new(),
            }
        };
        let parsed = vec![mk(sql_a, "d/x.sql"), mk(sql_b, "d/../d/x.sql")];
        let graph = crate::graph::builder::GraphBuilder::new().build(&parsed);
        let pa = graph
            .node_indices()
            .find(|&idx| matches!(&graph[idx], crate::graph::Node::Procedure { id, .. } if id.name == "pa"))
            .expect("pa in graph");

        // pa → pb puts both files in the closure; both map to "d/x.sql".
        let err = run_extract(
            &graph,
            &root,
            &[pa],
            petgraph::Direction::Outgoing,
            "outgoing",
            None,
            &root.join("out"),
            false,
            None,
        )
        .expect_err("colliding output paths must fail");
        assert!(matches!(err, ExtractError::ProtectedPath { .. }));
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

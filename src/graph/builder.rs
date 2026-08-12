#[allow(unused_imports)]
use crate::graph::key::NodeKey;
use crate::graph::store::GraphStore;
use crate::graph::{
    determine_call_scope, extract_routine_id, CallScope, CodeGraph, DataFlowKind, Edge, Node,
    RoutineId, RoutineKind, SourceLocation,
};
use crate::graph::{ColumnSummary, DistributeInfo, IndexConstraint, PartitionInfo};
use crate::parser::{
    AllParsedFiles, CallEdge, CallExtractor, ParsedFile, TypeSequenceRefExtractor,
};
use ogsql_parser::ast::{
    AlterTableAction, ColumnConstraint, PackageItem, Statement, TableConstraint,
};
use ogsql_parser::{walk_pl_block, walk_statement};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

/// Known GaussDB/openGauss system schemas.
/// Tables/views in these schemas are marked `system: true`.
const SYSTEM_SCHEMAS: &[&str] = &[
    "pg_catalog",
    "information_schema",
    "sys",
    "dbe_perf",
    "dbe_pldebugger",
    "dbe_scheduler",
    "dbe_session",
    "db4ai",
    "snapshot",
    "wdr_snapshot",
    "cstore",
];

/// Known system table/view names that lack a schema qualifier (e.g. `dual`).
const KNOWN_SYSTEM_NAMES: &[&str] = &["dual", "sys_dummy"];

/// Extract column summaries from a CREATE VIEW / CREATE MATERIALIZED VIEW
/// statement. Uses explicit column list if present, otherwise derives
/// column names from the SELECT targets.
fn extract_view_columns(
    explicit_columns: &[String],
    targets: &[ogsql_parser::ast::SelectTarget],
) -> Vec<ColumnSummary> {
    // Prefer explicit column list (CREATE VIEW v(col1, col2) AS ...)
    if !explicit_columns.is_empty() {
        return explicit_columns
            .iter()
            .map(|name| ColumnSummary {
                name: name.clone(),
                data_type: String::new(),
                nullable: true,
                is_primary_key: false,
                default_value: None,
                comment: None,
            })
            .collect();
    }

    // Derive column names from SELECT targets
    let mut columns = Vec::new();
    for target in targets {
        match target {
            ogsql_parser::ast::SelectTarget::Expr(expr, alias) => {
                let name = match alias {
                    Some(a) => a.to_string(),
                    None => extract_column_name_from_expr(expr),
                };
                if !name.is_empty() {
                    columns.push(ColumnSummary {
                        name,
                        data_type: String::new(),
                        nullable: true,
                        is_primary_key: false,
                        default_value: None,
                        comment: None,
                    });
                }
            }
            ogsql_parser::ast::SelectTarget::Star(_alias) => {
                // Cannot determine individual column names from SELECT *
                // without source table info; skip.
            }
        }
    }
    columns
}

/// Extract a column name from a SELECT expression.
/// For simple ColumnRef expressions, returns the column name.
/// For all other expressions, returns a "?" placeholder.
fn extract_column_name_from_expr(expr: &ogsql_parser::ast::Expr) -> String {
    use ogsql_parser::ast::Expr;
    match expr {
        Expr::ColumnRef(name) | Expr::ColumnRefOuterJoin(name) => {
            name.last().map(|i| i.to_string()).unwrap_or_default()
        }
        _ => String::new(),
    }
}

fn is_system(schema: Option<&str>, name: &str) -> bool {
    if KNOWN_SYSTEM_NAMES.contains(&name.to_lowercase().as_str()) {
        return true;
    }
    schema
        .map(|s| SYSTEM_SCHEMAS.contains(&s.to_lowercase().as_str()))
        .unwrap_or(false)
}

pub struct GraphBuilder {
    /// When enabled, column-level lineage edges (`DataFlow` / `Derived` /
    /// `Aggregated`) are extracted from `SELECT` statements and injected into
    /// the graph as `Node::Column` nodes. Default: disabled (MVP keeps column
    /// lineage opt-in).
    pub enable_column_lineage: bool,
}

/// A column comment from a standalone `COMMENT ON COLUMN` statement,
/// deferred until all table columns are populated in `finalize_graph`.
pub struct DeferredColumnComment {
    pub table_key: String,
    pub col_name: String,
    pub comment: String,
}

/// Accumulated indexing state for incremental graph building across chunks.
pub struct GraphBuildContext {
    pub graph: CodeGraph,
    pub proc_index: HashMap<RoutineId, petgraph::graph::NodeIndex>,
    pub package_index: HashMap<String, petgraph::graph::NodeIndex>,
    pub mapper_index: HashMap<String, petgraph::graph::NodeIndex>,
    pub table_index: HashMap<String, petgraph::graph::NodeIndex>,
    pub type_index: HashMap<String, petgraph::graph::NodeIndex>,
    pub sequence_index: HashMap<String, petgraph::graph::NodeIndex>,
    /// Shared dedup index for BuiltinFunction nodes (keyed by lowercased name).
    /// Threaded through SQL-proc / XML-mapper / Java / JSP paths so the same
    /// builtin called from multiple paths is a single graph node.
    pub builtin_index: HashMap<String, petgraph::graph::NodeIndex>,
    /// Deferred column comments from `COMMENT ON COLUMN` statements.
    /// Collected during `create_sql_nodes` and applied in `finalize_graph`
    /// after all table columns are populated.
    pub deferred_column_comments: Vec<DeferredColumnComment>,
}

impl GraphBuildContext {
    pub fn new() -> Self {
        Self {
            graph: CodeGraph::new(),
            proc_index: HashMap::new(),
            package_index: HashMap::new(),
            mapper_index: HashMap::new(),
            table_index: HashMap::new(),
            type_index: HashMap::new(),
            sequence_index: HashMap::new(),
            builtin_index: HashMap::new(),
            deferred_column_comments: Vec::new(),
        }
    }
}

/// A call extracted from a SQL statement (XML-mapper / Java / JSP path).
///
/// Carries builtin metadata so consumers can branch: builtins become
/// `BuiltinFunction` nodes + `UsesBuiltinFunction` edges; everything else
/// follows the existing `CallsProcedure` path.
#[derive(Clone)]
pub(crate) struct ExtractedCall {
    pub callee_name: String,
    pub builtin_meta: Option<ogsql_parser::ast::BuiltinFuncMeta>,
}

impl GraphBuilder {
    pub fn new() -> Self {
        Self {
            enable_column_lineage: false,
        }
    }

    /// Enable or disable column-level lineage extraction.
    #[allow(dead_code)]
    pub fn with_column_lineage(mut self, enabled: bool) -> Self {
        self.enable_column_lineage = enabled;
        self
    }

    pub fn build(&self, files: &[ParsedFile]) -> CodeGraph {
        Self::build_graph_internal(files, &[], &[], &[], self.enable_column_lineage)
    }

    #[cfg_attr(feature = "jsp", allow(dead_code))]
    pub fn build_all(&self, all: &AllParsedFiles) -> CodeGraph {
        Self::build_graph_internal(
            &all.sql_files,
            &all.ibatis_files,
            &all.java_files,
            &all.java_method_results,
            self.enable_column_lineage,
        )
    }

    #[allow(dead_code)]
    pub fn build_store(&self, all: &AllParsedFiles, project_name: &str) -> GraphStore {
        let graph = Self::build_graph_internal(
            &all.sql_files,
            &all.ibatis_files,
            &all.java_files,
            &all.java_method_results,
            self.enable_column_lineage,
        );
        GraphStore::from_graph(project_name, graph)
    }

    fn build_graph_internal(
        sql_files: &[ParsedFile],
        ibatis_files: &[crate::parser::ibatis_loader::IbatisParsedFile],
        java_files: &[crate::parser::java_loader::JavaParsedFile],
        java_method_results: &[crate::parser::java_method::JavaParseResult],
        enable_column_lineage: bool,
    ) -> CodeGraph {
        let mut ctx = GraphBuildContext::new();

        Self::build_sql_chunk(&mut ctx, sql_files, enable_column_lineage);
        Self::add_ibatis_nodes_from_parsed(
            ibatis_files,
            &mut ctx.graph,
            &mut ctx.proc_index,
            &mut ctx.mapper_index,
            &mut ctx.table_index,
            &mut ctx.builtin_index,
        );
        Self::add_java_nodes_from_parsed(
            java_files,
            &mut ctx.graph,
            &mut ctx.proc_index,
            &ctx.mapper_index,
            &mut ctx.table_index,
            &mut ctx.builtin_index,
        );
        Self::add_java_method_nodes_from_parsed(
            java_method_results,
            &mut ctx.graph,
            &mut ctx.proc_index,
            &ctx.mapper_index,
        );
        Self::finalize_graph(&mut ctx);

        ctx.graph
    }

    /// Build graph with explicit JSP file results. Only available with `jsp` feature.
    #[cfg(feature = "jsp")]
    pub fn build_all_with_jsp(
        &self,
        all: &AllParsedFiles,
        jsp_files: &[crate::parser::jsp_loader::JspFileResult],
    ) -> CodeGraph {
        let mut ctx = GraphBuildContext::new();
        Self::build_sql_chunk(&mut ctx, &all.sql_files, self.enable_column_lineage);
        Self::add_ibatis_nodes_from_parsed(
            &all.ibatis_files,
            &mut ctx.graph,
            &mut ctx.proc_index,
            &mut ctx.mapper_index,
            &mut ctx.table_index,
            &mut ctx.builtin_index,
        );
        Self::add_java_nodes_from_parsed(
            &all.java_files,
            &mut ctx.graph,
            &mut ctx.proc_index,
            &ctx.mapper_index,
            &mut ctx.table_index,
            &mut ctx.builtin_index,
        );
        Self::add_java_method_nodes_from_parsed(
            &all.java_method_results,
            &mut ctx.graph,
            &mut ctx.proc_index,
            &ctx.mapper_index,
        );
        Self::add_jsp_nodes_from_parsed(jsp_files, &mut ctx);
        let mut simple_to_fqn: HashMap<String, String> = HashMap::new();
        for result in &all.java_method_results {
            for class in &result.classes {
                simple_to_fqn.insert(class.name.clone(), class.fqn.clone());
            }
        }
        Self::bridge_jsp_to_java_methods(&mut ctx.graph, jsp_files, &simple_to_fqn);
        Self::finalize_graph(&mut ctx);
        ctx.graph
    }

    /// Process a single chunk of parsed SQL files into the accumulating context.
    /// The context's indices are updated so that subsequent chunks can
    /// reference nodes created in earlier chunks.
    pub fn build_sql_chunk(
        ctx: &mut GraphBuildContext,
        sql_files: &[ParsedFile],
        enable_column_lineage: bool,
    ) {
        Self::create_sql_nodes(
            sql_files,
            &mut ctx.graph,
            &mut ctx.proc_index,
            &mut ctx.package_index,
            &mut ctx.table_index,
            &mut ctx.type_index,
            &mut ctx.sequence_index,
            &mut ctx.deferred_column_comments,
        );
        Self::create_sql_edges(
            sql_files,
            &mut ctx.graph,
            &mut ctx.proc_index,
            &mut ctx.table_index,
            &ctx.type_index,
            &mut ctx.builtin_index,
            enable_column_lineage,
        );
        Self::create_object_ref_edges(
            sql_files,
            &mut ctx.graph,
            &ctx.proc_index,
            &ctx.type_index,
            &ctx.sequence_index,
        );
    }

    /// Finalize the graph after all files are processed.
    /// Must be called exactly once after all chunks and non-SQL files are added.
    pub fn finalize_graph(ctx: &mut GraphBuildContext) {
        Self::dedup_table_view_nodes(&mut ctx.graph);
        apply_deferred_column_comments(ctx);
        Self::merge_table_access_edges(&mut ctx.graph);
        Self::resolve_unresolved_nodes(&mut ctx.graph);
    }

    // ── Pass 1: Create all SQL nodes ─────────────────────────────
    // Merged from: create_procedure_nodes + detect_and_create_partial_nodes
    // + add_view_nodes (5 loops → 1 loop)

    #[allow(clippy::too_many_arguments)]
    fn create_sql_nodes(
        files: &[ParsedFile],
        graph: &mut CodeGraph,
        proc_index: &mut HashMap<RoutineId, petgraph::graph::NodeIndex>,
        package_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        table_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        type_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        sequence_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        deferred_column_comments: &mut Vec<DeferredColumnComment>,
    ) {
        for file in files {
            let file_arc: Arc<PathBuf> = Arc::new(file.path.clone());
            let mut spec_decls: Vec<(String, String, RoutineKind)> = Vec::new();
            let mut body_impls: Vec<(String, String)> = Vec::new();
            let mut has_body = false;

            for info in &file.statements {
                match &info.statement {
                    Statement::CreateProcedure(p) => {
                        let id = RoutineId::from_object_name(&p.name, RoutineKind::Procedure);
                        let body_sql = p
                            .block
                            .as_ref()
                            .map(|b| {
                                crate::parser::extract_body_sql(b)
                                    .into_iter()
                                    .map(|sql| crate::graph::ProcedureBodySql {
                                        sql_text: sql.sql_text,
                                        kind: sql.kind,
                                        line: sql.line,
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        proc_index.entry(id.normalized()).or_insert_with(|| {
                            let node = Node::Procedure {
                                id: id.normalized(),
                                location: SourceLocation {
                                    file: file_arc.clone(),
                                    line: info.start_line,
                                },
                                partial: false,
                                body_sql,
                            };
                            graph.add_node(node)
                        });
                    }
                    Statement::CreateFunction(f) => {
                        let id = RoutineId::from_object_name(&f.name, RoutineKind::Function);
                        let body_sql = f
                            .block
                            .as_ref()
                            .map(|b| {
                                crate::parser::extract_body_sql(b)
                                    .into_iter()
                                    .map(|sql| crate::graph::ProcedureBodySql {
                                        sql_text: sql.sql_text,
                                        kind: sql.kind,
                                        line: sql.line,
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        proc_index.entry(id.normalized()).or_insert_with(|| {
                            let node = Node::Function {
                                id: id.normalized(),
                                location: SourceLocation {
                                    file: file_arc.clone(),
                                    line: info.start_line,
                                },
                                partial: false,
                                body_sql,
                            };
                            graph.add_node(node)
                        });
                    }
                    Statement::CreatePackage(pkg) => {
                        Self::create_package_nodes(
                            &pkg.name,
                            &pkg.items,
                            true,
                            info.start_line,
                            &file_arc,
                            graph,
                            proc_index,
                            package_index,
                        );
                        let pkg_name = pkg.name.last().cloned().unwrap_or_default().to_lowercase();
                        for item in &pkg.items {
                            let (name, kind) = match item {
                                PackageItem::Procedure(p) => (
                                    p.name
                                        .iter()
                                        .map(|i| i.to_string().to_lowercase())
                                        .collect::<Vec<_>>()
                                        .join("."),
                                    RoutineKind::Procedure,
                                ),
                                PackageItem::Function(f) => (
                                    f.name
                                        .iter()
                                        .map(|i| i.to_string().to_lowercase())
                                        .collect::<Vec<_>>()
                                        .join("."),
                                    RoutineKind::Function,
                                ),
                                PackageItem::Raw(_)
                                | PackageItem::Variable(_)
                                | PackageItem::Type(_)
                                | PackageItem::Cursor(_) => continue,
                            };
                            spec_decls.push((pkg_name.clone(), name, kind));
                        }
                    }
                    Statement::CreatePackageBody(pkg) => {
                        has_body = true;
                        Self::create_package_nodes(
                            &pkg.name,
                            &pkg.items,
                            false,
                            info.start_line,
                            &file_arc,
                            graph,
                            proc_index,
                            package_index,
                        );
                        let pkg_name = pkg.name.last().cloned().unwrap_or_default().to_lowercase();
                        for item in &pkg.items {
                            let name = match item {
                                PackageItem::Procedure(p) => p
                                    .name
                                    .iter()
                                    .map(|i| i.to_string().to_lowercase())
                                    .collect::<Vec<_>>()
                                    .join("."),
                                PackageItem::Function(f) => f
                                    .name
                                    .iter()
                                    .map(|i| i.to_string().to_lowercase())
                                    .collect::<Vec<_>>()
                                    .join("."),
                                PackageItem::Raw(_)
                                | PackageItem::Variable(_)
                                | PackageItem::Type(_)
                                | PackageItem::Cursor(_) => continue,
                            };
                            body_impls.push((pkg_name.clone(), name));
                        }
                    }
                    Statement::CreateTrigger(t) => {
                        let trigger_node = Node::Trigger {
                            name: t.name.clone(),
                            table: t.table.iter().map(|i| i.to_string()).collect(),
                            location: SourceLocation {
                                file: file_arc.clone(),
                                line: info.start_line,
                            },
                        };
                        let trigger_idx = graph.add_node(trigger_node);

                        let func_id =
                            RoutineId::from_object_name(&t.func_name, RoutineKind::Function);
                        let func_idx = proc_index.get(&func_id.normalized()).copied().unwrap_or_else(|| {
                            let raw = t.func_name.join(".");
                            let snippet = crate::parser::snippet::read_snippet(
                                &file.path,
                                info.start_line,
                                1,
                            );
                            let suffix = unresolved_creation_suffix(
                                Some((
                                    func_id.schema.as_deref(),
                                    func_id.package.as_deref(),
                                    &func_id.name,
                                )),
                                snippet.as_deref(),
                            );
                            crate::parse_log::warn(
                                &format!(
                                    "{}:{}",
                                    file.path.to_string_lossy(),
                                    info.start_line
                                ),
                                &format!(
                                    "unresolved: trigger '{}' references function '{}' not found in parsed files{}",
                                    t.name, raw, suffix
                                ),
                            );
                            let unresolved = Node::Unresolved {
                                raw_expr: Box::new(raw),
                                context: Box::new(format!("trigger:{}", t.name)),
                            };
                            let idx = graph.add_node(unresolved);
                            proc_index.insert(func_id.normalized(), idx);
                            idx
                        });

                        graph.add_edge(
                            trigger_idx,
                            func_idx,
                            Edge::TriggersRoutine {
                                location: SourceLocation {
                                    file: file_arc.clone(),
                                    line: info.start_line,
                                },
                            },
                        );
                    }
                    Statement::CreateView(v) => {
                        let view_schema = if v.name.len() > 1 {
                            Some(v.name[..v.name.len() - 1].join("."))
                        } else {
                            None
                        };
                        let view_name = v.name.last().cloned().unwrap_or_default().to_string();
                        let columns = extract_view_columns(&v.columns, &v.query.targets);
                        let ddl_source = Some(Box::new(info.sql_text.clone()));
                        let view_node = Node::View {
                            schema: view_schema.clone(),
                            name: view_name.clone(),
                            explicit: true,
                            system: is_system(view_schema.as_deref(), &view_name),
                            location: Some(SourceLocation {
                                file: file_arc.clone(),
                                line: info.start_line,
                            }),
                            columns: Box::new(columns),
                            ddl_source,
                        };
                        let view_idx = graph.add_node(view_node);

                        let view_key = normalize_table_key(view_schema.as_deref(), &view_name);
                        table_index.entry(view_key).or_insert(view_idx);
                        if view_schema.is_some() {
                            table_index
                                .entry(view_name.to_lowercase())
                                .or_insert(view_idx);
                        }

                        let mut extractor = crate::parser::TableAccessExtractor::new();
                        let wrapped = Statement::Select(ogsql_parser::ast::Spanned {
                            node: v.query.as_ref().clone(),
                            span: None,
                        });
                        walk_statement(&mut extractor, &wrapped);

                        for access in &extractor.accesses {
                            let key = normalize_table_key(access.schema.as_deref(), &access.name);
                            let table_idx = *table_index.entry(key.clone()).or_insert_with(|| {
                                let node = Node::Table {
                                    schema: access.schema.clone(),
                                    name: access.name.clone(),
                                    explicit: false,
                                    system: is_system(access.schema.as_deref(), &access.name),
                                    location: None,
                                    columns: Box::new(vec![]),
                                    partition_by: None,
                                    distribute_by: None,
                                    tablespace: None,
                                    temporary: false,
                                    unlogged: false,
                                    ddl_source: None,
                                };
                                graph.add_node(node)
                            });
                            if access.schema.is_some() {
                                table_index
                                    .entry(access.name.to_lowercase())
                                    .or_insert(table_idx);
                            }
                            graph.add_edge(
                                view_idx,
                                table_idx,
                                Edge::DependsOn {
                                    location: SourceLocation {
                                        file: file_arc.clone(),
                                        line: info.start_line,
                                    },
                                },
                            );
                        }
                    }
                    Statement::CreateType(t) => {
                        let (schema, name) = split_object_name(&t.name);
                        let short_key = normalize_object_key(None, &name);
                        let full_key = normalize_object_key(schema.as_deref(), &name);
                        if !type_index.contains_key(&full_key) {
                            let type_kind = match &t.type_kind {
                                ogsql_parser::ast::TypeKind::Composite { .. } => "composite",
                                ogsql_parser::ast::TypeKind::Enum { .. } => "enum",
                                ogsql_parser::ast::TypeKind::Base { .. } => "base",
                                ogsql_parser::ast::TypeKind::Table { .. } => "table",
                                ogsql_parser::ast::TypeKind::Range { .. } => "range",
                                ogsql_parser::ast::TypeKind::Shell => "shell",
                            };
                            let type_node = Node::Type {
                                schema: schema.as_ref().map(|s| s.to_lowercase()),
                                name: name.to_lowercase(),
                                type_kind: type_kind.to_string(),
                                location: SourceLocation {
                                    file: file_arc.clone(),
                                    line: info.start_line,
                                },
                            };
                            let idx = graph.add_node(type_node);
                            type_index.entry(short_key).or_insert(idx);
                            type_index.insert(full_key, idx);
                        }
                    }
                    Statement::CreateSequence(s) => {
                        let (schema, name) = split_object_name(&s.name);
                        let short_key = normalize_object_key(None, &name);
                        let full_key = normalize_object_key(schema.as_deref(), &name);
                        if !sequence_index.contains_key(&full_key) {
                            let seq_node = Node::Sequence {
                                schema: schema.as_ref().map(|s| s.to_lowercase()),
                                name: name.to_lowercase(),
                                location: SourceLocation {
                                    file: file_arc.clone(),
                                    line: info.start_line,
                                },
                            };
                            let idx = graph.add_node(seq_node);
                            sequence_index.entry(short_key).or_insert(idx);
                            sequence_index.insert(full_key, idx);
                        }
                    }
                    Statement::CreateIndex(i) => {
                        let idx_name = i
                            .name
                            .as_ref()
                            .map(|n| n.last().cloned().unwrap_or_default().to_string());
                        let (table_schema, table_name) = split_object_name(&i.table);
                        let index_columns: Vec<String> = i
                            .columns
                            .iter()
                            .filter_map(|c| {
                                c.name.clone().or_else(|| {
                                    c.expr.as_ref().map(crate::graph::format::format_expr)
                                })
                            })
                            .collect();
                        let where_clause = i
                            .where_clause
                            .as_ref()
                            .map(crate::graph::format::format_expr);
                        let index_node = Node::Index {
                            name: idx_name,
                            table_schema: table_schema.clone(),
                            table_name: table_name.clone(),
                            unique: i.unique,
                            global: false,
                            index_method: i.using_method.clone(),
                            columns: index_columns,
                            tablespace: i.tablespace.clone(),
                            where_clause,
                            constraint: None,
                            location: SourceLocation {
                                file: file_arc.clone(),
                                line: info.start_line,
                            },
                        };
                        let idx = graph.add_node(index_node);
                        let table_key = normalize_table_key(table_schema.as_deref(), &table_name);
                        let table_idx =
                            *table_index.entry(table_key.clone()).or_insert_with(|| {
                                let node = Node::Table {
                                    schema: table_schema.clone(),
                                    name: table_name.clone(),
                                    explicit: false,
                                    system: is_system(table_schema.as_deref(), &table_name),
                                    location: None,
                                    columns: Box::new(vec![]),
                                    partition_by: None,
                                    distribute_by: None,
                                    tablespace: None,
                                    temporary: false,
                                    unlogged: false,
                                    ddl_source: None,
                                };
                                graph.add_node(node)
                            });
                        graph.add_edge(
                            idx,
                            table_idx,
                            Edge::IndexesTable {
                                location: SourceLocation {
                                    file: file_arc.clone(),
                                    line: info.start_line,
                                },
                            },
                        );
                    }
                    Statement::CreateGlobalIndex(gi) => {
                        let idx_name = gi
                            .name
                            .as_ref()
                            .map(|n| n.last().cloned().unwrap_or_default().to_string());
                        let (table_schema, table_name) = split_object_name(&gi.table);
                        let index_columns: Vec<String> =
                            gi.columns.iter().map(|c| c.name.clone()).collect();
                        let where_clause = gi
                            .where_clause
                            .as_ref()
                            .map(crate::graph::format::format_expr);
                        let index_node = Node::Index {
                            name: idx_name,
                            table_schema: table_schema.clone(),
                            table_name: table_name.clone(),
                            unique: gi.unique,
                            global: true,
                            index_method: gi.using_method.clone(),
                            columns: index_columns,
                            tablespace: gi.tablespace.clone(),
                            where_clause,
                            constraint: None,
                            location: SourceLocation {
                                file: file_arc.clone(),
                                line: info.start_line,
                            },
                        };
                        let idx = graph.add_node(index_node);
                        let table_key = normalize_table_key(table_schema.as_deref(), &table_name);
                        let table_idx =
                            *table_index.entry(table_key.clone()).or_insert_with(|| {
                                let node = Node::Table {
                                    schema: table_schema.clone(),
                                    name: table_name.clone(),
                                    explicit: false,
                                    system: is_system(table_schema.as_deref(), &table_name),
                                    location: None,
                                    columns: Box::new(vec![]),
                                    partition_by: None,
                                    distribute_by: None,
                                    tablespace: None,
                                    temporary: false,
                                    unlogged: false,
                                    ddl_source: None,
                                };
                                graph.add_node(node)
                            });
                        graph.add_edge(
                            idx,
                            table_idx,
                            Edge::IndexesTable {
                                location: SourceLocation {
                                    file: file_arc.clone(),
                                    line: info.start_line,
                                },
                            },
                        );
                    }
                    Statement::AlterTable(alt) => {
                        let (table_schema, table_name) = split_object_name(&alt.name);
                        let table_key = normalize_table_key(table_schema.as_deref(), &table_name);
                        let table_idx =
                            *table_index.entry(table_key.clone()).or_insert_with(|| {
                                let node = Node::Table {
                                    schema: table_schema.clone(),
                                    name: table_name.clone(),
                                    explicit: false,
                                    system: is_system(table_schema.as_deref(), &table_name),
                                    location: None,
                                    columns: Box::new(vec![]),
                                    partition_by: None,
                                    distribute_by: None,
                                    tablespace: None,
                                    temporary: false,
                                    unlogged: false,
                                    ddl_source: None,
                                };
                                graph.add_node(node)
                            });

                        for action in &alt.actions {
                            if let AlterTableAction::AddConstraint { name, constraint } = action {
                                match constraint {
                                    TableConstraint::PrimaryKey {
                                        columns,
                                        using_index,
                                    } => {
                                        let index_name = name
                                            .clone()
                                            .or_else(|| using_index.clone())
                                            .unwrap_or_else(|| format!("{}_pkey", table_name));
                                        let index_node = Node::Index {
                                            name: Some(index_name),
                                            table_schema: table_schema.clone(),
                                            table_name: table_name.clone(),
                                            unique: true,
                                            global: false,
                                            index_method: Some("btree".to_string()),
                                            columns: columns.clone(),
                                            tablespace: None,
                                            where_clause: None,
                                            constraint: Some(IndexConstraint::PrimaryKey),
                                            location: SourceLocation {
                                                file: file_arc.clone(),
                                                line: info.start_line,
                                            },
                                        };
                                        let idx = graph.add_node(index_node);
                                        graph.add_edge(
                                            idx,
                                            table_idx,
                                            Edge::IndexesTable {
                                                location: SourceLocation {
                                                    file: file_arc.clone(),
                                                    line: info.start_line,
                                                },
                                            },
                                        );

                                        if let Node::Table {
                                            columns: ref mut tbl_cols,
                                            ..
                                        } = &mut graph[table_idx]
                                        {
                                            for col in tbl_cols.iter_mut() {
                                                if columns.contains(&col.name) {
                                                    col.is_primary_key = true;
                                                }
                                            }
                                        }
                                    }
                                    TableConstraint::Unique {
                                        columns,
                                        using_index,
                                        ..
                                    } => {
                                        let index_name = name
                                            .clone()
                                            .or_else(|| using_index.clone())
                                            .unwrap_or_else(|| {
                                                format!(
                                                    "{}_unique_{}",
                                                    table_name,
                                                    columns.join("_")
                                                )
                                            });
                                        let index_node = Node::Index {
                                            name: Some(index_name),
                                            table_schema: table_schema.clone(),
                                            table_name: table_name.clone(),
                                            unique: true,
                                            global: false,
                                            index_method: Some("btree".to_string()),
                                            columns: columns.clone(),
                                            tablespace: None,
                                            where_clause: None,
                                            constraint: Some(IndexConstraint::Unique),
                                            location: SourceLocation {
                                                file: file_arc.clone(),
                                                line: info.start_line,
                                            },
                                        };
                                        let idx = graph.add_node(index_node);
                                        graph.add_edge(
                                            idx,
                                            table_idx,
                                            Edge::IndexesTable {
                                                location: SourceLocation {
                                                    file: file_arc.clone(),
                                                    line: info.start_line,
                                                },
                                            },
                                        );
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    Statement::CreateTable(t) => {
                        let (schema, name) = split_object_name(&t.name);

                        let columns: Vec<ColumnSummary> = t
                            .columns
                            .iter()
                            .map(|c| {
                                let is_pk = c
                                    .constraints
                                    .iter()
                                    .any(|cc| matches!(cc, ColumnConstraint::PrimaryKey));
                                let nullable = !c
                                    .constraints
                                    .iter()
                                    .any(|cc| matches!(cc, ColumnConstraint::NotNull));
                                let default_value = c.constraints.iter().find_map(|cc| {
                                    if let ColumnConstraint::Default(expr) = cc {
                                        Some(crate::graph::format::format_expr(expr))
                                    } else {
                                        None
                                    }
                                });
                                ColumnSummary {
                                    name: c.name.clone(),
                                    data_type: crate::graph::format::format_data_type(&c.data_type),
                                    nullable,
                                    is_primary_key: is_pk,
                                    default_value,
                                    comment: c.comment.clone(),
                                }
                            })
                            .collect();

                        let partition_by = t.partition_by.as_ref().map(|p| match p {
                            ogsql_parser::ast::PartitionClause::Range {
                                columns,
                                partitions,
                                ..
                            } => PartitionInfo::Range {
                                columns: columns.iter().map(|c| c.join(".")).collect(),
                                partitions: partitions.iter().map(|pd| pd.name.clone()).collect(),
                            },
                            ogsql_parser::ast::PartitionClause::List {
                                columns,
                                partitions,
                                ..
                            } => PartitionInfo::List {
                                columns: columns.iter().map(|c| c.join(".")).collect(),
                                partitions: partitions.iter().map(|pd| pd.name.clone()).collect(),
                            },
                            ogsql_parser::ast::PartitionClause::Hash {
                                columns,
                                partitions_count,
                                ..
                            } => PartitionInfo::Hash {
                                columns: columns.iter().map(|c| c.join(".")).collect(),
                                partitions_count: *partitions_count,
                            },
                        });

                        let distribute_by = t.distribute_by.as_ref().map(|d| match d {
                            ogsql_parser::ast::DistributeClause::Hash { columns } => {
                                DistributeInfo::Hash {
                                    columns: columns.clone(),
                                }
                            }
                            ogsql_parser::ast::DistributeClause::Replication => {
                                DistributeInfo::Replication
                            }
                            ogsql_parser::ast::DistributeClause::RoundRobin { columns } => {
                                DistributeInfo::RoundRobin {
                                    columns: columns.clone(),
                                }
                            }
                            ogsql_parser::ast::DistributeClause::Modulo { columns } => {
                                DistributeInfo::Modulo {
                                    columns: columns.clone(),
                                }
                            }
                        });

                        let table_node = Node::Table {
                            schema: schema.clone(),
                            name: name.clone(),
                            explicit: true,
                            system: is_system(schema.as_deref(), &name),
                            location: Some(SourceLocation {
                                file: file_arc.clone(),
                                line: info.start_line,
                            }),
                            columns: Box::new(columns),
                            partition_by: partition_by.map(Box::new),
                            distribute_by: distribute_by.map(Box::new),
                            tablespace: t.tablespace.clone(),
                            temporary: t.temporary,
                            unlogged: t.unlogged,
                            ddl_source: None,
                        };

                        let key = normalize_table_key(schema.as_deref(), &name);
                        let idx = *table_index
                            .entry(key.clone())
                            .or_insert_with(|| graph.add_node(table_node));

                        // If the node already existed (implicit), replace its data
                        if let Node::Table {
                            location,
                            explicit,
                            system,
                            columns,
                            partition_by,
                            distribute_by,
                            tablespace,
                            temporary,
                            unlogged,
                            ..
                        } = &mut graph[idx]
                        {
                            *explicit = true;
                            *system = is_system(schema.as_deref(), &name);
                            *location = Some(SourceLocation {
                                file: file_arc.clone(),
                                line: info.start_line,
                            });
                            let new_cols: Vec<ColumnSummary> = t
                                .columns
                                .iter()
                                .map(|c| {
                                    let mut nullable = true;
                                    let mut is_pk = false;
                                    let mut default_value = None;
                                    for constraint in &c.constraints {
                                        match constraint {
                                            ColumnConstraint::NotNull => nullable = false,
                                            ColumnConstraint::PrimaryKey => is_pk = true,
                                            ColumnConstraint::Default(expr) => {
                                                default_value =
                                                    Some(crate::graph::format::format_expr(expr))
                                            }
                                            _ => {}
                                        }
                                    }
                                    ColumnSummary {
                                        name: c.name.clone(),
                                        data_type: crate::graph::format::format_data_type(
                                            &c.data_type,
                                        ),
                                        nullable,
                                        is_primary_key: is_pk,
                                        default_value,
                                        comment: c.comment.clone(),
                                    }
                                })
                                .collect();
                            if columns.is_empty() {
                                **columns = new_cols;
                            }
                            if partition_by.is_none() && t.partition_by.is_some() {
                                *partition_by = t.partition_by.as_ref().map(|p| {
                                    Box::new(match p {
                                        ogsql_parser::ast::PartitionClause::Range {
                                            columns,
                                            partitions,
                                            ..
                                        } => PartitionInfo::Range {
                                            columns: columns.iter().map(|c| c.join(".")).collect(),
                                            partitions: partitions
                                                .iter()
                                                .map(|pd| pd.name.clone())
                                                .collect(),
                                        },
                                        ogsql_parser::ast::PartitionClause::List {
                                            columns,
                                            partitions,
                                            ..
                                        } => PartitionInfo::List {
                                            columns: columns.iter().map(|c| c.join(".")).collect(),
                                            partitions: partitions
                                                .iter()
                                                .map(|pd| pd.name.clone())
                                                .collect(),
                                        },
                                        ogsql_parser::ast::PartitionClause::Hash {
                                            columns,
                                            partitions_count,
                                            ..
                                        } => PartitionInfo::Hash {
                                            columns: columns.iter().map(|c| c.join(".")).collect(),
                                            partitions_count: *partitions_count,
                                        },
                                    })
                                });
                            }
                            if distribute_by.is_none() && t.distribute_by.is_some() {
                                *distribute_by = t.distribute_by.as_ref().map(|d| {
                                    Box::new(match d {
                                        ogsql_parser::ast::DistributeClause::Hash { columns } => {
                                            DistributeInfo::Hash {
                                                columns: columns.clone(),
                                            }
                                        }
                                        ogsql_parser::ast::DistributeClause::Replication => {
                                            DistributeInfo::Replication
                                        }
                                        ogsql_parser::ast::DistributeClause::RoundRobin {
                                            columns,
                                        } => DistributeInfo::RoundRobin {
                                            columns: columns.clone(),
                                        },
                                        ogsql_parser::ast::DistributeClause::Modulo { columns } => {
                                            DistributeInfo::Modulo {
                                                columns: columns.clone(),
                                            }
                                        }
                                    })
                                });
                            }
                            if tablespace.is_none() {
                                *tablespace = t.tablespace.clone();
                            }
                            *temporary = t.temporary;
                            *unlogged = t.unlogged;
                        }

                        for constraint in &t.constraints {
                            match constraint {
                                TableConstraint::PrimaryKey {
                                    columns,
                                    using_index,
                                } => {
                                    let index_name = using_index.clone().unwrap_or_else(|| {
                                        format!("{}_{}_pkey", name, columns.join("_"))
                                    });
                                    let idx_node = Node::Index {
                                        name: Some(index_name),
                                        table_schema: schema.clone(),
                                        table_name: name.clone(),
                                        unique: true,
                                        global: false,
                                        index_method: Some("btree".to_string()),
                                        columns: columns.clone(),
                                        tablespace: None,
                                        where_clause: None,
                                        constraint: Some(IndexConstraint::PrimaryKey),
                                        location: SourceLocation {
                                            file: file_arc.clone(),
                                            line: info.start_line,
                                        },
                                    };
                                    let new_idx = graph.add_node(idx_node);
                                    graph.add_edge(
                                        new_idx,
                                        idx,
                                        Edge::IndexesTable {
                                            location: SourceLocation {
                                                file: file_arc.clone(),
                                                line: info.start_line,
                                            },
                                        },
                                    );
                                    if let Node::Table {
                                        columns: ref mut tbl_cols,
                                        ..
                                    } = &mut graph[idx]
                                    {
                                        for col in tbl_cols.iter_mut() {
                                            if columns.contains(&col.name) {
                                                col.is_primary_key = true;
                                            }
                                        }
                                    }
                                }
                                TableConstraint::Unique {
                                    columns,
                                    using_index,
                                    ..
                                } => {
                                    let index_name = using_index.clone().unwrap_or_else(|| {
                                        format!("{}_{}_unique", name, columns.join("_"))
                                    });
                                    let idx_node = Node::Index {
                                        name: Some(index_name),
                                        table_schema: schema.clone(),
                                        table_name: name.clone(),
                                        unique: true,
                                        global: false,
                                        index_method: Some("btree".to_string()),
                                        columns: columns.clone(),
                                        tablespace: None,
                                        where_clause: None,
                                        constraint: Some(IndexConstraint::Unique),
                                        location: SourceLocation {
                                            file: file_arc.clone(),
                                            line: info.start_line,
                                        },
                                    };
                                    let new_idx = graph.add_node(idx_node);
                                    graph.add_edge(
                                        new_idx,
                                        idx,
                                        Edge::IndexesTable {
                                            location: SourceLocation {
                                                file: file_arc.clone(),
                                                line: info.start_line,
                                            },
                                        },
                                    );
                                }
                                _ => {}
                            }
                        }

                        if schema.is_some() {
                            table_index.entry(name.to_lowercase()).or_insert(idx);
                        }
                    }
                    Statement::CreateMaterializedView(v) => {
                        let (schema, name) = split_object_name(&v.name);
                        let columns = extract_view_columns(&v.columns, &v.query.targets);
                        let ddl_source = Some(Box::new(info.sql_text.clone()));
                        let mview_node = Node::MaterializedView {
                            schema: schema.clone(),
                            name: name.clone(),
                            location: SourceLocation {
                                file: file_arc.clone(),
                                line: info.start_line,
                            },
                            columns: Box::new(columns),
                            ddl_source,
                        };
                        let mview_idx = graph.add_node(mview_node);

                        let mview_key = normalize_table_key(schema.as_deref(), &name);
                        table_index.entry(mview_key).or_insert(mview_idx);
                        if schema.is_some() {
                            table_index.entry(name.to_lowercase()).or_insert(mview_idx);
                        }

                        let mut extractor = crate::parser::TableAccessExtractor::new();
                        let wrapped = Statement::Select(ogsql_parser::ast::Spanned {
                            node: v.query.as_ref().clone(),
                            span: None,
                        });
                        walk_statement(&mut extractor, &wrapped);

                        for access in &extractor.accesses {
                            let key = normalize_table_key(access.schema.as_deref(), &access.name);
                            let table_idx = *table_index.entry(key.clone()).or_insert_with(|| {
                                let node = Node::Table {
                                    schema: access.schema.clone(),
                                    name: access.name.clone(),
                                    explicit: false,
                                    system: is_system(access.schema.as_deref(), &access.name),
                                    location: None,
                                    columns: Box::new(vec![]),
                                    partition_by: None,
                                    distribute_by: None,
                                    tablespace: None,
                                    temporary: false,
                                    unlogged: false,
                                    ddl_source: None,
                                };
                                graph.add_node(node)
                            });
                            if access.schema.is_some() {
                                table_index
                                    .entry(access.name.to_lowercase())
                                    .or_insert(table_idx);
                            }
                            graph.add_edge(
                                mview_idx,
                                table_idx,
                                Edge::DependsOn {
                                    location: SourceLocation {
                                        file: file_arc.clone(),
                                        line: info.start_line,
                                    },
                                },
                            );
                        }
                    }
                    Statement::CreateSynonym(s) => {
                        let (schema, name) = split_object_name(&s.name);
                        let (target_schema, target_name) = split_object_name(&s.target);
                        let syn_node = Node::Synonym {
                            schema: schema.clone(),
                            name: name.clone(),
                            target_schema: target_schema.clone(),
                            target_name: target_name.clone(),
                            location: SourceLocation {
                                file: file_arc.clone(),
                                line: info.start_line,
                            },
                        };
                        let syn_idx = graph.add_node(syn_node);

                        let target_key =
                            normalize_table_key(target_schema.as_deref(), &target_name);
                        let target_idx = proc_index
                            .get(
                                &RoutineId::from_qualified_name(
                                    &target_key,
                                    RoutineKind::Procedure,
                                )
                                .normalized(),
                            )
                            .copied()
                            .or_else(|| {
                                proc_index
                                    .get(
                                        &RoutineId::from_qualified_name(
                                            &target_key,
                                            RoutineKind::Function,
                                        )
                                        .normalized(),
                                    )
                                    .copied()
                            })
                            .or_else(|| table_index.get(&target_key).copied())
                            .or_else(|| type_index.get(&target_key).copied())
                            .or_else(|| sequence_index.get(&target_key).copied())
                            .unwrap_or_else(|| {
                                let snippet = crate::parser::snippet::read_snippet(
                                    &file.path,
                                    info.start_line,
                                    1,
                                );
                                let (syn_schema, syn_name) = target_key
                                    .rsplit_once('.')
                                    .map(|(s, n)| (Some(s), n))
                                    .unwrap_or((None, target_key.as_str()));
                                let suffix = unresolved_creation_suffix(
                                    Some((syn_schema, None, syn_name)),
                                    snippet.as_deref(),
                                );
                                crate::parse_log::warn(
                                    &format!("{}:{}", file.path.to_string_lossy(), info.start_line),
                                    &format!(
                                        "unresolved: synonym '{}.{}' target '{}' not found{}",
                                        schema.as_deref().unwrap_or(""),
                                        name,
                                        target_key,
                                        suffix
                                    ),
                                );
                                let unresolved = Node::Unresolved {
                                    raw_expr: Box::new(target_key.clone()),
                                    context: Box::new(format!(
                                        "synonym:{}.{}",
                                        schema.as_deref().unwrap_or(""),
                                        name
                                    )),
                                };
                                graph.add_node(unresolved)
                            });
                        graph.add_edge(
                            syn_idx,
                            target_idx,
                            Edge::AliasesObject {
                                location: SourceLocation {
                                    file: file_arc.clone(),
                                    line: info.start_line,
                                },
                            },
                        );
                    }
                    Statement::CreateEvent(e) => {
                        let event_node = Node::Event {
                            name: e.name.clone(),
                            location: SourceLocation {
                                file: file_arc.clone(),
                                line: info.start_line,
                            },
                        };
                        graph.add_node(event_node);
                    }
                    Statement::Comment(comment_stmt)
                        if comment_stmt.object_type.eq_ignore_ascii_case("COLUMN") =>
                    {
                        let (schema, table, col_name) = split_comment_col_name(&comment_stmt.name);
                        let table_key = normalize_table_key(schema.as_deref(), &table);
                        deferred_column_comments.push(DeferredColumnComment {
                            table_key,
                            col_name,
                            comment: comment_stmt.comment.clone(),
                        });
                    }
                    _ => {}
                }
            }

            if !has_body {
                continue;
            }

            for (pkg_name, routine_name, kind) in &spec_decls {
                let found_in_body = body_impls
                    .iter()
                    .any(|(pn, rn)| pn == pkg_name && rn == routine_name);
                if !found_in_body {
                    let routine_id = RoutineId {
                        schema: None,
                        package: Some(pkg_name.clone()),
                        name: routine_name.clone(),
                        kind: *kind,
                    };
                    if let std::collections::hash_map::Entry::Vacant(e) =
                        proc_index.entry(routine_id.normalized())
                    {
                        let file_str = file.path.to_string_lossy().to_string();
                        crate::parse_log::warn(
                            &file_str,
                            &format!(
                                "package '{}' spec declares '{}' but body implementation could not be parsed (partial node)",
                                pkg_name, routine_name
                            ),
                        );
                        let node = match kind {
                            RoutineKind::Procedure => Node::Procedure {
                                id: routine_id.clone().normalized(),
                                location: SourceLocation {
                                    file: file_arc.clone(),
                                    line: 0,
                                },
                                partial: true,
                                body_sql: Vec::new(),
                            },
                            RoutineKind::Function => Node::Function {
                                id: routine_id.clone().normalized(),
                                location: SourceLocation {
                                    file: file_arc.clone(),
                                    line: 0,
                                },
                                partial: true,
                                body_sql: Vec::new(),
                            },
                        };
                        let idx = graph.add_node(node);
                        e.insert(idx);

                        if let Some(&pkg_idx) = package_index.get(pkg_name) {
                            graph.add_edge(pkg_idx, idx, Edge::ContainsRoutine);
                        }
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn create_package_nodes(
        pkg_name: &ogsql_parser::ast::ObjectName,
        pkg_items: &[PackageItem],
        is_spec: bool,
        start_line: usize,
        file_path: &Arc<PathBuf>,
        graph: &mut CodeGraph,
        proc_index: &mut HashMap<RoutineId, petgraph::graph::NodeIndex>,
        package_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
    ) {
        let pkg_name_part = pkg_name.last().cloned().unwrap_or_default().to_lowercase();
        let schema_part: Option<String> = if pkg_name.len() > 1 {
            Some(
                pkg_name[..pkg_name.len() - 1]
                    .iter()
                    .map(|i| i.to_string().to_lowercase())
                    .collect::<Vec<_>>()
                    .join("."),
            )
        } else {
            None
        };
        let qualified = match &schema_part {
            Some(ref s) => format!("{}.{}", s, pkg_name_part),
            None => pkg_name_part.clone(),
        };

        let pkg_idx = if is_spec {
            match package_index.get(&qualified) {
                Some(&idx) => {
                    if let Node::Package { location, .. } = &mut graph[idx] {
                        *location = SourceLocation {
                            file: file_path.clone(),
                            line: start_line,
                        };
                    }
                    idx
                }
                None => {
                    let idx = graph.add_node(Node::Package {
                        schema: schema_part.clone(),
                        name: pkg_name_part.clone(),
                        location: SourceLocation {
                            file: file_path.clone(),
                            line: start_line,
                        },
                    });
                    package_index.insert(qualified, idx);
                    idx
                }
            }
        } else {
            *package_index.entry(qualified).or_insert_with(|| {
                graph.add_node(Node::Package {
                    schema: schema_part.clone(),
                    name: pkg_name_part.clone(),
                    location: SourceLocation {
                        file: file_path.clone(),
                        line: start_line,
                    },
                })
            })
        };

        for item in pkg_items {
            let (proc_name, block, kind) = match item {
                PackageItem::Procedure(p) => (p.name.join("."), &p.block, RoutineKind::Procedure),
                PackageItem::Function(f) => (f.name.join("."), &f.block, RoutineKind::Function),
                PackageItem::Raw(_)
                | PackageItem::Variable(_)
                | PackageItem::Type(_)
                | PackageItem::Cursor(_) => continue,
            };
            let Some(_block) = block else {
                continue;
            };
            let body_sql = block
                .as_ref()
                .map(|b| {
                    crate::parser::extract_body_sql(b)
                        .into_iter()
                        .map(|sql| crate::graph::ProcedureBodySql {
                            sql_text: sql.sql_text,
                            kind: sql.kind,
                            line: sql.line,
                        })
                        .collect()
                })
                .unwrap_or_default();
            let proc_id = RoutineId {
                schema: schema_part.clone(),
                package: Some(pkg_name_part.clone()),
                name: proc_name,
                kind,
            };
            let proc_idx = proc_index.entry(proc_id.normalized()).or_insert_with(|| {
                let node = match kind {
                    RoutineKind::Procedure => Node::Procedure {
                        id: proc_id.clone().normalized(),
                        location: SourceLocation {
                            file: file_path.clone(),
                            line: start_line,
                        },
                        partial: false,
                        body_sql,
                    },
                    RoutineKind::Function => Node::Function {
                        id: proc_id.clone().normalized(),
                        location: SourceLocation {
                            file: file_path.clone(),
                            line: start_line,
                        },
                        partial: false,
                        body_sql,
                    },
                };
                graph.add_node(node)
            });
            graph.add_edge(pkg_idx, *proc_idx, Edge::ContainsRoutine);
        }
    }

    // ── Pass 2: Create all SQL edges + table access ─────────────
    // Merged from: collect_call_edges + collect_table_access (2 loops → 1 loop)

    fn create_sql_edges(
        files: &[ParsedFile],
        graph: &mut CodeGraph,
        proc_index: &mut HashMap<RoutineId, petgraph::graph::NodeIndex>,
        table_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        type_index: &HashMap<String, petgraph::graph::NodeIndex>,
        builtin_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        enable_column_lineage: bool,
    ) {
        let mut all_edges = Vec::new();

        let known_types: HashSet<String> = type_index.keys().cloned().collect();

        // Index package SPEC items by lowercased qualified package name so a
        // package BODY can inherit the spec's public Variable/Type declarations
        // into its call-edge extraction scope. Spec and body are parsed as
        // independent statements; without this linkage, a spec-declared symbol
        // (e.g. `vchar_array`) used in a body procedure is misread as a call.
        let mut spec_items_by_pkg: HashMap<String, &[PackageItem]> = HashMap::new();
        for file in files {
            for info in &file.statements {
                if let Statement::CreatePackage(pkg) = &info.statement {
                    spec_items_by_pkg.insert(pkg_qualified_key(&pkg.name), &pkg.items);
                }
            }
        }

        for file in files {
            let file_sw = std::time::Instant::now();
            let file_arc: Arc<PathBuf> = Arc::new(file.path.clone());
            for info in &file.statements {
                let mut extractor = CallExtractor::new(file_arc.clone(), known_types.clone());
                match &info.statement {
                    Statement::CreatePackage(pkg) => {
                        Self::collect_package_call_edges(
                            &pkg.name,
                            &pkg.items,
                            &[],
                            &mut extractor,
                        );
                    }
                    Statement::CreatePackageBody(pkg) => {
                        let inherited: &[PackageItem] = spec_items_by_pkg
                            .get(&pkg_qualified_key(&pkg.name))
                            .copied()
                            .unwrap_or(&[]);
                        Self::collect_package_call_edges(
                            &pkg.name,
                            &pkg.items,
                            inherited,
                            &mut extractor,
                        );
                    }
                    _ => {
                        walk_statement(&mut extractor, &info.statement);
                    }
                }
                all_edges.extend(extractor.edges);

                match &info.statement {
                    Statement::CreateProcedure(p) => {
                        let proc_id = RoutineId::from_object_name(&p.name, RoutineKind::Procedure);
                        if let Some(&proc_idx) = proc_index.get(&proc_id.normalized()) {
                            Self::collect_table_access_from_statements(
                                std::slice::from_ref(info),
                                &file_arc,
                                proc_idx,
                                proc_id.schema.as_deref(),
                                graph,
                                table_index,
                                enable_column_lineage,
                            );
                        }
                    }
                    Statement::CreateFunction(f) => {
                        let proc_id = RoutineId::from_object_name(&f.name, RoutineKind::Function);
                        if let Some(&proc_idx) = proc_index.get(&proc_id.normalized()) {
                            Self::collect_table_access_from_statements(
                                std::slice::from_ref(info),
                                &file_arc,
                                proc_idx,
                                proc_id.schema.as_deref(),
                                graph,
                                table_index,
                                enable_column_lineage,
                            );
                        }
                    }
                    Statement::CreatePackage(pkg) => {
                        Self::add_package_table_access(
                            &pkg.name,
                            &pkg.items,
                            info,
                            &file_arc,
                            proc_index,
                            graph,
                            table_index,
                            enable_column_lineage,
                        );
                    }
                    Statement::CreatePackageBody(pkg) => {
                        Self::add_package_table_access(
                            &pkg.name,
                            &pkg.items,
                            info,
                            &file_arc,
                            proc_index,
                            graph,
                            table_index,
                            enable_column_lineage,
                        );
                    }
                    _ => {}
                }
            }
            let extract_elapsed = file_sw.elapsed();
            if extract_elapsed > crate::parse_log::SLOW_FILE_THRESHOLD {
                crate::parse_log::warn(
                    &file.path.display().to_string(),
                    &format!(
                        "slow extract: call-edge + table-access extraction took {:.2}s ({} \
                         statements) — inspect for pathological EXECUTE IMMEDIATE / dynamic-SQL",
                        extract_elapsed.as_secs_f64(),
                        file.statements.len()
                    ),
                );
            }
        }

        Self::create_edges(&all_edges, graph, proc_index, builtin_index);
    }

    fn create_object_ref_edges(
        files: &[ParsedFile],
        graph: &mut CodeGraph,
        proc_index: &HashMap<RoutineId, petgraph::graph::NodeIndex>,
        type_index: &HashMap<String, petgraph::graph::NodeIndex>,
        sequence_index: &HashMap<String, petgraph::graph::NodeIndex>,
    ) {
        for file in files {
            let file_arc: Arc<PathBuf> = Arc::new(file.path.clone());
            for info in &file.statements {
                let known_types: HashSet<String> = type_index.keys().cloned().collect();
                let mut extractor = TypeSequenceRefExtractor::new(known_types);

                match &info.statement {
                    Statement::CreateProcedure(p) => {
                        let proc_id = RoutineId::from_object_name(&p.name, RoutineKind::Procedure);
                        extractor.current_context = proc_id.to_string();
                        if let Some(ref block) = p.block {
                            walk_pl_block(&mut extractor, block);
                        }
                        if let Some(&proc_idx) = proc_index.get(&proc_id.normalized()) {
                            for param in &p.parameters {
                                if let Some(&type_idx) =
                                    type_index.get(&param.data_type.to_lowercase())
                                {
                                    graph.add_edge(
                                        proc_idx,
                                        type_idx,
                                        Edge::ReferencesType {
                                            location: SourceLocation {
                                                file: file_arc.clone(),
                                                line: info.start_line,
                                            },
                                        },
                                    );
                                }
                            }
                            for type_ref in &extractor.type_refs {
                                if let Some(&type_idx) =
                                    type_index.get(&type_ref.type_name.to_lowercase())
                                {
                                    graph.add_edge(
                                        proc_idx,
                                        type_idx,
                                        Edge::ReferencesType {
                                            location: SourceLocation {
                                                file: file_arc.clone(),
                                                line: info.start_line,
                                            },
                                        },
                                    );
                                }
                            }
                            for seq_ref in &extractor.sequence_refs {
                                if let Some(&seq_idx) =
                                    sequence_index.get(&seq_ref.sequence_name.to_lowercase())
                                {
                                    graph.add_edge(
                                        proc_idx,
                                        seq_idx,
                                        Edge::UsesSequence {
                                            location: SourceLocation {
                                                file: file_arc.clone(),
                                                line: info.start_line,
                                            },
                                        },
                                    );
                                }
                            }
                        }
                    }
                    Statement::CreateFunction(f) => {
                        let proc_id = RoutineId::from_object_name(&f.name, RoutineKind::Function);
                        extractor.current_context = proc_id.to_string();
                        if let Some(ref block) = f.block {
                            walk_pl_block(&mut extractor, block);
                        }
                        if let Some(&proc_idx) = proc_index.get(&proc_id.normalized()) {
                            for param in &f.parameters {
                                if let Some(&type_idx) =
                                    type_index.get(&param.data_type.to_lowercase())
                                {
                                    graph.add_edge(
                                        proc_idx,
                                        type_idx,
                                        Edge::ReferencesType {
                                            location: SourceLocation {
                                                file: file_arc.clone(),
                                                line: info.start_line,
                                            },
                                        },
                                    );
                                }
                            }
                            if let Some(ref ret_type) = f.return_type {
                                if let Some(&type_idx) = type_index.get(&ret_type.to_lowercase()) {
                                    graph.add_edge(
                                        proc_idx,
                                        type_idx,
                                        Edge::ReferencesType {
                                            location: SourceLocation {
                                                file: file_arc.clone(),
                                                line: info.start_line,
                                            },
                                        },
                                    );
                                }
                            }
                            for type_ref in &extractor.type_refs {
                                if let Some(&type_idx) =
                                    type_index.get(&type_ref.type_name.to_lowercase())
                                {
                                    graph.add_edge(
                                        proc_idx,
                                        type_idx,
                                        Edge::ReferencesType {
                                            location: SourceLocation {
                                                file: file_arc.clone(),
                                                line: info.start_line,
                                            },
                                        },
                                    );
                                }
                            }
                            for seq_ref in &extractor.sequence_refs {
                                if let Some(&seq_idx) =
                                    sequence_index.get(&seq_ref.sequence_name.to_lowercase())
                                {
                                    graph.add_edge(
                                        proc_idx,
                                        seq_idx,
                                        Edge::UsesSequence {
                                            location: SourceLocation {
                                                file: file_arc.clone(),
                                                line: info.start_line,
                                            },
                                        },
                                    );
                                }
                            }
                        }
                    }
                    Statement::CreatePackage(pkg) => {
                        Self::collect_package_object_ref_edges(
                            &pkg.name,
                            &pkg.items,
                            info,
                            &file_arc,
                            proc_index,
                            type_index,
                            sequence_index,
                            graph,
                        );
                    }
                    Statement::CreatePackageBody(pkg) => {
                        Self::collect_package_object_ref_edges(
                            &pkg.name,
                            &pkg.items,
                            info,
                            &file_arc,
                            proc_index,
                            type_index,
                            sequence_index,
                            graph,
                        );
                    }
                    _ => {}
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_package_object_ref_edges(
        pkg_name: &ogsql_parser::ast::ObjectName,
        pkg_items: &[PackageItem],
        info: &ogsql_parser::StatementInfo,
        file_path: &Arc<PathBuf>,
        proc_index: &HashMap<RoutineId, petgraph::graph::NodeIndex>,
        type_index: &HashMap<String, petgraph::graph::NodeIndex>,
        sequence_index: &HashMap<String, petgraph::graph::NodeIndex>,
        graph: &mut CodeGraph,
    ) {
        let pkg_name_part = pkg_name.last().cloned().unwrap_or_default().to_string();
        let schema_part: Option<String> = if pkg_name.len() > 1 {
            Some(pkg_name[..pkg_name.len() - 1].join("."))
        } else {
            None
        };
        let known_types: HashSet<String> = type_index.keys().cloned().collect();

        for item in pkg_items {
            let (proc_name, block, kind) = match item {
                PackageItem::Procedure(p) => (p.name.join("."), &p.block, RoutineKind::Procedure),
                PackageItem::Function(f) => (f.name.join("."), &f.block, RoutineKind::Function),
                PackageItem::Raw(_)
                | PackageItem::Variable(_)
                | PackageItem::Type(_)
                | PackageItem::Cursor(_) => continue,
            };
            let proc_id = RoutineId {
                schema: schema_part.clone(),
                package: Some(pkg_name_part.clone()),
                name: proc_name,
                kind,
            };
            let Some(proc_idx) = proc_index.get(&proc_id.normalized()).copied() else {
                continue;
            };
            let Some(ref block) = block else {
                continue;
            };

            let mut extractor = TypeSequenceRefExtractor::new(known_types.clone());
            extractor.current_context = proc_id.to_string();
            walk_pl_block(&mut extractor, block);

            for type_ref in &extractor.type_refs {
                if let Some(&type_idx) = type_index.get(&type_ref.type_name.to_lowercase()) {
                    graph.add_edge(
                        proc_idx,
                        type_idx,
                        Edge::ReferencesType {
                            location: SourceLocation {
                                file: file_path.clone(),
                                line: info.start_line,
                            },
                        },
                    );
                }
            }
            for seq_ref in &extractor.sequence_refs {
                if let Some(&seq_idx) = sequence_index.get(&seq_ref.sequence_name.to_lowercase()) {
                    graph.add_edge(
                        proc_idx,
                        seq_idx,
                        Edge::UsesSequence {
                            location: SourceLocation {
                                file: file_path.clone(),
                                line: info.start_line,
                            },
                        },
                    );
                }
            }
        }
    }

    fn collect_package_call_edges(
        pkg_name: &ogsql_parser::ast::ObjectName,
        pkg_items: &[PackageItem],
        inherited_items: &[PackageItem],
        extractor: &mut CallExtractor,
    ) {
        let pkg_name_part = pkg_name.last().cloned().unwrap_or_default().to_string();
        let schema_part: Option<String> = if pkg_name.len() > 1 {
            Some(pkg_name[..pkg_name.len() - 1].join("."))
        } else {
            None
        };

        // Pre-pass: package-level Variable/Type names are visible to every
        // routine in the package. For a BODY, `inherited_items` carries the
        // matching SPEC's public Variable/Type declarations. Types are
        // registered once (sticky); variables must be re-injected per routine
        // because begin_routine_scope clears.
        let pkg_var_names: Vec<String> = pkg_items
            .iter()
            .chain(inherited_items.iter())
            .filter_map(|item| match item {
                PackageItem::Variable(v) => Some(v.name.to_lowercase()),
                _ => None,
            })
            .collect();
        for item in pkg_items.iter().chain(inherited_items.iter()) {
            if let PackageItem::Type(t) = item {
                extractor.register_type_name(crate::parser::pl_type_decl_name(t));
            }
        }

        for item in pkg_items {
            match item {
                PackageItem::Procedure(p) => {
                    if let Some(ref block) = p.block {
                        extractor.begin_routine_scope(&p.parameters);
                        extractor.extend_local_scope(pkg_var_names.iter().cloned());
                        extractor.current_procedure = Some(RoutineId {
                            schema: schema_part.clone(),
                            package: Some(pkg_name_part.clone()),
                            name: p.name.join("."),
                            kind: RoutineKind::Procedure,
                        });
                        walk_pl_block(extractor, block);
                    }
                }
                PackageItem::Function(f) => {
                    if let Some(ref block) = f.block {
                        extractor.begin_routine_scope(&f.parameters);
                        extractor.extend_local_scope(pkg_var_names.iter().cloned());
                        extractor.current_procedure = Some(RoutineId {
                            schema: schema_part.clone(),
                            package: Some(pkg_name_part.clone()),
                            name: f.name.join("."),
                            kind: RoutineKind::Function,
                        });
                        walk_pl_block(extractor, block);
                    }
                }
                PackageItem::Raw(_)
                | PackageItem::Variable(_)
                | PackageItem::Type(_)
                | PackageItem::Cursor(_) => {}
            }
        }
    }

    /// Find an existing `BuiltinFunction` node by lowercased name, or create a new one.
    ///
    /// Shared across the SQL-proc, XML-mapper, Java, and JSP paths so that the same
    /// builtin called from multiple sources collapses to a single node (dedup key:
    /// lowercased name, matching `NodeKey::BuiltinFunction`).
    fn find_or_create_builtin_node(
        graph: &mut CodeGraph,
        builtin_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        name: &str,
        meta: &ogsql_parser::ast::BuiltinFuncMeta,
        location: SourceLocation,
    ) -> petgraph::graph::NodeIndex {
        let name_lower = name.to_lowercase();
        let name_display = if name_lower != *name {
            format!("{} (raw={})", name_lower, name)
        } else {
            name_lower.clone()
        };
        let loc_file = location.file.display().to_string();
        let loc_line = location.line;
        if let Some(&idx) = builtin_index.get(&name_lower) {
            static REUSE_COUNTER: std::sync::atomic::AtomicU64 =
                std::sync::atomic::AtomicU64::new(0);
            let n = REUSE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            crate::parse_log::info(
                "builtin",
                &format!(
                    "[#{:<4} REUSE] {:>35} | domain={:>15} | {}:{}",
                    n, name_display, meta.domain, loc_file, loc_line,
                ),
            );
            return idx;
        }
        let idx = graph.add_node(Node::BuiltinFunction {
            name: name.to_string(),
            category: meta.category.clone(),
            domain: meta.domain.clone(),
            location,
        });
        builtin_index.insert(name_lower, idx);
        static CREATE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = CREATE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        crate::parse_log::info(
            "builtin",
            &format!(
                "[#{:<4} NEW  ] {:>35} | domain={:>15} category={:>12} | {}:{}",
                n, name_display, meta.domain, meta.category, loc_file, loc_line,
            ),
        );
        idx
    }

    fn create_edges(
        edges: &[CallEdge],
        graph: &mut CodeGraph,
        proc_index: &mut HashMap<RoutineId, petgraph::graph::NodeIndex>,
        builtin_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
    ) {
        let pkg_member_lower: HashMap<(String, String), petgraph::graph::NodeIndex> = proc_index
            .iter()
            .filter_map(|(rid, &idx)| {
                rid.package
                    .as_ref()
                    .map(|pkg| ((pkg.to_lowercase(), rid.name.to_lowercase()), idx))
            })
            .collect();

        let mut seen: HashMap<(Option<String>, String), usize> = HashMap::new();

        for edge in edges {
            let caller_key = edge.caller.as_ref().map(|c| c.to_string());
            let callee_key = edge.callee_name.clone();
            let key = (caller_key.clone(), callee_key.clone());

            if seen.contains_key(&key) {
                continue;
            }
            seen.insert(key, edge.location.line);

            let caller_idx = edge.caller.as_ref().and_then(|id| {
                proc_index.get(&id.normalized()).copied().or_else(|| {
                    let alt_kind = match id.kind {
                        RoutineKind::Procedure => RoutineKind::Function,
                        RoutineKind::Function => RoutineKind::Procedure,
                    };
                    let alt_id = RoutineId {
                        schema: id.schema.clone(),
                        package: id.package.clone(),
                        name: id.name.clone(),
                        kind: alt_kind,
                    };
                    proc_index.get(&alt_id.normalized()).copied()
                })
            });

            // ── Built-in function: create/reuse BuiltinFunction node, connect with UsesBuiltinFunction ──
            if let Some(meta) = &edge.builtin_meta {
                let builtin_idx = Self::find_or_create_builtin_node(
                    graph,
                    builtin_index,
                    &edge.callee_name,
                    meta,
                    edge.location.clone(),
                );
                if let Some(caller_idx) = caller_idx {
                    graph.add_edge(
                        caller_idx,
                        builtin_idx,
                        Edge::UsesBuiltinFunction {
                            location: edge.location.clone(),
                        },
                    );
                }
                continue;
            }

            let callee_id =
                RoutineId::from_qualified_name(&edge.callee_name, RoutineKind::Procedure);
            let callee_idx = proc_index
                .get(&callee_id.normalized())
                .copied()
                .or_else(|| {
                    let func_id =
                        RoutineId::from_qualified_name(&edge.callee_name, RoutineKind::Function);
                    proc_index.get(&func_id.normalized()).copied()
                })
                .or_else(|| {
                    if callee_id.schema.is_some() && callee_id.package.is_none() {
                        let alt_id = RoutineId {
                            schema: None,
                            package: callee_id.schema.clone(),
                            name: callee_id.name.clone(),
                            kind: RoutineKind::Procedure,
                        };
                        proc_index.get(&alt_id.normalized()).copied()
                    } else {
                        None
                    }
                })
                .or_else(|| {
                    let name_lower = callee_id.name.to_lowercase();

                    if let Some(pkg) = &callee_id.package {
                        if let Some(&idx) =
                            pkg_member_lower.get(&(pkg.to_lowercase(), name_lower.clone()))
                        {
                            return Some(idx);
                        }
                    }

                    if let Some(schema) = &callee_id.schema {
                        let pkg_part = schema
                            .rsplit_once('.')
                            .map(|(_, pkg)| pkg)
                            .unwrap_or(schema);
                        if let Some(&idx) =
                            pkg_member_lower.get(&(pkg_part.to_lowercase(), name_lower))
                        {
                            return Some(idx);
                        }
                    }

                    None
                })
                .or_else(|| {
                    // Caller-context fallback: unqualified bare name → look up in caller's package.
                    // In PL/pgSQL, an unqualified call from within a package body first resolves
                    // to a member of the same package.
                    if callee_id.schema.is_none() && callee_id.package.is_none() {
                        if let Some(caller) = &edge.caller {
                            if let Some(pkg) = &caller.package {
                                let name_lower = callee_id.name.to_lowercase();
                                if let Some(&idx) =
                                    pkg_member_lower.get(&(pkg.to_lowercase(), name_lower))
                                {
                                    return Some(idx);
                                }
                            }
                        }
                    }
                    None
                });

            match (caller_idx, callee_idx) {
                (Some(from), Some(to)) => {
                    let scope = edge_call_scope(graph, from, to);
                    let g_edge = if edge.is_dynamic {
                        Edge::DynamicCall {
                            raw_expr: edge.callee_name.clone(),
                            location: edge.location.clone(),
                        }
                    } else {
                        Edge::DirectCall {
                            scope,
                            location: edge.location.clone(),
                        }
                    };
                    graph.add_edge(from, to, g_edge);
                }
                (Some(from), None) => {
                    let snippet = crate::parser::snippet::read_snippet(
                        edge.location.file.as_ref(),
                        edge.location.line,
                        1,
                    );
                    let suffix = unresolved_creation_suffix(
                        Some((
                            callee_id.schema.as_deref(),
                            callee_id.package.as_deref(),
                            &callee_id.name,
                        )),
                        snippet.as_deref(),
                    );
                    crate::parse_log::warn(
                        &format!(
                            "{}:{}",
                            edge.location.file.to_string_lossy(),
                            edge.location.line
                        ),
                        &format!(
                            "unresolved: call target '{}' (from {}:{}) not found in parsed files{}",
                            edge.callee_name,
                            edge.location
                                .file
                                .file_name()
                                .map(|f| f.to_string_lossy().to_string())
                                .unwrap_or_default(),
                            edge.location.line,
                            suffix
                        ),
                    );
                    let unresolved_node = Node::Unresolved {
                        raw_expr: Box::new(edge.callee_name.clone()),
                        context: Box::new(
                            edge.caller
                                .as_ref()
                                .map(|c| c.to_string())
                                .unwrap_or_default(),
                        ),
                    };
                    let to = graph.add_node(unresolved_node);
                    proc_index.insert(callee_id.normalized(), to);

                    let g_edge = if edge.is_dynamic {
                        Edge::DynamicCall {
                            raw_expr: edge.callee_name.clone(),
                            location: edge.location.clone(),
                        }
                    } else {
                        Edge::DirectCall {
                            scope: CallScope::External,
                            location: edge.location.clone(),
                        }
                    };
                    graph.add_edge(from, to, g_edge);
                }
                _ => {}
            }
        }
    }

    pub(crate) fn add_ibatis_nodes_from_parsed(
        ibatis_files: &[crate::parser::ibatis_loader::IbatisParsedFile],
        graph: &mut CodeGraph,
        proc_index: &mut HashMap<RoutineId, petgraph::graph::NodeIndex>,
        mapper_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        table_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        builtin_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
    ) {
        Self::add_ibatis_nodes_from_parsed_with_source_paths(
            ibatis_files,
            graph,
            proc_index,
            mapper_index,
            table_index,
            builtin_index,
            &[],
        )
    }

    /// Like `add_ibatis_nodes_from_parsed` but accepts `source_paths` to make stored paths relative.
    pub(crate) fn add_ibatis_nodes_from_parsed_with_source_paths(
        ibatis_files: &[crate::parser::ibatis_loader::IbatisParsedFile],
        graph: &mut CodeGraph,
        proc_index: &mut HashMap<RoutineId, petgraph::graph::NodeIndex>,
        mapper_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        table_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        builtin_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        source_paths: &[PathBuf],
    ) {
        for ibatis_file in ibatis_files {
            let full_path =
                PathBuf::from(ibatis_file.result.file_path.as_deref().unwrap_or_default());
            let rel_path = source_paths
                .iter()
                .filter_map(|sp| full_path.strip_prefix(sp).ok())
                .next()
                .unwrap_or(&full_path);
            let xml_path = Arc::new(rel_path.to_path_buf());
            let namespace = &ibatis_file.result.namespace;

            for stmt in &ibatis_file.result.statements {
                let kind_label = crate::parser::ibatis_loader::statement_kind_label(&stmt.kind);
                let sql_text = {
                    let trimmed = stmt.flat_sql.trim();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_string())
                    }
                };
                let mapper_key = format!("{}.{}", namespace, stmt.id);
                let node_idx = *mapper_index.entry(mapper_key.clone()).or_insert_with(|| {
                    let node = Node::MappedStatement {
                        namespace: namespace.clone(),
                        statement_id: stmt.id.clone(),
                        kind: kind_label.to_string(),
                        xml_file: (*xml_path).clone(),
                        line: stmt.line,
                        sql: sql_text,
                    };
                    graph.add_node(node)
                });

                if let Some((statements, _errors)) = &stmt.parse_result {
                    let calls = Self::extract_calls_from_statements(statements, &xml_path);
                    let mut seen_builtin: HashSet<String> = HashSet::new();
                    for call in calls {
                        if let Some(meta) = &call.builtin_meta {
                            if !seen_builtin.insert(call.callee_name.to_lowercase()) {
                                continue;
                            }
                            let builtin_idx = Self::find_or_create_builtin_node(
                                graph,
                                builtin_index,
                                &call.callee_name,
                                meta,
                                SourceLocation {
                                    file: xml_path.clone(),
                                    line: stmt.line,
                                },
                            );
                            graph.add_edge(
                                node_idx,
                                builtin_idx,
                                Edge::UsesBuiltinFunction {
                                    location: SourceLocation {
                                        file: xml_path.clone(),
                                        line: stmt.line,
                                    },
                                },
                            );
                            continue;
                        }
                        let callee_name = call.callee_name;
                        let callee_id =
                            RoutineId::from_qualified_name(&callee_name, RoutineKind::Procedure);
                        let callee_idx = proc_index.entry(callee_id.normalized()).or_insert_with(|| {
                            let snippet = crate::parser::snippet::read_snippet(
                                xml_path.as_ref(),
                                stmt.line,
                                1,
                            );
                            let suffix = unresolved_creation_suffix(
                                Some((
                                    callee_id.schema.as_deref(),
                                    callee_id.package.as_deref(),
                                    &callee_id.name,
                                )),
                                snippet.as_deref(),
                            );
                            crate::parse_log::warn(
                                &format!("{}:{}", xml_path.to_string_lossy(), stmt.line),
                                &format!(
                                    "unresolved: mapper '{}.{}' calls '{}' not found in parsed files{}",
                                    namespace, stmt.id, callee_name, suffix
                                ),
                            );
                            let unresolved = Node::Unresolved {
                                raw_expr: Box::new(callee_name.clone()),
                                context: Box::new(format!("{}.{}", namespace, stmt.id)),
                            };
                            graph.add_node(unresolved)
                        });
                        graph.add_edge(
                            node_idx,
                            *callee_idx,
                            Edge::CallsProcedure {
                                location: SourceLocation {
                                    file: xml_path.clone(),
                                    line: stmt.line,
                                },
                            },
                        );
                    }

                    Self::collect_table_access_from_statements(
                        statements,
                        &xml_path,
                        node_idx,
                        None,
                        graph,
                        table_index,
                        false,
                    );
                }
            }
        }
    }

    pub(crate) fn add_ibatis_structured_variants(
        structured_files: &[crate::parser::ibatis_loader::IbatisStructuredFile],
        mapper_index: &HashMap<String, petgraph::graph::NodeIndex>,
        _source_paths: &[PathBuf],
    ) -> HashMap<String, Vec<String>> {
        use ogsql_parser::ibatis::{ExpandConfig, IfExpandStrategy, PlaceholderStrategy};

        let mut variant_map: HashMap<String, Vec<String>> = HashMap::new();

        let config = ExpandConfig {
            max_depth: 8,
            max_variants: 100,
            foreach_sizes: vec![1, 3],
            if_strategy: IfExpandStrategy::Both,
            placeholder: PlaceholderStrategy::PreserveInternalMarkers,
            generate_parse_results: false,
        };

        for structured_file in structured_files {
            let namespace = &structured_file.result.namespace;

            for stmt in &structured_file.result.statements {
                if !stmt.has_dynamic_elements {
                    continue;
                }

                let mapper_key = format!("{}.{}", namespace, stmt.id);
                if !mapper_index.contains_key(&mapper_key) {
                    continue;
                }

                let variants = stmt.expand_variants(&config);
                let variant_sqls: Vec<String> = variants
                    .into_iter()
                    .map(|v| v.sql)
                    .filter(|sql| !sql.trim().is_empty())
                    .collect();

                if !variant_sqls.is_empty() {
                    variant_map.insert(mapper_key, variant_sqls);
                }
            }
        }

        variant_map
    }

    pub(crate) fn add_java_nodes_from_parsed(
        java_files: &[crate::parser::java_loader::JavaParsedFile],
        graph: &mut CodeGraph,
        proc_index: &mut HashMap<RoutineId, petgraph::graph::NodeIndex>,
        mapper_index: &HashMap<String, petgraph::graph::NodeIndex>,
        table_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        builtin_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
    ) {
        Self::add_java_nodes_from_parsed_with_source_paths(
            java_files,
            graph,
            proc_index,
            mapper_index,
            table_index,
            builtin_index,
            &[],
        )
    }

    /// Like `add_java_nodes_from_parsed` but accepts `source_paths` (absolute analysis source dirs)
    /// so that stored Java file paths are relative to the matching analysis path.
    pub(crate) fn add_java_nodes_from_parsed_with_source_paths(
        java_files: &[crate::parser::java_loader::JavaParsedFile],
        graph: &mut CodeGraph,
        proc_index: &mut HashMap<RoutineId, petgraph::graph::NodeIndex>,
        mapper_index: &HashMap<String, petgraph::graph::NodeIndex>,
        table_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        builtin_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        source_paths: &[PathBuf],
    ) {
        let mut javasql_seen: HashSet<(PathBuf, usize)> = HashSet::new();
        for java_file in java_files {
            let full_path = PathBuf::from(&java_file.result.file_path);
            let rel_path = source_paths
                .iter()
                .filter_map(|sp| full_path.strip_prefix(sp).ok())
                .next()
                .unwrap_or(&full_path);
            let java_path = Arc::new(rel_path.to_path_buf());

            for extraction in &java_file.result.extractions {
                if !javasql_seen.insert(((*java_path).clone(), extraction.origin.line)) {
                    continue;
                }
                let method_label =
                    crate::parser::java_loader::extraction_method_label(&extraction.origin.method);
                let sql_text = {
                    let trimmed = extraction.sql.trim();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_string())
                    }
                };
                let node = Node::JavaSql {
                    class_name: extraction.origin.class_name.clone(),
                    method_name: extraction.origin.method_name.clone(),
                    extraction_method: method_label.to_string(),
                    java_file: (*java_path).clone(),
                    line: extraction.origin.line,
                    sql: sql_text,
                };
                let node_idx = graph.add_node(node);

                if let Some(parse_result) = &extraction.parse_result {
                    let calls =
                        Self::extract_calls_from_statements(&parse_result.statements, &java_path);
                    let mut seen_builtin: HashSet<String> = HashSet::new();
                    for call in calls {
                        if let Some(meta) = &call.builtin_meta {
                            if !seen_builtin.insert(call.callee_name.to_lowercase()) {
                                continue;
                            }
                            let builtin_idx = Self::find_or_create_builtin_node(
                                graph,
                                builtin_index,
                                &call.callee_name,
                                meta,
                                SourceLocation {
                                    file: java_path.clone(),
                                    line: extraction.origin.line,
                                },
                            );
                            graph.add_edge(
                                node_idx,
                                builtin_idx,
                                Edge::UsesBuiltinFunction {
                                    location: SourceLocation {
                                        file: java_path.clone(),
                                        line: extraction.origin.line,
                                    },
                                },
                            );
                            continue;
                        }
                        let callee_name = call.callee_name;
                        let callee_id =
                            RoutineId::from_qualified_name(&callee_name, RoutineKind::Procedure);
                        let callee_idx = proc_index.entry(callee_id.normalized()).or_insert_with(|| {
                            let snippet = crate::parser::snippet::read_snippet(
                                java_path.as_ref(),
                                extraction.origin.line,
                                1,
                            );
                            let suffix = unresolved_creation_suffix(
                                Some((
                                    callee_id.schema.as_deref(),
                                    callee_id.package.as_deref(),
                                    &callee_id.name,
                                )),
                                snippet.as_deref(),
                            );
                            crate::parse_log::warn(
                                &format!(
                                    "{}:{}",
                                    java_path.to_string_lossy(),
                                    extraction.origin.line
                                ),
                                &format!(
                                    "unresolved: Java '{}.{}' calls '{}' not found in parsed files{}",
                                    extraction.origin.class_name.as_deref().unwrap_or("?"),
                                    extraction.origin.method_name.as_deref().unwrap_or("?"),
                                    callee_name,
                                    suffix
                                ),
                            );
                            let unresolved = Node::Unresolved {
                                raw_expr: Box::new(callee_name.clone()),
                                context: Box::new(format!(
                                    "{}.{}",
                                    extraction.origin.class_name.as_deref().unwrap_or("?"),
                                    extraction.origin.method_name.as_deref().unwrap_or("?")
                                )),
                            };
                            graph.add_node(unresolved)
                        });
                        graph.add_edge(
                            node_idx,
                            *callee_idx,
                            Edge::CallsProcedure {
                                location: SourceLocation {
                                    file: java_path.clone(),
                                    line: extraction.origin.line,
                                },
                            },
                        );
                    }

                    Self::collect_table_access_from_statements(
                        &parse_result.statements,
                        &java_path,
                        node_idx,
                        None,
                        graph,
                        table_index,
                        false,
                    );
                }

                if let (Some(class), Some(method)) = (
                    &extraction.origin.class_name,
                    &extraction.origin.method_name,
                ) {
                    let mapper_key = format!("{}.{}", class, method);
                    if let Some(&mapper_idx) = mapper_index.get(&mapper_key) {
                        graph.add_edge(
                            node_idx,
                            mapper_idx,
                            Edge::InvokesMapper {
                                location: SourceLocation {
                                    file: java_path.clone(),
                                    line: extraction.origin.line,
                                },
                            },
                        );
                    }
                }
            }
        }
    }

    fn extract_calls_from_statements(
        statements: &[ogsql_parser::StatementInfo],
        file_path: &Arc<PathBuf>,
    ) -> Vec<ExtractedCall> {
        let mut calls = Vec::new();
        for info in statements {
            let mut extractor = CallExtractor::new(file_path.clone(), HashSet::new());
            walk_statement(&mut extractor, &info.statement);
            for edge in extractor.edges {
                calls.push(ExtractedCall {
                    callee_name: edge.callee_name,
                    builtin_meta: edge.builtin_meta,
                });
            }
        }
        calls
    }

    #[allow(clippy::too_many_arguments)]
    fn add_package_table_access(
        pkg_name: &ogsql_parser::ast::ObjectName,
        pkg_items: &[PackageItem],
        info: &ogsql_parser::StatementInfo,
        file_path: &Arc<PathBuf>,
        proc_index: &HashMap<RoutineId, petgraph::graph::NodeIndex>,
        graph: &mut CodeGraph,
        table_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        enable_column_lineage: bool,
    ) {
        let pkg_name_part = pkg_name.last().cloned().unwrap_or_default().to_string();
        let schema_part: Option<String> = if pkg_name.len() > 1 {
            Some(pkg_name[..pkg_name.len() - 1].join("."))
        } else {
            None
        };
        for item in pkg_items {
            let (proc_name, block, kind) = match item {
                PackageItem::Procedure(p) => (p.name.join("."), &p.block, RoutineKind::Procedure),
                PackageItem::Function(f) => (f.name.join("."), &f.block, RoutineKind::Function),
                PackageItem::Raw(_)
                | PackageItem::Variable(_)
                | PackageItem::Type(_)
                | PackageItem::Cursor(_) => continue,
            };
            let proc_id = RoutineId {
                schema: schema_part.clone(),
                package: Some(pkg_name_part.clone()),
                name: proc_name,
                kind,
            };
            if let Some(&proc_idx) = proc_index.get(&proc_id.normalized()) {
                if let Some(ref block) = block {
                    let block_stmt = ogsql_parser::StatementInfo {
                        sql_text: String::new(),
                        start_line: info.start_line,
                        start_col: 0,
                        end_line: info.end_line,
                        end_col: 0,
                        statement: Statement::AnonyBlock(ogsql_parser::ast::Spanned {
                            node: ogsql_parser::ast::AnonyBlockStatement {
                                block: block.clone(),
                            },
                            span: None,
                        }),
                    };
                    Self::collect_table_access_from_statements(
                        std::slice::from_ref(&block_stmt),
                        file_path,
                        proc_idx,
                        schema_part.as_deref(),
                        graph,
                        table_index,
                        enable_column_lineage,
                    );
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_table_access_from_statements(
        statements: &[ogsql_parser::StatementInfo],
        file_path: &Arc<PathBuf>,
        source_idx: petgraph::graph::NodeIndex,
        owner_schema: Option<&str>,
        graph: &mut CodeGraph,
        table_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        enable_column_lineage: bool,
    ) {
        let lineage_owner = crate::graph::node_display_name(&graph[source_idx]);

        for info in statements {
            let mut extractor = crate::parser::TableAccessExtractor::new();
            walk_statement(&mut extractor, &info.statement);

            let mut column_extractor = crate::parser::ColumnAccessExtractor::new();
            walk_statement(&mut column_extractor, &info.statement);
            let column_analysis = column_extractor.finish();
            let has_column_data = !column_analysis.column_refs.is_empty()
                || !column_analysis.join_conditions.is_empty()
                || !column_analysis.hard_filters.is_empty()
                || !column_analysis.enum_mappings.is_empty()
                || !column_analysis.insert_columns.is_empty()
                || !column_analysis.update_columns.is_empty()
                || !column_analysis.select_into.is_empty();

            if enable_column_lineage {
                add_column_lineage_edges(
                    graph,
                    std::slice::from_ref(info),
                    &lineage_owner,
                    &SourceLocation {
                        file: file_path.clone(),
                        line: info.start_line,
                    },
                );
            }

            for access in &extractor.accesses {
                let key = if access.schema.is_none() && owner_schema.is_some() {
                    let qualified = normalize_table_key(owner_schema, &access.name);
                    if table_index.contains_key(&qualified) {
                        qualified
                    } else {
                        normalize_table_key(None, &access.name)
                    }
                } else {
                    normalize_table_key(access.schema.as_deref(), &access.name)
                };
                let table_idx = *table_index.entry(key.clone()).or_insert_with(|| {
                    let node = Node::Table {
                        schema: access.schema.clone(),
                        name: access.name.clone(),
                        explicit: false,
                        system: is_system(access.schema.as_deref(), &access.name),
                        location: None,
                        columns: Box::new(vec![]),
                        partition_by: None,
                        distribute_by: None,
                        tablespace: None,
                        temporary: false,
                        unlogged: false,
                        ddl_source: None,
                    };
                    graph.add_node(node)
                });
                if access.schema.is_some() {
                    table_index
                        .entry(access.name.to_lowercase())
                        .or_insert(table_idx);
                }
                graph.add_edge(
                    source_idx,
                    table_idx,
                    Edge::TableAccess {
                        flow_kind: DataFlowKind::DmlAccess,
                        modes: access.modes,
                        write_kinds: access.write_kinds.clone(),
                        location: SourceLocation {
                            file: file_path.clone(),
                            line: info.start_line,
                        },
                        column_analysis: if has_column_data {
                            Some(Box::new(column_analysis.clone()))
                        } else {
                            None
                        },
                    },
                );
            }
        }
    }

    /// Add JSP page + JSP SQL nodes to the graph and link them to existing
    /// procedures/tables. Available only with the `jsp` feature.
    #[cfg(feature = "jsp")]
    pub(crate) fn add_jsp_nodes_from_parsed(
        jsp_results: &[crate::parser::jsp_loader::JspFileResult],
        ctx: &mut GraphBuildContext,
    ) {
        use crate::parser::jsp_loader::infer_kind;

        for file_result in jsp_results {
            let jsp_path: Arc<PathBuf> = Arc::new(file_result.file.clone());

            let page_node = Node::JspPage {
                path: file_result.file.clone(),
                display_name: file_result.display_name.clone(),
                line: file_result.line,
                url_pattern: None,
            };
            let page_idx = ctx.graph.add_node(page_node);

            let mut seen_sql: HashSet<String> = HashSet::new();
            for extraction in &file_result.extractions {
                let sql_hash =
                    blake3::hash(extraction.sql.as_bytes()).to_hex().as_str()[..16].to_string();
                if !seen_sql.insert(sql_hash.clone()) {
                    continue;
                }

                let kind = infer_kind(extraction);
                let parsed = extraction.parse_result.is_some();
                let sql_node = Node::JspSql {
                    sql: extraction.sql.clone(),
                    file: file_result.file.clone(),
                    line: extraction.origin.line,
                    kind,
                    parsed,
                };
                let sql_idx = ctx.graph.add_node(sql_node);

                ctx.graph.add_edge(page_idx, sql_idx, Edge::ContainsSql);

                if let Some(parse_result) = &extraction.parse_result {
                    let calls =
                        Self::extract_calls_from_statements(&parse_result.statements, &jsp_path);
                    let mut seen_builtin: HashSet<String> = HashSet::new();
                    for call in calls {
                        if let Some(meta) = &call.builtin_meta {
                            if !seen_builtin.insert(call.callee_name.to_lowercase()) {
                                continue;
                            }
                            let builtin_idx = Self::find_or_create_builtin_node(
                                &mut ctx.graph,
                                &mut ctx.builtin_index,
                                &call.callee_name,
                                meta,
                                SourceLocation {
                                    file: jsp_path.clone(),
                                    line: extraction.origin.line,
                                },
                            );
                            ctx.graph.add_edge(
                                sql_idx,
                                builtin_idx,
                                Edge::UsesBuiltinFunction {
                                    location: SourceLocation {
                                        file: jsp_path.clone(),
                                        line: extraction.origin.line,
                                    },
                                },
                            );
                            continue;
                        }
                        let callee_name = call.callee_name;
                        let callee_id =
                            RoutineId::from_qualified_name(&callee_name, RoutineKind::Procedure);
                        let callee_idx =
                                ctx.proc_index.entry(callee_id.normalized()).or_insert_with(|| {
                                    let snippet = crate::parser::snippet::read_snippet(
                                        jsp_path.as_ref(),
                                        extraction.origin.line,
                                        1,
                                    );
                                    let suffix = unresolved_creation_suffix(
                                        Some((
                                            callee_id.schema.as_deref(),
                                            callee_id.package.as_deref(),
                                            &callee_id.name,
                                        )),
                                        snippet.as_deref(),
                                    );
                                    crate::parse_log::warn(
                                        &format!(
                                            "{}:{}",
                                            jsp_path.to_string_lossy(),
                                            extraction.origin.line
                                        ),
                                        &format!(
                                            "unresolved: JSP '{}' calls '{}' not found in parsed files{}",
                                            file_result.display_name, callee_name, suffix
                                        ),
                                    );
                                let unresolved = Node::Unresolved {
                                    raw_expr: Box::new(callee_name.clone()),
                                    context: Box::new(file_result.display_name.clone()),
                                };
                                ctx.graph.add_node(unresolved)
                            });
                        ctx.graph.add_edge(
                            sql_idx,
                            *callee_idx,
                            Edge::CallsProcedure {
                                location: SourceLocation {
                                    file: jsp_path.clone(),
                                    line: extraction.origin.line,
                                },
                            },
                        );
                    }

                    Self::collect_table_access_from_statements(
                        &parse_result.statements,
                        &jsp_path,
                        sql_idx,
                        None,
                        &mut ctx.graph,
                        &mut ctx.table_index,
                        false,
                    );
                }
            }

            for err in &file_result.errors {
                crate::parse_log::warn(
                    &file_result.file.to_string_lossy(),
                    &format!("[jsp] {}", err),
                );
            }
        }
    }

    fn dedup_table_view_nodes(graph: &mut CodeGraph) {
        let mut merges: Vec<(petgraph::graph::NodeIndex, petgraph::graph::NodeIndex)> = Vec::new();

        // Phase 1: merge Table nodes into View/MaterializedView nodes
        {
            let mut view_map: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();

            for idx in graph.node_indices() {
                let (schema, name) = match &graph[idx] {
                    Node::View { schema, name, .. } => (schema.clone(), name.clone()),
                    Node::MaterializedView { schema, name, .. } => (schema.clone(), name.clone()),
                    _ => continue,
                };
                let key = normalize_table_key(schema.as_deref(), &name);
                view_map.entry(key).or_insert(idx);
                if schema.is_some() {
                    view_map.entry(name.to_lowercase()).or_insert(idx);
                }
            }

            if !view_map.is_empty() {
                for idx in graph.node_indices() {
                    let (schema, name) = match &graph[idx] {
                        Node::Table { schema, name, .. } => (schema.clone(), name.clone()),
                        _ => continue,
                    };
                    let key = normalize_table_key(schema.as_deref(), &name);
                    if let Some(&view_idx) = view_map.get(&key) {
                        merges.push((idx, view_idx));
                        continue;
                    }
                    if schema.is_none() {
                        if let Some(&view_idx) = view_map.get(&name.to_lowercase()) {
                            merges.push((idx, view_idx));
                        }
                    }
                }
            }
        }

        // Track from_idx nodes already targeted by Phase 1 to avoid
        // double-merge panics (removing a node twice causes petgraph
        // "node indices out of bounds" on the second attempt).
        let phase1_targets: std::collections::HashSet<petgraph::graph::NodeIndex> =
            merges.iter().map(|(from, _)| *from).collect();

        // Phase 2: merge bare-name Table nodes into schema-qualified Table nodes
        {
            // bare_name_lower → (schema, idx)
            let mut qualified: HashMap<String, (String, petgraph::graph::NodeIndex)> =
                HashMap::new();
            let mut bare: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();

            for idx in graph.node_indices() {
                // Skip Table nodes already scheduled for merge in Phase 1
                if phase1_targets.contains(&idx) {
                    continue;
                }
                let (schema, name) = match &graph[idx] {
                    Node::Table { schema, name, .. } => (schema.clone(), name.clone()),
                    _ => continue,
                };
                let lower = name.to_lowercase();
                match &schema {
                    Some(s) => {
                        // Keep first schema-qualified entry per bare name
                        qualified.entry(lower.clone()).or_insert((s.clone(), idx));
                    }
                    None => {
                        bare.entry(lower).or_insert(idx);
                    }
                }
            }

            // Only merge if there's exactly one schema for the bare name
            for (name_lower, &bare_idx) in &bare {
                if let Some(&(_, qual_idx)) = qualified.get(name_lower) {
                    if bare_idx != qual_idx && !phase1_targets.contains(&qual_idx) {
                        merges.push((bare_idx, qual_idx));
                    }
                }
            }
        }

        if merges.is_empty() {
            return;
        }

        // Defense-in-depth: deduplicate by from_idx — keep first occurrence
        // (Phase 1 merges are appended first, so they take priority).
        let mut seen_from = std::collections::HashSet::new();
        merges.retain(|(from, _)| seen_from.insert(*from));

        // Phase A: rewire all edges from merged nodes to their targets.
        // This must happen BEFORE any removal because petgraph's Graph uses
        // swap_remove semantics — removing a node invalidates any cached
        // NodeIndex that pointed to the old last slot. Doing all rewiring
        // first (without removals) keeps every index in `merges` valid.
        for &(from_idx, into_idx) in &merges {
            // Skip if the target is no longer present (shouldn't happen
            // before removal, but guard defensively).
            if graph.node_weight(from_idx).is_none() || graph.node_weight(into_idx).is_none() {
                continue;
            }
            let sources: Vec<_> = graph
                .neighbors_directed(from_idx, petgraph::Direction::Incoming)
                .collect();
            for src in sources {
                let weights: Vec<_> = graph
                    .edges_connecting(src, from_idx)
                    .map(|e| e.weight().clone())
                    .collect();
                for weight in weights {
                    graph.add_edge(src, into_idx, weight);
                }
            }
            let targets: Vec<_> = graph
                .neighbors_directed(from_idx, petgraph::Direction::Outgoing)
                .collect();
            for dst in targets {
                let weights: Vec<_> = graph
                    .edges_connecting(from_idx, dst)
                    .map(|e| e.weight().clone())
                    .collect();
                for weight in weights {
                    graph.add_edge(into_idx, dst, weight);
                }
            }
        }

        // Phase B: remove all merged nodes in descending index order.
        // Removing higher indices first guarantees lower indices stay
        // valid under petgraph's swap_remove semantics (the same pattern
        // used by resolve_unresolved_nodes).
        let mut to_remove: Vec<petgraph::graph::NodeIndex> =
            merges.iter().map(|(from, _)| *from).collect();
        to_remove.sort_unstable();
        to_remove.dedup();
        for idx in to_remove.into_iter().rev() {
            // Guard: the node may already be gone if a Phase 1 merge's
            // from_idx was targeted as into_idx by another iteration
            // (Phase A rewiring already handled that chain).
            if graph.node_weight(idx).is_some() {
                graph.remove_node(idx);
            }
        }
    }

    fn merge_table_access_edges(graph: &mut CodeGraph) {
        let mut merge_targets: HashMap<
            (
                petgraph::graph::NodeIndex,
                petgraph::graph::NodeIndex,
                DataFlowKind,
            ),
            Vec<petgraph::graph::EdgeIndex>,
        > = HashMap::new();
        for edge_idx in graph.edge_indices() {
            if let Edge::TableAccess { flow_kind, .. } = &graph[edge_idx] {
                let (src, dst) = graph.edge_endpoints(edge_idx).unwrap();
                merge_targets
                    .entry((src, dst, *flow_kind))
                    .or_default()
                    .push(edge_idx);
            }
        }
        let mut edges_to_remove = Vec::new();
        for (_, mut edge_indices) in merge_targets {
            if edge_indices.len() <= 1 {
                continue;
            }
            let keep = edge_indices.remove(0);
            let (mut merged_modes, mut merged_kinds, mut merged_col) = if let Edge::TableAccess {
                modes,
                write_kinds,
                column_analysis,
                ..
            } = &graph[keep]
            {
                (*modes, write_kinds.clone(), column_analysis.clone())
            } else {
                continue;
            };
            for &remove_idx in &edge_indices {
                if let Edge::TableAccess {
                    modes,
                    write_kinds,
                    column_analysis,
                    ..
                } = &graph[remove_idx]
                {
                    merged_modes |= *modes;
                    for wk in write_kinds {
                        merged_kinds.insert(*wk);
                    }
                    if merged_col.is_none() && column_analysis.is_some() {
                        merged_col = column_analysis.clone();
                    }
                }
            }
            if let Edge::TableAccess {
                modes,
                write_kinds,
                column_analysis,
                ..
            } = &mut graph[keep]
            {
                *modes = merged_modes;
                *write_kinds = merged_kinds;
                *column_analysis = merged_col;
            }
            edges_to_remove.extend(edge_indices);
        }
        for idx in edges_to_remove {
            graph.remove_edge(idx);
        }
    }

    /// Post-processing pass: resolve unresolved nodes against the complete graph.
    ///
    /// After all nodes and edges are created, some unresolved nodes may actually
    /// correspond to existing procedure/function nodes that weren't matched during
    /// edge creation (e.g. due to kind mismatch Procedure↔Function, case differences,
    /// or missing caller context). This pass attempts to resolve them and rewire edges.
    ///
    /// Also removes noise unresolved nodes whose `raw_expr` is an AST debug string
    /// (PlVariable, BinaryOp, FunctionCall, Literal) or a known non-routine pattern
    /// (SELF.xxx, collection methods .EXTEND/.TRIM/.DELETE, system packages).
    fn resolve_unresolved_nodes(graph: &mut CodeGraph) {
        // ── Build comprehensive resolution indexes ──
        let mut lower_qualified: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();
        let mut bare_name_lower: HashMap<String, Vec<petgraph::graph::NodeIndex>> = HashMap::new();
        let mut bare_name_schemas: HashMap<
            String,
            Vec<(Option<String>, petgraph::graph::NodeIndex)>,
        > = HashMap::new();
        let mut pkg_member_lower: HashMap<(String, String), petgraph::graph::NodeIndex> =
            HashMap::new();

        for idx in graph.node_indices() {
            let routine_id = match &graph[idx] {
                Node::Procedure { id, .. } | Node::Function { id, .. } => id,
                _ => continue,
            };
            let qualified_lower = routine_id.to_string().to_lowercase();
            // First registration wins (prefer real nodes over unresolved that
            // may have been inserted into proc_index during edge creation).
            lower_qualified.entry(qualified_lower).or_insert(idx);

            let name_lower = routine_id.name.to_lowercase();
            bare_name_lower
                .entry(name_lower.clone())
                .or_default()
                .push(idx);
            bare_name_schemas
                .entry(name_lower)
                .or_default()
                .push((routine_id.schema.as_ref().map(|s| s.to_lowercase()), idx));

            if let Some(ref pkg) = routine_id.package {
                pkg_member_lower
                    .entry((pkg.to_lowercase(), routine_id.name.to_lowercase()))
                    .or_insert(idx);
            }
            // For schema-qualified standalone routines, also index as if
            // schema were a package name (schema-as-package fallback).
            if let Some(ref schema) = routine_id.schema {
                if routine_id.package.is_none() {
                    pkg_member_lower
                        .entry((schema.to_lowercase(), routine_id.name.to_lowercase()))
                        .or_insert(idx);
                }
            }
        }

        // ── Collect unresolved nodes ──
        let unresolved: Vec<(petgraph::graph::NodeIndex, String, String)> = graph
            .node_indices()
            .filter_map(|idx| match &graph[idx] {
                Node::Unresolved { raw_expr, context } => {
                    Some((idx, (**raw_expr).clone(), (**context).clone()))
                }
                _ => None,
            })
            .collect();

        let mut to_remove: Vec<petgraph::graph::NodeIndex> = Vec::new();
        let mut resolved_count: usize = 0;
        let mut noise_count: usize = 0;

        for (unres_idx, raw_expr, _context) in &unresolved {
            if let Some(rule) = noise_rule(raw_expr) {
                crate::parse_log::info(
                    "(post-pass)",
                    &format!("noise-filtered '{}' (rule: {})", raw_expr, rule),
                );
                noise_count += 1;
                to_remove.push(*unres_idx);
                continue;
            }

            // ── Collect caller schemas from incoming edges for disambiguation ──
            let caller_schemas: Vec<Option<String>> = graph
                .neighbors_directed(*unres_idx, petgraph::Direction::Incoming)
                .filter_map(|src| match &graph[src] {
                    Node::Procedure { id, .. } | Node::Function { id, .. } => {
                        Some(id.schema.as_ref().map(|s| s.to_lowercase()))
                    }
                    _ => None,
                })
                .collect();

            // ── Try to resolve ──
            match try_resolve_routine(
                raw_expr,
                &lower_qualified,
                &bare_name_lower,
                &bare_name_schemas,
                &pkg_member_lower,
                &caller_schemas,
            ) {
                ResolveOutcome::Resolved(target_idx, strategy) => {
                    if target_idx == *unres_idx {
                        continue; // shouldn't happen, but guard
                    }

                    // Collect source nodes with their schemas for per-edge logic.
                    let sources: Vec<(petgraph::graph::NodeIndex, Option<String>)> = graph
                        .neighbors_directed(*unres_idx, petgraph::Direction::Incoming)
                        .map(|src| {
                            let schema = match &graph[src] {
                                Node::Procedure { id, .. } | Node::Function { id, .. } => {
                                    id.schema.as_ref().map(|s| s.to_lowercase())
                                }
                                _ => None,
                            };
                            (src, schema)
                        })
                        .collect();

                    // When Strategy 5 is the deciding factor AND multiple distinct
                    // caller schemas exist, resolve each edge independently based on
                    // that edge's source-node schema. Otherwise, all edges go to the
                    // same target (caller-independent strategies are correct for all).
                    // Only count non-None schemas (Proc/Function callers); non-Proc
                    // callers such as Trigger/Package bodies don't carry schema context.
                    let distinct_schemas: std::collections::HashSet<&str> =
                        sources.iter().filter_map(|(_, s)| s.as_deref()).collect();
                    let needs_per_edge =
                        strategy == ResolutionStrategy::CallerSchema && distinct_schemas.len() > 1;

                    for (src, src_schema) in &sources {
                        let edge_target = if needs_per_edge {
                            let single_schema: Vec<Option<String>> = vec![src_schema.clone()];
                            match try_resolve_routine(
                                raw_expr,
                                &lower_qualified,
                                &bare_name_lower,
                                &bare_name_schemas,
                                &pkg_member_lower,
                                &single_schema,
                            ) {
                                ResolveOutcome::Resolved(idx, _) => idx,
                                ResolveOutcome::Miss(_) => target_idx,
                            }
                        } else {
                            target_idx
                        };

                        let weights: Vec<_> = graph
                            .edges_connecting(*src, *unres_idx)
                            .map(|e| e.weight().clone())
                            .collect();
                        for weight in weights {
                            graph.add_edge(*src, edge_target, weight);
                        }
                    }
                    to_remove.push(*unres_idx);
                    resolved_count += 1;
                }
                ResolveOutcome::Miss(trace) => {
                    let nearest =
                        nearest_routine_candidates(raw_expr, &lower_qualified, graph, 3, 3);
                    let formatted = format_survivor_diagnostic(raw_expr, &trace, &nearest);
                    crate::parse_log::warn("(post-pass)", &formatted);
                }
            }
        }

        let survivors = unresolved.len() - resolved_count - noise_count;
        crate::parse_log::info(
            "(post-pass)",
            &format!(
                "resolve_unresolved_nodes: created={} resolved={} noise={} survivors={}",
                unresolved.len(),
                resolved_count,
                noise_count,
                survivors
            ),
        );

        // ── Remove resolved/filtered nodes ──
        // petgraph::Graph::remove_node swaps the last node into the freed slot,
        // so indices shift on each removal. Remove in descending index order
        // (with dedup) to keep all pending indices in to_remove valid.
        to_remove.sort_unstable();
        to_remove.dedup();
        for idx in to_remove.into_iter().rev() {
            graph.remove_node(idx);
        }
    }

    pub(crate) fn add_java_method_nodes_from_parsed(
        java_results: &[crate::parser::java_method::JavaParseResult],
        graph: &mut CodeGraph,
        _proc_index: &mut HashMap<RoutineId, petgraph::graph::NodeIndex>,
        mapper_index: &HashMap<String, petgraph::graph::NodeIndex>,
    ) {
        let mut class_index: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();
        let mut method_index: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();
        let mut simple_name_to_fqn: HashMap<String, String> = HashMap::new();
        let mut import_map: HashMap<PathBuf, HashMap<String, String>> = HashMap::new();

        for result in java_results {
            let mut file_imports = HashMap::new();
            for import in &result.imports {
                if let Some(simple) = import.rsplit('.').next() {
                    file_imports.insert(simple.to_string(), import.clone());
                }
            }
            import_map.insert(result.file.clone(), file_imports);
            for class in &result.classes {
                simple_name_to_fqn.insert(class.name.clone(), class.fqn.clone());
            }
        }

        let mut method_to_mappers: HashMap<String, Vec<(String, petgraph::graph::NodeIndex)>> =
            HashMap::new();
        for (key, &idx) in mapper_index.iter() {
            if let Some((_, method)) = key.rsplit_once('.') {
                method_to_mappers
                    .entry(method.to_string())
                    .or_default()
                    .push((key.clone(), idx));
            }
        }

        for result in java_results {
            for class in &result.classes {
                class_index.entry(class.fqn.clone()).or_insert_with(|| {
                    let node = Node::JavaClass {
                        fqn: class.fqn.clone(),
                        name: class.name.clone(),
                        package: if class.package.is_empty() {
                            None
                        } else {
                            Some(class.package.clone())
                        },
                        file: class.file.clone(),
                        line: class.line,
                    };
                    graph.add_node(node)
                });
            }
        }

        for result in java_results {
            for class in &result.classes {
                let class_idx = class_index[&class.fqn];
                if let Some(extends_name) = &class.extends {
                    if let Some(parent_fqn) = resolve_fqn(
                        extends_name,
                        &simple_name_to_fqn,
                        import_map.get(&result.file),
                    ) {
                        if let Some(&parent_idx) = class_index.get(&parent_fqn) {
                            graph.add_edge(
                                class_idx,
                                parent_idx,
                                Edge::Extends {
                                    location: SourceLocation {
                                        file: Arc::new(class.file.clone()),
                                        line: class.line,
                                    },
                                },
                            );
                        }
                    }
                }
                for iface_name in &class.implements {
                    if let Some(iface_fqn) = resolve_fqn(
                        iface_name,
                        &simple_name_to_fqn,
                        import_map.get(&result.file),
                    ) {
                        if let Some(&iface_idx) = class_index.get(&iface_fqn) {
                            graph.add_edge(
                                class_idx,
                                iface_idx,
                                Edge::Implements {
                                    location: SourceLocation {
                                        file: Arc::new(class.file.clone()),
                                        line: class.line,
                                    },
                                },
                            );
                        }
                    }
                }
            }
        }

        for result in java_results {
            for method in &result.methods {
                let method_fqn = format!("{}.{}", method.class_fqn, method.name);
                let method_idx = *method_index.entry(method_fqn.clone()).or_insert_with(|| {
                    let node = Node::JavaMethod {
                        fqn: method_fqn.clone(),
                        class_fqn: method.class_fqn.clone(),
                        name: method.name.clone(),
                        signature: method.signature.clone(),
                        file: method.file.clone(),
                        line: method.line,
                    };
                    graph.add_node(node)
                });

                if let Some(&class_idx) = class_index.get(&method.class_fqn) {
                    graph.add_edge(class_idx, method_idx, Edge::ContainsMethod);
                }
            }
        }

        for result in java_results {
            for method in &result.methods {
                let method_fqn = format!("{}.{}", method.class_fqn, method.name);
                let method_idx = method_index[&method_fqn];

                for call in &method.calls {
                    let location = SourceLocation {
                        file: Arc::new(method.file.clone()),
                        line: call.line,
                    };

                    if is_sqlsession_method(&call.method) {
                        if let Some(ns_id) = call.string_args.first() {
                            if let Some(&mapper_idx) = mapper_index.get(ns_id) {
                                graph.add_edge(
                                    method_idx,
                                    mapper_idx,
                                    Edge::InvokesMapper { location },
                                );
                                continue;
                            }
                        }
                    }

                    if let Some(obj) = &call.object {
                        if let Some(obj_fqn) =
                            resolve_fqn(obj, &simple_name_to_fqn, import_map.get(&result.file))
                        {
                            let mapper_key = format!("{}.{}", obj_fqn, call.method);
                            if let Some(&mapper_idx) = mapper_index.get(&mapper_key) {
                                graph.add_edge(
                                    method_idx,
                                    mapper_idx,
                                    Edge::InvokesMapper { location },
                                );
                                continue;
                            }

                            let mut found_mapper = false;
                            if let Some(candidates) = method_to_mappers.get(&call.method) {
                                for (key, mapper_idx) in candidates {
                                    if let Some((ns, _)) = key.rsplit_once('.') {
                                        let ns_simple = ns.rsplit('.').next().unwrap_or(ns);
                                        if names_match(obj, ns_simple) {
                                            graph.add_edge(
                                                method_idx,
                                                *mapper_idx,
                                                Edge::InvokesMapper {
                                                    location: location.clone(),
                                                },
                                            );
                                            found_mapper = true;
                                            break;
                                        }
                                    }
                                }
                            }
                            if found_mapper {
                                continue;
                            }

                            let callee_fqn = format!("{}.{}", method.class_fqn, call.method);
                            if let Some(&callee_idx) = method_index.get(&callee_fqn) {
                                graph.add_edge(
                                    method_idx,
                                    callee_idx,
                                    Edge::CallsJava { location },
                                );
                                continue;
                            }
                        } else {
                            let mut found_mapper = false;
                            if let Some(candidates) = method_to_mappers.get(&call.method) {
                                for (key, mapper_idx) in candidates {
                                    if let Some((ns, _)) = key.rsplit_once('.') {
                                        let ns_simple = ns.rsplit('.').next().unwrap_or(ns);
                                        if names_match(obj, ns_simple) {
                                            graph.add_edge(
                                                method_idx,
                                                *mapper_idx,
                                                Edge::InvokesMapper {
                                                    location: location.clone(),
                                                },
                                            );
                                            found_mapper = true;
                                            break;
                                        }
                                    }
                                }
                            }
                            if found_mapper {
                                continue;
                            }

                            let callee_fqn = format!("{}.{}", method.class_fqn, call.method);
                            if let Some(&callee_idx) = method_index.get(&callee_fqn) {
                                graph.add_edge(
                                    method_idx,
                                    callee_idx,
                                    Edge::CallsJava { location },
                                );
                            }
                        }
                    } else {
                        let callee_fqn = format!("{}.{}", method.class_fqn, call.method);
                        if let Some(&callee_idx) = method_index.get(&callee_fqn) {
                            graph.add_edge(method_idx, callee_idx, Edge::CallsJava { location });
                        }
                    }
                }
            }
        }

        for result in java_results {
            if result.di_injections.is_empty() {
                continue;
            }
            let file_imports = import_map.get(&result.file);
            let owning_class = result.classes.first();
            let Some(owning_class) = owning_class else {
                continue;
            };
            let Some(&owning_class_idx) = class_index.get(&owning_class.fqn) else {
                continue;
            };

            for injection in &result.di_injections {
                let target_fqn =
                    resolve_fqn(&injection.type_name, &simple_name_to_fqn, file_imports);
                let Some(target_fqn) = target_fqn else {
                    continue;
                };
                let Some(&target_class_idx) = class_index.get(&target_fqn) else {
                    continue;
                };

                let location = SourceLocation {
                    file: Arc::new(result.file.clone()),
                    line: injection.line,
                };
                graph.add_edge(
                    owning_class_idx,
                    target_class_idx,
                    Edge::CallsJava { location },
                );
            }
        }

        // Bridge: connect JavaSql nodes to their parent JavaMethod.
        // JavaSql nodes are created in a prior pass (add_java_nodes_from_parsed)
        // without method edges. Now that JavaMethod nodes exist, link them.
        let mut javasql_indices: Vec<petgraph::graph::NodeIndex> = Vec::new();
        for idx in graph.node_indices() {
            if matches!(graph[idx], Node::JavaSql { .. }) {
                javasql_indices.push(idx);
            }
        }
        for idx in javasql_indices {
            if let Node::JavaSql {
                ref class_name,
                ref method_name,
                ..
            } = graph[idx]
            {
                let m = match method_name {
                    Some(m) => m,
                    None => continue,
                };
                let c = class_name.as_deref().unwrap_or("");
                // Resolve short class name to FQN, then build method FQN
                let class_fqn = simple_name_to_fqn
                    .get(c)
                    .cloned()
                    .unwrap_or_else(|| c.to_string());
                let method_fqn = format!("{}.{}", class_fqn, m);
                if let Some(&method_idx) = method_index.get(&method_fqn) {
                    graph.add_edge(method_idx, idx, Edge::ContainsSql);
                }
            }
        }
    }

    #[cfg(feature = "jsp")]
    pub(crate) fn bridge_jsp_to_java_methods(
        graph: &mut CodeGraph,
        jsp_files: &[crate::parser::jsp_loader::JspFileResult],
        simple_to_fqn: &HashMap<String, String>,
    ) {
        let mut method_index: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();
        let mut class_index: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();
        for idx in graph.node_indices() {
            match &graph[idx] {
                Node::JavaMethod { ref fqn, .. } => {
                    method_index.insert(fqn.clone(), idx);
                }
                Node::JavaClass { ref fqn, .. } => {
                    class_index.insert(fqn.clone(), idx);
                }
                _ => {}
            }
        }

        // Map file path → JspPage node index
        let mut file_to_jsp: HashMap<PathBuf, petgraph::graph::NodeIndex> = HashMap::new();
        for idx in graph.node_indices() {
            if let Node::JspPage { ref path, .. } = graph[idx] {
                file_to_jsp.insert(path.clone(), idx);
            }
        }

        for jsp_file in jsp_files {
            if jsp_file.java_refs.is_empty() {
                continue;
            }
            let Some(&jsp_idx) = file_to_jsp.get(&jsp_file.file) else {
                continue;
            };
            for r in &jsp_file.java_refs {
                let class_fqn = simple_to_fqn
                    .get(&r.class_name)
                    .cloned()
                    .unwrap_or_else(|| r.class_name.clone());
                let location = crate::graph::SourceLocation {
                    file: Arc::new(PathBuf::new()),
                    line: r.line,
                };
                if r.method_name == "<init>" {
                    // Constructor → edge to JavaClass
                    if let Some(&class_idx) = class_index.get(&class_fqn) {
                        graph.add_edge(jsp_idx, class_idx, Edge::CallsJava { location });
                    }
                } else {
                    // Static method call → edge to JavaMethod
                    let method_fqn = format!("{}.{}", class_fqn, r.method_name);
                    if let Some(&method_idx) = method_index.get(&method_fqn) {
                        graph.add_edge(jsp_idx, method_idx, Edge::CallsJava { location });
                    }
                }
            }
        }
    }
}

fn resolve_fqn(
    simple_name: &str,
    simple_name_to_fqn: &HashMap<String, String>,
    file_imports: Option<&HashMap<String, String>>,
) -> Option<String> {
    if simple_name.contains('.') {
        return Some(simple_name.to_string());
    }
    if let Some(imports) = file_imports {
        if let Some(fqn) = imports.get(simple_name) {
            return Some(fqn.clone());
        }
    }
    if let Some(fqn) = simple_name_to_fqn.get(simple_name) {
        return Some(fqn.clone());
    }
    None
}

fn is_sqlsession_method(method: &str) -> bool {
    matches!(
        method,
        "selectList"
            | "selectOne"
            | "selectMap"
            | "select"
            | "insert"
            | "update"
            | "delete"
            | "query"
            | "queryForList"
            | "queryForObject"
    )
}

fn names_match(field_name: &str, class_name: &str) -> bool {
    if field_name == class_name {
        return true;
    }
    let mut chars = field_name.chars();
    if let Some(first) = chars.next() {
        let capitalized = first.to_uppercase().to_string() + chars.as_str();
        if capitalized == class_name {
            return true;
        }
    }
    false
}

/// Check if an unresolved node's `raw_expr` is clearly not a routine name.
///
/// Filters out AST debug strings from dynamic SQL (PlVariable, BinaryOp,
/// FunctionCall, Literal), object member access (SELF.xxx), PL/SQL collection
/// methods (.EXTEND, .TRIM, .DELETE), and known system packages/functions.
fn noise_rule(raw_expr: &str) -> Option<&'static str> {
    let trimmed = raw_expr.trim();
    if trimmed.starts_with("PlVariable(")
        || trimmed.starts_with("BinaryOp ")
        || trimmed.starts_with("BinaryOp{")
        || trimmed.starts_with("FunctionCall ")
        || trimmed.starts_with("FunctionCall{")
        || trimmed.starts_with("Literal(")
        || trimmed.starts_with("ColumnRef(")
    {
        return Some("ast-debug-string");
    }
    let upper = trimmed.to_uppercase();
    if upper.starts_with("SELF.") {
        return Some("self-member");
    }
    if upper.ends_with(".EXTEND")
        || upper.ends_with(".TRIM")
        || upper.ends_with(".DELETE")
        || upper.ends_with(".COUNT")
        || upper.ends_with(".FIRST")
        || upper.ends_with(".LAST")
        || upper.ends_with(".NEXT")
        || upper.ends_with(".PRIOR")
        || upper.ends_with(".EXISTS")
    {
        return Some("collection-method");
    }
    if ogsql_parser::parser::function_registry::lookup_builtin_meta(trimmed).is_some() {
        return Some("builtin-function");
    }
    None
}

/// Which resolution strategy succeeded.
///
/// Used in `resolve_unresolved_nodes` to decide whether per-edge rewiring is
/// needed: Strategy 5 (CallerSchema) is caller-dependent — when multiple
/// distinct caller schemas share the same unresolved node, each edge must be
/// rewired to the target matching that edge's source-node schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolutionStrategy {
    /// Strategy 1: Case-insensitive exact qualified-name match
    ExactQualified,
    /// Strategy 2: Bare name extracted from qualified (single unambiguous match)
    BareNameQualified,
    /// Strategy 3: Qualified prefix treated as package name
    SchemaAsPackage,
    /// Strategy 4: Unqualified bare name (single unambiguous match)
    SingleBareName,
    /// Strategy 5: Caller-schema disambiguation — **caller-dependent**
    CallerSchema,
    /// Strategy 6: Prefer schema=None (default schema)
    DefaultSchema,
    /// Strategy 7: Ambiguous — pick first candidate (best-effort)
    Ambiguous,
}

/// Outcome of multi-strategy routine resolution.
///
/// Either `Resolved(idx, strategy)` on success, or `Miss(trace)` with a
/// diagnostic trace explaining why all 7 strategies failed.
#[derive(Debug, Clone)]
pub(crate) enum ResolveOutcome {
    Resolved(petgraph::graph::NodeIndex, ResolutionStrategy),
    Miss(StrategyTrace),
}

/// Diagnostic trace for a failed resolution attempt.
///
/// Records per-strategy state at the time each strategy ran, so that
/// downstream consumers (logging, survivor reporting) can root-cause WHY
/// a reference could not be resolved.
///
/// Field conventions:
/// - `parsed_schema` / `parsed_name`: split of `raw_name` via `rsplit_once('.')`.
/// - `s1_qualified_key`: the lowercased `raw_name` used as the key in S1.
/// - `s1_hit` / `s3_hit`: whether that strategy's lookup succeeded.
/// - `s3_lookup`: the `(pkg_part_lower, name_part_lower)` pair used in S3,
///   or `None` when `raw_name` contained no dot.
/// - `caller_schemas`: snapshot of the caller-schema slice passed in.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct StrategyTrace {
    pub parsed_schema: Option<String>,
    pub parsed_name: String,
    pub s1_qualified_key: String,
    pub s1_hit: bool,
    pub s3_lookup: Option<(String, String)>,
    pub s3_hit: bool,
    pub caller_schemas: Vec<Option<String>>,
}

/// Multi-strategy routine resolution for unresolved nodes.
///
/// Resolution priority:
/// 1. Case-insensitive exact qualified-name match
/// 2. Qualified name → bare-name (unique match)
/// 3. Qualified name → schema-as-package lookup
/// 4. Unqualified bare name → unique match
/// 5. Ambiguous bare name → caller-schema disambiguation
/// 6. Ambiguous bare name → schema=None (default-schema) preference
/// 7. Ambiguous bare name → first candidate (best-effort)
fn try_resolve_routine(
    raw_name: &str,
    lower_qualified: &HashMap<String, petgraph::graph::NodeIndex>,
    bare_name_lower: &HashMap<String, Vec<petgraph::graph::NodeIndex>>,
    bare_name_schemas: &HashMap<String, Vec<(Option<String>, petgraph::graph::NodeIndex)>>,
    pkg_member_lower: &HashMap<(String, String), petgraph::graph::NodeIndex>,
    caller_schemas: &[Option<String>],
) -> ResolveOutcome {
    let name_lower = raw_name.to_lowercase();

    // Parse raw_name for trace: split into (schema, name) via rsplit_once('.').
    let (parsed_schema, parsed_name) = raw_name
        .rsplit_once('.')
        .map(|(s, n)| (Some(s.to_string()), n.to_string()))
        .unwrap_or_else(|| (None, raw_name.to_string()));

    // Initialize trace — fields are filled in as each strategy runs.
    let mut trace = StrategyTrace {
        parsed_schema,
        parsed_name,
        s1_qualified_key: name_lower.clone(),
        s1_hit: false,
        s3_lookup: None,
        s3_hit: false,
        caller_schemas: caller_schemas.to_vec(),
    };

    // Strategy 1: Case-insensitive exact match (handles Procedure↔Function implicitly
    // because lower_qualified is keyed by display string which doesn't include kind)
    if let Some(&idx) = lower_qualified.get(&name_lower) {
        trace.s1_hit = true;
        return ResolveOutcome::Resolved(idx, ResolutionStrategy::ExactQualified);
    }

    // Strategy 2: If raw_name is "schema.name", try bare name in bare_name_lower
    // (this handles cases where the schema prefix differs from what's stored)
    if let Some(dot_pos) = raw_name.rfind('.') {
        let bare = &raw_name[dot_pos + 1..];
        let bare_lower = bare.to_lowercase();

        // Try case-insensitive bare name (single unambiguous match)
        if let Some(matches) = bare_name_lower.get(&bare_lower) {
            if matches.len() == 1 {
                return ResolveOutcome::Resolved(matches[0], ResolutionStrategy::BareNameQualified);
            }
        }
    }

    // Strategy 3: Schema-as-package: treat the prefix as a package name
    if let Some(dot_pos) = raw_name.rfind('.') {
        let pkg_part = &raw_name[..dot_pos];
        let name_part = &raw_name[dot_pos + 1..];
        let pkg_part_lower = pkg_part.to_lowercase();
        let name_part_lower = name_part.to_lowercase();
        trace.s3_lookup = Some((pkg_part_lower.clone(), name_part_lower.clone()));
        if let Some(&idx) = pkg_member_lower.get(&(pkg_part_lower, name_part_lower)) {
            trace.s3_hit = true;
            return ResolveOutcome::Resolved(idx, ResolutionStrategy::SchemaAsPackage);
        }
    }

    // Strategy 4: Unqualified bare name — single unambiguous match
    if !raw_name.contains('.') {
        if let Some(matches) = bare_name_lower.get(&name_lower) {
            if matches.len() == 1 {
                return ResolveOutcome::Resolved(matches[0], ResolutionStrategy::SingleBareName);
            }
        }
    }

    // Strategy 5–7: Ambiguous bare-name disambiguation.
    // Extract the bare name to search (handles both qualified and unqualified raw_name).
    let bare_lookup = raw_name
        .rsplit_once('.')
        .map(|(_, bare)| bare.to_lowercase())
        .unwrap_or_else(|| name_lower.clone());

    if let Some(candidates) = bare_name_schemas.get(&bare_lookup) {
        if candidates.len() > 1 {
            // Strategy 5: Prefer candidate in the caller's schema.
            for caller_schema in caller_schemas.iter().flatten() {
                if let Some(&(_, idx)) = candidates
                    .iter()
                    .find(|(s, _)| s.as_deref() == Some(caller_schema.as_str()))
                {
                    return ResolveOutcome::Resolved(idx, ResolutionStrategy::CallerSchema);
                }
            }

            // Strategy 6: No caller-schema match — prefer schema=None (default schema).
            if let Some(&(_, idx)) = candidates.iter().find(|(s, _)| s.is_none()) {
                return ResolveOutcome::Resolved(idx, ResolutionStrategy::DefaultSchema);
            }

            // Strategy 7: Truly ambiguous — best-effort: pick first candidate.
            // For a static code graph this is better than leaving an Unresolved node,
            // because the user gets a starting point for investigation.
            return ResolveOutcome::Resolved(candidates[0].1, ResolutionStrategy::Ambiguous);
        }
    }

    ResolveOutcome::Miss(trace)
}

/// Standard Levenshtein edit distance (case-sensitive).
///
/// Uses O(min(m,n)) memory with a two-row DP table.
fn levenshtein(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();

    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    let mut prev: Vec<usize> = (0..=b_len).collect();
    let mut curr: Vec<usize> = vec![0; b_len + 1];

    for i in 1..=a_len {
        curr[0] = i;
        for j in 1..=b_len {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            curr[j] = std::cmp::min(
                std::cmp::min(curr[j - 1] + 1, prev[j] + 1),
                prev[j - 1] + cost,
            );
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b_len]
}

fn unresolved_creation_suffix(
    parsed: Option<(Option<&str>, Option<&str>, &str)>,
    snippet: Option<&str>,
) -> String {
    let mut out = String::new();
    if let Some((schema, package, name)) = parsed {
        out.push_str(&format!(
            "\n  parsed: {{schema:{:?}, package:{:?}, name:{:?}}}",
            schema, package, name
        ));
    }
    if let Some(s) = snippet {
        out.push('\n');
        out.push_str(s);
    }
    out
}

/// Find nearest routine candidates by Levenshtein distance on the bare name.
///
/// Extracts the bare (unqualified) part of `raw_name` and compares it
/// against each routine's `name` field (lowercased). Returns up to `limit`
/// candidates within `max_distance`, sorted by ascending distance.
fn nearest_routine_candidates(
    raw_name: &str,
    lower_qualified: &HashMap<String, petgraph::graph::NodeIndex>,
    graph: &CodeGraph,
    max_distance: usize,
    limit: usize,
) -> Vec<(usize, RoutineId)> {
    let bare = raw_name
        .rsplit_once('.')
        .map(|(_, b)| b)
        .unwrap_or(raw_name)
        .to_lowercase();

    let mut candidates: Vec<(usize, RoutineId)> = lower_qualified
        .iter()
        .filter_map(|(_key, &idx)| {
            let routine_id = match &graph[idx] {
                Node::Procedure { id, .. } | Node::Function { id, .. } => id,
                _ => return None,
            };
            let name_lower = routine_id.name.to_lowercase();
            let dist = levenshtein(&bare, &name_lower);
            if dist <= max_distance {
                Some((dist, routine_id.clone()))
            } else {
                None
            }
        })
        .collect();

    candidates.sort_by_key(|(dist, _)| *dist);
    candidates.truncate(limit);
    candidates
}

/// Format a rich survivor diagnostic string for logging.
///
/// Pure function (no I/O). The caller is responsible for writing the
/// returned string to the log via [`crate::parse_log::warn`].
fn format_survivor_diagnostic(
    raw_expr: &str,
    trace: &StrategyTrace,
    nearest: &[(usize, RoutineId)],
) -> String {
    let mut lines = Vec::new();

    // First line: header
    lines.push(format!(
        "unresolved(post-pass): '{}' survived resolution",
        raw_expr
    ));

    // Parsed expression breakdown
    lines.push(format!(
        "  parsed = {{schema:{:?}, package:None, name:{:?}}}",
        trace.parsed_schema, trace.parsed_name,
    ));

    // Strategy 1
    lines.push(format!(
        "  S1 lower_qualified['{}'] -> miss",
        trace.s1_qualified_key
    ));

    // Strategy 3 (only when raw_name had a dot)
    if let Some((ref pkg, ref name)) = trace.s3_lookup {
        lines.push(format!(
            "  S3 pkg_member_lower[('{}','{}')] -> miss",
            pkg, name
        ));
    }

    // Nearest candidates by edit distance
    if nearest.is_empty() {
        lines.push("  nearest by edit distance: (none within edit-distance threshold)".to_string());
    } else {
        let nearest_fmt: Vec<String> = nearest
            .iter()
            .map(|(dist, id)| {
                let fmt_opt = |o: &Option<String>| {
                    o.as_deref()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "None".to_string())
                };
                format!(
                    "'{}' {{schema:{},package:{},name:{},kind:{:?}}}(d={})",
                    id,
                    fmt_opt(&id.schema),
                    fmt_opt(&id.package),
                    id.name,
                    id.kind,
                    dist,
                )
            })
            .collect();
        lines.push(format!(
            "  nearest by edit distance: {}",
            nearest_fmt.join(", ")
        ));
    }

    lines.join("\n")
}

/// Normalize a table/view name for case-insensitive lookup in `table_index`.
///
/// openGauss/GaussDB (like PostgreSQL) folds unquoted identifiers to lowercase,
/// so `MV_ACCOUNT_PRIV` and `mv_account_priv` are the same object.
fn normalize_table_key(schema: Option<&str>, name: &str) -> String {
    match schema {
        Some(s) => format!("{}.{}", s.to_lowercase(), name.to_lowercase()),
        None => name.to_lowercase(),
    }
}

/// Normalize a non-table object key for case-insensitive lookup.
fn normalize_object_key(schema: Option<&str>, name: &str) -> String {
    match schema {
        Some(s) => format!("{}.{}", s.to_lowercase(), name.to_lowercase()),
        None => name.to_lowercase(),
    }
}

/// Split a `COMMENT ON COLUMN` object name into (schema, table, column).
/// Handles 2-part `["tbl", "col"]` and 3-part `["schema", "tbl", "col"]`.
fn split_comment_col_name(name: &[ogsql_parser::Ident]) -> (Option<String>, String, String) {
    if name.len() <= 2 {
        (
            None,
            name.first().map(|i| i.to_string()).unwrap_or_default(),
            name.get(1).map(|i| i.to_string()).unwrap_or_default(),
        )
    } else {
        let schema = name[..name.len() - 2]
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(".");
        (
            Some(schema),
            name[name.len() - 2].to_string(),
            name[name.len() - 1].to_string(),
        )
    }
}

fn apply_deferred_column_comments(ctx: &mut GraphBuildContext) {
    for dc in &ctx.deferred_column_comments {
        let Some(&table_idx) = ctx.table_index.get(&dc.table_key) else {
            continue;
        };
        // Guard: table_index may be stale after dedup_table_view_nodes
        // merged/removed Table nodes. NodeIndex survives swap_remove only
        // if it wasn't the swapped-out last slot.
        let Some(node) = ctx.graph.node_weight_mut(table_idx) else {
            continue;
        };
        if let Node::Table { columns, .. } = node {
            for col in columns.iter_mut() {
                if col.name.eq_ignore_ascii_case(&dc.col_name) {
                    col.comment = Some(dc.comment.clone());
                    break;
                }
            }
        }
    }
}

fn split_object_name(name: &[ogsql_parser::Ident]) -> (Option<String>, String) {
    if name.len() <= 1 {
        (
            None,
            name.first().map(|i| i.to_string()).unwrap_or_default(),
        )
    } else {
        (
            Some(name[..name.len() - 1].join(".")),
            name[name.len() - 1].to_string(),
        )
    }
}

// Lowercased qualified package name (`schema.pkg`) used as the case-insensitive
// key for linking a package BODY to its SPEC. Mirrors the key built inline by
// `create_package_nodes`; kept separate to avoid a wider refactor of that fn.
fn pkg_qualified_key(name: &ogsql_parser::ast::ObjectName) -> String {
    let pkg_part = name.last().cloned().unwrap_or_default().to_lowercase();
    if name.len() > 1 {
        let schema: String = name[..name.len() - 1]
            .iter()
            .map(|i| i.to_string().to_lowercase())
            .collect::<Vec<_>>()
            .join(".");
        format!("{}.{}", schema, pkg_part)
    } else {
        pkg_part
    }
}

fn edge_call_scope(
    graph: &CodeGraph,
    caller_idx: petgraph::graph::NodeIndex,
    callee_idx: petgraph::graph::NodeIndex,
) -> CallScope {
    let caller = extract_routine_id(&graph[caller_idx]);
    let callee = extract_routine_id(&graph[callee_idx]);
    match (caller, callee) {
        (Some(a), Some(b)) => determine_call_scope(&a, &b),
        _ => CallScope::External,
    }
}

impl Default for GraphBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Forwards `visit_select` to [`ColumnLineageExtractor::analyze_select_statement`]
/// so that `walk_statement` analyzes SELECTs nested anywhere in a statement
/// (e.g. inside a procedure body or an INSERT ... SELECT source).
struct ColumnLineageWalker<'a> {
    extractor: &'a mut crate::parser::ColumnLineageExtractor,
}

impl<'a> ogsql_parser::Visitor for ColumnLineageWalker<'a> {
    fn visit_select(
        &mut self,
        select: &ogsql_parser::ast::SelectStatement,
    ) -> ogsql_parser::VisitorResult {
        self.extractor.analyze_select_statement(select);
        ogsql_parser::VisitorResult::Continue
    }
}

/// Extract column-level lineage from SELECT statements and add it to the graph.
///
/// Every `SELECT` reachable from `statements` (including nested selects inside
/// procedure bodies and `INSERT ... SELECT` sources) is analyzed; the resulting
/// `ColumnEdge`s become `Node::Column` nodes connected by `Edge::DataFlow` /
/// `Edge::Derived` / `Edge::Aggregated` edges. Returns the number of edges added.
fn add_column_lineage_edges(
    graph: &mut CodeGraph,
    statements: &[ogsql_parser::StatementInfo],
    owner_table: &str,
    location: &SourceLocation,
) -> usize {
    let mut extractor = crate::parser::ColumnLineageExtractor::new();
    extractor.set_output(owner_table);

    for info in statements {
        // Run the base ColumnAccessExtractor first to populate alias_map
        // (resolves aliases like "t" → "mid_yjqs_detail" for physical table names)
        let mut base_extractor = crate::parser::ColumnAccessExtractor::new();
        walk_statement(&mut base_extractor, &info.statement);
        let base_analysis = base_extractor.finish();
        extractor.set_alias_map(base_analysis.alias_map);

        let mut walker = ColumnLineageWalker {
            extractor: &mut extractor,
        };
        walk_statement(&mut walker, &info.statement);
    }

    let column_edges = extractor.finish();
    let mut count = 0;

    for edge in &column_edges {
        match edge {
            crate::parser::ColumnEdge::Flow {
                target_col,
                source_table,
                source_col,
                location: _,
            } => {
                let source_owner = source_table.as_deref().unwrap_or(owner_table);
                let source_id = format!("col:{}.{}", source_owner, source_col);
                let target_id = format!("col:{}", target_col);

                let source_idx = upsert_column_node(graph, &source_id, source_owner, source_col);
                let target_idx = upsert_column_node(
                    graph,
                    &target_id,
                    owner_table,
                    extract_col_name(target_col),
                );

                graph.add_edge(
                    source_idx,
                    target_idx,
                    Edge::DataFlow {
                        source_col_id: source_id,
                        target_col_id: target_id,
                        location: Some(location.clone()),
                    },
                );
                count += 1;
            }
            crate::parser::ColumnEdge::Derived {
                target_col,
                source_cols,
                expression,
                location: _,
            } => {
                let target_id = format!("col:{}", target_col);
                let target_idx = upsert_column_node(
                    graph,
                    &target_id,
                    owner_table,
                    extract_col_name(target_col),
                );

                let mut source_ids = Vec::new();
                let mut source_idxs = Vec::new();
                for (src_table, src_col) in source_cols {
                    let src_owner = src_table.as_deref().unwrap_or(owner_table);
                    let src_id = format!("col:{}.{}", src_owner, src_col);
                    let src_idx = upsert_column_node(graph, &src_id, src_owner, src_col);
                    source_ids.push(src_id);
                    source_idxs.push(src_idx);
                }

                for src_idx in source_idxs {
                    graph.add_edge(
                        src_idx,
                        target_idx,
                        Edge::Derived {
                            source_col_ids: source_ids.clone(),
                            target_col_id: target_id.clone(),
                            expression: expression.clone(),
                            location: Some(location.clone()),
                        },
                    );
                }
                count += 1;
            }
            crate::parser::ColumnEdge::Aggregated {
                target_col,
                source_cols,
                function,
                distinct,
                group_by_cols,
                location: _,
            } => {
                let target_id = format!("col:{}", target_col);
                let target_idx = upsert_column_node(
                    graph,
                    &target_id,
                    owner_table,
                    extract_col_name(target_col),
                );

                let mut source_ids = Vec::new();
                let mut source_idxs = Vec::new();
                for (src_table, src_col) in source_cols {
                    let src_owner = src_table.as_deref().unwrap_or(owner_table);
                    let src_id = format!("col:{}.{}", src_owner, src_col);
                    let src_idx = upsert_column_node(graph, &src_id, src_owner, src_col);
                    source_ids.push(src_id);
                    source_idxs.push(src_idx);
                }

                let group_by_col_ids: Vec<String> = group_by_cols
                    .iter()
                    .map(|c| format!("col:{}.{}", owner_table, c))
                    .collect();

                // Ensure GROUP BY column nodes exist and mark them as grouping keys
                for gb_id in &group_by_col_ids {
                    let gb_col_name = extract_col_name(gb_id);
                    let gb_idx = upsert_column_node(graph, gb_id, owner_table, gb_col_name);
                    if let Node::Column {
                        ref mut is_grouping_key,
                        ..
                    } = graph[gb_idx]
                    {
                        *is_grouping_key = true;
                    }
                }

                for src_idx in source_idxs {
                    graph.add_edge(
                        src_idx,
                        target_idx,
                        Edge::Aggregated {
                            source_col_ids: source_ids.clone(),
                            target_col_id: target_id.clone(),
                            function: function.clone(),
                            distinct: *distinct,
                            group_by_col_ids: group_by_col_ids.clone(),
                            location: Some(location.clone()),
                        },
                    );
                }
                count += 1;
            }
        }
    }

    count
}

/// Find or create a column node, returning its `NodeIndex`.
fn upsert_column_node(
    graph: &mut CodeGraph,
    col_id: &str,
    owner_table: &str,
    col_name: &str,
) -> petgraph::graph::NodeIndex {
    if let Some(idx) = find_column_node(graph, col_id) {
        return idx;
    }
    graph.add_node(Node::Column {
        id: col_id.to_string(),
        owner_table: owner_table.to_string(),
        name: col_name.to_string(),
        data_type: None,
        expression: None,
        aggregation: None,
        is_grouping_key: false,
        location: None,
    })
}

/// Find a column node by its `col:<table>.<column>` id.
fn find_column_node(graph: &CodeGraph, col_id: &str) -> Option<petgraph::graph::NodeIndex> {
    graph
        .node_indices()
        .find(|&idx| matches!(&graph[idx], Node::Column { id, .. } if id == col_id))
}

/// Extract the column name from a `table.column` qualified name.
fn extract_col_name(qualified: &str) -> &str {
    qualified.rsplit('.').next().unwrap_or(qualified)
}

#[cfg(test)]
mod tests {
    use crate::graph::builder::GraphBuilder;
    use crate::graph::{Edge, Node};
    use crate::parser::ParsedFile;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn parse_sql(sql: &str) -> Vec<ogsql_parser::StatementInfo> {
        let tokens = ogsql_parser::Tokenizer::new(sql).tokenize().unwrap();
        let mut parser = ogsql_parser::Parser::with_source(tokens, sql.to_string());
        parser.parse_with_text()
    }

    fn build_from_sql(sql: &str) -> crate::graph::CodeGraph {
        let stmts = parse_sql(sql);
        let parsed = vec![ParsedFile {
            path: PathBuf::from("test.sql"),
            statements: stmts,
            content_hash: String::new(),
        }];
        GraphBuilder::new().build(&parsed)
    }

    #[test]
    fn package_body_creates_package_and_procedure_nodes() {
        let sql = r#"
            CREATE OR REPLACE PACKAGE BODY pkg_api AS
                PROCEDURE do_work(p_id INT) IS
                BEGIN
                    helper.validate(p_id);
                END;
            END pkg_api;
        "#;
        let graph = build_from_sql(sql);

        let package_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::Package { name, .. } if name == "pkg_api"))
            .collect();
        assert_eq!(package_nodes.len(), 1, "Expected exactly 1 Package node");

        let proc_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::Procedure { id, .. } if id.name == "do_work" && id.package == Some("pkg_api".to_string())))
            .collect();
        assert_eq!(
            proc_nodes.len(),
            1,
            "Expected 1 Procedure with package=Some(pkg_api)"
        );

        let contains_edges: Vec<_> = graph
            .edge_indices()
            .filter(|e| matches!(&graph[*e], Edge::ContainsRoutine))
            .collect();
        assert_eq!(contains_edges.len(), 1, "Expected 1 ContainsRoutine edge");
    }

    #[test]
    fn package_body_procedure_calls_have_correct_caller() {
        let sql = r#"
            CREATE OR REPLACE PACKAGE BODY pkg_api AS
                PROCEDURE do_work(p_id INT) IS
                BEGIN
                    helper.validate(p_id);
                    helper.process(p_id);
                END;
            END pkg_api;
        "#;
        let graph = build_from_sql(sql);

        let direct_edges: Vec<_> = graph
            .edge_indices()
            .filter(|e| matches!(&graph[*e], Edge::DirectCall { .. }))
            .collect();
        assert_eq!(direct_edges.len(), 2, "Expected 2 DirectCall edges");

        let dowork_idx = graph
            .node_indices()
            .find(|i| matches!(&graph[*i], Node::Procedure { id, .. } if id.name == "do_work"))
            .expect("do_work node should exist");

        for edge_idx in &direct_edges {
            let (src, _) = graph.edge_endpoints(*edge_idx).unwrap();
            assert_eq!(
                src, dowork_idx,
                "DirectCall edges should originate from do_work"
            );
        }
    }

    #[test]
    fn trigger_creates_trigger_node_and_edge() {
        let sql = r#"
            CREATE OR REPLACE FUNCTION trg_func() RETURNS TRIGGER AS $$
            BEGIN
                RETURN NEW;
            END;
            $$ LANGUAGE plpgsql;

            CREATE TRIGGER trg_after_insert
            AFTER INSERT ON t_users
            FOR EACH ROW EXECUTE PROCEDURE trg_func();
        "#;
        let graph = build_from_sql(sql);

        let trigger_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(graph[*i], Node::Trigger { .. }))
            .collect();
        assert_eq!(trigger_nodes.len(), 1, "Expected 1 Trigger node");

        if let Node::Trigger { name, .. } = &graph[trigger_nodes[0]] {
            assert_eq!(name, "trg_after_insert");
        }

        let trigger_edges: Vec<_> = graph
            .edge_indices()
            .filter(|e| matches!(graph[*e], Edge::TriggersRoutine { .. }))
            .collect();
        assert_eq!(trigger_edges.len(), 1, "Expected 1 TriggersRoutine edge");

        let (src, dst) = graph.edge_endpoints(trigger_edges[0]).unwrap();
        assert!(matches!(graph[src], Node::Trigger { .. }));
        assert!(matches!(graph[dst], Node::Function { .. }));
    }

    #[test]
    fn standalone_call_to_package_routine_resolves() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURE caller_proc() AS $$
            BEGIN
                pkg_api.do_work(42);
            END;
            $$;

            CREATE OR REPLACE PACKAGE BODY pkg_api AS
                PROCEDURE do_work(p_id INT) IS
                BEGIN
                    helper.validate(p_id);
                END;
            END pkg_api;
        "#;
        let graph = build_from_sql(sql);

        let caller_idx = graph
            .node_indices()
            .find(|i| matches!(&graph[*i], Node::Procedure { id, .. } if id.name == "caller_proc"))
            .expect("caller_proc should exist");

        let dowork_idx = graph
            .node_indices()
            .find(|i| matches!(&graph[*i], Node::Procedure { id, .. } if id.name == "do_work"))
            .expect("do_work should exist");

        let has_edge = graph.edge_indices().any(|e| {
            matches!(&graph[e], Edge::DirectCall { .. }) && {
                let (src, dst) = graph.edge_endpoints(e).unwrap();
                src == caller_idx && dst == dowork_idx
            }
        });
        assert!(
            has_edge,
            "Expected DirectCall edge from caller_proc to do_work"
        );

        if let Node::Procedure { id, .. } = &graph[dowork_idx] {
            assert_eq!(id.package, Some("pkg_api".to_string()));
        }
    }

    #[test]
    fn view_creates_view_node_and_table_refs() {
        let sql = r#"
            CREATE VIEW v_active_users AS
            SELECT u.id, u.name
            FROM t_users u
            WHERE u.status = 'ACTIVE';
        "#;
        let graph = build_from_sql(sql);

        let view_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(graph[*i], Node::View { .. }))
            .collect();
        assert_eq!(view_nodes.len(), 1, "Expected 1 View node");

        if let Node::View { name, .. } = &graph[view_nodes[0]] {
            assert_eq!(name, "v_active_users");
        }

        let refs_from_view: Vec<_> = graph
            .edges_directed(view_nodes[0], petgraph::Direction::Outgoing)
            .filter(|e| matches!(e.weight(), Edge::DependsOn { .. }))
            .collect();
        assert_eq!(
            refs_from_view.len(),
            1,
            "View should depend on 1 table (t_users)"
        );
    }

    #[test]
    fn view_extracts_explicit_column_list() {
        let sql = r#"
            CREATE VIEW v_report (id, total, status) AS
            SELECT a.id, SUM(a.amount), a.status
            FROM t_accounts a
            GROUP BY a.id, a.status;
        "#;
        let graph = build_from_sql(sql);
        let view_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(graph[*i], Node::View { .. }))
            .collect();
        assert_eq!(view_nodes.len(), 1);
        if let Node::View { columns, .. } = &graph[view_nodes[0]] {
            assert_eq!(columns.len(), 3, "should have 3 explicit columns");
            assert_eq!(columns[0].name, "id");
            assert_eq!(columns[1].name, "total");
            assert_eq!(columns[2].name, "status");
        } else {
            panic!("expected View node");
        }
    }

    #[test]
    fn view_extracts_columns_from_select_targets() {
        let sql = r#"
            CREATE VIEW v_active_users AS
            SELECT u.id, u.name, u.email
            FROM t_users u
            WHERE u.status = 'ACTIVE';
        "#;
        let graph = build_from_sql(sql);
        let view_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(graph[*i], Node::View { .. }))
            .collect();
        assert_eq!(view_nodes.len(), 1);
        if let Node::View { columns, .. } = &graph[view_nodes[0]] {
            assert_eq!(
                columns.len(),
                3,
                "should extract 3 columns from SELECT targets"
            );
            assert_eq!(columns[0].name, "id");
            assert_eq!(columns[1].name, "name");
            assert_eq!(columns[2].name, "email");
        } else {
            panic!("expected View node");
        }
    }

    #[test]
    fn view_select_star_has_empty_columns() {
        let sql = "CREATE VIEW v_all AS SELECT * FROM t_users;";
        let graph = build_from_sql(sql);
        let view_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(graph[*i], Node::View { .. }))
            .collect();
        if let Node::View { columns, .. } = &graph[view_nodes[0]] {
            assert!(
                columns.is_empty(),
                "SELECT * cannot determine individual column names without table info"
            );
        }
    }

    #[test]
    fn view_stores_ddl_source() {
        let sql = "CREATE VIEW v_test AS SELECT 1 AS flag;";
        let graph = build_from_sql(sql);
        let view_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(graph[*i], Node::View { .. }))
            .collect();
        if let Node::View { ddl_source, .. } = &graph[view_nodes[0]] {
            // DDL source is stored — may be empty in test parser but is present in production
            assert!(
                ddl_source.is_some(),
                "view should have ddl_source field set"
            );
        }
    }

    #[test]
    fn view_has_file_location() {
        let sql = "CREATE VIEW v_loc AS SELECT 1;";
        let graph = build_from_sql(sql);
        let view_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(graph[*i], Node::View { .. }))
            .collect();
        if let Node::View { location, .. } = &graph[view_nodes[0]] {
            assert!(location.is_some(), "view should have a file location");
        }
    }

    #[test]
    fn create_type_creates_type_node() {
        let sql = r#"
            CREATE TYPE my_schema.my_enum AS ENUM ('a', 'b');
        "#;
        let graph = build_from_sql(sql);
        let type_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::Type { name, .. } if name == "my_enum"))
            .collect();
        assert_eq!(type_nodes.len(), 1, "Expected 1 Type node");
    }

    #[test]
    fn create_sequence_creates_sequence_node() {
        let sql = r#"
            CREATE SEQUENCE my_seq START 1;
        "#;
        let graph = build_from_sql(sql);
        let seq_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::Sequence { name, .. } if name == "my_seq"))
            .collect();
        assert_eq!(seq_nodes.len(), 1, "Expected 1 Sequence node");
    }

    #[test]
    fn create_index_creates_index_node_and_indexes_table_edge() {
        let sql = r#"
            CREATE TABLE t_users (id INT);
            CREATE UNIQUE INDEX idx_users ON t_users (id);
        "#;
        let graph = build_from_sql(sql);
        let idx_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::Index { name: Some(n), .. } if n == "idx_users"))
            .collect();
        assert_eq!(idx_nodes.len(), 1, "Expected 1 Index node");

        let idx_edges: Vec<_> = graph
            .edge_indices()
            .filter(|e| matches!(&graph[*e], Edge::IndexesTable { .. }))
            .collect();
        assert_eq!(idx_edges.len(), 1, "Expected 1 IndexesTable edge");
    }

    #[test]
    fn create_materialized_view_creates_mview_node_and_table_access() {
        let sql = r#"
            CREATE TABLE t_src (id INT);
            CREATE MATERIALIZED VIEW mv_data AS SELECT * FROM t_src;
        "#;
        let graph = build_from_sql(sql);
        let mview_nodes: Vec<_> = graph
            .node_indices()
            .filter(
                |i| matches!(&graph[*i], Node::MaterializedView { name, .. } if name == "mv_data"),
            )
            .collect();
        assert_eq!(mview_nodes.len(), 1, "Expected 1 MaterializedView node");

        let refs: Vec<_> = graph
            .edges_directed(mview_nodes[0], petgraph::Direction::Outgoing)
            .filter(|e| matches!(e.weight(), Edge::DependsOn { .. }))
            .collect();
        assert_eq!(refs.len(), 1, "MView should depend on 1 table");
    }

    #[test]
    fn create_synonym_creates_synonym_node_and_aliases_edge() {
        let sql = r#"
            CREATE TABLE real_table (id INT);
            CREATE SYNONYM s_table FOR real_table;
        "#;
        let graph = build_from_sql(sql);
        let syn_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::Synonym { name, .. } if name == "s_table"))
            .collect();
        assert_eq!(syn_nodes.len(), 1, "Expected 1 Synonym node");

        let alias_edges: Vec<_> = graph
            .edge_indices()
            .filter(|e| matches!(&graph[*e], Edge::AliasesObject { .. }))
            .collect();
        assert_eq!(alias_edges.len(), 1, "Expected 1 AliasesObject edge");
    }

    #[test]
    fn create_event_creates_event_node() {
        let sql = r#"
            CREATE EVENT my_event ON SCHEDULE EVERY 1 DAY DO BEGIN END;
        "#;
        let graph = build_from_sql(sql);
        let event_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::Event { name, .. } if name == "my_event"))
            .collect();
        assert_eq!(event_nodes.len(), 1, "Expected 1 Event node");
    }

    #[test]
    fn procedure_referencing_type_creates_references_type_edge() {
        let sql = r#"
            CREATE TYPE my_custom_type AS (x INT);
            CREATE PROCEDURE test_proc() AS $$
            DECLARE
                v_foo my_custom_type;
            BEGIN
                NULL;
            END;
            $$ LANGUAGE plpgsql;
        "#;
        let graph = build_from_sql(sql);
        let type_edges: Vec<_> = graph
            .edge_indices()
            .filter(|e| matches!(&graph[*e], Edge::ReferencesType { .. }))
            .collect();
        assert_eq!(type_edges.len(), 1, "Expected 1 ReferencesType edge");
    }

    #[test]
    fn procedure_using_nextval_creates_uses_sequence_edge() {
        let sql = r#"
            CREATE SEQUENCE my_seq START 1;
            CREATE PROCEDURE test_proc() AS $$
            BEGIN
                SELECT nextval('my_seq') INTO v_id;
            END;
            $$ LANGUAGE plpgsql;
        "#;
        let graph = build_from_sql(sql);
        let seq_edges: Vec<_> = graph
            .edge_indices()
            .filter(|e| matches!(&graph[*e], Edge::UsesSequence { .. }))
            .collect();
        assert_eq!(seq_edges.len(), 1, "Expected 1 UsesSequence edge");
    }

    #[test]
    fn procedure_using_dot_nextval_creates_uses_sequence_edge() {
        let sql = r#"
            CREATE SEQUENCE my_seq START 1;
            CREATE PROCEDURE test_proc() AS $$
            BEGIN
                v_id := my_seq.NEXTVAL;
            END;
            $$ LANGUAGE plpgsql;
        "#;
        let graph = build_from_sql(sql);
        let seq_edges: Vec<_> = graph
            .edge_indices()
            .filter(|e| matches!(&graph[*e], Edge::UsesSequence { .. }))
            .collect();
        assert_eq!(
            seq_edges.len(),
            1,
            "Expected 1 UsesSequence edge from dot NEXTVAL"
        );
    }

    #[test]
    fn gap_detection_creates_partial_nodes_for_missing_body_items() {
        use ogsql_parser::ast::{
            CreatePackageBodyStatement, CreatePackageStatement, PackageItem, PackageProcedure,
        };

        let spec = ogsql_parser::StatementInfo {
            sql_text: String::new(),
            start_line: 1,
            start_col: 0,
            end_line: 5,
            end_col: 0,
            statement: ogsql_parser::ast::Statement::CreatePackage(ogsql_parser::ast::Spanned {
                node: CreatePackageStatement {
                    replace: true,
                    name: vec!["pkg_test".into()],
                    authid: None,
                    items: vec![
                        PackageItem::Procedure(PackageProcedure {
                            name: vec!["prc_found".into()],
                            parameters: vec![],
                            block: None,
                            start_line: 2,
                            end_line: 2,
                        }),
                        PackageItem::Procedure(PackageProcedure {
                            name: vec!["prc_missing".into()],
                            parameters: vec![],
                            block: None,
                            start_line: 3,
                            end_line: 3,
                        }),
                    ],
                },
                span: None,
            }),
        };

        let body = ogsql_parser::StatementInfo {
            sql_text: String::new(),
            start_line: 7,
            start_col: 0,
            end_line: 20,
            end_col: 0,
            statement: ogsql_parser::ast::Statement::CreatePackageBody(
                ogsql_parser::ast::Spanned {
                    node: CreatePackageBodyStatement {
                        replace: true,
                        name: vec!["pkg_test".into()],
                        items: vec![PackageItem::Procedure(PackageProcedure {
                            name: vec!["prc_found".into()],
                            parameters: vec![],
                            block: Some(ogsql_parser::ast::plpgsql::PlBlock {
                                label: None,
                                declarations: vec![],
                                body: vec![],
                                exception_block: None,
                                end_label: None,
                            }),
                            start_line: 8,
                            end_line: 18,
                        })],
                    },
                    span: None,
                },
            ),
        };

        let files = vec![ParsedFile {
            path: PathBuf::from("test.sql"),
            statements: vec![spec, body],
            content_hash: String::new(),
        }];

        let graph = GraphBuilder::new().build(&files);

        let procs: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::Procedure { .. }))
            .collect();
        assert_eq!(procs.len(), 2, "Expected 2 procedure nodes");

        let found_node = procs
            .iter()
            .find(|i| {
                if let Node::Procedure { id, .. } = &graph[**i] {
                    id.name == "prc_found"
                } else {
                    false
                }
            })
            .expect("prc_found should exist");
        if let Node::Procedure { partial, .. } = &graph[*found_node] {
            assert!(!partial, "prc_found should NOT be partial");
        }

        let missing_node = procs
            .iter()
            .find(|i| {
                if let Node::Procedure { id, .. } = &graph[**i] {
                    id.name == "prc_missing"
                } else {
                    false
                }
            })
            .expect("prc_missing should exist as partial node");
        if let Node::Procedure { partial, .. } = &graph[*missing_node] {
            assert!(partial, "prc_missing SHOULD be partial");
        }

        let pkg_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::Package { .. }))
            .collect();
        assert_eq!(pkg_nodes.len(), 1, "Expected 1 package node");
        let edges_from_pkg: Vec<_> = graph
            .edges_directed(pkg_nodes[0], petgraph::Direction::Outgoing)
            .filter(|e| matches!(e.weight(), Edge::ContainsRoutine))
            .collect();
        assert_eq!(
            edges_from_pkg.len(),
            2,
            "Package should have 2 ContainsRoutine edges"
        );
    }

    #[test]
    fn view_and_table_reference_dedup_case_insensitive() {
        let sql = r#"
            CREATE VIEW BIGFUND.MV_ACCOUNT_PRIV AS
            SELECT * FROM account_priv;

            CREATE OR REPLACE PROCEDURE query_priv() AS $$
            BEGIN
                SELECT * FROM mv_account_priv;
            END;
            $$;
        "#;
        let graph = build_from_sql(sql);

        let view_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::View { name, .. } if name.to_lowercase() == "mv_account_priv"))
            .collect();
        assert_eq!(
            view_nodes.len(),
            1,
            "Expected exactly 1 View node for mv_account_priv"
        );

        let table_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::Table { name, .. } if name.to_lowercase() == "mv_account_priv"))
            .collect();
        assert_eq!(
            table_nodes.len(),
            0,
            "No Table node should be created for mv_account_priv — it's a View"
        );

        let proc_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::Procedure { id, .. } if id.name == "query_priv"))
            .collect();
        assert_eq!(proc_nodes.len(), 1);

        use petgraph::visit::EdgeRef;
        let access_edges: Vec<_> = graph
            .edges_directed(proc_nodes[0], petgraph::Direction::Outgoing)
            .filter(|e| matches!(e.weight(), Edge::TableAccess { .. }))
            .collect();
        assert_eq!(
            access_edges.len(),
            1,
            "Procedure should reference the view via 1 TableAccess edge"
        );

        let target = access_edges[0].target();
        assert!(
            matches!(&graph[target], Node::View { .. }),
            "TableAccess edge should point to the View node, not a Table node"
        );
    }

    #[test]
    fn table_name_case_insensitive_dedup() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURE p1() AS $$
            BEGIN
                INSERT INTO MY_TABLE VALUES (1);
            END;
            $$;

            CREATE OR REPLACE PROCEDURE p2() AS $$
            BEGIN
                DELETE FROM my_table WHERE id = 1;
            END;
            $$;
        "#;
        let graph = build_from_sql(sql);

        let table_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::Table { name, .. } if name.to_lowercase() == "my_table"))
            .collect();
        assert_eq!(
            table_nodes.len(),
            1,
            "MY_TABLE and my_table should resolve to a single Table node"
        );
    }

    #[test]
    fn table_created_before_view_definition_gets_merged() {
        let sql = r#"
            CREATE VIEW other_view AS
            SELECT * FROM v_par_client_acnt_info_noflag;

            CREATE VIEW BIGFUND.v_par_client_acnt_info_noflag AS
            SELECT * FROM account_priv;
        "#;
        let graph = build_from_sql(sql);

        let view_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::View { name, .. } if name.to_lowercase() == "v_par_client_acnt_info_noflag"))
            .collect();
        assert_eq!(
            view_nodes.len(),
            1,
            "Should have exactly 1 View node for v_par_client_acnt_info_noflag"
        );

        let table_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::Table { name, .. } if name.to_lowercase() == "v_par_client_acnt_info_noflag"))
            .collect();
        assert_eq!(
            table_nodes.len(),
            0,
            "Table node for v_par_client_acnt_info_noflag should be merged into the View node"
        );
    }

    #[test]
    fn bare_name_table_merged_into_schema_qualified() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURE p1() AS $$
            BEGIN
                SELECT * FROM aas_account;
            END;
            $$;

            CREATE OR REPLACE PROCEDURE p2() AS $$
            BEGIN
                INSERT INTO bigfund.aas_account VALUES (1);
            END;
            $$;
        "#;
        let graph = build_from_sql(sql);

        let account_tables: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::Table { name, .. } if name.to_lowercase() == "aas_account"))
            .collect();
        assert_eq!(
            account_tables.len(),
            1,
            "aas_account and bigfund.aas_account should resolve to a single Table node"
        );

        if let Node::Table { schema, name, .. } = &graph[account_tables[0]] {
            assert_eq!(name.to_lowercase(), "aas_account");
            assert_eq!(
                schema.as_ref().map(|s| s.to_lowercase()),
                Some("bigfund".to_string()),
                "Should keep the schema-qualified node"
            );
        }
    }

    /// Regression test: a bare Table targeted by both Phase 1 (Table→View merge)
    /// and Phase 2 (bare→schema-qualified merge) caused petgraph panic.
    /// The crash requires multi-chunk processing where table_index dedup
    /// doesn't suppress the bare Table node — this test exercises the code
    /// path to ensure the dedup + defense-in-depth guard work correctly.
    #[test]
    fn bare_table_merged_into_view_and_schema_qualified_no_panic() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURE p1() AS $$
            BEGIN
                SELECT * FROM orders;
            END;
            $$;

            CREATE OR REPLACE PROCEDURE p2() AS $$
            BEGIN
                INSERT INTO bigfund.orders VALUES (1);
            END;
            $$;

            CREATE VIEW orders AS
            SELECT * FROM another_table;
        "#;
        let graph = build_from_sql(sql);

        let view_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::View { name, .. } if name.to_lowercase() == "orders"))
            .collect();
        assert_eq!(view_nodes.len(), 1, "View 'orders' should exist");
    }

    /// Simulates the chunked-processing crash: Phase 1 removes a qualified
    /// Table (merged into View), then Phase 2 tries to merge a bare Table
    /// into that now-removed qualified Table → add_edge panics on dead idx.
    ///
    /// View query table references (unlike procedure body references) do NOT
    /// insert bare-name entries into table_index, so the chunk-1 qualified
    /// Table does not suppress the chunk-2 bare Table creation.
    #[test]
    fn qualified_table_into_view_merged_then_bare_into_qualified() {
        use crate::graph::builder::GraphBuildContext;
        use crate::parser::ParsedFile;
        use std::path::PathBuf;

        let mut ctx = GraphBuildContext::new();

        // Chunk 1: View query creates qualified Table "bigfund.orders"
        // without polluting table_index["orders"] (View query handler
        // lacks the bare-name insertion present in procedure-body handler).
        let file1 = ParsedFile {
            path: PathBuf::from("chunk1.sql"),
            statements: parse_sql("CREATE VIEW v1 AS SELECT * FROM bigfund.orders;"),
            content_hash: String::new(),
        };
        GraphBuilder::build_sql_chunk(&mut ctx, &[file1], false);

        // Chunk 2: View "bigfund.orders" + bare Table "orders" from procedure
        let file2 = ParsedFile {
            path: PathBuf::from("chunk2.sql"),
            statements: parse_sql(
                "\
                CREATE VIEW bigfund.orders AS SELECT * FROM x;\n\
                CREATE OR REPLACE PROCEDURE p2() AS $$\n\
                BEGIN\n\
                    SELECT * FROM orders;\n\
                END;\n\
                $$;",
            ),
            content_hash: String::new(),
        };
        GraphBuilder::build_sql_chunk(&mut ctx, &[file2], false);

        // This would panic before the fix. After the fix, it completes.
        GraphBuilder::finalize_graph(&mut ctx);

        let view_nodes: Vec<_> = ctx
            .graph
            .node_indices()
            .filter(|i| {
                matches!(&ctx.graph[*i], Node::View { schema, name, .. }
                    if *schema == Some("bigfund".to_string()) && name.to_lowercase() == "orders")
            })
            .collect();
        assert_eq!(view_nodes.len(), 1, "View 'bigfund.orders' should exist");
    }

    #[test]
    fn create_table_produces_rich_table_node() {
        let sql = r#"
            CREATE TABLE public.orders (
                id BIGINT NOT NULL PRIMARY KEY,
                amount NUMERIC(10,2) DEFAULT 0,
                status VARCHAR(20),
                created_at TIMESTAMP NOT NULL
            ) PARTITION BY RANGE (created_at)
            DISTRIBUTE BY HASH (id)
            TABLESPACE pg_default;
        "#;
        let graph = build_from_sql(sql);

        let table_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::Table { .. }))
            .collect();

        assert_eq!(table_nodes.len(), 1, "should have exactly one table node");
        let table_node = &graph[table_nodes[0]];

        if let Node::Table {
            schema,
            name,
            location,
            columns,
            partition_by,
            distribute_by,
            tablespace,
            temporary,
            unlogged,
            ..
        } = table_node
        {
            assert_eq!(schema.as_deref(), Some("public"));
            assert_eq!(name, "orders");
            assert!(location.is_some(), "should have source location");
            assert_eq!(columns.len(), 4);
            assert!(columns[0].is_primary_key);
            assert_eq!(columns[0].name, "id");
            assert!(!columns[0].nullable);
            assert!(partition_by.is_some());
            assert!(distribute_by.is_some());
            assert_eq!(tablespace.as_deref(), Some("pg_default"));
            assert!(!temporary);
            assert!(!unlogged);
        } else {
            panic!("expected Table node");
        }
    }

    #[test]
    fn create_table_simple_no_partition() {
        let sql = "CREATE TABLE simple_t (id INT, name TEXT);";
        let graph = build_from_sql(sql);

        let table_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::Table { name, .. } if name == "simple_t"))
            .collect();

        assert_eq!(table_nodes.len(), 1);
        if let Node::Table {
            columns,
            partition_by,
            distribute_by,
            location,
            ..
        } = &graph[table_nodes[0]]
        {
            assert!(location.is_some());
            assert_eq!(columns.len(), 2);
            assert!(partition_by.is_none());
            assert!(distribute_by.is_none());
        } else {
            panic!("expected Table");
        }
    }

    #[test]
    fn create_table_merges_with_implicit_reference() {
        let sql = r#"
            CREATE TABLE public.my_table (id INT PRIMARY KEY);
            CREATE PROCEDURE do_insert() AS BEGIN INSERT INTO my_table(id) VALUES(1); END;
        "#;
        let graph = build_from_sql(sql);

        let table_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::Table { name, .. } if name == "my_table"))
            .collect();

        assert_eq!(table_nodes.len(), 1, "should merge into single table node");
        if let Node::Table {
            columns, location, ..
        } = &graph[table_nodes[0]]
        {
            assert!(
                !columns.is_empty(),
                "merged node should keep columns from CREATE TABLE"
            );
            assert!(
                location.is_some(),
                "merged node should have location from CREATE TABLE"
            );
        }
    }

    #[test]
    fn unresolved_call_to_function_resolved_via_kind_swap() {
        let sql = r#"
            CREATE OR REPLACE FUNCTION BIGFUND.get_par_fund_info_a(p_id INT)
            RETURNS VARCHAR AS $$
            BEGIN
                RETURN 'test';
            END;
            $$;

            CREATE OR REPLACE PACKAGE BODY BIGFUND.PKG_TRD_BALANCE_ACCRUAL AS
                PROCEDURE prc_trd_ej_listquery_zh IS
                BEGIN
                    BIGFUND.get_par_fund_info_a(1);
                END;
            END;
        "#;
        let graph = build_from_sql(sql);

        let unresolved: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::Unresolved { .. }))
            .collect();
        assert!(
            unresolved.is_empty(),
            "Expected no unresolved nodes, but found: {:?}",
            unresolved
                .iter()
                .map(|i| match &graph[*i] {
                    Node::Unresolved { raw_expr, .. } => (**raw_expr).clone(),
                    _ => String::new(),
                })
                .collect::<Vec<_>>()
        );

        let func_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::Function { id, .. } if id.name == "get_par_fund_info_a"))
            .collect();
        assert_eq!(func_nodes.len(), 1, "Expected exactly 1 Function node");

        let proc_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::Procedure { id, .. } if id.name == "prc_trd_ej_listquery_zh"))
            .collect();
        assert_eq!(proc_nodes.len(), 1, "Expected exactly 1 Procedure node");

        let call_edges: Vec<_> = graph
            .edge_indices()
            .filter(|e| matches!(&graph[*e], Edge::DirectCall { .. }))
            .collect();
        assert_eq!(
            call_edges.len(),
            1,
            "Expected 1 DirectCall edge from procedure to function"
        );

        let (src, dst) = graph.edge_endpoints(call_edges[0]).unwrap();
        assert_eq!(
            src, proc_nodes[0],
            "Call should originate from prc_trd_ej_listquery_zh"
        );
        assert_eq!(
            dst, func_nodes[0],
            "Call should target get_par_fund_info_a function"
        );
    }

    #[test]
    fn unresolved_call_case_insensitive_resolved() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURE BIGFUND.MT_541_CREATE AS $$
            BEGIN
                NULL;
            END;
            $$;

            CREATE OR REPLACE PACKAGE BODY BIGFUND.PKG_INST_CONTROL AS
                PROCEDURE proc_inst_mt_gen IS
                BEGIN
                    mt_541_create;
                END;
            END;
        "#;
        let graph = build_from_sql(sql);

        let unresolved: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::Unresolved { .. }))
            .collect();
        assert!(
            unresolved.is_empty(),
            "Expected no unresolved nodes (case-insensitive match should resolve), found: {:?}",
            unresolved
                .iter()
                .map(|i| match &graph[*i] {
                    Node::Unresolved { raw_expr, .. } => (**raw_expr).clone(),
                    _ => String::new(),
                })
                .collect::<Vec<_>>()
        );

        let proc_create: Vec<_> = graph
            .node_indices()
            .filter(
                |i| matches!(&graph[*i], Node::Procedure { id, .. } if id.name == "mt_541_create"),
            )
            .collect();
        assert_eq!(proc_create.len(), 1, "Expected mt_541_create procedure");
    }

    #[test]
    fn noise_unresolved_self_reference_removed() {
        let sql = r#"
            CREATE OR REPLACE PACKAGE BODY BIGFUND.OBJ_ACCOUNT_RECORDSET2 AS
                PROCEDURE add_set_member IS
                BEGIN
                    SELF.account_record_entries(1);
                END;
            END;
        "#;
        let graph = build_from_sql(sql);

        let unresolved: Vec<_> = graph
            .node_indices()
            .filter(|i| {
                matches!(&graph[*i], Node::Unresolved { raw_expr, .. }
                if raw_expr.contains("SELF."))
            })
            .collect();
        assert!(
            unresolved.is_empty(),
            "SELF.xxx unresolved nodes should be filtered as noise"
        );
    }

    #[test]
    fn noise_unresolved_system_function_removed() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURE BIGFUND.JOB_MAP_OBJECT.submit_by_scheduler AS $$
            BEGIN
                DBE_SCHEDULER.enable('my_job');
            END;
            $$;
        "#;
        let graph = build_from_sql(sql);

        let unresolved: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::Unresolved { .. }))
            .collect();
        assert!(
            unresolved.is_empty(),
            "DBE_SCHEDULER.enable unresolved node should be filtered as system call"
        );
    }

    #[test]
    fn noise_filter_recognizes_dbe_xmldom_builtins() {
        assert!(super::noise_rule("dbe_xmldom.setattribute").is_some());
        assert!(super::noise_rule("dbe_xmldom.appendchild").is_some());
        assert!(super::noise_rule("DBE_XMLDOM.SETATTRIBUTE").is_some());
    }

    #[test]
    fn noise_filter_preserves_prior_system_package_coverage() {
        assert!(super::noise_rule("dbe_scheduler.enable").is_some());
        assert!(super::noise_rule("dbe_output.put_line").is_some());
        assert!(super::noise_rule("DBE_OUTPUT.PUT_LINE").is_some());
    }

    #[test]
    fn noise_filter_keeps_user_routines_unfiltered() {
        assert!(super::noise_rule("calc_total").is_none());
        assert!(super::noise_rule("biz.calc_total").is_none());
        assert!(super::noise_rule("my_pkg.do_work").is_none());
    }

    #[test]
    fn noise_rule_returns_reason_for_each_category() {
        assert_eq!(super::noise_rule("PlVariable(x)"), Some("ast-debug-string"));
        assert_eq!(super::noise_rule("SELF.foo"), Some("self-member"));
        assert_eq!(
            super::noise_rule("mylist.EXTEND"),
            Some("collection-method")
        );
        assert_eq!(
            super::noise_rule("dbe_output.put_line"),
            Some("builtin-function")
        );
        assert_eq!(super::noise_rule("calc_total"), None);
    }

    #[test]
    fn raise_statement_does_not_spawn_unresolved() {
        let sql = r#"
            CREATE PROCEDURE log_warn AS $$
            BEGIN
                RAISE NOTICE 'started';
                RAISE WARNING 'risky value: %', 42;
            END;
            $$;
        "#;
        let graph = build_from_sql(sql);
        let unresolved: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::Unresolved { .. }))
            .collect();
        assert!(
            unresolved.is_empty(),
            "RAISE statements must not spawn unresolved nodes: {:?}",
            unresolved
                .iter()
                .map(|i| format!("{:?}", graph[*i]))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn dbe_xmldom_calls_not_unresolved_in_build() {
        let sql = r#"
            CREATE PROCEDURE build_xml_doc AS $$
            DECLARE
                l_doc  INTEGER;
                l_elem INTEGER;
            BEGIN
                l_doc  := dbe_xmldom.newdomdocument();
                l_elem := dbe_xmldom.createelement(l_doc, 'root');
                dbe_xmldom.setattribute(l_elem, 'id', '1');
                dbe_xmldom.appendchild(l_doc, l_elem);
            END;
            $$;
        "#;
        let graph = build_from_sql(sql);
        let unresolved: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::Unresolved { .. }))
            .collect();
        let details: Vec<_> = unresolved
            .iter()
            .map(|i| format!("{:?}", graph[*i]))
            .collect();
        assert!(
            unresolved.is_empty(),
            "dbe_xmldom builtins must not spawn unresolved nodes: {details:?}"
        );
    }

    #[test]
    fn duplicate_ibatis_files_produce_single_mapper_node() {
        use crate::graph::builder::GraphBuildContext;
        use ogsql_parser::ibatis::{ParsedMapper, ParsedStatement, StatementKind};

        let stmt = ParsedStatement {
            id: "findById".to_string(),
            kind: StatementKind::Select,
            parameter_type: None,
            result_type: None,
            flat_sql: "SELECT * FROM users WHERE id = #{id}".to_string(),
            parameters: vec![],
            has_dynamic_elements: false,
            line: 5,
            body_start_line: 5,
            parse_result: None,
            database_id: None,
            statement_type: None,
        };

        let make_parsed_file = |path_str: &str| crate::parser::ibatis_loader::IbatisParsedFile {
            path: PathBuf::from(path_str),
            result: ParsedMapper {
                file_path: Some(path_str.to_string()),
                namespace: "com.example.UserMapper".to_string(),
                statements: vec![stmt.clone()],
                errors: vec![],
            },
            content_hash: "abc".to_string(),
        };

        let ibatis_files = vec![
            make_parsed_file("/a/UserMapper.xml"),
            make_parsed_file("/b/UserMapper.xml"),
        ];

        let mut ctx = GraphBuildContext::new();
        GraphBuilder::add_ibatis_nodes_from_parsed(
            &ibatis_files,
            &mut ctx.graph,
            &mut ctx.proc_index,
            &mut ctx.mapper_index,
            &mut ctx.table_index,
            &mut ctx.builtin_index,
        );

        let mapper_nodes: Vec<_> = ctx
            .graph
            .node_indices()
            .filter(|i| {
                matches!(
                    &ctx.graph[*i],
                    Node::MappedStatement { statement_id, namespace, .. }
                    if *statement_id == "findById" && *namespace == "com.example.UserMapper"
                )
            })
            .collect();

        assert_eq!(
            mapper_nodes.len(),
            1,
            "duplicate ibatis files with same namespace.statement_id should produce exactly 1 MappedStatement node, got {}",
            mapper_nodes.len()
        );
    }

    #[test]
    fn builtin_function_captured_from_mapper_sql() {
        use crate::graph::builder::GraphBuildContext;
        use ogsql_parser::ibatis::{ParsedMapper, ParsedStatement, StatementKind};

        // SQL containing a builtin aggregate function
        let sql = "SELECT COUNT(*) FROM orders";
        // Parse it to get StatementInfo carrying builtin metadata (ogsql-parser v0.8.11 tags builtins during parsing)
        let statements = parse_sql(sql);

        let stmt = ParsedStatement {
            id: "countOrders".to_string(),
            kind: StatementKind::Select,
            parameter_type: None,
            result_type: None,
            flat_sql: sql.to_string(),
            parameters: vec![],
            has_dynamic_elements: false,
            line: 5,
            body_start_line: 5,
            parse_result: Some((statements, vec![])),
            database_id: None,
            statement_type: None,
        };

        let ibatis_file = crate::parser::ibatis_loader::IbatisParsedFile {
            path: PathBuf::from("/mapper/OrderMapper.xml"),
            result: ParsedMapper {
                file_path: Some("/mapper/OrderMapper.xml".to_string()),
                namespace: "com.example.OrderMapper".to_string(),
                statements: vec![stmt],
                errors: vec![],
            },
            content_hash: "abc".to_string(),
        };

        let mut ctx = GraphBuildContext::new();
        GraphBuilder::add_ibatis_nodes_from_parsed(
            std::slice::from_ref(&ibatis_file),
            &mut ctx.graph,
            &mut ctx.proc_index,
            &mut ctx.mapper_index,
            &mut ctx.table_index,
            &mut ctx.builtin_index,
        );

        // Assert a BuiltinFunction node named "count" exists (case-insensitive)
        let has_count = ctx.graph.node_weights().any(|n| {
            matches!(n, Node::BuiltinFunction { name, .. } if name.eq_ignore_ascii_case("count"))
        });
        assert!(has_count, "expected a BuiltinFunction node for COUNT");

        // Assert a UsesBuiltinFunction edge connects the mapper to the builtin
        let has_edge = ctx
            .graph
            .edge_weights()
            .any(|e| matches!(e, Edge::UsesBuiltinFunction { .. }));
        assert!(
            has_edge,
            "expected a UsesBuiltinFunction edge from the mapper"
        );
    }

    #[test]
    fn duplicate_java_files_produce_single_javasql_node() {
        use crate::graph::builder::GraphBuildContext;
        use ogsql_parser::java::{
            ExtractedSql, ExtractionMethod, JavaExtractResult, ParameterStyle, SqlKind, SqlOrigin,
        };

        let extraction = ExtractedSql {
            sql: "SELECT * FROM users".to_string(),
            origin: SqlOrigin {
                method: ExtractionMethod::Annotation,
                class_name: Some("UserDao".to_string()),
                method_name: Some("findAll".to_string()),
                annotation_name: None,
                api_method_name: None,
                variable_name: None,
                line: 10,
                column: 0,
            },
            sql_kind: SqlKind::NativeSql,
            parameter_style: ParameterStyle::None,
            is_concatenated: false,
            is_text_block: false,
            parse_result: None,
        };

        let make_java_file =
            |path_str: &str, file_path: &str| crate::parser::java_loader::JavaParsedFile {
                path: PathBuf::from(path_str),
                result: JavaExtractResult {
                    file_path: file_path.to_string(),
                    extractions: vec![extraction.clone()],
                    errors: vec![],
                },
                content_hash: "abc".to_string(),
            };

        // Same file_path inside result — simulates same file scanned twice via overlapping dirs
        let java_files = vec![
            make_java_file("/a/UserDao.java", "/src/UserDao.java"),
            make_java_file("/b/UserDao.java", "/src/UserDao.java"),
        ];

        let mut ctx = GraphBuildContext::new();
        GraphBuilder::add_java_nodes_from_parsed(
            &java_files,
            &mut ctx.graph,
            &mut ctx.proc_index,
            &ctx.mapper_index,
            &mut ctx.table_index,
            &mut ctx.builtin_index,
        );

        let javasql_nodes: Vec<_> = ctx
            .graph
            .node_indices()
            .filter(|i| {
                matches!(
                    &ctx.graph[*i],
                    Node::JavaSql { class_name, method_name, line, .. }
                    if *class_name == Some("UserDao".to_string())
                        && *method_name == Some("findAll".to_string())
                        && *line == 10
                )
            })
            .collect();

        assert_eq!(
            javasql_nodes.len(),
            1,
            "duplicate Java extractions with same file+line should produce exactly 1 JavaSql node, got {}",
            javasql_nodes.len()
        );
    }

    #[test]
    fn builtin_function_captured_from_java_sql() {
        use crate::graph::builder::GraphBuildContext;
        use ogsql_parser::java::{
            ExtractedSql, ExtractionMethod, JavaExtractResult, ParameterStyle, SqlKind, SqlOrigin,
            SqlParseResult,
        };

        let sql = "SELECT SUBSTR(name, 1, 3) FROM users";
        let statements = parse_sql(sql);

        let extraction = ExtractedSql {
            sql: sql.to_string(),
            origin: SqlOrigin {
                method: ExtractionMethod::Annotation,
                class_name: Some("UserDao".to_string()),
                method_name: Some("findUsers".to_string()),
                annotation_name: None,
                api_method_name: None,
                variable_name: None,
                line: 10,
                column: 0,
            },
            sql_kind: SqlKind::NativeSql,
            parameter_style: ParameterStyle::None,
            is_concatenated: false,
            is_text_block: false,
            parse_result: Some(SqlParseResult {
                statements,
                errors: vec![],
            }),
        };

        let java_file = crate::parser::java_loader::JavaParsedFile {
            path: PathBuf::from("/dao/UserDao.java"),
            result: JavaExtractResult {
                file_path: "/src/UserDao.java".to_string(),
                extractions: vec![extraction],
                errors: vec![],
            },
            content_hash: "abc".to_string(),
        };

        let mut ctx = GraphBuildContext::new();
        GraphBuilder::add_java_nodes_from_parsed(
            std::slice::from_ref(&java_file),
            &mut ctx.graph,
            &mut ctx.proc_index,
            &ctx.mapper_index,
            &mut ctx.table_index,
            &mut ctx.builtin_index,
        );

        let has_substr = ctx.graph.node_weights().any(|n| {
            matches!(n, Node::BuiltinFunction { name, .. } if name.eq_ignore_ascii_case("substr"))
        });
        assert!(has_substr, "expected a BuiltinFunction node for SUBSTR");

        // Assert a UsesBuiltinFunction edge connects the JavaSql node to the builtin
        let has_edge = ctx
            .graph
            .edge_weights()
            .any(|e| matches!(e, Edge::UsesBuiltinFunction { .. }));
        assert!(
            has_edge,
            "expected a UsesBuiltinFunction edge from the JavaSql node"
        );
    }

    #[test]
    #[cfg(feature = "jsp")]
    fn builtin_function_captured_from_jsp_sql() {
        use crate::graph::builder::GraphBuildContext;
        use crate::parser::jsp_loader::load_jsp_string;
        use ogsql_parser::java::JavaExtractConfig;

        let jsp_source = r#"<%@ page import="java.sql.*" %>
<%
Connection conn = DriverManager.getConnection("jdbc:default:connection");
Statement stmt = conn.createStatement();
ResultSet rs = stmt.executeQuery("SELECT COUNT(*) FROM products");
%>"#;
        let config = JavaExtractConfig {
            extra_sql_methods: vec![],
            extra_sql_var_patterns: vec![],
        };
        let jsp_result = load_jsp_string(
            jsp_source.to_string(),
            std::path::Path::new("/web/products.jsp"),
            &config,
        );

        let mut ctx = GraphBuildContext::new();
        GraphBuilder::add_jsp_nodes_from_parsed(std::slice::from_ref(&jsp_result), &mut ctx);

        let has_count = ctx.graph.node_weights().any(|n| {
            matches!(n, Node::BuiltinFunction { name, .. } if name.eq_ignore_ascii_case("count"))
        });
        assert!(has_count, "expected a BuiltinFunction node for COUNT");

        let has_edge = ctx
            .graph
            .edge_weights()
            .any(|e| matches!(e, Edge::UsesBuiltinFunction { .. }));
        assert!(
            has_edge,
            "expected a UsesBuiltinFunction edge from the JspSql node"
        );
    }

    #[test]
    fn builtin_node_cross_path_dedup() {
        use crate::graph::builder::GraphBuildContext;
        use crate::parser::ParsedFile;
        use ogsql_parser::ibatis::{ParsedMapper, ParsedStatement, StatementKind};

        let proc_sql = r#"
            CREATE PROCEDURE use_count AS $$
            BEGIN
                PERFORM COUNT(*) FROM dual;
            END;
            $$;
        "#;
        let parsed_sql = vec![ParsedFile {
            path: PathBuf::from("proc.sql"),
            statements: parse_sql(proc_sql),
            content_hash: String::new(),
        }];

        let mapper_sql = "SELECT COUNT(*) FROM orders";
        let mapper_stmt = ParsedStatement {
            id: "countOrders".to_string(),
            kind: StatementKind::Select,
            parameter_type: None,
            result_type: None,
            flat_sql: mapper_sql.to_string(),
            parameters: vec![],
            has_dynamic_elements: false,
            line: 5,
            body_start_line: 5,
            parse_result: Some((parse_sql(mapper_sql), vec![])),
            database_id: None,
            statement_type: None,
        };
        let ibatis_file = crate::parser::ibatis_loader::IbatisParsedFile {
            path: PathBuf::from("/mapper/OrderMapper.xml"),
            result: ParsedMapper {
                file_path: Some("/mapper/OrderMapper.xml".to_string()),
                namespace: "com.example.OrderMapper".to_string(),
                statements: vec![mapper_stmt],
                errors: vec![],
            },
            content_hash: "abc".to_string(),
        };

        let mut ctx = GraphBuildContext::new();
        GraphBuilder::build_sql_chunk(&mut ctx, &parsed_sql, false);
        GraphBuilder::add_ibatis_nodes_from_parsed(
            std::slice::from_ref(&ibatis_file),
            &mut ctx.graph,
            &mut ctx.proc_index,
            &mut ctx.mapper_index,
            &mut ctx.table_index,
            &mut ctx.builtin_index,
        );

        let count_nodes = ctx.graph.node_weights().filter(|n| {
            matches!(n, Node::BuiltinFunction { name, .. } if name.eq_ignore_ascii_case("count"))
        }).count();
        assert_eq!(
            count_nodes, 1,
            "cross-path dedup: one COUNT node shared between proc and mapper"
        );

        let builtin_edges = ctx
            .graph
            .edge_weights()
            .filter(|e| matches!(e, Edge::UsesBuiltinFunction { .. }))
            .count();
        assert_eq!(
            builtin_edges, 2,
            "two callers (proc + mapper) should produce two UsesBuiltinFunction edges"
        );
    }

    #[test]
    fn builtin_edge_dedup_within_single_statement() {
        use crate::graph::builder::GraphBuildContext;
        use ogsql_parser::ibatis::{ParsedMapper, ParsedStatement, StatementKind};

        let sql = "SELECT COUNT(*) + COUNT(*) FROM orders";
        let stmt = ParsedStatement {
            id: "doubleCount".to_string(),
            kind: StatementKind::Select,
            parameter_type: None,
            result_type: None,
            flat_sql: sql.to_string(),
            parameters: vec![],
            has_dynamic_elements: false,
            line: 5,
            body_start_line: 5,
            parse_result: Some((parse_sql(sql), vec![])),
            database_id: None,
            statement_type: None,
        };
        let ibatis_file = crate::parser::ibatis_loader::IbatisParsedFile {
            path: PathBuf::from("/mapper/OrderMapper.xml"),
            result: ParsedMapper {
                file_path: Some("/mapper/OrderMapper.xml".to_string()),
                namespace: "com.example.OrderMapper".to_string(),
                statements: vec![stmt],
                errors: vec![],
            },
            content_hash: "abc".to_string(),
        };

        let mut ctx = GraphBuildContext::new();
        GraphBuilder::add_ibatis_nodes_from_parsed(
            std::slice::from_ref(&ibatis_file),
            &mut ctx.graph,
            &mut ctx.proc_index,
            &mut ctx.mapper_index,
            &mut ctx.table_index,
            &mut ctx.builtin_index,
        );

        let count_nodes = ctx.graph.node_weights().filter(|n| {
            matches!(n, Node::BuiltinFunction { name, .. } if name.eq_ignore_ascii_case("count"))
        }).count();
        assert_eq!(count_nodes, 1, "one COUNT node");

        let builtin_edges = ctx
            .graph
            .edge_weights()
            .filter(|e| matches!(e, Edge::UsesBuiltinFunction { .. }))
            .count();
        assert_eq!(
            builtin_edges, 1,
            "COUNT appearing twice in one statement should produce one edge (dedup)"
        );
    }

    #[test]
    fn duplicate_java_classes_produce_single_class_node() {
        use crate::graph::builder::GraphBuildContext;
        use crate::parser::java_method::{JavaClassInfo, JavaParseResult};

        let class = JavaClassInfo {
            fqn: "com.example.UserDao".to_string(),
            name: "UserDao".to_string(),
            package: "com.example".to_string(),
            extends: None,
            implements: vec![],
            file: PathBuf::from("/a/UserDao.java"),
            line: 3,
        };

        let make_result = |path_str: &str| JavaParseResult {
            file: PathBuf::from(path_str),
            package: "com.example".to_string(),
            imports: vec![],
            classes: vec![class.clone()],
            methods: vec![],
            di_injections: vec![],
            content_hash: "abc".to_string(),
        };

        let results = vec![
            make_result("/a/UserDao.java"),
            make_result("/b/UserDao.java"),
        ];

        let mut ctx = GraphBuildContext::new();
        GraphBuilder::add_java_method_nodes_from_parsed(
            &results,
            &mut ctx.graph,
            &mut ctx.proc_index,
            &ctx.mapper_index,
        );

        let class_nodes: Vec<_> = ctx
            .graph
            .node_indices()
            .filter(|i| {
                matches!(
                    &ctx.graph[*i],
                    Node::JavaClass { fqn, .. } if fqn == "com.example.UserDao"
                )
            })
            .collect();

        assert_eq!(
            class_nodes.len(),
            1,
            "duplicate JavaClassInfo with same FQN should produce exactly 1 JavaClass node, got {}",
            class_nodes.len()
        );
    }

    #[test]
    fn function_call_in_expression_creates_edge() {
        let sql = r#"
            CREATE FUNCTION calc_total(p INT) RETURNS INTEGER AS $$
            BEGIN RETURN p * 2; END;
            $$;
            CREATE PROCEDURE process_order AS $$
            DECLARE v INT;
            BEGIN
                v := calc_total(1);
            END;
            $$;
        "#;
        let graph = build_from_sql(sql);

        let func_idx = graph
            .node_indices()
            .find(|i| matches!(&graph[*i], Node::Function { id, .. } if id.name == "calc_total"))
            .expect("calc_total Function node should exist");

        let proc_idx = graph
            .node_indices()
            .find(
                |i| matches!(&graph[*i], Node::Procedure { id, .. } if id.name == "process_order"),
            )
            .expect("process_order Procedure node should exist");

        let has_edge = graph.edge_indices().any(|e| {
            let (src, dst) = graph.edge_endpoints(e).unwrap();
            src == proc_idx && dst == func_idx && matches!(&graph[e], Edge::DirectCall { .. })
        });
        assert!(
            has_edge,
            "Expected DirectCall edge from process_order to calc_total"
        );
    }

    #[test]
    fn function_call_via_perform_creates_edge() {
        let sql = r#"
            CREATE FUNCTION bar() RETURNS INTEGER AS $$
            BEGIN RETURN 1; END;
            $$;
            CREATE PROCEDURE foo() AS $$
            BEGIN
                PERFORM bar();
            END;
            $$;
        "#;
        let graph = build_from_sql(sql);

        let bar_idx = graph
            .node_indices()
            .find(|i| matches!(&graph[*i], Node::Function { id, .. } if id.name == "bar"))
            .expect("bar Function node should exist");

        let has_edge = graph.edge_indices().any(|e| {
            let (_, dst) = graph.edge_endpoints(e).unwrap();
            dst == bar_idx && matches!(&graph[e], Edge::DirectCall { .. })
        });
        assert!(has_edge, "Expected DirectCall edge to bar via PERFORM");
    }

    #[test]
    fn builtin_function_not_captured_as_call() {
        let sql = r#"
            CREATE PROCEDURE aggregate_data AS $$
            BEGIN
                PERFORM COUNT(*) FROM dual;
            END;
            $$;
        "#;
        let graph = build_from_sql(sql);

        let call_edges: Vec<_> = graph
            .edge_indices()
            .filter(|e| {
                matches!(
                    &graph[*e],
                    Edge::DirectCall { .. } | Edge::DynamicCall { .. }
                )
            })
            .collect();
        assert!(
            call_edges.is_empty(),
            "Built-in COUNT should not create call edges"
        );
    }

    #[test]
    fn builtin_function_captured_as_node() {
        let sql = r#"
            CREATE PROCEDURE aggregate_data AS $$
            BEGIN
                PERFORM COUNT(*) FROM dual;
            END;
            $$;
        "#;
        let graph = build_from_sql(sql);

        // Assert a BuiltinFunction node exists for COUNT
        let builtin_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| {
                matches!(
                    &graph[*i],
                    Node::BuiltinFunction { name, .. } if name.eq_ignore_ascii_case("count")
                )
            })
            .collect();
        assert_eq!(
            builtin_nodes.len(),
            1,
            "Expected exactly one BuiltinFunction node for 'count'"
        );

        // Verify category and domain are populated (from AST builtin meta)
        let builtin_idx = builtin_nodes[0];
        match &graph[builtin_idx] {
            Node::BuiltinFunction {
                category, domain, ..
            } => {
                assert!(!category.is_empty(), "category should not be empty");
                assert!(!domain.is_empty(), "domain should not be empty");
            }
            _ => panic!("expected BuiltinFunction node"),
        }

        // Assert a UsesBuiltinFunction edge exists from the procedure
        let has_uses_edge = graph
            .edge_indices()
            .any(|e| matches!(&graph[e], Edge::UsesBuiltinFunction { .. }));
        assert!(
            has_uses_edge,
            "Expected at least one UsesBuiltinFunction edge"
        );

        // Verify the edge connects the procedure to the builtin node
        let procedure_idx = graph
            .node_indices()
            .find(
                |i| matches!(&graph[*i], Node::Procedure { id, .. } if id.name == "aggregate_data"),
            )
            .expect("Procedure node aggregate_data should exist");
        let edge_connects_proc_to_builtin = graph.edge_indices().any(|e| {
            let (src, dst) = graph.edge_endpoints(e).unwrap();
            src == procedure_idx
                && dst == builtin_idx
                && matches!(&graph[e], Edge::UsesBuiltinFunction { .. })
        });
        assert!(
            edge_connects_proc_to_builtin,
            "Expected UsesBuiltinFunction edge from aggregate_data to count"
        );
    }

    #[test]
    fn procedure_call_builtin_captured_as_node() {
        let sql = r#"
            CREATE PROCEDURE log_msg AS $$
            BEGIN
                dbe_output.put_line('hello');
            END;
            $$;
        "#;
        let graph = build_from_sql(sql);

        let builtin_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| {
                matches!(
                    &graph[*i],
                    Node::BuiltinFunction { name, .. }
                        if name.eq_ignore_ascii_case("dbe_output.put_line")
                )
            })
            .collect();
        assert_eq!(
            builtin_nodes.len(),
            1,
            "Expected one BuiltinFunction node for dbe_output.put_line (procedure call path)"
        );

        match &graph[builtin_nodes[0]] {
            Node::BuiltinFunction {
                category, domain, ..
            } => {
                assert!(!category.is_empty(), "category should not be empty");
                assert!(!domain.is_empty(), "domain should not be empty");
            }
            _ => panic!("expected BuiltinFunction node"),
        }

        let proc_idx = graph
            .node_indices()
            .find(|i| matches!(&graph[*i], Node::Procedure { id, .. } if id.name == "log_msg"))
            .expect("Procedure log_msg should exist");
        let has_edge = graph.edge_indices().any(|e| {
            let (src, dst) = graph.edge_endpoints(e).unwrap();
            src == proc_idx
                && dst == builtin_nodes[0]
                && matches!(&graph[e], Edge::UsesBuiltinFunction { .. })
        });
        assert!(
            has_edge,
            "Expected UsesBuiltinFunction edge from log_msg to dbe_output.put_line"
        );
    }

    #[test]
    fn operator_any_creates_builtin_node() {
        let sql = "CREATE OR REPLACE PROCEDURE proc_any() AS $$
        BEGIN
            FOR r IN (SELECT * FROM t1 WHERE col > ANY(SELECT col FROM t2)) LOOP
                NULL;
            END LOOP;
        END;
        $$ LANGUAGE plpgsql;";
        let graph = build_from_sql(sql);
        let any_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| {
                matches!(&graph[*i], Node::BuiltinFunction { name, category, .. }
                    if name.eq_ignore_ascii_case("any") && category == "Operator")
            })
            .collect();
        assert_eq!(
            any_nodes.len(),
            1,
            "expected one BuiltinFunction node for ANY"
        );
        let has_edge = graph
            .edge_indices()
            .any(|e| matches!(&graph[e], Edge::UsesBuiltinFunction { .. }));
        assert!(has_edge, "expected UsesBuiltinFunction edge");
    }

    #[test]
    fn operator_exists_creates_builtin_node() {
        let sql = "CREATE OR REPLACE PROCEDURE proc_exists() AS $$
        BEGIN
            FOR r IN (SELECT * FROM t1 WHERE EXISTS(SELECT 1 FROM t2 WHERE t2.id = t1.id)) LOOP
                NULL;
            END LOOP;
        END;
        $$ LANGUAGE plpgsql;";
        let graph = build_from_sql(sql);
        let exists_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| {
                matches!(&graph[*i], Node::BuiltinFunction { name, category, domain, .. }
                    if name.eq_ignore_ascii_case("exists")
                    && category == "Operator"
                    && domain == "Predicate")
            })
            .collect();
        assert_eq!(
            exists_nodes.len(),
            1,
            "expected one BuiltinFunction node for EXISTS"
        );
    }

    #[test]
    fn operator_in_subquery_creates_builtin_node() {
        let sql = "CREATE OR REPLACE PROCEDURE proc_in() AS $$
        BEGIN
            FOR r IN (SELECT * FROM t1 WHERE col IN (SELECT col FROM t2)) LOOP
                NULL;
            END LOOP;
        END;
        $$ LANGUAGE plpgsql;";
        let graph = build_from_sql(sql);
        let in_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| {
                matches!(&graph[*i], Node::BuiltinFunction { name, domain, .. }
                    if name.eq_ignore_ascii_case("in") && domain == "Predicate")
            })
            .collect();
        assert_eq!(
            in_nodes.len(),
            1,
            "expected one BuiltinFunction node for IN"
        );
    }

    #[test]
    fn operator_all_creates_builtin_node() {
        let sql = "CREATE OR REPLACE PROCEDURE proc_all() AS $$
        BEGIN
            FOR r IN (SELECT * FROM t1 WHERE col > ALL(SELECT col FROM t2)) LOOP
                NULL;
            END LOOP;
        END;
        $$ LANGUAGE plpgsql;";
        let graph = build_from_sql(sql);
        let all_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| {
                matches!(&graph[*i], Node::BuiltinFunction { name, domain, .. }
                    if name.eq_ignore_ascii_case("all") && domain == "Comparison")
            })
            .collect();
        assert_eq!(
            all_nodes.len(),
            1,
            "expected one BuiltinFunction node for ALL"
        );
    }

    #[test]
    fn operator_some_creates_builtin_node() {
        let sql = "CREATE OR REPLACE PROCEDURE proc_some() AS $$
        BEGIN
            FOR r IN (SELECT * FROM t1 WHERE col = SOME(SELECT col FROM t2)) LOOP
                NULL;
            END LOOP;
        END;
        $$ LANGUAGE plpgsql;";
        let graph = build_from_sql(sql);
        let some_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| {
                matches!(&graph[*i], Node::BuiltinFunction { name, .. }
                    if name.eq_ignore_ascii_case("some"))
            })
            .collect();
        assert_eq!(
            some_nodes.len(),
            1,
            "expected one BuiltinFunction node for SOME"
        );
        let any_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| {
                matches!(&graph[*i], Node::BuiltinFunction { name, .. }
                    if name.eq_ignore_ascii_case("any"))
            })
            .collect();
        assert!(any_nodes.is_empty(), "SOME should NOT create an ANY node");
    }

    #[test]
    fn operator_not_in_creates_builtin_node() {
        let sql = "CREATE OR REPLACE PROCEDURE proc_not_in() AS $$
        BEGIN
            FOR r IN (SELECT * FROM t1 WHERE col NOT IN (SELECT col FROM t2)) LOOP
                NULL;
            END LOOP;
        END;
        $$ LANGUAGE plpgsql;";
        let graph = build_from_sql(sql);
        let not_in_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| {
                matches!(&graph[*i], Node::BuiltinFunction { name, domain, .. }
                    if name.eq_ignore_ascii_case("not_in") && domain == "Predicate")
            })
            .collect();
        assert_eq!(
            not_in_nodes.len(),
            1,
            "expected one BuiltinFunction node for NOT_IN"
        );
    }

    #[test]
    fn operator_extraction_does_not_break_function_call() {
        let sql = "CREATE OR REPLACE PROCEDURE proc_with_count() AS $$
        BEGIN
            PERFORM COUNT(*) FROM dual;
        END;
        $$ LANGUAGE plpgsql;";
        let graph = build_from_sql(sql);
        let count_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| {
                matches!(&graph[*i], Node::BuiltinFunction { name, .. }
                    if name.eq_ignore_ascii_case("count"))
            })
            .collect();
        assert_eq!(
            count_nodes.len(),
            1,
            "COUNT should still be extracted as BuiltinFunction"
        );
        for idx in graph.node_indices() {
            if let Node::BuiltinFunction { name, category, .. } = &graph[idx] {
                if category == "Operator" {
                    panic!(
                        "unexpected Operator node '{}' from FunctionCall-only SQL",
                        name
                    );
                }
            }
        }
    }

    #[test]
    fn hint_tablescan_creates_builtin_node() {
        let sql = "CREATE OR REPLACE PROCEDURE proc_hint() AS $$
        BEGIN
            FOR r IN (SELECT /*+ tablescan(t1) */ * FROM t1) LOOP
                NULL;
            END LOOP;
        END;
        $$ LANGUAGE plpgsql;";
        let graph = build_from_sql(sql);
        let hint_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| {
                matches!(&graph[*i], Node::BuiltinFunction { name, category, .. }
                    if name.eq_ignore_ascii_case("tablescan") && category == "Hint")
            })
            .collect();
        assert_eq!(
            hint_nodes.len(),
            1,
            "expected one BuiltinFunction node for tablescan hint"
        );
    }

    #[test]
    fn hint_nestloop_creates_builtin_node() {
        let sql = "CREATE OR REPLACE PROCEDURE proc_hints() AS $$
        BEGIN
            FOR r IN (SELECT /*+ nestloop(t1 t2) */ * FROM t1 JOIN t2 ON t1.id = t2.id) LOOP
                NULL;
            END LOOP;
        END;
        $$ LANGUAGE plpgsql;";
        let graph = build_from_sql(sql);
        let nestloop: Vec<_> = graph
            .node_indices()
            .filter(
                |i| matches!(&graph[*i], Node::BuiltinFunction { name, .. } if name == "nestloop"),
            )
            .collect();
        assert_eq!(nestloop.len(), 1, "expected nestloop node");
    }

    #[test]
    fn multi_hint_use_cplan_indexscan_not_combined() {
        let sql = "CREATE OR REPLACE PROCEDURE p() AS $$
        BEGIN
            FOR r IN (SELECT /*+ use_cplan indexscan(t1) */ * FROM t1) LOOP NULL; END LOOP;
        END;
        $$ LANGUAGE plpgsql;";
        let graph = build_from_sql(sql);
        let use_cplan: Vec<_> = graph
            .node_indices()
            .filter(
                |i| matches!(&graph[*i], Node::BuiltinFunction { name, .. } if name == "use_cplan"),
            )
            .collect();
        let indexscan: Vec<_> = graph
            .node_indices()
            .filter(
                |i| matches!(&graph[*i], Node::BuiltinFunction { name, .. } if name == "indexscan"),
            )
            .collect();
        assert_eq!(use_cplan.len(), 1, "use_cplan should be a separate node");
        assert_eq!(indexscan.len(), 1, "indexscan should be a separate node");
        let combined: Vec<_> = graph
            .node_indices()
            .filter(
                |i| matches!(&graph[*i], Node::BuiltinFunction { name, .. } if name.contains(' ')),
            )
            .collect();
        assert!(combined.is_empty(), "no hint node should contain spaces");
    }

    #[test]
    fn duplicate_java_methods_produce_single_method_node() {
        use crate::graph::builder::GraphBuildContext;
        use crate::parser::java_method::{JavaClassInfo, JavaMethodInfo, JavaParseResult};

        let class = JavaClassInfo {
            fqn: "com.example.UserDao".to_string(),
            name: "UserDao".to_string(),
            package: "com.example".to_string(),
            extends: None,
            implements: vec![],
            file: PathBuf::from("/a/UserDao.java"),
            line: 3,
        };

        let method = JavaMethodInfo {
            name: "findAll".to_string(),
            class_fqn: "com.example.UserDao".to_string(),
            signature: "List<User> findAll()".to_string(),
            file: PathBuf::from("/a/UserDao.java"),
            line: 8,
            calls: vec![],
        };

        let make_result = |path_str: &str| JavaParseResult {
            file: PathBuf::from(path_str),
            package: "com.example".to_string(),
            imports: vec![],
            classes: vec![class.clone()],
            methods: vec![method.clone()],
            di_injections: vec![],
            content_hash: "abc".to_string(),
        };

        let results = vec![
            make_result("/a/UserDao.java"),
            make_result("/b/UserDao.java"),
        ];

        let mut ctx = GraphBuildContext::new();
        GraphBuilder::add_java_method_nodes_from_parsed(
            &results,
            &mut ctx.graph,
            &mut ctx.proc_index,
            &ctx.mapper_index,
        );

        let method_nodes: Vec<_> = ctx
            .graph
            .node_indices()
            .filter(|i| {
                matches!(
                    &ctx.graph[*i],
                    Node::JavaMethod { fqn, .. } if fqn == "com.example.UserDao.findAll"
                )
            })
            .collect();

        assert_eq!(
            method_nodes.len(),
            1,
            "duplicate JavaMethodInfo with same FQN should produce exactly 1 JavaMethod node, got {}",
            method_nodes.len()
        );
    }

    #[test]
    fn plsql_varray_type_constructor_not_unresolved() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURE PROC_VARRAY AS
            BEGIN
                DECLARE
                    TYPE arr_type IS VARRAY(4) OF VARCHAR2(100);
                    table_array arr_type := arr_type('A','B','C','D');
                BEGIN
                    NULL;
                END;
            END;
        "#;
        let graph = build_from_sql(sql);
        let unresolved: Vec<String> = graph
            .node_indices()
            .filter_map(|i| match &graph[i] {
                Node::Unresolved { raw_expr, .. } => Some((**raw_expr).clone()),
                _ => None,
            })
            .collect();
        assert!(
            unresolved.is_empty(),
            "VARRAY type constructor must not spawn unresolved node: {:?}",
            unresolved
        );
    }

    #[test]
    fn plsql_table_of_type_constructor_not_unresolved() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURE PROC_TABLEOF AS
            BEGIN
                DECLARE
                    TYPE t_work_array IS TABLE OF VARCHAR2(100);
                    v_work_array t_work_array := t_work_array();
                BEGIN
                    NULL;
                END;
            END;
        "#;
        let graph = build_from_sql(sql);
        let unresolved: Vec<String> = graph
            .node_indices()
            .filter_map(|i| match &graph[i] {
                Node::Unresolved { raw_expr, .. } => Some((**raw_expr).clone()),
                _ => None,
            })
            .collect();
        assert!(
            unresolved.is_empty(),
            "TABLE OF type constructor must not spawn unresolved node: {:?}",
            unresolved
        );
    }

    #[test]
    fn plsql_index_by_type_declaration_no_unresolved() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURE PROC_INDEXBY AS
            BEGIN
                DECLARE
                    TYPE vchartab IS TABLE OF VARCHAR2(4000) INDEX BY INTEGER;
                    vchar_array vchartab;
                BEGIN
                    vchar_array(1) := 'x';
                END;
            END;
        "#;
        let graph = build_from_sql(sql);
        let unresolved: Vec<String> = graph
            .node_indices()
            .filter_map(|i| match &graph[i] {
                Node::Unresolved { raw_expr, .. } => Some((**raw_expr).clone()),
                _ => None,
            })
            .collect();
        assert!(
            unresolved.is_empty(),
            "INDEX BY collection indexing on local var must not spawn unresolved node: {:?}",
            unresolved
        );
    }

    #[test]
    fn plsql_package_variable_collection_index_not_unresolved() {
        let sql = r#"
            CREATE OR REPLACE PACKAGE BODY BIGFUND.PKG_CLR_RULE_OPT AS
                TYPE vchartab_pkg IS TABLE OF VARCHAR2(4000) INDEX BY INTEGER;
                vchar_array_pkg vchartab_pkg;

                PROCEDURE use_pkg_var IS
                BEGIN
                    vchar_array_pkg(1) := 'x';
                END;
            END;
        "#;
        let graph = build_from_sql(sql);
        let unresolved: Vec<String> = graph
            .node_indices()
            .filter_map(|i| match &graph[i] {
                Node::Unresolved { raw_expr, .. } => Some((**raw_expr).clone()),
                _ => None,
            })
            .collect();
        assert!(
            unresolved.is_empty(),
            "package-level variable used as collection index must not spawn unresolved node: {:?}",
            unresolved
        );
    }

    #[test]
    fn plsql_package_type_constructor_not_unresolved() {
        // The constructor call must be inside a PROCEDURE body (not a
        // package-level Variable initializer) so collect_package_call_edges
        // actually walks it — package Variable initializers are never visited.
        let sql = r#"
            CREATE OR REPLACE PACKAGE BODY BIGFUND.PKG_DEMO AS
                TYPE int_list IS TABLE OF INTEGER;

                PROCEDURE init IS
                    v int_list := int_list(1, 2, 3);
                BEGIN
                    NULL;
                END;
            END;
        "#;
        let graph = build_from_sql(sql);
        let unresolved: Vec<String> = graph
            .node_indices()
            .filter_map(|i| match &graph[i] {
                Node::Unresolved { raw_expr, .. } => Some((**raw_expr).clone()),
                _ => None,
            })
            .collect();
        assert!(
            unresolved.is_empty(),
            "package-level TYPE used as constructor in procedure body must not spawn unresolved \
             node: {:?}",
            unresolved
        );
    }

    #[test]
    fn spec_declared_collection_var_visible_in_body_no_unresolved() {
        // TYPE + variable declared in the package SPEC; used in the package
        // BODY. The body extractor must inherit the spec's public symbols so
        // `vchar_array(1)` is not misread as a procedure call.
        let sql = r#"
            CREATE OR REPLACE PACKAGE pkg_specdemo IS
                TYPE vchartab IS TABLE OF VARCHAR2(4000) INDEX BY INTEGER;
                vchar_array vchartab;
            END pkg_specdemo;

            CREATE OR REPLACE PACKAGE BODY pkg_specdemo AS
                PROCEDURE use_it IS
                BEGIN
                    vchar_array(1) := 'x';
                END use_it;
            END pkg_specdemo;
        "#;
        let graph = build_from_sql(sql);
        let unresolved: Vec<String> = graph
            .node_indices()
            .filter_map(|i| match &graph[i] {
                Node::Unresolved { raw_expr, .. } => Some((**raw_expr).clone()),
                _ => None,
            })
            .collect();
        assert!(
            unresolved.is_empty(),
            "spec-declared collection variable used in body must not spawn unresolved node: {:?}",
            unresolved
        );
    }

    #[test]
    fn spec_declared_var_visible_in_body_with_pkg_name_case_mismatch() {
        // Spec and body use DIFFERENT casing for the package name. Spec↔body
        // linkage must be case-insensitive so the spec's symbols still seed
        // the body extractor's scope.
        let sql = r#"
            CREATE OR REPLACE PACKAGE PKG_SPECCASE IS
                TYPE vchartab IS TABLE OF VARCHAR2(4000) INDEX BY INTEGER;
                vchar_array vchartab;
            END PKG_SPECCASE;

            CREATE OR REPLACE PACKAGE BODY pkg_speccase AS
                PROCEDURE use_it IS
                BEGIN
                    vchar_array(1) := 'x';
                END use_it;
            END pkg_speccase;
        "#;
        let graph = build_from_sql(sql);
        let unresolved: Vec<String> = graph
            .node_indices()
            .filter_map(|i| match &graph[i] {
                Node::Unresolved { raw_expr, .. } => Some((**raw_expr).clone()),
                _ => None,
            })
            .collect();
        assert!(
            unresolved.is_empty(),
            "spec-declared variable must remain visible to body even when package name casing \
             differs; got unresolved: {:?}",
            unresolved
        );
    }

    #[test]
    fn plsql_type_filter_does_not_suppress_real_call() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURE real_callee AS BEGIN NULL; END;

            CREATE OR REPLACE PROCEDURE caller AS
            BEGIN
                DECLARE
                    TYPE local_type IS TABLE OF VARCHAR2(10);
                    v local_type := local_type();
                BEGIN
                    real_callee();
                END;
            END;
        "#;
        let graph = build_from_sql(sql);

        let unresolved: Vec<String> = graph
            .node_indices()
            .filter_map(|i| match &graph[i] {
                Node::Unresolved { raw_expr, .. } => Some((**raw_expr).clone()),
                _ => None,
            })
            .collect();
        assert!(
            unresolved.is_empty(),
            "no unresolved nodes expected: {:?}",
            unresolved
        );

        let call_edges: Vec<_> = graph
            .edge_indices()
            .filter(|e| matches!(&graph[*e], Edge::DirectCall { .. }))
            .collect();
        assert_eq!(
            call_edges.len(),
            1,
            "real_callee() call must still produce a DirectCall edge"
        );
    }

    // ── try_resolve_routine tests ────────────────────────────────────

    /// Helper to build resolver indexes from a graph, matching the logic
    /// in `resolve_unresolved_nodes`.
    fn build_resolver_indexes(
        graph: &crate::graph::CodeGraph,
    ) -> (
        HashMap<String, petgraph::graph::NodeIndex>,
        HashMap<String, Vec<petgraph::graph::NodeIndex>>,
        HashMap<String, Vec<(Option<String>, petgraph::graph::NodeIndex)>>,
        HashMap<(String, String), petgraph::graph::NodeIndex>,
    ) {
        let mut lower_qualified: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();
        let mut bare_name_lower: HashMap<String, Vec<petgraph::graph::NodeIndex>> = HashMap::new();
        let mut bare_name_schemas: HashMap<
            String,
            Vec<(Option<String>, petgraph::graph::NodeIndex)>,
        > = HashMap::new();
        let mut pkg_member_lower: HashMap<(String, String), petgraph::graph::NodeIndex> =
            HashMap::new();

        for idx in graph.node_indices() {
            let routine_id = match &graph[idx] {
                crate::graph::Node::Procedure { id, .. }
                | crate::graph::Node::Function { id, .. } => id,
                _ => continue,
            };
            let qualified_lower = routine_id.to_string().to_lowercase();
            lower_qualified.entry(qualified_lower).or_insert(idx);

            let name_lower = routine_id.name.to_lowercase();
            bare_name_lower
                .entry(name_lower.clone())
                .or_default()
                .push(idx);
            bare_name_schemas
                .entry(name_lower)
                .or_default()
                .push((routine_id.schema.as_ref().map(|s| s.to_lowercase()), idx));

            // Schema-as-package indexing (same logic as resolve_unresolved_nodes)
            if let Some(ref schema) = routine_id.schema {
                if routine_id.package.is_none() {
                    pkg_member_lower
                        .entry((schema.to_lowercase(), routine_id.name.to_lowercase()))
                        .or_insert(idx);
                }
            }
        }

        (
            lower_qualified,
            bare_name_lower,
            bare_name_schemas,
            pkg_member_lower,
        )
    }

    #[test]
    fn try_resolve_routine_resolved_returns_correct_idx() {
        // Test (a): a resolvable name returns Resolved(idx) with the SAME idx
        // as the old code path (regression guard).
        use crate::graph::{RoutineId, RoutineKind, SourceLocation};
        use std::sync::Arc;

        let mut graph = crate::graph::CodeGraph::new();
        let proc_idx = graph.add_node(crate::graph::Node::Procedure {
            id: RoutineId {
                schema: Some("public".to_string()),
                package: None,
                name: "my_proc".to_string(),
                kind: RoutineKind::Procedure,
            },
            location: SourceLocation {
                file: Arc::new(std::path::PathBuf::from("test.sql")),
                line: 1,
            },
            partial: false,
            body_sql: vec![],
        });

        let (lower_qualified, bare_name_lower, bare_name_schemas, pkg_member_lower) =
            build_resolver_indexes(&graph);

        let result = super::try_resolve_routine(
            "public.my_proc",
            &lower_qualified,
            &bare_name_lower,
            &bare_name_schemas,
            &pkg_member_lower,
            &[],
        );

        match result {
            super::ResolveOutcome::Resolved(idx, _) => assert_eq!(
                idx, proc_idx,
                "Resolved index must match the known procedure node"
            ),
            super::ResolveOutcome::Miss(trace) => {
                panic!("Expected Resolved but got Miss with trace: {trace:?}")
            }
        }
    }

    #[test]
    fn try_resolve_routine_miss_has_correct_trace() {
        // Test (b): a name with no match returns Miss whose StrategyTrace
        // fields are correct and s1_hit == false.
        let lower_qualified = HashMap::new();
        let bare_name_lower = HashMap::new();
        let bare_name_schemas = HashMap::new();
        let pkg_member_lower = HashMap::new();
        let caller_schemas: Vec<Option<String>> = vec![];

        let result = super::try_resolve_routine(
            "nonexistent.proc",
            &lower_qualified,
            &bare_name_lower,
            &bare_name_schemas,
            &pkg_member_lower,
            &caller_schemas,
        );

        match result {
            super::ResolveOutcome::Resolved(_, _) => {
                panic!("Expected Miss for a name with no matching nodes")
            }
            super::ResolveOutcome::Miss(trace) => {
                // parsed_schema/name from rsplit_once('.')
                assert_eq!(
                    trace.parsed_schema,
                    Some("nonexistent".to_string()),
                    "parsed_schema should be the part before the last dot"
                );
                assert_eq!(
                    trace.parsed_name, "proc",
                    "parsed_name should be the part after the last dot"
                );
                // s1_qualified_key = raw_name.to_lowercase()
                assert_eq!(
                    trace.s1_qualified_key, "nonexistent.proc",
                    "s1_qualified_key should be the lowercased raw_name"
                );
                assert!(!trace.s1_hit, "s1 must not have hit on empty indexes");
                // raw_name contains '.' so s3_lookup should be set
                assert_eq!(
                    trace.s3_lookup,
                    Some(("nonexistent".to_string(), "proc".to_string())),
                    "s3_lookup should be (pkg_part_lower, name_part_lower)"
                );
                assert!(!trace.s3_hit, "s3 must not have hit on empty indexes");
                assert!(
                    trace.caller_schemas.is_empty(),
                    "caller_schemas should match the passed slice"
                );
            }
        }
    }

    #[test]
    fn try_resolve_routine_ambiguous_bare_resolves_via_s7() {
        // Test (c): 2 procedures sharing the same name but different schemas.
        // Old behavior resolves via S7 (first-candidate best-effort).
        // New behavior must match.
        use crate::graph::{RoutineId, RoutineKind, SourceLocation};
        use std::sync::Arc;

        let mut graph = crate::graph::CodeGraph::new();
        let _idx_a = graph.add_node(crate::graph::Node::Procedure {
            id: RoutineId {
                schema: Some("schema_a".to_string()),
                package: None,
                name: "shared_name".to_string(),
                kind: RoutineKind::Procedure,
            },
            location: SourceLocation {
                file: Arc::new(std::path::PathBuf::from("test.sql")),
                line: 1,
            },
            partial: false,
            body_sql: vec![],
        });
        let _idx_b = graph.add_node(crate::graph::Node::Procedure {
            id: RoutineId {
                schema: Some("schema_b".to_string()),
                package: None,
                name: "shared_name".to_string(),
                kind: RoutineKind::Procedure,
            },
            location: SourceLocation {
                file: Arc::new(std::path::PathBuf::from("test.sql")),
                line: 2,
            },
            partial: false,
            body_sql: vec![],
        });

        let (lower_qualified, bare_name_lower, bare_name_schemas, pkg_member_lower) =
            build_resolver_indexes(&graph);

        // Query by bare name (no qualifier) with no caller schemas.
        // Old code: S4 sees 2 matches → ambiguous → S5/S6 no match → S7 picks first.
        // S7 always resolves, so we assert Resolved.
        let result = super::try_resolve_routine(
            "shared_name",
            &lower_qualified,
            &bare_name_lower,
            &bare_name_schemas,
            &pkg_member_lower,
            &[], // empty caller schemas
        );

        match result {
            super::ResolveOutcome::Resolved(idx, _) => {
                // S7 picks the first candidate registered (schema_a's entry).
                assert_eq!(
                    idx, _idx_a,
                    "S7 should resolve to the first-registered candidate (schema_a)"
                );
            }
            super::ResolveOutcome::Miss(trace) => {
                panic!("Expected Resolved (S7 disambiguation) but got Miss: {trace:?}");
            }
        }
    }

    // ── survivor diagnostic tests ──

    #[test]
    fn levenshtein_identical() {
        assert_eq!(super::levenshtein("hello", "hello"), 0);
    }

    #[test]
    fn levenshtein_one_edit() {
        assert_eq!(super::levenshtein("hello", "hallo"), 1);
    }

    #[test]
    fn levenshtein_empty_vs_nonempty() {
        assert_eq!(super::levenshtein("", "hello"), 5);
        assert_eq!(super::levenshtein("hello", ""), 5);
    }

    #[test]
    fn levenshtein_insert_delete() {
        assert_eq!(super::levenshtein("hello", "hell"), 1);
        assert_eq!(super::levenshtein("hell", "hello"), 1);
    }

    #[test]
    fn levenshtein_case_sensitive() {
        // levenshtein is case-sensitive; callers lowercase inputs first
        assert_eq!(super::levenshtein("Hello", "hello"), 1);
    }

    #[test]
    fn format_survivor_diagnostic_with_s3() {
        use crate::graph::{RoutineId, RoutineKind};

        let trace = super::StrategyTrace {
            parsed_schema: Some("S".to_string()),
            parsed_name: "M".to_string(),
            s1_qualified_key: "s.m".to_string(),
            s1_hit: false,
            s3_lookup: Some(("s".to_string(), "m".to_string())),
            s3_hit: false,
            caller_schemas: vec![],
        };

        let candidates = vec![(
            1usize,
            RoutineId {
                schema: Some("S".to_string()),
                package: Some("P".to_string()),
                name: "M".to_string(),
                kind: RoutineKind::Procedure,
            },
        )];

        let output = super::format_survivor_diagnostic("S.M", &trace, &candidates);

        assert!(output.contains("'S.M' survived resolution"), "raw_expr");
        assert!(output.contains("S1"), "s1 line");
        assert!(output.contains("miss"), "miss indicator");
        assert!(output.contains("S3"), "s3 line present");
        assert!(output.contains("'S.P.M'"), "candidate display");
        assert!(output.contains("(d=1)"), "distance");
        assert!(output.contains("package:P"), "candidate package field");
    }

    #[test]
    fn format_survivor_diagnostic_no_s3() {
        use crate::graph::RoutineId;

        let trace = super::StrategyTrace {
            parsed_schema: None,
            parsed_name: "M".to_string(),
            s1_qualified_key: "m".to_string(),
            s1_hit: false,
            s3_lookup: None,
            s3_hit: false,
            caller_schemas: vec![],
        };

        let candidates: Vec<(usize, RoutineId)> = vec![];

        let output = super::format_survivor_diagnostic("M", &trace, &candidates);

        assert!(output.contains("'M' survived resolution"));
        assert!(
            !output.contains("S3"),
            "must not contain S3 when s3_lookup is None"
        );
        assert!(
            output.contains("(none within edit-distance threshold)"),
            "none within threshold"
        );
    }

    #[test]
    fn unresolved_creation_suffix_includes_parsed_and_snippet() {
        let suffix = super::unresolved_creation_suffix(
            Some((Some("PKG"), None, "LOG_ORDER")),
            Some(">  42 |   call"),
        );
        assert!(
            suffix.contains(r#"parsed: {schema:Some("PKG"), package:None, name:"LOG_ORDER"}"#),
            "got: {}",
            suffix
        );
        assert!(
            suffix.contains(">  42 |   call"),
            "snippet missing: {}",
            suffix
        );
    }

    #[test]
    fn unresolved_creation_suffix_none_yields_empty() {
        assert_eq!(super::unresolved_creation_suffix(None, None), "");
    }

    #[test]
    fn unresolved_creation_suffix_only_snippet_when_no_parsed() {
        let suffix = super::unresolved_creation_suffix(None, Some("> 1 | x"));
        assert!(!suffix.contains("parsed"), "no parsed line: {}", suffix);
        assert!(suffix.contains("> 1 | x"));
    }

    #[test]
    fn nearest_routine_candidates_finds_close() {
        use crate::graph::{CodeGraph, Node, RoutineId, RoutineKind, SourceLocation};
        use std::sync::Arc;

        let mut graph = CodeGraph::new();
        let idx = graph.add_node(Node::Procedure {
            id: RoutineId {
                schema: Some("S".to_string()),
                package: None,
                name: "process_order".to_string(),
                kind: RoutineKind::Procedure,
            },
            location: SourceLocation {
                file: Arc::new(PathBuf::from("test.sql")),
                line: 1,
            },
            partial: false,
            body_sql: vec![],
        });

        let mut lower_qualified = HashMap::new();
        lower_qualified.insert("s.process_order".to_string(), idx);

        // "process_orders" is 1 edit from "process_order"
        let result =
            super::nearest_routine_candidates("s.process_orders", &lower_qualified, &graph, 3, 3);

        assert!(!result.is_empty(), "should find a close candidate");
        assert_eq!(result[0].0, 1, "edit distance should be 1");
        assert_eq!(result[0].1.name, "process_order");
    }

    #[test]
    fn nearest_routine_candidates_distant_returns_empty() {
        use crate::graph::{CodeGraph, Node, RoutineId, RoutineKind, SourceLocation};
        use std::sync::Arc;

        let mut graph = CodeGraph::new();
        let idx = graph.add_node(Node::Procedure {
            id: RoutineId {
                schema: None,
                package: None,
                name: "calculate_total".to_string(),
                kind: RoutineKind::Procedure,
            },
            location: SourceLocation {
                file: Arc::new(PathBuf::from("test.sql")),
                line: 1,
            },
            partial: false,
            body_sql: vec![],
        });

        let mut lower_qualified = HashMap::new();
        lower_qualified.insert("calculate_total".to_string(), idx);

        // "xyzzy" is far from "calculate_total" (>3 edits at 10 chars)
        let result = super::nearest_routine_candidates("xyzzy", &lower_qualified, &graph, 3, 3);

        assert!(result.is_empty(), "distant name should return empty");
    }

    #[test]
    fn comment_on_column_populates_column_comment() {
        let sql = r#"
            CREATE TABLE orders (
                id INTEGER,
                amount NUMERIC
            );
            COMMENT ON COLUMN orders.id IS 'primary key';
            COMMENT ON COLUMN orders.amount IS 'order amount';
        "#;
        let graph = build_from_sql(sql);

        let table_idx = graph
            .node_indices()
            .find(|&i| matches!(&graph[i], Node::Table { name, .. } if name == "orders"))
            .expect("orders table should exist");

        if let Node::Table { columns, .. } = &graph[table_idx] {
            assert_eq!(columns.len(), 2);
            let id_col = columns.iter().find(|c| c.name == "id").unwrap();
            let amount_col = columns.iter().find(|c| c.name == "amount").unwrap();
            assert_eq!(id_col.comment.as_deref(), Some("primary key"));
            assert_eq!(amount_col.comment.as_deref(), Some("order amount"));
        } else {
            panic!("expected table node");
        }
    }

    #[test]
    fn comment_on_column_with_schema_qualifier() {
        let sql = r#"
            CREATE TABLE public.items (
                name TEXT
            );
            COMMENT ON COLUMN public.items.name IS 'item name';
        "#;
        let graph = build_from_sql(sql);

        let table_idx = graph
            .node_indices()
            .find(|&i| matches!(&graph[i], Node::Table { name, .. } if name == "items"))
            .expect("items table should exist");

        if let Node::Table { columns, .. } = &graph[table_idx] {
            let name_col = columns.iter().find(|c| c.name == "name").unwrap();
            assert_eq!(name_col.comment.as_deref(), Some("item name"));
        } else {
            panic!("expected table node");
        }
    }

    #[test]
    fn comment_on_column_before_create_table_is_applied() {
        let sql = r#"
            COMMENT ON COLUMN users.email IS 'user email';
            CREATE TABLE users (
                id INTEGER,
                email TEXT
            );
        "#;
        let graph = build_from_sql(sql);

        let table_idx = graph
            .node_indices()
            .find(|&i| matches!(&graph[i], Node::Table { name, .. } if name == "users"))
            .expect("users table should exist");

        if let Node::Table { columns, .. } = &graph[table_idx] {
            let email_col = columns.iter().find(|c| c.name == "email").unwrap();
            assert_eq!(email_col.comment.as_deref(), Some("user email"));
        } else {
            panic!("expected table node");
        }
    }

    #[test]
    fn comment_on_column_with_chinese_characters() {
        let sql = r#"
            CREATE TABLE products (
                name TEXT
            );
            COMMENT ON COLUMN products.name IS '产品名称';
        "#;
        let graph = build_from_sql(sql);

        let table_idx = graph
            .node_indices()
            .find(|&i| matches!(&graph[i], Node::Table { name, .. } if name == "products"))
            .expect("products table should exist");

        if let Node::Table { columns, .. } = &graph[table_idx] {
            let name_col = columns.iter().find(|c| c.name == "name").unwrap();
            assert_eq!(name_col.comment.as_deref(), Some("产品名称"));
            assert_eq!(name_col.comment.as_deref().unwrap().len(), 12);
        } else {
            panic!("expected table node");
        }
    }

    // ── column lineage integration tests ──

    #[test]
    fn column_lineage_disabled_by_default() {
        let sql = "CREATE OR REPLACE PROCEDURE p1() AS $$ BEGIN SELECT a AS x FROM t1; END; $$;";
        let files = vec![ParsedFile {
            path: PathBuf::from("test.sql"),
            statements: parse_sql(sql),
            content_hash: String::new(),
        }];
        let graph = GraphBuilder::new().build(&files);

        let col_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::Column { .. }))
            .collect();
        assert!(
            col_nodes.is_empty(),
            "column lineage must be disabled by default, found {} column nodes",
            col_nodes.len()
        );
    }

    #[test]
    fn column_lineage_adds_flow_and_aggregated_edges_when_enabled() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURE p1() AS $$
            BEGIN
                SELECT a AS x FROM t1;
                SELECT SUM(b) AS total FROM t2 GROUP BY c;
            END;
            $$;
        "#;
        let files = vec![ParsedFile {
            path: PathBuf::from("test.sql"),
            statements: parse_sql(sql),
            content_hash: String::new(),
        }];
        let graph = GraphBuilder::new().with_column_lineage(true).build(&files);

        let col_nodes: Vec<_> = graph
            .node_indices()
            .filter(|i| matches!(&graph[*i], Node::Column { .. }))
            .collect();
        assert!(
            !col_nodes.is_empty(),
            "expected column nodes when column lineage is enabled"
        );

        let has_dataflow = graph
            .edge_indices()
            .any(|e| matches!(&graph[e], Edge::DataFlow { .. }));
        let has_aggregated = graph
            .edge_indices()
            .any(|e| matches!(&graph[e], Edge::Aggregated { .. }));
        assert!(has_dataflow, "expected a DataFlow edge for SELECT a AS x");
        assert!(
            has_aggregated,
            "expected an Aggregated edge for SELECT SUM(b) AS total"
        );

        let grouping_keys: Vec<_> = col_nodes
            .iter()
            .filter(|i| {
                matches!(
                    &graph[**i],
                    Node::Column {
                        is_grouping_key: true,
                        ..
                    }
                )
            })
            .collect();
        assert_eq!(grouping_keys.len(), 1, "expected exactly one GROUP BY key");
    }

    #[test]
    fn column_lineage_derived_expression_edge() {
        let sql =
            "CREATE OR REPLACE PROCEDURE p2() AS $$ BEGIN SELECT t.a + t.b AS c FROM t; END; $$;";
        let files = vec![ParsedFile {
            path: PathBuf::from("test.sql"),
            statements: parse_sql(sql),
            content_hash: String::new(),
        }];
        let graph = GraphBuilder::new().with_column_lineage(true).build(&files);

        let has_derived = graph
            .edge_indices()
            .any(|e| matches!(&graph[e], Edge::Derived { .. }));
        assert!(
            has_derived,
            "expected a Derived edge for SELECT t.a + t.b AS c"
        );
    }
}

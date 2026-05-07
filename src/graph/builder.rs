#[allow(unused_imports)]
use crate::graph::key::NodeKey;
use crate::graph::store::GraphStore;
use crate::graph::{
    determine_call_scope, extract_routine_id, CallScope, CodeGraph, DataFlowKind, Edge, Node,
    RoutineId, RoutineKind, SourceLocation,
};
use crate::graph::{ColumnSummary, DistributeInfo, PartitionInfo};
use crate::parser::{
    AllParsedFiles, CallEdge, CallExtractor, ParsedFile, TypeSequenceRefExtractor,
};
use ogsql_parser::ast::{ColumnConstraint, PackageItem, Statement};
use ogsql_parser::{walk_pl_block, walk_statement};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

pub struct GraphBuilder;

impl GraphBuilder {
    pub fn new() -> Self {
        Self
    }

    pub fn build(&self, files: &[ParsedFile]) -> CodeGraph {
        Self::build_graph_internal(files, &[], &[], &[])
    }

    pub fn build_all(&self, all: &AllParsedFiles) -> CodeGraph {
        Self::build_graph_internal(
            &all.sql_files,
            &all.ibatis_files,
            &all.java_files,
            &all.java_method_results,
        )
    }

    #[allow(dead_code)]
    pub fn build_store(&self, all: &AllParsedFiles, project_name: &str) -> GraphStore {
        let graph = Self::build_graph_internal(
            &all.sql_files,
            &all.ibatis_files,
            &all.java_files,
            &all.java_method_results,
        );
        GraphStore::from_graph(project_name, graph)
    }

    fn build_graph_internal(
        sql_files: &[ParsedFile],
        ibatis_files: &[crate::parser::ibatis_loader::IbatisParsedFile],
        java_files: &[crate::parser::java_loader::JavaParsedFile],
        java_method_results: &[crate::parser::java_method::JavaParseResult],
    ) -> CodeGraph {
        let mut graph = CodeGraph::new();
        let mut proc_index: HashMap<RoutineId, petgraph::graph::NodeIndex> = HashMap::new();
        let mut package_index: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();
        let mut mapper_index: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();
        let mut table_index: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();
        let mut type_index: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();
        let mut sequence_index: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();

        // Pass 1: Create all SQL nodes (single file iteration)
        Self::create_sql_nodes(
            sql_files,
            &mut graph,
            &mut proc_index,
            &mut package_index,
            &mut table_index,
            &mut type_index,
            &mut sequence_index,
        );

        // Pass 2: Create all SQL edges + table access (single file iteration)
        Self::create_sql_edges(sql_files, &mut graph, &mut proc_index, &mut table_index);

        Self::create_object_ref_edges(
            sql_files,
            &mut graph,
            &proc_index,
            &type_index,
            &sequence_index,
        );

        // Single-pass for other file types
        Self::add_ibatis_nodes_from_parsed(
            ibatis_files,
            &mut graph,
            &mut proc_index,
            &mut mapper_index,
            &mut table_index,
        );
        Self::add_java_nodes_from_parsed(
            java_files,
            &mut graph,
            &mut proc_index,
            &mapper_index,
            &mut table_index,
        );
        Self::add_java_method_nodes_from_parsed(
            java_method_results,
            &mut graph,
            &mut proc_index,
            &mapper_index,
        );
        Self::dedup_table_view_nodes(&mut graph);
        Self::merge_table_access_edges(&mut graph);
        Self::resolve_unresolved_nodes(&mut graph);

        graph
    }

    // ── Pass 1: Create all SQL nodes ─────────────────────────────
    // Merged from: create_procedure_nodes + detect_and_create_partial_nodes
    // + add_view_nodes (5 loops → 1 loop)

    fn create_sql_nodes(
        files: &[ParsedFile],
        graph: &mut CodeGraph,
        proc_index: &mut HashMap<RoutineId, petgraph::graph::NodeIndex>,
        package_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        table_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        type_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        sequence_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
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
                        proc_index.entry(id.clone()).or_insert_with(|| {
                            let node = Node::Procedure {
                                id,
                                location: SourceLocation {
                                    file: file_arc.clone(),
                                    line: info.start_line,
                                },
                                partial: false,
                            };
                            graph.add_node(node)
                        });
                    }
                    Statement::CreateFunction(f) => {
                        let id = RoutineId::from_object_name(&f.name, RoutineKind::Function);
                        proc_index.entry(id.clone()).or_insert_with(|| {
                            let node = Node::Function {
                                id,
                                location: SourceLocation {
                                    file: file_arc.clone(),
                                    line: info.start_line,
                                },
                                partial: false,
                            };
                            graph.add_node(node)
                        });
                    }
                    Statement::CreatePackage(pkg) => {
                        Self::create_package_nodes(
                            &pkg.name,
                            &pkg.items,
                            info.start_line,
                            &file_arc,
                            graph,
                            proc_index,
                            package_index,
                        );
                        let pkg_name = pkg.name.last().cloned().unwrap_or_default();
                        for item in &pkg.items {
                            let (name, kind) = match item {
                                PackageItem::Procedure(p) => {
                                    (p.name.join("."), RoutineKind::Procedure)
                                }
                                PackageItem::Function(f) => {
                                    (f.name.join("."), RoutineKind::Function)
                                }
                                PackageItem::Raw(_)
                                | PackageItem::Variable(_)
                                | PackageItem::Type(_) => continue,
                            };
                            spec_decls.push((pkg_name.clone(), name, kind));
                        }
                    }
                    Statement::CreatePackageBody(pkg) => {
                        has_body = true;
                        Self::create_package_nodes(
                            &pkg.name,
                            &pkg.items,
                            info.start_line,
                            &file_arc,
                            graph,
                            proc_index,
                            package_index,
                        );
                        let pkg_name = pkg.name.last().cloned().unwrap_or_default();
                        for item in &pkg.items {
                            let name = match item {
                                PackageItem::Procedure(p) => p.name.join("."),
                                PackageItem::Function(f) => f.name.join("."),
                                PackageItem::Raw(_)
                                | PackageItem::Variable(_)
                                | PackageItem::Type(_) => continue,
                            };
                            body_impls.push((pkg_name.clone(), name));
                        }
                    }
                    Statement::CreateTrigger(t) => {
                        let trigger_node = Node::Trigger {
                            name: t.name.clone(),
                            table: t.table.clone(),
                            location: SourceLocation {
                                file: file_arc.clone(),
                                line: info.start_line,
                            },
                        };
                        let trigger_idx = graph.add_node(trigger_node);

                        let func_id =
                            RoutineId::from_object_name(&t.func_name, RoutineKind::Function);
                        let func_idx = proc_index.get(&func_id).copied().unwrap_or_else(|| {
                            let raw = t.func_name.join(".");
                            crate::parse_log::warn(
                                &file.path.to_string_lossy(),
                                &format!(
                                    "unresolved: trigger '{}' references function '{}' not found in parsed files",
                                    t.name, raw
                                ),
                            );
                            let unresolved = Node::Unresolved {
                                raw_expr: Box::new(raw),
                                context: Box::new(format!("trigger:{}", t.name)),
                            };
                            let idx = graph.add_node(unresolved);
                            proc_index.insert(func_id, idx);
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
                        let view_name = v.name.last().cloned().unwrap_or_default();
                        let view_node = Node::View {
                            schema: view_schema.clone(),
                            name: view_name.clone(),
                            location: None,
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
                        let type_kind = match &t.type_kind {
                            ogsql_parser::ast::TypeKind::Composite { .. } => "composite",
                            ogsql_parser::ast::TypeKind::Enum { .. } => "enum",
                            ogsql_parser::ast::TypeKind::Base { .. } => "base",
                            ogsql_parser::ast::TypeKind::Table { .. } => "table",
                            ogsql_parser::ast::TypeKind::Range { .. } => "range",
                            ogsql_parser::ast::TypeKind::Shell => "shell",
                        };
                        let type_node = Node::Type {
                            schema: schema.clone(),
                            name: name.clone(),
                            type_kind: type_kind.to_string(),
                            location: SourceLocation {
                                file: file_arc.clone(),
                                line: info.start_line,
                            },
                        };
                        let idx = graph.add_node(type_node);
                        let short_key = name.clone();
                        let full_key = match &schema {
                            Some(s) => format!("{}.{}", s, name),
                            None => name.clone(),
                        };
                        type_index.entry(short_key).or_insert(idx);
                        type_index.entry(full_key).or_insert(idx);
                    }
                    Statement::CreateSequence(s) => {
                        let (schema, name) = split_object_name(&s.name);
                        let seq_node = Node::Sequence {
                            schema: schema.clone(),
                            name: name.clone(),
                            location: SourceLocation {
                                file: file_arc.clone(),
                                line: info.start_line,
                            },
                        };
                        let idx = graph.add_node(seq_node);
                        let short_key = name.clone();
                        let full_key = match &schema {
                            Some(sc) => format!("{}.{}", sc, name),
                            None => name.clone(),
                        };
                        sequence_index.entry(short_key).or_insert(idx);
                        sequence_index.entry(full_key).or_insert(idx);
                    }
                    Statement::CreateIndex(i) => {
                        let idx_name = i
                            .name
                            .as_ref()
                            .map(|n| n.last().cloned().unwrap_or_default());
                        let (table_schema, table_name) = split_object_name(&i.table);
                        let index_node = Node::Index {
                            name: idx_name,
                            table_schema: table_schema.clone(),
                            table_name: table_name.clone(),
                            unique: i.unique,
                            global: false,
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
                                        Some(format!("{:?}", expr))
                                    } else {
                                        None
                                    }
                                });
                                ColumnSummary {
                                    name: c.name.clone(),
                                    data_type: format!("{:?}", c.data_type),
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
                            columns,
                            partition_by,
                            distribute_by,
                            tablespace,
                            temporary,
                            unlogged,
                            ..
                        } = &mut graph[idx]
                        {
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
                                                default_value = Some(format!("{:?}", expr))
                                            }
                                            _ => {}
                                        }
                                    }
                                    ColumnSummary {
                                        name: c.name.clone(),
                                        data_type: format!("{:?}", c.data_type),
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

                        if schema.is_some() {
                            table_index.entry(name.to_lowercase()).or_insert(idx);
                        }
                    }
                    Statement::CreateMaterializedView(v) => {
                        let (schema, name) = split_object_name(&v.name);
                        let mview_node = Node::MaterializedView {
                            schema: schema.clone(),
                            name: name.clone(),
                            location: SourceLocation {
                                file: file_arc.clone(),
                                line: info.start_line,
                            },
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
                            .get(&RoutineId::from_qualified_name(
                                &target_key,
                                RoutineKind::Procedure,
                            ))
                            .copied()
                            .or_else(|| {
                                proc_index
                                    .get(&RoutineId::from_qualified_name(
                                        &target_key,
                                        RoutineKind::Function,
                                    ))
                                    .copied()
                            })
                            .or_else(|| table_index.get(&target_key).copied())
                            .or_else(|| type_index.get(&target_key).copied())
                            .or_else(|| sequence_index.get(&target_key).copied())
                            .unwrap_or_else(|| {
                                crate::parse_log::warn(
                                    &file.path.to_string_lossy(),
                                    &format!(
                                        "unresolved: synonym '{}.{}' target '{}' not found",
                                        schema.as_deref().unwrap_or(""),
                                        name,
                                        target_key
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
                    if !proc_index.contains_key(&routine_id) {
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
                                id: routine_id.clone(),
                                location: SourceLocation {
                                    file: file_arc.clone(),
                                    line: 0,
                                },
                                partial: true,
                            },
                            RoutineKind::Function => Node::Function {
                                id: routine_id.clone(),
                                location: SourceLocation {
                                    file: file_arc.clone(),
                                    line: 0,
                                },
                                partial: true,
                            },
                        };
                        let idx = graph.add_node(node);
                        proc_index.insert(routine_id.clone(), idx);

                        if let Some(&pkg_idx) = package_index.get(pkg_name) {
                            graph.add_edge(pkg_idx, idx, Edge::ContainsRoutine);
                        }
                    }
                }
            }
        }
    }

    fn create_package_nodes(
        pkg_name: &ogsql_parser::ast::ObjectName,
        pkg_items: &[PackageItem],
        start_line: usize,
        file_path: &Arc<PathBuf>,
        graph: &mut CodeGraph,
        proc_index: &mut HashMap<RoutineId, petgraph::graph::NodeIndex>,
        package_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
    ) {
        let pkg_name_part = pkg_name.last().cloned().unwrap_or_default();
        let schema_part: Option<String> = if pkg_name.len() > 1 {
            Some(pkg_name[..pkg_name.len() - 1].join("."))
        } else {
            None
        };
        let qualified = match &schema_part {
            Some(s) => format!("{}.{}", s, pkg_name_part),
            None => pkg_name_part.clone(),
        };

        let pkg_idx = *package_index.entry(qualified).or_insert_with(|| {
            let node = Node::Package {
                schema: schema_part.clone(),
                name: pkg_name_part.clone(),
                location: SourceLocation {
                    file: file_path.clone(),
                    line: start_line,
                },
            };
            graph.add_node(node)
        });

        for item in pkg_items {
            let (proc_name, block, kind) = match item {
                PackageItem::Procedure(p) => (p.name.join("."), &p.block, RoutineKind::Procedure),
                PackageItem::Function(f) => (f.name.join("."), &f.block, RoutineKind::Function),
                PackageItem::Raw(_) | PackageItem::Variable(_) | PackageItem::Type(_) => continue,
            };
            let Some(_block) = block else {
                continue;
            };
            let proc_id = RoutineId {
                schema: schema_part.clone(),
                package: Some(pkg_name_part.clone()),
                name: proc_name,
                kind,
            };
            let proc_idx = proc_index.entry(proc_id.clone()).or_insert_with(|| {
                let node = match kind {
                    RoutineKind::Procedure => Node::Procedure {
                        id: proc_id.clone(),
                        location: SourceLocation {
                            file: file_path.clone(),
                            line: start_line,
                        },
                        partial: false,
                    },
                    RoutineKind::Function => Node::Function {
                        id: proc_id.clone(),
                        location: SourceLocation {
                            file: file_path.clone(),
                            line: start_line,
                        },
                        partial: false,
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
    ) {
        let mut all_edges = Vec::new();

        for file in files {
            let file_arc: Arc<PathBuf> = Arc::new(file.path.clone());
            for info in &file.statements {
                let mut extractor = CallExtractor::new(file_arc.clone());
                match &info.statement {
                    Statement::CreatePackage(pkg) => {
                        Self::collect_package_call_edges(&pkg.name, &pkg.items, &mut extractor);
                    }
                    Statement::CreatePackageBody(pkg) => {
                        Self::collect_package_call_edges(&pkg.name, &pkg.items, &mut extractor);
                    }
                    _ => {
                        walk_statement(&mut extractor, &info.statement);
                    }
                }
                all_edges.extend(extractor.edges);

                match &info.statement {
                    Statement::CreateProcedure(p) => {
                        let proc_id = RoutineId::from_object_name(&p.name, RoutineKind::Procedure);
                        if let Some(&proc_idx) = proc_index.get(&proc_id) {
                            Self::collect_table_access_from_statements(
                                std::slice::from_ref(info),
                                &file_arc,
                                proc_idx,
                                graph,
                                table_index,
                            );
                        }
                    }
                    Statement::CreateFunction(f) => {
                        let proc_id = RoutineId::from_object_name(&f.name, RoutineKind::Function);
                        if let Some(&proc_idx) = proc_index.get(&proc_id) {
                            Self::collect_table_access_from_statements(
                                std::slice::from_ref(info),
                                &file_arc,
                                proc_idx,
                                graph,
                                table_index,
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
                        );
                    }
                    _ => {}
                }
            }
        }

        Self::create_edges(&all_edges, graph, proc_index);
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
                        if let Some(&proc_idx) = proc_index.get(&proc_id) {
                            for param in &p.parameters {
                                if let Some(&type_idx) = type_index.get(&param.data_type) {
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
                                if let Some(&type_idx) = type_index.get(&type_ref.type_name) {
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
                                if let Some(&seq_idx) = sequence_index.get(&seq_ref.sequence_name) {
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
                        if let Some(&proc_idx) = proc_index.get(&proc_id) {
                            for param in &f.parameters {
                                if let Some(&type_idx) = type_index.get(&param.data_type) {
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
                                if let Some(&type_idx) = type_index.get(ret_type) {
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
                                if let Some(&type_idx) = type_index.get(&type_ref.type_name) {
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
                                if let Some(&seq_idx) = sequence_index.get(&seq_ref.sequence_name) {
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
        let pkg_name_part = pkg_name.last().cloned().unwrap_or_default();
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
                PackageItem::Raw(_) | PackageItem::Variable(_) | PackageItem::Type(_) => continue,
            };
            let proc_id = RoutineId {
                schema: schema_part.clone(),
                package: Some(pkg_name_part.clone()),
                name: proc_name,
                kind,
            };
            let Some(proc_idx) = proc_index.get(&proc_id).copied() else {
                continue;
            };
            let Some(ref block) = block else {
                continue;
            };

            let mut extractor = TypeSequenceRefExtractor::new(known_types.clone());
            extractor.current_context = proc_id.to_string();
            walk_pl_block(&mut extractor, block);

            for type_ref in &extractor.type_refs {
                if let Some(&type_idx) = type_index.get(&type_ref.type_name) {
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
                if let Some(&seq_idx) = sequence_index.get(&seq_ref.sequence_name) {
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
        extractor: &mut CallExtractor,
    ) {
        let pkg_name_part = pkg_name.last().cloned().unwrap_or_default();
        let schema_part: Option<String> = if pkg_name.len() > 1 {
            Some(pkg_name[..pkg_name.len() - 1].join("."))
        } else {
            None
        };
        for item in pkg_items {
            match item {
                PackageItem::Procedure(p) => {
                    if let Some(ref block) = p.block {
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
                        extractor.current_procedure = Some(RoutineId {
                            schema: schema_part.clone(),
                            package: Some(pkg_name_part.clone()),
                            name: f.name.join("."),
                            kind: RoutineKind::Function,
                        });
                        walk_pl_block(extractor, block);
                    }
                }
                PackageItem::Raw(_) | PackageItem::Variable(_) | PackageItem::Type(_) => {}
            }
        }
    }

    fn create_edges(
        edges: &[CallEdge],
        graph: &mut CodeGraph,
        proc_index: &mut HashMap<RoutineId, petgraph::graph::NodeIndex>,
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
                proc_index.get(id).copied().or_else(|| {
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
                    proc_index.get(&alt_id).copied()
                })
            });

            let callee_id =
                RoutineId::from_qualified_name(&edge.callee_name, RoutineKind::Procedure);
            let callee_idx = proc_index
                .get(&callee_id)
                .copied()
                .or_else(|| {
                    if callee_id.schema.is_some() && callee_id.package.is_none() {
                        let alt_id = RoutineId {
                            schema: None,
                            package: callee_id.schema.clone(),
                            name: callee_id.name.clone(),
                            kind: RoutineKind::Procedure,
                        };
                        proc_index.get(&alt_id).copied()
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
                    crate::parse_log::warn(
                        &edge.location.file.to_string_lossy(),
                        &format!(
                            "unresolved: call target '{}' (from {}:{}) not found in parsed files",
                            edge.callee_name,
                            edge.location
                                .file
                                .file_name()
                                .map(|f| f.to_string_lossy().to_string())
                                .unwrap_or_default(),
                            edge.location.line,
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
                    proc_index.insert(callee_id, to);

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

    fn add_ibatis_nodes_from_parsed(
        ibatis_files: &[crate::parser::ibatis_loader::IbatisParsedFile],
        graph: &mut CodeGraph,
        proc_index: &mut HashMap<RoutineId, petgraph::graph::NodeIndex>,
        mapper_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        table_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
    ) {
        for ibatis_file in ibatis_files {
            let xml_path = Arc::new(PathBuf::from(
                ibatis_file.result.file_path.as_deref().unwrap_or_default(),
            ));
            let namespace = &ibatis_file.result.namespace;

            for stmt in &ibatis_file.result.statements {
                let kind_label = crate::parser::ibatis_loader::statement_kind_label(&stmt.kind);
                let node = Node::MappedStatement {
                    namespace: namespace.clone(),
                    statement_id: stmt.id.clone(),
                    kind: kind_label.to_string(),
                    xml_file: (*xml_path).clone(),
                    line: stmt.line,
                };
                let node_idx = graph.add_node(node);

                let mapper_key = format!("{}.{}", namespace, stmt.id);
                mapper_index.insert(mapper_key, node_idx);

                if let Some((statements, _errors)) = &stmt.parse_result {
                    let calls = Self::extract_calls_from_statements(statements, &xml_path);
                    for callee_name in calls {
                        let callee_id =
                            RoutineId::from_qualified_name(&callee_name, RoutineKind::Procedure);
                        let callee_idx = proc_index.entry(callee_id.clone()).or_insert_with(|| {
                            crate::parse_log::warn(
                                &xml_path.to_string_lossy(),
                                &format!(
                                    "unresolved: mapper '{}.{}' calls '{}' not found in parsed files",
                                    namespace, stmt.id, callee_name
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
                        graph,
                        table_index,
                    );
                }
            }
        }
    }

    fn add_java_nodes_from_parsed(
        java_files: &[crate::parser::java_loader::JavaParsedFile],
        graph: &mut CodeGraph,
        proc_index: &mut HashMap<RoutineId, petgraph::graph::NodeIndex>,
        mapper_index: &HashMap<String, petgraph::graph::NodeIndex>,
        table_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
    ) {
        for java_file in java_files {
            let java_path = Arc::new(PathBuf::from(&java_file.result.file_path));

            for extraction in &java_file.result.extractions {
                let method_label =
                    crate::parser::java_loader::extraction_method_label(&extraction.origin.method);
                let node = Node::JavaSql {
                    class_name: extraction.origin.class_name.clone(),
                    method_name: extraction.origin.method_name.clone(),
                    extraction_method: method_label.to_string(),
                    java_file: (*java_path).clone(),
                    line: extraction.origin.line,
                };
                let node_idx = graph.add_node(node);

                if let Some(parse_result) = &extraction.parse_result {
                    let calls =
                        Self::extract_calls_from_statements(&parse_result.statements, &java_path);
                    for callee_name in calls {
                        let callee_id =
                            RoutineId::from_qualified_name(&callee_name, RoutineKind::Procedure);
                        let callee_idx = proc_index.entry(callee_id.clone()).or_insert_with(|| {
                            crate::parse_log::warn(
                                &java_path.to_string_lossy(),
                                &format!(
                                    "unresolved: Java '{}.{}' calls '{}' not found in parsed files",
                                    extraction.origin.class_name.as_deref().unwrap_or("?"),
                                    extraction.origin.method_name.as_deref().unwrap_or("?"),
                                    callee_name
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
                        graph,
                        table_index,
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
    ) -> Vec<String> {
        let mut calls = Vec::new();
        for info in statements {
            let mut extractor = CallExtractor::new(file_path.clone());
            walk_statement(&mut extractor, &info.statement);
            for edge in extractor.edges {
                calls.push(edge.callee_name);
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
    ) {
        let pkg_name_part = pkg_name.last().cloned().unwrap_or_default();
        let schema_part: Option<String> = if pkg_name.len() > 1 {
            Some(pkg_name[..pkg_name.len() - 1].join("."))
        } else {
            None
        };
        for item in pkg_items {
            let (proc_name, block, kind) = match item {
                PackageItem::Procedure(p) => (p.name.join("."), &p.block, RoutineKind::Procedure),
                PackageItem::Function(f) => (f.name.join("."), &f.block, RoutineKind::Function),
                PackageItem::Raw(_) | PackageItem::Variable(_) | PackageItem::Type(_) => continue,
            };
            let proc_id = RoutineId {
                schema: schema_part.clone(),
                package: Some(pkg_name_part.clone()),
                name: proc_name,
                kind,
            };
            if let Some(&proc_idx) = proc_index.get(&proc_id) {
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
                        graph,
                        table_index,
                    );
                }
            }
        }
    }

    fn collect_table_access_from_statements(
        statements: &[ogsql_parser::StatementInfo],
        file_path: &Arc<PathBuf>,
        source_idx: petgraph::graph::NodeIndex,
        graph: &mut CodeGraph,
        table_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
    ) {
        for info in statements {
            let mut extractor = crate::parser::TableAccessExtractor::new();
            walk_statement(&mut extractor, &info.statement);
            for access in &extractor.accesses {
                let key = normalize_table_key(access.schema.as_deref(), &access.name);
                let table_idx = *table_index.entry(key.clone()).or_insert_with(|| {
                    let node = Node::Table {
                        schema: access.schema.clone(),
                        name: access.name.clone(),
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
                    },
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

        // Phase 2: merge bare-name Table nodes into schema-qualified Table nodes
        {
            // bare_name_lower → (schema, idx)
            let mut qualified: HashMap<String, (String, petgraph::graph::NodeIndex)> =
                HashMap::new();
            let mut bare: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();

            for idx in graph.node_indices() {
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
                    if bare_idx != qual_idx {
                        merges.push((bare_idx, qual_idx));
                    }
                }
            }
        }

        if merges.is_empty() {
            return;
        }

        for (from_idx, into_idx) in merges {
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
            graph.remove_node(from_idx);
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
            let (mut merged_modes, mut merged_kinds) =
                if let Edge::TableAccess {
                    modes, write_kinds, ..
                } = &graph[keep]
                {
                    (*modes, write_kinds.clone())
                } else {
                    continue;
                };
            for &remove_idx in &edge_indices {
                if let Edge::TableAccess {
                    modes, write_kinds, ..
                } = &graph[remove_idx]
                {
                    merged_modes |= *modes;
                    for wk in write_kinds {
                        merged_kinds.insert(*wk);
                    }
                }
            }
            if let Edge::TableAccess {
                modes, write_kinds, ..
            } = &mut graph[keep]
            {
                *modes = merged_modes;
                *write_kinds = merged_kinds;
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
            lower_qualified
                .entry(qualified_lower)
                .or_insert(idx);

            bare_name_lower
                .entry(routine_id.name.to_lowercase())
                .or_default()
                .push(idx);

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

        for (unres_idx, raw_expr, _context) in &unresolved {
            // ── Noise filtering: skip clearly non-routine patterns ──
            if is_noise_unresolved(raw_expr) {
                to_remove.push(*unres_idx);
                continue;
            }

            // ── Try to resolve ──
            if let Some(target_idx) =
                try_resolve_routine(raw_expr, &lower_qualified, &bare_name_lower, &pkg_member_lower)
            {
                if target_idx == *unres_idx {
                    continue; // shouldn't happen, but guard
                }
                // Rewire all edges pointing to/from the unresolved node
                let sources: Vec<_> = graph
                    .neighbors_directed(*unres_idx, petgraph::Direction::Incoming)
                    .collect();
                for src in sources {
                    let weights: Vec<_> = graph
                        .edges_connecting(src, *unres_idx)
                        .map(|e| e.weight().clone())
                        .collect();
                    for weight in weights {
                        graph.add_edge(src, target_idx, weight);
                    }
                }
                to_remove.push(*unres_idx);
            }
        }

        // ── Remove resolved/filtered nodes (reverse order not needed, slot-based) ──
        for idx in to_remove {
            graph.remove_node(idx);
        }
    }

    fn add_java_method_nodes_from_parsed(
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
                let idx = graph.add_node(node);
                class_index.insert(class.fqn.clone(), idx);
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
                let node = Node::JavaMethod {
                    fqn: method_fqn.clone(),
                    class_fqn: method.class_fqn.clone(),
                    name: method.name.clone(),
                    signature: method.signature.clone(),
                    file: method.file.clone(),
                    line: method.line,
                };
                let method_idx = graph.add_node(node);
                method_index.insert(method_fqn, method_idx);

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
fn is_noise_unresolved(raw_expr: &str) -> bool {
    let trimmed = raw_expr.trim();

    // AST debug strings from EXECUTE IMMEDIATE with non-literal expressions
    if trimmed.starts_with("PlVariable(")
        || trimmed.starts_with("BinaryOp ")
        || trimmed.starts_with("BinaryOp{")
        || trimmed.starts_with("FunctionCall ")
        || trimmed.starts_with("FunctionCall{")
        || trimmed.starts_with("Literal(")
        || trimmed.starts_with("ColumnRef(")
    {
        return true;
    }

    // Object member access (SELF.xxx) — not a procedure call
    let upper = trimmed.to_uppercase();
    if upper.starts_with("SELF.") {
        return true;
    }

    // PL/SQL collection methods
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
        return true;
    }

    // Known system packages and built-in functions in openGauss/GaussDB
    if is_known_system_call(trimmed) {
        return true;
    }

    false
}

/// Known system packages and functions that are never user-defined routines.
fn is_known_system_call(name: &str) -> bool {
    let upper = name.to_uppercase();
    let system_prefixes = [
        "DBE_SCHEDULER.",
        "DBE_OUTPUT.",
        "DBE_UTILITY.",
        "DBE_STATS.",
        "DBE_SQL.",
        "DBE_LOB.",
        "DBE_RAW.",
        "DBE_DESCRIBE.",
        "DBE_ASSERT.",
        "DBE_PROFILER.",
        "DBE_PLDEBUGGER.",
        "DBE_PLDEVELOPER.",
        "DBE_TASK.",
        "DBE_FILE.",
        "DBE_PERF.",
        "DBE_SESSION.",
        "DBE_APPLICATION_INFO.",
        "PG_",
        "DBMS_",
    ];
    let system_functions = [
        "PG_SLEEP",
        "PG_SLEEP_FOR",
        "PG_SLEEP_UNTIL",
        "RAISE_NOTICE",
        "RAISE_WARNING",
        "RAISE_EXCEPTION",
        "RAISE_INFO",
        "RAISE_LOG",
        "RAISE_DEBUG",
        "RAISE",
        "DBE_OUTPUT.PRINT",
        "DBE_OUTPUT.PRINT_LINE",
        "DBE_OUTPUT.PRINTF",
        "PRINT",
    ];

    for prefix in &system_prefixes {
        if upper.starts_with(prefix) {
            return true;
        }
    }
    for func in &system_functions {
        if upper == *func || upper.starts_with(&format!("{}(", func)) {
            return true;
        }
    }
    false
}

/// Multi-strategy routine resolution for unresolved nodes.
fn try_resolve_routine(
    raw_name: &str,
    lower_qualified: &HashMap<String, petgraph::graph::NodeIndex>,
    bare_name_lower: &HashMap<String, Vec<petgraph::graph::NodeIndex>>,
    pkg_member_lower: &HashMap<(String, String), petgraph::graph::NodeIndex>,
) -> Option<petgraph::graph::NodeIndex> {
    let name_lower = raw_name.to_lowercase();

    // Strategy 1: Case-insensitive exact match (handles Procedure↔Function implicitly
    // because lower_qualified is keyed by display string which doesn't include kind)
    if let Some(&idx) = lower_qualified.get(&name_lower) {
        return Some(idx);
    }

    // Strategy 2: If raw_name is "schema.name", try bare name in bare_name_lower
    // (this handles cases where the schema prefix differs from what's stored)
    if let Some(dot_pos) = raw_name.rfind('.') {
        let bare = &raw_name[dot_pos + 1..];
        let bare_lower = bare.to_lowercase();

        // Try case-insensitive bare name (single unambiguous match)
        if let Some(matches) = bare_name_lower.get(&bare_lower) {
            if matches.len() == 1 {
                return Some(matches[0]);
            }
        }
    }

    // Strategy 3: Schema-as-package: treat the prefix as a package name
    if let Some(dot_pos) = raw_name.rfind('.') {
        let pkg_part = &raw_name[..dot_pos];
        let name_part = &raw_name[dot_pos + 1..];
        if let Some(&idx) = pkg_member_lower.get(&(pkg_part.to_lowercase(), name_part.to_lowercase())) {
            return Some(idx);
        }
    }

    // Strategy 4: Unqualified bare name — single unambiguous match
    if !raw_name.contains('.') {
        if let Some(matches) = bare_name_lower.get(&name_lower) {
            if matches.len() == 1 {
                return Some(matches[0]);
            }
        }
    }

    None
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

fn split_object_name(name: &[String]) -> (Option<String>, String) {
    if name.len() <= 1 {
        (None, name.first().cloned().unwrap_or_default())
    } else {
        (
            Some(name[..name.len() - 1].join(".")),
            name[name.len() - 1].clone(),
        )
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

#[cfg(test)]
mod tests {
    use crate::graph::builder::GraphBuilder;
    use crate::graph::{Edge, Node};
    use crate::parser::ParsedFile;
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
        assert_eq!(call_edges.len(), 1, "Expected 1 DirectCall edge from procedure to function");

        let (src, dst) = graph.edge_endpoints(call_edges[0]).unwrap();
        assert_eq!(src, proc_nodes[0], "Call should originate from prc_trd_ej_listquery_zh");
        assert_eq!(dst, func_nodes[0], "Call should target get_par_fund_info_a function");
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
            .filter(|i| matches!(&graph[*i], Node::Procedure { id, .. } if id.name == "MT_541_CREATE"))
            .collect();
        assert_eq!(proc_create.len(), 1, "Expected MT_541_CREATE procedure");
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
            .filter(|i| matches!(&graph[*i], Node::Unresolved { raw_expr, .. }
                if raw_expr.contains("SELF.")))
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
}

#[allow(unused_imports)]
use crate::graph::key::NodeKey;
use crate::graph::store::GraphStore;
use crate::graph::{CodeGraph, Edge, Node, ProcedureId, SourceLocation};
use crate::parser::{AllParsedFiles, CallEdge, CallExtractor, ParsedFile};
use ogsql_parser::ast::{PackageItem, Statement};
use ogsql_parser::{walk_pl_block, walk_statement};
use std::collections::HashMap;
use std::path::PathBuf;

pub struct GraphBuilder;

impl GraphBuilder {
    pub fn new() -> Self {
        Self
    }

    pub fn build(&self, files: &[ParsedFile]) -> CodeGraph {
        let mut graph = CodeGraph::new();
        let mut proc_index: HashMap<ProcedureId, petgraph::graph::NodeIndex> = HashMap::new();
        let mut package_index: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();
        let mut table_index: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();

        Self::create_procedure_nodes(files, &mut graph, &mut proc_index, &mut package_index);
        let edges = Self::collect_call_edges(files);
        Self::create_edges(&edges, &mut graph, &mut proc_index);
        Self::add_table_refs_from_sql(
            files,
            &mut graph,
            &proc_index,
            &package_index,
            &mut table_index,
        );

        graph
    }

    pub fn build_all(&self, all: &AllParsedFiles) -> CodeGraph {
        let mut graph = CodeGraph::new();
        let mut proc_index: HashMap<ProcedureId, petgraph::graph::NodeIndex> = HashMap::new();
        let mut package_index: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();
        let mut mapper_index: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();
        let mut table_index: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();

        Self::create_procedure_nodes(
            &all.sql_files,
            &mut graph,
            &mut proc_index,
            &mut package_index,
        );
        let edges = Self::collect_call_edges(&all.sql_files);
        Self::create_edges(&edges, &mut graph, &mut proc_index);
        Self::add_table_refs_from_sql(
            &all.sql_files,
            &mut graph,
            &proc_index,
            &package_index,
            &mut table_index,
        );

        Self::add_ibatis_nodes_from_parsed(
            &all.ibatis_files,
            &mut graph,
            &mut proc_index,
            &mut mapper_index,
            &mut table_index,
        );
        Self::add_java_nodes_from_parsed(
            &all.java_files,
            &mut graph,
            &mut proc_index,
            &mapper_index,
            &mut table_index,
        );
        Self::add_java_method_nodes_from_parsed(
            &all.java_method_results,
            &mut graph,
            &mut proc_index,
            &mapper_index,
        );

        graph
    }

    #[allow(dead_code)]
    pub fn build_store(&self, all: &AllParsedFiles, project_name: &str) -> GraphStore {
        let mut graph = CodeGraph::new();
        let mut proc_index: HashMap<ProcedureId, petgraph::graph::NodeIndex> = HashMap::new();
        let mut package_index: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();
        let mut mapper_index: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();
        let mut table_index: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();

        Self::create_procedure_nodes(
            &all.sql_files,
            &mut graph,
            &mut proc_index,
            &mut package_index,
        );
        let edges = Self::collect_call_edges(&all.sql_files);
        Self::create_edges(&edges, &mut graph, &mut proc_index);
        Self::add_table_refs_from_sql(
            &all.sql_files,
            &mut graph,
            &proc_index,
            &package_index,
            &mut table_index,
        );

        Self::add_ibatis_nodes_from_parsed(
            &all.ibatis_files,
            &mut graph,
            &mut proc_index,
            &mut mapper_index,
            &mut table_index,
        );
        Self::add_java_nodes_from_parsed(
            &all.java_files,
            &mut graph,
            &mut proc_index,
            &mapper_index,
            &mut table_index,
        );
        Self::add_java_method_nodes_from_parsed(
            &all.java_method_results,
            &mut graph,
            &mut proc_index,
            &mapper_index,
        );

        GraphStore::from_graph(project_name, graph)
    }

    fn create_procedure_nodes(
        files: &[ParsedFile],
        graph: &mut CodeGraph,
        proc_index: &mut HashMap<ProcedureId, petgraph::graph::NodeIndex>,
        package_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
    ) {
        for file in files {
            for info in &file.statements {
                match &info.statement {
                    Statement::CreateProcedure(p) => {
                        let id = ProcedureId::from_object_name(&p.name);
                        proc_index.entry(id.clone()).or_insert_with(|| {
                            let node = Node::Procedure {
                                id,
                                location: SourceLocation {
                                    file: file.path.clone(),
                                    line: info.start_line,
                                },
                            };
                            graph.add_node(node)
                        });
                    }
                    Statement::CreateFunction(f) => {
                        let id = ProcedureId::from_object_name(&f.name);
                        proc_index.entry(id.clone()).or_insert_with(|| {
                            let node = Node::Procedure {
                                id,
                                location: SourceLocation {
                                    file: file.path.clone(),
                                    line: info.start_line,
                                },
                            };
                            graph.add_node(node)
                        });
                    }
                    Statement::CreatePackage(pkg) => {
                        Self::create_package_nodes(
                            &pkg.name,
                            &pkg.items,
                            info.start_line,
                            &file.path,
                            graph,
                            proc_index,
                            package_index,
                        );
                    }
                    Statement::CreatePackageBody(pkg) => {
                        Self::create_package_nodes(
                            &pkg.name,
                            &pkg.items,
                            info.start_line,
                            &file.path,
                            graph,
                            proc_index,
                            package_index,
                        );
                    }
                    _ => {}
                }
            }
        }
    }

    fn create_package_nodes(
        pkg_name: &ogsql_parser::ast::ObjectName,
        pkg_items: &[PackageItem],
        start_line: usize,
        file_path: &std::path::Path,
        graph: &mut CodeGraph,
        proc_index: &mut HashMap<ProcedureId, petgraph::graph::NodeIndex>,
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
                    file: file_path.to_path_buf(),
                    line: start_line,
                },
            };
            graph.add_node(node)
        });

        for item in pkg_items {
            let (proc_name, has_block) = match item {
                PackageItem::Procedure(p) => (p.name.join("."), p.block.is_some()),
                PackageItem::Function(f) => (f.name.join("."), f.block.is_some()),
                PackageItem::Raw(_) => continue,
            };
            let proc_id = ProcedureId {
                schema: schema_part.clone(),
                package: Some(pkg_name_part.clone()),
                name: proc_name,
            };
            let proc_idx = proc_index.entry(proc_id.clone()).or_insert_with(|| {
                let node = Node::Procedure {
                    id: proc_id.clone(),
                    location: SourceLocation {
                        file: file_path.to_path_buf(),
                        line: start_line,
                    },
                };
                graph.add_node(node)
            });
            if has_block {
                graph.add_edge(pkg_idx, *proc_idx, Edge::ContainsRoutine);
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
                        extractor.current_procedure = Some(ProcedureId {
                            schema: schema_part.clone(),
                            package: Some(pkg_name_part.clone()),
                            name: p.name.join("."),
                        });
                        walk_pl_block(extractor, block);
                    }
                }
                PackageItem::Function(f) => {
                    if let Some(ref block) = f.block {
                        extractor.current_procedure = Some(ProcedureId {
                            schema: schema_part.clone(),
                            package: Some(pkg_name_part.clone()),
                            name: f.name.join("."),
                        });
                        walk_pl_block(extractor, block);
                    }
                }
                PackageItem::Raw(_) => {}
            }
        }
    }

    fn collect_call_edges(files: &[ParsedFile]) -> Vec<CallEdge> {
        let mut all_edges = Vec::new();
        for file in files {
            for info in &file.statements {
                let mut extractor = CallExtractor::new(file.path.clone());
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
            }
        }
        all_edges
    }

    fn create_edges(
        edges: &[CallEdge],
        graph: &mut CodeGraph,
        proc_index: &mut HashMap<ProcedureId, petgraph::graph::NodeIndex>,
    ) {
        let mut seen: HashMap<(Option<String>, String), usize> = HashMap::new();

        for edge in edges {
            let caller_key = edge.caller.as_ref().map(|c| c.to_string());
            let callee_key = edge.callee_name.clone();
            let key = (caller_key.clone(), callee_key.clone());

            if seen.contains_key(&key) {
                continue;
            }
            seen.insert(key, edge.location.line);

            let caller_idx = edge
                .caller
                .as_ref()
                .and_then(|id| proc_index.get(id).copied());

            let callee_id = ProcedureId::from_qualified_name(&edge.callee_name);
            let callee_idx = proc_index.get(&callee_id).copied().or_else(|| {
                if callee_id.schema.is_some() && callee_id.package.is_none() {
                    let alt_id = ProcedureId {
                        schema: None,
                        package: callee_id.schema.clone(),
                        name: callee_id.name.clone(),
                    };
                    proc_index.get(&alt_id).copied()
                } else {
                    None
                }
            });

            match (caller_idx, callee_idx) {
                (Some(from), Some(to)) => {
                    let g_edge = if edge.is_dynamic {
                        Edge::DynamicCall {
                            raw_expr: edge.callee_name.clone(),
                            location: edge.location.clone(),
                        }
                    } else {
                        Edge::DirectCall {
                            location: edge.location.clone(),
                        }
                    };
                    graph.add_edge(from, to, g_edge);
                }
                (Some(from), None) => {
                    let unresolved_node = Node::Unresolved {
                        raw_expr: edge.callee_name.clone(),
                        context: edge
                            .caller
                            .as_ref()
                            .map(|c| c.to_string())
                            .unwrap_or_default(),
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
        proc_index: &mut HashMap<ProcedureId, petgraph::graph::NodeIndex>,
        mapper_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
        table_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
    ) {
        for ibatis_file in ibatis_files {
            let xml_path =
                PathBuf::from(ibatis_file.result.file_path.as_deref().unwrap_or_default());
            let namespace = &ibatis_file.result.namespace;

            for stmt in &ibatis_file.result.statements {
                let kind_label = crate::parser::ibatis_loader::statement_kind_label(&stmt.kind);
                let node = Node::MappedStatement {
                    namespace: namespace.clone(),
                    statement_id: stmt.id.clone(),
                    kind: kind_label.to_string(),
                    xml_file: xml_path.clone(),
                    line: stmt.line,
                };
                let node_idx = graph.add_node(node);

                let mapper_key = format!("{}.{}", namespace, stmt.id);
                mapper_index.insert(mapper_key, node_idx);

                if let Some((statements, _errors)) = &stmt.parse_result {
                    let calls = Self::extract_calls_from_statements(statements, &xml_path);
                    for callee_name in calls {
                        let callee_id = ProcedureId::from_qualified_name(&callee_name);
                        let callee_idx = proc_index.entry(callee_id.clone()).or_insert_with(|| {
                            let unresolved = Node::Unresolved {
                                raw_expr: callee_name.clone(),
                                context: format!("{}.{}", namespace, stmt.id),
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

                    Self::extract_and_add_table_refs(
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
        proc_index: &mut HashMap<ProcedureId, petgraph::graph::NodeIndex>,
        mapper_index: &HashMap<String, petgraph::graph::NodeIndex>,
        table_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
    ) {
        for java_file in java_files {
            let java_path = PathBuf::from(&java_file.result.file_path);

            for extraction in &java_file.result.extractions {
                let method_label =
                    crate::parser::java_loader::extraction_method_label(&extraction.origin.method);
                let node = Node::JavaSql {
                    class_name: extraction.origin.class_name.clone(),
                    method_name: extraction.origin.method_name.clone(),
                    extraction_method: method_label.to_string(),
                    java_file: java_path.clone(),
                    line: extraction.origin.line,
                };
                let node_idx = graph.add_node(node);

                if let Some(parse_result) = &extraction.parse_result {
                    let calls =
                        Self::extract_calls_from_statements(&parse_result.statements, &java_path);
                    for callee_name in calls {
                        let callee_id = ProcedureId::from_qualified_name(&callee_name);
                        let callee_idx = proc_index.entry(callee_id.clone()).or_insert_with(|| {
                            let unresolved = Node::Unresolved {
                                raw_expr: callee_name.clone(),
                                context: format!(
                                    "{}.{}",
                                    extraction.origin.class_name.as_deref().unwrap_or("?"),
                                    extraction.origin.method_name.as_deref().unwrap_or("?")
                                ),
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

                    Self::extract_and_add_table_refs(
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
        file_path: &std::path::Path,
    ) -> Vec<String> {
        let mut calls = Vec::new();
        for info in statements {
            let mut extractor = CallExtractor::new(file_path.to_path_buf());
            walk_statement(&mut extractor, &info.statement);
            for edge in extractor.edges {
                calls.push(edge.callee_name);
            }
        }
        calls
    }

    fn add_table_refs_from_sql(
        files: &[ParsedFile],
        graph: &mut CodeGraph,
        proc_index: &HashMap<ProcedureId, petgraph::graph::NodeIndex>,
        _package_index: &HashMap<String, petgraph::graph::NodeIndex>,
        table_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
    ) {
        for file in files {
            for info in &file.statements {
                match &info.statement {
                    Statement::CreateProcedure(p) => {
                        let proc_id = ProcedureId::from_object_name(&p.name);
                        if let Some(&proc_idx) = proc_index.get(&proc_id) {
                            Self::extract_and_add_table_refs(
                                std::slice::from_ref(info),
                                &file.path,
                                proc_idx,
                                graph,
                                table_index,
                            );
                        }
                    }
                    Statement::CreateFunction(f) => {
                        let proc_id = ProcedureId::from_object_name(&f.name);
                        if let Some(&proc_idx) = proc_index.get(&proc_id) {
                            Self::extract_and_add_table_refs(
                                std::slice::from_ref(info),
                                &file.path,
                                proc_idx,
                                graph,
                                table_index,
                            );
                        }
                    }
                    Statement::CreatePackage(pkg) => {
                        Self::add_package_table_refs(
                            &pkg.name,
                            &pkg.items,
                            info,
                            &file.path,
                            proc_index,
                            graph,
                            table_index,
                        );
                    }
                    Statement::CreatePackageBody(pkg) => {
                        Self::add_package_table_refs(
                            &pkg.name,
                            &pkg.items,
                            info,
                            &file.path,
                            proc_index,
                            graph,
                            table_index,
                        );
                    }
                    _ => {}
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add_package_table_refs(
        pkg_name: &ogsql_parser::ast::ObjectName,
        pkg_items: &[PackageItem],
        info: &ogsql_parser::StatementInfo,
        file_path: &std::path::Path,
        proc_index: &HashMap<ProcedureId, petgraph::graph::NodeIndex>,
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
            let (proc_name, block) = match item {
                PackageItem::Procedure(p) => (p.name.join("."), &p.block),
                PackageItem::Function(f) => (f.name.join("."), &f.block),
                PackageItem::Raw(_) => continue,
            };
            let proc_id = ProcedureId {
                schema: schema_part.clone(),
                package: Some(pkg_name_part.clone()),
                name: proc_name,
            };
            if let Some(&proc_idx) = proc_index.get(&proc_id) {
                if let Some(ref block) = block {
                    let block_stmt = ogsql_parser::StatementInfo {
                        sql_text: String::new(),
                        start_line: info.start_line,
                        start_col: 0,
                        end_line: info.end_line,
                        end_col: 0,
                        statement: Statement::AnonyBlock(ogsql_parser::ast::AnonyBlockStatement {
                            block: block.clone(),
                        }),
                    };
                    Self::extract_and_add_table_refs(
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

    fn extract_and_add_table_refs(
        statements: &[ogsql_parser::StatementInfo],
        file_path: &std::path::Path,
        source_idx: petgraph::graph::NodeIndex,
        graph: &mut CodeGraph,
        table_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
    ) {
        for info in statements {
            let mut extractor = crate::parser::TableRefExtractor::new();
            walk_statement(&mut extractor, &info.statement);
            for tref in &extractor.tables {
                let key = match &tref.schema {
                    Some(s) => format!("{}.{}", s, tref.name),
                    None => tref.name.clone(),
                };
                let table_idx = *table_index.entry(key.clone()).or_insert_with(|| {
                    let node = Node::Table {
                        schema: tref.schema.clone(),
                        name: tref.name.clone(),
                    };
                    graph.add_node(node)
                });
                graph.add_edge(
                    source_idx,
                    table_idx,
                    Edge::ReferencesTable {
                        location: SourceLocation {
                            file: file_path.to_path_buf(),
                            line: info.start_line,
                        },
                    },
                );
            }
        }
    }

    fn add_java_method_nodes_from_parsed(
        java_results: &[crate::parser::java_method::JavaParseResult],
        graph: &mut CodeGraph,
        _proc_index: &mut HashMap<ProcedureId, petgraph::graph::NodeIndex>,
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

        // Reverse index: method_name → [(mapper_key, node_idx)] for O(1) lookup
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
                                        file: class.file.clone(),
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
                                        file: class.file.clone(),
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
                        file: method.file.clone(),
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

                            let callee_fqn = format!("{}.{}", obj_fqn, call.method);
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
                        }

                        // Same-class fallback for qualified calls: currentClass.method
                        let callee_fqn = format!("{}.{}", method.class_fqn, call.method);
                        if let Some(&callee_idx) = method_index.get(&callee_fqn) {
                            graph.add_edge(method_idx, callee_idx, Edge::CallsJava { location });
                        }
                    } else {
                        // Unqualified call: method() within same class
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
}

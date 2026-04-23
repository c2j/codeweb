use crate::graph::{CodeGraph, Edge, Node, ProcedureId, SourceLocation};
use crate::parser::{AllParsedFiles, CallEdge, CallExtractor, ParsedFile};
use ogsql_parser::ast::Statement;
use ogsql_parser::walk_statement;
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

        Self::create_procedure_nodes(files, &mut graph, &mut proc_index);
        let edges = Self::collect_call_edges(files);
        Self::create_edges(&edges, &mut graph, &mut proc_index);

        graph
    }

    pub fn build_all(&self, all: &AllParsedFiles) -> CodeGraph {
        let mut graph = CodeGraph::new();
        let mut proc_index: HashMap<ProcedureId, petgraph::graph::NodeIndex> = HashMap::new();
        let mut mapper_index: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();

        Self::create_procedure_nodes(&all.sql_files, &mut graph, &mut proc_index);
        let edges = Self::collect_call_edges(&all.sql_files);
        Self::create_edges(&edges, &mut graph, &mut proc_index);

        Self::add_ibatis_nodes_from_parsed(
            &all.ibatis_files,
            &mut graph,
            &mut proc_index,
            &mut mapper_index,
        );
        Self::add_java_nodes_from_parsed(
            &all.java_files,
            &mut graph,
            &mut proc_index,
            &mapper_index,
        );
        Self::add_java_method_nodes_from_parsed(
            &all.java_method_results,
            &mut graph,
            &mut proc_index,
            &mapper_index,
        );

        graph
    }

    fn create_procedure_nodes(
        files: &[ParsedFile],
        graph: &mut CodeGraph,
        proc_index: &mut HashMap<ProcedureId, petgraph::graph::NodeIndex>,
    ) {
        for file in files {
            for info in &file.statements {
                let proc_id = match &info.statement {
                    Statement::CreateProcedure(p) => Some(ProcedureId::from_object_name(&p.name)),
                    Statement::CreateFunction(f) => Some(ProcedureId::from_object_name(&f.name)),
                    _ => None,
                };

                if let Some(id) = proc_id {
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
            }
        }
    }

    fn collect_call_edges(files: &[ParsedFile]) -> Vec<CallEdge> {
        let mut all_edges = Vec::new();
        for file in files {
            for info in &file.statements {
                let mut extractor = CallExtractor::new(file.path.clone());
                walk_statement(&mut extractor, &info.statement);
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
            let callee_idx = proc_index.get(&callee_id).copied();

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
                }
            }
        }
    }

    fn add_java_nodes_from_parsed(
        java_files: &[crate::parser::java_loader::JavaParsedFile],
        graph: &mut CodeGraph,
        proc_index: &mut HashMap<ProcedureId, petgraph::graph::NodeIndex>,
        mapper_index: &HashMap<String, petgraph::graph::NodeIndex>,
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
                            for (key, &mapper_idx) in mapper_index.iter() {
                                if key.ends_with(&format!(".{}", call.method)) {
                                    if let Some((ns, _)) = key.rsplit_once('.') {
                                        let ns_simple = ns.rsplit('.').next().unwrap_or(ns);
                                        if names_match(obj, ns_simple) {
                                            graph.add_edge(
                                                method_idx,
                                                mapper_idx,
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
                            // resolve_fqn failed — try heuristic mapper matching against namespace suffix
                            let mut found_mapper = false;
                            for (key, &mapper_idx) in mapper_index.iter() {
                                if key.ends_with(&format!(".{}", call.method)) {
                                    if let Some((ns, _)) = key.rsplit_once('.') {
                                        let ns_simple = ns.rsplit('.').next().unwrap_or(ns);
                                        if names_match(obj, ns_simple) {
                                            graph.add_edge(
                                                method_idx,
                                                mapper_idx,
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

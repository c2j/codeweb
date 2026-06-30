use petgraph::graph::NodeIndex;
use serde::{Deserialize, Serialize};

/// A declarative query spec that can be serialized to/from JSON.
#[derive(Debug, Deserialize, Serialize)]
pub struct QuerySpec {
    /// How to find the starting node(s)
    pub start: StartSpec,
    /// Optional traversal steps to apply sequentially
    #[serde(default)]
    pub steps: Vec<StepSpec>,
    /// What to collect from the result
    #[serde(default = "default_collect")]
    pub collect: CollectMode,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StartSpec {
    /// Node type tag: "proc", "func", "table", "method", etc.
    #[serde(rename = "type", default)]
    pub type_tag: Option<String>,
    /// Name search query (supports exact/substring matching)
    #[serde(default)]
    pub name: Option<String>,
    /// Schema filter
    #[serde(default)]
    pub schema: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "action")]
pub enum StepSpec {
    /// Follow outgoing edges
    #[serde(rename = "outgoing")]
    Outgoing {
        #[serde(default)]
        edge_categories: Option<Vec<String>>,
        #[serde(default)]
        max_depth: Option<usize>,
    },
    /// Follow incoming edges
    #[serde(rename = "incoming")]
    Incoming {
        #[serde(default)]
        edge_categories: Option<Vec<String>>,
        #[serde(default)]
        max_depth: Option<usize>,
    },
    /// Filter current node set
    #[serde(rename = "filter")]
    Filter {
        #[serde(default)]
        type_tag: Option<String>,
        #[serde(default)]
        schema: Option<String>,
    },
    /// Stop traversal when reaching a node type
    #[serde(rename = "until")]
    Until { type_tag: String },
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CollectMode {
    Nodes,
    Paths,
    Subgraph,
}

fn default_collect() -> CollectMode {
    CollectMode::Nodes
}

/// Result of executing a query spec
#[derive(Debug, Serialize)]
pub struct QueryResult {
    pub nodes: Vec<NodeResult>,
    pub paths: Vec<Vec<NodeResult>>,
}

#[derive(Debug, Serialize)]
pub struct NodeResult {
    pub index: usize,
    pub key: String,
    pub type_tag: String,
}

fn node_result(store: &crate::graph::store::GraphStore, idx: NodeIndex) -> NodeResult {
    use crate::graph::key::NodeKey;
    NodeResult {
        index: idx.index(),
        key: NodeKey::from_node(&store.graph()[idx]).to_string(),
        type_tag: crate::graph::node_type_tag(&store.graph()[idx]).to_string(),
    }
}

fn parse_edge_category(s: &str) -> Result<crate::graph::EdgeCategory, String> {
    match s.to_lowercase().as_str() {
        "call" => Ok(crate::graph::EdgeCategory::Call),
        "composition" => Ok(crate::graph::EdgeCategory::Composition),
        "dataflow" => Ok(crate::graph::EdgeCategory::DataFlow),
        "reference" => Ok(crate::graph::EdgeCategory::Reference),
        "inheritance" => Ok(crate::graph::EdgeCategory::Inheritance),
        other => Err(format!("Unknown edge category: '{}'", other)),
    }
}

impl QuerySpec {
    /// Execute this query against a GraphStore.
    pub fn execute(&self, store: &crate::graph::store::GraphStore) -> Result<QueryResult, String> {
        let starts = self.resolve_starts(store)?;
        if starts.is_empty() {
            return Ok(QueryResult {
                nodes: Vec::new(),
                paths: Vec::new(),
            });
        }

        if self.steps.is_empty() {
            let nodes = starts.iter().map(|&idx| node_result(store, idx)).collect();
            return Ok(QueryResult {
                nodes,
                paths: Vec::new(),
            });
        }

        let mut all_nodes = Vec::new();
        let mut all_paths = Vec::new();

        for &start in &starts {
            let (nodes, paths) = self.execute_traversal(store, start)?;
            all_nodes.extend(nodes);
            all_paths.extend(paths);
        }

        // Deduplicate nodes by index
        let mut seen = std::collections::HashSet::new();
        all_nodes.retain(|n| seen.insert(n.index));

        Ok(QueryResult {
            nodes: all_nodes,
            paths: all_paths,
        })
    }

    fn resolve_starts(
        &self,
        store: &crate::graph::store::GraphStore,
    ) -> Result<Vec<NodeIndex>, String> {
        let mut candidates: Option<Vec<NodeIndex>> = None;

        if let Some(ref type_tag) = self.start.type_tag {
            let idxs = store.nodes_by_type(type_tag).to_vec();
            candidates = Some(match candidates {
                Some(existing) => existing.into_iter().filter(|i| idxs.contains(i)).collect(),
                None => idxs,
            });
        }

        if let Some(ref name) = self.start.name {
            let search_results = store.search_nodes(name);
            let idxs: Vec<NodeIndex> = search_results.into_iter().map(|(idx, _)| idx).collect();
            candidates = Some(match candidates {
                Some(existing) => existing.into_iter().filter(|i| idxs.contains(i)).collect(),
                None => idxs,
            });
        }

        if let Some(ref schema) = self.start.schema {
            let idxs = store
                .schema_index()
                .get(&schema.to_lowercase())
                .map(|v| v.as_slice())
                .unwrap_or(&[])
                .to_vec();
            candidates = Some(match candidates {
                Some(existing) => existing.into_iter().filter(|i| idxs.contains(i)).collect(),
                None => idxs,
            });
        }

        candidates.ok_or_else(|| "Start spec must have at least one filter criterion".to_string())
    }

    fn execute_traversal(
        &self,
        store: &crate::graph::store::GraphStore,
        start: NodeIndex,
    ) -> Result<(Vec<NodeResult>, Vec<Vec<NodeResult>>), String> {
        use crate::graph::query::filter::EdgeFilter;
        use crate::graph::query::filter::NodeFilter;
        use crate::graph::query::traversal::GraphTraversal;

        let graph = store.graph();
        let mut traversal = GraphTraversal::new(graph, start);
        let mut node_filter = None;
        let collect_paths = self.collect == CollectMode::Paths;

        for step in &self.steps {
            match step {
                StepSpec::Outgoing {
                    edge_categories,
                    max_depth,
                } => {
                    traversal = traversal.outgoing();
                    if let Some(cats) = edge_categories {
                        let mut ef = EdgeFilter::new();
                        for cat_str in cats {
                            let cat = parse_edge_category(cat_str)?;
                            ef = ef.with_category(cat);
                        }
                        traversal = traversal.edge_filter(ef);
                    }
                    if let Some(depth) = max_depth {
                        traversal = traversal.max_depth(*depth);
                    }
                }
                StepSpec::Incoming {
                    edge_categories,
                    max_depth,
                } => {
                    traversal = traversal.incoming();
                    if let Some(cats) = edge_categories {
                        let mut ef = EdgeFilter::new();
                        for cat_str in cats {
                            let cat = parse_edge_category(cat_str)?;
                            ef = ef.with_category(cat);
                        }
                        traversal = traversal.edge_filter(ef);
                    }
                    if let Some(depth) = max_depth {
                        traversal = traversal.max_depth(*depth);
                    }
                }
                StepSpec::Filter { type_tag, schema } => {
                    let mut nf = NodeFilter::new();
                    if let Some(ref tt) = type_tag {
                        nf = nf.with_type(tt);
                    }
                    if let Some(ref s) = schema {
                        nf = nf.with_schema(s);
                    }
                    node_filter = Some(nf);
                }
                StepSpec::Until { type_tag } => {
                    let tt = type_tag.clone();
                    traversal = traversal.until(move |n| crate::graph::node_type_tag(n) == tt);
                }
            }
        }

        if let Some(nf) = node_filter {
            traversal = traversal.node_filter(nf);
        }

        if collect_paths {
            let paths = traversal.collect_paths();
            let path_results = paths
                .into_iter()
                .map(|path| {
                    path.into_iter()
                        .map(|idx| node_result(store, idx))
                        .collect()
                })
                .collect();
            Ok((Vec::new(), path_results))
        } else {
            let nodes = traversal.collect_nodes();
            let node_results = nodes
                .into_iter()
                .map(|idx| node_result(store, idx))
                .collect();
            Ok((node_results, Vec::new()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn make_file() -> Arc<PathBuf> {
        Arc::new(PathBuf::from("test.sql"))
    }

    fn make_proc(name: &str, schema: Option<&str>) -> Node {
        Node::Procedure {
            id: RoutineId {
                schema: schema.map(String::from),
                package: None,
                name: name.to_string(),
                kind: RoutineKind::Procedure,
            },
            location: SourceLocation {
                file: make_file(),
                line: 1,
            },
            partial: false,
            body_sql: Vec::new(),
        }
    }

    fn make_table(name: &str) -> Node {
        Node::Table {
            schema: None,
            name: name.to_string(),
            explicit: false,
            system: false,
            location: None,
            columns: Box::new(vec![]),
            partition_by: None,
            distribute_by: None,
            tablespace: None,
            temporary: false,
            unlogged: false,
            ddl_source: None,
        }
    }

    #[test]
    fn deserialize_query_spec() {
        let json = r#"{
            "start": { "type": "proc", "name": "calculate" },
            "steps": [
                { "action": "outgoing", "edge_categories": ["call"], "max_depth": 3 }
            ],
            "collect": "nodes"
        }"#;
        let spec: QuerySpec = serde_json::from_str(json).unwrap();
        assert!(spec.start.type_tag.as_deref() == Some("proc"));
        assert_eq!(spec.steps.len(), 1);
        assert_eq!(spec.collect, CollectMode::Nodes);
    }

    #[test]
    fn execute_finds_nodes_by_type_and_name() {
        let mut graph = CodeGraph::new();

        let proc = graph.add_node(make_proc("calculate_total", None));
        let table = graph.add_node(make_table("orders"));

        graph.add_edge(
            proc,
            table,
            Edge::TableAccess {
                flow_kind: DataFlowKind::DmlAccess,
                modes: AccessMode::Read,
                write_kinds: std::collections::HashSet::new(),
                location: SourceLocation {
                    file: make_file(),
                    line: 5,
                },
                column_analysis: None,
            },
        );

        let store = crate::graph::store::GraphStore::from_graph("test", graph);

        let spec = QuerySpec {
            start: StartSpec {
                type_tag: Some("proc".into()),
                name: Some("calculate".into()),
                schema: None,
            },
            steps: vec![],
            collect: CollectMode::Nodes,
        };

        let result = spec.execute(&store).unwrap();
        assert_eq!(result.nodes.len(), 1);
        assert!(result.nodes[0].key.contains("calculate_total"));
    }

    #[test]
    fn execute_traversal_with_steps() {
        let mut graph = CodeGraph::new();
        let file = make_file();

        let a = graph.add_node(Node::Procedure {
            id: RoutineId {
                schema: None,
                package: None,
                name: "a".into(),
                kind: RoutineKind::Procedure,
            },
            location: SourceLocation {
                file: file.clone(),
                line: 1,
            },
            partial: false,
            body_sql: Vec::new(),
        });
        let b = graph.add_node(Node::Procedure {
            id: RoutineId {
                schema: None,
                package: None,
                name: "b".into(),
                kind: RoutineKind::Procedure,
            },
            location: SourceLocation {
                file: file.clone(),
                line: 2,
            },
            partial: false,
            body_sql: Vec::new(),
        });
        graph.add_edge(
            a,
            b,
            Edge::DirectCall {
                scope: CallScope::IntraPackage,
                location: SourceLocation {
                    file: make_file(),
                    line: 1,
                },
            },
        );

        let store = crate::graph::store::GraphStore::from_graph("test", graph);

        let spec = QuerySpec {
            start: StartSpec {
                type_tag: Some("proc".into()),
                name: Some("a".into()),
                schema: None,
            },
            steps: vec![StepSpec::Outgoing {
                edge_categories: Some(vec!["call".into()]),
                max_depth: Some(1),
            }],
            collect: CollectMode::Nodes,
        };

        let result = spec.execute(&store).unwrap();
        assert_eq!(result.nodes.len(), 1);
        assert!(result.nodes[0].key.contains("proc:b"));
    }

    #[test]
    fn execute_returns_empty_for_no_matches() {
        let mut graph = CodeGraph::new();
        graph.add_node(make_proc("foo", None));

        let store = crate::graph::store::GraphStore::from_graph("test", graph);

        let spec = QuerySpec {
            start: StartSpec {
                type_tag: Some("table".into()),
                name: None,
                schema: None,
            },
            steps: vec![],
            collect: CollectMode::Nodes,
        };

        let result = spec.execute(&store).unwrap();
        assert!(result.nodes.is_empty());
    }

    #[test]
    fn collect_paths_mode() {
        let mut graph = CodeGraph::new();
        let file = make_file();

        let a = graph.add_node(make_proc("a", None));
        let b = graph.add_node(make_proc("b", None));
        let c = graph.add_node(make_proc("c", None));

        graph.add_edge(
            a,
            b,
            Edge::DirectCall {
                scope: CallScope::IntraPackage,
                location: SourceLocation {
                    file: file.clone(),
                    line: 1,
                },
            },
        );
        graph.add_edge(
            b,
            c,
            Edge::DirectCall {
                scope: CallScope::IntraPackage,
                location: SourceLocation {
                    file: file.clone(),
                    line: 2,
                },
            },
        );

        let store = crate::graph::store::GraphStore::from_graph("test", graph);

        let spec = QuerySpec {
            start: StartSpec {
                type_tag: Some("proc".into()),
                name: Some("a".into()),
                schema: None,
            },
            steps: vec![StepSpec::Outgoing {
                edge_categories: Some(vec!["call".into()]),
                max_depth: None,
            }],
            collect: CollectMode::Paths,
        };

        let result = spec.execute(&store).unwrap();
        assert_eq!(result.paths.len(), 1);
        assert_eq!(result.paths[0].len(), 3);
        assert!(result.paths[0][0].key.contains("proc:a"));
        assert!(result.paths[0][2].key.contains("proc:c"));
    }
}

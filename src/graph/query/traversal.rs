use super::filter::{EdgeFilter, NodeFilter};
use crate::graph::CodeGraph;
use petgraph::graph::NodeIndex;
use petgraph::Direction;
use std::collections::HashSet;

type UntilCondition<'a> = Box<dyn Fn(&crate::graph::Node) -> bool + 'a>;

pub struct TraversalResult {
    pub nodes: Vec<NodeIndex>,
    pub paths: Vec<Vec<NodeIndex>>,
}

pub struct GraphTraversal<'a> {
    graph: &'a CodeGraph,
    start: NodeIndex,
    direction: Direction,
    edge_filter: EdgeFilter,
    node_filter: Option<NodeFilter>,
    max_depth: Option<usize>,
    max_paths: Option<usize>,
    until: Option<UntilCondition<'a>>,
    target: Option<NodeIndex>,
}

impl<'a> GraphTraversal<'a> {
    pub fn new(graph: &'a CodeGraph, start: NodeIndex) -> Self {
        Self {
            graph,
            start,
            direction: Direction::Outgoing,
            edge_filter: EdgeFilter::new(),
            node_filter: None,
            max_depth: None,
            max_paths: None,
            until: None,
            target: None,
        }
    }

    pub fn direction(mut self, dir: Direction) -> Self {
        self.direction = dir;
        self
    }
    pub fn outgoing(self) -> Self {
        self.direction(Direction::Outgoing)
    }
    pub fn incoming(self) -> Self {
        self.direction(Direction::Incoming)
    }
    pub fn edge_filter(mut self, filter: EdgeFilter) -> Self {
        self.edge_filter = filter;
        self
    }
    pub fn max_depth(mut self, depth: usize) -> Self {
        self.max_depth = Some(depth);
        self
    }
    pub fn max_paths(mut self, n: usize) -> Self {
        self.max_paths = Some(n);
        self
    }
    pub fn node_filter(mut self, filter: NodeFilter) -> Self {
        self.node_filter = Some(filter);
        self
    }
    pub fn until(mut self, cond: impl Fn(&crate::graph::Node) -> bool + 'a) -> Self {
        self.until = Some(Box::new(cond));
        self
    }

    /// Set a target node: only paths ending at this node will be collected.
    pub fn target(mut self, target: NodeIndex) -> Self {
        self.target = Some(target);
        self
    }

    pub fn collect_nodes(self) -> Vec<NodeIndex> {
        let mut result = Vec::new();
        let mut visited = HashSet::new();
        self.dfs_collect(self.start, 0, &mut visited, &mut result);
        result
    }

    pub fn collect_paths(self) -> Vec<Vec<NodeIndex>> {
        let mut paths = Vec::new();
        let mut current_path = vec![self.start];
        let mut visited = HashSet::new();
        visited.insert(self.start);
        self.dfs_paths(self.start, 0, &mut visited, &mut current_path, &mut paths);
        paths
    }

    /// Collect only paths that end at the target node.
    /// Falls back to `collect_paths()` if no target is set.
    pub fn collect_paths_to_target(self) -> Vec<Vec<NodeIndex>> {
        if self.target.is_none() {
            return self.collect_paths();
        }
        let mut paths = Vec::new();
        let mut current_path = vec![self.start];
        let mut visited = HashSet::new();
        visited.insert(self.start);
        self.dfs_paths_to_target(self.start, 0, &mut visited, &mut current_path, &mut paths);
        paths
    }
}

impl<'a> GraphTraversal<'a> {
    fn dfs_collect(
        &self,
        current: NodeIndex,
        depth: usize,
        visited: &mut HashSet<NodeIndex>,
        result: &mut Vec<NodeIndex>,
    ) {
        if let Some(max) = self.max_paths {
            if result.len() >= max {
                return;
            }
        }
        if let Some(max) = self.max_depth {
            if depth >= max {
                return;
            }
        }

        let neighbors: Vec<_> = self
            .graph
            .neighbors_directed(current, self.direction)
            .collect();
        for neighbor in neighbors {
            if visited.contains(&neighbor) {
                continue;
            }

            let (from, to) = match self.direction {
                Direction::Outgoing => (current, neighbor),
                Direction::Incoming => (neighbor, current),
            };
            let edge_matches = self
                .graph
                .edges_connecting(from, to)
                .any(|e| self.edge_filter.matches(e.weight()));
            if !edge_matches {
                continue;
            }

            visited.insert(neighbor);

            if let Some(ref until) = self.until {
                if until(&self.graph[neighbor]) {
                    result.push(neighbor);
                    continue;
                }
            }

            if self
                .node_filter
                .as_ref()
                .is_none_or(|f| f.matches(&self.graph[neighbor]))
            {
                result.push(neighbor);
            }

            self.dfs_collect(neighbor, depth + 1, visited, result);
        }
    }

    fn dfs_paths(
        &self,
        current: NodeIndex,
        depth: usize,
        visited: &mut HashSet<NodeIndex>,
        current_path: &mut Vec<NodeIndex>,
        paths: &mut Vec<Vec<NodeIndex>>,
    ) {
        if let Some(max) = self.max_paths {
            if paths.len() >= max {
                return;
            }
        }
        if let Some(max) = self.max_depth {
            if depth >= max {
                if current_path.len() > 1 {
                    paths.push(current_path.clone());
                }
                return;
            }
        }

        let neighbors: Vec<_> = self
            .graph
            .neighbors_directed(current, self.direction)
            .collect();
        let mut has_unvisited = false;

        for neighbor in neighbors {
            if visited.contains(&neighbor) {
                continue;
            }

            let (from, to) = match self.direction {
                Direction::Outgoing => (current, neighbor),
                Direction::Incoming => (neighbor, current),
            };
            let edge_matches = self
                .graph
                .edges_connecting(from, to)
                .any(|e| self.edge_filter.matches(e.weight()));
            if !edge_matches {
                continue;
            }

            if !self
                .node_filter
                .as_ref()
                .is_none_or(|f| f.matches(&self.graph[neighbor]))
            {
                continue;
            }

            has_unvisited = true;
            visited.insert(neighbor);
            current_path.push(neighbor);

            let is_until = self
                .until
                .as_ref()
                .is_some_and(|u| u(&self.graph[neighbor]));

            if is_until {
                paths.push(current_path.clone());
            } else {
                self.dfs_paths(neighbor, depth + 1, visited, current_path, paths);
            }

            current_path.pop();
            visited.remove(&neighbor);
        }

        if !has_unvisited && current_path.len() > 1 {
            paths.push(current_path.clone());
        }
    }

    fn dfs_paths_to_target(
        &self,
        current: NodeIndex,
        depth: usize,
        visited: &mut HashSet<NodeIndex>,
        current_path: &mut Vec<NodeIndex>,
        paths: &mut Vec<Vec<NodeIndex>>,
    ) {
        if let Some(max) = self.max_depth {
            if depth >= max {
                return;
            }
        }
        if let Some(max) = self.max_paths {
            if paths.len() >= max {
                return;
            }
        }

        for neighbor in self.graph.neighbors_directed(current, self.direction) {
            if visited.contains(&neighbor) {
                continue;
            }

            let (from, to) = match self.direction {
                Direction::Outgoing => (current, neighbor),
                Direction::Incoming => (neighbor, current),
            };
            let edge_matches = self
                .graph
                .edges_connecting(from, to)
                .any(|e| self.edge_filter.matches(e.weight()));
            if !edge_matches {
                continue;
            }

            if !self
                .node_filter
                .as_ref()
                .is_none_or(|f| f.matches(&self.graph[neighbor]))
            {
                continue;
            }

            visited.insert(neighbor);
            current_path.push(neighbor);

            let is_until = self
                .until
                .as_ref()
                .is_some_and(|u| u(&self.graph[neighbor]));

            if is_until || Some(neighbor) == self.target {
                paths.push(current_path.clone());
            } else {
                self.dfs_paths_to_target(neighbor, depth + 1, visited, current_path, paths);
            }

            current_path.pop();
            visited.remove(&neighbor);
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
                line: 0,
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
    fn outgoing_calls_from_procedure() {
        let mut graph = CodeGraph::new();
        let proc_a = graph.add_node(make_proc("proc_a", Some("public")));
        let proc_b = graph.add_node(make_proc("proc_b", Some("public")));
        let table = graph.add_node(make_table("orders"));

        graph.add_edge(
            proc_a,
            proc_b,
            Edge::DirectCall {
                scope: CallScope::CrossPackage,
                location: SourceLocation {
                    file: make_file(),
                    line: 1,
                },
            },
        );
        graph.add_edge(
            proc_b,
            table,
            Edge::TableAccess {
                flow_kind: DataFlowKind::DmlAccess,
                modes: AccessMode::Read,
                write_kinds: std::collections::HashSet::new(),
                location: SourceLocation {
                    file: make_file(),
                    line: 1,
                },
                column_analysis: None,
            },
        );

        let nodes = GraphTraversal::new(&graph, proc_a)
            .outgoing()
            .edge_filter(EdgeFilter::calls_only())
            .max_depth(1)
            .collect_nodes();

        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0], proc_b);
    }

    #[test]
    fn data_flow_reaches_tables() {
        let mut graph = CodeGraph::new();
        let proc = graph.add_node(make_proc("proc", None));
        let table1 = graph.add_node(make_table("t1"));
        let table2 = graph.add_node(make_table("t2"));

        graph.add_edge(
            proc,
            table1,
            Edge::TableAccess {
                flow_kind: DataFlowKind::DmlAccess,
                modes: AccessMode::Read,
                write_kinds: std::collections::HashSet::new(),
                location: SourceLocation {
                    file: make_file(),
                    line: 1,
                },
                column_analysis: None,
            },
        );
        graph.add_edge(
            proc,
            table2,
            Edge::TableAccess {
                flow_kind: DataFlowKind::DmlAccess,
                modes: AccessMode::Write,
                write_kinds: {
                    let mut set = std::collections::HashSet::new();
                    set.insert(WriteKind::Insert);
                    set
                },
                location: SourceLocation {
                    file: make_file(),
                    line: 1,
                },
                column_analysis: None,
            },
        );

        let nodes = GraphTraversal::new(&graph, proc)
            .outgoing()
            .edge_filter(EdgeFilter::data_flow())
            .collect_nodes();

        assert_eq!(nodes.len(), 2);
    }

    #[test]
    fn incoming_finds_callers() {
        let mut graph = CodeGraph::new();
        let caller = graph.add_node(make_proc("caller", None));
        let target = graph.add_node(make_proc("target", None));

        graph.add_edge(
            caller,
            target,
            Edge::DirectCall {
                scope: CallScope::IntraPackage,
                location: SourceLocation {
                    file: make_file(),
                    line: 1,
                },
            },
        );

        let nodes = GraphTraversal::new(&graph, target)
            .incoming()
            .edge_filter(EdgeFilter::calls_only())
            .collect_nodes();

        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0], caller);
    }

    #[test]
    fn max_depth_limits_traversal() {
        let mut graph = CodeGraph::new();
        let a = graph.add_node(make_proc("a", None));
        let b = graph.add_node(make_proc("b", None));
        let c = graph.add_node(make_proc("c", None));

        graph.add_edge(
            a,
            b,
            Edge::DirectCall {
                scope: CallScope::CrossPackage,
                location: SourceLocation {
                    file: make_file(),
                    line: 1,
                },
            },
        );
        graph.add_edge(
            b,
            c,
            Edge::DirectCall {
                scope: CallScope::CrossPackage,
                location: SourceLocation {
                    file: make_file(),
                    line: 1,
                },
            },
        );

        let nodes = GraphTraversal::new(&graph, a)
            .outgoing()
            .edge_filter(EdgeFilter::calls_only())
            .max_depth(1)
            .collect_nodes();

        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0], b);
    }

    #[test]
    fn collect_paths_returns_root_to_leaf() {
        let mut graph = CodeGraph::new();
        let a = graph.add_node(make_proc("a", None));
        let b = graph.add_node(make_proc("b", None));
        let c = graph.add_node(make_proc("c", None));

        graph.add_edge(
            a,
            b,
            Edge::DirectCall {
                scope: CallScope::CrossPackage,
                location: SourceLocation {
                    file: make_file(),
                    line: 1,
                },
            },
        );
        graph.add_edge(
            b,
            c,
            Edge::DirectCall {
                scope: CallScope::CrossPackage,
                location: SourceLocation {
                    file: make_file(),
                    line: 1,
                },
            },
        );

        let paths = GraphTraversal::new(&graph, a)
            .outgoing()
            .edge_filter(EdgeFilter::calls_only())
            .collect_paths();

        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], vec![a, b, c]);
    }

    #[test]
    fn node_filter_restricts_results() {
        let mut graph = CodeGraph::new();
        let proc = graph.add_node(make_proc("proc", None));
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
                    line: 1,
                },
                column_analysis: None,
            },
        );

        let nodes = GraphTraversal::new(&graph, proc)
            .outgoing()
            .node_filter(NodeFilter::new().with_type("table*"))
            .collect_nodes();

        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0], table);
    }

    #[test]
    fn max_paths_caps_collect_paths() {
        let mut graph = CodeGraph::new();
        let a = graph.add_node(make_proc("a", None));
        let b = graph.add_node(make_proc("b", None));
        let c = graph.add_node(make_proc("c", None));
        graph.add_edge(
            a,
            b,
            Edge::DirectCall {
                scope: CallScope::CrossPackage,
                location: SourceLocation {
                    file: make_file(),
                    line: 1,
                },
            },
        );
        graph.add_edge(
            a,
            c,
            Edge::DirectCall {
                scope: CallScope::CrossPackage,
                location: SourceLocation {
                    file: make_file(),
                    line: 1,
                },
            },
        );

        // a→b and a→c are two paths. max_paths(1) should return at most 1.
        let paths = GraphTraversal::new(&graph, a)
            .outgoing()
            .edge_filter(EdgeFilter::calls_only())
            .max_paths(1)
            .collect_paths();

        assert_eq!(paths.len(), 1);
    }

    #[test]
    fn max_paths_caps_collect_paths_to_target() {
        let mut graph = CodeGraph::new();
        let a = graph.add_node(make_proc("a", None));
        let x = graph.add_node(make_proc("x", None));
        let y = graph.add_node(make_proc("y", None));
        let b = graph.add_node(make_proc("b", None));
        let loc = SourceLocation {
            file: make_file(),
            line: 1,
        };
        graph.add_edge(
            a,
            x,
            Edge::DirectCall {
                scope: CallScope::CrossPackage,
                location: loc.clone(),
            },
        );
        graph.add_edge(
            x,
            b,
            Edge::DirectCall {
                scope: CallScope::CrossPackage,
                location: loc.clone(),
            },
        );
        graph.add_edge(
            a,
            y,
            Edge::DirectCall {
                scope: CallScope::CrossPackage,
                location: loc.clone(),
            },
        );
        graph.add_edge(
            y,
            b,
            Edge::DirectCall {
                scope: CallScope::CrossPackage,
                location: loc,
            },
        );

        // Two paths a→x→b and a→y→b. max_paths(1) should cap at 1.
        let paths = GraphTraversal::new(&graph, a)
            .outgoing()
            .edge_filter(EdgeFilter::calls_only())
            .max_paths(1)
            .target(b)
            .collect_paths_to_target();

        assert_eq!(paths.len(), 1);
    }
}

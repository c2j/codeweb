use crate::graph::{node_type_tag, Edge, EdgeCategory, Node};

type NodePredicate = Box<dyn Fn(&Node) -> bool + Send + Sync>;

pub struct NodeFilter {
    predicates: Vec<NodePredicate>,
}

impl NodeFilter {
    pub fn new() -> Self {
        Self {
            predicates: Vec::new(),
        }
    }

    pub fn with_type(mut self, tag: &str) -> Self {
        let tag = tag.to_lowercase();
        self.predicates.push(Box::new(move |n| {
            node_type_tag(n).eq_ignore_ascii_case(&tag)
        }));
        self
    }

    pub fn with_schema(mut self, schema: &str) -> Self {
        let schema = schema.to_lowercase();
        self.predicates.push(Box::new(move |n| {
            crate::graph::store::extract_schema(n)
                .map(|s| s.eq_ignore_ascii_case(&schema))
                .unwrap_or(false)
        }));
        self
    }

    pub fn with_predicate(mut self, pred: impl Fn(&Node) -> bool + Send + Sync + 'static) -> Self {
        self.predicates.push(Box::new(pred));
        self
    }

    pub fn matches(&self, node: &Node) -> bool {
        self.predicates.iter().all(|p| p(node))
    }
}

impl Default for NodeFilter {
    fn default() -> Self {
        Self::new()
    }
}

type EdgePredicate = Box<dyn Fn(&Edge) -> bool + Send + Sync>;

pub struct EdgeFilter {
    categories: Option<Vec<EdgeCategory>>,
    predicate: Option<EdgePredicate>,
}

impl EdgeFilter {
    pub fn new() -> Self {
        Self {
            categories: None,
            predicate: None,
        }
    }

    pub fn with_category(mut self, cat: EdgeCategory) -> Self {
        self.categories.get_or_insert_with(Vec::new).push(cat);
        self
    }

    pub fn from_categories(cats: &[EdgeCategory]) -> Self {
        let mut filter = Self::new();
        for &cat in cats {
            filter = filter.with_category(cat);
        }
        filter
    }

    pub fn calls_only() -> Self {
        Self::new().with_category(EdgeCategory::Call)
    }

    pub fn data_flow() -> Self {
        Self::new().with_category(EdgeCategory::DataFlow)
    }

    pub fn with_predicate(mut self, pred: impl Fn(&Edge) -> bool + Send + Sync + 'static) -> Self {
        self.predicate = Some(Box::new(pred));
        self
    }

    pub fn matches(&self, edge: &Edge) -> bool {
        if let Some(ref cats) = self.categories {
            if !cats.contains(&edge.category()) {
                return false;
            }
        }
        if let Some(ref pred) = self.predicate {
            if !pred(edge) {
                return false;
            }
        }
        true
    }
}

impl Default for EdgeFilter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    #[test]
    fn node_filter_with_type_matches() {
        let proc = Node::Procedure {
            id: RoutineId {
                schema: None,
                package: None,
                name: "test".into(),
                kind: RoutineKind::Procedure,
            },
            location: SourceLocation {
                file: Arc::new(PathBuf::from("test.sql")),
                line: 1,
            },
            partial: false,
            body_sql: Vec::new(),
        };
        let filter = NodeFilter::new().with_type("proc");
        assert!(filter.matches(&proc));
    }

    #[test]
    fn edge_filter_calls_only() {
        let call_edge = Edge::DirectCall {
            scope: CallScope::IntraPackage,
            location: SourceLocation {
                file: Arc::new(PathBuf::from("test.sql")),
                line: 1,
            },
        };
        let data_edge = Edge::TableAccess {
            flow_kind: DataFlowKind::DmlAccess,
            modes: AccessMode::Read,
            write_kinds: std::collections::HashSet::new(),
            location: SourceLocation {
                file: Arc::new(PathBuf::from("test.sql")),
                line: 1,
            },
            column_analysis: None,
        };
        let filter = EdgeFilter::calls_only();
        assert!(filter.matches(&call_edge));
        assert!(!filter.matches(&data_edge));
    }
}

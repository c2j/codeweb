use std::fmt;

/// Match mode for node name resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchMode {
    /// Case-insensitive substring matching (default, current behavior).
    Substring,
    /// Exact case-insensitive key match.
    Exact,
    /// Compiled regex matching against the lowercase key.
    Regex,
}

/// Result of resolving a single node name to one or more graph nodes.
#[derive(Debug)]
pub enum ResolveResult {
    /// Exactly one match found.
    Single(petgraph::graph::NodeIndex, String),
    /// Multiple matches found (returned when `all_matches` is true).
    Multiple(Vec<(petgraph::graph::NodeIndex, String)>),
    /// No matches found.
    Empty,
    /// Multiple matches found but `fail_on_multiple` was set — the query is
    /// ambiguous and should be treated as an error by the caller.
    Ambiguous,
}

impl fmt::Display for ResolveResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveResult::Single(_, name) => write!(f, "1 match: {}", name),
            ResolveResult::Multiple(matches) => {
                write!(f, "{} matches", matches.len())
            }
            ResolveResult::Empty => write!(f, "0 matches"),
            ResolveResult::Ambiguous => write!(f, "ambiguous match"),
        }
    }
}

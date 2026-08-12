#[macro_use]
extern crate rust_i18n;

i18n!("locales", fallback = "en");

mod error;
mod export;
mod graph;
#[allow(dead_code)]
mod import;
mod mark;
#[cfg(feature = "mcp")]
mod mcp;
mod parse_log;
#[allow(dead_code)]
mod parser;
#[allow(dead_code)]
mod project;
#[cfg(feature = "serve")]
mod server;
mod sql_match;
#[cfg(feature = "tui")]
mod tui;

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;

/// Like `println_stdout!` but handles `BrokenPipe` gracefully (exit 0 instead of panic).
macro_rules! println_stdout {
    () => {{
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        match writeln!(handle) {
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => std::process::exit(0),
            Err(e) => panic!("{e}"),
            Ok(_) => {}
        }
    }};
    ($($arg:tt)*) => {{
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        match writeln!(handle, $($arg)*) {
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => std::process::exit(0),
            Err(e) => panic!("{e}"),
            Ok(_) => {}
        }
    }};
}

use clap::{Parser, Subcommand};
use error::Result;
use graph::Node;
use serde::Serialize;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SortKey {
    Name,
    Type,
    In,
    Out,
    Total,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SortDir {
    Asc,
    Desc,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct SortSpec {
    key: SortKey,
    dir: SortDir,
}

fn parse_sort_spec(s: &str) -> std::result::Result<SortSpec, String> {
    let (key_str, dir_str) = match s.split_once(':') {
        Some((k, d)) => (k, d),
        None => (s, "asc"),
    };
    let key = match key_str.to_ascii_lowercase().as_str() {
        "name" => SortKey::Name,
        "type" => SortKey::Type,
        "in" => SortKey::In,
        "out" => SortKey::Out,
        "total" => SortKey::Total,
        other => {
            return Err(format!(
                "unknown sort key '{other}' (allowed: name, type, in, out, total)"
            ))
        }
    };
    let dir = match dir_str.to_ascii_lowercase().as_str() {
        "asc" => SortDir::Asc,
        "desc" => SortDir::Desc,
        other => {
            return Err(format!(
                "unknown sort direction '{other}' (allowed: asc, desc)"
            ))
        }
    };
    Ok(SortSpec { key, dir })
}

struct NodeRow {
    in_deg: usize,
    out_deg: usize,
    total: usize,
    tag: String,
    name: String,
}

fn compare_rows(a: &NodeRow, b: &NodeRow, specs: &[SortSpec]) -> std::cmp::Ordering {
    for spec in specs {
        let raw = match spec.key {
            SortKey::Name => a.name.cmp(&b.name),
            SortKey::Type => a.tag.cmp(&b.tag),
            SortKey::In => a.in_deg.cmp(&b.in_deg),
            SortKey::Out => a.out_deg.cmp(&b.out_deg),
            SortKey::Total => a.total.cmp(&b.total),
        };
        let ord = if spec.dir == SortDir::Desc {
            raw.reverse()
        } else {
            raw
        };
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    std::cmp::Ordering::Equal
}

/// JSON output schema entry for upstream/downstream
#[derive(Serialize, PartialEq, Eq, Hash, Clone)]
struct ImpactEntry {
    file_path: Option<String>,
    symbol: String,
    line: Option<usize>,
}

/// `impact --file` / `impact --node` JSON output schema (schema_version=2)
#[derive(Serialize)]
struct ImpactResult {
    schema_version: u32,
    /// `--file` 入口时为 Some,`--node` 入口时为 None
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    /// `--node` 入口时为 Some,`--file` 入口时为 None
    #[serde(skip_serializing_if = "Option::is_none")]
    node: Option<String>,
    upstream: Vec<ImpactEntry>,
    downstream: Vec<ImpactEntry>,
}

const LOGO: &str = r#"
  ██████╗  ██████╗  ██████╗  ███████╗ ██╗    ██╗ ███████╗ ██████╗
 ██╔════╝ ██╔═══██╗ ██╔══██╗ ██╔════╝ ██║    ██║ ██╔════╝ ██╔══██╗
 ██║      ██║   ██║ ██║  ██║ ███████╗ ██║ █╗ ██║ ███████╗ ██████╔╝
 ██║      ██║   ██║ ██║  ██║ ██╔═══╝  ██║███╗██║ ██╔═══╝  ██╔══██╗
 ╚██████╗ ╚██████╔╝ ██████╔╝ ███████╗ ╚███╔███╔╝ ███████╗ ██████╔╝
  ╚═════╝  ╚═════╝  ╚═════╝  ╚══════╝  ╚══╝╚══╝  ╚══════╝ ╚═════╝
  "#;

fn print_banner() {
    println_stdout!("{}", LOGO);
    println_stdout!();
    println_stdout!("  codeweb v{}", env!("CARGO_PKG_VERSION"));
    println_stdout!("  Semantic code graph analyzer");
    println_stdout!();
}

#[derive(Parser)]
#[command(
    name = "codeweb",
    version,
    about = "Semantic code graph analyzer — call graphs for SQL stored procedures"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Input file or directory (legacy mode, no subcommand)
    input: Option<PathBuf>,

    /// Output format
    #[arg(long, default_value = "dot", value_parser = ["dot", "json", "mermaid", "ndjson"])]
    format: String,

    /// Output file (stdout if omitted)
    #[arg(long)]
    output: Option<PathBuf>,

    /// Include unresolved/dynamic call nodes
    #[arg(long)]
    include_unresolved: bool,

    /// Only scan SQL files (ignore Java and XML)
    #[arg(long)]
    sql_only: bool,

    /// Output language (zh-CN or en, default: zh-CN)
    #[arg(long, global = true)]
    lang: Option<String>,

    /// Number of worker threads for parallel file parsing.
    /// Default: max(4, logical_cpus - 2). Set lower to reduce peak memory.
    /// Also respects RAYON_NUM_THREADS env var (CLI flag takes precedence).
    #[arg(long, global = true)]
    threads: Option<usize>,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a new codeweb project
    Init {
        /// Project name
        name: String,

        /// Source directories to analyze (can specify multiple)
        #[arg(short, long, num_args = 1..)]
        dir: Vec<PathBuf>,
    },

    /// Analyze project (full or incremental)
    Analyze {
        /// Project directory (default: current directory)
        #[arg(short, long, default_value = ".")]
        project: PathBuf,

        /// Enable column-level lineage analysis
        #[arg(long)]
        column_lineage: bool,
    },

    /// Show changes since last analysis
    Diff {
        /// Project directory (default: current directory)
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
    },

    /// Export graph to various formats
    Export {
        /// Output format
        #[arg(short, long, default_value = "dot", value_parser = ["dot", "json", "mermaid", "ndjson"])]
        format: String,

        /// Output file (stdout if omitted)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Project directory (default: current directory)
        #[arg(short, long, default_value = ".")]
        project: PathBuf,

        /// Filter nodes by name (substring match), export only the matching
        /// subgraph (seed nodes + direct neighbors + edges between them).
        #[arg(long)]
        filter: Option<String>,
    },

    /// Merge multiple project stores
    Merge {
        /// Store files to merge
        stores: Vec<PathBuf>,

        /// Output store file
        #[arg(short, long)]
        output: PathBuf,

        /// Merged project name
        #[arg(long, default_value = "merged")]
        name: String,
    },

    /// Open interactive TUI
    #[cfg(feature = "tui")]
    Tui {
        /// Project directory (default: current directory)
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
    },

    /// Start HTTP server with browser-based graph viewer
    ///
    /// Launches a local web server that serves an interactive UI for exploring
    /// the code graph. The UI provides a node list with search, a Cytoscape.js
    /// graph canvas with dagre layout, and a detail panel showing callers/callees.
    ///
    /// Requires the "serve" feature flag: cargo run --features serve -- serve
    #[cfg(feature = "serve")]
    Serve {
        /// Project directory (default: current directory)
        #[arg(short, long, default_value = ".")]
        project: PathBuf,

        /// Bind address (host:port)
        #[arg(short, long, default_value = "127.0.0.1:3000")]
        addr: String,

        /// Open browser automatically after server starts
        #[arg(long)]
        open: bool,

        /// Access log level: info (default) or debug
        #[arg(long, default_value = "info", value_parser = ["info", "debug"])]
        log_level: String,
    },

    /// Start MCP server for LLM integration (stdio JSON-RPC)
    ///
    /// Launches a Model Context Protocol server over stdio, enabling LLM clients
    /// (Claude Desktop, Cursor, etc.) to query the code graph via JSON-RPC tools.
    ///
    /// Requires the "mcp" feature flag: cargo run --features mcp -- mcp
    #[cfg(feature = "mcp")]
    Mcp {
        /// Project directory (default: current directory)
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
    },

    /// Trace complete call chain from a node
    Trace {
        /// Node name to search for (substring match)
        from: String,

        /// Project directory
        #[arg(short, long, default_value = ".")]
        project: PathBuf,

        /// Display style for call chain output
        #[arg(short, long, default_value = "tree", value_parser = ["tree", "path"])]
        style: String,

        /// Show built-in function calls in the chain (default: hidden)
        #[arg(long = "builtfunc")]
        builtfunc: bool,

        /// Exact case-insensitive key match
        #[arg(long, conflicts_with = "regex")]
        exact: bool,

        /// Regex match against node keys
        #[arg(long, conflicts_with = "exact")]
        regex: bool,

        /// Process all matching nodes
        #[arg(long)]
        all_matches: bool,

        /// Exit with error on ambiguous match
        #[arg(long)]
        fail_on_multiple: bool,
    },

    /// Show project statistics
    Stats {
        /// Project directory (default: current directory)
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
    },

    /// List analyzed files with node counts
    Files {
        /// Project directory (default: current directory)
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
    },

    /// List graph nodes with optional filtering
    Nodes {
        /// Search nodes by name (substring match)
        #[arg(short, long)]
        search: Option<String>,

        /// Limit the number of search results returned
        #[arg(long)]
        limit: Option<usize>,

        /// Show only orphan nodes (no connections)
        #[arg(long)]
        orphan: bool,

        /// Show nodes with total degree ≤ N
        #[arg(long)]
        low_degree: Option<usize>,

        /// Filter by node type (proc, mapper, method, class, table, view, unres)
        #[arg(short = 't', long)]
        node_type: Option<String>,

        /// Show only partitioned tables
        #[arg(long)]
        has_partition: bool,

        /// Show only distributed tables
        #[arg(long)]
        has_distribute: bool,

        /// Show only inferred nodes (tables/views without DDL definition)
        #[arg(long)]
        inferred: bool,

        /// Show only system tables/views (pg_catalog, sys, dbe_*, etc.)
        #[arg(long)]
        system: bool,

        /// Sort keys (comma-separated, left=primary). Format: key[:dir].
        /// Keys: name, type, in, out, total. Dir: asc (default), desc.
        #[arg(
            long,
            value_delimiter = ',',
            value_parser = parse_sort_spec,
        )]
        sort_by: Option<Vec<SortSpec>>,

        /// Project directory (default: current directory)
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
    },

    /// Mark WDR SQL detail CSV rows by relation to a target graph node
    Mark {
        /// Target node name(s) — can specify multiple (table, procedure, or function)
        #[arg(short, long, num_args = 1..)]
        node: Vec<String>,

        /// Input CSV file from WDR SQL detail report
        #[arg(short, long)]
        csv: PathBuf,

        /// Output CSV file (stdout if omitted)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Project directory (default: current directory)
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
    },

    /// Import a CGEF JSON graph file into a standalone GraphStore
    Import {
        /// Path to the CGEF JSON file to import
        #[arg(short, long)]
        file: PathBuf,

        /// Output path for the generated GraphStore (.bincode or .json)
        #[arg(short, long)]
        output: PathBuf,

        /// Path prefix to prepend to all relative file paths
        #[arg(short, long)]
        prefix: Option<String>,

        /// Project name for the imported GraphStore
        #[arg(short, long)]
        name: Option<String>,

        /// Force import even when validation or parse errors are found
        #[arg(long)]
        force: bool,
    },

    /// Show callers/callees detail for one or more nodes
    Detail {
        /// Node name(s) to search for (substring match)
        names: Vec<String>,

        /// Project directory (default: current directory)
        #[arg(short, long, default_value = ".")]
        project: PathBuf,

        /// Display style for call chain output
        #[arg(short, long, default_value = "tree", value_parser = ["tree", "path"])]
        style: String,

        /// Traversal depth: 1 = direct callers/callees, 0 = target only, -1 = unlimited
        #[arg(short = 'd', long, default_value = "1", allow_hyphen_values = true)]
        depth: i64,

        /// Show source files involved in the upstream/downstream chain
        #[arg(short, long)]
        files: bool,

        /// Show built-in function calls in the chain (default: hidden)
        #[arg(long = "builtfunc")]
        builtfunc: bool,

        /// Show DDL source text for views and other objects
        #[arg(short = 'v', long, default_value = "false")]
        verbose: bool,

        /// Exact case-insensitive key match
        #[arg(long, conflicts_with = "regex")]
        exact: bool,

        /// Regex match against node keys
        #[arg(long, conflicts_with = "exact")]
        regex: bool,

        /// Process all matching nodes
        #[arg(long)]
        all_matches: bool,

        /// Exit with error on ambiguous match
        #[arg(long)]
        fail_on_multiple: bool,

        /// Summarize R/W table access for a package by aggregating
        /// its child procedures' TableAccess edges.
        #[arg(long = "summarize-tables")]
        summarize_tables: bool,
    },

    /// Search MappedStatement and JavaSql nodes by SQL fragment, then
    /// trace back to the invoking Java methods (via InvokesMapper edges).
    ///
    /// Use --file to read the SQL fragment from a file (avoids shell quoting issues).
    TraceSql {
        /// SQL fragment to search for (substring match, case-insensitive).
        /// Omit this and use --file to read from a file instead.
        sql: Option<String>,

        /// Read the SQL fragment from a file (use - for stdin).
        /// This avoids shell quoting issues with SQL containing quotes.
        #[arg(short = 'f', long)]
        file: Option<PathBuf>,

        /// Project directory (default: current directory)
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
    },

    /// Deduplicate graph nodes and edges
    Dedup {
        /// Project directory (default: current directory)
        #[arg(short, long, default_value = ".")]
        project: PathBuf,

        /// Output deduplicated store to a new file (default: in-place)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Show what would be deduplicated without modifying the store
        #[arg(long)]
        dry_run: bool,
    },

    /// Execute a JSON query spec against the graph
    Query {
        /// Path to JSON query spec file (use - for stdin)
        #[arg(short, long)]
        file: Option<PathBuf>,

        /// Inline JSON query spec string
        #[arg(short, long)]
        spec: Option<String>,

        /// Project directory (default: current directory)
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
    },

    /// Partition graph nodes into k clusters for system decomposition analysis
    Partition {
        /// Target number of clusters (omit for auto-discovery)
        #[arg(short, long)]
        k: Option<usize>,

        /// Resolution parameter γ (lower = fewer/larger clusters, default 1.0)
        #[arg(long)]
        gamma: Option<f64>,

        /// Auto-discover optimal cluster count via γ sweep
        #[arg(long)]
        auto: bool,

        /// Cap natural-mode CNM iterations per sweep (e.g. 5000). Forced-k
        /// sweeps ignore this. Useful to bound runtime on very large graphs.
        #[arg(long)]
        max_iterations: Option<usize>,

        /// Stop merging when ΔQ falls at or below this threshold (natural mode
        /// only). Default 0.0; raise to prune negligible merges.
        #[arg(long)]
        min_delta_q: Option<f64>,

        /// Minimum weakly connected component size to participate in clustering.
        /// Components smaller than this are reported but excluded from clustering.
        /// Use to focus on the giant component when the graph has many isolates.
        /// Default: 1 (all components participate).
        #[arg(long, default_value = "1")]
        min_component_size: usize,

        /// Enable TF-IDF table-access projection. Bridges procedures that
        /// share table accesses but don't call each other directly.
        /// Optional value format: "tau:lambda:k_neighbors" (e.g., "0.1:0.3:10").
        /// Bare flag uses defaults: tau=0.1, lambda=0.3, k=10.
        #[arg(long, num_args = 0..=1, default_missing_value = "0.1:0.3:10")]
        table_projection: Option<String>,

        /// Project directory (default: current directory)
        #[arg(short, long, default_value = ".")]
        project: PathBuf,

        /// Export clustered graph as DOT to file (with subgraph cluster_* blocks)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Show upstream callers and downstream callees for a file or a node
    ///
    /// Pass `--file <path>` to aggregate impact across all nodes defined in
    /// that file (useful for subprocess integration with `git diff`).
    /// Pass `--node <name>` to query a single symbol (e.g. a procedure,
    /// Java method, or mapper id). The two flags are mutually exclusive.
    Impact {
        /// File path(s) to analyze (relative to CWD or absolute).
        /// Repeatable for batch queries. Mutually exclusive with --node.
        #[arg(long, action = clap::ArgAction::Append)]
        file: Vec<PathBuf>,

        /// Node symbol name to analyze (e.g. "proc_create_order").
        /// Supports fuzzy match like `trace`/`detail`. Mutually exclusive with --file.
        #[arg(long)]
        node: Option<String>,

        /// Project directory (default: current directory)
        #[arg(short, long, default_value = ".")]
        project: PathBuf,

        /// Output format (json for integration, text for human reading)
        #[arg(short, long, default_value = "json", value_parser = ["json", "text"])]
        format: String,

        /// Traversal depth (1 = direct callers/callees only)
        #[arg(short, long, default_value = "1")]
        depth: usize,

        /// Edge types to include in impact traversal (comma-separated).
        /// Valid: all, call, dataflow, reference, composition, inheritance.
        /// Default "all" includes every edge type.
        #[arg(short = 'e', long, default_value = "all", value_delimiter = ',')]
        edge_types: Vec<String>,

        /// Exact case-insensitive key match
        #[arg(long, conflicts_with = "regex")]
        exact: bool,

        /// Regex match against node keys
        #[arg(long, conflicts_with = "exact")]
        regex: bool,
    },

    /// Query table-level or column-level data lineage
    ///
    /// Pass a table name for table-level lineage, or "table.column" for
    /// column-level lineage. Requires column lineage to be enabled during
    /// analysis (`codeweb analyze --column-lineage`).
    Lineage {
        /// Starting node: "table_name" (table-level) or "table.column" (column-level)
        #[arg(value_name = "TARGET")]
        target: String,

        /// Direction: upstream (backward), downstream (forward), or both
        #[arg(short = 'd', long, default_value = "upstream",
              value_parser = ["upstream", "downstream", "both"])]
        direction: String,

        /// Maximum depth (default: 10)
        #[arg(long, default_value = "10")]
        depth: usize,

        /// Output format: tree (default), table, or json
        #[arg(short = 'f', long, default_value = "tree",
              value_parser = ["tree", "table", "json"])]
        format: String,

        /// Output file (default: stdout)
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,

        /// Project directory (default: current directory)
        #[arg(short = 'p', long, default_value = ".")]
        project: PathBuf,

        /// Show intermediate procedure-level column nodes (default: hide)
        #[arg(long)]
        show_procedures: bool,
    },

    /// Find reachability paths connecting multiple nodes
    ///
    /// Given two or more node names, discovers all directed paths
    /// between them across all edge types. Unreachable pairs are hidden
    /// by default; use --unreachable to show all pairs.
    Inspect {
        /// Node names to inspect (substring match, at least 2 required)
        nodes: Vec<String>,

        /// Project directory (default: current directory)
        #[arg(short, long, default_value = ".")]
        project: PathBuf,

        /// Max traversal depth between any two nodes
        #[arg(short, long, default_value_t = 15)]
        max_depth: usize,

        /// Max paths to show per node pair
        #[arg(long, default_value_t = 10)]
        max_paths: usize,

        /// Display style
        #[arg(short, long, default_value = "both", value_parser = ["summary", "paths", "both", "tree"])]
        style: String,

        /// Show unreachable node pairs (0 paths) in output
        #[arg(long, default_value = "false")]
        unreachable: bool,

        /// Exact case-insensitive key match
        #[arg(long, conflicts_with = "regex")]
        exact: bool,

        /// Regex match against node keys
        #[arg(long, conflicts_with = "exact")]
        regex: bool,

        /// Process all matching nodes (show paths for all combinations)
        #[arg(long)]
        all_matches: bool,

        /// Exit with error on ambiguous match
        #[arg(long)]
        fail_on_multiple: bool,
    },
}

fn match_mode_from_flags(exact: bool, regex: bool) -> crate::graph::search::MatchMode {
    if exact {
        crate::graph::search::MatchMode::Exact
    } else if regex {
        crate::graph::search::MatchMode::Regex
    } else {
        crate::graph::search::MatchMode::Substring
    }
}

fn main() {
    // Run main work in a thread with an enlarged stack.
    //
    // Windows main thread default stack = 1 MB (Linux/macOS = 8 MB).
    // ogsql-parser uses a recursive descent walker that recurses deeply on
    // PL/pgSQL with nested SELECT/subquery expressions, which overflows the
    // 1 MB stack on Windows (regress_where_subquery). 8 MB matches the Unix
    // default and is also what rayon worker threads already use (see above).
    let exit_code = std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            if let Err(e) = run() {
                eprintln!("error: {}", e);
                1
            } else {
                0
            }
        })
        .expect("failed to spawn main worker thread")
        .join()
        .unwrap_or(1);

    std::process::exit(exit_code);
}

fn init_thread_pool(cli_threads: Option<usize>) {
    let threads = resolve_thread_count(cli_threads);

    let builder = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|idx| format!("codeweb-worker-{idx}"))
        .spawn_handler(|thread| {
            std::thread::Builder::new()
                .name(thread.name().unwrap_or_default().to_string())
                .stack_size(4 * 1024 * 1024)
                .spawn(move || {
                    #[cfg(target_os = "macos")]
                    {
                        let qos: libc::qos_class_t = unsafe { std::mem::transmute(0x11u32) };
                        let _ = unsafe { libc::pthread_set_qos_class_self_np(qos, 0) };
                    }
                    #[cfg(target_os = "linux")]
                    {
                        let _ = unsafe { libc::nice(10) };
                    }
                    thread.run()
                })
                .map(|_| ())
        });

    if let Err(e) = builder.build_global() {
        eprintln!("warning: failed to configure thread pool: {e}");
    }
}

fn resolve_thread_count(cli_threads: Option<usize>) -> usize {
    if let Some(n) = cli_threads.filter(|&n| n > 0) {
        return n;
    }
    if let Ok(val) = std::env::var("RAYON_NUM_THREADS") {
        if let Ok(n) = val.parse::<usize>() {
            if n > 0 {
                return n;
            }
        }
    }
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    std::cmp::max(4, cores - 2)
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let show_banner =
        args.len() == 1 || args.contains(&"--help".to_string()) || args.contains(&"-h".to_string());

    if show_banner {
        print_banner();
    }

    let cli = Cli::parse();

    init_thread_pool(cli.threads);

    if let Some(ref lang) = cli.lang {
        rust_i18n::set_locale(lang);
    } else {
        rust_i18n::set_locale("zh-CN");
    }

    match cli.command {
        None => {
            if cli.input.is_none() {
                println_stdout!("Usage: codeweb <COMMAND>");
                println_stdout!();
                println_stdout!("Run `codeweb --help` for more information.");
                return Ok(());
            }
            cmd_legacy(cli)
        }
        Some(Commands::Init { name, dir }) => cmd_init(&name, &dir),
        Some(Commands::Analyze {
            project,
            column_lineage,
        }) => cmd_analyze(&project, column_lineage),
        Some(Commands::Diff { project }) => cmd_diff(&project),
        Some(Commands::Export {
            format,
            output,
            project,
            filter,
        }) => cmd_export(&format, output.as_deref(), &project, filter.as_deref()),
        Some(Commands::Merge {
            stores,
            output,
            name,
        }) => cmd_merge(&stores, &output, &name),
        #[cfg(feature = "tui")]
        Some(Commands::Tui { project }) => cmd_tui(&project),
        #[cfg(feature = "serve")]
        Some(Commands::Serve {
            project,
            addr,
            open,
            log_level,
        }) => server::run(&project, &addr, open, &log_level),
        #[cfg(feature = "mcp")]
        Some(Commands::Mcp { project }) => mcp::server::run(&project),
        Some(Commands::Trace {
            from,
            project,
            style,
            builtfunc,
            exact,
            regex,
            all_matches,
            fail_on_multiple,
        }) => cmd_trace(
            &from,
            &project,
            &style,
            builtfunc,
            match_mode_from_flags(exact, regex),
            all_matches,
            fail_on_multiple,
        ),
        Some(Commands::Stats { project }) => cmd_stats(&project),
        Some(Commands::Files { project }) => cmd_files(&project),
        Some(Commands::Nodes {
            search,
            orphan,
            low_degree,
            node_type,
            has_partition,
            has_distribute,
            inferred,
            system,
            sort_by,
            project,
            limit,
        }) => cmd_nodes(
            search.as_deref(),
            orphan,
            low_degree,
            node_type.as_deref(),
            has_partition,
            has_distribute,
            inferred,
            system,
            sort_by.as_deref(),
            &project,
            limit,
        ),
        Some(Commands::Mark {
            node,
            csv,
            output,
            project,
        }) => {
            let mut proj = project::Project::find(&project)?;
            let store = proj.load_store()?;
            mark::process_mark(store, &node, &csv, output.as_deref())
        }
        Some(Commands::Detail {
            names,
            project,
            style,
            depth,
            files,
            builtfunc,
            verbose,
            exact,
            regex,
            all_matches,
            fail_on_multiple,
            summarize_tables,
        }) => cmd_detail(
            &names,
            &project,
            &style,
            depth,
            files,
            builtfunc,
            verbose,
            match_mode_from_flags(exact, regex),
            all_matches,
            fail_on_multiple,
            summarize_tables,
        ),
        Some(Commands::Import {
            file,
            output,
            prefix,
            name,
            force,
        }) => cmd_import(&file, &output, prefix.as_deref(), name.as_deref(), force),
        Some(Commands::TraceSql { sql, file, project }) => {
            cmd_trace_sql(sql.as_deref(), file.as_deref(), &project)
        }
        Some(Commands::Query {
            file,
            spec,
            project,
        }) => cmd_query(file.as_deref(), spec.as_deref(), &project),
        Some(Commands::Dedup {
            project,
            output,
            dry_run,
        }) => cmd_dedup(&project, output.as_deref(), dry_run),
        Some(Commands::Partition {
            k,
            gamma,
            auto,
            max_iterations,
            min_delta_q,
            min_component_size,
            table_projection,
            project,
            output,
        }) => cmd_partition(
            k,
            gamma,
            auto,
            max_iterations,
            min_delta_q,
            min_component_size,
            table_projection,
            &project,
            output.as_deref(),
        ),
        Some(Commands::Impact {
            file,
            node,
            project,
            format,
            depth,
            edge_types,
            exact,
            regex,
        }) => {
            if !file.is_empty() && node.is_some() {
                eprintln!("Error: --file and --node are mutually exclusive. Pass exactly one.");
                std::process::exit(2);
            }
            if file.is_empty() && node.is_none() {
                eprintln!("Error: must pass exactly one of --file <path> or --node <name>.");
                std::process::exit(2);
            }
            cmd_impact(
                &file,
                node.as_deref(),
                &project,
                &format,
                depth,
                &edge_types,
                match_mode_from_flags(exact, regex),
            )
        }
        Some(Commands::Lineage {
            target,
            direction,
            depth,
            format,
            output,
            project,
            show_procedures,
        }) => cmd_lineage(
            &target,
            &direction,
            depth,
            &format,
            output.as_deref(),
            &project,
            show_procedures,
        ),
        Some(Commands::Inspect {
            nodes,
            project,
            max_depth,
            max_paths,
            style,
            unreachable,
            exact,
            regex,
            all_matches,
            fail_on_multiple,
        }) => {
            if nodes.len() < 2 {
                eprintln!("Error: inspect requires at least 2 node names.");
                std::process::exit(2);
            }
            cmd_inspect(
                &nodes,
                &project,
                max_depth,
                max_paths,
                &style,
                unreachable,
                match_mode_from_flags(exact, regex),
                all_matches,
                fail_on_multiple,
            )
        }
    }
}

fn cmd_init(name: &str, dirs: &[PathBuf]) -> Result<()> {
    let mut proj = project::Project::init(dirs, name)?;
    eprintln!(
        "Initialized project '{}' in {}",
        proj.name(),
        proj.root().display()
    );
    let report = proj.analyze(false)?;
    print_analyze_report(&report);
    Ok(())
}

fn cmd_analyze(project: &Path, column_lineage: bool) -> Result<()> {
    let mut proj = project::Project::find(project)?;
    let report = proj.analyze(column_lineage)?;
    print_analyze_report(&report);
    Ok(())
}

fn cmd_diff(project: &Path) -> Result<()> {
    let mut proj = project::Project::find(project)?;
    let changes = proj.diff()?;
    if changes.is_empty() {
        eprintln!("Up to date. No changes detected.");
    } else {
        eprintln!(
            "{} modified, {} added, {} deleted, {} unchanged",
            changes.modified.len(),
            changes.added.len(),
            changes.deleted.len(),
            changes.unchanged.len()
        );
        for p in &changes.modified {
            eprintln!("  M {}", p.display());
        }
        for p in &changes.added {
            eprintln!("  + {}", p.display());
        }
        for p in &changes.deleted {
            eprintln!("  - {}", p.display());
        }
    }
    Ok(())
}

fn cmd_export(
    format: &str,
    output: Option<&Path>,
    project: &Path,
    filter: Option<&str>,
) -> Result<()> {
    let mut proj = project::Project::find(project)?;
    let store = proj.load_store()?;

    let full_graph = store.graph();
    let filtered_graph;
    let graph_to_export = if let Some(filter_query) = filter {
        filtered_graph = build_filtered_subgraph(full_graph, store, filter_query);
        &filtered_graph
    } else {
        full_graph
    };

    match format {
        "ndjson" => {
            let mut writer: Box<dyn std::io::Write> = match output {
                Some(path) => Box::new(std::fs::File::create(path).map_err(|source| {
                    error::CodeWebError::FileRead {
                        path: path.to_path_buf(),
                        source,
                    }
                })?),
                None => Box::new(std::io::stdout()),
            };
            export::ndjson::to_ndjson(graph_to_export, &mut writer)
        }
        "dot" => write_output(&export::dot::to_dot(graph_to_export), output),
        "json" => write_output(&export::json::to_json(graph_to_export)?, output),
        "mermaid" => write_output(&export::mermaid::to_mermaid(graph_to_export), output),
        _ => unreachable!(),
    }
}

fn build_filtered_subgraph(
    full_graph: &crate::graph::CodeGraph,
    store: &crate::graph::store::GraphStore,
    filter_query: &str,
) -> crate::graph::CodeGraph {
    use petgraph::graph::NodeIndex;
    use std::collections::HashSet;

    let matches = store.search_nodes(filter_query);

    if matches.is_empty() {
        eprintln!(
            "Filter '{}' matched 0 nodes; exporting empty graph.",
            filter_query
        );
        return crate::graph::CodeGraph::new();
    }

    eprintln!("Filter '{}': {} seed nodes", filter_query, matches.len());

    // Collect seed node indices + their direct neighbors (1-hop)
    let mut selected: HashSet<NodeIndex> = HashSet::new();
    for (idx, _) in &matches {
        selected.insert(*idx);
        // Add outgoing neighbors
        for neighbor in full_graph.neighbors_directed(*idx, petgraph::Direction::Outgoing) {
            selected.insert(neighbor);
        }
        // Add incoming neighbors
        for neighbor in full_graph.neighbors_directed(*idx, petgraph::Direction::Incoming) {
            selected.insert(neighbor);
        }
    }

    // Build filtered graph: copy selected nodes, keep edges between them
    let mut filtered = crate::graph::CodeGraph::new();
    let mut old_to_new: std::collections::HashMap<NodeIndex, NodeIndex> =
        std::collections::HashMap::new();

    // Copy nodes
    for old_idx in &selected {
        let new_idx = filtered.add_node(full_graph[*old_idx].clone());
        old_to_new.insert(*old_idx, new_idx);
    }

    // Copy edges where both endpoints are in selected set
    for edge_idx in full_graph.edge_indices() {
        if let Some((src, dst)) = full_graph.edge_endpoints(edge_idx) {
            if selected.contains(&src) && selected.contains(&dst) {
                if let (Some(&new_src), Some(&new_dst)) =
                    (old_to_new.get(&src), old_to_new.get(&dst))
                {
                    filtered.add_edge(new_src, new_dst, full_graph[edge_idx].clone());
                }
            }
        }
    }

    eprintln!(
        "Subgraph: {} nodes, {} edges",
        filtered.node_count(),
        filtered.edge_count()
    );
    filtered
}

fn cmd_merge(stores: &[PathBuf], output: &Path, name: &str) -> Result<()> {
    let mut loaded = Vec::new();
    for path in stores {
        let store = if path.extension().is_some_and(|e| e == "json") {
            graph::store::GraphStore::load_json(path)?
        } else {
            graph::store::GraphStore::load_bincode(path)?
        };
        eprintln!(
            "Loaded {} ({} nodes)",
            path.display(),
            store.graph().node_count()
        );
        loaded.push(store);
    }

    let merged = graph::store::GraphStore::merge(loaded, name);
    eprintln!(
        "Merged: {} nodes, {} edges",
        merged.graph().node_count(),
        merged.graph().edge_count()
    );
    merged.save_bincode(output)?;
    eprintln!("Saved to {}", output.display());
    Ok(())
}

fn cmd_dedup(project: &Path, output: Option<&Path>, dry_run: bool) -> Result<()> {
    let mut proj = project::Project::find(project)?;
    let store = proj.load_store()?;

    eprintln!(
        "Before: {} nodes, {} edges",
        store.graph().node_count(),
        store.graph().edge_count()
    );

    if dry_run {
        eprintln!("(dry-run mode — no changes made)");
        return Ok(());
    }

    let mut store = proj.take_store().unwrap();
    let report = store.dedup();

    eprintln!(
        "After:  {} nodes, {} edges (removed {} nodes, {} edges)",
        store.graph().node_count(),
        store.graph().edge_count(),
        report.nodes_removed,
        report.edges_removed,
    );

    if let Some(out_path) = output {
        store.save_bincode(out_path)?;
        eprintln!("Saved to {}", out_path.display());
    } else {
        proj.save_store(&store)?;
        eprintln!("Store updated in-place");
    }

    Ok(())
}

#[cfg(feature = "tui")]
fn cmd_tui(project: &Path) -> Result<()> {
    tui::run(project)
}

fn cmd_trace(
    from: &str,
    project: &Path,
    style: &str,
    show_builtins: bool,
    match_mode: crate::graph::search::MatchMode,
    all_matches: bool,
    fail_on_multiple: bool,
) -> Result<()> {
    let mut proj = project::Project::find(project)?;
    let store = proj.load_store()?;

    let graph = store.graph();
    let result = store.resolve_single_node(from, match_mode, all_matches, fail_on_multiple);

    let start_idx = match result {
        crate::graph::search::ResolveResult::Single(idx, _) => idx,
        crate::graph::search::ResolveResult::Multiple(matches) => {
            eprintln!("Processing all {} matches...", matches.len());
            for (idx, name) in &matches {
                eprintln!("  {}", name);
                let (chain, _) = graph::traverse::trace_chain(
                    graph,
                    *idx,
                    51,
                    usize::MAX,
                    !show_builtins && !matches!(graph[*idx], graph::Node::BuiltinFunction { .. }),
                );
                let chain_style: graph::traverse::ChainStyle = style.parse().unwrap_or_default();
                println_stdout!(
                    "{}",
                    graph::traverse::format_chain(&chain, graph, chain_style)
                );
            }
            return Ok(());
        }
        crate::graph::search::ResolveResult::Empty => {
            eprintln!("No nodes matching '{}'", from);
            return Ok(());
        }
        crate::graph::search::ResolveResult::Ambiguous => {
            eprintln!("Error: ambiguous match for '{}'", from);
            std::process::exit(2);
        }
    };

    let target_is_builtin = matches!(graph[start_idx], graph::Node::BuiltinFunction { .. });
    let skip_builtins = !show_builtins && !target_is_builtin;

    let (chain, _) = graph::traverse::trace_chain(graph, start_idx, 51, usize::MAX, skip_builtins);
    let chain_style: graph::traverse::ChainStyle = style.parse().unwrap_or_default();
    println_stdout!(
        "{}",
        graph::traverse::format_chain(&chain, graph, chain_style)
    );
    Ok(())
}

fn cmd_stats(project: &Path) -> Result<()> {
    let mut proj = project::Project::find(project)?;
    let store = proj.load_store()?;
    let stats = store.stats();

    println_stdout!("Project: {}", proj.name());
    println_stdout!();
    println_stdout!("  {:>12}  procedures", stats.procedures,);
    println_stdout!("  {:>12}  functions", stats.functions,);
    println_stdout!("  {:>12}  packages", stats.packages,);
    println_stdout!("  {:>12}  triggers", stats.triggers,);
    println_stdout!("  {:>12}  types", stats.types,);
    println_stdout!("  {:>12}  sequences", stats.sequences,);
    println_stdout!("  {:>12}  indexes", stats.indexes,);
    println_stdout!("  {:>12}  views", stats.views,);
    println_stdout!("  {:>12}  materialized views", stats.materialized_views,);
    println_stdout!("  {:>12}  synonyms", stats.synonyms,);
    println_stdout!("  {:>12}  events", stats.events,);
    println_stdout!("  {:>12}  tables", stats.tables,);
    println_stdout!("  {:>12}  mappers", stats.mappers,);
    println_stdout!("  {:>12}  java methods", stats.java_methods,);
    println_stdout!("  {:>12}  java classes", stats.java_classes,);
    if stats.java_sql > 0 {
        println_stdout!("  {:>12}  java sql sources", stats.java_sql,);
    }
    if stats.unresolved > 0 {
        println_stdout!("  {:>12}  unresolved", stats.unresolved,);
    }
    if stats.builtin_functions > 0 {
        println_stdout!("  {:>12}  builtin functions", stats.builtin_functions);
    }
    if stats.custom_nodes > 0 {
        println_stdout!("  {:>12}  custom nodes", stats.custom_nodes,);
    }
    #[cfg(feature = "jsp")]
    if stats.jsp_pages > 0 {
        println_stdout!("  {:>12}  jsp pages", stats.jsp_pages);
    }
    #[cfg(feature = "jsp")]
    if stats.jsp_sql > 0 {
        println_stdout!("  {:>12}  jsp sql sources", stats.jsp_sql);
    }
    println_stdout!();
    println_stdout!("  {:>12}  edges", stats.edges,);
    println_stdout!("  {:>12}  files", stats.files,);

    Ok(())
}

fn cmd_files(project: &Path) -> Result<()> {
    let mut proj = project::Project::find(project)?;
    let root = proj.root().to_path_buf();
    let store = proj.load_store();

    if let Ok(store) = store {
        let manifest: &std::collections::HashMap<std::path::PathBuf, _> = store.manifest();
        let file_nodes = store.file_nodes();
        let mut entries: Vec<_> = manifest.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));

        println_stdout!("{:<4} {:>5}  PATH", "TYPE", "NODES");
        for (path, record) in &entries {
            let rel = path.strip_prefix(&root).unwrap_or(path);
            let type_tag = match record.file_type {
                parser::fingerprint::FileType::Sql => "SQL",
                parser::fingerprint::FileType::Java => "Java",
                parser::fingerprint::FileType::Xml => "XML",
                #[cfg(feature = "jsp")]
                parser::fingerprint::FileType::Jsp => "JSP",
            };
            let node_count = file_nodes
                .get(path as &std::path::Path)
                .map(|v| v.len())
                .unwrap_or(0);
            println_stdout!(
                "{:<4} {:>5}  {}",
                type_tag,
                node_count,
                rel.to_string_lossy()
            );
        }
        println_stdout!();
        println_stdout!("{} files total", entries.len());
    } else {
        let report = proj.analyze(false)?;
        print_analyze_report(&report);
    }

    Ok(())
}

fn node_type_tag(node: &Node) -> std::borrow::Cow<'static, str> {
    match node {
        Node::Procedure { partial: true, .. } => std::borrow::Cow::Borrowed("proc*"),
        Node::Procedure { .. } => std::borrow::Cow::Borrowed("proc"),
        Node::Function { partial: true, .. } => std::borrow::Cow::Borrowed("func*"),
        Node::Function { .. } => std::borrow::Cow::Borrowed("func"),
        Node::Unresolved { .. } => std::borrow::Cow::Borrowed("unres"),
        Node::MappedStatement { .. } => std::borrow::Cow::Borrowed("mapper"),
        Node::JavaSql { .. } => std::borrow::Cow::Borrowed("sql"),
        Node::JavaMethod { .. } => std::borrow::Cow::Borrowed("method"),
        Node::JavaClass { .. } => std::borrow::Cow::Borrowed("class"),
        Node::Table { .. } => std::borrow::Cow::Borrowed("table"),
        Node::View { .. } => std::borrow::Cow::Borrowed("view"),
        Node::Package { .. } => std::borrow::Cow::Borrowed("pkg"),
        Node::Trigger { .. } => std::borrow::Cow::Borrowed("trigger"),
        Node::Type { .. } => std::borrow::Cow::Borrowed("type"),
        Node::Sequence { .. } => std::borrow::Cow::Borrowed("seq"),
        Node::Index { .. } => std::borrow::Cow::Borrowed("index"),
        Node::MaterializedView { .. } => std::borrow::Cow::Borrowed("mview"),
        Node::Synonym { .. } => std::borrow::Cow::Borrowed("synonym"),
        Node::Event { .. } => std::borrow::Cow::Borrowed("event"),
        Node::BuiltinFunction { category, .. } => match category.as_str() {
            "Operator" => std::borrow::Cow::Borrowed("builtin:op"),
            "Hint" => std::borrow::Cow::Borrowed("builtin:hint"),
            "Special" => std::borrow::Cow::Borrowed("builtin:special"),
            _ => std::borrow::Cow::Borrowed("builtin:func"),
        },
        Node::Custom { type_name, .. } => std::borrow::Cow::Owned((**type_name).clone()),
        #[cfg(feature = "jsp")]
        Node::JspPage { .. } => std::borrow::Cow::Borrowed("jsp"),
        #[cfg(feature = "jsp")]
        Node::JspSql { .. } => std::borrow::Cow::Borrowed("jspsql"),
        Node::Column { .. } => std::borrow::Cow::Borrowed("col"),
    }
}

#[allow(clippy::too_many_arguments)]
fn cmd_nodes(
    search: Option<&str>,
    orphan: bool,
    low_degree: Option<usize>,
    node_type: Option<&str>,
    has_partition: bool,
    has_distribute: bool,
    inferred: bool,
    system: bool,
    sort_by: Option<&[SortSpec]>,
    project: &Path,
    limit: Option<usize>,
) -> Result<()> {
    let mut proj = project::Project::find(project)?;
    let store = proj.load_store()?;
    let graph = store.graph();

    let max_degree = if orphan { Some(0) } else { low_degree };

    let type_filter = node_type.map(|t| t.to_lowercase());

    let indices: Vec<petgraph::graph::NodeIndex> = if let Some(query) = search {
        let matches = store.search_nodes_limit(query, limit);
        if matches.is_empty() {
            eprintln!("No nodes matching '{}'", query);
            return Ok(());
        }
        matches.into_iter().map(|(idx, _)| idx).collect()
    } else {
        graph.node_indices().collect()
    };

    let mut filtered: Vec<NodeRow> = indices
        .into_iter()
        .filter(|idx| {
            if let Some(ref tf) = type_filter {
                let tag = node_type_tag(&graph[*idx]).to_lowercase();
                if !tag.starts_with(tf.as_str()) && tag != *tf {
                    return false;
                }
            }
            true
        })
        .filter(|idx| {
            if has_partition || has_distribute {
                match &graph[*idx] {
                    Node::Table {
                        partition_by,
                        distribute_by,
                        ..
                    } => {
                        (!has_partition || partition_by.is_some())
                            && (!has_distribute || distribute_by.is_some())
                    }
                    _ => false,
                }
            } else {
                true
            }
        })
        .filter(|idx| {
            if inferred {
                is_inferred_node(&graph[*idx])
            } else {
                true
            }
        })
        .filter(|idx| {
            if system {
                is_system_node(&graph[*idx])
            } else {
                true
            }
        })
        .filter_map(|idx| {
            let in_deg = graph
                .neighbors_directed(idx, petgraph::Direction::Incoming)
                .count();
            let out_deg = graph
                .neighbors_directed(idx, petgraph::Direction::Outgoing)
                .count();
            let total = in_deg + out_deg;

            if let Some(max) = max_degree {
                if total > max {
                    return None;
                }
            }

            Some(NodeRow {
                in_deg,
                out_deg,
                total,
                tag: node_type_tag(&graph[idx]).into_owned(),
                name: graph::node_display_name(&graph[idx]),
            })
        })
        .collect();

    if let Some(specs) = sort_by {
        filtered.sort_by(|a, b| compare_rows(a, b, specs));
    }

    if let Some(max) = max_degree {
        let label = if orphan { "orphan" } else { "low-degree" };
        println_stdout!("{} (degree ≤ {}, {} shown)", label, max, filtered.len(),);
        println_stdout!();
    }

    println_stdout!(
        "{:<15} {:>5} {:>5} {:>5}  NAME",
        "TYPE",
        "IN",
        "OUT",
        "TOTAL"
    );
    for row in &filtered {
        println_stdout!(
            "{:<15} {:>5} {:>5} {:>5}  {}",
            row.tag,
            row.in_deg,
            row.out_deg,
            row.total,
            row.name
        );
    }

    if !filtered.is_empty() {
        println_stdout!();
        println_stdout!("{} nodes", filtered.len());
    }

    Ok(())
}

fn is_partial(node: &Node) -> bool {
    matches!(
        node,
        Node::Procedure { partial: true, .. } | Node::Function { partial: true, .. }
    )
}

fn is_inferred_node(node: &Node) -> bool {
    matches!(
        node,
        Node::Table {
            explicit: false,
            ..
        } | Node::View {
            explicit: false,
            ..
        }
    )
}

fn is_inferred(node: &Node) -> bool {
    is_inferred_node(node)
}

fn is_system_node(node: &Node) -> bool {
    matches!(
        node,
        Node::Table { system: true, .. } | Node::View { system: true, .. }
    )
}

fn filter_indexes_from_tree(
    nodes: &mut Vec<graph::traverse::TreeNode>,
    graph: &crate::graph::CodeGraph,
) {
    nodes.retain(|n| !matches!(&graph[n.idx], Node::Index { .. }));
    for node in nodes.iter_mut() {
        filter_indexes_from_tree(&mut node.children, graph);
    }
}

#[allow(clippy::too_many_arguments)]
fn cmd_detail(
    names: &[String],
    project: &Path,
    style: &str,
    depth: i64,
    show_files: bool,
    show_builtins: bool,
    verbose: bool,
    match_mode: crate::graph::search::MatchMode,
    all_matches: bool,
    fail_on_multiple: bool,
    summarize_tables: bool,
) -> Result<()> {
    let mut proj = project::Project::find(project)?;
    let store = proj.load_store()?;
    let graph = store.graph();

    for name in names {
        detail_one(
            name,
            graph,
            store,
            style,
            depth,
            show_files,
            show_builtins,
            verbose,
            match_mode,
            all_matches,
            fail_on_multiple,
            summarize_tables,
        );
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn detail_one(
    name: &str,
    graph: &crate::graph::CodeGraph,
    store: &crate::graph::store::GraphStore,
    style: &str,
    depth: i64,
    show_files: bool,
    show_builtins: bool,
    verbose: bool,
    match_mode: crate::graph::search::MatchMode,
    all_matches: bool,
    fail_on_multiple: bool,
    summarize_tables: bool,
) {
    let result = store.resolve_single_node(name, match_mode, all_matches, fail_on_multiple);

    let start_idx = match result {
        crate::graph::search::ResolveResult::Single(idx, _) => idx,
        crate::graph::search::ResolveResult::Multiple(matches) => {
            for (idx, _name) in &matches {
                print_node_detail(
                    *idx,
                    graph,
                    style,
                    depth,
                    show_files,
                    show_builtins,
                    verbose,
                );
            }
            return;
        }
        crate::graph::search::ResolveResult::Empty => {
            eprintln!("No nodes matching '{}'", name);
            return;
        }
        crate::graph::search::ResolveResult::Ambiguous => {
            eprintln!("Error: ambiguous match for '{}'", name);
            std::process::exit(2);
        }
    };

    if summarize_tables && matches!(graph[start_idx], crate::graph::Node::Package { .. }) {
        print_table_summary(graph, start_idx);
        return;
    }

    print_node_detail(
        start_idx,
        graph,
        style,
        depth,
        show_files,
        show_builtins,
        verbose,
    );
}

fn print_table_summary(graph: &crate::graph::CodeGraph, pkg_idx: petgraph::graph::NodeIndex) {
    use crate::graph::{AccessMode, DataFlowKind, Edge, WriteKind};
    use std::collections::{BTreeMap, HashSet};

    let display_name = crate::graph::node_display_name(&graph[pkg_idx]);

    // Collect child procedures via ContainsRoutine edges
    let mut table_access: BTreeMap<String, (AccessMode, HashSet<WriteKind>)> = BTreeMap::new();

    for edge_ref in graph.edges_directed(pkg_idx, petgraph::Direction::Outgoing) {
        if !matches!(edge_ref.weight(), Edge::ContainsRoutine) {
            continue;
        }
        let child_idx = edge_ref.target();

        // For each child, collect its outgoing TableAccess edges
        for ta_ref in graph.edges_directed(child_idx, petgraph::Direction::Outgoing) {
            if let Edge::TableAccess {
                flow_kind,
                modes,
                write_kinds,
                ..
            } = ta_ref.weight()
            {
                if *flow_kind != DataFlowKind::DmlAccess {
                    continue;
                }
                let dst = ta_ref.target();
                if let crate::graph::Node::Table { name, .. } = &graph[dst] {
                    let entry = table_access
                        .entry(name.clone())
                        .or_insert_with(|| (AccessMode::empty(), HashSet::new()));
                    entry.0 |= *modes;
                    for wk in write_kinds {
                        entry.1.insert(*wk);
                    }
                }
            }
        }
    }

    if table_access.is_empty() {
        println_stdout!("Table Access Summary for {}:  (none)", display_name);
        return;
    }

    println_stdout!("══ TABLE ACCESS ──");
    println_stdout!("  {}", display_name);

    let mut reads: Vec<&String> = Vec::new();
    let mut writes: Vec<(&String, Vec<&str>)> = Vec::new();
    let mut rw: Vec<&String> = Vec::new();

    for (tbl, (modes, wk)) in &table_access {
        let is_read = modes.contains(AccessMode::Read);
        let is_write = modes.contains(AccessMode::Write) || !wk.is_empty();

        if is_read && is_write {
            rw.push(tbl);
        } else if is_read {
            reads.push(tbl);
        } else if is_write {
            let wk_labels: Vec<&str> = wk
                .iter()
                .map(|w| match w {
                    WriteKind::Insert => "insert",
                    WriteKind::InsertSelect => "insert_select",
                    WriteKind::Update => "update",
                    WriteKind::Delete => "delete",
                    WriteKind::MergeInsert => "merge_insert",
                    WriteKind::MergeUpdate => "merge_update",
                    WriteKind::MergeDelete => "merge_delete",
                    WriteKind::SelectInto => "select_into",
                    WriteKind::Truncate => "truncate",
                })
                .collect();
            writes.push((tbl, wk_labels));
        }
    }

    if !reads.is_empty() {
        println_stdout!(
            "  READ ({}):  {}",
            reads.len(),
            reads
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if !writes.is_empty() {
        println_stdout!("  WRITE ({}):", writes.len());
        for (tbl, labels) in &writes {
            if labels.is_empty() {
                println_stdout!("    W:             {}", tbl);
            } else {
                println_stdout!("    {}:  {}", labels.join(","), tbl);
            }
        }
    }
    if !rw.is_empty() {
        println_stdout!(
            "  READ+WRITE ({}):  {}",
            rw.len(),
            rw.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        );
    }
    println_stdout!();
}

#[allow(clippy::too_many_arguments)]
fn print_node_detail(
    start_idx: petgraph::graph::NodeIndex,
    graph: &crate::graph::CodeGraph,
    style: &str,
    depth: i64,
    show_files: bool,
    show_builtins: bool,
    verbose: bool,
) {
    let tag = node_type_tag(&graph[start_idx]);
    let in_deg = graph
        .neighbors_directed(start_idx, petgraph::Direction::Incoming)
        .count();
    let out_deg = graph
        .neighbors_directed(start_idx, petgraph::Direction::Outgoing)
        .count();

    let display_name = graph::node_display_name(&graph[start_idx]);
    println_stdout!("══ SUMMARY ══");
    println_stdout!("  {}  {}", tag, display_name);
    if is_partial(&graph[start_idx]) {
        println_stdout!("  ⚠ partial node — body implementation could not be parsed");
    }
    if is_inferred(&graph[start_idx]) {
        println_stdout!("  ⚠ inferred node — no DDL definition found");
    }
    if is_system_node(&graph[start_idx]) {
        println_stdout!("  ⚙ system object — belongs to a known system schema");
    }
    println_stdout!(
        "  in:{}  out:{}  total:{}",
        in_deg,
        out_deg,
        in_deg + out_deg
    );
    if let Node::Table {
        location,
        temporary,
        unlogged,
        tablespace,
        ..
    } = &graph[start_idx]
    {
        if let Some(loc) = location {
            println_stdout!("  file   {}:{}", loc.file.to_string_lossy(), loc.line);
        } else {
            println_stdout!("  file   (implicit)");
        }
        if *temporary {
            println_stdout!("  temporary: true");
        }
        if *unlogged {
            println_stdout!("  unlogged: true");
        }
        if let Some(ts) = tablespace {
            println_stdout!("  tablespace: {}", ts);
        }
    }
    if let Node::View { location, .. } = &graph[start_idx] {
        if let Some(loc) = location {
            println_stdout!("  file   {}:{}", loc.file.to_string_lossy(), loc.line);
        } else {
            println_stdout!("  file   (implicit)");
        }
    }
    if let Node::MaterializedView { location, .. } = &graph[start_idx] {
        println_stdout!(
            "  file   {}:{}",
            location.file.to_string_lossy(),
            location.line
        );
    }
    println_stdout!();

    let target_is_builtin = matches!(graph[start_idx], graph::Node::BuiltinFunction { .. });
    let skip_builtins = !show_builtins && !target_is_builtin;

    let chain_max_depth = match depth {
        -1 => usize::MAX,
        n if n >= 0 => n as usize,
        _ => 1,
    };
    let (mut chain, _) =
        graph::traverse::trace_chain(graph, start_idx, chain_max_depth, usize::MAX, skip_builtins);
    filter_indexes_from_tree(&mut chain.callers, graph);
    let chain_style: graph::traverse::ChainStyle = style.parse().unwrap_or_default();
    println_stdout!(
        "{}",
        graph::traverse::format_chain(&chain, graph, chain_style)
    );

    print_node_details(&graph[start_idx], verbose);

    // For View / MaterializedView nodes, show referenced tables
    if matches!(
        &graph[start_idx],
        Node::View { .. } | Node::MaterializedView { .. }
    ) {
        let ref_output = format_referenced_tables(graph, start_idx);
        if !ref_output.is_empty() {
            println_stdout!();
            println_stdout!("{}", ref_output);
        }
    }

    let indexes_output = format_indexes(graph, start_idx);
    if !indexes_output.is_empty() {
        println_stdout!();
        println_stdout!("{}", indexes_output);
    }

    if show_files {
        let chain_files = graph::traverse::collect_chain_files(&chain, graph);
        println_stdout!();
        println_stdout!("── FILES ({}) ──", chain_files.len());
        if chain_files.is_empty() {
            println_stdout!("  (none)");
        } else {
            for (file, nodes) in &chain_files {
                println_stdout!("  {:>3}  {}", nodes.len(), file.to_string_lossy());
                for node_label in nodes.iter().take(8) {
                    println_stdout!("       {}", node_label);
                }
                if nodes.len() > 8 {
                    println_stdout!("       ... +{} more", nodes.len() - 8);
                }
            }
        }
    }
}

fn cmd_trace_sql(sql: Option<&str>, file: Option<&Path>, project: &Path) -> Result<()> {
    let fragment = match (sql, file) {
        (Some(s), None) => s.to_string(),
        (None, Some(f)) => {
            if f.to_str() == Some("-") {
                let mut buf = String::new();
                use std::io::Read;
                std::io::stdin().read_to_string(&mut buf).map_err(|e| {
                    error::CodeWebError::ExportError {
                        message: format!("read stdin: {}", e),
                    }
                })?;
                buf
            } else {
                std::fs::read_to_string(f).map_err(|e| error::CodeWebError::FileRead {
                    path: f.to_path_buf(),
                    source: e,
                })?
            }
        }
        (None, None) => {
            eprintln!(
                "error: provide a SQL fragment as argument or use --file to read from a file"
            );
            std::process::exit(1);
        }
        (Some(_), Some(_)) => {
            eprintln!("error: provide either a SQL fragment argument OR --file, not both");
            std::process::exit(1);
        }
    };
    let fragment = fragment.trim();

    let mut proj = project::Project::find(project)?;
    let store = proj.load_store()?;
    let graph = store.graph();

    let prepared = sql_match::PreparedQuery::new(fragment);
    let matches = store.search_by_sql(fragment);

    if matches.is_empty() {
        eprintln!("No matching SQL found for fragment: '{}'", fragment);
        return Ok(());
    }

    fn print_sql_block(lines: &[&str], kind_tag: &str, max_lines: usize) {
        let total = lines.len();
        let display = if total > max_lines {
            &lines[..max_lines]
        } else {
            lines
        };
        for (i, l) in display.iter().enumerate() {
            if i == display.len() - 1 {
                println_stdout!("    sql:   {} [{}]", l, kind_tag);
            } else {
                println_stdout!("    sql:   {}", l);
            }
        }
        if total > max_lines {
            println_stdout!("    sql:   ... +{} more lines", total - max_lines);
        }
    }

    fn kind_tag_from_sql(sql: &str) -> &str {
        let first = sql
            .trim_start()
            .split(|c: char| !c.is_ascii_alphabetic())
            .next()
            .unwrap_or("");
        match first.to_uppercase().as_str() {
            "SELECT" | "WITH" => "SELECT",
            "INSERT" => "INSERT",
            "UPDATE" => "UPDATE",
            "DELETE" => "DELETE",
            "MERGE" => "MERGE",
            _ => "",
        }
    }

    println_stdout!("SQL fragment: '{}'", fragment);
    println_stdout!("Found {} matching node(s)", matches.len());
    println_stdout!();

    for (idx, _, score) in &matches {
        let node = &graph[*idx];
        let score_pct = (*score * 100.0).round() as u8;
        match node {
            Node::MappedStatement {
                namespace,
                statement_id,
                kind,
                xml_file,
                line,
                sql: Some(sql_text),
                ..
            } => {
                println_stdout!(
                    "  MappedStatement: {}.{}  [{}%]",
                    namespace,
                    statement_id,
                    score_pct
                );
                println_stdout!("    file:  {}:{}", xml_file.to_string_lossy(), line);
                let lines: Vec<&str> = sql_text.lines().collect();
                let tag = kind.to_uppercase();
                print_sql_block(&lines, &tag, 5);

                let callers: Vec<petgraph::graph::NodeIndex> = graph
                    .neighbors_directed(*idx, petgraph::Direction::Incoming)
                    .collect();
                let java_methods: Vec<&Node> = callers
                    .iter()
                    .filter_map(|ci| match &graph[*ci] {
                        n @ Node::JavaMethod { .. } => Some(n),
                        _ => None,
                    })
                    .collect();

                if !java_methods.is_empty() {
                    println_stdout!("    invoked by:");
                    for caller in &java_methods {
                        if let Node::JavaMethod {
                            fqn, file, line, ..
                        } = caller
                        {
                            println_stdout!("      JavaMethod: {}", fqn);
                            println_stdout!(
                                "        file:     {}:{}",
                                file.to_string_lossy(),
                                line
                            );
                        }
                    }
                }
                println_stdout!();
            }
            Node::JavaSql {
                class_name,
                method_name,
                extraction_method,
                java_file,
                line,
                sql: Some(sql_text),
                ..
            } => {
                let ctx = match (class_name, method_name) {
                    (Some(c), Some(m)) => format!("{}.{}", c, m),
                    (Some(c), None) => c.clone(),
                    (None, Some(m)) => m.clone(),
                    (None, None) => "?".to_string(),
                };
                println_stdout!(
                    "  JavaSql: {} ({})  [{}%]",
                    ctx,
                    extraction_method,
                    score_pct
                );
                println_stdout!("    file:  {}:{}", java_file.to_string_lossy(), line);
                let lines: Vec<&str> = sql_text.lines().collect();
                let tag = kind_tag_from_sql(sql_text);
                print_sql_block(&lines, tag, 5);
                println_stdout!();
            }
            Node::Procedure {
                id,
                location,
                body_sql,
                ..
            } => {
                println_stdout!("  Procedure: {}  [{}%]", id, score_pct);
                println_stdout!(
                    "    file:  {}:{}",
                    location.file.to_string_lossy(),
                    location.line
                );
                let matching: Vec<_> = body_sql
                    .iter()
                    .filter(|s| prepared.matches(&s.sql_text))
                    .collect();
                for sql in matching.iter().take(5) {
                    let lines: Vec<&str> = sql.sql_text.lines().collect();
                    print_sql_block(&lines, &sql.kind, 3);
                }
                if matching.len() > 5 {
                    println_stdout!(
                        "    sql:   ... +{} more matching SQL statements",
                        matching.len() - 5
                    );
                }
                let unmatched = body_sql.len() - matching.len();
                if unmatched > 0 {
                    println_stdout!(
                        "    ({} other SQL statement(s) in body not shown)",
                        unmatched
                    );
                }
                let callers: Vec<petgraph::graph::NodeIndex> = graph
                    .neighbors_directed(*idx, petgraph::Direction::Incoming)
                    .collect();
                if !callers.is_empty() {
                    println_stdout!("    called by:");
                    for ci in &callers {
                        let key = crate::graph::key::NodeKey::from_node(&graph[*ci]);
                        println_stdout!("      {}", key);
                    }
                }
                println_stdout!();
            }
            Node::Function {
                id,
                location,
                body_sql,
                ..
            } => {
                println_stdout!("  Function: {}  [{}%]", id, score_pct);
                println_stdout!(
                    "    file:  {}:{}",
                    location.file.to_string_lossy(),
                    location.line
                );
                let matching: Vec<_> = body_sql
                    .iter()
                    .filter(|s| prepared.matches(&s.sql_text))
                    .collect();
                for sql in matching.iter().take(5) {
                    let lines: Vec<&str> = sql.sql_text.lines().collect();
                    print_sql_block(&lines, &sql.kind, 3);
                }
                if matching.len() > 5 {
                    println_stdout!(
                        "    sql:   ... +{} more matching SQL statements",
                        matching.len() - 5
                    );
                }
                let unmatched = body_sql.len() - matching.len();
                if unmatched > 0 {
                    println_stdout!(
                        "    ({} other SQL statement(s) in body not shown)",
                        unmatched
                    );
                }
                let callers: Vec<petgraph::graph::NodeIndex> = graph
                    .neighbors_directed(*idx, petgraph::Direction::Incoming)
                    .collect();
                if !callers.is_empty() {
                    println_stdout!("    called by:");
                    for ci in &callers {
                        let key = crate::graph::key::NodeKey::from_node(&graph[*ci]);
                        println_stdout!("      {}", key);
                    }
                }
                println_stdout!();
            }
            _ => {}
        }
    }

    Ok(())
}

fn cmd_query(file: Option<&Path>, spec_str: Option<&str>, project: &Path) -> Result<()> {
    let mut proj = project::Project::find(project)?;
    let store = proj.load_store()?;

    let json_str = match (file, spec_str) {
        (Some(path), _) => {
            if path.to_str() == Some("-") {
                use std::io::Read;
                let mut buf = String::new();
                std::io::stdin()
                    .read_to_string(&mut buf)
                    .map_err(|source| error::CodeWebError::ExportError {
                        message: source.to_string(),
                    })?;
                buf
            } else {
                std::fs::read_to_string(path).map_err(|source| error::CodeWebError::FileRead {
                    path: path.to_path_buf(),
                    source,
                })?
            }
        }
        (None, Some(s)) => s.to_string(),
        (None, None) => {
            eprintln!("Error: provide --file or --spec");
            std::process::exit(1);
        }
    };

    let query_spec: crate::graph::query::spec::QuerySpec = serde_json::from_str(&json_str)
        .map_err(|e| error::CodeWebError::ExportError {
            message: format!("Invalid query spec: {}", e),
        })?;

    let result = query_spec
        .execute(store)
        .map_err(|e| error::CodeWebError::ExportError { message: e })?;

    let output =
        serde_json::to_string_pretty(&result).map_err(|e| error::CodeWebError::ExportError {
            message: e.to_string(),
        })?;
    println_stdout!("{}", output);
    Ok(())
}

fn display_width(s: &str) -> usize {
    s.chars()
        .map(|c| {
            if c.is_ascii() {
                1
            } else if ('\u{1100}'..='\u{115F}').contains(&c)
                || ('\u{2E80}'..='\u{A4CF}').contains(&c)
                || ('\u{AC00}'..='\u{D7A3}').contains(&c)
                || ('\u{F900}'..='\u{FAFF}').contains(&c)
                || ('\u{FE10}'..='\u{FE19}').contains(&c)
                || ('\u{FE30}'..='\u{FE6F}').contains(&c)
                || ('\u{FF00}'..='\u{FF60}').contains(&c)
                || ('\u{FFE0}'..='\u{FFE6}').contains(&c)
                || ('\u{20000}'..='\u{2FFFD}').contains(&c)
                || ('\u{30000}'..='\u{3FFFD}').contains(&c)
            {
                2
            } else {
                1
            }
        })
        .sum()
}

fn pad_display(s: &str, width: usize) -> String {
    let dw = display_width(s);
    if dw >= width {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(width - dw))
    }
}

fn format_columns(columns: &[graph::ColumnSummary]) -> String {
    if columns.is_empty() {
        return String::new();
    }

    let max_name_width = columns
        .iter()
        .map(|c| display_width(&c.name))
        .max()
        .unwrap_or(0)
        .max(4);
    let max_type_width = columns
        .iter()
        .map(|c| display_width(&c.data_type))
        .max()
        .unwrap_or(0)
        .max(4);
    let has_default = columns.iter().any(|c| c.default_value.is_some());
    let max_default_width = columns
        .iter()
        .map(|c| c.default_value.as_deref().map(display_width).unwrap_or(0))
        .max()
        .unwrap_or(0)
        .max(if has_default { 7 } else { 0 });
    let has_comment = columns.iter().any(|c| c.comment.is_some());
    let max_comment_width = columns
        .iter()
        .map(|c| c.comment.as_deref().map(display_width).unwrap_or(0))
        .max()
        .unwrap_or(0)
        .max(if has_comment { 7 } else { 0 });

    let mut lines = Vec::new();
    lines.push(format!("── COLUMNS ({}) ──", columns.len()));

    {
        let default_hdr = if has_default {
            format!("  {}", pad_display("DEFAULT", max_default_width))
        } else {
            String::new()
        };
        let comment_hdr = if has_comment {
            format!("  {}", pad_display("COMMENT", max_comment_width))
        } else {
            String::new()
        };
        lines.push(format!(
            "{:>2}  {}  {}  {:5}{}{}",
            "#",
            pad_display("NAME", max_name_width),
            pad_display("TYPE", max_type_width),
            "NULL?",
            default_hdr,
            comment_hdr
        ));
    }

    {
        let sep_inner = 2 + max_name_width + 2 + max_type_width + 2 + 5;
        let sep_default = if has_default {
            2 + max_default_width
        } else {
            0
        };
        let sep_comment = if has_comment {
            2 + max_comment_width
        } else {
            0
        };
        let sep_width = 2 + sep_inner + sep_default + sep_comment;
        lines.push(format!("  {}", "-".repeat(sep_width.saturating_sub(2))));
    }

    for (i, col) in columns.iter().enumerate() {
        let null_str = if col.nullable { "NULL " } else { "NOT  " };
        let default_str = if has_default {
            let d = col.default_value.as_deref().unwrap_or("—");
            format!("  {}", pad_display(d, max_default_width))
        } else {
            String::new()
        };
        let comment_str = if has_comment {
            let c = col.comment.as_deref().unwrap_or("");
            format!("  {}", pad_display(c, max_comment_width))
        } else {
            String::new()
        };
        lines.push(format!(
            "{:>2}  {}  {}  {}{}{}",
            i + 1,
            pad_display(&col.name, max_name_width),
            pad_display(&col.data_type, max_type_width),
            null_str,
            default_str,
            comment_str,
        ));
    }

    let pk_cols: Vec<&str> = columns
        .iter()
        .filter(|c| c.is_primary_key)
        .map(|c| c.name.as_str())
        .collect();
    if !pk_cols.is_empty() {
        lines.push(String::new());
        lines.push(format!("  ◆  PK ({})", pk_cols.join(", ")));
    }

    lines.join("\n")
}

fn format_referenced_tables(graph: &crate::graph::CodeGraph, view_idx: NodeIndex) -> String {
    let mut ref_nodes: Vec<(NodeIndex, &Node)> = graph
        .neighbors_directed(view_idx, petgraph::Direction::Outgoing)
        .filter_map(|n| {
            if matches!(
                &graph[n],
                Node::Table { .. } | Node::View { .. } | Node::MaterializedView { .. }
            ) {
                Some((n, &graph[n]))
            } else {
                None
            }
        })
        .collect();

    if ref_nodes.is_empty() {
        return String::new();
    }

    ref_nodes.sort_by(|a, b| {
        let name_a = graph::node_display_name(a.1);
        let name_b = graph::node_display_name(b.1);
        name_a.cmp(&name_b)
    });

    let mut lines = Vec::new();
    lines.push(format!("── REFERENCED OBJECTS ({}) ──", ref_nodes.len()));

    for (_node_idx, node) in &ref_nodes {
        let display = graph::node_display_name(node);
        let tag = graph::node_type_tag(node);
        match node {
            Node::Table {
                location, columns, ..
            } => {
                if let Some(loc) = location {
                    lines.push(format!(
                        "  {}  {}  [file: {}:{}]",
                        tag,
                        display,
                        loc.file.to_string_lossy(),
                        loc.line
                    ));
                } else {
                    lines.push(format!("  {}  {}", tag, display));
                }
                for col in columns.iter() {
                    lines.push(format!("    {}", col.name));
                }
            }
            Node::View {
                location: Some(loc),
                ..
            } => {
                lines.push(format!(
                    "  {}  {}  [file: {}:{}]",
                    tag,
                    display,
                    loc.file.to_string_lossy(),
                    loc.line
                ));
            }
            Node::View { .. } => {
                lines.push(format!("  {}  {}", tag, display));
            }
            Node::MaterializedView { location, .. } => {
                lines.push(format!(
                    "  {}  {}  [file: {}:{}]",
                    tag,
                    display,
                    location.file.to_string_lossy(),
                    location.line
                ));
            }
            _ => {
                lines.push(format!("  {}  {}", tag, display));
            }
        }
    }

    lines.join("\n")
}

fn print_node_details(node: &Node, verbose: bool) {
    use graph::{DistributeInfo, PartitionInfo};
    match node {
        Node::Table {
            columns,
            partition_by,
            distribute_by,
            ddl_source,
            ..
        } => {
            if !columns.is_empty() {
                let cols_output = format_columns(columns);
                if !cols_output.is_empty() {
                    println_stdout!("{}", cols_output);
                }
            }
            let has_partition_info = partition_by.is_some() || distribute_by.is_some();
            if has_partition_info {
                println_stdout!();
                println_stdout!("── PARTITIONS ──");
            }
            if let Some(part) = partition_by {
                match part.as_ref() {
                    PartitionInfo::Range {
                        columns,
                        partitions,
                    } => {
                        println_stdout!(
                            "  partition: RANGE({}) [{} partitions]",
                            columns.join(", "),
                            partitions.len()
                        );
                    }
                    PartitionInfo::List {
                        columns,
                        partitions,
                    } => {
                        println_stdout!(
                            "  partition: LIST({}) [{} partitions]",
                            columns.join(", "),
                            partitions.len()
                        );
                    }
                    PartitionInfo::Hash {
                        columns,
                        partitions_count,
                    } => {
                        println_stdout!(
                            "  partition: HASH({}) [{}]",
                            columns.join(", "),
                            partitions_count
                                .map(|n| n.to_string())
                                .unwrap_or_else(|| "auto".to_string())
                        );
                    }
                }
            }
            if let Some(dist) = distribute_by {
                match dist.as_ref() {
                    DistributeInfo::Hash { columns } => {
                        println_stdout!("  distribute: HASH({})", columns.join(", "));
                    }
                    DistributeInfo::Replication => {
                        println_stdout!("  distribute: REPLICATION");
                    }
                    DistributeInfo::RoundRobin { columns } => {
                        println_stdout!("  distribute: ROUNDROBIN({})", columns.join(", "));
                    }
                    DistributeInfo::Modulo { columns } => {
                        println_stdout!("  distribute: MODULO({})", columns.join(", "));
                    }
                }
            }
            if let Some(ddl) = ddl_source {
                println_stdout!("  ddl: {}", ddl.as_ref());
            }
        }
        Node::View {
            columns,
            ddl_source,
            ..
        }
        | Node::MaterializedView {
            columns,
            ddl_source,
            ..
        } => {
            if !columns.is_empty() {
                let cols_output = format_columns(columns);
                if !cols_output.is_empty() {
                    println_stdout!("{}", cols_output);
                }
            }
            if verbose {
                if let Some(ddl) = ddl_source {
                    println_stdout!();
                    println_stdout!("── DEFINITION ──");
                    println_stdout!("{}", ddl);
                }
            }
        }
        Node::JavaSql {
            class_name,
            method_name,
            extraction_method,
            java_file,
            line,
            sql,
            ..
        } => {
            println_stdout!("  file: {}:{}", java_file.to_string_lossy(), line);
            if let (Some(c), Some(m)) = (class_name, method_name) {
                println_stdout!("  method: {}.{}", c, m);
            } else if let Some(c) = class_name {
                println_stdout!("  class: {}", c);
            } else if let Some(m) = method_name {
                println_stdout!("  method: {}", m);
            }
            println_stdout!("  extraction: {}", extraction_method);
            if let Some(sql_text) = sql {
                for line in sql_text.lines() {
                    println_stdout!("  sql: {}", line);
                }
            }
        }
        Node::MappedStatement {
            kind,
            xml_file,
            line,
            sql,
            ..
        } => {
            println_stdout!("  file: {}:{}", xml_file.to_string_lossy(), line);
            println_stdout!("  kind: {}", kind);
            if let Some(sql_text) = sql {
                for line in sql_text.lines() {
                    println_stdout!("  sql: {}", line);
                }
            }
        }
        _ => {}
    }
}

fn index_sort_key(graph: &crate::graph::CodeGraph, idx: NodeIndex) -> impl Ord {
    match &graph[idx] {
        Node::Index {
            constraint,
            unique,
            name,
            ..
        } => {
            let constraint_order = match constraint {
                Some(graph::IndexConstraint::PrimaryKey) => 0,
                Some(graph::IndexConstraint::Unique) => 1,
                None if *unique => 2,
                None => 3,
            };
            (constraint_order, name.clone().unwrap_or_default())
        }
        _ => (4, String::new()),
    }
}

fn format_indexes(graph: &crate::graph::CodeGraph, table_idx: NodeIndex) -> String {
    let mut indexes: Vec<NodeIndex> = graph
        .neighbors_directed(table_idx, petgraph::Direction::Incoming)
        .filter(|n| matches!(&graph[*n], Node::Index { .. }))
        .collect();

    if indexes.is_empty() {
        return String::new();
    }

    indexes.sort_by_key(|a| index_sort_key(graph, *a));

    let max_name_width = indexes
        .iter()
        .map(|idx| {
            if let Node::Index { name: Some(n), .. } = &graph[*idx] {
                n.len()
            } else {
                0
            }
        })
        .max()
        .unwrap_or(0)
        .max(30);

    let mut lines = Vec::new();
    lines.push(format!("── INDEXES ({}) ──", indexes.len()));
    lines.push(format!(
        "  {:<name$}  {:7}  {:7}  {:8}  COLUMNS",
        "NAME",
        "METHOD",
        "UNIQUE",
        "CONSTR",
        name = max_name_width,
    ));

    for idx in &indexes {
        if let Node::Index {
            name,
            unique,
            index_method,
            columns,
            constraint,
            ..
        } = &graph[*idx]
        {
            let name_str = name.as_deref().unwrap_or("(unnamed)");
            let method = index_method.as_deref().unwrap_or("btree");
            let (unique_str, constr_str) = match constraint {
                Some(graph::IndexConstraint::PrimaryKey) => {
                    ("★ PK".to_string(), "PRI KEY".to_string())
                }
                Some(graph::IndexConstraint::Unique) => {
                    ("★ UNIQ".to_string(), "UNIQUE".to_string())
                }
                None if *unique => ("★".to_string(), "—".to_string()),
                None => ("—".to_string(), "—".to_string()),
            };
            let cols_str = if columns.is_empty() {
                "—".to_string()
            } else {
                columns.join(", ")
            };
            lines.push(format!(
                "  {:<name$}  {:7}  {:7}  {:8}  {}",
                name_str,
                method,
                unique_str,
                constr_str,
                cols_str,
                name = max_name_width,
            ));
        }
    }

    lines.join("\n")
}

fn cmd_import(
    file: &Path,
    output: &Path,
    prefix: Option<&str>,
    name: Option<&str>,
    force: bool,
) -> Result<()> {
    let json_str =
        std::fs::read_to_string(file).map_err(|source| error::CodeWebError::FileRead {
            path: file.to_path_buf(),
            source,
        })?;

    let doc: import::format::CgefDocument =
        serde_json::from_str(&json_str).map_err(|e| error::CodeWebError::ExportError {
            message: format!("invalid CGEF JSON: {}", e),
        })?;

    let report = import::validator::validate(&doc);
    let mut all_errors: Vec<String> = report.errors.iter().map(|e| e.to_string()).collect();
    for w in &report.warnings {
        eprintln!("warning: {}", w.message);
    }

    let schema_registry = import::schema::SchemaRegistry::from_document(&doc);
    let path_mapper = import::path_mapper::PathMapper::new(prefix);
    let parser = import::parser::CgefParser::new(path_mapper, schema_registry);

    let parsed = parser.parse(doc);
    for e in &parsed.errors {
        all_errors.push(e.to_string());
    }

    if !all_errors.is_empty() {
        eprintln!("found {} error(s):", all_errors.len());
        for (i, e) in all_errors.iter().enumerate() {
            eprintln!("  {}. {}", i + 1, e);
        }
        if !force {
            return Err(error::CodeWebError::ExportError {
                message: format!(
                    "{} error(s) found; use --force to import anyway",
                    all_errors.len()
                ),
            });
        }
        eprintln!(
            "--force: importing anyway ({} nodes, {} edges skipped due to errors)",
            parsed
                .errors
                .iter()
                .filter(|e| matches!(
                    e,
                    import::parser::ParseError::MissingKeyField { .. }
                        | import::parser::ParseError::UnknownNodeType { .. }
                        | import::parser::ParseError::InvalidNode { .. }
                ))
                .count(),
            parsed
                .errors
                .iter()
                .filter(|e| matches!(
                    e,
                    import::parser::ParseError::SourceNotFound { .. }
                        | import::parser::ParseError::TargetNotFound { .. }
                        | import::parser::ParseError::UnknownEdgeType { .. }
                        | import::parser::ParseError::InvalidEdge { .. }
                ))
                .count(),
        );
    }

    let project_name = name.unwrap_or_else(|| {
        file.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("imported")
    });

    let store = graph::store::GraphStore::from_graph(project_name, parsed.graph);
    let stats = store.stats();

    if output.extension().is_some_and(|e| e == "json") {
        store.save_json(output)?;
    } else {
        store.save_bincode(output)?;
    }

    eprintln!(
        "Imported: {} nodes ({} custom), {} edges ({} custom) → {}",
        stats.procedures
            + stats.functions
            + stats.tables
            + stats.views
            + stats.mappers
            + stats.java_methods
            + stats.java_classes
            + stats.java_sql
            + stats.packages
            + stats.triggers
            + stats.types
            + stats.sequences
            + stats.indexes
            + stats.materialized_views
            + stats.synonyms
            + stats.events
            + stats.custom_nodes,
        stats.custom_nodes,
        stats.edges,
        stats.custom_edges,
        output.display()
    );

    Ok(())
}

fn cmd_legacy(cli: Cli) -> Result<()> {
    let input = cli.input.ok_or_else(|| error::CodeWebError::NoFilesFound {
        path: PathBuf::from("."),
    })?;

    let graph = if cli.sql_only {
        let files = parser::load_sql_files(&input)?;
        eprintln!("loaded {} SQL file(s)", files.len());
        let builder = graph::builder::GraphBuilder::new();
        builder.build(&files)
    } else {
        let all = parser::load_all_files(&input)?;
        eprintln!(
            "loaded {} SQL, {} Java, {} XML file(s){}",
            all.sql_files.len(),
            all.java_files.len(),
            all.ibatis_files.len(),
            jsp_count_fragment(
                #[cfg(feature = "jsp")]
                all.jsp_files.len(),
            ),
        );
        let builder = graph::builder::GraphBuilder::new();
        #[cfg(feature = "jsp")]
        {
            builder.build_all_with_jsp(&all, &all.jsp_files)
        }
        #[cfg(not(feature = "jsp"))]
        {
            builder.build_all(&all)
        }
    };

    print_stats(&graph, cli.include_unresolved);

    let output = match cli.format.as_str() {
        "ndjson" => {
            let mut buf = Vec::new();
            export::ndjson::to_ndjson(&graph, &mut buf)?;
            String::from_utf8_lossy(&buf).to_string()
        }
        "dot" => export::dot::to_dot(&graph),
        "json" => export::json::to_json(&graph)?,
        "mermaid" => export::mermaid::to_mermaid(&graph),
        _ => unreachable!(),
    };

    write_output(
        &output,
        cli.output.as_deref().map(|p| p as &std::path::Path),
    )
}

fn write_output(content: &str, output: Option<&std::path::Path>) -> Result<()> {
    match output {
        Some(path) => {
            std::fs::write(path, content).map_err(|source| error::CodeWebError::FileRead {
                path: path.to_path_buf(),
                source,
            })?;
        }
        None => {
            std::io::stdout()
                .write_all(content.as_bytes())
                .map_err(|source| error::CodeWebError::ExportError {
                    message: source.to_string(),
                })?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn cmd_partition(
    k: Option<usize>,
    gamma: Option<f64>,
    auto: bool,
    max_iterations: Option<usize>,
    min_delta_q: Option<f64>,
    min_component_size: usize,
    table_projection: Option<String>,
    project: &Path,
    output: Option<&Path>,
) -> Result<()> {
    let mut proj = project::Project::find(project)?;
    let store = proj.load_store()?;

    if auto || (k.is_none() && gamma.is_none()) {
        let mut base_config = graph::cluster::ClusterConfig::auto();
        base_config = base_config.with_min_component_size(min_component_size);
        if let Some(spec) = &table_projection {
            let parts: Vec<&str> = spec.split(':').collect();
            let tau: f64 = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0.1);
            let lambda: f64 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0.3);
            let k: usize = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(10);
            base_config = base_config.with_table_projection(tau, lambda, k);
        }
        if let Some(n) = max_iterations {
            base_config = base_config.with_max_iterations(n);
        }
        if let Some(q) = min_delta_q {
            base_config = base_config.with_min_delta_q(q);
        }

        let pb = indicatif::ProgressBar::new((graph::cluster::AUTO_GAMMA_SWEEP_LEN + 5) as u64);
        pb.set_style(
            indicatif::ProgressStyle::with_template(
                "  {bar:40.cyan/blue} {pos}/{len} {wide_msg:.dim}",
            )
            .unwrap()
            .progress_chars("━━╾─"),
        );
        let auto_report =
            graph::cluster::auto_partition_with_progress(store.graph(), &base_config, Some(&pb));
        pb.finish_with_message(format!(
            "auto-partition complete | recommended k={} γ={:.2}",
            auto_report.recommended_k, auto_report.recommended_gamma
        ));

        println_stdout!(
            "{}",
            t!(
                "partition.auto_title",
                total = auto_report.report.total_nodes
            )
        );
        println_stdout!();

        let gamma_count = graph::cluster::AUTO_GAMMA_SWEEP_LEN;
        println_stdout!("  {}", t!("partition.gamma_sweep"));
        println_stdout!(
            "  {:>10}  {:>8}  {:>12}  {:>14}",
            "Gamma",
            "Clusters",
            "Modularity Q",
            "Avg Cluster Size"
        );
        for e in auto_report.sweep.iter().take(gamma_count) {
            println_stdout!(
                "  {:>10.2}  {:>8}  {:>12.3}  {:>14.1}",
                e.gamma,
                e.k,
                e.modularity,
                e.avg_cluster_size
            );
        }
        println_stdout!();
        println_stdout!("  {}", t!("partition.k_sweep"));
        println_stdout!(
            "  {:>10}  {:>8}  {:>12}  {:>14}",
            t!("partition.forced_k"),
            t!("partition.actual"),
            "Modularity Q",
            "Avg Cluster Size"
        );
        for e in auto_report.sweep.iter().skip(gamma_count) {
            println_stdout!(
                "  {:>10}  {:>8}  {:>12.3}  {:>14.1}",
                e.k,
                e.k,
                e.modularity,
                e.avg_cluster_size
            );
        }
        println_stdout!();
        let rec_q = auto_report
            .sweep
            .iter()
            .find(|e| e.k == auto_report.recommended_k)
            .map(|e| e.modularity)
            .unwrap_or(0.0);
        println_stdout!(
            "{}",
            t!(
                "partition.recommended",
                k = auto_report.recommended_k,
                q = format!("{:.3}", rec_q)
            )
        );
        println_stdout!();
        print_wcc_topology(
            &auto_report.report,
            min_component_size,
            table_projection.as_deref(),
        );
        print_cluster_details(&auto_report.report);
        print_cluster_analysis(&auto_report.report);

        if let Some(path) = output {
            let cr = graph::cluster::ClusterResult::from(&auto_report.report);
            let dot = export::dot::to_dot_with_clusters(store.graph(), Some(&cr));
            write_output(&dot, Some(path))?;
            eprintln!("Clustered DOT exported to {}", path.display());
        }
        return Ok(());
    }

    let mut config = match k {
        Some(k) => graph::cluster::ClusterConfig::new(k),
        None => graph::cluster::ClusterConfig::auto(),
    };
    if let Some(g) = gamma {
        config = config.with_gamma(g);
    }
    if let Some(n) = max_iterations {
        config = config.with_max_iterations(n);
    }
    if let Some(q) = min_delta_q {
        config = config.with_min_delta_q(q);
    }
    config = config.with_min_component_size(min_component_size);
    if let Some(spec) = &table_projection {
        let parts: Vec<&str> = spec.split(':').collect();
        let tau: f64 = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0.1);
        let lambda: f64 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0.3);
        let k: usize = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(10);
        config = config.with_table_projection(tau, lambda, k);
    }

    let report = store.partition(&config);

    let k_desc = match report.k_requested {
        Some(k) => format!("requested {}", k),
        None => "auto-discovered".to_string(),
    };
    println_stdout!(
        "Partitioned {} nodes into {} clusters ({}, γ={:.2}, modularity Q = {:.3})",
        report.total_nodes,
        report.k_actual,
        k_desc,
        report.gamma,
        report.modularity
    );
    println_stdout!();
    print_wcc_topology(&report, min_component_size, table_projection.as_deref());
    print_cluster_details(&report);

    if !report.inter_cluster_coupling.is_empty() {
        println_stdout!();
        let total_coupling: f64 = report.inter_cluster_coupling.iter().map(|c| c.weight).sum();
        println_stdout!(
            "Inter-cluster coupling ({} entries, total weight {:.1}):",
            report.inter_cluster_coupling.len(),
            total_coupling
        );
        for c in report.inter_cluster_coupling.iter().take(10) {
            println_stdout!(
                "  Cluster {} → Cluster {}:  {:.1}  ({} edges)",
                c.from,
                c.to,
                c.weight,
                c.edge_count
            );
        }
    }

    if let Some(path) = output {
        let cluster_result = graph::cluster::ClusterResult::from(&report);
        let dot = export::dot::to_dot_with_clusters(store.graph(), Some(&cluster_result));
        write_output(&dot, Some(path))?;
        eprintln!("Clustered DOT exported to {}", path.display());
    }

    Ok(())
}

fn print_wcc_topology(
    report: &graph::cluster::PartitionReport,
    min_component_size: usize,
    table_projection_spec: Option<&str>,
) {
    let Some(topo) = report.topology.as_ref() else {
        return;
    };
    println_stdout!("{}", t!("partition.wcc_topology"));
    println_stdout!(
        "{}",
        t!("partition.wcc_total", total = topo.total_participants)
    );
    println_stdout!("{}", t!("partition.wcc_count", count = topo.wcc_count));

    let gcc_pct = if topo.total_participants > 0 {
        topo.gcc_size as f64 * 100.0 / topo.total_participants as f64
    } else {
        0.0
    };
    println_stdout!(
        "{}",
        t!(
            "partition.wcc_gcc",
            size = topo.gcc_size,
            pct = format!("{:.1}", gcc_pct)
        )
    );
    println_stdout!(
        "{}",
        t!(
            "partition.wcc_isolates",
            components = topo.isolates_count,
            nodes = topo.isolates_node_count
        )
    );

    // Report filter status: compare clustered node count to total participants
    let clustered_nodes: usize = report.cluster_stats.iter().map(|s| s.node_count).sum();
    if min_component_size > 1 && clustered_nodes < topo.total_participants {
        let excluded = topo.total_participants - clustered_nodes;
        println_stdout!(
            "{}",
            t!(
                "partition.wcc_filter_active",
                threshold = min_component_size,
                excluded = excluded
            )
        );
    }

    if let Some(spec) = table_projection_spec {
        let parts: Vec<&str> = spec.split(':').collect();
        let tau = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0.1);
        let lambda = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0.3);
        let k = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(10);
        println_stdout!(
            "{}",
            t!(
                "partition.projection_active",
                tau = tau,
                lambda = lambda,
                k = k
            )
        );
    } else {
        println_stdout!("{}", t!("partition.projection_off"));
    }
    println_stdout!();
}

fn print_cluster_details(report: &graph::cluster::PartitionReport) {
    println_stdout!(
        "  {:>7}  {:>6}  {:>10}  {:>10}  {}",
        t!("partition.cluster"),
        t!("partition.size"),
        t!("partition.internal"),
        t!("partition.external"),
        t!("partition.type_breakdown")
    );
    for stat in &report.cluster_stats {
        let breakdown: Vec<String> = {
            let mut entries: Vec<(graph::cluster::NodeKind, usize)> = stat
                .type_distribution
                .iter()
                .map(|(k, c)| (*k, *c))
                .collect();
            entries.sort_by_key(|b| std::cmp::Reverse(b.1));
            entries
                .iter()
                .map(|(kind, count)| format!("{}: {}", kind.tag(), count))
                .collect::<Vec<_>>()
        };
        println_stdout!(
            "  {:>7}  {:>6}  {:>10.1}  {:>10.1}  {}",
            stat.id,
            stat.node_count,
            stat.internal_weight,
            stat.external_weight,
            breakdown.join(", ")
        );
    }
}

fn print_cluster_analysis(report: &graph::cluster::PartitionReport) {
    println_stdout!();
    println_stdout!("{}", t!("partition.analysis_title"));
    println_stdout!();

    let total = report.total_nodes.max(1);
    let largest = report.cluster_stats.iter().max_by_key(|s| s.node_count);
    let disconnected: Vec<_> = report
        .cluster_stats
        .iter()
        .filter(|s| s.external_weight == 0.0)
        .collect();
    let connected: Vec<_> = report
        .cluster_stats
        .iter()
        .filter(|s| s.external_weight > 0.0)
        .collect();
    let total_coupling: f64 = report.inter_cluster_coupling.iter().map(|c| c.weight).sum();
    let total_internal: f64 = report.cluster_stats.iter().map(|s| s.internal_weight).sum();

    if let Some(lg) = largest {
        let pct = lg.node_count * 100 / total;
        if pct > 40 {
            println_stdout!(
                "{}",
                t!(
                    "partition.catchall",
                    id = lg.id,
                    pct = pct,
                    size = lg.node_count,
                    total = total
                )
            );
            println_stdout!("{}", t!("partition.catchall_2"));
        }
    }

    if disconnected.len() == report.cluster_stats.len() && !disconnected.is_empty() {
        let n = disconnected.len();
        println_stdout!("{}", t!("partition.all_disconnected", n = n));
        println_stdout!("{}", t!("partition.all_disconnected_2"));
        println_stdout!("{}", t!("partition.all_disconnected_3"));
    } else if !disconnected.is_empty() {
        println_stdout!(
            "{}",
            t!(
                "partition.some_disconnected",
                disc = disconnected.len(),
                total = report.cluster_stats.len(),
                conn = connected.len()
            )
        );
    }

    if total_coupling > 0.0 && total_internal + total_coupling > 0.0 {
        let ratio = total_coupling / (total_internal + total_coupling) * 100.0;
        println_stdout!(
            "{}",
            t!("partition.coupling_ratio", ratio = format!("{:.0}", ratio))
        );
        if ratio < 20.0 {
            println_stdout!("{}", t!("partition.coupling_low"));
        } else if ratio < 50.0 {
            println_stdout!("{}", t!("partition.coupling_mid"));
        } else {
            println_stdout!("{}", t!("partition.coupling_high"));
        }
    }

    let has_proc = report.cluster_stats.iter().any(|s| {
        s.type_distribution
            .contains_key(&graph::cluster::NodeKind::Procedure)
    });
    let has_method = report.cluster_stats.iter().any(|s| {
        s.type_distribution
            .contains_key(&graph::cluster::NodeKind::JavaMethod)
    });
    let mixed = report.cluster_stats.iter().any(|s| {
        s.type_distribution
            .contains_key(&graph::cluster::NodeKind::Procedure)
            && s.type_distribution
                .contains_key(&graph::cluster::NodeKind::JavaMethod)
    });
    if has_proc && has_method && !mixed {
        println_stdout!("{}", t!("partition.layer_split"));
        println_stdout!("{}", t!("partition.layer_split_2"));
    }

    let q = report.modularity;
    let q_str = format!("{:.3}", q);
    if q > 0.7 {
        println_stdout!("{}", t!("partition.q_strong", q = q_str));
    } else if q > 0.3 {
        println_stdout!("{}", t!("partition.q_moderate", q = q_str));
    } else {
        println_stdout!("{}", t!("partition.q_weak", q = q_str));
    }
}

fn print_analyze_report(report: &project::AnalyzeReport) {
    if report.is_up_to_date {
        eprintln!(
            "Up to date. {} files, {} nodes, {} edges.",
            report.files_scanned, report.nodes, report.edges
        );
        return;
    }
    let build_type = if report.is_full_build {
        "full"
    } else {
        "incremental"
    };
    eprintln!(
        "{} build: {} files ({} unchanged, {} changed, {} added, {} deleted) → {} nodes, {} edges ({:.1}s)",
        build_type,
        report.files_scanned,
        report.files_unchanged,
        report.files_changed,
        report.files_added,
        report.files_deleted,
        report.nodes,
        report.edges,
        report.elapsed_ms as f64 / 1000.0,
    );

    let (warnings, errors) = parse_log::summary();
    if warnings > 0 || errors > 0 {
        eprintln!(
            "  ⚠ {} warnings, {} errors — see .codeweb/parse.log",
            warnings, errors
        );
    }
}

fn print_stats(graph: &graph::CodeGraph, include_unresolved: bool) {
    let mut procedures = 0usize;
    let mut functions = 0usize;
    let mut unresolved = 0usize;
    let mut mappers = 0usize;
    let mut java_sql = 0usize;
    let mut java_methods = 0usize;
    let mut java_classes = 0usize;
    let mut tables = 0usize;
    let mut views = 0usize;
    let mut packages = 0usize;
    let mut triggers = 0usize;
    let mut types = 0usize;
    let mut sequences = 0usize;
    let mut indexes = 0usize;
    let mut materialized_views = 0usize;
    let mut synonyms = 0usize;
    let mut events = 0usize;
    let mut builtin_functions = 0usize;
    let mut partial = 0usize;
    let mut custom_nodes = 0usize;
    let mut columns = 0usize;
    #[cfg(feature = "jsp")]
    let mut jsp_pages = 0usize;
    #[cfg(feature = "jsp")]
    let mut jsp_sql = 0usize;

    for idx in graph.node_indices() {
        match &graph[idx] {
            Node::Procedure { partial: true, .. } => {
                procedures += 1;
                partial += 1;
            }
            Node::Procedure { .. } => procedures += 1,
            Node::Function { partial: true, .. } => {
                functions += 1;
                partial += 1;
            }
            Node::Function { .. } => functions += 1,
            Node::Unresolved { .. } => unresolved += 1,
            Node::MappedStatement { .. } => mappers += 1,
            Node::JavaSql { .. } => java_sql += 1,
            Node::JavaMethod { .. } => java_methods += 1,
            Node::JavaClass { .. } => java_classes += 1,
            Node::Table { .. } => tables += 1,
            Node::View { .. } => views += 1,
            Node::Package { .. } => packages += 1,
            Node::Trigger { .. } => triggers += 1,
            Node::Type { .. } => types += 1,
            Node::Sequence { .. } => sequences += 1,
            Node::Index { .. } => indexes += 1,
            Node::MaterializedView { .. } => materialized_views += 1,
            Node::Synonym { .. } => synonyms += 1,
            Node::Event { .. } => events += 1,
            Node::BuiltinFunction { .. } => builtin_functions += 1,
            Node::Custom { .. } => custom_nodes += 1,
            #[cfg(feature = "jsp")]
            Node::JspPage { .. } => jsp_pages += 1,
            #[cfg(feature = "jsp")]
            Node::JspSql { .. } => jsp_sql += 1,
            Node::Column { .. } => columns += 1,
        }
    }

    let edges = graph.edge_count();

    let jsp_fragment = build_jsp_fragment(
        #[cfg(feature = "jsp")]
        jsp_pages,
        #[cfg(feature = "jsp")]
        jsp_sql,
    );

    if include_unresolved {
        eprintln!(
            "graph: {} procedures, {} functions, {} packages, {} triggers, {} types, {} sequences, {} indexes, {} views, {} materialized views, {} synonyms, {} events, {} tables, {} mappers, {} java-sql, {} java-methods, {} java-classes, {} custom, {} columns, {} unresolved, {} builtin, {} edges{}",
            procedures, functions, packages, triggers, types, sequences, indexes, views, materialized_views, synonyms, events, tables, mappers, java_sql, java_methods, java_classes, custom_nodes, columns, unresolved, builtin_functions, edges,
            jsp_fragment
        );
    } else {
        eprintln!(
            "graph: {} procedures, {} functions, {} packages, {} triggers, {} types, {} sequences, {} indexes, {} views, {} materialized views, {} synonyms, {} events, {} tables, {} mappers, {} java-sql, {} java-methods, {} java-classes, {} custom, {} columns, {} builtin, {} edges{}",
            procedures, functions, packages, triggers, types, sequences, indexes, views, materialized_views, synonyms, events, tables, mappers, java_sql, java_methods, java_classes, custom_nodes, columns, builtin_functions, edges,
            jsp_fragment
        );
    }
    if partial > 0 {
        eprintln!("  ⚠ {} partial nodes (unparsed body)", partial);
    }
}

#[cfg(feature = "jsp")]
fn build_jsp_fragment(jsp_pages: usize, jsp_sql: usize) -> String {
    format!(", {} jsp-pages, {} jsp-sql", jsp_pages, jsp_sql)
}

#[cfg(not(feature = "jsp"))]
fn build_jsp_fragment() -> String {
    String::new()
}

#[cfg(feature = "jsp")]
fn jsp_count_fragment(jsp_count: usize) -> String {
    format!(", {} JSP", jsp_count)
}

#[cfg(not(feature = "jsp"))]
fn jsp_count_fragment() -> String {
    String::new()
}

fn build_edge_filter(types: &[String]) -> Result<crate::graph::query::filter::EdgeFilter> {
    use crate::graph::query::filter::EdgeFilter;

    if types.is_empty() || types.iter().any(|t| t.eq_ignore_ascii_case("all")) {
        return Ok(EdgeFilter::new());
    }

    let categories: Vec<crate::graph::EdgeCategory> = types
        .iter()
        .map(|t| {
            t.parse::<crate::graph::EdgeCategory>()
                .map_err(|e| crate::error::CodeWebError::ConfigError { message: e })
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(EdgeFilter::from_categories(&categories))
}

fn cmd_impact(
    files: &[PathBuf],
    node: Option<&str>,
    project: &Path,
    format: &str,
    depth: usize,
    edge_types: &[String],
    match_mode: crate::graph::search::MatchMode,
) -> Result<()> {
    use petgraph::Direction;

    let mut proj = project::Project::find(project)?;
    let store = match proj.load_store() {
        Ok(s) => s,
        Err(_) => {
            eprintln!("Project not analyzed. Run `codeweb analyze` first.");
            return Ok(());
        }
    };

    let graph = store.graph();
    let edge_filter = build_edge_filter(edge_types)?;
    let file_nodes = store.file_nodes();
    let key_index = store.node_key_index();

    let compute_file_impact = |file_path: &Path| -> Result<ImpactResult> {
        let (start_nodes, target) = resolve_file_target(graph, file_nodes, key_index, file_path)?;

        if start_nodes.is_empty() {
            return Ok(build_impact_result(&target, vec![], vec![]));
        }

        let mut upstream_map: HashMap<(Option<String>, String), ImpactEntry> = HashMap::new();
        let mut downstream_map: HashMap<(Option<String>, String), ImpactEntry> = HashMap::new();

        collect_impact_entries(
            graph,
            &start_nodes,
            Direction::Incoming,
            depth,
            &edge_filter,
            &mut upstream_map,
        );
        collect_impact_entries(
            graph,
            &start_nodes,
            Direction::Outgoing,
            depth,
            &edge_filter,
            &mut downstream_map,
        );

        let mut upstream: Vec<ImpactEntry> = upstream_map.into_values().collect();
        let mut downstream: Vec<ImpactEntry> = downstream_map.into_values().collect();
        upstream.sort_by(|a, b| (&a.file_path, &a.symbol).cmp(&(&b.file_path, &b.symbol)));
        downstream.sort_by(|a, b| (&a.file_path, &a.symbol).cmp(&(&b.file_path, &b.symbol)));

        Ok(build_impact_result(&target, upstream, downstream))
    };

    if !files.is_empty() {
        let mut results: Vec<ImpactResult> = Vec::with_capacity(files.len());
        for file in files {
            match compute_file_impact(file) {
                Ok(result) => results.push(result),
                Err(_) => eprintln!("Warning: file '{}' not found in graph", file.display()),
            }
        }
        if results.len() == 1 {
            emit_result(&results[0], format)?;
        } else {
            emit_batch_result(&results, format)?;
        }
    } else if let Some(name) = node {
        let (start_nodes, target) = resolve_node_target(store, name, match_mode)?;
        if start_nodes.is_empty() {
            emit_empty_result(&target, format)?;
            return Ok(());
        }

        let mut upstream_map: HashMap<(Option<String>, String), ImpactEntry> = HashMap::new();
        let mut downstream_map: HashMap<(Option<String>, String), ImpactEntry> = HashMap::new();

        collect_impact_entries(
            graph,
            &start_nodes,
            Direction::Incoming,
            depth,
            &edge_filter,
            &mut upstream_map,
        );
        collect_impact_entries(
            graph,
            &start_nodes,
            Direction::Outgoing,
            depth,
            &edge_filter,
            &mut downstream_map,
        );

        let mut upstream: Vec<ImpactEntry> = upstream_map.into_values().collect();
        let mut downstream: Vec<ImpactEntry> = downstream_map.into_values().collect();
        upstream.sort_by(|a, b| (&a.file_path, &a.symbol).cmp(&(&b.file_path, &b.symbol)));
        downstream.sort_by(|a, b| (&a.file_path, &a.symbol).cmp(&(&b.file_path, &b.symbol)));

        let result = build_impact_result(&target, upstream, downstream);
        emit_result(&result, format)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn cmd_inspect(
    node_names: &[String],
    project: &Path,
    max_depth: usize,
    max_paths: usize,
    style: &str,
    show_unreachable: bool,
    match_mode: crate::graph::search::MatchMode,
    all_matches: bool,
    fail_on_multiple: bool,
) -> Result<()> {
    use crate::graph::inspect::{
        find_paths_between, format_inspect_result, InspectOptions, InspectStyle,
    };

    let mut proj = project::Project::find(project)?;
    let store = match proj.load_store() {
        Ok(s) => s,
        Err(_) => {
            eprintln!("Project not analyzed. Run `codeweb analyze` first.");
            return Ok(());
        }
    };
    let graph = store.graph();

    let mut targets: Vec<NodeIndex> = Vec::with_capacity(node_names.len());
    let mut all_matched_names: Vec<String> = Vec::with_capacity(node_names.len());

    eprintln!("── NAME RESOLUTION ──");
    for name in node_names {
        let result = store.resolve_single_node(name, match_mode, all_matches, fail_on_multiple);
        match result {
            crate::graph::search::ResolveResult::Single(idx, display) => {
                eprintln!("  \"{}\" → 1 match  (exact)", name);
                targets.push(idx);
                all_matched_names.push(display);
            }
            crate::graph::search::ResolveResult::Multiple(matches) => {
                eprintln!("  \"{}\" → {} matches (using all)", name, matches.len());
                for (i, (idx, display)) in matches.iter().enumerate() {
                    eprintln!("    {}. {}", i + 1, display);
                    targets.push(*idx);
                    all_matched_names.push(display.clone());
                }
            }
            crate::graph::search::ResolveResult::Empty => {
                eprintln!("  \"{}\" → 0 matches", name);
                eprintln!("Error: '{}' did not match any node.", name);
                std::process::exit(2);
            }
            crate::graph::search::ResolveResult::Ambiguous => {
                eprintln!("  \"{}\" → ambiguous match", name);
                eprintln!("Error: ambiguous match for '{}'", name);
                std::process::exit(2);
            }
        }
    }
    eprintln!();

    let opts = InspectOptions {
        max_depth,
        max_paths_per_pair: max_paths,
        max_total_paths: max_paths.saturating_mul(10),
    };

    let style_enum = match style {
        "summary" => InspectStyle::Summary,
        "paths" => InspectStyle::Paths,
        "tree" => InspectStyle::Tree,
        _ => InspectStyle::Both,
    };

    let result = find_paths_between(graph, &targets, &opts);

    println_stdout!(
        "{}",
        format_inspect_result(
            &result,
            graph,
            &all_matched_names,
            style_enum,
            show_unreachable
        )
    );

    Ok(())
}

/// `cmd_impact` 的目标解析结果
enum ImpactTarget {
    File { path: String },
    Node { name: String },
}

/// 从文件入口解析出起始节点列表。
/// 文件不在图中或无节点时返回 Ok((vec![], ImpactTarget::File{...})),由调用方走空结果路径。
fn resolve_file_target(
    _graph: &crate::graph::CodeGraph,
    file_nodes: &HashMap<PathBuf, Vec<crate::graph::key::NodeKey>>,
    key_index: &HashMap<crate::graph::key::NodeKey, NodeIndex>,
    path: &Path,
) -> Result<(Vec<NodeIndex>, ImpactTarget)> {
    let target = ImpactTarget::File {
        path: path.to_string_lossy().to_string(),
    };

    let Some((matched_path, was_fuzzy)) = resolve_file_path(path, file_nodes) else {
        eprintln!(
            "Warning: file '{}' not found in graph (no nodes analyzed for this file)",
            path.display()
        );
        return Ok((vec![], target));
    };

    if was_fuzzy {
        eprintln!(
            "Warning: '{}' resolved via fuzzy match to '{}'",
            path.display(),
            matched_path.display()
        );
    }

    let nodes: Vec<NodeIndex> = file_nodes
        .get(matched_path)
        .map(|keys| {
            keys.iter()
                .filter_map(|k| key_index.get(k).copied())
                .collect()
        })
        .unwrap_or_default();

    // 注意:这里把 matched_path 的字符串回填到 target,以便 JSON 里的 file 字段显示规范化后的路径
    let target = ImpactTarget::File {
        path: matched_path.to_string_lossy().to_string(),
    };
    Ok((nodes, target))
}

/// 从节点名入口解析出起始节点列表(单个节点)。
/// 复用 store.search_nodes(),与 cmd_trace / cmd_detail 一致。
fn resolve_node_target(
    store: &crate::graph::store::GraphStore,
    name: &str,
    match_mode: crate::graph::search::MatchMode,
) -> Result<(Vec<NodeIndex>, ImpactTarget)> {
    let matches = store.search_nodes_with_mode(name, match_mode);

    if matches.is_empty() {
        eprintln!("No nodes matching '{}'", name);
        return Ok((
            vec![],
            ImpactTarget::Node {
                name: name.to_string(),
            },
        ));
    }

    if matches.len() > 1 {
        eprintln!("Multiple matches found for '{}':", name);
        for (i, (_, n)) in matches.iter().enumerate() {
            eprintln!("  {}: {}", i + 1, n);
        }
        eprintln!("Using first match: {}", matches[0].1);
    } else {
        eprintln!("Impact from: {}", matches[0].1);
    }

    let start_idx = matches[0].0;
    Ok((
        vec![start_idx],
        ImpactTarget::Node {
            name: matches[0].1.clone(),
        },
    ))
}

fn build_impact_result(
    target: &ImpactTarget,
    upstream: Vec<ImpactEntry>,
    downstream: Vec<ImpactEntry>,
) -> ImpactResult {
    let (file, node) = match target {
        ImpactTarget::File { path } => (Some(path.clone()), None),
        ImpactTarget::Node { name } => (None, Some(name.clone())),
    };
    ImpactResult {
        schema_version: 2,
        file,
        node,
        upstream,
        downstream,
    }
}

fn emit_result(result: &ImpactResult, format: &str) -> Result<()> {
    if format == "json" {
        let json =
            serde_json::to_string_pretty(result).map_err(|e| error::CodeWebError::ExportError {
                message: format!("JSON serialization: {}", e),
            })?;
        println_stdout!("{}", json);
    } else {
        print_impact_text(result);
    }
    Ok(())
}

fn emit_empty_result(target: &ImpactTarget, format: &str) -> Result<()> {
    let result = build_impact_result(target, vec![], vec![]);
    emit_result(&result, format)
}

fn emit_batch_result(results: &[ImpactResult], format: &str) -> Result<()> {
    if format == "json" {
        let json = serde_json::to_string_pretty(results).map_err(|e| {
            error::CodeWebError::ExportError {
                message: format!("JSON batch serialization: {}", e),
            }
        })?;
        println_stdout!("{}", json);
    } else {
        for (i, result) in results.iter().enumerate() {
            if i > 0 {
                println_stdout!("\n---\n");
            }
            print_impact_text(result);
        }
    }
    Ok(())
}

/// Match user-supplied path against `file_nodes` keys.
/// Tries canonicalize → absolute → ends_with fuzzy.
/// Returns (matched_path, was_fuzzy).
fn resolve_file_path<'a>(
    input: &Path,
    file_nodes: &'a HashMap<PathBuf, Vec<crate::graph::key::NodeKey>>,
) -> Option<(&'a PathBuf, bool)> {
    if let Ok(canon) = input.canonicalize() {
        if let Some((k, _)) = file_nodes.get_key_value(canon.as_path()) {
            return Some((k, false));
        }
    }

    if input.is_absolute() {
        if let Some((k, _)) = file_nodes.get_key_value(input) {
            return Some((k, false));
        }
    }

    let input_str = input.to_string_lossy();
    file_nodes
        .keys()
        .find(|key| key.to_string_lossy().ends_with(input_str.as_ref()))
        .map(|k| (k, true))
}

/// `Incoming` → upstream callers, `Outgoing` → downstream callees.
fn collect_impact_entries(
    graph: &crate::graph::CodeGraph,
    start_nodes: &[NodeIndex],
    direction: petgraph::Direction,
    depth: usize,
    edge_filter: &crate::graph::query::filter::EdgeFilter,
    out: &mut HashMap<(Option<String>, String), ImpactEntry>,
) {
    use crate::graph::key::NodeKey;

    if depth == 0 {
        return;
    }

    let mut visited: std::collections::HashSet<NodeIndex> = start_nodes.iter().copied().collect();
    let mut frontier: Vec<NodeIndex> = start_nodes.to_vec();

    for _ in 0..depth {
        let mut next_frontier: Vec<NodeIndex> = Vec::new();

        for &node in &frontier {
            let neighbors: Vec<_> = graph.neighbors_directed(node, direction).collect();

            for neighbor in neighbors {
                if visited.contains(&neighbor) {
                    continue;
                }

                let (src, dst) = match direction {
                    petgraph::Direction::Outgoing => (node, neighbor),
                    petgraph::Direction::Incoming => (neighbor, node),
                };

                let matching_weight = graph
                    .find_edge(src, dst)
                    .and_then(|eid| graph.edge_weight(eid))
                    .filter(|w| edge_filter.matches(w));

                let Some(weight) = matching_weight else {
                    continue;
                };

                visited.insert(neighbor);

                let neighbor_node = &graph[neighbor];
                let file_path = crate::graph::store::node_source_file(neighbor_node)
                    .map(|p| p.to_string_lossy().to_string());
                let symbol = NodeKey::from_node(neighbor_node).to_string();
                let line = impact_entry_line(neighbor_node, weight);

                out.entry((file_path.clone(), symbol.clone()))
                    .or_insert(ImpactEntry {
                        file_path,
                        symbol,
                        line,
                    });

                next_frontier.push(neighbor);
            }
        }

        frontier = next_frontier;
        if frontier.is_empty() {
            break;
        }
    }
}

/// Returns the source line for the ImpactEntry.
///
/// For nodes whose edge location comes from extracted SQL fragments
/// (JavaSql, MappedStatement, JspSql), the edge's `location.line` is
/// relative to the extracted SQL text, not the source file. Use the
/// node's own `line` field instead.
fn impact_entry_line(
    neighbor_node: &crate::graph::Node,
    edge: &crate::graph::Edge,
) -> Option<usize> {
    use crate::graph::Node;
    match neighbor_node {
        Node::JavaSql { line, .. } => Some(*line),
        Node::MappedStatement { line, .. } => Some(*line),
        #[cfg(feature = "jsp")]
        Node::JspSql { line, .. } => Some(*line),
        _ => edge_location_line(edge),
    }
}

fn edge_location_line(edge: &crate::graph::Edge) -> Option<usize> {
    use crate::graph::Edge;
    match edge {
        Edge::DirectCall { location, .. } => Some(location.line),
        Edge::DynamicCall { location, .. } => Some(location.line),
        Edge::CallsProcedure { location, .. } => Some(location.line),
        Edge::InvokesMapper { location, .. } => Some(location.line),
        Edge::CallsJava { location, .. } => Some(location.line),
        Edge::UsesBuiltinFunction { location, .. } => Some(location.line),
        Edge::Extends { location, .. } => Some(location.line),
        Edge::Implements { location, .. } => Some(location.line),
        Edge::TableAccess { location, .. } => Some(location.line),
        Edge::DependsOn { location, .. } => Some(location.line),
        Edge::TriggersRoutine { location, .. } => Some(location.line),
        Edge::ReferencesType { location, .. } => Some(location.line),
        Edge::UsesSequence { location, .. } => Some(location.line),
        Edge::IndexesTable { location, .. } => Some(location.line),
        Edge::AliasesObject { location, .. } => Some(location.line),
        Edge::CustomEdge { location, .. } => location.as_ref().map(|l| l.line),
        Edge::DataFlow { location, .. }
        | Edge::Derived { location, .. }
        | Edge::Aggregated { location, .. } => location.as_ref().map(|l| l.line),
        Edge::ContainsMethod | Edge::ContainsRoutine => None,
        #[cfg(feature = "jsp")]
        Edge::ContainsSql => None,
    }
}

fn print_impact_text(result: &ImpactResult) {
    // 头部:File 或 Node 二选一显示
    if let Some(file) = &result.file {
        println_stdout!("File: {}", file);
    } else if let Some(node) = &result.node {
        println_stdout!("Node: {}", node);
    }
    println_stdout!();
    println_stdout!("── UPSTREAM ({}) ──", result.upstream.len());
    if result.upstream.is_empty() {
        println_stdout!("  (none)");
    } else {
        print_grouped_entries(&result.upstream);
    }
    println_stdout!();
    println_stdout!("── DOWNSTREAM ({}) ──", result.downstream.len());
    if result.downstream.is_empty() {
        println_stdout!("  (none)");
    } else {
        print_grouped_entries(&result.downstream);
    }
}

/// Print entries grouped by file_path to reduce repetition.
/// Entries are assumed sorted by (file_path, symbol).
fn print_grouped_entries(entries: &[ImpactEntry]) {
    use std::collections::BTreeMap;

    let mut groups: BTreeMap<&str, Vec<&ImpactEntry>> = BTreeMap::new();
    for entry in entries {
        let key = entry.file_path.as_deref().unwrap_or("<unknown>");
        groups.entry(key).or_default().push(entry);
    }
    for (file, group) in &groups {
        println_stdout!("  {}", file);
        for entry in group {
            let line_tag = entry.line.map(|l| format!(":{}", l)).unwrap_or_default();
            println_stdout!("    {}{}", entry.symbol, line_tag);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn cmd_lineage(
    target: &str,
    direction: &str,
    depth: usize,
    format: &str,
    output: Option<&Path>,
    project: &Path,
    show_procedures: bool,
) -> Result<()> {
    let mut proj = project::Project::find(project)?;
    let store = match proj.load_store() {
        Ok(s) => s,
        Err(_) => {
            eprintln!("Project not analyzed. Run `codeweb analyze` first.");
            return Ok(());
        }
    };
    let graph = store.graph();

    if target.contains('.') {
        cmd_column_lineage(graph, target, direction, depth, format, output, show_procedures)
    } else {
        cmd_table_lineage(graph, target, direction, depth, format, output, show_procedures)
    }
}

#[allow(clippy::too_many_arguments)]
fn cmd_column_lineage(
    graph: &crate::graph::CodeGraph,
    col_target: &str,
    direction: &str,
    depth: usize,
    format: &str,
    output: Option<&Path>,
    show_procedures: bool,
) -> Result<()> {
    use crate::graph::traverse::ColumnLineageQuery;

    let parts: Vec<&str> = col_target.splitn(2, '.').collect();
    if parts.len() != 2 {
        eprintln!("Invalid column target format. Use 'table.column'");
        return Ok(());
    }
    let table = parts[0];
    let column = parts[1];

    // Try exact table.column match first, then fall back to name-only search.
    let exact_id = format!("col:{}.{}", table, column);
    let mut col_ids = vec![exact_id];
    // Also add name-only matches for robustness.
    let name_matches: Vec<String> = graph
        .node_indices()
        .filter(|&idx| matches!(&graph[idx], crate::graph::Node::Column { .. }))
        .filter_map(|idx| {
            if let crate::graph::Node::Column { id, name, .. } = &graph[idx] {
                if name == column && id != &col_ids[0] {
                    Some(id.clone())
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();
    col_ids.extend(name_matches);

    let mut all_paths = vec![];
    for col_id in &col_ids {
        let paths = graph.column_lineage(col_id, direction, depth);
        if !paths.is_empty() {
            all_paths = paths;
            break;
        }
    }

    if all_paths.is_empty() {
        eprintln!("No column lineage found for '{}'", col_target);
        eprintln!("Hint: column lineage requires 'codeweb analyze --column-lineage'.");
        return Ok(());
    }

    let output_str = match format {
        "tree" => format_col_lineage_tree(&all_paths, col_target, show_procedures),
        "table" => format_col_lineage_table(&all_paths),
        "json" => serde_json::to_string_pretty(&all_paths).unwrap_or_default(),
        _ => unreachable!(),
    };

    write_or_print(&output_str, output)
}

fn cmd_table_lineage(
    graph: &crate::graph::CodeGraph,
    table_target: &str,
    direction: &str,
    depth: usize,
    format: &str,
    output: Option<&Path>,
    _show_procedures: bool,
) -> Result<()> {
    use crate::graph::{node_display_name, Node};

    let table_node = graph.node_indices().find(|&idx| match &graph[idx] {
        Node::Table { name, .. } => name.eq_ignore_ascii_case(table_target),
        Node::View { name, .. } => name.eq_ignore_ascii_case(table_target),
        _ => false,
    });

    let Some(start_node) = table_node else {
        eprintln!("Table '{}' not found in graph", table_target);
        return Ok(());
    };

    let (upstream, downstream) = if direction == "upstream" || direction == "both" {
        let up = trace_table_upstream(graph, start_node, depth);
        let down = if direction == "both" {
            trace_table_downstream(graph, start_node, depth)
        } else {
            vec![]
        };
        (up, down)
    } else {
        (vec![], trace_table_downstream(graph, start_node, depth))
    };

    let output_str = match format {
        "tree" => format_tbl_lineage_tree(graph, table_target, &upstream, &downstream, direction),
        "table" => format_tbl_lineage_table(graph, table_target, &upstream, &downstream, direction),
        "json" => {
            let result = serde_json::json!({
                "target": table_target,
                "direction": direction,
                "upstream": upstream.iter().map(|t| format_table_ref(graph, t)).collect::<Vec<_>>(),
                "downstream": downstream.iter().map(|t| format_table_ref(graph, t)).collect::<Vec<_>>(),
            });
            serde_json::to_string_pretty(&result).unwrap_or_default()
        }
        _ => unreachable!(),
    };

    write_or_print(&output_str, output)
}

/// Table lineage reference: table accessed + how (R/W) + via which procedure.
struct TableRef {
    table_name: String,
    mode: String,
    via_proc: String,
}

fn trace_table_upstream(
    graph: &crate::graph::CodeGraph,
    start: NodeIndex,
    max_depth: usize,
) -> Vec<TableRef> {
    use crate::graph::node_display_name;
    let mut results = Vec::new();
    // Find procedures that WRITE to this table (incoming W edges to start)
    for edge in graph.edges_directed(start, petgraph::Direction::Incoming) {
        if let crate::graph::Edge::TableAccess { modes, .. } = &graph[edge.id()] {
            if modes.contains(crate::graph::AccessMode::Write) {
                let proc_idx = edge.source();
                let proc_name = node_display_name(&graph[proc_idx]);
                // Find tables this procedure READS from (outgoing R edges from proc)
                for e2 in graph.edges_directed(proc_idx, petgraph::Direction::Outgoing) {
                    if let crate::graph::Edge::TableAccess { modes: m2, .. } = &graph[e2.id()] {
                        if m2.contains(crate::graph::AccessMode::Read) {
                            let table_idx = e2.target();
                            if table_idx != start {
                                // Don't show self-reference
                                results.push(TableRef {
                                    table_name: node_display_name(&graph[table_idx]),
                                    mode: format_mode(m2),
                                    via_proc: proc_name.clone(),
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    results
}

fn trace_table_downstream(
    graph: &crate::graph::CodeGraph,
    start: NodeIndex,
    max_depth: usize,
) -> Vec<TableRef> {
    use crate::graph::node_display_name;
    let mut results = Vec::new();
    // Find procedures that READ from this table (incoming R edges to start)
    for edge in graph.edges_directed(start, petgraph::Direction::Incoming) {
        if let crate::graph::Edge::TableAccess { modes, .. } = &graph[edge.id()] {
            if modes.contains(crate::graph::AccessMode::Read) {
                let proc_idx = edge.source();
                let proc_name = node_display_name(&graph[proc_idx]);
                // Find tables this procedure WRITES to (outgoing W edges from proc)
                for e2 in graph.edges_directed(proc_idx, petgraph::Direction::Outgoing) {
                    if let crate::graph::Edge::TableAccess { modes: m2, .. } = &graph[e2.id()] {
                        if m2.contains(crate::graph::AccessMode::Write) {
                            let table_idx = e2.target();
                            if table_idx != start {
                                results.push(TableRef {
                                    table_name: node_display_name(&graph[table_idx]),
                                    mode: format_mode(m2),
                                    via_proc: proc_name.clone(),
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    results
}

fn format_mode(m: &crate::graph::AccessMode) -> String {
    let mut flags = Vec::new();
    if m.contains(crate::graph::AccessMode::Read) { flags.push("R"); }
    if m.contains(crate::graph::AccessMode::Write) { flags.push("W"); }
    flags.join("/")
}

fn format_table_ref(graph: &crate::graph::CodeGraph, t: &TableRef) -> String {
    format!("{} [{}:{}]", t.table_name, t.mode, t.via_proc)
}

fn format_col_lineage_tree(
    paths: &[Vec<crate::graph::traverse::ColumnLineageStep>],
    target: &str,
    show_procedures: bool,
) -> String {
    let mut out = String::new();
    out.push_str(target);
    out.push('\n');

    for path in paths {
        // Filter out procedure-owned intermediate column nodes unless --show-procedures.
        let filtered: Vec<&crate::graph::traverse::ColumnLineageStep> = if show_procedures {
            path.iter().collect()
        } else {
            path.iter()
                .filter(|s| !(is_proc_col(&s.source_col_id) && is_proc_col(&s.target_col_id)))
                .collect()
        };
        if filtered.is_empty() {
            continue;
        }
        for (i, step) in filtered.iter().enumerate() {
            let indent = "  ".repeat(i);
            let prefix = if i == filtered.len() - 1 {
                "  └── "
            } else {
                "  ├── "
            };

            let kind_label: String = match step.edge_kind.as_str() {
                "dataflow" => "DataFlow".to_string(),
                "derived" => match &step.expression {
                    Some(e) if !e.is_empty() => {
                        format!("Derived: {}", truncate_str(e, 50))
                    }
                    _ => "Derived".to_string(),
                },
                "aggregated" => match &step.aggregation {
                    Some(a) => format!("Aggregated: {}", a),
                    _ => "Aggregated".to_string(),
                },
                _ => step.edge_kind.clone(),
            };

            // Show clean column names (strip proc:/func: prefixes for readability)
            let display_id = clean_display_id(&step.source_col_id);
            let line = format!(
                "{}{}{} [{}]\n",
                indent, prefix, display_id, kind_label
            );
            out.push_str(&line);
        }
    }
    out
}

/// Check if a column ID belongs to a procedure (not a table).
fn is_proc_col(col_id: &str) -> bool {
    let id = col_id.strip_prefix("col:").unwrap_or(col_id);
    if let Some(dot) = id.rfind('.') {
        is_proc_owner(&id[..dot])
    } else {
        is_proc_owner(id)
    }
}

/// Clean column ID for display: strip col: prefix, keep table.column,
/// strip only procedure-owned prefixes to just column name.
fn clean_display_id(col_id: &str) -> String {
    let id = col_id.strip_prefix("col:").unwrap_or(col_id);
    if id.is_empty() {
        return id.to_string();
    }
    if let Some(dot) = id.rfind('.') {
        let owner = &id[..dot];
        let col = &id[dot + 1..];
        if is_proc_owner(owner) {
            // Procedure-owned: show just column name
            return col.to_string();
        }
        // Table-owned: show table.column
    }
    id.to_string()
}

fn is_proc_owner(owner: &str) -> bool {
    owner.starts_with("proc:")
        || owner.starts_with("func:")
        || owner.starts_with("prc_")
        || owner.starts_with("pkg_")
        || owner.starts_with("fnc_")
}

fn format_col_lineage_table(paths: &[Vec<crate::graph::traverse::ColumnLineageStep>]) -> String {
    let mut out = String::from(
        "SOURCE_COLUMN           | TRANSFORM       | TARGET_COLUMN           | DEPTH\n",
    );
    out.push_str("------------------------+-----------------+-------------------------+-------\n");
    for path in paths {
        for step in path {
            let expr = step.expression.as_deref().unwrap_or("-");
            out.push_str(&format!(
                "{:<24} | {:<15} | {:<24} | {}\n",
                truncate_str(&step.source_col_id, 24),
                truncate_str(expr, 15),
                truncate_str(&step.target_col_id, 24),
                step.depth,
            ));
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn format_tbl_lineage_tree(
    graph: &crate::graph::CodeGraph,
    target: &str,
    upstream: &[TableRef],
    downstream: &[TableRef],
    direction: &str,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("{} [table]\n", target));

    if (direction == "upstream" || direction == "both") && !upstream.is_empty() {
        out.push_str("  Upstream (source tables → this table):\n");
        for t in upstream {
            out.push_str(&format!(
                "    {} [{} via {}]\n",
                t.table_name, t.mode, t.via_proc
            ));
        }
    }
    if (direction == "downstream" || direction == "both") && !downstream.is_empty() {
        out.push_str("  Downstream (this table → target tables):\n");
        for t in downstream {
            out.push_str(&format!(
                "    {} [{} via {}]\n",
                t.table_name, t.mode, t.via_proc
            ));
        }
    }
    out
}

fn format_tbl_lineage_table(
    _graph: &crate::graph::CodeGraph,
    table_target: &str,
    upstream: &[TableRef],
    downstream: &[TableRef],
    direction: &str,
) -> String {
    let mut out = String::from("DIRECTION  | SOURCE_TABLE            | TARGET_TABLE            | MODE | VIA\n");
    out.push_str("-----------+-------------------------+-------------------------+------+----\n");
    if direction == "upstream" || direction == "both" {
        for t in upstream {
            out.push_str(&format!(
                "upstream   | {:<23} | {:<23} | {:<4} | {}\n",
                t.table_name, table_target, t.mode, t.via_proc,
            ));
        }
    }
    if direction == "downstream" || direction == "both" {
        for t in downstream {
            out.push_str(&format!(
                "downstream | {:<23} | {:<23} | {:<4} | {}\n",
                table_target, t.table_name, t.mode, t.via_proc,
            ));
        }
    }
    out
}

fn write_or_print(content: &str, output: Option<&Path>) -> Result<()> {
    if let Some(path) = output {
        std::fs::write(path, content).map_err(|e| crate::error::CodeWebError::FileRead {
            path: path.to_path_buf(),
            source: e,
        })?;
        eprintln!("Output written to {}", path.display());
    } else {
        println_stdout!("{}", content);
    }
    Ok(())
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    fn row(in_deg: usize, out_deg: usize, tag: &str, name: &str) -> NodeRow {
        NodeRow {
            in_deg,
            out_deg,
            total: in_deg + out_deg,
            tag: tag.to_string(),
            name: name.to_string(),
        }
    }

    #[test]
    fn parse_sort_spec_defaults_dir_to_asc() {
        assert_eq!(
            parse_sort_spec("name").unwrap(),
            SortSpec {
                key: SortKey::Name,
                dir: SortDir::Asc
            }
        );
    }

    #[test]
    fn parse_sort_spec_explicit_dir() {
        assert_eq!(
            parse_sort_spec("total:desc").unwrap(),
            SortSpec {
                key: SortKey::Total,
                dir: SortDir::Desc
            }
        );
    }

    #[test]
    fn parse_sort_spec_case_insensitive() {
        assert_eq!(
            parse_sort_spec("IN:ASC").unwrap(),
            SortSpec {
                key: SortKey::In,
                dir: SortDir::Asc
            }
        );
    }

    #[test]
    fn parse_sort_spec_rejects_unknown_key() {
        assert!(parse_sort_spec("foo").is_err());
        assert!(parse_sort_spec("foo:asc").is_err());
    }

    #[test]
    fn parse_sort_spec_rejects_unknown_dir() {
        assert!(parse_sort_spec("name:up").is_err());
    }

    #[test]
    fn parse_sort_spec_rejects_empty_key() {
        assert!(parse_sort_spec("").is_err());
        assert!(parse_sort_spec(":asc").is_err());
    }

    #[test]
    fn compare_rows_single_key_asc() {
        let a = row(5, 1, "proc", "alpha");
        let b = row(3, 9, "proc", "beta");
        let specs = [SortSpec {
            key: SortKey::In,
            dir: SortDir::Asc,
        }];
        assert_eq!(compare_rows(&a, &b, &specs), Ordering::Greater);
    }

    #[test]
    fn compare_rows_single_key_desc() {
        let a = row(5, 1, "proc", "alpha");
        let b = row(3, 9, "proc", "beta");
        let specs = [SortSpec {
            key: SortKey::In,
            dir: SortDir::Desc,
        }];
        assert_eq!(compare_rows(&a, &b, &specs), Ordering::Less);
    }

    #[test]
    fn compare_rows_multi_key_primary_wins() {
        let a = row(5, 1, "proc", "alpha");
        let b = row(3, 9, "proc", "beta");
        let specs = [
            SortSpec {
                key: SortKey::In,
                dir: SortDir::Asc,
            },
            SortSpec {
                key: SortKey::Out,
                dir: SortDir::Desc,
            },
        ];
        assert_eq!(
            compare_rows(&a, &b, &specs),
            Ordering::Greater,
            "primary in:asc (5 vs 3) decides; out never consulted"
        );
    }

    #[test]
    fn compare_rows_multi_key_tiebreaker_kicks_in() {
        let a = row(5, 1, "proc", "alpha");
        let b = row(5, 9, "proc", "beta");
        let specs = [
            SortSpec {
                key: SortKey::In,
                dir: SortDir::Asc,
            },
            SortSpec {
                key: SortKey::Out,
                dir: SortDir::Desc,
            },
        ];
        assert_eq!(
            compare_rows(&a, &b, &specs),
            Ordering::Greater,
            "in tied; out:desc puts b (out=9) before a (out=1), so a > b"
        );
        assert_eq!(
            compare_rows(&b, &a, &specs),
            Ordering::Less,
            "symmetry: b < a"
        );
    }

    #[test]
    fn compare_rows_all_equal_returns_equal() {
        let a = row(5, 9, "proc", "alpha");
        let b = row(5, 9, "proc", "alpha");
        let specs = [
            SortSpec {
                key: SortKey::In,
                dir: SortDir::Asc,
            },
            SortSpec {
                key: SortKey::Name,
                dir: SortDir::Desc,
            },
        ];
        assert_eq!(compare_rows(&a, &b, &specs), Ordering::Equal);
    }

    #[test]
    fn compare_rows_no_specs_returns_equal() {
        let a = row(5, 9, "proc", "alpha");
        let b = row(3, 1, "func", "beta");
        assert_eq!(compare_rows(&a, &b, &[]), Ordering::Equal);
    }
}

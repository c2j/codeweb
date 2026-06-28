#[macro_use]
extern crate rust_i18n;

i18n!("locales", fallback = "en");

mod error;
mod export;
mod graph;
#[allow(dead_code)]
mod import;
#[cfg(feature = "mcp")]
mod mcp;
mod parse_log;
#[allow(dead_code)]
mod parser;
#[allow(dead_code)]
mod project;
#[cfg(feature = "serve")]
mod server;
#[cfg(feature = "tui")]
mod tui;

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use petgraph::graph::NodeIndex;

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
    #[arg(long, default_value = "dot", value_parser = ["dot", "json", "mermaid"])]
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
        #[arg(short, long, default_value = "dot", value_parser = ["dot", "json", "mermaid"])]
        format: String,

        /// Output file (stdout if omitted)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Project directory (default: current directory)
        #[arg(short, long, default_value = ".")]
        project: PathBuf,
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

    /// Show callers/callees detail for a node
    Detail {
        /// Node name to search for (substring match)
        name: String,

        /// Project directory (default: current directory)
        #[arg(short, long, default_value = ".")]
        project: PathBuf,

        /// Display style for call chain output
        #[arg(short, long, default_value = "tree", value_parser = ["tree", "path"])]
        style: String,

        /// Show source files involved in the upstream/downstream chain
        #[arg(short, long)]
        files: bool,

        /// Show built-in function calls in the chain (default: hidden)
        #[arg(long = "builtfunc")]
        builtfunc: bool,
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
        /// File path to analyze (relative to CWD or absolute).
        /// Mutually exclusive with --node.
        #[arg(long)]
        file: Option<PathBuf>,

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
    },
}

fn main() {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let rayon_threads = std::cmp::max(4, cores - 2);

    let builder = rayon::ThreadPoolBuilder::new()
        .num_threads(rayon_threads)
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

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let show_banner =
        args.len() == 1 || args.contains(&"--help".to_string()) || args.contains(&"-h".to_string());

    if show_banner {
        print_banner();
    }

    let cli = Cli::parse();

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
        Some(Commands::Analyze { project }) => cmd_analyze(&project),
        Some(Commands::Diff { project }) => cmd_diff(&project),
        Some(Commands::Export {
            format,
            output,
            project,
        }) => cmd_export(&format, output.as_deref(), &project),
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
        }) => server::run(&project, &addr, open),
        #[cfg(feature = "mcp")]
        Some(Commands::Mcp { project }) => mcp::server::run(&project),
        Some(Commands::Trace {
            from,
            project,
            style,
            builtfunc,
        }) => cmd_trace(&from, &project, &style, builtfunc),
        Some(Commands::Stats { project }) => cmd_stats(&project),
        Some(Commands::Files { project }) => cmd_files(&project),
        Some(Commands::Nodes {
            search,
            orphan,
            low_degree,
            node_type,
            has_partition,
            has_distribute,
            project,
        }) => cmd_nodes(
            search.as_deref(),
            orphan,
            low_degree,
            node_type.as_deref(),
            has_partition,
            has_distribute,
            &project,
        ),
        Some(Commands::Detail {
            name,
            project,
            style,
            files,
            builtfunc,
        }) => cmd_detail(&name, &project, &style, files, builtfunc),
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
        }) => {
            // 互斥校验:恰好一个必须是 Some
            match (&file, &node) {
                (Some(_), Some(_)) => {
                    eprintln!("Error: --file and --node are mutually exclusive. Pass exactly one.");
                    std::process::exit(2);
                }
                (None, None) => {
                    eprintln!("Error: must pass exactly one of --file <path> or --node <name>.");
                    std::process::exit(2);
                }
                _ => cmd_impact(file.as_deref(), node.as_deref(), &project, &format, depth),
            }
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
    let report = proj.analyze()?;
    print_analyze_report(&report);
    Ok(())
}

fn cmd_analyze(project: &Path) -> Result<()> {
    let mut proj = project::Project::find(project)?;
    let report = proj.analyze()?;
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

fn cmd_export(format: &str, output: Option<&Path>, project: &Path) -> Result<()> {
    let mut proj = project::Project::find(project)?;
    let store = proj.load_store()?;

    let graph = store.graph();
    let result = match format {
        "dot" => export::dot::to_dot(graph),
        "json" => export::json::to_json(graph)?,
        "mermaid" => export::mermaid::to_mermaid(graph),
        _ => unreachable!(),
    };

    write_output(&result, output)
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

fn cmd_trace(from: &str, project: &Path, style: &str, show_builtins: bool) -> Result<()> {
    let mut proj = project::Project::find(project)?;
    let store = proj.load_store()?;

    let matches = store.search_nodes(from);
    let graph = store.graph();

    if matches.is_empty() {
        eprintln!("No nodes matching '{}'", from);
        return Ok(());
    }

    if matches.len() > 1 {
        eprintln!("Multiple matches found:");
        for (i, (_, name)) in matches.iter().enumerate() {
            eprintln!("  {}: {}", i + 1, name);
        }
        eprintln!("Using first match: {}", matches[0].1);
    }

    let (start_idx, start_name) = &matches[0];
    eprintln!("Tracing from: {}", start_name);

    let target_is_builtin = matches!(graph[*start_idx], graph::Node::BuiltinFunction { .. });
    let skip_builtins = !show_builtins && !target_is_builtin;

    let (chain, _) = graph::traverse::trace_chain(graph, *start_idx, 50, usize::MAX, skip_builtins);
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
        let report = proj.analyze()?;
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
        Node::BuiltinFunction { .. } => std::borrow::Cow::Borrowed("builtin"),
        Node::Custom { type_name, .. } => std::borrow::Cow::Owned((**type_name).clone()),
        #[cfg(feature = "jsp")]
        Node::JspPage { .. } => std::borrow::Cow::Borrowed("jsp"),
        #[cfg(feature = "jsp")]
        Node::JspSql { .. } => std::borrow::Cow::Borrowed("jsql"),
    }
}

fn cmd_nodes(
    search: Option<&str>,
    orphan: bool,
    low_degree: Option<usize>,
    node_type: Option<&str>,
    has_partition: bool,
    has_distribute: bool,
    project: &Path,
) -> Result<()> {
    let mut proj = project::Project::find(project)?;
    let store = proj.load_store()?;
    let graph = store.graph();

    let max_degree = if orphan { Some(0) } else { low_degree };

    let type_filter = node_type.map(|t| t.to_lowercase());

    let indices: Vec<petgraph::graph::NodeIndex> = if let Some(query) = search {
        let matches = store.search_nodes(query);
        if matches.is_empty() {
            eprintln!("No nodes matching '{}'", query);
            return Ok(());
        }
        matches.into_iter().map(|(idx, _)| idx).collect()
    } else {
        graph.node_indices().collect()
    };

    let filtered: Vec<_> = indices
        .into_iter()
        .filter(|idx| {
            if let Some(ref tf) = type_filter {
                let tag = node_type_tag(&graph[*idx]).to_lowercase();
                if tag != *tf {
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

            Some((idx, in_deg, out_deg, total))
        })
        .collect();

    if let Some(max) = max_degree {
        let label = if orphan { "orphan" } else { "low-degree" };
        println_stdout!("{} (degree ≤ {}, {} shown)", label, max, filtered.len(),);
        println_stdout!();
    }

    println_stdout!("{:<8} {:>3} {:>3} {:>3}  NAME", "TYPE", "IN", "OUT", "TOT");
    for (idx, in_deg, out_deg, total) in &filtered {
        let tag = node_type_tag(&graph[*idx]);
        let key = graph::key::NodeKey::from_node(&graph[*idx]);
        println_stdout!(
            "{:<8} {:>3} {:>3} {:>3}  {}",
            tag,
            in_deg,
            out_deg,
            total,
            key
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

fn cmd_detail(
    name: &str,
    project: &Path,
    style: &str,
    show_files: bool,
    show_builtins: bool,
) -> Result<()> {
    let mut proj = project::Project::find(project)?;
    let store = proj.load_store()?;
    let graph = store.graph();

    let matches = store.search_nodes(name);

    if matches.is_empty() {
        eprintln!("No nodes matching '{}'", name);
        return Ok(());
    }

    if matches.len() > 1 {
        eprintln!("Multiple matches found:");
        for (i, (_, n)) in matches.iter().enumerate() {
            eprintln!("  {}: {}", i + 1, n);
        }
        eprintln!("Using first match: {}", matches[0].1);
    }

    let (start_idx, start_name) = &matches[0];

    let tag = node_type_tag(&graph[*start_idx]);
    let in_deg = graph
        .neighbors_directed(*start_idx, petgraph::Direction::Incoming)
        .count();
    let out_deg = graph
        .neighbors_directed(*start_idx, petgraph::Direction::Outgoing)
        .count();

    println_stdout!("  {} {}", tag, start_name);
    if is_partial(&graph[*start_idx]) {
        println_stdout!("  ⚠ partial node — body implementation could not be parsed");
    }
    println_stdout!("  in:{} out:{} total:{}", in_deg, out_deg, in_deg + out_deg);
    print_node_details(&graph[*start_idx]);
    println_stdout!();

    let target_is_builtin = matches!(graph[*start_idx], graph::Node::BuiltinFunction { .. });
    let skip_builtins = !show_builtins && !target_is_builtin;

    let (chain, _) = graph::traverse::trace_chain(graph, *start_idx, 50, usize::MAX, skip_builtins);
    let chain_style: graph::traverse::ChainStyle = style.parse().unwrap_or_default();
    println_stdout!(
        "{}",
        graph::traverse::format_chain(&chain, graph, chain_style)
    );

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

    Ok(())
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

    let matches = store.search_by_sql(fragment);

    if matches.is_empty() {
        eprintln!("No matching SQL found for fragment: '{}'", fragment);
        return Ok(());
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
                println_stdout!("    kind:  {}", kind);
                println_stdout!("    file:  {}:{}", xml_file.to_string_lossy(), line);
                for l in sql_text.lines().take(5) {
                    println_stdout!("    sql:   {}", l);
                }
                let line_count = sql_text.lines().count();
                if line_count > 5 {
                    println_stdout!("    sql:   ... +{} more lines", line_count - 5);
                }

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
                for l in sql_text.lines().take(5) {
                    println_stdout!("    sql:   {}", l);
                }
                let line_count = sql_text.lines().count();
                if line_count > 5 {
                    println_stdout!("    sql:   ... +{} more lines", line_count - 5);
                }
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
                for sql in body_sql.iter().take(5) {
                    for l in sql.sql_text.lines().take(3) {
                        println_stdout!("    sql:   {} [{}]", l, sql.kind);
                    }
                }
                let total = body_sql.len();
                if total > 5 {
                    println_stdout!("    sql:   ... +{} more SQL statements", total - 5);
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
                for sql in body_sql.iter().take(5) {
                    for l in sql.sql_text.lines().take(3) {
                        println_stdout!("    sql:   {} [{}]", l, sql.kind);
                    }
                }
                let total = body_sql.len();
                if total > 5 {
                    println_stdout!("    sql:   ... +{} more SQL statements", total - 5);
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

fn print_node_details(node: &Node) {
    use graph::{DistributeInfo, PartitionInfo};
    match node {
        Node::Table {
            location,
            columns,
            partition_by,
            distribute_by,
            tablespace,
            temporary,
            unlogged,
            ddl_source,
            ..
        } => {
            if let Some(loc) = location {
                println_stdout!("  file: {}:{}", loc.file.to_string_lossy(), loc.line);
            } else {
                println_stdout!("  file: (implicit)");
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
            if !columns.is_empty() {
                println_stdout!("  columns ({}):", columns.len());
                for col in columns.iter() {
                    let pk = if col.is_primary_key { " [PK]" } else { "" };
                    let null = if col.nullable { "NULL" } else { "NOT NULL" };
                    let def = col
                        .default_value
                        .as_deref()
                        .map(|d| format!(" DEFAULT {}", d))
                        .unwrap_or_default();
                    println_stdout!("    {} {} {}{}{}", col.name, col.data_type, null, pk, def);
                }
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
            "graph: {} procedures, {} functions, {} packages, {} triggers, {} types, {} sequences, {} indexes, {} views, {} materialized views, {} synonyms, {} events, {} tables, {} mappers, {} java-sql, {} java-methods, {} java-classes, {} custom, {} unresolved, {} builtin, {} edges{}",
            procedures, functions, packages, triggers, types, sequences, indexes, views, materialized_views, synonyms, events, tables, mappers, java_sql, java_methods, java_classes, custom_nodes, unresolved, builtin_functions, edges,
            jsp_fragment
        );
    } else {
        eprintln!(
            "graph: {} procedures, {} functions, {} packages, {} triggers, {} types, {} sequences, {} indexes, {} views, {} materialized views, {} synonyms, {} events, {} tables, {} mappers, {} java-sql, {} java-methods, {} java-classes, {} custom, {} builtin, {} edges{}",
            procedures, functions, packages, triggers, types, sequences, indexes, views, materialized_views, synonyms, events, tables, mappers, java_sql, java_methods, java_classes, custom_nodes, builtin_functions, edges,
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

fn cmd_impact(
    file: Option<&Path>,
    node: Option<&str>,
    project: &Path,
    format: &str,
    depth: usize,
) -> Result<()> {
    use crate::graph::query::filter::EdgeFilter;
    use petgraph::Direction;

    let mut proj = project::Project::find(project)?;
    // Note: `load_store()` returns single-layer `Result<&GraphStore>` (not nested).
    // See project/mod.rs:420. Match without `?` to handle "not yet analyzed" gracefully.
    let store = match proj.load_store() {
        Ok(s) => s,
        Err(_) => {
            eprintln!("Project not analyzed. Run `codeweb analyze` first.");
            return Ok(());
        }
    };

    let graph = store.graph();
    let calls_filter = EdgeFilter::calls_only();

    // ── 解析起点节点 ───────────────────────────────────────────────
    // 返回 (start_nodes, ImpactTarget)
    let (start_nodes, target) = match (file, node) {
        (Some(path), None) => {
            let file_nodes = store.file_nodes();
            let key_index = store.node_key_index();
            resolve_file_target(graph, file_nodes, key_index, path)?
        }
        (None, Some(name)) => {
            resolve_node_target(store, name)?
        }
        // 不可达:clap 分发层已校验互斥
        _ => unreachable!("clap layer guarantees exactly one of --file/--node is set"),
    };

    if start_nodes.is_empty() {
        emit_empty_result(&target, format)?;
        return Ok(());
    }

    // ── 双向遍历 ──────────────────────────────────────────────────
    let mut upstream_map: HashMap<(Option<String>, String), ImpactEntry> = HashMap::new();
    let mut downstream_map: HashMap<(Option<String>, String), ImpactEntry> = HashMap::new();

    collect_impact_entries(
        graph,
        &start_nodes,
        Direction::Incoming,
        depth,
        &calls_filter,
        &mut upstream_map,
    );
    collect_impact_entries(
        graph,
        &start_nodes,
        Direction::Outgoing,
        depth,
        &calls_filter,
        &mut downstream_map,
    );

    let mut upstream: Vec<ImpactEntry> = upstream_map.into_values().collect();
    let mut downstream: Vec<ImpactEntry> = downstream_map.into_values().collect();
    upstream.sort_by(|a, b| (&a.file_path, &a.symbol).cmp(&(&b.file_path, &b.symbol)));
    downstream.sort_by(|a, b| (&a.file_path, &a.symbol).cmp(&(&b.file_path, &b.symbol)));

    let result = build_impact_result(&target, upstream, downstream);
    emit_result(&result, format)?;
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
) -> Result<(Vec<NodeIndex>, ImpactTarget)> {
    let matches = store.search_nodes(name);

    if matches.is_empty() {
        eprintln!("No nodes matching '{}'", name);
        return Ok((vec![], ImpactTarget::Node { name: name.to_string() }));
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
    Ok((vec![start_idx], ImpactTarget::Node { name: matches[0].1.clone() }))
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
        let json = serde_json::to_string_pretty(result).map_err(|e| {
            error::CodeWebError::ExportError {
                message: format!("JSON serialization: {}", e),
            }
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
                let line = edge_location_line(weight);

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
        for entry in &result.upstream {
            let line_tag = entry.line.map(|l| format!(":{}", l)).unwrap_or_default();
            let file = entry.file_path.as_deref().unwrap_or("<unknown>");
            println_stdout!("  {}  {}{}", entry.symbol, file, line_tag);
        }
    }
    println_stdout!();
    println_stdout!("── DOWNSTREAM ({}) ──", result.downstream.len());
    if result.downstream.is_empty() {
        println_stdout!("  (none)");
    } else {
        for entry in &result.downstream {
            let line_tag = entry.line.map(|l| format!(":{}", l)).unwrap_or_default();
            let file = entry.file_path.as_deref().unwrap_or("<unknown>");
            println_stdout!("  {}  {}{}", entry.symbol, file, line_tag);
        }
    }
}

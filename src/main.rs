#[macro_use]
extern crate rust_i18n;

i18n!("locales", fallback = "en");

mod error;
mod export;
mod graph;
#[allow(dead_code)]
mod import;
mod parse_log;
#[allow(dead_code)]
mod parser;
#[allow(dead_code)]
mod project;
#[cfg(feature = "serve")]
mod server;
#[cfg(feature = "tui")]
mod tui;

use std::io::Write;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use error::Result;
use graph::Node;

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

    if let Err(e) = run() {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
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
        Some(Commands::Trace {
            from,
            project,
            style,
        }) => cmd_trace(&from, &project, &style),
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
        }) => cmd_detail(&name, &project, &style, files),
        Some(Commands::Import {
            file,
            output,
            prefix,
            name,
            force,
        }) => cmd_import(&file, &output, prefix.as_deref(), name.as_deref(), force),
        Some(Commands::Query {
            file,
            spec,
            project,
        }) => cmd_query(file.as_deref(), spec.as_deref(), &project),
        None => cmd_legacy(cli),
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

#[cfg(feature = "tui")]
fn cmd_tui(project: &Path) -> Result<()> {
    tui::run(project)
}

fn cmd_trace(from: &str, project: &Path, style: &str) -> Result<()> {
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

    let (chain, _) = graph::traverse::trace_chain(graph, *start_idx, 50, usize::MAX);
    let chain_style: graph::traverse::ChainStyle = style.parse().unwrap_or_default();
    println!(
        "{}",
        graph::traverse::format_chain(&chain, graph, chain_style)
    );
    Ok(())
}

fn cmd_stats(project: &Path) -> Result<()> {
    let mut proj = project::Project::find(project)?;
    let store = proj.load_store()?;
    let stats = store.stats();

    println!("Project: {}", proj.name());
    println!();
    println!("  {:>12}  procedures", stats.procedures,);
    println!("  {:>12}  functions", stats.functions,);
    println!("  {:>12}  packages", stats.packages,);
    println!("  {:>12}  triggers", stats.triggers,);
    println!("  {:>12}  types", stats.types,);
    println!("  {:>12}  sequences", stats.sequences,);
    println!("  {:>12}  indexes", stats.indexes,);
    println!("  {:>12}  views", stats.views,);
    println!("  {:>12}  materialized views", stats.materialized_views,);
    println!("  {:>12}  synonyms", stats.synonyms,);
    println!("  {:>12}  events", stats.events,);
    println!("  {:>12}  tables", stats.tables,);
    println!("  {:>12}  mappers", stats.mappers,);
    println!("  {:>12}  java methods", stats.java_methods,);
    println!("  {:>12}  java classes", stats.java_classes,);
    if stats.unresolved > 0 {
        println!("  {:>12}  unresolved", stats.unresolved,);
    }
    if stats.custom_nodes > 0 {
        println!("  {:>12}  custom nodes", stats.custom_nodes,);
    }
    println!();
    println!("  {:>12}  edges", stats.edges,);
    println!("  {:>12}  files", stats.files,);

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

        println!("{:<4} {:>5}  PATH", "TYPE", "NODES");
        for (path, record) in &entries {
            let rel = path.strip_prefix(&root).unwrap_or(path);
            let type_tag = match record.file_type {
                parser::fingerprint::FileType::Sql => "SQL",
                parser::fingerprint::FileType::Java => "Java",
                parser::fingerprint::FileType::Xml => "XML",
            };
            let node_count = file_nodes
                .get(path as &std::path::Path)
                .map(|v| v.len())
                .unwrap_or(0);
            println!(
                "{:<4} {:>5}  {}",
                type_tag,
                node_count,
                rel.to_string_lossy()
            );
        }
        println!();
        println!("{} files total", entries.len());
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
        Node::Custom { type_name, .. } => std::borrow::Cow::Owned((**type_name).clone()),
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
        println!("{} (degree ≤ {}, {} shown)", label, max, filtered.len(),);
        println!();
    }

    println!("{:<8} {:>3} {:>3} {:>3}  NAME", "TYPE", "IN", "OUT", "TOT");
    for (idx, in_deg, out_deg, total) in &filtered {
        let tag = node_type_tag(&graph[*idx]);
        let key = graph::key::NodeKey::from_node(&graph[*idx]);
        println!(
            "{:<8} {:>3} {:>3} {:>3}  {}",
            tag, in_deg, out_deg, total, key
        );
    }

    if !filtered.is_empty() {
        println!();
        println!("{} nodes", filtered.len());
    }

    Ok(())
}

fn is_partial(node: &Node) -> bool {
    matches!(
        node,
        Node::Procedure { partial: true, .. } | Node::Function { partial: true, .. }
    )
}

fn cmd_detail(name: &str, project: &Path, style: &str, show_files: bool) -> Result<()> {
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

    println!("  {} {}", tag, start_name);
    if is_partial(&graph[*start_idx]) {
        println!("  ⚠ partial node — body implementation could not be parsed");
    }
    println!("  in:{} out:{} total:{}", in_deg, out_deg, in_deg + out_deg);
    print_node_details(&graph[*start_idx]);
    println!();

    let (chain, _) = graph::traverse::trace_chain(graph, *start_idx, 50, usize::MAX);
    let chain_style: graph::traverse::ChainStyle = style.parse().unwrap_or_default();
    println!(
        "{}",
        graph::traverse::format_chain(&chain, graph, chain_style)
    );

    if show_files {
        let chain_files = graph::traverse::collect_chain_files(&chain, graph);
        println!();
        println!("── FILES ({}) ──", chain_files.len());
        if chain_files.is_empty() {
            println!("  (none)");
        } else {
            for (file, nodes) in &chain_files {
                println!("  {:>3}  {}", nodes.len(), file.to_string_lossy());
                for node_label in nodes.iter().take(8) {
                    println!("       {}", node_label);
                }
                if nodes.len() > 8 {
                    println!("       ... +{} more", nodes.len() - 8);
                }
            }
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
    println!("{}", output);
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
                println!("  file: {}:{}", loc.file.to_string_lossy(), loc.line);
            } else {
                println!("  file: (implicit)");
            }
            if *temporary {
                println!("  temporary: true");
            }
            if *unlogged {
                println!("  unlogged: true");
            }
            if let Some(ts) = tablespace {
                println!("  tablespace: {}", ts);
            }
            if !columns.is_empty() {
                println!("  columns ({}):", columns.len());
                for col in columns.iter() {
                    let pk = if col.is_primary_key { " [PK]" } else { "" };
                    let null = if col.nullable { "NULL" } else { "NOT NULL" };
                    let def = col
                        .default_value
                        .as_deref()
                        .map(|d| format!(" DEFAULT {}", d))
                        .unwrap_or_default();
                    println!("    {} {} {}{}{}", col.name, col.data_type, null, pk, def);
                }
            }
            if let Some(part) = partition_by {
                match part.as_ref() {
                    PartitionInfo::Range {
                        columns,
                        partitions,
                    } => {
                        println!(
                            "  partition: RANGE({}) [{} partitions]",
                            columns.join(", "),
                            partitions.len()
                        );
                    }
                    PartitionInfo::List {
                        columns,
                        partitions,
                    } => {
                        println!(
                            "  partition: LIST({}) [{} partitions]",
                            columns.join(", "),
                            partitions.len()
                        );
                    }
                    PartitionInfo::Hash {
                        columns,
                        partitions_count,
                    } => {
                        println!(
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
                        println!("  distribute: HASH({})", columns.join(", "));
                    }
                    DistributeInfo::Replication => {
                        println!("  distribute: REPLICATION");
                    }
                    DistributeInfo::RoundRobin { columns } => {
                        println!("  distribute: ROUNDROBIN({})", columns.join(", "));
                    }
                    DistributeInfo::Modulo { columns } => {
                        println!("  distribute: MODULO({})", columns.join(", "));
                    }
                }
            }
            if let Some(ddl) = ddl_source {
                println!("  ddl: {}", ddl.as_ref());
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
            println!("  file: {}:{}", java_file.to_string_lossy(), line);
            if let (Some(c), Some(m)) = (class_name, method_name) {
                println!("  method: {}.{}", c, m);
            } else if let Some(c) = class_name {
                println!("  class: {}", c);
            } else if let Some(m) = method_name {
                println!("  method: {}", m);
            }
            println!("  extraction: {}", extraction_method);
            if let Some(sql_text) = sql {
                for line in sql_text.lines() {
                    println!("  sql: {}", line);
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
            println!("  file: {}:{}", xml_file.to_string_lossy(), line);
            println!("  kind: {}", kind);
            if let Some(sql_text) = sql {
                for line in sql_text.lines() {
                    println!("  sql: {}", line);
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
            "loaded {} SQL, {} Java, {} XML file(s)",
            all.sql_files.len(),
            all.java_files.len(),
            all.ibatis_files.len()
        );
        let builder = graph::builder::GraphBuilder::new();
        builder.build_all(&all)
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
    let mut partial = 0usize;
    let mut custom_nodes = 0usize;

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
            Node::Custom { .. } => custom_nodes += 1,
        }
    }

    let edges = graph.edge_count();

    if include_unresolved {
        eprintln!(
            "graph: {} procedures, {} functions, {} packages, {} triggers, {} types, {} sequences, {} indexes, {} views, {} materialized views, {} synonyms, {} events, {} tables, {} mappers, {} java-sql, {} java-methods, {} java-classes, {} custom, {} unresolved, {} edges",
            procedures, functions, packages, triggers, types, sequences, indexes, views, materialized_views, synonyms, events, tables, mappers, java_sql, java_methods, java_classes, custom_nodes, unresolved, edges
        );
    } else {
        eprintln!(
            "graph: {} procedures, {} functions, {} packages, {} triggers, {} types, {} sequences, {} indexes, {} views, {} materialized views, {} synonyms, {} events, {} tables, {} mappers, {} java-sql, {} java-methods, {} java-classes, {} custom, {} edges",
            procedures, functions, packages, triggers, types, sequences, indexes, views, materialized_views, synonyms, events, tables, mappers, java_sql, java_methods, java_classes, custom_nodes, edges
        );
    }
    if partial > 0 {
        eprintln!("  ⚠ {} partial nodes (unparsed body)", partial);
    }
}

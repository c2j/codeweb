mod error;
mod export;
mod graph;
#[allow(dead_code)]
mod parser;
#[allow(dead_code)]
mod project;
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

        /// Directory to initialize (default: current directory)
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,
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
}

fn main() {
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
        None => cmd_legacy(cli),
    }
}

fn cmd_init(name: &str, dir: &Path) -> Result<()> {
    let mut proj = project::Project::init(dir, name)?;
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
    let proj = project::Project::find(project)?;
    let store = proj
        .store()
        .ok_or_else(|| error::CodeWebError::ExportError {
            message: "no store found — run `codeweb analyze` first".to_string(),
        })?;

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
}

fn print_stats(graph: &graph::CodeGraph, include_unresolved: bool) {
    let mut procedures = 0usize;
    let mut unresolved = 0usize;
    let mut mappers = 0usize;
    let mut java_sql = 0usize;
    let mut java_methods = 0usize;
    let mut java_classes = 0usize;
    let mut tables = 0usize;
    let mut views = 0usize;

    for idx in graph.node_indices() {
        match &graph[idx] {
            Node::Procedure { .. } => procedures += 1,
            Node::Unresolved { .. } => unresolved += 1,
            Node::MappedStatement { .. } => mappers += 1,
            Node::JavaSql { .. } => java_sql += 1,
            Node::JavaMethod { .. } => java_methods += 1,
            Node::JavaClass { .. } => java_classes += 1,
            Node::Table { .. } => tables += 1,
            Node::View { .. } => views += 1,
        }
    }

    let edges = graph.edge_count();

    if include_unresolved {
        eprintln!(
            "graph: {} procedures, {} mappers, {} java-sql, {} java-methods, {} java-classes, {} tables, {} views, {} unresolved, {} edges",
            procedures, mappers, java_sql, java_methods, java_classes, tables, views, unresolved, edges
        );
    } else {
        eprintln!(
            "graph: {} procedures, {} mappers, {} java-sql, {} java-methods, {} java-classes, {} tables, {} views, {} edges",
            procedures, mappers, java_sql, java_methods, java_classes, tables, views, edges
        );
    }
}

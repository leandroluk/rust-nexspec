//! `nexspec` CLI (Fase 6, REQ-602..609 in
//! `.specs/features/cli-mcp-server/spec.md`) — a thin `clap` front end over
//! [`nexspec::Engine`]; every subcommand delegates to the same `Engine`
//! methods the MCP server (`nexspec mcp`) uses, so neither duplicates
//! business logic.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Parser, Subcommand};
use nexspec::{Engine, NexSpecMcp};

#[derive(Parser)]
#[command(name = "nexspec", version, about = "In-process context engine for AI coding agents")]
struct Cli {
    /// Repository root to operate on.
    #[arg(long, global = true, default_value = ".")]
    repo: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create `.specs/.index/` if it doesn't exist yet (idempotent).
    Init,
    /// Run one incremental sync cycle against the repository's Git history.
    Sync {
        /// Replay any WAL frame no store fully applied yet before syncing.
        #[arg(long)]
        resume: bool,
    },
    /// Force CSR delta-layer compaction.
    Compact,
    /// Hybrid search (BM25 + vector when available).
    Search {
        query: String,
        /// Prune/budget/serialize the result into dense Markdown fitting
        /// this many tokens (90% safety margin).
        #[arg(long = "max-tokens")]
        max_tokens: Option<u32>,
    },
    /// Deterministic topological dependency trace.
    Trace { target: String },
    /// AST-aware git blame for a symbol, scoped to its line range.
    Blame {
        symbol: String,
        /// Use the unbounded co-change window instead of the default.
        #[arg(long = "full-history")]
        full_history: bool,
    },
    /// Structural change analysis for CI/review workflows.
    Diff {
        /// Currently the only supported mode -- dirty/staged working tree.
        #[arg(long)]
        staged: bool,
    },
    /// Run the embedded MCP server over stdio.
    Mcp,
}

fn index_dir(repo: &Path) -> PathBuf {
    repo.join(".specs").join(".index")
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let repo = cli.repo.canonicalize().unwrap_or(cli.repo);
    let index_dir = index_dir(&repo);

    match cli.command {
        Command::Init => {
            Engine::open(&index_dir, &repo)?;
            println!("initialized {}", index_dir.display());
        }
        Command::Sync { resume } => {
            let engine = Engine::open(&index_dir, &repo)?;
            if resume {
                engine.resume()?;
                println!("resumed pending WAL frames");
            }
            let report = engine.sync()?;
            println!(
                "sync: target_version={:?} added={} modified={} deleted={} dirty={}",
                report.target_version, report.files_added, report.files_modified, report.files_deleted, report.files_dirty
            );
        }
        Command::Compact => {
            let engine = Engine::open(&index_dir, &repo)?;
            engine.compact()?;
            println!("compacted");
        }
        Command::Search { query, max_tokens } => {
            let engine = Engine::open(&index_dir, &repo)?;
            let result = engine.search(&query, max_tokens)?;
            if let Some(markdown) = result.markdown {
                println!("{markdown}");
            } else {
                for hit in &result.hits {
                    println!("{} {:>7.4} {}", nexspec::engine::id_hex(&hit.id), hit.score, describe_payload(&hit.payload));
                }
            }
        }
        Command::Trace { target } => {
            let engine = Engine::open(&index_dir, &repo)?;
            let result = engine.trace(&target)?;
            for hop in &result.hops {
                println!(
                    "depth={} {}{:?} {} {}",
                    hop.depth,
                    if hop.incoming { "<-" } else { "" },
                    hop.edge_type,
                    nexspec::engine::id_hex(&hop.id),
                    describe_payload(&hop.payload)
                );
            }
        }
        Command::Blame { symbol, full_history } => {
            let engine = Engine::open(&index_dir, &repo)?;
            let result = engine.blame(&symbol, full_history)?;
            for hunk in &result.hunks {
                let commit_hex: String = hunk.commit_oid.iter().map(|b| format!("{b:02x}")).collect();
                println!(
                    "{}..{} {} <{}> {}",
                    hunk.lines.start, hunk.lines.end, commit_hex, hunk.author_email, hunk.author_name
                );
            }
            if !result.co_changed_files.is_empty() {
                println!("co-changes with: {}", result.co_changed_files.join(", "));
            }
        }
        Command::Diff { staged: _ } => {
            let engine = Engine::open(&index_dir, &repo)?;
            let result = engine.diff_staged()?;
            for symbol in &result.changed_symbols {
                println!(
                    "{} {} <- [{}]",
                    nexspec::engine::id_hex(&symbol.id),
                    symbol.name,
                    symbol.dependants.iter().map(nexspec::engine::id_hex).collect::<Vec<_>>().join(", ")
                );
            }
        }
        Command::Mcp => {
            let engine = Engine::open(&index_dir, &repo)?;
            run_mcp(engine)?;
        }
    }
    Ok(())
}

fn describe_payload(payload: &nexspec::NodePayload) -> String {
    use nexspec::NodePayload::*;
    match payload {
        Requirement { title, .. } | Task { title, .. } | Adr { title, .. } => title.clone(),
        DocSection { title, .. } => title.clone(),
        File { path, .. } => path.clone(),
        Symbol { name, .. } => name.clone(),
    }
}

fn run_mcp(engine: Engine) -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(async {
        let mcp = NexSpecMcp::new(Arc::new(engine));
        let service = rmcp::ServiceExt::serve(mcp, rmcp::transport::stdio()).await?;
        service.waiting().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
}

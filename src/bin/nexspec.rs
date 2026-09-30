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

#[derive(clap::Args)]
struct BenchArgs {
    /// Corpus TOML. Default: `<repo>/.specs/bench/queries.toml`.
    #[arg(long)]
    corpus: Option<PathBuf>,
    /// Ranks to report recall for, comma separated.
    #[arg(long = "k", value_delimiter = ',', default_values_t = [5usize, 10])]
    ks: Vec<usize>,
    /// Build the index here instead of a temporary directory.
    #[arg(long)]
    index_dir: Option<PathBuf>,
    /// `md` (default) or `json`.
    #[arg(long, default_value = "md")]
    format: String,
    /// `heuristic` (default, offline) or `tiktoken`.
    #[arg(long, default_value = "heuristic")]
    tokenizer: String,
    /// `max_tokens` requested from `search` for each answer.
    #[arg(long, default_value_t = 2000)]
    budget: u32,
    /// Write the report to this file instead of stdout.
    #[arg(long)]
    output: Option<PathBuf>,
    /// BM25 only, ignoring any local ONNX model: reproducible across machines.
    #[arg(long)]
    no_vector: bool,
    /// Fail (exit 1) if `locate` recall@5 is below `--min-locate-recall` or
    /// any recall regressed > 5 points against this baseline file.
    #[arg(long, num_args = 0..=1, default_missing_value = "bench/baseline.json")]
    check: Option<PathBuf>,
    /// Record this run as the new baseline (default file: bench/baseline.json).
    #[arg(long, num_args = 0..=1, default_missing_value = "bench/baseline.json")]
    update_baseline: Option<PathBuf>,
    /// Absolute floor for `locate` recall@5 used by `--check`.
    #[arg(long, default_value_t = 0.8)]
    min_locate_recall: f64,
    /// A file an agent loads every session regardless of the question (repeatable);
    /// reported as a fixed cost with its break-even point.
    #[arg(long = "fixed-cost-file")]
    fixed_cost_files: Vec<PathBuf>,
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
        /// Print time per phase and staged node/edge counts.
        #[arg(long)]
        verbose: bool,
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
    /// Measure retrieval quality and token cost against a question corpus
    /// (never writes into the repository: the index is built in a temp dir).
    Bench(Box<BenchArgs>),
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
        Command::Sync { resume, verbose } => {
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
            if verbose {
                let t = &report.timings;
                println!(
                    "phases: diff={:.3}s markdown={:.3}s code={:.3}s co_change={:.3}s stage={:.3}s",
                    t.diff.as_secs_f64(),
                    t.markdown.as_secs_f64(),
                    t.code.as_secs_f64(),
                    t.co_change.as_secs_f64(),
                    t.stage.as_secs_f64()
                );
                println!("staged: nodes={} edges={} (co_change={})", t.nodes, t.edges, t.co_change_edges);
            }
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
                let (confidence, context) = nexspec::graph::edge::decode_meta(hop.meta);
                let mut flags = String::new();
                if confidence == nexspec::graph::edge::Confidence::Inferred {
                    flags.push_str(" [inferred]");
                }
                match context {
                    nexspec::graph::edge::EdgeContext::Runtime => {}
                    nexspec::graph::edge::EdgeContext::TypeOnly => flags.push_str(" [type-only]"),
                    nexspec::graph::edge::EdgeContext::Test => flags.push_str(" [test]"),
                    nexspec::graph::edge::EdgeContext::Spec => flags.push_str(" [spec]"),
                }
                let location = match (&hop.path, &hop.payload) {
                    (Some(path), nexspec::NodePayload::Symbol { .. }) => format!(" ({path})"),
                    _ => String::new(),
                };
                println!(
                    "depth={} {}{:?} {} {}{}{}",
                    hop.depth,
                    if hop.incoming { "<-" } else { "" },
                    hop.edge_type,
                    nexspec::engine::id_hex(&hop.id),
                    describe_payload(&hop.payload),
                    location,
                    flags
                );
            }
            if result.omitted > 0 {
                println!("(+{} omitted: raise the limit or trace a narrower target)", result.omitted);
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
        Command::Bench(args) => {
            let BenchArgs { corpus, ks, index_dir, format, tokenizer, budget, output, no_vector, check, update_baseline, min_locate_recall, fixed_cost_files } = *args;
            use nexspec::bench::{report, runner};
            let corpus_path = corpus.unwrap_or_else(|| runner::default_corpus_path(&repo));
            let corpus = nexspec::bench::Corpus::load(&corpus_path)?;
            let tokenizer = match tokenizer.as_str() {
                "heuristic" => runner::TokenizerKind::Heuristic,
                "tiktoken" => runner::TokenizerKind::Tiktoken,
                other => return Err(format!("unknown tokenizer {other:?} (expected heuristic or tiktoken)").into()),
            };
            if !matches!(format.as_str(), "md" | "json") {
                return Err(format!("unknown format {format:?} (expected md or json)").into());
            }
            let mut options = runner::BenchOptions::new(&repo);
            options.index_dir = index_dir;
            options.ks = ks;
            options.budget_tokens = budget;
            options.tokenizer = tokenizer;
            options.vector_search = !no_vector;
            options.fixed_cost_files = fixed_cost_files;
            let result = runner::run(&corpus, &options)?;
            let text = if format == "json" { report::to_json(&result) } else { report::to_markdown(&result) };
            match output {
                Some(path) => std::fs::write(path, text)?,
                None => println!("{text}"),
            }
            if let Some(path) = update_baseline {
                nexspec::bench::baseline::Baseline::from_report(&result).save(&path)?;
                eprintln!("baseline recorded in {}", path.display());
            }
            if let Some(path) = check {
                let baseline = if path.exists() {
                    Some(nexspec::bench::baseline::Baseline::load(&path)?)
                } else {
                    eprintln!("warning: no baseline at {}; only the absolute floor is checked", path.display());
                    None
                };
                let outcome = nexspec::bench::baseline::check(&result, baseline.as_ref(), min_locate_recall);
                for warning in &outcome.warnings {
                    eprintln!("warning: {warning}");
                }
                if !outcome.passed() {
                    return Err(format!("benchmark gate failed:
  - {}", outcome.failures.join("
  - ")).into());
                }
                eprintln!("benchmark gate passed");
            }
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

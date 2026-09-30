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

/// Edge filters shared by the graph queries (REQ-1105).
#[derive(clap::Args, Default)]
struct FilterArgs {
    /// Follow only these relations (repeatable): imports, reexports, calls,
    /// instantiates, extends, references, satisfies, implements, defined_in,
    /// depends_on, cochanges, or `dependencies` for all dependency kinds.
    #[arg(long = "relation")]
    relations: Vec<String>,
    /// Keep only edges from these contexts (repeatable): runtime, type-only, test, spec.
    #[arg(long = "context")]
    contexts: Vec<String>,
    /// `extracted` drops edges that were only inferred; `inferred` (default) keeps both.
    #[arg(long = "min-confidence")]
    min_confidence: Option<String>,
}

impl FilterArgs {
    fn build(&self) -> Result<nexspec::query::EdgeFilter, nexspec::query::FilterError> {
        nexspec::query::EdgeFilter::from_strings(&self.relations, self.min_confidence.as_deref(), &self.contexts)
    }
}

/// Output options shared by the graph queries (REQ-1106).
#[derive(clap::Args, Default)]
struct OutputArgs {
    /// Fit the answer into this many tokens (90% safety margin).
    #[arg(long = "max-tokens", alias = "budget")]
    max_tokens: Option<u32>,
    /// `md` (default) or `json`.
    #[arg(long, default_value = "md")]
    format: String,
}

impl OutputArgs {
    fn common(&self, pick: Option<usize>) -> Result<nexspec::query::api::Common, Box<dyn std::error::Error>> {
        if !matches!(self.format.as_str(), "md" | "json") {
            return Err(format!("unknown format {:?} (expected md or json)", self.format).into());
        }
        Ok(nexspec::query::api::Common { max_tokens: self.max_tokens, json: self.format == "json", pick })
    }
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
    Trace {
        target: String,
        /// Levels to walk (default 3).
        #[arg(long)]
        depth: Option<u8>,
        /// Fit the trace into this many tokens.
        #[arg(long = "max-tokens", alias = "budget")]
        max_tokens: Option<u32>,
    },
    /// Answer a question from the graph: hybrid-search seeds expanded through their relations.
    Query {
        question: String,
        /// Depth-first instead of breadth-first.
        #[arg(long)]
        dfs: bool,
        /// How far to expand from the seeds.
        #[arg(long, default_value_t = 3)]
        depth: u8,
        /// Token budget for the answer (default 2000).
        #[arg(long = "max-tokens", alias = "budget")]
        max_tokens: Option<u32>,
        #[command(flatten)]
        filter: FilterArgs,
        /// `md` (default) or `json`.
        #[arg(long, default_value = "md")]
        format: String,
    },
    /// Shortest chain of relations between two nodes (symbol, file or REQ-…).
    Path {
        from: String,
        to: String,
        #[command(flatten)]
        filter: FilterArgs,
        #[command(flatten)]
        output: OutputArgs,
    },
    /// Describe one node: where it is, what it depends on, what depends on it, requirements, community, authors.
    Explain {
        target: String,
        /// Choose among ambiguous matches (1-based, as listed by the error).
        #[arg(long)]
        pick: Option<usize>,
        #[command(flatten)]
        filter: FilterArgs,
        #[command(flatten)]
        output: OutputArgs,
    },
    /// Who depends on a node, transitively, grouped by file.
    Affected {
        target: String,
        /// Levels to walk (default 2).
        #[arg(long, default_value_t = 2)]
        depth: u8,
        /// Most nodes listed per level (default 25).
        #[arg(long, default_value_t = 25)]
        limit: usize,
        /// Choose among ambiguous matches (1-based, as listed by the error).
        #[arg(long)]
        pick: Option<usize>,
        #[command(flatten)]
        filter: FilterArgs,
        #[command(flatten)]
        output: OutputArgs,
    },
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
    Mcp {
        /// Also keep the index fresh: sync after changes in the working tree.
        #[arg(long)]
        watch: bool,
        /// Quiet time (ms) after the last change before `--watch` syncs.
        #[arg(long, default_value_t = 500)]
        debounce: u64,
    },
    /// Watch the working tree and sync after each burst of changes (one watcher per repository).
    Watch {
        /// Quiet time (ms) after the last change before syncing.
        #[arg(long, default_value_t = 500)]
        debounce: u64,
        /// Do not sync once at start-up.
        #[arg(long)]
        no_initial_sync: bool,
    },
    /// Structural report: God nodes, communities, requirement coverage,
    /// surprising connections, import cycles and suggested questions.
    Report {
        /// `md` (default) or `json`.
        #[arg(long, default_value = "md")]
        format: String,
        /// Fit the Markdown report into this many tokens (90% safety margin).
        #[arg(long = "max-tokens")]
        max_tokens: Option<u32>,
        /// God nodes to list.
        #[arg(long, default_value_t = 10)]
        top: usize,
        /// Exit with status 2 when an import cycle exists (for CI).
        #[arg(long)]
        fail_on_cycle: bool,
        /// Compare the current graph with this revision (branch, tag, sha, `HEAD~3`...)
        /// instead of printing the report.
        #[arg(long)]
        diff: Option<String>,
    },
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
        Command::Trace { target, depth, max_tokens } => {
            let engine = Engine::open(&index_dir, &repo)?;
            let mut trace_options = nexspec::engine::TraceOptions::default();
            if let Some(depth) = depth {
                trace_options.max_depth = depth;
            }
            let result = engine.trace_with(&target, trace_options)?;
            let mut printed: Vec<String> = Vec::new();
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
                printed.push(format!(
                    "depth={} {}{:?} {} {}{}{}",
                    hop.depth,
                    if hop.incoming { "<-" } else { "" },
                    hop.edge_type,
                    nexspec::engine::id_hex(&hop.id),
                    describe_payload(&hop.payload),
                    location,
                    flags
                ));
            }
            if result.omitted > 0 {
                printed.push(format!("(+{} omitted: raise the limit or trace a narrower target)", result.omitted));
            }
            if max_tokens.is_some() {
                // A title line is what the budget keeps no matter what.
                printed.insert(0, format!("# Trace of {target}"));
            }
            for line in nexspec::query::budget::fit_lines(printed, max_tokens) {
                println!("{line}");
            }
        }
        Command::Query { question, dfs, depth, max_tokens, filter, format } => {
            let output = OutputArgs { max_tokens: Some(max_tokens.unwrap_or(2000)), format };
            let engine = Engine::open(&index_dir, &repo)?;
            let options = nexspec::query::expand::ExpandOptions { dfs, max_depth: depth, filter: filter.build()?, ..Default::default() };
            print!("{}", nexspec::query::api::query_graph(&engine, &question, options, &output.common(None)?)?);
        }
        Command::Path { from, to, filter, output } => {
            let engine = Engine::open(&index_dir, &repo)?;
            print!("{}", nexspec::query::api::find_path(&engine, &from, &to, &filter.build()?, &output.common(None)?)?);
        }
        Command::Explain { target, pick, filter, output } => {
            let engine = Engine::open(&index_dir, &repo)?;
            print!("{}", nexspec::query::api::explain(&engine, &target, &filter.build()?, &output.common(pick)?)?);
        }
        Command::Affected { target, depth, limit, pick, filter, output } => {
            let engine = Engine::open(&index_dir, &repo)?;
            let options = nexspec::query::affected::AffectedOptions { depth, max_per_hop: limit.max(1), filter: filter.build()? };
            print!("{}", nexspec::query::api::affected(&engine, &target, options, &output.common(pick)?)?);
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
        Command::Mcp { watch, debounce } => {
            let engine = Arc::new(Engine::open(&index_dir, &repo)?);
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let watcher = watch.then(|| {
                let (engine, stop, repo) = (Arc::clone(&engine), Arc::clone(&stop), repo.clone());
                std::thread::spawn(move || {
                    let options = nexspec::workflow::watch::WatchOptions {
                        debounce: std::time::Duration::from_millis(debounce),
                        ..Default::default()
                    };
                    let result = nexspec::workflow::watch::run(
                        &repo,
                        &options,
                        &stop,
                        || engine.sync().map(|_| ()).map_err(|e| e.to_string()),
                        |message| eprintln!("nexspec watch: sync failed: {message}"),
                    );
                    if let Err(e) = result {
                        eprintln!("nexspec watch: {e}");
                    }
                })
            });
            let served = run_mcp(Arc::clone(&engine));
            stop.store(true, std::sync::atomic::Ordering::SeqCst);
            if let Some(handle) = watcher {
                let _ = handle.join();
            }
            served?;
        }
        Command::Watch { debounce, no_initial_sync } => {
            std::fs::create_dir_all(&index_dir)?;
            let _watch_lock = nexspec::sync::SyncLock::acquire_named(&index_dir, "watch.lock", std::time::Duration::ZERO).map_err(|e| match e {
                nexspec::sync::LockError::Timeout { holder, .. } => {
                    format!("another `nexspec watch` is already running for this repository{holder}")
                }
                other => other.to_string(),
            })?;
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            {
                let stop = Arc::clone(&stop);
                ctrlc::set_handler(move || stop.store(true, std::sync::atomic::Ordering::SeqCst))?;
            }
            // The index is opened only for the length of a cycle, so hooks and manual
            // syncs are never kept waiting behind the watcher.
            let cycle = || -> Result<(), String> {
                let engine = Engine::open(&index_dir, &repo).map_err(|e| e.to_string())?;
                let report = engine.sync().map_err(|e| e.to_string())?;
                if report.target_version.is_some() {
                    println!(
                        "synced: added={} modified={} deleted={} dirty={}",
                        report.files_added, report.files_modified, report.files_deleted, report.files_dirty
                    );
                }
                Ok(())
            };
            eprintln!("watching {} (Ctrl+C to stop)", repo.display());
            if !no_initial_sync && let Err(message) = cycle() {
                eprintln!("nexspec watch: initial sync failed: {message}");
            }
            let options = nexspec::workflow::watch::WatchOptions {
                debounce: std::time::Duration::from_millis(debounce),
                ..Default::default()
            };
            nexspec::workflow::watch::run(&repo, &options, &stop, cycle, |message| {
                eprintln!("nexspec watch: sync failed: {message}");
            })?;
            eprintln!("stopped");
        }
        Command::Report { format, max_tokens, top, fail_on_cycle, diff } => {
            if !matches!(format.as_str(), "md" | "json") {
                return Err(format!("unknown format {format:?} (expected md or json)").into());
            }
            let engine = Engine::open(&index_dir, &repo)?;
            let options = nexspec::report::ReportOptions { top: top.max(1), ..Default::default() };
            if let Some(revision) = diff {
                use nexspec::report::diff as report_diff;
                // The current graph is whatever the index holds; bring it up to date first.
                engine.sync()?;
                let snapshot = engine.snapshot()?;
                let current = nexspec::report::build(&snapshot, engine.index_info()?, options);
                let (base, base_files) = report_diff::report_at_revision(&repo, &revision, options)?;
                let result = report_diff::compare(&revision, &base, &base_files, &current, &report_diff::file_paths(&snapshot));
                if format == "json" {
                    println!("{}", serde_json::to_string_pretty(&result)?);
                } else {
                    print!("{}", report_diff::to_markdown(&result));
                }
                if fail_on_cycle && !result.import_cycles.new.is_empty() {
                    eprintln!("error: {} new import cycle(s) since {revision}", result.import_cycles.new.len());
                    std::process::exit(2);
                }
                return Ok(());
            }
            let report = engine.report(options)?;
            if format == "json" {
                println!("{}", nexspec::report::render::to_json(&report));
            } else {
                print!("{}", nexspec::report::render::to_markdown(&report, max_tokens));
            }
            if fail_on_cycle && !report.import_cycles.is_empty() {
                eprintln!("error: {} import cycle(s) found", report.import_cycles.len());
                std::process::exit(2);
            }
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

fn run_mcp(engine: Arc<Engine>) -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(async {
        let mcp = NexSpecMcp::new(engine);
        let service = rmcp::ServiceExt::serve(mcp, rmcp::transport::stdio()).await?;
        service.waiting().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
}

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
    /// Ask the global graph (see `global add`) instead of this repository. With `--global`, `--repo TAG`
    /// picks the repository the target is looked up in; the walk still crosses repositories.
    #[arg(long)]
    global: bool,
}

impl OutputArgs {
    fn common(&self, pick: Option<usize>) -> Result<nexspec::query::api::Common, Box<dyn std::error::Error>> {
        if !matches!(self.format.as_str(), "md" | "json") {
            return Err(format!("unknown format {:?} (expected md or json)", self.format).into());
        }
        Ok(nexspec::query::api::Common { max_tokens: self.max_tokens, json: self.format == "json", pick, repo: None })
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
    /// Run the corpus without and with the `enrich` summaries and judge the difference (REQ-1911).
    #[arg(long)]
    compare_enrich: bool,
}

#[derive(clap::Args)]
struct PlatformArgs {
    /// Agent to configure: claude, gemini, cursor, vscode or codex (repeatable, or `all`).
    #[arg(long = "platform", required = true)]
    platforms: Vec<String>,
    /// Where the configuration lives: `user` is required for codex and invalid for the others.
    #[arg(long, value_enum)]
    scope: Option<ScopeArg>,
    /// Show what would change without writing anything.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum ScopeArg {
    Project,
    User,
}

#[derive(clap::Args)]
struct EnrichArgs {
    /// Provider: `gemini` (needs GEMINI_API_KEY). `fake` reads NEXSPEC_ENRICH_FIXTURES (tests).
    #[arg(long, default_value = "gemini")]
    provider: String,
    /// Model name (default: NEXSPEC_ENRICH_MODEL or the provider's default).
    #[arg(long)]
    model: Option<String>,
    /// Summary languages, comma separated (for example `en,pt`). One index field per language.
    #[arg(long, default_value = "en", value_delimiter = ',')]
    lang: Vec<String>,
    /// Files per request.
    #[arg(long, default_value_t = 8)]
    batch: usize,
    /// Requests in flight at once.
    #[arg(long, default_value_t = 4)]
    max_concurrency: usize,
    /// Only the most important files: `300` or `20%`.
    #[arg(long)]
    top: Option<String>,
    /// Stop after this many tokens (input + output, as the provider reports them).
    #[arg(long)]
    token_budget: Option<u64>,
    /// Characters of each file that are sent.
    #[arg(long, default_value_t = nexspec::enrich::select::DEFAULT_SNIPPET_CHARS)]
    snippet_chars: usize,
    /// Estimate files, tokens and cost offline; send nothing.
    #[arg(long)]
    dry_run: bool,
    /// Show what is enriched, stale and pending; send nothing.
    #[arg(long)]
    status: bool,
    /// Delete the enrichment cache (the index keeps working; summaries leave it at the next sync).
    #[arg(long)]
    clear: bool,
    /// Do not ask for confirmation (first use, or more than 500 000 estimated input tokens).
    #[arg(long)]
    yes: bool,
}

#[derive(clap::Args)]
struct ExtractArgs {
    /// Connection string of the database to read (`postgres://user:pass@host/db`). Read-only; never stored.
    #[arg(long, conflicts_with = "live_file")]
    postgres: Option<String>,
    /// A schema previously saved as JSON (`.specs/.cache/live-schema.json` format) instead of a connection.
    #[arg(long)]
    live_file: Option<PathBuf>,
    /// Only report the drift: do not save the live schema or touch the index.
    #[arg(long)]
    dry_run: bool,
}

#[derive(clap::Args)]
struct ExportArgs {
    /// `json` (portable, importable), `html` (interactive graph), `tree` (collapsible hierarchy) or `wiki` (Markdown per community).
    #[arg(long, default_value = "json")]
    format: String,
    /// File to write (`wiki`: a directory). Without it, json/html/tree go to stdout.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Keep only nodes whose path matches this glob (repeatable), e.g. `src/**`.
    #[arg(long = "path")]
    paths: Vec<String>,
    /// Keep only these node kinds (repeatable): file, symbol, requirement, task, adr, doc_section, table, view, column, constraint, package.
    #[arg(long = "kind")]
    kinds: Vec<String>,
    /// `html`: draw at most this many nodes (the most connected), and say how many were left out.
    #[arg(long, default_value_t = nexspec::export::html::DEFAULT_MAX_NODES)]
    max_nodes: usize,
    /// Do not write: exit 7 if the existing export differs from what would be written now.
    #[arg(long, requires = "out")]
    check: bool,
}

#[derive(Subcommand)]
enum GlobalAction {
    /// Add a repository (its synced local index) or an export file to the global graph; the same `--as` replaces it.
    Add {
        /// A repository directory, or a JSON export made with `nexspec export`.
        source: PathBuf,
        /// Tag the repository goes by (default: the directory or file name).
        #[arg(long = "as")]
        tag: Option<String>,
    },
    /// The repositories in the global graph.
    List,
    /// Take a repository out of the global graph.
    Remove { tag: String },
    /// Print the folder that holds the global graph.
    Path,
}

#[derive(clap::Args)]
struct MergeArgs {
    /// Two or more JSON exports to unite.
    #[arg(required = true, num_args = 1..)]
    inputs: Vec<PathBuf>,
    /// Tag for each input, in order (default: the file name without extension).
    #[arg(long = "as")]
    tags: Vec<String>,
    /// Where to write the merged export.
    #[arg(long)]
    out: PathBuf,
}

#[derive(Subcommand)]
enum HookAction {
    /// Append the nexspec block to post-commit, post-merge and post-checkout (idempotent).
    Install,
    /// Remove only the nexspec block from those hooks.
    Uninstall,
    /// Show which hooks carry the block.
    Status,
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
        /// Ignore the file summaries written by `enrich`; rank on code text only.
        #[arg(long)]
        no_enrich: bool,
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
        /// Ignore the file summaries written by `enrich`; rank on code text only.
        #[arg(long)]
        no_enrich: bool,
        /// Ask the global graph (see `global add`); `--repo TAG` narrows seeds and targets to one repository.
        #[arg(long)]
        global: bool,
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
    /// Install, remove or inspect the git hooks that run `nexspec sync` after commit, merge and checkout.
    Hook {
        #[command(subcommand)]
        action: HookAction,
    },
    /// Register the nexspec MCP server with a coding agent (backup first, other entries untouched).
    Install(PlatformArgs),
    /// Remove the nexspec MCP server entry from a coding agent's configuration.
    Uninstall(PlatformArgs),
    /// Check the installation (build, model, index, WAL, hooks, agents, .gitignore); exit 5 on any failure.
    Doctor,
    /// Ask an LLM for a short summary of each file, so prose questions can find code (opt-in; sends code
    /// to the provider; never runs from a hook). `--dry-run` and `--status` make no request.
    Enrich(EnrichArgs),
    /// Compare the changesets with a live PostgreSQL database (read-only, opt-in) and add the objects that
    /// exist only there to the graph. Prints `drift: none` or `drift: N difference(s)` first.
    Extract(ExtractArgs),
    /// The graph of several repositories in one: add, list, remove, or find the folder (`--global` queries use it).
    Global {
        #[command(subcommand)]
        action: GlobalAction,
    },
    /// Git merge driver for `*.graph.json` exports (called by Git: `nexspec merge-driver %O %A %B`); writes the union over `ours`.
    MergeDriver {
        base: PathBuf,
        ours: PathBuf,
        theirs: PathBuf,
    },
    /// Unite JSON exports of several repositories into one export (ids tagged per repository, links across them).
    MergeGraphs(MergeArgs),
    /// Export the graph: portable JSON, an interactive HTML page, a collapsible tree or a Markdown wiki.
    Export(ExportArgs),
    /// Say whether the index is in step with HEAD and the working tree. First stdout line:
    /// `up-to-date` (exit 0), `stale: <reason>` (exit 3) or `no-index` (exit 4). Never writes.
    CheckUpdate,
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

fn engine_options(no_enrich: bool) -> nexspec::engine::EngineOptions {
    nexspec::engine::EngineOptions { summary_weight: no_enrich.then_some(0.0), ..Default::default() }
}

/// Exit code of `enrich` when some files could not be summarised (REQ-1909).
const EXIT_PARTIAL: i32 = 6;

fn run_enrich(repo: &Path, index_dir: &Path, args: &EnrichArgs) -> Result<i32, Box<dyn std::error::Error>> {
    use nexspec::enrich::cost::{CONFIRM_ABOVE_INPUT_TOKENS, Pricing, coverage_curve, format_usd};
    use nexspec::enrich::provider::{EnrichProvider, FakeProvider, GeminiProvider};
    use nexspec::enrich::{cache::EnrichmentCache, run as enrich_run, select::Top};
    use std::io::IsTerminal;

    if args.clear {
        let removed = enrich_run::clear(repo)?;
        println!("{}", if removed { "enrichment cache cleared" } else { "no enrichment cache" });
        return Ok(0);
    }
    let langs: Vec<String> = args.lang.iter().map(|l| l.trim().to_lowercase()).filter(|l| !l.is_empty()).collect();
    let engine = Engine::open(index_dir, repo)?;

    if args.status {
        let status = enrich_run::status(&engine, repo, &langs)?;
        println!("{}", status.first_line());
        let state = &status.state;
        println!("cache: {} entries in {}", status.cache_entries, enrich_run::cache_dir(repo).display());
        if state.runs > 0 {
            println!("last run: model {}, {} run(s), {} input / {} output tokens, {}", state.model, state.runs, state.input_tokens, state.output_tokens, format_usd(state.cost_usd));
            for path in &state.failed {
                println!("failed: {path}");
            }
            for item in &state.omitted_secret {
                println!("omitted (secret): {item}");
            }
        }
        return Ok(0);
    }

    let options = enrich_run::EnrichOptions {
        langs: langs.clone(),
        batch: args.batch,
        concurrency: args.max_concurrency,
        top: args.top.as_deref().map(Top::parse).transpose()?,
        token_budget: args.token_budget,
        snippet_chars: args.snippet_chars,
    };
    let provider: Box<dyn EnrichProvider> = match (args.dry_run, args.provider.as_str()) {
        (_, "fake") => {
            let path = std::env::var("NEXSPEC_ENRICH_FIXTURES").map_err(|_| "NEXSPEC_ENRICH_FIXTURES is not set")?;
            let table: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>> = serde_json::from_str(&std::fs::read_to_string(path)?)?;
            let answers: Vec<(&str, Vec<(&str, &str)>)> = table.iter().map(|(p, l)| (p.as_str(), l.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect())).collect();
            let borrowed: Vec<(&str, &[(&str, &str)])> = answers.iter().map(|(p, l)| (*p, l.as_slice())).collect();
            Box::new(FakeProvider::new(&borrowed))
        }
        (false, "gemini") => Box::new(GeminiProvider::from_env(args.model.as_deref())?),
        (true, "gemini") => Box::new(FakeProvider::new(&[])),
        (_, other) => return Err(format!("unknown provider `{other}` (gemini)").into()),
    };
    let model = match (&args.model, args.dry_run && args.provider == "gemini") {
        (Some(model), _) => model.clone(),
        (None, true) => std::env::var("NEXSPEC_ENRICH_MODEL").unwrap_or_else(|_| nexspec::enrich::provider::DEFAULT_MODEL.to_string()),
        (None, false) => provider.model().to_string(),
    };

    let cache = EnrichmentCache::load(repo)?;
    let plan = enrich_run::plan(&engine, repo, &cache, &options, &model)?;
    let pricing = Pricing::for_model(&model);
    let snippets: Vec<&str> = plan.files.iter().map(|f| f.snippet.as_str()).collect();
    let total = nexspec::enrich::cost::estimate(&snippets, options.langs.len(), options.batch, &pricing);

    println!(
        "{} file(s) to send ({} already up to date, {} kept back for looking like secrets), {} chars, ~{} input / ~{} output tokens, ~{} with {model}{}",
        plan.files.len(),
        plan.fresh,
        plan.omitted_secret.len(),
        total.chars,
        total.usage.input_tokens,
        total.usage.output_tokens,
        format_usd(total.cost_usd),
        if pricing.known { "" } else { " (price unknown for this model: set NEXSPEC_ENRICH_PRICE_IN/OUT)" }
    );
    if args.dry_run {
        for (percent, estimate) in coverage_curve(&snippets, options.langs.len(), options.batch, &pricing) {
            println!("  top {percent:>3}%: {:>5} files, ~{} tokens in, ~{}", estimate.files, estimate.usage.input_tokens, format_usd(estimate.cost_usd));
        }
        for (path, kind) in &plan.omitted_secret {
            println!("  omitted: {path} ({kind})");
        }
        return Ok(0);
    }
    if plan.files.is_empty() {
        println!("nothing to do");
        return Ok(0);
    }

    let first_use = !nexspec::enrich::run::State::load(repo).used_before();
    let large = total.usage.input_tokens > CONFIRM_ABOVE_INPUT_TOKENS;
    if (first_use || large) && !args.yes {
        if !std::io::stdin().is_terminal() {
            return Err(if large {
                format!("~{} input tokens is a large run: pass --yes to go ahead", total.usage.input_tokens).into()
            } else {
                "the first `enrich` sends code to the provider: pass --yes to confirm".into()
            });
        }
        eprint!("send {} file(s), {} chars, to {} ({model}) in {}? [y/N] ", plan.files.len(), total.chars, args.provider, options.langs.join(","));
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
            println!("cancelled, nothing was sent");
            return Ok(0);
        }
    }

    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let stop = Arc::clone(&stop);
        ctrlc::set_handler(move || stop.store(true, std::sync::atomic::Ordering::SeqCst))?;
    }
    let report = enrich_run::run(&engine, repo, provider.as_ref(), &options, plan, &stop, |progress, batches| {
        eprintln!("batch {}/{batches}: {} file(s) done, {} failed", progress.batches, progress.enriched_files, progress.failed.len());
    })?;
    for (path, reason) in &report.failed {
        eprintln!("warning: {path}: {reason}");
    }
    println!(
        "enriched {} file(s), {} failed; {} input / {} output tokens, {}{}{}",
        report.enriched_files,
        report.failed.len(),
        report.usage.input_tokens,
        report.usage.output_tokens,
        format_usd(pricing.cost(report.usage)),
        if report.budget_reached { "; stopped at the token budget" } else { "" },
        if report.interrupted { "; interrupted, progress saved" } else { "" },
    );
    Ok(if report.failed.is_empty() { 0 } else { EXIT_PARTIAL })
}

/// Exit code of `export --check` when the export on disk is out of date.
const EXIT_EXPORT_STALE: i32 = 7;

fn run_export(repo: &Path, index_dir: &Path, args: &ExportArgs) -> Result<i32, Box<dyn std::error::Error>> {
    use nexspec::export::{ExportFilter, html, tree, wiki};
    let engine = Engine::open(index_dir, repo)?;
    let graph = engine.export_graph(&ExportFilter { paths: args.paths.clone(), kinds: args.kinds.clone() })?;
    // Relative file name -> content; the single-file formats use an empty name for `--out` itself.
    let files: std::collections::BTreeMap<String, String> = match args.format.as_str() {
        "json" => [(String::new(), graph.to_json())].into(),
        "html" => [(String::new(), html::render(&graph, args.max_nodes))].into(),
        "tree" => [(String::new(), tree::render(&graph))].into(),
        "wiki" => wiki::render(&graph),
        other => return Err(format!("unknown format {other:?} (expected json, html, tree or wiki)").into()),
    };
    let wiki = args.format == "wiki";
    let Some(out) = &args.out else {
        if wiki {
            return Err("--format wiki needs --out <directory>".into());
        }
        print!("{}", files.values().next().expect("one file"));
        return Ok(0);
    };
    let target = |name: &str| if name.is_empty() { out.clone() } else { out.join(name) };
    let normalise = |text: &str| text.replace("\r\n", "\n");

    if args.check {
        let mut stale: Vec<String> = Vec::new();
        for (name, content) in &files {
            match std::fs::read_to_string(target(name)) {
                Ok(existing) if normalise(&existing) == *content => {}
                Ok(_) => stale.push(format!("{} differs", target(name).display())),
                Err(_) => stale.push(format!("{} is missing", target(name).display())),
            }
        }
        if wiki {
            for extra in wiki_files_on_disk(out) {
                if !files.contains_key(&extra) {
                    stale.push(format!("{} is no longer generated", out.join(&extra).display()));
                }
            }
        }
        if stale.is_empty() {
            println!("export up to date");
            return Ok(0);
        }
        println!("export stale: {}", stale.len());
        for line in stale.iter().take(10) {
            println!("  {line}");
        }
        return Ok(EXIT_EXPORT_STALE);
    }

    if wiki {
        // Articles of communities that no longer exist must not linger.
        for extra in wiki_files_on_disk(out) {
            if !files.contains_key(&extra) {
                let _ = std::fs::remove_file(out.join(&extra));
            }
        }
    }
    for (name, content) in &files {
        let path = target(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, content)?;
    }
    println!("exported {} file(s) to {}", files.len(), out.display());
    Ok(0)
}

/// The `.md` files a previous wiki export left in `out` (`index.md` and `communities/*.md`).
fn wiki_files_on_disk(out: &Path) -> Vec<String> {
    let mut found = Vec::new();
    if out.join("index.md").is_file() {
        found.push("index.md".to_string());
    }
    if let Ok(entries) = std::fs::read_dir(out.join("communities")) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".md") {
                found.push(format!("communities/{name}"));
            }
        }
    }
    found.sort();
    found
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)
}

fn run_platforms(repo: &Path, args: &PlatformArgs, install: bool) -> Result<(), Box<dyn std::error::Error>> {
    use nexspec::workflow::install::{self as inst, Action, Platform, Scope};
    let mut platforms = Vec::new();
    for name in &args.platforms {
        if name == "all" {
            // Codex is user-level: only with an explicit `--scope user`.
            platforms.extend(Platform::ALL.into_iter().filter(|p| *p != Platform::Codex || matches!(args.scope, Some(ScopeArg::User))));
        } else {
            platforms.push(Platform::parse(name).ok_or_else(|| format!("unknown platform `{name}` (claude, gemini, cursor, vscode, codex, all)"))?);
        }
    }
    let scope = args.scope.map(|s| match s {
        ScopeArg::Project => Scope::Project,
        ScopeArg::User => Scope::User,
    });
    let home = home_dir();
    // Plan everything first: a bad platform must not leave the others half done.
    let mut plans = Vec::new();
    for platform in platforms {
        // `all --scope user` also covers the project-level agents.
        let scope_for = if platform == Platform::Codex { scope } else { scope.filter(|s| *s != Scope::User) };
        let plan = if install {
            inst::plan_install(platform, scope_for, repo, home.as_deref())?
        } else {
            inst::plan_uninstall(platform, scope_for, repo, home.as_deref())?
        };
        plans.push(plan);
    }
    for plan in &plans {
        let verb = match (plan.action, args.dry_run) {
            (Action::Unchanged, _) => "unchanged",
            (Action::Create, true) => "would create",
            (Action::Create, false) => "created",
            (Action::Update, true) => "would update",
            (Action::Update, false) => "updated",
            (Action::Remove, true) => "would remove the entry from",
            (Action::Remove, false) => "removed the entry from",
        };
        println!("{}: {verb} {}", plan.platform.name(), plan.path.display());
        for note in &plan.notes {
            println!("  note: {note}");
        }
        if !args.dry_run && let Some(backup) = inst::apply(plan)? {
            println!("  backup: {}", backup.display());
        }
    }
    Ok(())
}

fn index_dir(repo: &Path) -> PathBuf {
    repo.join(".specs").join(".index")
}

/// The engine a query runs on: this repository's index, or the global graph.
fn query_engine(repo: &Path, index_dir: &Path, global: bool, options: nexspec::engine::EngineOptions) -> Result<Engine, Box<dyn std::error::Error>> {
    if !global {
        return Ok(Engine::open_with(index_dir, repo, options)?);
    }
    use nexspec::global::store;
    let home = store::home()?;
    let index = store::index_dir(&home);
    if !index.join("metadata.redb").is_file() {
        return Err("no global graph yet: add a repository with `nexspec global add <repo>`".into());
    }
    Ok(Engine::open_with(&index, &store::global_dir(&home), options)?)
}

/// With `--global`, an explicit `--repo` is a repository *tag* (the default `.` means "all").
fn global_repo(repo_arg: &Path, global: bool) -> Option<String> {
    (global && repo_arg != Path::new(".")).then(|| repo_arg.to_string_lossy().to_string())
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
    let repo_arg = cli.repo.clone();
    let repo = cli.repo.canonicalize().unwrap_or(cli.repo);
    let index_dir = index_dir(&repo);

    match cli.command {
        Command::Hook { action } => {
            use nexspec::workflow::hooks::{self, HookChange, HookState};
            match action {
                HookAction::Install | HookAction::Uninstall => {
                    let installing = matches!(action, HookAction::Install);
                    let changes = if installing { hooks::install(&repo)? } else { hooks::uninstall(&repo)? };
                    for (name, change) in changes {
                        let what = match (change, installing) {
                            (HookChange::Added, _) => "installed",
                            (HookChange::Removed, _) => "removed",
                            (HookChange::Unchanged, true) => "already installed",
                            (HookChange::Unchanged, false) => "not installed",
                        };
                        println!("{name}: {what}");
                    }
                }
                HookAction::Status => {
                    for (name, state) in hooks::status(&repo)? {
                        println!("{name}: {}", if state == HookState::Installed { "installed" } else { "not installed" });
                    }
                }
            }
        }
        Command::Global { action } => {
            use nexspec::export::ExportGraph;
            use nexspec::global::store;
            let home = store::home()?;
            match action {
                GlobalAction::Add { source, tag } => {
                    let tag = tag.unwrap_or_else(|| store::default_tag(&source));
                    let graph = if source.is_dir() {
                        store::export_repository(&source)?
                    } else {
                        ExportGraph::from_json(&std::fs::read_to_string(&source)?)?
                    };
                    let entry = store::add(&home, &tag, &graph, &source.display().to_string())?;
                    println!("added {}: {} nodes, {} edges", entry.tag, entry.nodes, entry.edges);
                    println!("global graph: {} repositories", store::list(&home).len());
                }
                GlobalAction::List => {
                    let entries = store::list(&home);
                    if entries.is_empty() {
                        println!("no repositories in the global graph");
                    }
                    for entry in entries {
                        println!("{}  {} nodes, {} edges  ({})", entry.tag, entry.nodes, entry.edges, entry.source);
                    }
                }
                GlobalAction::Remove { tag } => {
                    if store::remove(&home, &tag)? {
                        println!("removed {tag}");
                    } else {
                        return Err(format!("no repository tagged `{tag}` (see `nexspec global list`)").into());
                    }
                }
                GlobalAction::Path => println!("{}", store::global_dir(&home).display()),
            }
        }
        Command::MergeDriver { base, ours, theirs } => {
            nexspec::global::driver::run(&base, &ours, &theirs)?;
        }
        Command::MergeGraphs(args) => {
            use nexspec::export::ExportGraph;
            if !args.tags.is_empty() && args.tags.len() != args.inputs.len() {
                return Err(format!("{} input(s) but {} --as tag(s): give one tag per input, or none", args.inputs.len(), args.tags.len()).into());
            }
            let mut parts = Vec::new();
            for (i, input) in args.inputs.iter().enumerate() {
                let tag = args.tags.get(i).cloned().unwrap_or_else(|| nexspec::global::store::default_tag(input));
                if !nexspec::global::merge::valid_tag(&tag) {
                    return Err(format!("invalid tag `{tag}` for {}: use letters, digits, `.`, `_` and `-`", input.display()).into());
                }
                parts.push((tag, ExportGraph::from_json(&std::fs::read_to_string(input)?)?));
            }
            let merged = nexspec::global::merge::merge(parts);
            std::fs::write(&args.out, merged.to_json())?;
            println!("merged {} export(s): {} nodes, {} edges -> {}", args.inputs.len(), merged.nodes.len(), merged.edges.len(), args.out.display());
        }
        Command::Export(args) => {
            let code = run_export(&repo, &index_dir, &args)?;
            if code != 0 {
                std::process::exit(code);
            }
        }
        Command::Extract(args) => {
            use nexspec::domain::live;
            let dsn = args.postgres.clone().or_else(|| std::env::var("NEXSPEC_POSTGRES_DSN").ok().filter(|v| !v.trim().is_empty()));
            let schema = match (&dsn, &args.live_file) {
                (_, Some(path)) => live::read_file(path)?,
                (Some(dsn), None) => live::read_database(dsn)?,
                _ => return Err("pass --postgres <DSN> (or set NEXSPEC_POSTGRES_DSN) or --live-file <FILE>".into()),
            };
            let engine = Engine::open(&index_dir, &repo)?;
            let drift = live::compare(&engine.changeset_schema()?, &schema);
            print!("{}", drift.render());
            if !args.dry_run {
                live::save(&repo, &schema)?;
                let report = engine.sync_domain()?;
                println!("live schema: {} object(s) saved to {}; graph updated (version {:?})", schema.len(), live::cache_path(&repo).display(), report.target_version);
            }
        }
        Command::Enrich(args) => {
            let code = run_enrich(&repo, &index_dir, &args)?;
            if code != 0 {
                std::process::exit(code);
            }
        }
        Command::Install(args) => run_platforms(&repo, &args, true)?,
        Command::Uninstall(args) => run_platforms(&repo, &args, false)?,
        Command::Doctor => {
            let checks = nexspec::workflow::doctor::run(&repo, &index_dir, home_dir().as_deref());
            print!("{}", nexspec::workflow::doctor::render(&checks));
            std::process::exit(nexspec::workflow::doctor::exit_code(&checks));
        }
        Command::CheckUpdate => {
            let freshness = nexspec::workflow::check::check(&repo, &index_dir)?;
            println!("{}", freshness.line());
            std::process::exit(freshness.exit_code());
        }
        Command::Init => {
            Engine::open(&index_dir, &repo)?;
            println!("initialized {}", index_dir.display());
            use nexspec::workflow::gitignore::{CACHE_ENTRY, INDEX_ENTRY, ensure};
            for added in ensure(&repo, &[INDEX_ENTRY, CACHE_ENTRY])? {
                println!("added {added} to .gitignore");
            }
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
            // Only when there already is an enrichment cache: a hint, no network, never an error.
            if nexspec::enrich::cache::cache_path(&repo).is_file()
                && let Ok(status) = nexspec::enrich::run::status(&engine, &repo, &[])
                && (status.stale > 0 || status.pending > 0)
            {
                println!("enrichment: {} stale, {} pending — nexspec enrich", status.stale, status.pending);
            }
            if verbose {
                let t = &report.timings;
                println!(
                    "phases: diff={:.3}s markdown={:.3}s code={:.3}s domain={:.3}s co_change={:.3}s stage={:.3}s",
                    t.diff.as_secs_f64(),
                    t.markdown.as_secs_f64(),
                    t.code.as_secs_f64(),
                    t.domain.as_secs_f64(),
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
        Command::Search { query, max_tokens, no_enrich } => {
            let engine = Engine::open_with(&index_dir, &repo, engine_options(no_enrich))?;
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
        Command::Query { question, dfs, depth, max_tokens, no_enrich, global, filter, format } => {
            let output = OutputArgs { max_tokens: Some(max_tokens.unwrap_or(2000)), format, global };
            let engine = query_engine(&repo, &index_dir, global, engine_options(no_enrich))?;
            let options = nexspec::query::expand::ExpandOptions { dfs, max_depth: depth, filter: filter.build()?, ..Default::default() };
            let mut common = output.common(None)?;
            common.repo = global_repo(&repo_arg, global);
            print!("{}", nexspec::query::api::query_graph(&engine, &question, options, &common)?);
        }
        Command::Path { from, to, filter, output } => {
            let engine = query_engine(&repo, &index_dir, output.global, Default::default())?;
            let mut common = output.common(None)?;
            common.repo = global_repo(&repo_arg, output.global);
            print!("{}", nexspec::query::api::find_path(&engine, &from, &to, &filter.build()?, &common)?);
        }
        Command::Explain { target, pick, filter, output } => {
            let engine = query_engine(&repo, &index_dir, output.global, Default::default())?;
            let mut common = output.common(pick)?;
            common.repo = global_repo(&repo_arg, output.global);
            print!("{}", nexspec::query::api::explain(&engine, &target, &filter.build()?, &common)?);
        }
        Command::Affected { target, depth, limit, pick, filter, output } => {
            let engine = query_engine(&repo, &index_dir, output.global, Default::default())?;
            let options = nexspec::query::affected::AffectedOptions { depth, max_per_hop: limit.max(1), filter: filter.build()? };
            let mut common = output.common(pick)?;
            common.repo = global_repo(&repo_arg, output.global);
            print!("{}", nexspec::query::api::affected(&engine, &target, options, &common)?);
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
            let BenchArgs { corpus, ks, index_dir, format, tokenizer, budget, output, no_vector, check, update_baseline, min_locate_recall, fixed_cost_files, compare_enrich } = *args;
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
            if compare_enrich {
                let state = nexspec::enrich::run::State::load(&repo);
                let spent = (state.runs > 0).then_some(state.input_tokens + state.output_tokens);
                let comparison = nexspec::bench::compare::compare(&corpus, &options, spent)?;
                let text = if format == "json" { serde_json::to_string_pretty(&comparison)? } else { nexspec::bench::compare::to_markdown(&comparison) };
                match output {
                    Some(path) => std::fs::write(path, text)?,
                    None => println!("{text}"),
                }
                return Ok(());
            }
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
        domain => domain.domain_label().map(|(kind, label)| format!("{kind} {label}")).unwrap_or_default(),
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

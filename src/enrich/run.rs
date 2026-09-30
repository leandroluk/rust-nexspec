//! `nexspec enrich` end to end (REQ-1901, REQ-1906, REQ-1909 in
//! `.specs/features/retrieval-enrichment/spec.md`): plan what to send, send it in
//! batches, persist after every batch and apply each batch to the index, so an
//! interruption loses nothing and repeats nothing.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, mpsc};

use serde::{Deserialize, Serialize};

use crate::engine::{Engine, EngineError};
use crate::enrich::cache::{CACHE_DIR, CacheError, EnrichmentCache, Entry, content_hash};
use crate::enrich::cost::Pricing;
use crate::enrich::provider::{BatchOutcome, EnrichProvider, FileRequest, PROMPT_VERSION, ProviderError, Usage, validate_languages};
use crate::enrich::select::{Candidate, DEFAULT_SNIPPET_CHARS, Top, eligible_files, find_secret, order_by_importance, snippet};
use crate::graph::edge::EdgeType;
use crate::graph::node::NodePayload;
use crate::report::GraphSnapshot;

pub const STATE_FILE: &str = "enrichment.state.json";

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("{0}")]
    Engine(#[from] EngineError),
    #[error("{0}")]
    Cache(#[from] CacheError),
    #[error("{0}")]
    Provider(#[from] ProviderError),
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone)]
pub struct EnrichOptions {
    pub langs: Vec<String>,
    pub batch: usize,
    pub concurrency: usize,
    pub top: Option<Top>,
    /// Stop dispatching batches once this many tokens (input + output, as reported) were used.
    pub token_budget: Option<u64>,
    pub snippet_chars: usize,
}

impl Default for EnrichOptions {
    fn default() -> Self {
        Self { langs: vec!["en".to_string()], batch: 8, concurrency: 4, top: None, token_budget: None, snippet_chars: DEFAULT_SNIPPET_CHARS }
    }
}

#[derive(Debug, Clone)]
pub struct PlannedFile {
    pub path: String,
    pub hash: String,
    pub snippet: String,
    /// Languages still to be written for this file.
    pub langs: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Plan {
    /// Files to send, most important first.
    pub files: Vec<PlannedFile>,
    /// Indexed files that are eligible at all.
    pub eligible: usize,
    /// Files in the `--top` slice that already have every requested language.
    pub fresh: usize,
    /// Files kept on the machine because the snippet looked like it holds a secret.
    pub omitted_secret: Vec<(String, &'static str)>,
}

#[derive(Debug, Clone, Default)]
pub struct RunReport {
    pub enriched_files: usize,
    pub failed: Vec<(String, String)>,
    pub usage: Usage,
    pub batches: usize,
    pub interrupted: bool,
    pub budget_reached: bool,
}

/// What survives between runs (besides the cache itself): for `--status` and the first-use check.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    pub runs: u64,
    pub model: String,
    pub langs: Vec<String>,
    pub last_run_at: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub failed: Vec<String>,
    pub omitted_secret: Vec<String>,
}

impl State {
    pub fn load(repo: &Path) -> Self {
        std::fs::read_to_string(repo.join(CACHE_DIR).join(STATE_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, repo: &Path) -> Result<(), RunError> {
        let path = repo.join(CACHE_DIR).join(STATE_FILE);
        let io = |source| RunError::Io { path: path.display().to_string(), source };
        std::fs::create_dir_all(path.parent().expect("has a parent")).map_err(io)?;
        std::fs::write(&path, serde_json::to_string_pretty(self).expect("serialises")).map_err(io)
    }

    /// Whether `enrich` ever sent anything from this repository (the first use asks for confirmation).
    pub fn used_before(&self) -> bool {
        self.runs > 0
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Importance inputs per indexed file, from the graph (REQ-1906).
fn candidates(snapshot: &GraphSnapshot, eligible: &BTreeSet<String>) -> Vec<Candidate> {
    let mut by_path: HashMap<String, Candidate> = eligible.iter().map(|p| (p.clone(), Candidate { path: p.clone(), degree: 0, requirement_links: 0, cochange: 0 })).collect();
    let file_path = |id: &crate::sync::mutation::StableId| snapshot.file_id_of(id).and_then(|f| snapshot.path_of(&f)).map(str::to_string);
    for edge in &snapshot.edges {
        let counts = matches!(edge.edge_type, EdgeType::Satisfies | EdgeType::Implements) || edge.edge_type.is_dependency();
        let is_satisfies = edge.edge_type == EdgeType::Satisfies;
        let is_cochange = edge.edge_type == EdgeType::CoChanges;
        if !(counts || is_cochange) {
            continue;
        }
        for (endpoint, other) in [(&edge.from, &edge.to), (&edge.to, &edge.from)] {
            let Some(path) = file_path(endpoint) else { continue };
            let Some(candidate) = by_path.get_mut(&path) else { continue };
            if is_cochange {
                candidate.cochange += 1;
            } else {
                candidate.degree += 1;
                if is_satisfies && matches!(snapshot.nodes.get(other), Some(NodePayload::Requirement { .. } | NodePayload::Task { .. })) {
                    candidate.requirement_links += 1;
                }
            }
        }
    }
    by_path.into_values().collect()
}

pub fn indexed_eligible(engine: &Engine, repo: &Path) -> Result<(BTreeSet<String>, GraphSnapshot), RunError> {
    let snapshot = engine.snapshot()?;
    let indexed: BTreeSet<&str> = snapshot.nodes.values().filter_map(|n| if let NodePayload::File { path, .. } = n { Some(path.as_str()) } else { None }).collect();
    let eligible: BTreeSet<String> = eligible_files(repo).into_iter().filter(|p| indexed.contains(p.as_str())).collect();
    Ok((eligible, snapshot))
}

pub fn plan(engine: &Engine, repo: &Path, cache: &EnrichmentCache, options: &EnrichOptions, model: &str) -> Result<Plan, RunError> {
    validate_languages(&options.langs)?;
    let (eligible, snapshot) = indexed_eligible(engine, repo)?;
    let ordered = order_by_importance(candidates(&snapshot, &eligible));
    let limit = options.top.map_or(ordered.len(), |t| t.limit(ordered.len()));
    let mut plan = Plan { eligible: eligible.len(), ..Plan::default() };
    for candidate in ordered.into_iter().take(limit) {
        let Ok(bytes) = std::fs::read(repo.join(&candidate.path)) else { continue };
        let hash = content_hash(&bytes);
        let langs: Vec<String> = options.langs.iter().filter(|l| !cache.is_fresh(&candidate.path, l, &hash, model, PROMPT_VERSION)).cloned().collect();
        if langs.is_empty() {
            plan.fresh += 1;
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        let cut = snippet(&text, options.snippet_chars);
        if let Some(kind) = find_secret(cut) {
            plan.omitted_secret.push((candidate.path, kind));
            continue;
        }
        plan.files.push(PlannedFile { path: candidate.path, hash, snippet: cut.to_string(), langs });
    }
    Ok(plan)
}

/// Batches keep the importance order; a batch only holds files that need the same languages.
fn make_batches(files: &[PlannedFile], size: usize) -> Vec<Vec<PlannedFile>> {
    let size = size.max(1);
    let mut open: BTreeMap<Vec<String>, Vec<PlannedFile>> = BTreeMap::new();
    let mut batches = Vec::new();
    for file in files {
        let group = open.entry(file.langs.clone()).or_default();
        group.push(file.clone());
        if group.len() == size {
            batches.push(std::mem::take(group));
        }
    }
    batches.extend(open.into_values().filter(|g| !g.is_empty()));
    batches
}

type Finished = (usize, Result<BatchOutcome, ProviderError>);

pub fn run(
    engine: &Engine,
    repo: &Path,
    provider: &dyn EnrichProvider,
    options: &EnrichOptions,
    plan: Plan,
    stop: &AtomicBool,
    mut on_batch: impl FnMut(&RunReport, usize),
) -> Result<RunReport, RunError> {
    let mut cache = EnrichmentCache::load(repo)?;
    let (eligible, _) = indexed_eligible(engine, repo)?;
    if cache.retain_paths(&eligible) > 0 {
        cache.save(repo)?;
    }
    let batches = make_batches(&plan.files, options.batch);
    let total_batches = batches.len();
    let next = AtomicUsize::new(0);
    let spent = AtomicU64::new(0);
    let budget_reached = AtomicBool::new(false);
    let (tx, rx) = mpsc::channel::<Finished>();
    let queue = Mutex::new(&batches);
    let mut report = RunReport::default();
    let mut outcome: Result<(), RunError> = Ok(());

    std::thread::scope(|scope| {
        for _ in 0..options.concurrency.clamp(1, 16) {
            let tx = tx.clone();
            let (next, spent, budget_reached, queue) = (&next, &spent, &budget_reached, &queue);
            scope.spawn(move || {
                loop {
                    if stop.load(Ordering::SeqCst) {
                        return;
                    }
                    if options.token_budget.is_some_and(|b| spent.load(Ordering::SeqCst) >= b) {
                        budget_reached.store(true, Ordering::SeqCst);
                        return;
                    }
                    let index = next.fetch_add(1, Ordering::SeqCst);
                    let batch = { queue.lock().unwrap().get(index).cloned() };
                    let Some(batch) = batch else { return };
                    let requests: Vec<FileRequest> = batch.iter().map(|f| FileRequest { path: f.path.clone(), snippet: f.snippet.clone() }).collect();
                    let result = provider.summarize(&requests, &batch[0].langs);
                    if let Ok(done) = &result {
                        spent.fetch_add(done.usage.input_tokens + done.usage.output_tokens, Ordering::SeqCst);
                    }
                    if tx.send((index, result)).is_err() {
                        return;
                    }
                }
            });
        }
        drop(tx);

        for (index, result) in rx {
            let batch = &batches[index];
            report.batches += 1;
            match result {
                Ok(done) => {
                    report.usage += done.usage;
                    let at = unix_now();
                    let mut applied = Vec::new();
                    for item in &done.items {
                        let Some(planned) = batch.iter().find(|f| f.path == item.path) else { continue };
                        for (lang, text) in &item.summaries {
                            cache.upsert(Entry::new(&item.path, &planned.hash, lang, text, provider.model(), PROMPT_VERSION, at));
                        }
                        applied.push(item.path.clone());
                    }
                    report.failed.extend(done.failed);
                    report.enriched_files += applied.len();
                    if let Err(e) = cache.save(repo).map_err(RunError::from).and_then(|()| engine.apply_enrichment(&applied).map(|_| ()).map_err(RunError::from)) {
                        outcome = Err(e);
                        stop.store(true, Ordering::SeqCst);
                    }
                }
                Err(error) => {
                    // The code was not sent back and forth for nothing: say which files, never what was in them.
                    let reason = error.to_string();
                    report.failed.extend(batch.iter().map(|f| (f.path.clone(), reason.clone())));
                }
            }
            on_batch(&report, total_batches);
        }
    });

    outcome?;
    report.interrupted = stop.load(Ordering::SeqCst);
    report.budget_reached = budget_reached.load(Ordering::SeqCst);

    let mut state = State::load(repo);
    state.runs += 1;
    state.model = provider.model().to_string();
    state.langs = options.langs.clone();
    state.last_run_at = unix_now();
    state.input_tokens += report.usage.input_tokens;
    state.output_tokens += report.usage.output_tokens;
    state.cost_usd += Pricing::for_model(provider.model()).cost(report.usage);
    state.failed = report.failed.iter().map(|(p, _)| p.clone()).collect();
    state.omitted_secret = plan.omitted_secret.iter().map(|(p, kind)| format!("{p} ({kind})")).collect();
    state.save(repo)?;
    Ok(report)
}

#[derive(Debug, Clone, Default)]
pub struct Status {
    pub eligible: usize,
    /// Files with every requested language written for their current content.
    pub up_to_date: usize,
    /// Files with a cached summary whose content changed since.
    pub stale: usize,
    pub pending: usize,
    pub langs: Vec<String>,
    pub state: State,
    pub cache_entries: usize,
}

impl Status {
    /// Stable first line for tools (REQ-1913).
    pub fn first_line(&self) -> String {
        format!(
            "enrichment: {}/{} files up to date ({}), {} stale, {} pending",
            self.up_to_date,
            self.eligible,
            if self.langs.is_empty() { "-".to_string() } else { self.langs.join(",") },
            self.stale,
            self.pending
        )
    }
}

pub fn status(engine: &Engine, repo: &Path, langs: &[String]) -> Result<Status, RunError> {
    let cache = EnrichmentCache::load(repo)?;
    let (eligible, _) = indexed_eligible(engine, repo)?;
    let state = State::load(repo);
    let langs: Vec<String> = if langs.is_empty() { state.langs.clone() } else { langs.to_vec() };
    let mut status = Status { eligible: eligible.len(), langs: langs.clone(), cache_entries: cache.len(), state, ..Status::default() };
    for path in &eligible {
        let Ok(bytes) = std::fs::read(repo.join(path)) else { continue };
        let hash = content_hash(&bytes);
        let changed = cache.entries().any(|e| &e.path == path && e.content_hash != hash);
        let complete = !langs.is_empty() && langs.iter().all(|l| cache.get(path, l).is_some_and(|e| e.content_hash == hash));
        if complete {
            status.up_to_date += 1;
        } else if changed {
            status.stale += 1;
        } else {
            status.pending += 1;
        }
    }
    Ok(status)
}

pub fn clear(repo: &Path) -> Result<bool, RunError> {
    let removed = EnrichmentCache::clear(repo)?;
    let _ = std::fs::remove_file(repo.join(CACHE_DIR).join(STATE_FILE));
    Ok(removed)
}

pub fn cache_dir(repo: &Path) -> PathBuf {
    repo.join(CACHE_DIR)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn planned(path: &str, langs: &[&str]) -> PlannedFile {
        PlannedFile { path: path.into(), hash: "h".into(), snippet: "s".into(), langs: langs.iter().map(|l| l.to_string()).collect() }
    }

    #[test]
    fn batches_keep_importance_order_and_never_mix_language_sets() {
        let files = [planned("a", &["en"]), planned("b", &["en", "pt"]), planned("c", &["en"]), planned("d", &["en"]), planned("e", &["en", "pt"])];
        let batches = make_batches(&files, 2);
        let shape: Vec<Vec<&str>> = batches.iter().map(|b| b.iter().map(|f| f.path.as_str()).collect()).collect();
        assert_eq!(shape, [vec!["a", "c"], vec!["b", "e"], vec!["d"]]);
        assert!(batches.iter().all(|b| b.iter().all(|f| f.langs == b[0].langs)));
    }

    #[test]
    fn status_first_line_is_short_and_stable() {
        let status = Status { eligible: 10, up_to_date: 4, stale: 1, pending: 5, langs: vec!["en".into(), "pt".into()], ..Status::default() };
        assert_eq!(status.first_line(), "enrichment: 4/10 files up to date (en,pt), 1 stale, 5 pending");
    }
}

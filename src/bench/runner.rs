//! Benchmark runner (REQ-802, REQ-803): indexes the target repository into a
//! throwaway index, asks every corpus question through the same
//! [`Engine::search`] an agent uses, and measures quality, tokens and latency.
//!
//! The target repository is only read; the index lives in a temporary
//! directory unless one is given (`.specs/features/retrieval-benchmark/design.md` D2).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::Serialize;
use tempfile::TempDir;

use crate::bench::baselines::{BaselineError, RepoSnapshot};
use crate::bench::corpus::{Corpus, Kind};
use crate::bench::locate::ranked_for;
use crate::bench::metrics::{QueryMetrics, aggregate, aggregate_by_kind, evaluate};
use crate::engine::{Engine, EngineError};
use crate::git::{GitError, GitSource};
use crate::token::budget::{CharHeuristicTokenizer, TiktokenTokenizer, TokenError, Tokenizer};

#[derive(Debug, thiserror::Error)]
pub enum BenchError {
    #[error("engine error: {0}")]
    Engine(#[from] EngineError),
    #[error("git error: {0}")]
    Git(#[from] GitError),
    #[error("baseline error: {0}")]
    Baseline(#[from] BaselineError),
    #[error("tokenizer error: {0}")]
    Token(#[from] TokenError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TokenizerKind {
    /// `chars / 3.5`, offline and deterministic (the default).
    Heuristic,
    /// `cl100k_base` BPE.
    Tiktoken,
}

impl TokenizerKind {
    pub fn build(self) -> Result<Box<dyn Tokenizer>, TokenError> {
        Ok(match self {
            TokenizerKind::Heuristic => Box::new(CharHeuristicTokenizer),
            TokenizerKind::Tiktoken => Box::new(TiktokenTokenizer::new()?),
        })
    }
}

#[derive(Debug, Clone)]
pub struct BenchOptions {
    pub repo: PathBuf,
    /// Where to build the index; a temporary directory when `None`.
    pub index_dir: Option<PathBuf>,
    pub ks: Vec<usize>,
    /// `max_tokens` given to `Engine::search` (what the agent would request).
    pub budget_tokens: u32,
    pub tokenizer: TokenizerKind,
}

impl BenchOptions {
    pub fn new(repo: impl Into<PathBuf>) -> Self {
        Self {
            repo: repo.into(),
            index_dir: None,
            ks: vec![5, 10],
            budget_tokens: 2000,
            tokenizer: TokenizerKind::Heuristic,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct QueryResult {
    pub id: String,
    pub kind: Kind,
    pub query: String,
    pub expect: Vec<String>,
    /// 1-based rank of each `expect` entry (`null` = not found).
    pub ranks: Vec<Option<usize>>,
    /// `k -> recall@k`.
    pub recall: BTreeMap<String, f64>,
    pub reciprocal_rank: f64,
    /// Tokens of the answer `search` produced within the budget.
    pub tokens_nexspec: u32,
    /// Tokens of `grep -rn <term>`.
    pub tokens_grep: u32,
    /// Tokens of reading the expected files whole (`null` = no file expected).
    pub tokens_read: Option<u32>,
    pub latency_ms: f64,
    /// First files returned, in order (for reading the report).
    pub top_files: Vec<String>,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct KindSummary {
    pub queries: usize,
    pub recall: BTreeMap<String, f64>,
    pub mrr: f64,
    /// Sum over the kind's queries. For `behavior` this includes
    /// the cost of then reading the expected files (REQ-803).
    pub tokens_nexspec: u64,
    pub tokens_grep: u64,
    pub tokens_read: Option<u64>,
    /// `1 - nexspec/grep`, weighted by tokens (negative = nexspec costs more).
    pub savings_vs_grep: f64,
    pub savings_vs_read: Option<f64>,
    pub savings_vs_corpus: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchReport {
    pub tool_version: String,
    pub repo: String,
    /// HEAD of the target when the benchmark ran.
    pub repo_commit: String,
    /// Commit the corpus says its expectations were written for, if any.
    pub corpus_commit: Option<String>,
    pub tokenizer: TokenizerKind,
    pub budget_tokens: u32,
    pub ks: Vec<usize>,
    /// Whether the vector half of hybrid search was available.
    pub vector_search: bool,
    pub index_seconds: f64,
    pub files_in_corpus: usize,
    /// Tokens of reading every tracked text file (the "whole corpus" baseline).
    pub corpus_tokens: u32,
    pub queries: Vec<QueryResult>,
    pub by_kind: BTreeMap<String, KindSummary>,
    pub overall: KindSummary,
    /// `corpus_tokens / mean nexspec tokens per query` (graphify benchmark's ratio).
    pub reduction_ratio: f64,
}

pub fn run(corpus: &Corpus, options: &BenchOptions) -> Result<BenchReport, BenchError> {
    let tokenizer = options.tokenizer.build()?;
    let repo = options.repo.canonicalize().unwrap_or_else(|_| options.repo.clone());

    // Keep the TempDir alive for the whole run.
    let temp: Option<TempDir> = match options.index_dir {
        Some(_) => None,
        None => Some(TempDir::new()?),
    };
    let index_dir: PathBuf = options
        .index_dir
        .clone()
        .unwrap_or_else(|| temp.as_ref().expect("temp dir exists when no index dir is given").path().to_path_buf());

    let started = Instant::now();
    let engine = Engine::open(&index_dir, &repo)?;
    engine.sync()?;
    let index_seconds = started.elapsed().as_secs_f64();

    let git = GitSource::open(&repo)?;
    let snapshot = RepoSnapshot::load(&git)?;
    let corpus_tokens = snapshot.corpus_tokens(tokenizer.as_ref());
    let repo_commit = hex_oid(&git.head_commit_oid()?);

    let mut results = Vec::with_capacity(corpus.queries.len());
    let mut per_query_metrics: Vec<(Kind, QueryMetrics)> = Vec::new();
    for query in &corpus.queries {
        let t = Instant::now();
        let result = engine.search(&query.query, Some(options.budget_tokens))?;
        let latency_ms = t.elapsed().as_secs_f64() * 1000.0;

        let ranked = ranked_for(&engine, &result)?;
        let metrics = evaluate(&query.expect, &ranked, &options.ks);
        let tokens_nexspec = result
            .markdown
            .as_deref()
            .map_or(0, |md| tokenizer.estimate(md));
        let tokens_grep = snapshot.grep_tokens(&query.grep, tokenizer.as_ref());
        let tokens_read = snapshot.read_tokens(&query.expect, tokenizer.as_ref())?;

        results.push(QueryResult {
            id: query.id.clone(),
            kind: query.kind,
            query: query.query.clone(),
            expect: query.expect.clone(),
            ranks: metrics.ranks.clone(),
            recall: metrics.recall.iter().map(|(k, r)| (k.to_string(), *r)).collect(),
            reciprocal_rank: metrics.reciprocal_rank,
            tokens_nexspec,
            tokens_grep,
            tokens_read,
            latency_ms,
            top_files: ranked.files.iter().take(10).cloned().collect(),
            notes: query.notes.clone(),
        });
        per_query_metrics.push((query.kind, metrics));
    }

    let by_kind_metrics = aggregate_by_kind(&per_query_metrics, &options.ks);
    let mut by_kind = BTreeMap::new();
    for (kind, agg) in &by_kind_metrics {
        let rows: Vec<&QueryResult> = results.iter().filter(|r| r.kind == *kind).collect();
        by_kind.insert(kind.as_str().to_string(), summarize(&rows, agg.recall.clone(), agg.mrr, corpus_tokens));
    }
    let overall_agg = aggregate(per_query_metrics.iter().map(|(_, m)| m), &options.ks);
    let all_rows: Vec<&QueryResult> = results.iter().collect();
    let overall = summarize(&all_rows, overall_agg.recall.clone(), overall_agg.mrr, corpus_tokens);

    let mean_tokens = if results.is_empty() {
        0.0
    } else {
        results.iter().map(|r| r.tokens_nexspec as f64).sum::<f64>() / results.len() as f64
    };
    let reduction_ratio = if mean_tokens > 0.0 { corpus_tokens as f64 / mean_tokens } else { 0.0 };

    Ok(BenchReport {
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        repo: repo.display().to_string(),
        repo_commit,
        corpus_commit: corpus.commit.clone(),
        tokenizer: options.tokenizer,
        budget_tokens: options.budget_tokens,
        ks: options.ks.clone(),
        vector_search: engine.vector_search_available(),
        index_seconds,
        files_in_corpus: snapshot.file_count(),
        corpus_tokens,
        queries: results,
        by_kind,
        overall,
        reduction_ratio,
    })
}

fn summarize(rows: &[&QueryResult], recall: Vec<(usize, f64)>, mrr: f64, corpus_tokens: u32) -> KindSummary {
    // `behavior` answers need the expected files read afterwards: the cost
    // of that follow-up reading is added, never omitted (REQ-803).
    let nexspec_cost = |r: &QueryResult| -> u64 {
        let extra = if r.kind == Kind::Behavior { r.tokens_read.unwrap_or(0) } else { 0 };
        r.tokens_nexspec as u64 + extra as u64
    };
    let tokens_nexspec: u64 = rows.iter().map(|r| nexspec_cost(r)).sum();
    let tokens_grep: u64 = rows.iter().map(|r| r.tokens_grep as u64).sum();
    let with_read: Vec<&&QueryResult> = rows.iter().filter(|r| r.tokens_read.is_some()).collect();
    let tokens_read = (!with_read.is_empty()).then(|| with_read.iter().map(|r| r.tokens_read.unwrap_or(0) as u64).sum());
    let nexspec_where_read: u64 = with_read.iter().map(|r| nexspec_cost(r)).sum();
    let corpus_total = corpus_tokens as u64 * rows.len() as u64;
    KindSummary {
        queries: rows.len(),
        recall: recall.into_iter().map(|(k, r)| (k.to_string(), r)).collect(),
        mrr,
        tokens_nexspec,
        tokens_grep,
        tokens_read,
        savings_vs_grep: savings(tokens_nexspec, tokens_grep),
        savings_vs_read: tokens_read.map(|t| savings(nexspec_where_read, t)),
        savings_vs_corpus: savings(tokens_nexspec, corpus_total),
    }
}

fn savings(nexspec: u64, baseline: u64) -> f64 {
    if baseline == 0 { 0.0 } else { 1.0 - nexspec as f64 / baseline as f64 }
}

fn hex_oid(oid: &[u8; 20]) -> String {
    oid.iter().map(|b| format!("{b:02x}")).collect()
}

/// `<repo>/.specs/bench/queries.toml`, the per-project corpus convention (design.md D9).
pub fn default_corpus_path(repo: &Path) -> PathBuf {
    repo.join(".specs").join("bench").join("queries.toml")
}

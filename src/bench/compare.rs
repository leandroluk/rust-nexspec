//! `bench --compare-enrich`: the same corpus without and with the `enrich`
//! summaries, side by side, judged against the acceptance criteria of
//! REQ-1911 in `.specs/features/retrieval-enrichment/spec.md`.

use std::fmt::Write as _;

use serde::Serialize;

use crate::bench::corpus::{Corpus, Kind};
use crate::bench::runner::{BenchError, BenchOptions, BenchReport, run};
use crate::enrich::cost::break_even_queries;

/// Prose questions (`behavior`) must gain at least this many recall@5 points.
pub const MIN_PROSE_RECALL_GAIN: f64 = 15.0;
/// `locate` MRR may drop at most this many points.
pub const MAX_LOCATE_MRR_DROP: f64 = 2.0;
/// Median search latency may grow at most this much (ratio).
pub const MAX_LATENCY_RATIO: f64 = 1.2;

#[derive(Debug, Clone, Serialize)]
pub struct Verdict {
    /// `None` when the corpus has no `behavior` questions.
    pub prose_recall5_gain: Option<f64>,
    pub locate_mrr_drop: Option<f64>,
    pub latency_ratio: f64,
    pub prose_ok: bool,
    pub locate_ok: bool,
    pub latency_ok: bool,
    /// All three hold: the summaries may keep a weight above zero.
    pub accepted: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Comparison {
    pub without: BenchReport,
    pub with: BenchReport,
    pub verdict: Verdict,
    /// Tokens the enrichment itself used (from `.specs/.cache/enrichment.state.json`), if any.
    pub enrichment_tokens: Option<u64>,
    /// Mean tokens saved per prose question (answer + reading the expected files).
    pub saving_per_prose_query: Option<f64>,
    /// Questions until the enrichment tokens are paid back.
    pub break_even_queries: Option<u64>,
}

fn recall5(report: &BenchReport, kind: Kind) -> Option<f64> {
    report.by_kind.get(kind.as_str())?.recall.get("5").copied()
}

fn mrr(report: &BenchReport, kind: Kind) -> Option<f64> {
    report.by_kind.get(kind.as_str()).map(|k| k.mrr)
}

fn median_latency(report: &BenchReport) -> f64 {
    let mut values: Vec<f64> = report.queries.iter().map(|q| q.latency_ms).collect();
    values.sort_by(f64::total_cmp);
    match values.len() {
        0 => 0.0,
        n => values[n / 2],
    }
}

fn prose_tokens_per_query(report: &BenchReport) -> Option<f64> {
    let kind = report.by_kind.get(Kind::Behavior.as_str())?;
    (kind.queries > 0).then(|| kind.tokens_nexspec as f64 / kind.queries as f64)
}

pub fn judge(without: &BenchReport, with: &BenchReport) -> Verdict {
    let prose_recall5_gain = recall5(without, Kind::Behavior).zip(recall5(with, Kind::Behavior)).map(|(a, b)| (b - a) * 100.0);
    let locate_mrr_drop = mrr(without, Kind::Locate).zip(mrr(with, Kind::Locate)).map(|(a, b)| (a - b) * 100.0);
    let latency_ratio = match median_latency(without) {
        base if base > 0.0 => median_latency(with) / base,
        _ => 1.0,
    };
    let prose_ok = prose_recall5_gain.is_some_and(|gain| gain >= MIN_PROSE_RECALL_GAIN);
    let locate_ok = locate_mrr_drop.is_none_or(|drop| drop <= MAX_LOCATE_MRR_DROP);
    let latency_ok = latency_ratio <= MAX_LATENCY_RATIO;
    Verdict { prose_recall5_gain, locate_mrr_drop, latency_ratio, prose_ok, locate_ok, latency_ok, accepted: prose_ok && locate_ok && latency_ok }
}

pub fn compare(corpus: &Corpus, options: &BenchOptions, enrichment_tokens: Option<u64>) -> Result<Comparison, BenchError> {
    let mut off = options.clone();
    off.summary_weight = Some(0.0);
    off.index_dir = None;
    let without = run(corpus, &off)?;
    let mut on = options.clone();
    on.index_dir = None;
    let with = run(corpus, &on)?;

    let verdict = judge(&without, &with);
    let saving = prose_tokens_per_query(&without).zip(prose_tokens_per_query(&with)).map(|(a, b)| a - b);
    let break_even = enrichment_tokens.zip(saving).and_then(|(spent, saving)| break_even_queries(spent, saving));
    Ok(Comparison { without, with, verdict, enrichment_tokens, saving_per_prose_query: saving, break_even_queries: break_even })
}

fn pct(value: Option<f64>) -> String {
    value.map_or("-".to_string(), |v| format!("{:.0}%", v * 100.0))
}

pub fn to_markdown(c: &Comparison) -> String {
    let mut md = String::new();
    let _ = writeln!(md, "# nexspec enrichment comparison\n");
    let _ = writeln!(md, "Same corpus, same commit, with and without the `enrich` summaries.\n");
    let _ = writeln!(md, "| kind | queries | recall@5 without | recall@5 with | MRR without | MRR with |");
    let _ = writeln!(md, "| --- | ---: | ---: | ---: | ---: | ---: |");
    for kind in Kind::ALL {
        let Some(w) = c.without.by_kind.get(kind.as_str()) else { continue };
        let _ = writeln!(
            md,
            "| {} | {} | {} | {} | {:.2} | {:.2} |",
            kind.as_str(),
            w.queries,
            pct(recall5(&c.without, kind)),
            pct(recall5(&c.with, kind)),
            w.mrr,
            mrr(&c.with, kind).unwrap_or(0.0)
        );
    }
    let _ = writeln!(
        md,
        "\nMedian search latency: {:.1} ms without, {:.1} ms with (x{:.2}).\n",
        median_latency(&c.without),
        median_latency(&c.with),
        c.verdict.latency_ratio
    );
    let v = &c.verdict;
    let mark = |ok: bool| if ok { "pass" } else { "FAIL" };
    let _ = writeln!(md, "## Acceptance (REQ-1911)\n");
    let _ = writeln!(
        md,
        "- prose `recall@5` gain >= {MIN_PROSE_RECALL_GAIN:.0} points: {} ({})",
        v.prose_recall5_gain.map_or("no `behavior` questions in the corpus".to_string(), |g| format!("{g:+.1} points")),
        mark(v.prose_ok)
    );
    let _ = writeln!(
        md,
        "- `locate` MRR drop <= {MAX_LOCATE_MRR_DROP:.0} points: {} ({})",
        v.locate_mrr_drop.map_or("no `locate` questions".to_string(), |d| format!("{d:+.1} points")),
        mark(v.locate_ok)
    );
    let _ = writeln!(md, "- latency <= x{MAX_LATENCY_RATIO:.1}: x{:.2} ({})", v.latency_ratio, mark(v.latency_ok));
    let _ = writeln!(
        md,
        "\n**{}**",
        if v.accepted { "Accepted: the summaries may keep a weight above zero." } else { "Not accepted: keep NEXSPEC_ENRICH_WEIGHT at 0; the feature stays experimental." }
    );
    if let Some(tokens) = c.enrichment_tokens {
        let _ = write!(md, "\nEnrichment used {tokens} tokens");
        match (c.saving_per_prose_query, c.break_even_queries) {
            (Some(saving), Some(n)) => {
                let _ = writeln!(md, "; a prose question saves ~{saving:.0} tokens, so it pays back after ~{n} questions.");
            }
            (Some(saving), None) => {
                let _ = writeln!(md, "; a prose question saves {saving:.0} tokens, which never pays it back.");
            }
            _ => {
                let _ = writeln!(md, "; there is no prose question to measure the saving.");
            }
        }
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bench::runner::{KindSummary, QueryResult};
    use crate::bench::runner::TokenizerKind;
    use std::collections::BTreeMap;

    fn kind_summary(queries: usize, recall5: f64, mrr: f64, tokens: u64) -> KindSummary {
        KindSummary {
            queries,
            recall: BTreeMap::from([("5".to_string(), recall5)]),
            mrr,
            tokens_nexspec: tokens,
            tokens_grep: 0,
            tokens_read: None,
            savings_vs_grep: 0.0,
            savings_vs_read: None,
            savings_vs_corpus: 0.0,
        }
    }

    fn report(behavior_recall: f64, locate_mrr: f64, latency_ms: f64, behavior_tokens: u64) -> BenchReport {
        let q = |latency_ms: f64| QueryResult {
            id: "q".into(),
            kind: Kind::Locate,
            query: String::new(),
            expect: vec![],
            ranks: vec![],
            recall: BTreeMap::new(),
            reciprocal_rank: 0.0,
            tokens_nexspec: 0,
            tokens_grep: 0,
            tokens_read: None,
            latency_ms,
            top_files: vec![],
            notes: String::new(),
        };
        let overall = kind_summary(0, 0.0, 0.0, 0);
        BenchReport {
            tool_version: String::new(),
            repo: String::new(),
            repo_commit: "abc".into(),
            corpus_commit: None,
            tokenizer: TokenizerKind::Heuristic,
            budget_tokens: 2000,
            ks: vec![5],
            vector_search: false,
            index_seconds: 0.0,
            files_in_corpus: 0,
            corpus_tokens: 0,
            queries: vec![q(latency_ms), q(latency_ms), q(latency_ms)],
            by_kind: BTreeMap::from([
                ("behavior".to_string(), kind_summary(4, behavior_recall, 0.5, behavior_tokens)),
                ("locate".to_string(), kind_summary(10, 1.0, locate_mrr, 0)),
            ]),
            overall,
            reduction_ratio: 0.0,
            avg_query_tokens: 0.0,
            fixed_cost: None,
            break_even: None,
        }
    }

    #[test]
    fn a_big_prose_gain_with_no_regression_is_accepted() {
        let verdict = judge(&report(0.25, 0.87, 10.0, 8000), &report(0.75, 0.86, 11.0, 4000));
        assert!(verdict.prose_ok && verdict.locate_ok && verdict.latency_ok && verdict.accepted, "{verdict:?}");
        assert!((verdict.prose_recall5_gain.unwrap() - 50.0).abs() < 1e-9);
    }

    #[test]
    fn each_criterion_can_reject_on_its_own() {
        assert!(!judge(&report(0.25, 0.87, 10.0, 0), &report(0.35, 0.87, 10.0, 0)).accepted, "gain below 15 points");
        assert!(!judge(&report(0.25, 0.87, 10.0, 0), &report(0.75, 0.80, 10.0, 0)).accepted, "locate MRR fell 7 points");
        assert!(!judge(&report(0.25, 0.87, 10.0, 0), &report(0.75, 0.87, 13.0, 0)).accepted, "30% slower");
    }

    #[test]
    fn the_markdown_states_the_verdict_and_the_payback() {
        let (without, with) = (report(0.25, 0.87, 10.0, 8000), report(0.75, 0.87, 10.0, 4000));
        let verdict = judge(&without, &with);
        let comparison = Comparison { without, with, verdict, enrichment_tokens: Some(40_000), saving_per_prose_query: Some(1000.0), break_even_queries: Some(40) };
        let md = to_markdown(&comparison);
        assert!(md.contains("Accepted") && md.contains("+50.0 points") && md.contains("pays back after ~40 questions"), "{md}");
    }
}

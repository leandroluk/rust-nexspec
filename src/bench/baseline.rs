//! Recorded baseline and regression gate (REQ-805 in
//! `.specs/features/retrieval-benchmark/spec.md`).
//!
//! `bench/baseline.json` keeps the quality of the last accepted run.
//! `--check` fails when (a) `locate` recall@5 is below an absolute floor
//! (default 0.8) or (b) any recorded recall dropped by more than
//! [`MAX_REGRESSION`] against the baseline. Latency and token counts are
//! informative only: they are too noisy (latency) or proxy-based (tokens) to
//! gate a build.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::bench::runner::BenchReport;

/// Largest tolerated drop of any recall value (absolute, 0.05 = 5 points).
pub const MAX_REGRESSION: f64 = 0.05;
/// A kind with fewer queries than this is tracked, not gated.
pub const MIN_QUERIES_TO_GATE: usize = 5;
/// Default absolute floor for `locate` recall@5.
pub const DEFAULT_MIN_LOCATE_RECALL: f64 = 0.8;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KindBaseline {
    pub queries: usize,
    /// `k -> recall@k`.
    pub recall: BTreeMap<String, f64>,
    pub mrr: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Baseline {
    pub schema: u32,
    /// Commit of the target when this baseline was recorded.
    pub repo_commit: String,
    /// Whether vectors were on: results are only comparable when this matches.
    pub vector_search: bool,
    pub ks: Vec<usize>,
    pub by_kind: BTreeMap<String, KindBaseline>,
}

#[derive(Debug, thiserror::Error)]
pub enum BaselineFileError {
    #[error("cannot read baseline {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid baseline JSON in {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: serde_json::Error,
    },
}

impl Baseline {
    pub fn from_report(report: &BenchReport) -> Self {
        Baseline {
            schema: 1,
            repo_commit: report.repo_commit.clone(),
            vector_search: report.vector_search,
            ks: report.ks.clone(),
            by_kind: report
                .by_kind
                .iter()
                .map(|(kind, s)| (kind.clone(), KindBaseline { queries: s.queries, recall: s.recall.clone(), mrr: s.mrr }))
                .collect(),
        }
    }

    pub fn load(path: &Path) -> Result<Self, BaselineFileError> {
        let text = std::fs::read_to_string(path).map_err(|source| BaselineFileError::Io { path: path.display().to_string(), source })?;
        serde_json::from_str(&text).map_err(|source| BaselineFileError::Parse { path: path.display().to_string(), source })
    }

    pub fn save(&self, path: &Path) -> Result<(), BaselineFileError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|source| BaselineFileError::Io { path: path.display().to_string(), source })?;
        }
        let text = serde_json::to_string_pretty(self).expect("Baseline always serializes") + "\n";
        std::fs::write(path, text).map_err(|source| BaselineFileError::Io { path: path.display().to_string(), source })
    }
}

/// Outcome of comparing a report with a baseline.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CheckOutcome {
    /// Each entry is a human-readable reason the gate failed.
    pub failures: Vec<String>,
    /// Non-fatal remarks (e.g. the baseline is not comparable).
    pub warnings: Vec<String>,
}

impl CheckOutcome {
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Apply the gate. `baseline` may be `None` (absolute floor only).
pub fn check(report: &BenchReport, baseline: Option<&Baseline>, min_locate_recall: f64) -> CheckOutcome {
    let mut outcome = CheckOutcome::default();

    match report.by_kind.get("locate").and_then(|s| s.recall.get("5")) {
        Some(&recall) if recall + 1e-9 < min_locate_recall => outcome.failures.push(format!(
            "locate recall@5 is {recall:.2}, below the required {min_locate_recall:.2}"
        )),
        Some(_) => {}
        None => outcome
            .warnings
            .push("no `locate` queries with recall@5 in this run: the absolute floor was not checked".to_string()),
    }

    let Some(baseline) = baseline else { return outcome };
    if baseline.vector_search != report.vector_search {
        outcome.warnings.push(format!(
            "baseline was recorded with vector search {} but this run has it {}: regression comparison skipped",
            if baseline.vector_search { "on" } else { "off" },
            if report.vector_search { "on" } else { "off" }
        ));
        return outcome;
    }
    for (kind, recorded) in &baseline.by_kind {
        let Some(current) = report.by_kind.get(kind) else {
            outcome.warnings.push(format!("kind `{kind}` is in the baseline but has no queries now"));
            continue;
        };
        for (k, &before) in &recorded.recall {
            let Some(&now) = current.recall.get(k) else { continue };
            let drop = before - now;
            if drop > MAX_REGRESSION + 1e-9 && recorded.queries < MIN_QUERIES_TO_GATE {
                // Two queries move recall in steps of 25 points: drift in the repository alone would trip the gate.
                outcome.warnings.push(format!("{kind} recall@{k} fell from {before:.2} to {now:.2}, not gated: only {} queries", recorded.queries));
            } else if drop > MAX_REGRESSION + 1e-9 {
                outcome.failures.push(format!(
                    "{kind} recall@{k} fell from {before:.2} to {now:.2} ({:+.0} points, limit -{:.0})",
                    (now - before) * 100.0,
                    MAX_REGRESSION * 100.0
                ));
            }
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bench::runner::{KindSummary, TokenizerKind};

    fn summary(recall5: f64, recall10: f64) -> KindSummary {
        KindSummary {
            queries: 10,
            recall: BTreeMap::from([("5".to_string(), recall5), ("10".to_string(), recall10)]),
            mrr: 0.5,
            tokens_nexspec: 0,
            tokens_grep: 0,
            tokens_read: None,
            savings_vs_grep: 0.0,
            savings_vs_read: None,
            savings_vs_corpus: 0.0,
        }
    }

    fn report(locate5: f64, structure5: f64) -> BenchReport {
        let by_kind = BTreeMap::from([
            ("locate".to_string(), summary(locate5, locate5)),
            ("structure".to_string(), summary(structure5, structure5)),
        ]);
        BenchReport {
            tool_version: "test".into(),
            repo: "r".into(),
            repo_commit: "abc".into(),
            corpus_commit: None,
            tokenizer: TokenizerKind::Heuristic,
            budget_tokens: 2000,
            ks: vec![5, 10],
            vector_search: false,
            index_seconds: 0.0,
            files_in_corpus: 0,
            corpus_tokens: 0,
            queries: vec![],
            overall: summary(0.0, 0.0),
            by_kind,
            reduction_ratio: 0.0,
            avg_query_tokens: 0.0,
            fixed_cost: None,
            break_even: None,
        }
    }

    #[test]
    fn absolute_floor_fails_below_and_passes_at_the_limit() {
        assert!(!check(&report(0.79, 1.0), None, 0.8).passed());
        assert!(check(&report(0.80, 1.0), None, 0.8).passed());
        let failed = check(&report(0.5, 1.0), None, 0.8);
        assert!(failed.failures[0].contains("locate recall@5 is 0.50"), "{failed:?}");
    }

    #[test]
    fn a_kind_with_very_few_queries_is_tracked_not_gated() {
        let mut few = report(0.9, 0.9);
        few.by_kind.get_mut("structure").unwrap().queries = 2;
        let baseline = Baseline::from_report(&few);
        let outcome = check(&report(0.90, 0.40), Some(&baseline), 0.0);
        assert!(outcome.passed(), "{outcome:?}");
        assert!(outcome.warnings.iter().any(|w| w.contains("not gated: only 2 queries")), "{outcome:?}");
    }

    #[test]
    fn regression_beyond_five_points_fails_and_names_the_delta() {
        let baseline = Baseline::from_report(&report(0.9, 0.9));
        let ok = check(&report(0.86, 0.9), Some(&baseline), 0.0);
        assert!(ok.passed(), "a 4 point drop is tolerated: {ok:?}");

        let bad = check(&report(0.90, 0.80), Some(&baseline), 0.0);
        assert!(!bad.passed());
        assert!(bad.failures.iter().any(|f| f.contains("structure recall@5 fell from 0.90 to 0.80")), "{bad:?}");
    }

    #[test]
    fn improvements_never_fail() {
        let baseline = Baseline::from_report(&report(0.5, 0.5));
        assert!(check(&report(0.9, 0.9), Some(&baseline), 0.8).passed());
    }

    #[test]
    fn baseline_with_different_vector_setting_is_not_compared() {
        let mut baseline = Baseline::from_report(&report(1.0, 1.0));
        baseline.vector_search = true;
        let outcome = check(&report(0.3, 0.3), Some(&baseline), 0.0);
        assert!(outcome.passed());
        assert!(outcome.warnings.iter().any(|w| w.contains("regression comparison skipped")), "{outcome:?}");
    }

    #[test]
    fn baseline_roundtrips_through_a_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("nested").join("baseline.json");
        let baseline = Baseline::from_report(&report(0.75, 0.25));
        baseline.save(&path).unwrap();
        assert_eq!(Baseline::load(&path).unwrap(), baseline);
        assert!(matches!(Baseline::load(&dir.path().join("missing.json")), Err(BaselineFileError::Io { .. })));
    }
}

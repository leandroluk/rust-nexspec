//! Retrieval-quality metrics (REQ-802): `recall@k` and `MRR`, computed from
//! what a search returned versus what the corpus expects. Pure functions, no
//! I/O, so they are easy to test by hand.
//!
//! A search result is reduced to three de-duplicated rankings, each in order
//! of first appearance (what an agent would read first): files, symbols and
//! requirement/task/ADR markers. An `expect` entry is matched against the
//! ranking of its own kind.

use std::collections::BTreeMap;

use crate::bench::corpus::Kind;

/// One search hit reduced to what can be compared with an expectation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Location {
    pub path: Option<String>,
    pub symbol: Option<String>,
    pub marker: Option<String>,
}

/// De-duplicated rankings, first appearance wins.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ranked {
    pub files: Vec<String>,
    pub symbols: Vec<String>,
    pub markers: Vec<String>,
}

impl Ranked {
    pub fn from_locations(locations: &[Location]) -> Self {
        fn push_unique(list: &mut Vec<String>, value: &Option<String>) {
            if let Some(v) = value
                && !list.contains(v)
            {
                list.push(v.clone());
            }
        }
        let mut ranked = Ranked::default();
        for location in locations {
            push_unique(&mut ranked.files, &location.path.as_ref().map(|p| normalize(p)));
            push_unique(&mut ranked.symbols, &location.symbol);
            push_unique(&mut ranked.markers, &location.marker);
        }
        ranked
    }
}

fn normalize(path: &str) -> String {
    path.replace('\\', "/")
}

/// What one `expect` entry asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expectation {
    /// A file: matches any ranked file whose path ends with it.
    Path(String),
    /// `REQ-…`, `TASK-…`, `ADR-…`.
    Marker(String),
    /// A symbol name.
    Symbol(String),
}

impl Expectation {
    /// `REQ-`/`TASK-`/`ADR-` prefix → marker; contains `/` or `.` → path;
    /// anything else → symbol name.
    pub fn parse(entry: &str) -> Self {
        let entry = entry.trim();
        let is_marker = ["REQ-", "TASK-", "ADR-"].iter().any(|prefix| {
            entry
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.chars().next().is_some_and(|c| c.is_ascii_alphanumeric()))
        });
        if is_marker {
            Expectation::Marker(entry.to_string())
        } else if entry.contains('/') || entry.contains('\\') || entry.contains('.') {
            Expectation::Path(normalize(entry))
        } else {
            Expectation::Symbol(entry.to_string())
        }
    }

    /// 1-based rank of the first match, or `None` when it never shows up.
    pub fn rank(&self, ranked: &Ranked) -> Option<usize> {
        let position = match self {
            Expectation::Path(expected) => {
                let expected = expected.trim_start_matches("./");
                ranked.files.iter().position(|f| f == expected || f.ends_with(&format!("/{expected}")))
            }
            Expectation::Marker(m) => ranked.markers.iter().position(|x| x == m),
            Expectation::Symbol(s) => ranked.symbols.iter().position(|x| x == s),
        };
        position.map(|p| p + 1)
    }
}

/// Metrics for one query.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryMetrics {
    /// `(k, recall@k)` in the order `ks` was given.
    pub recall: Vec<(usize, f64)>,
    /// Reciprocal rank of the first relevant result (0 when none).
    pub reciprocal_rank: f64,
    /// 1-based rank of each expectation, in `expect` order.
    pub ranks: Vec<Option<usize>>,
}

pub fn evaluate(expect: &[String], ranked: &Ranked, ks: &[usize]) -> QueryMetrics {
    let ranks: Vec<Option<usize>> = expect.iter().map(|e| Expectation::parse(e).rank(ranked)).collect();
    let total = expect.len().max(1) as f64;
    let recall = ks
        .iter()
        .map(|&k| {
            let found = ranks.iter().flatten().filter(|&&r| r <= k).count();
            (k, found as f64 / total)
        })
        .collect();
    let reciprocal_rank = ranks.iter().flatten().min().map_or(0.0, |&best| 1.0 / best as f64);
    QueryMetrics { recall, reciprocal_rank, ranks }
}

/// Mean metrics over a group of queries (one `Kind`, or all).
#[derive(Debug, Clone, PartialEq)]
pub struct Aggregate {
    pub queries: usize,
    pub recall: Vec<(usize, f64)>,
    pub mrr: f64,
}

pub fn aggregate<'a>(metrics: impl IntoIterator<Item = &'a QueryMetrics>, ks: &[usize]) -> Aggregate {
    let all: Vec<&QueryMetrics> = metrics.into_iter().collect();
    let n = all.len();
    let mean = |f: &dyn Fn(&QueryMetrics) -> f64| if n == 0 { 0.0 } else { all.iter().map(|m| f(m)).sum::<f64>() / n as f64 };
    Aggregate {
        queries: n,
        recall: ks
            .iter()
            .enumerate()
            .map(|(i, &k)| (k, mean(&|m| m.recall[i].1)))
            .collect(),
        mrr: mean(&|m| m.reciprocal_rank),
    }
}

/// Aggregate per `Kind`, only for kinds that have at least one query.
pub fn aggregate_by_kind(items: &[(Kind, QueryMetrics)], ks: &[usize]) -> BTreeMap<Kind, Aggregate> {
    let mut groups: BTreeMap<Kind, Vec<&QueryMetrics>> = BTreeMap::new();
    for (kind, m) in items {
        groups.entry(*kind).or_default().push(m);
    }
    groups.into_iter().map(|(kind, ms)| (kind, aggregate(ms, ks))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc(path: &str, symbol: Option<&str>, marker: Option<&str>) -> Location {
        Location {
            path: Some(path.to_string()),
            symbol: symbol.map(str::to_string),
            marker: marker.map(str::to_string),
        }
    }

    fn ranked(paths: &[&str]) -> Ranked {
        Ranked::from_locations(&paths.iter().map(|p| loc(p, None, None)).collect::<Vec<_>>())
    }

    #[test]
    fn expectation_kinds_are_told_apart() {
        assert_eq!(Expectation::parse("REQ-021b"), Expectation::Marker("REQ-021b".into()));
        assert_eq!(Expectation::parse("TASK-CTR-001"), Expectation::Marker("TASK-CTR-001".into()));
        assert_eq!(Expectation::parse("src/a.ts"), Expectation::Path("src/a.ts".into()));
        assert_eq!(Expectation::parse("outbox.decorator.ts"), Expectation::Path("outbox.decorator.ts".into()));
        assert_eq!(Expectation::parse("OutboxDecorator"), Expectation::Symbol("OutboxDecorator".into()));
        assert_eq!(Expectation::parse("REQ-"), Expectation::Symbol("REQ-".into()), "bare prefix is not a marker");
    }

    #[test]
    fn path_matches_by_suffix_at_a_directory_boundary() {
        let r = ranked(&["a/b/outbox.decorator.ts", "xoutbox.decorator.ts"]);
        assert_eq!(Expectation::parse("outbox.decorator.ts").rank(&r), Some(1));
        assert_eq!(Expectation::parse("b/outbox.decorator.ts").rank(&r), Some(1));
        // "xoutbox…" must not satisfy a bare file name expectation on its own.
        let only_x = ranked(&["xoutbox.decorator.ts"]);
        assert_eq!(Expectation::parse("outbox.decorator.ts").rank(&only_x), None);
    }

    #[test]
    fn windows_separators_are_normalized() {
        let r = ranked(&["src\\outbox\\outbox.decorator.ts"]);
        assert_eq!(Expectation::parse("src/outbox/outbox.decorator.ts").rank(&r), Some(1));
    }

    #[test]
    fn hit_at_rank_three_counts_for_k5_but_not_k1() {
        let r = ranked(&["x.ts", "y.ts", "target.ts", "z.ts"]);
        let m = evaluate(&["target.ts".to_string()], &r, &[1, 3, 5]);
        assert_eq!(m.recall, vec![(1, 0.0), (3, 1.0), (5, 1.0)]);
        assert!((m.reciprocal_rank - 1.0 / 3.0).abs() < 1e-9);
        assert_eq!(m.ranks, vec![Some(3)]);
    }

    #[test]
    fn partial_hit_gives_fractional_recall_and_mrr_uses_the_best_rank() {
        let r = ranked(&["a.ts", "b.ts", "c.ts"]);
        let m = evaluate(&["b.ts".to_string(), "missing.ts".to_string()], &r, &[5]);
        assert_eq!(m.recall, vec![(5, 0.5)]);
        assert!((m.reciprocal_rank - 0.5).abs() < 1e-9);
        assert_eq!(m.ranks, vec![Some(2), None]);
    }

    #[test]
    fn no_hit_gives_zero_recall_and_zero_mrr() {
        let m = evaluate(&["nope.ts".to_string()], &ranked(&["a.ts"]), &[5, 10]);
        assert_eq!(m.recall, vec![(5, 0.0), (10, 0.0)]);
        assert_eq!(m.reciprocal_rank, 0.0);
    }

    #[test]
    fn duplicates_collapse_keeping_first_position() {
        let locations = vec![
            loc("a.ts", Some("A"), None),
            loc("b.ts", Some("B"), None),
            loc("a.ts", Some("A2"), Some("REQ-1")),
        ];
        let r = Ranked::from_locations(&locations);
        assert_eq!(r.files, vec!["a.ts", "b.ts"]);
        assert_eq!(r.symbols, vec!["A", "B", "A2"]);
        assert_eq!(r.markers, vec!["REQ-1"]);
        let m = evaluate(&["b.ts".to_string(), "REQ-1".to_string(), "A2".to_string()], &r, &[2]);
        assert_eq!(m.ranks, vec![Some(2), Some(1), Some(3)]);
        assert_eq!(m.recall, vec![(2, 2.0 / 3.0)]);
    }

    #[test]
    fn aggregate_averages_per_kind() {
        let hit = evaluate(&["a.ts".to_string()], &ranked(&["a.ts"]), &[5]);
        let miss = evaluate(&["z.ts".to_string()], &ranked(&["a.ts"]), &[5]);
        let items = vec![
            (Kind::Locate, hit.clone()),
            (Kind::Locate, miss.clone()),
            (Kind::Structure, hit),
        ];
        let by_kind = aggregate_by_kind(&items, &[5]);
        assert_eq!(by_kind[&Kind::Locate].queries, 2);
        assert_eq!(by_kind[&Kind::Locate].recall, vec![(5, 0.5)]);
        assert_eq!(by_kind[&Kind::Locate].mrr, 0.5);
        assert_eq!(by_kind[&Kind::Structure].recall, vec![(5, 1.0)]);
        assert!(!by_kind.contains_key(&Kind::Behavior));
        assert_eq!(aggregate(std::iter::empty(), &[5]).mrr, 0.0);
    }
}

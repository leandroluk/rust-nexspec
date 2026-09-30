//! Benchmark report rendering (REQ-802): JSON for machines, Markdown for
//! people. The Markdown always states what the numbers are *not* (REQ-807).

use std::fmt::Write;

use crate::bench::runner::{BenchReport, KindSummary, TokenizerKind};

pub fn to_json(report: &BenchReport) -> String {
    serde_json::to_string_pretty(report).expect("BenchReport always serializes")
}

fn pct(value: f64) -> String {
    format!("{:.0}%", value * 100.0)
}

fn recall_cells(summary: &KindSummary, ks: &[usize]) -> String {
    ks.iter()
        .map(|k| format!("{:.2}", summary.recall.get(&k.to_string()).copied().unwrap_or(0.0)))
        .collect::<Vec<_>>()
        .join(" | ")
}

fn opt_pct(value: Option<f64>) -> String {
    value.map_or_else(|| "-".to_string(), pct)
}

pub fn to_markdown(report: &BenchReport) -> String {
    let mut md = String::new();
    let _ = writeln!(md, "# nexspec retrieval benchmark\n");
    let _ = writeln!(md, "- repository: `{}` @ `{}`", report.repo, &report.repo_commit[..report.repo_commit.len().min(12)]);
    if let Some(expected) = &report.corpus_commit
        && !report.repo_commit.starts_with(expected.as_str())
    {
        let _ = writeln!(
            md,
            "- **warning:** the corpus was written for commit `{expected}` but the repository is at `{}`; expectations may have drifted",
            &report.repo_commit[..report.repo_commit.len().min(12)]
        );
    }
    let tokenizer = match report.tokenizer {
        TokenizerKind::Heuristic => "heuristic (chars/3.5)",
        TokenizerKind::Tiktoken => "tiktoken cl100k_base",
    };
    let _ = writeln!(
        md,
        "- answer budget: {} tokens, tokenizer: {tokenizer}, vector search: {}",
        report.budget_tokens,
        if report.vector_search { "on" } else { "off (BM25 only)" }
    );
    let _ = writeln!(
        md,
        "- indexed {} text files in {:.1}s; reading the whole corpus costs {} tokens\n",
        report.files_in_corpus, report.index_seconds, report.corpus_tokens
    );
    let _ = writeln!(
        md,
        "> Token counts are a local proxy (not provider billing). `locate` savings measure finding the place; \
         `behavior` questions also need the expected files read, and that cost is added to nexspec's. \
         Fixed per-session overhead is not included here.\n"
    );

    let k_headers: String = report.ks.iter().map(|k| format!("recall@{k}")).collect::<Vec<_>>().join(" | ");
    let k_rule: String = report.ks.iter().map(|_| "---:").collect::<Vec<_>>().join(" | ");
    let _ = writeln!(md, "## By kind\n");
    let _ = writeln!(
        md,
        "| kind | queries | {k_headers} | MRR | nexspec tokens | grep tokens | saving vs grep | saving vs reading files | saving vs whole corpus |"
    );
    let _ = writeln!(md, "|---|---:| {k_rule} | ---: | ---: | ---: | ---: | ---: | ---: |");
    let row = |md: &mut String, name: &str, s: &KindSummary| {
        let _ = writeln!(
            md,
            "| {name} | {} | {} | {:.2} | {} | {} | {} | {} | {} |",
            s.queries,
            recall_cells(s, &report.ks),
            s.mrr,
            s.tokens_nexspec,
            s.tokens_grep,
            pct(s.savings_vs_grep),
            opt_pct(s.savings_vs_read),
            pct(s.savings_vs_corpus)
        );
    };
    for (kind, summary) in &report.by_kind {
        row(&mut md, kind, summary);
    }
    row(&mut md, "**all**", &report.overall);
    let _ = writeln!(
        md,
        "\nReduction vs. reading the whole corpus per question: **{:.1}x** (same ratio `graphify benchmark` reports).\n",
        report.reduction_ratio
    );

    let _ = writeln!(md, "## Queries\n");
    let _ = writeln!(md, "| id | kind | ranks of expected | nexspec tok | grep tok | read tok | ms |");
    let _ = writeln!(md, "|---|---|---|---:|---:|---:|---:|");
    for q in &report.queries {
        let ranks = q
            .ranks
            .iter()
            .map(|r| r.map_or_else(|| "MISS".to_string(), |r| r.to_string()))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            md,
            "| {} | {} | {ranks} | {} | {} | {} | {:.0} |",
            q.id,
            q.kind.as_str(),
            q.tokens_nexspec,
            q.tokens_grep,
            q.tokens_read.map_or_else(|| "-".to_string(), |t| t.to_string()),
            q.latency_ms
        );
    }

    let misses: Vec<_> = report.queries.iter().filter(|q| q.ranks.iter().any(|r| r.is_none())).collect();
    if !misses.is_empty() {
        let _ = writeln!(md, "\n## Misses\n");
        for q in misses {
            let _ = writeln!(md, "- **{}** `{}`: expected {:?}; top files: {}", q.id, q.query, q.expect, q.top_files.join(", "));
        }
    }
    md
}

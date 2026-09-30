//! Tokens and money for `enrich`, with and without a provider (REQ-1908 in
//! `.specs/features/retrieval-enrichment/spec.md`).
//!
//! Before a run: an offline estimate from the local tokenizer heuristic (no
//! network, so `--dry-run` is safe anywhere). After: the provider's own usage
//! numbers, priced per million tokens. Prices change, so they come from the
//! environment with a small documented table as the default.

use crate::enrich::provider::Usage;
use crate::token::budget::{CharHeuristicTokenizer, Tokenizer};

/// Above this many estimated input tokens, `enrich` asks for `--yes`.
pub const CONFIRM_ABOVE_INPUT_TOKENS: u64 = 500_000;

const PROMPT_OVERHEAD_TOKENS: u64 = 220;
const PER_FILE_OVERHEAD_TOKENS: u64 = 18;
const OUTPUT_TOKENS_PER_SUMMARY: u64 = 60;
const OUTPUT_TOKENS_PER_FILE: u64 = 14;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pricing {
    /// USD per million input tokens.
    pub input_per_million: f64,
    /// USD per million output tokens.
    pub output_per_million: f64,
    /// `false` when neither the environment nor the built-in table knows the model.
    pub known: bool,
}

/// Reference prices (USD per 1M tokens) for common models; override with
/// `NEXSPEC_ENRICH_PRICE_IN` / `NEXSPEC_ENRICH_PRICE_OUT`.
const PRICE_TABLE: &[(&str, f64, f64)] = &[
    ("gemini-flash-lite-latest", 0.10, 0.40),
    ("gemini-2.5-flash-lite", 0.10, 0.40),
    ("gemini-2.5-flash", 0.30, 2.50),
];

impl Pricing {
    pub fn for_model(model: &str) -> Self {
        let env = |name: &str| std::env::var(name).ok().and_then(|v| v.trim().parse::<f64>().ok()).filter(|p| p.is_finite() && *p >= 0.0);
        Self::resolve(model, env("NEXSPEC_ENRICH_PRICE_IN"), env("NEXSPEC_ENRICH_PRICE_OUT"))
    }

    fn resolve(model: &str, price_in: Option<f64>, price_out: Option<f64>) -> Self {
        let table = PRICE_TABLE.iter().find(|(name, _, _)| *name == model);
        match (price_in, price_out, table) {
            (Some(i), Some(o), _) => Self { input_per_million: i, output_per_million: o, known: true },
            (i, o, Some((_, ti, to))) => Self { input_per_million: i.unwrap_or(*ti), output_per_million: o.unwrap_or(*to), known: true },
            // Unknown model: the cheapest row, flagged so the report says the figure is a guess.
            (i, o, None) => Self { input_per_million: i.unwrap_or(0.10), output_per_million: o.unwrap_or(0.40), known: false },
        }
    }

    pub fn cost(&self, usage: Usage) -> f64 {
        usage.input_tokens as f64 / 1e6 * self.input_per_million + usage.output_tokens as f64 / 1e6 * self.output_per_million
    }
}

pub fn format_usd(amount: f64) -> String {
    if amount < 0.01 { format!("${amount:.4}") } else { format!("${amount:.2}") }
}

/// A forecast for enriching the first `files` files of the importance order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    pub files: usize,
    pub chars: u64,
    pub usage: Usage,
    pub cost_usd: f64,
}

/// Forecasts sending `snippets` (already cut to what would be sent) in batches of
/// `batch` files, asking for `languages` summaries each.
pub fn estimate(snippets: &[&str], languages: usize, batch: usize, pricing: &Pricing) -> Estimate {
    let tokenizer = CharHeuristicTokenizer;
    let batches = snippets.len().div_ceil(batch.max(1)) as u64;
    let mut usage = Usage { input_tokens: batches * PROMPT_OVERHEAD_TOKENS, output_tokens: 0 };
    let mut chars = 0u64;
    for snippet in snippets {
        chars += snippet.chars().count() as u64;
        usage.input_tokens += u64::from(tokenizer.estimate(snippet)) + PER_FILE_OVERHEAD_TOKENS;
        usage.output_tokens += OUTPUT_TOKENS_PER_FILE + OUTPUT_TOKENS_PER_SUMMARY * languages as u64;
    }
    Estimate { files: snippets.len(), chars, usage, cost_usd: pricing.cost(usage) }
}

/// The 20 % / 50 % / 100 % points of the coverage curve, in importance order.
pub fn coverage_curve(snippets_by_importance: &[&str], languages: usize, batch: usize, pricing: &Pricing) -> Vec<(u8, Estimate)> {
    [20u8, 50, 100]
        .into_iter()
        .map(|percent| {
            let take = (snippets_by_importance.len() * usize::from(percent)).div_ceil(100).min(snippets_by_importance.len());
            (percent, estimate(&snippets_by_importance[..take], languages, batch, pricing))
        })
        .collect()
}

/// In how many queries the tokens spent on enrichment are paid back by the
/// average saving per query (from the benchmark). `None` without a saving.
pub fn break_even_queries(spent_tokens: u64, saving_tokens_per_query: f64) -> Option<u64> {
    (saving_tokens_per_query > 0.0).then(|| (spent_tokens as f64 / saving_tokens_per_query).ceil() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pricing_comes_from_the_environment_then_the_table_then_a_flagged_default() {
        let env = Pricing::resolve("anything", Some(1.0), Some(2.0));
        assert_eq!((env.input_per_million, env.output_per_million, env.known), (1.0, 2.0, true));
        let table = Pricing::resolve("gemini-2.5-flash", None, None);
        assert_eq!((table.input_per_million, table.known), (0.30, true));
        let partial = Pricing::resolve("gemini-2.5-flash", None, Some(9.0));
        assert_eq!((partial.input_per_million, partial.output_per_million), (0.30, 9.0));
        let unknown = Pricing::resolve("mystery-model", None, None);
        assert!(!unknown.known, "the report must say the price is a guess");
    }

    #[test]
    fn cost_is_tokens_times_price_per_million() {
        let pricing = Pricing { input_per_million: 0.10, output_per_million: 0.40, known: true };
        let cost = pricing.cost(Usage { input_tokens: 1_000_000, output_tokens: 500_000 });
        assert!((cost - 0.30).abs() < 1e-9);
        assert_eq!(format_usd(0.3), "$0.30");
        assert_eq!(format_usd(0.0042), "$0.0042");
    }

    #[test]
    fn the_estimate_grows_with_files_and_languages() {
        let pricing = Pricing { input_per_million: 0.10, output_per_million: 0.40, known: true };
        let code = "x".repeat(700);
        let snippets = vec![code.as_str(); 10];
        let one = estimate(&snippets, 1, 5, &pricing);
        let two = estimate(&snippets, 2, 5, &pricing);
        assert_eq!(one.files, 10);
        assert_eq!(one.chars, 7000);
        assert_eq!(one.usage.input_tokens, two.usage.input_tokens, "the code is sent once whatever the languages");
        assert!(two.usage.output_tokens > one.usage.output_tokens);
        assert!(estimate(&snippets, 1, 1, &pricing).usage.input_tokens > one.usage.input_tokens, "smaller batches repeat the instructions");
        assert!(one.cost_usd > 0.0);
    }

    #[test]
    fn the_curve_has_three_monotonic_points() {
        let pricing = Pricing { input_per_million: 0.10, output_per_million: 0.40, known: true };
        let snippets = vec!["fn main() {}"; 50];
        let curve = coverage_curve(&snippets, 1, 10, &pricing);
        assert_eq!(curve.iter().map(|(p, e)| (*p, e.files)).collect::<Vec<_>>(), [(20, 10), (50, 25), (100, 50)]);
        assert!(curve[0].1.cost_usd < curve[1].1.cost_usd && curve[1].1.cost_usd < curve[2].1.cost_usd);
    }

    #[test]
    fn break_even_is_spent_tokens_over_the_saving_per_query() {
        assert_eq!(break_even_queries(100_000, 500.0), Some(200));
        assert_eq!(break_even_queries(100_000, 0.0), None);
    }
}

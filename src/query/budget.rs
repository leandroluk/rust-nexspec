//! One token budget for every query output (REQ-1106): keep the lines that
//! fit, say how many were left out. Same estimator and 90% safety margin as
//! `search --max-tokens` and `report --max-tokens`.

use crate::token::budget::{CharHeuristicTokenizer, Tokenizer};

/// Fits `lines` into `budget_tokens` (90% margin). The first line (the
/// title) is always kept. `None` returns the lines untouched.
pub fn fit_lines(lines: Vec<String>, budget_tokens: Option<u32>) -> Vec<String> {
    let Some(budget) = budget_tokens else { return lines };
    let limit = (budget as f64 * 0.9) as u32;
    let tokenizer = CharHeuristicTokenizer;
    let footer_cost = tokenizer.estimate("… +999 more lines omitted to fit --max-tokens") + 1;
    let mut used = 0u32;
    let mut kept: Vec<String> = Vec::new();
    let total = lines.len();
    for (index, line) in lines.into_iter().enumerate() {
        let cost = tokenizer.estimate(&line) + 1; // the newline
        let is_title = index == 0;
        if !is_title && used + cost + footer_cost > limit && index + 1 < total {
            // Not the last line, and adding it would leave no room for the footer.
            let remaining = total - kept.len();
            kept.push(format!("… +{remaining} more lines omitted to fit --max-tokens"));
            return kept;
        }
        if !is_title && used + cost > limit {
            let remaining = total - kept.len();
            kept.push(format!("… +{remaining} more lines omitted to fit --max-tokens"));
            return kept;
        }
        used += cost;
        kept.push(line);
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(n: usize) -> Vec<String> {
        std::iter::once("# Title".to_string()).chain((0..n).map(|i| format!("- item number {i} with some text"))).collect()
    }

    #[test]
    fn no_budget_changes_nothing() {
        let input = lines(10);
        assert_eq!(fit_lines(input.clone(), None), input);
    }

    #[test]
    fn a_budget_keeps_the_title_and_a_prefix_and_counts_the_rest() {
        let out = fit_lines(lines(100), Some(120));
        assert_eq!(out[0], "# Title");
        assert!(out.len() > 2 && out.len() < 50, "{}", out.len());
        let last = out.last().unwrap();
        assert!(last.starts_with("… +") && last.contains("more lines omitted"), "{last}");
        let omitted: usize = last.trim_start_matches("… +").split(' ').next().unwrap().parse().unwrap();
        assert_eq!(out.len() - 1 + omitted, 101, "kept + omitted covers every line: {out:?}");
        let tokens = CharHeuristicTokenizer.estimate(&out.join("\n"));
        assert!(tokens <= 120, "{tokens}");
    }

    #[test]
    fn output_that_fits_is_returned_whole_without_a_footer() {
        let input = lines(3);
        assert_eq!(fit_lines(input.clone(), Some(5000)), input);
    }

    #[test]
    fn a_tiny_budget_still_returns_the_title() {
        let out = fit_lines(lines(5), Some(1));
        assert_eq!(out[0], "# Title");
    }
}

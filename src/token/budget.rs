//! [`Tokenizer`] trait + [`Budget`] (REQ-502, REQ-503, REQ-504 in
//! `.specs/features/token-budgeting/spec.md`).

#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("tokenizer unavailable: {0}")]
    Unavailable(String),
}

/// Estimates the token cost of a piece of text. Implementations may be
/// exact (BPE) or a cheap heuristic — callers only see a `u32` count.
pub trait Tokenizer {
    fn estimate(&self, text: &str) -> u32;
}

/// Offline fallback (REQ-502): `char_count / 3.5`, rounded up. No I/O, no
/// network, never fails — the estimate a caller reaches for when
/// [`TiktokenTokenizer::new`] can't load its BPE ranks.
#[derive(Debug, Default, Clone, Copy)]
pub struct CharHeuristicTokenizer;

impl Tokenizer for CharHeuristicTokenizer {
    fn estimate(&self, text: &str) -> u32 {
        (text.chars().count() as f32 / 3.5).ceil() as u32
    }
}

/// Default [`Tokenizer`] (REQ-502): real BPE estimation via `tiktoken-rs`'s
/// `cl100k_base` vocabulary. The ranks file is fetched from a public URL
/// and cached locally on first construction — [`TiktokenTokenizer::new`]
/// surfaces that failure as `Err` instead of panicking, so callers can fall
/// back to [`CharHeuristicTokenizer`] when offline.
pub struct TiktokenTokenizer {
    bpe: tiktoken_rs::CoreBPE,
}

impl TiktokenTokenizer {
    pub fn new() -> Result<Self, TokenError> {
        let bpe = tiktoken_rs::cl100k_base().map_err(|e| TokenError::Unavailable(e.to_string()))?;
        Ok(Self { bpe })
    }
}

impl Tokenizer for TiktokenTokenizer {
    fn estimate(&self, text: &str) -> u32 {
        self.bpe.encode_ordinary(text).len() as u32
    }
}

/// Priority tier for REQ-504's deterministic cutoff. Ordered
/// `Target < Seed < Dependency` — [`Budget::fit`] sorts ascending by tier
/// (stable sort, preserving relative order within a tier) before
/// accumulating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Target,
    Seed,
    Dependency,
}

/// One node's already-rendered text plus its priority tier, ready for
/// [`Budget::fit`]. `lang` is the fenced-code-block language
/// [`crate::token::serializer::serialize`] emits (REQ-505) — `None` for
/// non-code nodes (specs/ADRs/docs).
#[derive(Debug, Clone)]
pub struct TieredItem {
    pub tier: Tier,
    pub text: String,
    pub lang: Option<&'static str>,
}

impl TieredItem {
    pub fn new(tier: Tier, text: impl Into<String>) -> Self {
        Self {
            tier,
            text: text.into(),
            lang: None,
        }
    }

    pub fn with_lang(mut self, lang: &'static str) -> Self {
        self.lang = Some(lang);
        self
    }
}

/// A token budget with a safety margin (REQ-503): the *effective* limit
/// used internally is `max_tokens * margin`, never the raw value, absorbing
/// the gap between this crate's BPE estimate and the consuming model's real
/// tokenizer.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    max_tokens: u32,
    margin: f32,
}

impl Budget {
    /// `margin` must be in `(0.0, 1.0]`; anything else is a programmer
    /// error (a margin outside that range doesn't correspond to any
    /// sensible safety policy), so it panics rather than threading a
    /// `Result` through every call site for an input that's always a
    /// compile-time-known constant in practice.
    pub fn new(max_tokens: u32, margin: f32) -> Self {
        assert!(
            margin > 0.0 && margin <= 1.0,
            "margin must be in (0.0, 1.0], got {margin}"
        );
        Self { max_tokens, margin }
    }

    /// Default safety margin (REQ-503): 90% of `max_tokens`.
    pub fn with_default_margin(max_tokens: u32) -> Self {
        Self::new(max_tokens, 0.9)
    }

    pub fn effective_limit(&self) -> u32 {
        (self.max_tokens as f32 * self.margin).floor() as u32
    }

    /// REQ-504: sorts `items` by tier (stable), then accumulates in that
    /// order, stopping *before* any item that would push the running total
    /// past [`Budget::effective_limit`]. Items past the cutoff are dropped
    /// whole — an item is never partially included.
    pub fn fit(&self, mut items: Vec<TieredItem>, tokenizer: &impl Tokenizer) -> Vec<TieredItem> {
        items.sort_by_key(|item| item.tier);
        let limit = self.effective_limit();
        let mut used = 0u32;
        let mut fitted = Vec::with_capacity(items.len());
        for item in items {
            let cost = tokenizer.estimate(&item.text);
            if used.saturating_add(cost) > limit {
                break;
            }
            used += cost;
            fitted.push(item);
        }
        fitted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod char_heuristic {
        use super::*;

        #[test]
        fn empty_string_costs_zero_tokens() {
            assert_eq!(CharHeuristicTokenizer.estimate(""), 0);
        }

        #[test]
        fn seven_chars_rounds_up_to_two_tokens() {
            assert_eq!(CharHeuristicTokenizer.estimate("1234567"), 2);
        }

        #[test]
        fn counts_chars_not_bytes_for_multibyte_utf8() {
            // "café" is 4 chars but 5 bytes (é is 2 bytes in UTF-8).
            let text = "café";
            assert_eq!(text.chars().count(), 4);
            assert_eq!(CharHeuristicTokenizer.estimate(text), 2); // ceil(4/3.5)
        }
    }

    mod tiktoken {
        use super::*;

        #[test]
        fn new_never_panics_regardless_of_network_availability() {
            let _ = TiktokenTokenizer::new();
        }

        #[test]
        #[ignore = "needs network access to fetch cl100k_base ranks on first run"]
        fn real_inference_estimates_a_known_short_phrase() {
            let tokenizer = TiktokenTokenizer::new().expect("tiktoken ranks available");
            let count = tokenizer.estimate("hello world");
            assert!(count > 0 && count < 5, "unexpected token count: {count}");
        }
    }

    mod budget {
        use super::*;

        #[test]
        fn effective_limit_rounds_down() {
            let budget = Budget::new(100, 0.9);
            assert_eq!(budget.effective_limit(), 90);

            let budget = Budget::new(101, 0.9);
            assert_eq!(budget.effective_limit(), 90); // 90.9 floors to 90
        }

        #[test]
        fn default_margin_is_ninety_percent() {
            let budget = Budget::with_default_margin(1000);
            assert_eq!(budget.effective_limit(), 900);
        }

        #[test]
        #[should_panic(expected = "margin must be in (0.0, 1.0]")]
        fn margin_of_zero_is_rejected() {
            Budget::new(100, 0.0);
        }

        #[test]
        #[should_panic(expected = "margin must be in (0.0, 1.0]")]
        fn margin_above_one_is_rejected() {
            Budget::new(100, 1.5);
        }
    }

    mod fit {
        use super::*;

        #[test]
        fn drops_dependency_tier_when_budget_only_fits_target_and_seed() {
            let budget = Budget::new(3, 1.0); // effective_limit = 3
            let items = vec![
                TieredItem::new(Tier::Dependency, "dddd"), // 4 chars -> ceil(4/3.5)=2
                TieredItem::new(Tier::Target, "tt"),       // 2 chars -> ceil(2/3.5)=1
                TieredItem::new(Tier::Seed, "sss"),        // 3 chars -> ceil(3/3.5)=1
            ];
            let fitted = budget.fit(items, &CharHeuristicTokenizer);

            let tiers: Vec<Tier> = fitted.iter().map(|i| i.tier).collect();
            assert_eq!(tiers, vec![Tier::Target, Tier::Seed]);
        }

        #[test]
        fn an_item_larger_than_the_whole_budget_is_dropped_without_blocking_earlier_items() {
            let budget = Budget::new(2, 1.0); // effective_limit = 2
            let items = vec![
                TieredItem::new(Tier::Target, "ab"), // 2 chars -> ceil(2/3.5)=1, fits
                TieredItem::new(Tier::Seed, "a".repeat(50)), // way over budget alone
            ];
            let fitted = budget.fit(items, &CharHeuristicTokenizer);

            assert_eq!(fitted.len(), 1);
            assert_eq!(fitted[0].tier, Tier::Target);
        }
    }
}

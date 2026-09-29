//! High-density Markdown serializer (REQ-505 in
//! `.specs/features/token-budgeting/spec.md`).

use super::budget::{Tier, TieredItem};

fn tier_label(tier: Tier) -> &'static str {
    match tier {
        Tier::Target => "Target",
        Tier::Seed => "Seed",
        Tier::Dependency => "Dependency",
    }
}

/// Serializes `items` (already pruned and budget-fitted, in the order given)
/// as compact Markdown: one `### <tier>` heading per item followed by a
/// fenced code block. No per-item metadata beyond the tier label and the
/// code fence's language — optimized for tokens-per-bit of information, not
/// for long-form human reading.
pub fn serialize(items: &[TieredItem]) -> String {
    let mut out = String::new();
    for item in items {
        out.push_str("### ");
        out.push_str(tier_label(item.tier));
        out.push('\n');
        out.push_str("```");
        out.push_str(item.lang.unwrap_or(""));
        out.push('\n');
        out.push_str(&item.text);
        out.push('\n');
        out.push_str("```\n\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_one_section_per_item_in_input_order() {
        let items = vec![
            TieredItem::new(Tier::Target, "REQ-501: prune bodies").with_lang("markdown"),
            TieredItem::new(Tier::Seed, "fn hello() { ... }").with_lang("rust"),
            TieredItem::new(Tier::Dependency, "fn world() { ... }").with_lang("rust"),
        ];

        let output = serialize(&items);

        assert_eq!(
            output,
            "### Target\n```markdown\nREQ-501: prune bodies\n```\n\n\
             ### Seed\n```rust\nfn hello() { ... }\n```\n\n\
             ### Dependency\n```rust\nfn world() { ... }\n```\n\n"
        );
    }

    #[test]
    fn empty_input_produces_empty_output() {
        assert_eq!(serialize(&[]), "");
    }
}

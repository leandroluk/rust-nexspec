//! End-to-end integration test for Fase 5 (T-508): pruner → budget →
//! serializer, wired together the way a Fase 6 CLI/MCP caller would use
//! them. Uses `CharHeuristicTokenizer` to stay network-independent.

use nexspec::{Budget, CharHeuristicTokenizer, Language, TieredItem, Tier, prune_symbol, serialize};

#[test]
fn pruned_symbol_flows_through_budget_and_serializer() {
    let source = "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n";
    let pruned = prune_symbol(source, Language::Rust, 0, 2);
    assert_eq!(pruned, "fn add(a: i32, b: i32) -> i32 { ... }");

    let items = vec![
        TieredItem::new(Tier::Target, "REQ-501: prune AST signatures for dense LLM context.")
            .with_lang("markdown"),
        TieredItem::new(Tier::Seed, pruned.clone()).with_lang("rust"),
        // Deliberately oversized dependency to force REQ-504's cutoff.
        TieredItem::new(Tier::Dependency, "x".repeat(500)).with_lang("rust"),
    ];

    // Budget tight enough to fit Target + Seed but not the oversized Dependency.
    let budget = Budget::new(30, 1.0);
    let fitted = budget.fit(items, &CharHeuristicTokenizer);

    assert_eq!(fitted.len(), 2, "the oversized dependency must be cut, not truncated in");
    assert_eq!(fitted[0].tier, Tier::Target);
    assert_eq!(fitted[1].tier, Tier::Seed);
    assert_eq!(fitted[1].text, pruned);

    let markdown = serialize(&fitted);
    assert!(markdown.contains("### Target"));
    assert!(markdown.contains("### Seed"));
    assert!(!markdown.contains("### Dependency"));
    assert!(markdown.contains("fn add(a: i32, b: i32) -> i32 { ... }"));
    assert!(!markdown.contains("a + b"), "the pruned body must not resurface in the final payload");
}

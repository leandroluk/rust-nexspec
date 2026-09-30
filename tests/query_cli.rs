//! `query`, `path`, `explain` and `affected` end to end (T-1108, REQ-1101..1106).

mod fixtures;

use std::path::Path;
use std::process::{Command, Output};

use fixtures::FixtureRepo;

fn sync(repo: &Path) {
    let out = Command::new(env!("CARGO_BIN_EXE_nexspec")).arg("--repo").arg(repo).arg("sync").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

fn run(repo: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nexspec"))
        .arg("--repo")
        .arg(repo)
        .args(args)
        .output()
        .expect("run nexspec")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

fn workspace() -> FixtureRepo {
    let repo = fixtures::ts_workspace::build();
    sync(repo.path());
    repo
}

#[test]
fn affected_lists_dependents_grouped_by_file_and_honours_filters() {
    let repo = workspace();
    let out = run(repo.path(), &["affected", "CachePort"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("# Affected by `CachePort"), "{text}");
    assert!(text.contains("## src/app/service.ts") && text.contains("## src/cache/redis.adapter.ts"), "{text}");
    assert!(text.contains("`Service (src/app/service.ts)` (references)") && text.contains("(extends)"), "{text}");

    let only_extends = stdout(&run(repo.path(), &["affected", "CachePort", "--relation", "extends"]));
    assert!(only_extends.contains("RedisAdapter") && !only_extends.contains("Service ("), "{only_extends}");

    let strict = run(repo.path(), &["affected", "CachePort", "--context", "runtime"]);
    let strict_text = stdout(&strict);
    assert!(!strict_text.contains("RedisAdapter"), "the implements edge is type-only: {strict_text}");

    let json: serde_json::Value = serde_json::from_str(&stdout(&run(repo.path(), &["affected", "CachePort", "--format", "json"]))).unwrap();
    assert!(json["nodes"].as_array().unwrap().iter().any(|n| n["relation"] == "extends"));
}

#[test]
fn path_shows_each_hop_and_no_path_is_a_normal_answer() {
    let repo = workspace();
    let out = run(repo.path(), &["path", "helper", "CachePort"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("# Path from `helper (src/app/util.ts)` to `CachePort"), "{text}");
    assert!(text.contains("1. ") && text.contains("calls") && text.contains("references"), "{text}");

    // Nothing but `helper`'s own edges connects to an unrelated, isolated file.
    let isolated = FixtureRepo::init();
    isolated.write_file("src/a.ts", "export function alpha() {}\n");
    isolated.write_file("src/b.ts", "export function beta() {}\n");
    isolated.commit("feat: two unrelated files");
    sync(isolated.path());
    let none = run(isolated.path(), &["path", "alpha", "beta"]);
    assert!(none.status.success(), "no path is an answer, not an error: {}", stderr(&none));
    assert!(stdout(&none).contains("No path between"), "{}", stdout(&none));
}

#[test]
fn explain_describes_a_symbol_and_ambiguity_lists_candidates() {
    let repo = workspace();
    let out = run(repo.path(), &["explain", "Service"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("# Service (src/app/service.ts) (symbol)"), "{text}");
    assert!(text.contains("- Where: `src/app/service.ts` lines 6-"), "{text}");
    assert!(text.contains("## Signature") && text.contains("class Service"), "{text}");
    assert!(text.contains("## Depends on") && text.contains("instantiates"), "{text}");
    assert!(text.contains("## Depended on by"), "{text}");

    let clash = FixtureRepo::init();
    clash.write_file("src/a/handler.ts", "export function handler() {}\n");
    clash.write_file("src/b/handler.ts", "export function handler() {}\n");
    clash.commit("feat: same name twice");
    sync(clash.path());
    let ambiguous = run(clash.path(), &["explain", "handler"]);
    assert!(!ambiguous.status.success());
    let message = stderr(&ambiguous);
    assert!(message.contains("matches 2 nodes") && message.contains("1. handler (src/a/handler.ts)") && message.contains("2. handler (src/b/handler.ts)"), "{message}");

    let picked = run(clash.path(), &["explain", "handler", "--pick", "2"]);
    assert!(picked.status.success(), "{}", stderr(&picked));
    assert!(stdout(&picked).contains("src/b/handler.ts"), "{}", stdout(&picked));
    let qualified = run(clash.path(), &["explain", "a/handler.ts:handler"]);
    assert!(qualified.status.success() && stdout(&qualified).contains("src/a/handler.ts"), "{}", stderr(&qualified));
    let out_of_range = run(clash.path(), &["explain", "handler", "--pick", "9"]);
    assert!(stderr(&out_of_range).contains("out of range"));
}

#[test]
fn query_answers_from_seeds_with_relations_and_a_default_budget() {
    let repo = workspace();
    let out = run(repo.path(), &["query", "what uses CachePort"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("# Query: what uses CachePort"), "{text}");
    assert!(text.contains("## Starting points") && text.contains("CachePort"), "{text}");
    assert!(text.contains("## Connected") && text.contains("--"), "{text}");
    assert!(nexspec_tokens(&text) <= 2000, "default budget is 2000 tokens");

    let tiny = stdout(&run(repo.path(), &["query", "what uses CachePort", "--max-tokens", "60"]));
    assert!(tiny.len() < text.len() && tiny.contains("omitted to fit --max-tokens"), "{tiny}");

    let dfs = run(repo.path(), &["query", "what uses CachePort", "--dfs", "--depth", "2"]);
    assert!(dfs.status.success());
    let json: serde_json::Value = serde_json::from_str(&stdout(&run(repo.path(), &["query", "CachePort", "--format", "json"]))).unwrap();
    assert!(json["seeds"].as_array().unwrap().iter().any(|s| s.as_str().unwrap().starts_with("CachePort")));
}

fn nexspec_tokens(text: &str) -> usize {
    // Same estimator family as the CLI's (chars / 3.5), generously rounded up.
    (text.chars().count() as f64 / 3.5).ceil() as usize
}

#[test]
fn bad_flags_fail_readably_and_trace_accepts_a_budget() {
    let repo = workspace();
    let bad = run(repo.path(), &["affected", "CachePort", "--relation", "imoprts"]);
    assert!(!bad.status.success());
    assert!(stderr(&bad).contains("unknown relation") && stderr(&bad).contains("imports"), "{}", stderr(&bad));
    assert!(stderr(&run(repo.path(), &["path", "a", "b", "--min-confidence", "sure"])).contains("unknown confidence"));
    assert!(stderr(&run(repo.path(), &["explain", "Service", "--format", "xml"])).contains("unknown format"));
    assert!(stderr(&run(repo.path(), &["affected", "NoSuchThing"])).contains("no node matches"));

    let trace = run(repo.path(), &["trace", "CachePort", "--max-tokens", "80", "--depth", "2"]);
    assert!(trace.status.success(), "{}", stderr(&trace));
    let text = stdout(&trace);
    assert!(text.starts_with("# Trace of CachePort"), "{text}");
    assert!(nexspec_tokens(&text) <= 80, "{} tokens: {text}", nexspec_tokens(&text));
}

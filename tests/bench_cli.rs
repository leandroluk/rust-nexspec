//! `nexspec bench` end to end on a fixture repository (T-805).

mod fixtures;

use std::path::Path;
use std::process::Command;

use fixtures::FixtureRepo;

const CORPUS: &str = r#"
[[query]]
id = "locate-handler"
kind = "locate"
query = "handleInvoice"
grep = "handleInvoice"
expect = ["src/invoice/handler.ts"]

[[query]]
id = "trace-req"
kind = "traceability"
query = "invoices must be idempotent"
expect = ["REQ-580", "src/invoice/handler.ts"]

[[query]]
id = "behavior-tax"
kind = "behavior"
query = "computeTax"
expect = ["src/tax/tax.ts"]

[[query]]
id = "miss"
kind = "locate"
query = "nothingLikeThisExists"
expect = ["src/ghost/none.ts"]
"#;

fn fixture() -> FixtureRepo {
    let repo = FixtureRepo::init();
    repo.write_file(".specs/feat/spec.md", "## Requirements\n\n- REQ-580: invoices must be idempotent\n");
    repo.write_file(
        "src/invoice/handler.ts",
        "// @spec REQ-580\nexport function handleInvoice() {\n  return 1;\n}\n",
    );
    repo.write_file("src/tax/tax.ts", "export function computeTax(amount: number) {\n  return amount * 0.2;\n}\n");
    repo.write_file("src/ghost/none.ts", "export const none = 0;\n");
    repo.commit("feat: invoices");
    repo
}

fn bench(repo: &Path, corpus: &Path, extra: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_nexspec"))
        .arg("--repo")
        .arg(repo)
        .arg("bench")
        .arg("--corpus")
        .arg(corpus)
        .args(extra)
        .output()
        .expect("run nexspec bench")
}

fn write_corpus(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("corpus.toml");
    std::fs::write(&path, CORPUS).unwrap();
    path
}

#[test]
fn json_report_has_metrics_tokens_and_leaves_the_repo_untouched() {
    let repo = fixture();
    let corpus_dir = tempfile::TempDir::new().unwrap();
    let corpus = write_corpus(corpus_dir.path());

    let out = bench(repo.path(), &corpus, &["--format", "json"]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON on stdout");

    let queries = report["queries"].as_array().unwrap();
    assert_eq!(queries.len(), 4);
    let by_id = |id: &str| queries.iter().find(|q| q["id"] == id).unwrap_or_else(|| panic!("query {id}"));

    let handler = by_id("locate-handler");
    assert_eq!(handler["recall"]["5"], 1.0, "{handler}");
    assert!(handler["reciprocal_rank"].as_f64().unwrap() > 0.0);
    assert!(handler["tokens_nexspec"].as_u64().unwrap() > 0);
    assert!(handler["tokens_grep"].as_u64().unwrap() > 0);
    assert!(handler["tokens_read"].as_u64().unwrap() > 0);
    assert!(handler["latency_ms"].as_f64().unwrap() >= 0.0);

    let trace = by_id("trace-req");
    assert_eq!(trace["recall"]["5"], 1.0, "marker and implementer both found: {trace}");

    let miss = by_id("miss");
    assert_eq!(miss["recall"]["5"], 0.0);
    assert!(miss["ranks"][0].is_null());

    assert!(report["by_kind"]["locate"]["queries"] == 2);
    assert!(report["by_kind"]["locate"]["recall"]["5"].as_f64().unwrap() > 0.0);
    assert!(report["corpus_tokens"].as_u64().unwrap() > 0);
    assert!(report["reduction_ratio"].as_f64().unwrap() >= 0.0);
    assert_eq!(report["ks"], serde_json::json!([5, 10]));

    assert!(!repo.path().join(".specs/.index").exists(), "the benchmark must not write an index into the target repo");
    let status = Command::new("git").args(["status", "--porcelain"]).current_dir(repo.path()).output().unwrap();
    assert!(String::from_utf8_lossy(&status.stdout).trim().is_empty(), "repo stays clean");
}

#[test]
fn markdown_report_has_kind_sections_misses_and_the_proxy_caveat() {
    let repo = fixture();
    let corpus_dir = tempfile::TempDir::new().unwrap();
    let corpus = write_corpus(corpus_dir.path());

    let out = bench(repo.path(), &corpus, &["--k", "1,3"]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let md = String::from_utf8_lossy(&out.stdout);
    assert!(md.contains("## By kind"), "{md}");
    assert!(md.contains("recall@1") && md.contains("recall@3"), "{md}");
    assert!(md.contains("| locate |") && md.contains("| behavior |") && md.contains("| traceability |"), "{md}");
    assert!(md.contains("local proxy"), "tokens are declared a proxy: {md}");
    assert!(md.contains("## Misses") && md.contains("nothingLikeThisExists"), "{md}");
    assert!(md.contains("whole corpus"), "{md}");
}

#[test]
fn bad_corpus_and_bad_flags_fail_with_a_readable_error() {
    let repo = fixture();
    let dir = tempfile::TempDir::new().unwrap();
    let broken = dir.path().join("broken.toml");
    std::fs::write(&broken, "[[query]]\nid = \"a\"\nkind = \"locate\"\nquery = \"x\"\nexpect = []\n").unwrap();
    let out = bench(repo.path(), &broken, &[]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("\"a\""), "names the query: {}", String::from_utf8_lossy(&out.stderr));

    let good = write_corpus(dir.path());
    let out = bench(repo.path(), &good, &["--format", "xml"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown format"));
}

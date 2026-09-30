//! Baseline recording and the regression gate of `nexspec bench` (T-807, REQ-805).

mod fixtures;

use std::path::Path;
use std::process::{Command, Output};

use fixtures::FixtureRepo;

const CORPUS: &str = r#"
[[query]]
id = "found"
kind = "locate"
query = "handleInvoice"
expect = ["src/invoice/handler.ts"]

[[query]]
id = "missing"
kind = "locate"
query = "nothingLikeThisExists"
expect = ["src/ghost/none.ts"]
"#;

fn fixture() -> (FixtureRepo, tempfile::TempDir) {
    let repo = FixtureRepo::init();
    repo.write_file("src/invoice/handler.ts", "export function handleInvoice() {\n  return 1;\n}\n");
    repo.write_file("src/ghost/none.ts", "export const none = 0;\n");
    repo.commit("feat: invoices");
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("corpus.toml"), CORPUS).unwrap();
    (repo, dir)
}

fn bench(repo: &Path, dir: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nexspec"))
        .arg("--repo")
        .arg(repo)
        .args(["bench", "--no-vector", "--corpus"])
        .arg(dir.join("corpus.toml"))
        .arg("--output")
        .arg(dir.join("report.md"))
        .args(extra)
        .output()
        .expect("run nexspec bench")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

#[test]
fn recorded_baseline_passes_its_own_check() {
    let (repo, dir) = fixture();
    let baseline = dir.path().join("baseline.json");

    let out = bench(repo.path(), dir.path(), &["--update-baseline", baseline.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr(&out));
    let recorded: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&baseline).unwrap()).unwrap();
    assert_eq!(recorded["by_kind"]["locate"]["recall"]["5"], 0.5);
    assert_eq!(recorded["vector_search"], false);

    let out = bench(repo.path(), dir.path(), &["--check", baseline.to_str().unwrap(), "--min-locate-recall", "0.5"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains("benchmark gate passed"));
}

#[test]
fn regression_against_a_better_baseline_fails_with_the_delta() {
    let (repo, dir) = fixture();
    let baseline = dir.path().join("baseline.json");
    bench(repo.path(), dir.path(), &["--update-baseline", baseline.to_str().unwrap()]);

    let mut recorded: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&baseline).unwrap()).unwrap();
    recorded["by_kind"]["locate"]["recall"]["5"] = serde_json::json!(0.9);
    std::fs::write(&baseline, serde_json::to_string(&recorded).unwrap()).unwrap();

    let out = bench(repo.path(), dir.path(), &["--check", baseline.to_str().unwrap(), "--min-locate-recall", "0.0"]);
    assert!(!out.status.success(), "a 40 point drop must fail the gate");
    let message = stderr(&out);
    assert!(message.contains("benchmark gate failed"), "{message}");
    assert!(message.contains("locate recall@5 fell from 0.90 to 0.50"), "{message}");
}

#[test]
fn absolute_floor_applies_even_without_a_baseline_file() {
    let (repo, dir) = fixture();
    let missing = dir.path().join("none.json");
    let out = bench(repo.path(), dir.path(), &["--check", missing.to_str().unwrap()]);
    assert!(!out.status.success());
    let message = stderr(&out);
    assert!(message.contains("no baseline at"), "{message}");
    assert!(message.contains("locate recall@5 is 0.50, below the required 0.80"), "{message}");
}

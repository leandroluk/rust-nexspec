//! `nexspec enrich` end to end with a fake provider (T-1906, REQ-1901, REQ-1906, REQ-1909, REQ-1912).

mod fixtures;

use std::path::Path;
use std::process::{Command, Output};

use fixtures::FixtureRepo;

fn run(repo: &Path, fixtures: Option<&Path>, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nexspec"));
    command.arg("--repo").arg(repo).args(args);
    if let Some(path) = fixtures {
        command.env("NEXSPEC_ENRICH_FIXTURES", path);
    }
    command.output().unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

fn repo_with_files() -> FixtureRepo {
    let repo = FixtureRepo::init();
    repo.write_file(".gitignore", ".specs/.index/\n.specs/.cache/\n");
    repo.write_file("src/billing/charge.usecase.ts", "export class ChargeUsecase { run() { return 1; } }\n");
    repo.write_file("src/lease/create.usecase.ts", "export class CreateUsecase { run() { return 2; } }\n");
    repo.write_file("src/config/secrets.ts", "export const password = \"hunter2hunter2\";\n");
    repo.commit("init");
    repo
}

fn write_fixtures(dir: &Path, json: &str) -> std::path::PathBuf {
    let path = dir.join("fixtures.json");
    std::fs::write(&path, json).unwrap();
    path
}

const ANSWERS: &str = r#"{
  "src/billing/charge.usecase.ts": { "en": "Charges residents for their monthly condominium invoices" },
  "src/lease/create.usecase.ts": { "en": "Creates and validates a tenant rental contract" }
}"#;

#[test]
fn enrich_makes_a_prose_question_find_the_file_and_no_enrich_does_not() {
    let repo = repo_with_files();
    let tmp = tempfile::TempDir::new().unwrap();
    let fixtures = write_fixtures(tmp.path(), ANSWERS);
    assert!(run(repo.path(), None, &["sync"]).status.success());

    let question = "how are invoices charged to residents";
    let before = out(&run(repo.path(), None, &["search", question]));
    assert!(!before.contains("charge"), "prose finds nothing before enrich: {before}");

    // The first use needs an explicit yes (no terminal in a test).
    let refused = run(repo.path(), Some(&fixtures), &["enrich", "--provider", "fake"]);
    assert!(!refused.status.success() && err(&refused).contains("--yes"), "{}", err(&refused));

    let done = run(repo.path(), Some(&fixtures), &["enrich", "--provider", "fake", "--yes"]);
    assert!(done.status.success(), "{}", err(&done));
    assert!(out(&done).contains("enriched 2 file(s), 0 failed"), "{}", out(&done));

    let after = run(repo.path(), None, &["search", question]);
    let after_text = out(&after);
    assert!(after.status.success(), "{}", err(&after));
    assert!(after_text.contains("src/billing/charge.usecase.ts"), "{after_text}");
    let without = out(&run(repo.path(), None, &["search", "--no-enrich", question]));
    assert_eq!(without, before, "--no-enrich ranks on code text only");

    // Nothing left to do; the cache is where the spec says.
    let again = run(repo.path(), Some(&fixtures), &["enrich", "--provider", "fake", "--yes"]);
    assert!(out(&again).contains("nothing to do"), "{}", out(&again));
    assert!(repo.path().join(".specs/.cache/enrichment.jsonl").is_file());
}

#[test]
fn a_file_with_a_secret_is_kept_back_and_a_missing_answer_is_a_partial_failure() {
    let repo = repo_with_files();
    let tmp = tempfile::TempDir::new().unwrap();
    let fixtures = write_fixtures(tmp.path(), r#"{ "src/billing/charge.usecase.ts": { "en": "Charges residents" } }"#);
    assert!(run(repo.path(), None, &["sync"]).status.success());

    let dry = run(repo.path(), None, &["enrich", "--dry-run"]);
    assert!(dry.status.success(), "a dry run needs no key: {}", err(&dry));
    let text = out(&dry);
    assert!(text.contains("2 file(s) to send") && text.contains("1 kept back"), "{text}");
    assert!(text.contains("omitted: src/config/secrets.ts (hard-coded credential)") && text.contains("top  20%"), "{text}");

    let run1 = run(repo.path(), Some(&fixtures), &["enrich", "--provider", "fake", "--yes"]);
    assert_eq!(run1.status.code(), Some(6), "a partial failure has its own exit code: {}", err(&run1));
    assert!(err(&run1).contains("src/lease/create.usecase.ts"), "{}", err(&run1));
    assert!(out(&run1).contains("enriched 1 file(s), 1 failed"), "{}", out(&run1));

    let status = out(&run(repo.path(), None, &["enrich", "--status"]));
    assert!(status.starts_with("enrichment: 1/"), "{status}");
    assert!(status.contains("failed: src/lease/create.usecase.ts") && status.contains("omitted (secret): src/config/secrets.ts"), "{status}");
}

#[test]
fn a_changed_file_is_stale_and_leaves_the_ranking_until_reenriched_and_clear_removes_the_cache() {
    let repo = repo_with_files();
    let tmp = tempfile::TempDir::new().unwrap();
    let fixtures = write_fixtures(tmp.path(), ANSWERS);
    assert!(run(repo.path(), None, &["sync"]).status.success());
    assert!(run(repo.path(), Some(&fixtures), &["enrich", "--provider", "fake", "--yes"]).status.success());

    repo.write_file("src/billing/charge.usecase.ts", "export class ChargeUsecase { run() { return 99; } }\n");
    let status = out(&run(repo.path(), None, &["enrich", "--status"]));
    assert!(status.contains("1 stale"), "{status}");

    let cleared = run(repo.path(), None, &["enrich", "--clear"]);
    assert!(out(&cleared).contains("cleared"));
    assert!(!repo.path().join(".specs/.cache/enrichment.jsonl").exists());
}

#[test]
fn an_unsupported_language_is_refused_with_a_clear_message() {
    let repo = repo_with_files();
    let tmp = tempfile::TempDir::new().unwrap();
    let fixtures = write_fixtures(tmp.path(), ANSWERS);
    assert!(run(repo.path(), None, &["sync"]).status.success());
    let out = run(repo.path(), Some(&fixtures), &["enrich", "--provider", "fake", "--yes", "--lang", "zh"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(err(&out).contains("unsupported summary language `zh`"), "{}", err(&out));
}

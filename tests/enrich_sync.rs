//! Enrichment and the rest of the workflow (T-1907, REQ-1907, REQ-1910): a rebuilt index
//! recovers its summaries from the cache without a provider, `sync` hints, `init`/`doctor`
//! look after `.gitignore`.

mod fixtures;

use std::path::Path;
use std::process::{Command, Output};

use fixtures::FixtureRepo;

fn run(repo: &Path, fixtures: Option<&Path>, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nexspec"));
    command.arg("--repo").arg(repo).args(args).env_remove("GEMINI_API_KEY").env_remove("GOOGLE_API_KEY");
    if let Some(path) = fixtures {
        command.env("NEXSPEC_ENRICH_FIXTURES", path);
    }
    command.output().unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn enriched_repo() -> (FixtureRepo, tempfile::TempDir) {
    let repo = FixtureRepo::init();
    repo.write_file(".gitignore", ".specs/.index/\n.specs/.cache/\n");
    repo.write_file("src/billing/charge.usecase.ts", "export class ChargeUsecase { run() { return 1; } }\n");
    repo.commit("init");
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::write(tmp.path().join("f.json"), r#"{ "src/billing/charge.usecase.ts": { "en": "Charges residents for their monthly condominium invoices" } }"#).unwrap();
    assert!(run(repo.path(), None, &["sync"]).status.success());
    let done = run(repo.path(), Some(&tmp.path().join("f.json")), &["enrich", "--provider", "fake", "--yes"]);
    assert!(done.status.success(), "{}", String::from_utf8_lossy(&done.stderr));
    (repo, tmp)
}

#[test]
fn a_rebuilt_index_recovers_its_summaries_from_the_cache_without_a_provider() {
    let (repo, _tmp) = enriched_repo();
    let question = "how are invoices charged to residents";
    assert!(out(&run(repo.path(), None, &["search", question])).contains("src/billing/charge.usecase.ts"));

    std::fs::remove_dir_all(repo.path().join(".specs/.index")).unwrap();
    assert!(run(repo.path(), None, &["sync"]).status.success(), "rebuild");
    assert!(out(&run(repo.path(), None, &["search", question])).contains("src/billing/charge.usecase.ts"), "summaries came back from the cache");
}

#[test]
fn sync_hints_about_stale_summaries_and_stays_quiet_without_a_cache() {
    let (repo, _tmp) = enriched_repo();
    repo.write_file("src/billing/charge.usecase.ts", "export class ChargeUsecase { run() { return 2; } }\n");
    let sync = run(repo.path(), None, &["sync"]);
    assert!(out(&sync).contains("enrichment: 1 stale, 0 pending — nexspec enrich"), "{}", out(&sync));

    let plain = FixtureRepo::init();
    plain.write_file("a.ts", "export const a = 1;\n");
    plain.commit("init");
    assert!(!out(&run(plain.path(), None, &["sync"])).contains("enrichment"));
}

#[test]
fn init_adds_the_local_directories_to_gitignore_and_doctor_notices_their_absence() {
    let repo = FixtureRepo::init();
    repo.write_file("a.ts", "export const a = 1;\n");
    repo.commit("init");
    let init = run(repo.path(), None, &["init"]);
    assert!(out(&init).contains("added .specs/.index/ to .gitignore") && out(&init).contains("added .specs/.cache/ to .gitignore"), "{}", out(&init));
    let text = std::fs::read_to_string(repo.path().join(".gitignore")).unwrap();
    assert!(text.contains(".specs/.index/") && text.contains(".specs/.cache/"));
    assert!(!out(&run(repo.path(), None, &["init"])).contains("added"), "idempotent");

    std::fs::write(repo.path().join(".gitignore"), ".specs/.index/\n").unwrap();
    std::fs::create_dir_all(repo.path().join(".specs/.cache")).unwrap();
    std::fs::write(repo.path().join(".specs/.cache/enrichment.jsonl"), "").unwrap();
    let doctor = out(&run(repo.path(), None, &["doctor"]));
    assert!(doctor.contains("[warn] gitignore") && doctor.contains(".specs/.cache/"), "{doctor}");
}

#[test]
fn doctor_warns_about_the_missing_key_only_when_a_cache_exists() {
    let (repo, _tmp) = enriched_repo();
    let doctor = out(&run(repo.path(), None, &["doctor"]));
    assert!(doctor.contains("[warn] enrichment") && doctor.contains("GEMINI_API_KEY is not set"), "{doctor}");

    let plain = FixtureRepo::init();
    plain.write_file("a.ts", "export const a = 1;\n");
    plain.commit("init");
    assert!(!out(&run(plain.path(), None, &["doctor"])).contains("enrichment"));
}

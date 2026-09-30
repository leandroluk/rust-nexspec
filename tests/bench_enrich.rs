//! `bench --compare-enrich` (T-1908, REQ-1911): prose questions without and with the summaries.

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

const CORPUS: &str = r#"
[[query]]
id = "locate-charge"
kind = "locate"
query = "ChargeUsecase"
grep = "ChargeUsecase"
expect = ["src/billing/charge.usecase.ts"]

[[query]]
id = "prose-invoices"
kind = "behavior"
query = "how are invoices charged to residents"
expect = ["src/billing/charge.usecase.ts"]

[[query]]
id = "prose-contract"
kind = "behavior"
query = "how is a tenant contract created"
expect = ["src/lease/create.usecase.ts"]
"#;

#[test]
fn enrichment_lifts_prose_recall_without_hurting_locate_and_the_report_says_so() {
    let repo = FixtureRepo::init();
    repo.write_file(".gitignore", ".specs/.index/\n.specs/.cache/\n");
    repo.write_file("src/billing/charge.usecase.ts", "export class ChargeUsecase { run() { return 1; } }\n");
    repo.write_file("src/lease/create.usecase.ts", "export class CreateUsecase { run() { return 2; } }\n");
    repo.write_file("src/noise/a.ts", "export const a = 1;\n");
    repo.commit("init");
    let tmp = tempfile::TempDir::new().unwrap();
    let corpus = tmp.path().join("corpus.toml");
    std::fs::write(&corpus, CORPUS).unwrap();
    let fixtures = tmp.path().join("f.json");
    std::fs::write(
        &fixtures,
        r#"{ "src/billing/charge.usecase.ts": { "en": "Charges residents for their monthly condominium invoices" },
             "src/lease/create.usecase.ts": { "en": "Creates and validates a tenant rental contract" },
             "src/noise/a.ts": { "en": "A constant" } }"#,
    )
    .unwrap();
    assert!(run(repo.path(), None, &["sync"]).status.success());
    let enriched = run(repo.path(), Some(&fixtures), &["enrich", "--provider", "fake", "--yes"]);
    assert!(enriched.status.success(), "{}", String::from_utf8_lossy(&enriched.stderr));

    let out = run(repo.path(), None, &["bench", "--corpus", corpus.to_str().unwrap(), "--no-vector", "--compare-enrich"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(text.contains("# nexspec enrichment comparison") && text.contains("## Acceptance (REQ-1911)"), "{text}");
    assert!(text.contains("| behavior | 2 | 0% | 100% |"), "prose goes from nothing to everything: {text}");
    assert!(text.contains("+100.0 points (pass)"), "the prose criterion is judged (latency on a three-file fixture is noise, so it is not asserted): {text}");
    assert!(text.contains("Enrichment used"), "the payback line needs the state of the last run: {text}");
}

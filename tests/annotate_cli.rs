//! Semantic annotations end to end (T-1802..T-1804, REQ-1801..1807).

mod fixtures;

use std::path::Path;
use std::process::{Command, Output};

use fixtures::FixtureRepo;

fn run(repo: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nexspec")).arg("--repo").arg(repo).args(args).output().unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

fn ok(o: Output) -> Output {
    assert!(o.status.success(), "{}", err(&o));
    o
}

fn repo() -> FixtureRepo {
    let repo = FixtureRepo::init();
    repo.write_file(".gitignore", ".specs/.index/\n.specs/.cache/\n.specs/.memory/notes/\n");
    repo.write_file("src/invoice/current.ts", "export function alphaInvoiceHandler() { return 1; }\n");
    repo.write_file("src/invoice/legacy.ts", "export function alphaInvoiceHandlerLegacy() { return 2; }\n");
    repo.write_file("src/other/util.ts", "import { alphaInvoiceHandler } from '../invoice/current';\nexport function unrelatedHelper() { return alphaInvoiceHandler(); }\n");
    repo.commit("init");
    ok(run(repo.path(), &["sync"]));
    repo
}

fn annotations_file(repo: &Path) -> String {
    std::fs::read_to_string(repo.join(".specs/.memory/annotations.jsonl")).unwrap_or_default()
}

#[test]
fn an_annotation_is_saved_with_provenance_and_shows_in_explain_and_query() {
    let repo = repo();
    let done = ok(run(repo.path(), &["annotate", "alphaInvoiceHandler", "--label", "Billing entry", "--note", "Charges residents; called by the nightly job.", "--author", "agent", "--model", "test-model"]));
    assert!(out(&done).contains("annotated `src/invoice/current.ts::alphaInvoiceHandler`"), "{}", out(&done));

    let line: serde_json::Value = serde_json::from_str(annotations_file(repo.path()).lines().next().unwrap()).unwrap();
    assert_eq!(line["target"], "src/invoice/current.ts::alphaInvoiceHandler", "a stable key, not an internal id");
    assert_eq!((line["author"].as_str(), line["model"].as_str(), line["confidence"].as_str()), (Some("agent"), Some("test-model"), Some("INFERRED")));
    assert!(line["source_hash"].as_str().is_some_and(|h| h.len() == 64) && line["at"].as_str().is_some_and(|t| t.ends_with('Z')), "{line}");

    let explain = out(&ok(run(repo.path(), &["explain", "alphaInvoiceHandler"])));
    assert!(explain.contains("## Annotations") && explain.contains("[fresh] agent,") && explain.contains("Billing entry: Charges residents; called by the nightly job."), "{explain}");
    let query = out(&ok(run(repo.path(), &["query", "alphaInvoiceHandler"])));
    assert!(query.contains("notes: Billing entry: Charges residents"), "{query}");
    let affected = out(&ok(run(repo.path(), &["affected", "alphaInvoiceHandler"])));
    assert!(affected.contains("unrelatedHelper"), "{affected}");

    // The same thing said again is the same annotation.
    ok(run(repo.path(), &["annotate", "alphaInvoiceHandler", "--label", "Billing entry", "--note", "Charges residents; called by the nightly job.", "--author", "agent", "--model", "test-model"]));
    assert_eq!(annotations_file(repo.path()).lines().count(), 1);
}

#[test]
fn the_file_is_the_truth_and_the_index_can_be_rebuilt_from_it() {
    let repo = repo();
    ok(run(repo.path(), &["annotate", "src/invoice/current.ts", "--note", "The current handler lives here."]));
    std::fs::remove_dir_all(repo.path().join(".specs/.index")).unwrap();
    ok(run(repo.path(), &["sync"]));
    let explain = out(&ok(run(repo.path(), &["explain", "src/invoice/current.ts"])));
    assert!(explain.contains("The current handler lives here."), "{explain}");
}

#[test]
fn a_changed_target_makes_the_annotation_stale_and_a_removed_one_dangling() {
    let repo = repo();
    ok(run(repo.path(), &["annotate", "src/invoice/current.ts", "--note", "Zanzibar handler of invoices."]));
    ok(run(repo.path(), &["annotate", "src/invoice/legacy.ts", "--note", "Old handler, to be removed."]));
    assert!(out(&ok(run(repo.path(), &["search", "zanzibar"]))).contains("annotation"), "a fresh annotation can be found");
    assert!(out(&ok(run(repo.path(), &["annotate", "lint"]))).starts_with("lint: 0 finding(s)"));

    repo.write_file("src/invoice/current.ts", "export function alphaInvoiceHandler() { return 99; }\n");
    repo.remove_file("src/invoice/legacy.ts");
    repo.commit("change and delete");
    ok(run(repo.path(), &["sync"]));

    let stale = out(&ok(run(repo.path(), &["annotate", "list", "--state", "stale"])));
    assert!(stale.contains("`src/invoice/current.ts`"), "{stale}");
    let dangling = out(&ok(run(repo.path(), &["annotate", "list", "--state", "dangling"])));
    assert!(dangling.contains("`src/invoice/legacy.ts`"), "{dangling}");
    let explain = out(&ok(run(repo.path(), &["explain", "src/invoice/current.ts"])));
    assert!(explain.contains("[stale]") && explain.contains("Zanzibar"), "a stale annotation stays visible, marked: {explain}");
    assert!(!out(&ok(run(repo.path(), &["search", "zanzibar"]))).contains("annotation"), "but it leaves the search");
    assert!(!out(&ok(run(repo.path(), &["query", "alphaInvoiceHandler"]))).contains("notes: Zanzibar"), "and the inline notes");

    let lint = run(repo.path(), &["annotate", "lint"]);
    assert_eq!(lint.status.code(), Some(8), "{}", out(&lint));
    let text = out(&lint);
    assert!(text.contains("lint: 2 finding(s)") && text.contains("stale:") && text.contains("dangling:"), "{text}");
}

#[test]
fn an_annotated_relation_is_inferred_marked_and_never_replaces_an_extracted_edge() {
    let repo = repo();
    ok(run(repo.path(), &["annotate", "src/invoice/legacy.ts", "--relation", "references", "--to", "src/invoice/current.ts", "--note", "The legacy file wraps the current one."]));
    let affected = out(&ok(run(repo.path(), &["affected", "src/invoice/current.ts"])));
    assert!(affected.contains("legacy.ts") && affected.contains("inferred") && affected.contains("annotation"), "{affected}");
    let only_extracted = out(&ok(run(repo.path(), &["affected", "src/invoice/current.ts", "--min-confidence", "extracted"])));
    assert!(!only_extracted.contains("legacy.ts"), "annotations are filtered out by --min-confidence extracted: {only_extracted}");
    assert!(only_extracted.contains("util.ts"), "the extracted import is untouched: {only_extracted}");
}

#[test]
fn bad_annotations_are_refused_and_the_governance_commands_work() {
    let repo = repo();
    let long = run(repo.path(), &["annotate", "src/invoice/current.ts", "--note", &"x".repeat(501)]);
    assert!(!long.status.success() && err(&long).contains("limit is 500"), "{}", err(&long));
    let token = format!("{}{}", "gh", "p_abcdefghijklmnopqrstuvwxyz0123456789");
    let secret = run(repo.path(), &["annotate", "src/invoice/current.ts", "--note", &format!("token {token}")]);
    assert!(!secret.status.success() && err(&secret).contains("GitHub token") && !err(&secret).contains(&token), "{}", err(&secret));
    let nothing = run(repo.path(), &["annotate", "src/invoice/current.ts"]);
    assert!(!nothing.status.success() && err(&nothing).contains("needs a --label"), "{}", err(&nothing));
    let missing = run(repo.path(), &["annotate", "noSuchThingAnywhere", "--note", "x"]);
    assert!(!missing.status.success() && err(&missing).contains("no node matches"), "{}", err(&missing));
    assert_eq!(annotations_file(repo.path()), "", "nothing was written");

    ok(run(repo.path(), &["annotate", "src/invoice/current.ts", "--note", "First note."]));
    let id = out(&ok(run(repo.path(), &["annotate", "list"]))).lines().nth(1).unwrap().split_whitespace().next().unwrap().to_string();
    let show = out(&ok(run(repo.path(), &["annotate", "show", &id[..4]])));
    assert!(show.contains("- state: fresh") && show.contains("- note: First note.") && show.contains("- confidence: INFERRED"), "{show}");
    let removed = out(&ok(run(repo.path(), &["annotate", "remove", &id])));
    assert!(removed.contains("removed"), "{removed}");
    assert_eq!(annotations_file(repo.path()), "");
    assert!(!run(repo.path(), &["annotate", "remove", "zzzz"]).status.success());
}

#[test]
fn outcomes_nudge_the_ranking_and_no_memory_undoes_it() {
    let repo = repo();
    let order = |extra: &[&str]| -> Vec<String> {
        let mut args = vec!["search", "alpha invoice handler"];
        args.extend_from_slice(extra);
        out(&ok(run(repo.path(), &args))).lines().filter_map(|l| l.split_whitespace().last().map(str::to_string)).collect()
    };
    // Annotations are searchable too; the ranking of the two handlers is what matters here.
    let handlers = |o: Vec<String>| -> Vec<String> { o.into_iter().filter(|n| n.starts_with("alphaInvoiceHandler") && !n.contains("::")).collect() };
    let before = handlers(order(&[]));
    let first = before.iter().find(|n| n.starts_with("alphaInvoiceHandler")).cloned().unwrap();
    let other = before.iter().find(|n| n.starts_with("alphaInvoiceHandler") && **n != first).cloned().unwrap();

    ok(run(repo.path(), &["annotate", &first, "--outcome", "dead_end", "--note", "Not the one we want."]));
    let after = handlers(order(&[]));
    let pos = |o: &[String], n: &str| o.iter().position(|x| x == n).unwrap();
    assert!(pos(&after, &other) < pos(&after, &first), "{after:?}");
    assert_eq!(handlers(order(&["--no-memory"])), before);
}

#[test]
fn a_community_label_replaces_the_derived_one_in_the_report_and_the_wiki() {
    let repo = FixtureRepo::init();
    repo.write_file(".gitignore", ".specs/.index/\n.specs/.cache/\n");
    for (dir, names) in [("a", ["one", "two", "three"]), ("b", ["four", "five", "six"])] {
        for (i, name) in names.iter().enumerate() {
            let next = names[(i + 1) % 3];
            repo.write_file(&format!("src/{dir}/{name}.ts"), &format!("import {{ {next}Fn }} from './{next}';\nexport function {name}Fn() {{ return {next}Fn; }}\n"));
        }
    }
    repo.commit("init");
    ok(run(repo.path(), &["sync"]));
    let before = out(&ok(run(repo.path(), &["report"])));
    assert!(before.contains("## Communities") && !before.contains("Billing core"));

    let done = ok(run(repo.path(), &["annotate", "community:1", "--label", "Billing core"]));
    assert!(out(&done).contains("annotated `community:"), "{}", out(&done));
    let after = out(&ok(run(repo.path(), &["report"])));
    assert!(after.contains("Billing core"), "{after}");
    let wiki = tempfile::TempDir::new().unwrap();
    ok(run(repo.path(), &["export", "--format", "wiki", "--out", wiki.path().to_str().unwrap()]));
    assert!(std::fs::read_to_string(wiki.path().join("index.md")).unwrap().contains("Billing core"));

    let unknown = run(repo.path(), &["annotate", "community:99", "--label", "x"]);
    assert!(!unknown.status.success() && err(&unknown).contains("no community matches"), "{}", err(&unknown));
}

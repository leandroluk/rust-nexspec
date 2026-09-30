//! The work memory end to end (T-1501..T-1505, REQ-1501..1505): save results, reflect, nudge the ranking.

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
    repo.write_file("src/other/util.ts", "export function unrelatedHelper() { return 3; }\n");
    repo.commit("init");
    ok(run(repo.path(), &["sync"]));
    repo
}

fn save(repo: &Path, question: &str, node: &str, outcome: &str) -> Output {
    run(repo, &["save-result", "--question", question, "--answer", "short", "--nodes", node, "--outcome", outcome])
}

/// The names `search` lists, best first.
fn search_order(repo: &Path, extra: &[&str]) -> Vec<String> {
    let mut args = vec!["search", "alpha invoice handler"];
    args.extend_from_slice(extra);
    out(&ok(run(repo, &args))).lines().filter_map(|l| l.split_whitespace().last().map(str::to_string)).collect()
}

#[test]
fn results_become_lessons_and_a_short_session_summary() {
    let repo = repo();
    let first = ok(save(repo.path(), "where are invoices handled", "alphaInvoiceHandler", "useful"));
    assert!(out(&first).contains("saved ") && out(&first).contains("(useful, 1 node(s))"), "{}", out(&first));
    ok(save(repo.path(), "who handles invoices now", "alphaInvoiceHandler", "useful"));
    ok(save(repo.path(), "where was the old invoice handler", "alphaInvoiceHandlerLegacy", "dead_end"));
    let corrected = run(
        repo.path(),
        &["save-result", "--question", "which file bills residents", "--nodes", "unrelatedHelper", "--outcome", "corrected", "--correction", "it is in src/invoice/current.ts"],
    );
    assert!(corrected.status.success(), "{}", err(&corrected));

    let reflect = ok(run(repo.path(), &["reflect"]));
    assert!(out(&reflect).contains("reflect: 3 lesson(s) from 4 note(s): 1 preferred"), "{}", out(&reflect));
    let lessons = std::fs::read_to_string(repo.path().join(".specs/.memory/LESSONS.md")).unwrap();
    assert!(lessons.contains("## Preferred") && lessons.contains("alphaInvoiceHandler (src/invoice/current.ts)` — useful ×2"), "{lessons}");
    assert!(lessons.contains("## Dead ends") && lessons.contains("alphaInvoiceHandlerLegacy"), "{lessons}");
    assert!(lessons.contains("**which file bills residents** → it is in src/invoice/current.ts"), "{lessons}");

    // The same lessons again: same bytes (nothing in the file depends on when it was made).
    ok(run(repo.path(), &["reflect"]));
    assert_eq!(lessons, std::fs::read_to_string(repo.path().join(".specs/.memory/LESSONS.md")).unwrap());

    let summary = out(&ok(run(repo.path(), &["reflect", "--max-tokens", "400"])));
    assert!(summary.starts_with("# Work memory (nexspec)\n- correction:"), "corrections come first: {summary}");
    assert!(summary.contains("- avoid: alphaInvoiceHandlerLegacy") && summary.contains("- prefer: alphaInvoiceHandler"), "{summary}");
    let tiny = out(&ok(run(repo.path(), &["reflect", "--max-tokens", "20"])));
    assert!(tiny.lines().count() < summary.lines().count(), "the budget cuts the summary: {tiny}");
}

#[test]
fn bad_results_are_refused_with_a_reason() {
    let repo = repo();
    let no_fix = run(repo.path(), &["save-result", "--question", "q", "--nodes", "alphaInvoiceHandler", "--outcome", "corrected"]);
    assert_eq!(no_fix.status.code(), Some(1));
    assert!(err(&no_fix).contains("--correction"), "{}", err(&no_fix));

    let missing = save(repo.path(), "q", "noSuchThingAnywhere", "useful");
    assert!(!missing.status.success() && err(&missing).contains("no node matches"), "{}", err(&missing));

    let token = format!("{}{}", "gh", "p_abcdefghijklmnopqrstuvwxyz0123456789");
    let secret = run(repo.path(), &["save-result", "--question", "q", "--answer", &format!("the token is {token}"), "--nodes", "alphaInvoiceHandler", "--outcome", "useful"]);
    assert!(!secret.status.success() && err(&secret).contains("GitHub token"), "{}", err(&secret));
    assert!(!err(&secret).contains(&token), "the secret itself is never echoed");

    let outcome = save(repo.path(), "q", "alphaInvoiceHandler", "great");
    assert!(err(&outcome).contains("unknown outcome"), "{}", err(&outcome));
    let notes = repo.path().join(".specs/.memory/notes");
    assert!(!notes.exists() || std::fs::read_dir(&notes).unwrap().count() == 0, "nothing was written");
}

#[test]
fn a_dead_end_sinks_in_the_ranking_and_no_memory_brings_it_back() {
    let repo = repo();
    let before = search_order(repo.path(), &[]);
    let first_hit = before.iter().find(|n| n.starts_with("alphaInvoiceHandler")).cloned().expect("a handler is found");
    let other = before.iter().find(|n| n.starts_with("alphaInvoiceHandler") && **n != first_hit).cloned().expect("both handlers are found");

    ok(save(repo.path(), "was this the one?", &first_hit, "dead_end"));
    ok(run(repo.path(), &["reflect"]));
    assert!(repo.path().join(".specs/.cache/memory.json").is_file(), "the ranking cache is derived data");

    let with_memory = search_order(repo.path(), &[]);
    let pos = |order: &[String], name: &str| order.iter().position(|n| n == name).unwrap();
    assert!(pos(&with_memory, &other) < pos(&with_memory, &first_hit), "the dead end now ranks below the other handler: {with_memory:?}");
    let without = search_order(repo.path(), &["--no-memory"]);
    assert_eq!(without, before, "--no-memory is the ranking as it was");
}

#[test]
fn bench_compare_memory_judges_the_nudge_and_its_exit_code_agrees_with_the_verdict() {
    let repo = repo();
    let corpus = repo.path().join("corpus.toml");
    std::fs::write(
        &corpus,
        "[[query]]\nid = \"current\"\nkind = \"locate\"\nquery = \"alphaInvoiceHandler\"\nexpect = [\"src/invoice/current.ts\"]\n\n[[query]]\nid = \"helper\"\nkind = \"locate\"\nquery = \"unrelatedHelper\"\nexpect = [\"src/other/util.ts\"]\n",
    )
    .unwrap();
    let args = ["bench", "--corpus", corpus.to_str().unwrap(), "--no-vector", "--compare-memory"];

    ok(save(repo.path(), "noise", "unrelatedHelper", "useful"));
    ok(save(repo.path(), "noise again", "unrelatedHelper", "useful"));
    ok(run(repo.path(), &["reflect"]));
    let fine = ok(run(repo.path(), &args));
    assert!(out(&fine).contains("**Accepted**") && out(&fine).contains("| locate |"), "{}", out(&fine));

    // Now the memory calls what answers the first question a dead end.
    ok(save(repo.path(), "bad advice", "alphaInvoiceHandler", "dead_end"));
    ok(save(repo.path(), "bad advice at file level", "src/invoice/current.ts", "dead_end"));
    ok(run(repo.path(), &["reflect"]));
    let results = run(repo.path(), &args);
    let text = out(&results);
    assert!(text.contains("| locate |"), "{text}");
    assert_eq!(text.contains("**Accepted**"), results.status.success(), "{text}{}", err(&results));
}

#[test]
fn init_keeps_the_raw_notes_out_of_git_but_not_the_lessons() {
    let repo = FixtureRepo::init();
    repo.write_file("a.ts", "export const a = 1;\n");
    repo.commit("init");
    let init = out(&ok(run(repo.path(), &["init"])));
    assert!(init.contains("added .specs/.memory/notes/ to .gitignore"), "{init}");
    let ignore = std::fs::read_to_string(repo.path().join(".gitignore")).unwrap();
    assert!(ignore.contains(".specs/.memory/notes/") && !ignore.contains("LESSONS"), "{ignore}");
}

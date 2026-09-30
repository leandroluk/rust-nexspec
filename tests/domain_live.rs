//! `extract` (T-1406, REQ-1405): drift between the changesets and a database, and live-only objects in the
//! graph. The database side is a saved schema file: the tests never need a server.

mod fixtures;

use std::path::Path;
use std::process::{Command, Output};

use fixtures::FixtureRepo;

fn run(repo: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nexspec")).arg("--repo").arg(repo).args(args).env_remove("NEXSPEC_POSTGRES_DSN").output().unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

const LIVE: &str = r#"{ "tables": [
  { "schema": "public", "name": "tb_contract", "is_view": false, "defined_in": "",
    "columns": [ { "name": "id", "sql_type": "uuid", "nullable": true } ], "constraints": [], "view_sources": [] },
  { "schema": "public", "name": "tb_only_in_the_database", "is_view": false, "defined_in": "",
    "columns": [ { "name": "id", "sql_type": "uuid", "nullable": false } ], "constraints": [], "view_sources": [] }
] }"#;

fn repo() -> FixtureRepo {
    let repo = FixtureRepo::init();
    repo.write_file(".gitignore", ".specs/.index/\n.specs/.cache/\n");
    repo.write_file("db/001.sql", "CREATE TABLE tb_contract (id uuid NOT NULL);\nCREATE TABLE tb_missing_in_db (id int);\n");
    repo.commit("init");
    repo
}

#[test]
fn drift_is_reported_first_and_a_dry_run_changes_nothing() {
    let repo = repo();
    let live = repo.path().join("live.json");
    std::fs::write(&live, LIVE).unwrap();
    let dry = run(repo.path(), &["extract", "--live-file", live.to_str().unwrap(), "--dry-run"]);
    assert!(dry.status.success(), "{}", String::from_utf8_lossy(&dry.stderr));
    let text = out(&dry);
    assert!(text.starts_with("drift: 3 difference(s)\n"), "{text}");
    assert!(text.contains("public.tb_missing_in_db") && text.contains("public.tb_only_in_the_database"), "{text}");
    assert!(text.contains("tb_contract.id") && text.contains("not null in the changesets, nullable in the database"), "{text}");
    assert!(!repo.path().join(".specs/.cache/live-schema.json").exists(), "a dry run saves nothing");
}

#[test]
fn live_only_objects_join_the_graph_and_survive_the_next_sync() {
    let repo = repo();
    assert!(run(repo.path(), &["sync"]).status.success());
    assert!(!out(&run(repo.path(), &["search", "tb_only_in_the_database"])).contains("table tb_only_in_the_database"));

    let live = repo.path().join("live.json");
    std::fs::write(&live, LIVE).unwrap();
    let done = run(repo.path(), &["extract", "--live-file", live.to_str().unwrap()]);
    assert!(done.status.success(), "{}", String::from_utf8_lossy(&done.stderr));
    assert!(out(&done).contains("live schema:"), "{}", out(&done));
    assert!(repo.path().join(".specs/.cache/live-schema.json").is_file());
    assert!(out(&run(repo.path(), &["search", "tb_only_in_the_database"])).contains("table tb_only_in_the_database"), "the live-only table is in the graph");

    // A later change to the changesets rebuilds the domain subgraph with the saved live schema still in it.
    repo.write_file("db/002.sql", "CREATE TABLE tb_later (id int);\n");
    repo.commit("later");
    assert!(run(repo.path(), &["sync"]).status.success());
    let text = out(&run(repo.path(), &["search", "tb_only_in_the_database"]));
    assert!(text.contains("table tb_only_in_the_database"), "{text}");
    assert!(out(&run(repo.path(), &["search", "tb_later"])).contains("table tb_later"));
}

#[test]
fn extract_needs_a_source() {
    let repo = repo();
    let none = run(repo.path(), &["extract"]);
    assert_eq!(none.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&none.stderr).contains("--postgres"), "{}", String::from_utf8_lossy(&none.stderr));
}

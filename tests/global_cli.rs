//! The global graph end to end (T-1301..T-1305, REQ-1301..1305): two repositories, links across them.

mod fixtures;

use std::path::Path;
use std::process::{Command, Output};

use fixtures::FixtureRepo;

fn run(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nexspec")).args(args).env("NEXSPEC_HOME", home).output().unwrap()
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

/// A web front end: depends on `@acme/shared` and calls `GET /contracts/{id}`.
fn web() -> (FixtureRepo, tempfile::TempDir) {
    let repo = FixtureRepo::init();
    repo.write_file(".gitignore", ".specs/.index/\n");
    repo.write_file("package.json", r#"{ "name": "web", "dependencies": { "@acme/shared": "*", "react": "*" } }"#);
    repo.write_file("src/client.ts", "export async function loadContract(id: string) {\n  return fetch(`/api/v1/contracts/${id}`);\n}\n");
    repo.write_file("src/util.ts", "export const x = 1;\n");
    repo.commit("init");
    let dir = tempfile::TempDir::new().unwrap();
    (repo, dir)
}

/// The service: publishes `@acme/shared` and serves `GET /contracts/{id}`.
fn service() -> FixtureRepo {
    let repo = FixtureRepo::init();
    repo.write_file(".gitignore", ".specs/.index/\n");
    repo.write_file("package.json", r#"{ "name": "@acme/shared", "version": "2.0.0" }"#);
    repo.write_file("openapi.json", r#"{ "openapi": "3.0.0", "paths": { "/contracts/{id}": { "get": { "operationId": "getContract" } } } }"#);
    repo.write_file("src/contract.ts", "export function getContract() { return 1; }\n");
    repo.commit("init");
    repo
}

fn sync(repo: &Path) {
    let o = Command::new(env!("CARGO_BIN_EXE_nexspec")).arg("--repo").arg(repo).arg("sync").output().unwrap();
    assert!(o.status.success(), "{}", err(&o));
}

#[test]
fn repositories_are_added_listed_replaced_and_removed() {
    let (web, home) = web();
    let service = service();
    sync(web.path());
    sync(service.path());

    let added = ok(run(home.path(), &["global", "add", web.path().to_str().unwrap(), "--as", "web"]));
    assert!(out(&added).contains("added web:"), "{}", out(&added));
    ok(run(home.path(), &["global", "add", service.path().to_str().unwrap(), "--as", "contracts"]));
    ok(run(home.path(), &["global", "add", web.path().to_str().unwrap(), "--as", "web"])); // same tag: replaced

    let list = out(&ok(run(home.path(), &["global", "list"])));
    assert_eq!(list.lines().count(), 2, "{list}");
    assert!(list.contains("web ") && list.contains("contracts "), "{list}");
    assert!(out(&ok(run(home.path(), &["global", "path"]))).trim().ends_with("global"));

    let bad = run(home.path(), &["global", "add", web.path().to_str().unwrap(), "--as", "a/b"]);
    assert_eq!(bad.status.code(), Some(1));
    assert!(err(&bad).contains("invalid tag"), "{}", err(&bad));

    ok(run(home.path(), &["global", "remove", "contracts"]));
    assert_eq!(out(&ok(run(home.path(), &["global", "list"]))).lines().count(), 1);
    assert_eq!(run(home.path(), &["global", "remove", "contracts"]).status.code(), Some(1), "removing twice is an error");
}

#[test]
fn a_repository_without_an_index_is_refused_with_the_fix() {
    let (web, home) = web();
    let refused = run(home.path(), &["global", "add", web.path().to_str().unwrap()]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(err(&refused).contains("nexspec sync"), "{}", err(&refused));
}

#[test]
fn global_queries_cross_repositories_and_show_where_each_node_lives() {
    let (web, home) = web();
    let service = service();
    sync(web.path());
    sync(service.path());
    ok(run(home.path(), &["global", "add", web.path().to_str().unwrap(), "--as", "web"]));
    ok(run(home.path(), &["global", "add", service.path().to_str().unwrap(), "--as", "contracts"]));

    // The package of one repository is used by the other.
    let package = out(&ok(run(home.path(), &["affected", "@acme/shared", "--global"])));
    assert!(package.contains("web"), "the web package depends on it: {package}");

    // The endpoint one repository serves is called by a file of the other.
    let endpoint = out(&ok(run(home.path(), &["affected", "GET /contracts/{}", "--global", "--depth", "3"])));
    assert!(endpoint.contains("web/src/client.ts"), "the caller is named with its repository: {endpoint}");

    // A path present in both repositories is ambiguous until a repository is chosen.
    let ambiguous = run(home.path(), &["explain", "package.json", "--global"]);
    assert!(!ambiguous.status.success() && err(&ambiguous).contains("web/package.json") && err(&ambiguous).contains("contracts/package.json"), "{}", err(&ambiguous));
    let scoped = out(&ok(run(home.path(), &["explain", "package.json", "--global", "--repo", "web"])));
    assert!(scoped.contains("web/package.json"), "{scoped}");

    let none = run(home.path(), &["affected", "x", "--global"]);
    assert!(!none.status.success());
    let empty_home = tempfile::TempDir::new().unwrap();
    let missing = run(empty_home.path(), &["affected", "x", "--global"]);
    assert!(err(&missing).contains("no global graph yet"), "{}", err(&missing));
}

#[test]
fn merge_graphs_unites_exports_with_tags_and_links_across_them() {
    let (web, home) = web();
    let service = service();
    sync(web.path());
    sync(service.path());
    let web_json = home.path().join("web.json");
    let service_json = home.path().join("contracts.json");
    for (repo, path) in [(web.path(), &web_json), (service.path(), &service_json)] {
        let o = Command::new(env!("CARGO_BIN_EXE_nexspec")).arg("--repo").arg(repo).args(["export", "--out"]).arg(path).output().unwrap();
        assert!(o.status.success(), "{}", err(&o));
    }
    let merged_path = home.path().join("all.graph.json");
    let merged = run(home.path(), &["merge-graphs", web_json.to_str().unwrap(), service_json.to_str().unwrap(), "--out", merged_path.to_str().unwrap()]);
    assert!(merged.status.success(), "{}", err(&merged));
    let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&merged_path).unwrap()).unwrap();
    let nodes = value["nodes"].as_array().unwrap();
    assert!(nodes.iter().any(|n| n["repo"] == "web") && nodes.iter().any(|n| n["repo"] == "contracts"));
    let labels: Vec<&str> = nodes.iter().map(|n| n["label"].as_str().unwrap()).collect();
    assert!(labels.contains(&"web/package.json") && labels.contains(&"contracts/package.json"), "same path, two nodes: {labels:?}");
    let cross: Vec<_> = value["edges"].as_array().unwrap().iter().filter(|e| e["confidence"] == "inferred" && (e["type"] == "depends_on" || e["type"] == "calls")).collect();
    assert!(cross.iter().any(|e| e["type"] == "depends_on") && cross.iter().any(|e| e["type"] == "calls"), "package and endpoint links: {cross:?}");

    // Deterministic, and the same merge can be fed to the global graph.
    let again = home.path().join("again.json");
    ok(run(home.path(), &["merge-graphs", web_json.to_str().unwrap(), service_json.to_str().unwrap(), "--out", again.to_str().unwrap()]));
    assert_eq!(std::fs::read_to_string(&merged_path).unwrap(), std::fs::read_to_string(&again).unwrap());

    let wrong = run(home.path(), &["merge-graphs", web_json.to_str().unwrap(), service_json.to_str().unwrap(), "--as", "only-one", "--out", again.to_str().unwrap()]);
    assert_eq!(wrong.status.code(), Some(1));
}

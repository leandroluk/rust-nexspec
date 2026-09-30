//! The versioned corpora must stay true to the repository they describe
//! (T-806, REQ-804): every expected file exists and every expected marker
//! is defined in a spec.

use std::path::Path;
use std::process::Command;

use nexspec::bench::metrics::Expectation;
use nexspec::bench::{Corpus, Kind};

fn tracked_files() -> Vec<String> {
    let out = Command::new("git")
        .args(["ls-files"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run git ls-files");
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap().lines().map(str::to_string).collect()
}

fn spec_text() -> String {
    let mut all = String::new();
    for file in tracked_files().into_iter().filter(|f| f.starts_with(".specs/") && f.ends_with(".md")) {
        all.push_str(&std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(&file)).unwrap_or_default());
    }
    all
}

#[test]
fn self_corpus_expectations_exist_in_the_repository() {
    let corpus = Corpus::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("bench/self.toml")).expect("bench/self.toml loads");
    assert!(corpus.queries.len() >= 20, "the corpus should have ~20 questions, has {}", corpus.queries.len());
    for kind in Kind::ALL {
        assert!(corpus.queries.iter().any(|q| q.kind == kind), "no {kind:?} question in the self corpus");
    }

    let files = tracked_files();
    let specs = spec_text();
    for query in &corpus.queries {
        for entry in &query.expect {
            match Expectation::parse(entry) {
                Expectation::Path(path) => assert!(
                    files.iter().any(|f| f == &path || f.ends_with(&format!("/{path}"))),
                    "{}: expected file {path:?} is not tracked",
                    query.id
                ),
                Expectation::Marker(marker) => assert!(
                    specs.contains(&marker),
                    "{}: marker {marker} is not defined in any .specs markdown",
                    query.id
                ),
                Expectation::Symbol(symbol) => {
                    let found = files
                        .iter()
                        .filter(|f| f.starts_with("src/") && f.ends_with(".rs"))
                        .any(|f| std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(f)).is_ok_and(|t| t.contains(&symbol)));
                    assert!(found, "{}: symbol {symbol} not found in src/", query.id);
                }
            }
        }
    }
}

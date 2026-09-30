//! `sync --embed` and `--similar` (T-1805, REQ-1808). Without the model they skip; with it (run the `#[ignore]`
//! test locally: `cargo test --test annotate_similar -- --ignored`) they make real vectors and `SimilarTo` edges.

mod fixtures;

use std::path::Path;
use std::process::{Command, Output};

use fixtures::FixtureRepo;

fn run(repo: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nexspec")).arg("--repo").arg(repo).args(args).output().unwrap()
}

#[cfg(feature = "full")]
fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn repo() -> FixtureRepo {
    let repo = FixtureRepo::init();
    repo.write_file(".gitignore", ".specs/.index/\n.specs/.cache/\n.models/\n");
    repo.write_file(
        "src/billing.ts",
        "export function chargeMonthlyInvoice() { return 1; }\nexport function chargeMonthlyInvoiceForResident() { return 2; }\nexport function renderLoginScreen() { return 3; }\n",
    );
    repo.write_file(".specs/feat/spec.md", "## Requirements\n\n- REQ-1: residents are charged every month for their invoices\n- REQ-2: residents can log in with email and password\n");
    repo.commit("init");
    repo
}

#[cfg(feature = "full")]
#[test]
fn without_the_model_embed_and_similar_say_so_and_change_nothing() {
    let repo = repo();
    let sync = run(repo.path(), &["sync", "--similar"]);
    assert!(sync.status.success(), "{}", String::from_utf8_lossy(&sync.stderr));
    assert!(out(&sync).contains("embed: skipped, the embedding model is not in .models/"), "{}", out(&sync));
    assert!(!out(&sync).contains("similar:"), "{}", out(&sync));
}

#[cfg(not(feature = "full"))]
#[test]
fn a_lean_build_refuses_embed_with_a_clear_message() {
    let repo = repo();
    let sync = run(repo.path(), &["sync", "--embed"]);
    assert_eq!(sync.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&sync.stderr).contains("lean build"), "{}", String::from_utf8_lossy(&sync.stderr));
}

/// The real model: needs `.models/` next to this crate (it is git-ignored, see the README).
#[cfg(feature = "full")]
#[test]
#[ignore]
fn with_the_model_nodes_get_real_vectors_and_similar_edges() {
    let models = Path::new(env!("CARGO_MANIFEST_DIR")).join(".models");
    if !models.join("model_quantized.onnx").is_file() {
        eprintln!("skipped: no model in {}", models.display());
        return;
    }
    let repo = repo();
    let target = repo.path().join(".models");
    std::fs::create_dir_all(&target).unwrap();
    for file in ["model_quantized.onnx", "tokenizer.json", "config.json"] {
        std::fs::copy(models.join(file), target.join(file)).unwrap();
    }
    let first = run(repo.path(), &["sync", "--similar", "--similar-threshold", "0.6"]);
    let text = out(&first);
    assert!(first.status.success() && text.contains("embed: ") && !text.contains("skipped"), "{text}{}", String::from_utf8_lossy(&first.stderr));
    assert!(text.contains("similar: ") && !text.contains("similar: 0 "), "{text}");

    let second = out(&run(repo.path(), &["sync", "--embed"]));
    assert!(second.contains("0 node(s) embedded") && second.contains("already up to date"), "only what changed is embedded again: {second}");

    let explain = out(&run(repo.path(), &["explain", "chargeMonthlyInvoice"]));
    assert!(explain.contains("## Similar (embeddings, inferred)") && explain.contains("chargeMonthlyInvoiceForResident"), "{explain}");
}

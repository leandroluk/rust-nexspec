//! Cost baselines over a fixture repository (T-804).

mod fixtures;

use fixtures::FixtureRepo;
use nexspec::GitSource;
use nexspec::bench::baselines::{BaselineError, RepoSnapshot};
use nexspec::token::budget::{CharHeuristicTokenizer, Tokenizer};

fn snapshot() -> (FixtureRepo, RepoSnapshot) {
    let repo = FixtureRepo::init();
    repo.write_file("src/a.ts", "export function outboxDispatch() {}\nconst x = 1;\n// outbox again\n");
    repo.write_file("src/b.ts", "export const unrelated = 2;\n");
    repo.write_file("docs/notes.md", "# Notes\n\nnothing about it\n");
    repo.write_file("assets/logo.bin", "\u{0}\u{1}\u{2}binary");
    repo.commit("chore: seed");
    let git = GitSource::open(repo.path()).unwrap();
    let snapshot = RepoSnapshot::load(&git).unwrap();
    (repo, snapshot)
}

#[test]
fn grep_output_matches_grep_rn_rows_and_counts_their_tokens() {
    let (_repo, snap) = snapshot();
    let out = snap.grep_output("outbox");
    assert_eq!(
        out,
        "src/a.ts:1:export function outboxDispatch() {}\nsrc/a.ts:3:// outbox again\n"
    );
    assert_eq!(snap.grep_tokens("outbox", &CharHeuristicTokenizer), CharHeuristicTokenizer.estimate(&out));
    assert_eq!(snap.grep_output("zzz-nothing"), "");
    assert_eq!(snap.grep_output(""), "", "an empty term must not dump the repository");
}

#[test]
fn binary_files_are_ignored_everywhere() {
    let (_repo, snap) = snapshot();
    assert_eq!(snap.file_count(), 3, "a.ts, b.ts, notes.md; logo.bin is skipped");
    assert_eq!(snap.grep_output("binary"), "");
}

#[test]
fn read_tokens_sums_the_expected_files_and_skips_non_path_entries() {
    let (repo, snap) = snapshot();
    let a = std::fs::read_to_string(repo.path().join("src/a.ts")).unwrap();
    let b = std::fs::read_to_string(repo.path().join("src/b.ts")).unwrap();
    let t = CharHeuristicTokenizer;

    let both = snap
        .read_tokens(&["a.ts".into(), "src/b.ts".into(), "outboxDispatch".into(), "REQ-1".into()], &t)
        .unwrap();
    assert_eq!(both, Some(t.estimate(&a) + t.estimate(&b)));

    let symbols_only = snap.read_tokens(&["outboxDispatch".into()], &t).unwrap();
    assert_eq!(symbols_only, None, "nothing to read when only symbols are expected");
}

#[test]
fn missing_expected_file_is_an_explicit_error() {
    let (_repo, snap) = snapshot();
    let err = snap.read_tokens(&["src/ghost.ts".into()], &CharHeuristicTokenizer).unwrap_err();
    assert!(matches!(&err, BaselineError::ExpectedFileMissing { expected } if expected == "src/ghost.ts"), "{err}");
}

#[test]
fn corpus_tokens_is_the_sum_over_all_tracked_text_files() {
    let (repo, snap) = snapshot();
    let t = CharHeuristicTokenizer;
    let expected: u32 = ["src/a.ts", "src/b.ts", "docs/notes.md"]
        .iter()
        .map(|p| t.estimate(&std::fs::read_to_string(repo.path().join(p)).unwrap()))
        .sum();
    assert_eq!(snap.corpus_tokens(&t), expected);
}

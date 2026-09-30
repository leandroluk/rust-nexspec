//! Scale smoke tests over the synthetic repository (T-904, REQ-903): the
//! co-change graph must stay bounded no matter how big one commit is.

mod fixtures;

use fixtures::synthetic::{SyntheticParams, SyntheticRepo};
use nexspec::git::GitSource;
use nexspec::git::cochange::CoChangeWindow;

#[test]
fn co_change_edges_stay_bounded_on_the_reference_shape() {
    let repo = SyntheticRepo::generate(&SyntheticParams::default());
    let git = GitSource::open(repo.path()).unwrap();

    let capped = git.co_change_edges(&CoChangeWindow::default()).unwrap().len();
    assert!(capped < 300_000, "capped co-change edges: {capped}");

    // Guard against the test passing vacuously: without the caps the same
    // history must blow past the bound (the 800-file commit alone is ~640k).
    let uncapped_window = CoChangeWindow {
        max_files_per_commit: usize::MAX,
        max_pairs_per_file: usize::MAX,
        ..CoChangeWindow::default()
    };
    let uncapped = git.co_change_edges(&uncapped_window).unwrap().len();
    assert!(uncapped > 600_000, "uncapped co-change edges: {uncapped}");
}

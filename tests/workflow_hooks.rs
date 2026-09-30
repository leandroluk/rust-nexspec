//! `nexspec hook install|uninstall|status` (T-1603, REQ-1602).

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

fn hook(repo: &FixtureRepo, name: &str) -> std::path::PathBuf {
    repo.path().join(".git").join("hooks").join(name)
}

#[test]
fn install_creates_the_hooks_and_is_idempotent() {
    let repo = FixtureRepo::init();
    let first = run(repo.path(), &["hook", "install"]);
    assert!(first.status.success());
    assert!(out(&first).contains("post-commit: installed"));
    let text = std::fs::read_to_string(hook(&repo, "post-commit")).unwrap();
    assert!(text.starts_with("#!/bin/sh"));
    assert!(text.contains("# >>> nexspec >>>") && text.contains("command -v nexspec"));

    let second = run(repo.path(), &["hook", "install"]);
    assert!(out(&second).contains("post-commit: already installed"));
    let again = std::fs::read_to_string(hook(&repo, "post-commit")).unwrap();
    assert_eq!(text, again, "a second install changes nothing");
    assert_eq!(again.matches("# >>> nexspec >>>").count(), 1);
}

#[test]
fn an_existing_hook_keeps_its_content_and_uninstall_removes_only_our_block() {
    let repo = FixtureRepo::init();
    let path = hook(&repo, "post-merge");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "#!/bin/sh\necho mine\n").unwrap();

    assert!(run(repo.path(), &["hook", "install"]).status.success());
    let installed = std::fs::read_to_string(&path).unwrap();
    assert!(installed.contains("echo mine") && installed.contains("# >>> nexspec >>>"));

    let status = out(&run(repo.path(), &["hook", "status"]));
    assert!(status.contains("post-merge: installed"), "{status}");

    assert!(run(repo.path(), &["hook", "uninstall"]).status.success());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "#!/bin/sh\necho mine\n", "the user's hook is back as it was");
    assert!(!hook(&repo, "post-commit").exists(), "a hook we created is deleted entirely");
    let status = out(&run(repo.path(), &["hook", "status"]));
    assert!(status.contains("post-merge: not installed"), "{status}");
}

//! `nexspec install|uninstall --platform …` (T-1605, REQ-1604).

use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

fn run(repo: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nexspec"))
        .arg("--repo")
        .arg(repo)
        .args(args)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .output()
        .unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

fn json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn backups(dir: &Path, stem: &str) -> usize {
    std::fs::read_dir(dir).unwrap().filter_map(Result::ok).filter(|e| e.file_name().to_string_lossy().starts_with(&format!("{stem}.bak-"))).count()
}

#[test]
fn install_keeps_other_servers_backs_up_and_is_idempotent() {
    let (repo, home) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let path = repo.path().join(".mcp.json");
    std::fs::write(&path, r#"{"mcpServers":{"other":{"command":"x"}},"extra":1}"#).unwrap();

    let first = run(repo.path(), home.path(), &["install", "--platform", "claude"]);
    assert!(first.status.success(), "{}", stderr(&first));
    assert!(stdout(&first).contains("updated"), "{}", stdout(&first));
    let value = json(&path);
    assert_eq!(value["mcpServers"]["other"]["command"], "x", "other entries survive");
    assert_eq!(value["extra"], 1);
    assert_eq!(value["mcpServers"]["nexspec"]["command"], "nexspec");
    assert_eq!(backups(repo.path(), ".mcp.json"), 1);

    let second = run(repo.path(), home.path(), &["install", "--platform", "claude"]);
    assert!(stdout(&second).contains("unchanged"), "{}", stdout(&second));
    assert_eq!(backups(repo.path(), ".mcp.json"), 1, "an unchanged install writes no backup");

    let gone = run(repo.path(), home.path(), &["uninstall", "--platform", "claude"]);
    assert!(gone.status.success());
    let value = json(&path);
    assert!(value["mcpServers"].get("nexspec").is_none());
    assert_eq!(value["mcpServers"]["other"]["command"], "x");
}

#[test]
fn dry_run_writes_nothing_and_each_project_platform_gets_its_own_file_and_shape() {
    let (repo, home) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let dry = run(repo.path(), home.path(), &["install", "--platform", "all", "--dry-run"]);
    assert!(dry.status.success(), "{}", stderr(&dry));
    assert!(stdout(&dry).contains("would create"));
    assert!(!repo.path().join(".mcp.json").exists());
    assert!(!stdout(&dry).contains("codex"), "codex needs an explicit user scope");

    assert!(run(repo.path(), home.path(), &["install", "--platform", "all"]).status.success());
    assert_eq!(json(&repo.path().join(".gemini/settings.json"))["mcpServers"]["nexspec"]["command"], "nexspec");
    assert_eq!(json(&repo.path().join(".cursor/mcp.json"))["mcpServers"]["nexspec"]["command"], "nexspec");
    let vscode = json(&repo.path().join(".vscode/mcp.json"));
    assert_eq!(vscode["servers"]["nexspec"]["type"], "stdio");
}

#[test]
fn codex_needs_a_user_scope_and_writes_toml_keeping_other_servers() {
    let (repo, home) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let refused = run(repo.path(), home.path(), &["install", "--platform", "codex"]);
    assert!(!refused.status.success());
    assert!(stderr(&refused).contains("--scope user"), "{}", stderr(&refused));

    let config = home.path().join(".codex").join("config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "model = \"x\"\n\n[mcp_servers.other]\ncommand = \"y\"\n").unwrap();
    let ok = run(repo.path(), home.path(), &["install", "--platform", "codex", "--scope", "user"]);
    assert!(ok.status.success(), "{}", stderr(&ok));
    let table: toml::Table = std::fs::read_to_string(&config).unwrap().parse().unwrap();
    assert_eq!(table["model"].as_str(), Some("x"));
    assert_eq!(table["mcp_servers"]["other"]["command"].as_str(), Some("y"));
    assert_eq!(table["mcp_servers"]["nexspec"]["command"].as_str(), Some("nexspec"));

    assert!(run(repo.path(), home.path(), &["uninstall", "--platform", "codex", "--scope", "user"]).status.success());
    let table: toml::Table = std::fs::read_to_string(&config).unwrap().parse().unwrap();
    assert!(table["mcp_servers"].get("nexspec").is_none());
    assert_eq!(table["mcp_servers"]["other"]["command"].as_str(), Some("y"));
}

#[test]
fn a_file_that_cannot_be_parsed_is_left_untouched_with_a_snippet() {
    let (repo, home) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let path = repo.path().join(".mcp.json");
    std::fs::write(&path, "{ // comments are not JSON\n}").unwrap();
    let out = run(repo.path(), home.path(), &["install", "--platform", "claude"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("left untouched") && stderr(&out).contains("mcpServers"), "{}", stderr(&out));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ // comments are not JSON\n}");
}

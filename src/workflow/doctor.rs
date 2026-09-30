//! `nexspec doctor` (REQ-1605 in `.specs/features/workflow-integration/spec.md`):
//! short checks, each with the command that fixes it. Exit code 5 only when
//! something *fails*; warnings (no hooks, agent not configured) are advice.

use std::path::Path;

use redb::Database;

use crate::engine::INDEX_FORMAT;
use crate::sync::{SyncLock, VersionPointer};
use crate::workflow::hooks::{self, HookState};
use crate::workflow::install::{self, Action, Platform, Scope};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone)]
pub struct Check {
    pub name: &'static str,
    pub level: Level,
    pub detail: String,
    /// The command (or step) that resolves a warning or failure.
    pub fix: Option<String>,
}

fn check(name: &'static str, level: Level, detail: impl Into<String>, fix: Option<&str>) -> Check {
    Check { name, level, detail: detail.into(), fix: fix.map(str::to_string) }
}

pub const EXIT_FAILURE: i32 = 5;

pub fn exit_code(checks: &[Check]) -> i32 {
    if checks.iter().any(|c| c.level == Level::Fail) { EXIT_FAILURE } else { 0 }
}

pub fn run(repo: &Path, index_dir: &Path, home: Option<&Path>) -> Vec<Check> {
    let mut checks = vec![build_check()];
    checks.push(model_check(repo));
    checks.push(index_check(index_dir));
    checks.push(wal_check(index_dir));
    checks.push(hooks_check(repo));
    checks.extend(mcp_checks(repo, home));
    checks.push(gitignore_check(repo));
    checks
}

fn build_check() -> Check {
    let flavor = if cfg!(feature = "full") { "full (vector search available)" } else { "lean (BM25 only)" };
    check("version", Level::Ok, format!("nexspec {} — {flavor}", env!("CARGO_PKG_VERSION")), None)
}

fn model_check(repo: &Path) -> Check {
    if !cfg!(feature = "full") {
        return check("model", Level::Ok, "not needed in a lean build", None);
    }
    let models = repo.join(".models");
    if models.join("model_quantized.onnx").is_file() && models.join("tokenizer.json").is_file() {
        check("model", Level::Ok, format!("embedding model found in {}", models.display()), None)
    } else {
        check(
            "model",
            Level::Warn,
            format!("no embedding model in {}; search falls back to BM25 only", models.display()),
            Some("put model_quantized.onnx and tokenizer.json under .models/ (see the README), or ignore it if BM25 is enough"),
        )
    }
}

fn index_check(index_dir: &Path) -> Check {
    let db_path = index_dir.join("metadata.redb");
    if !db_path.is_file() {
        return check("index", Level::Fail, "no index yet", Some("nexspec sync"));
    }
    let read = || -> Result<Option<u64>, String> {
        let _lock = SyncLock::acquire(index_dir, SyncLock::timeout_from_env()).map_err(|e| e.to_string())?;
        let db = Database::open(&db_path).map_err(|e| e.to_string())?;
        VersionPointer::new(&db).index_format().map_err(|e| e.to_string())
    };
    match read() {
        Ok(Some(INDEX_FORMAT)) => check("index", Level::Ok, format!("index format {INDEX_FORMAT}"), None),
        Ok(found) => check(
            "index",
            Level::Fail,
            format!("index format {} but this build expects {INDEX_FORMAT}", found.map_or("unknown".into(), |f| f.to_string())),
            Some("nexspec sync   (rebuilds the derived index)"),
        ),
        Err(e) => check("index", Level::Fail, format!("cannot read the index: {e}"), Some("stop the other nexspec process, or `nexspec sync` to rebuild")),
    }
}

fn wal_check(index_dir: &Path) -> Check {
    let pending = std::fs::metadata(index_dir.join("sync.wal")).map(|m| m.len() > 0).unwrap_or(false);
    if pending {
        check("wal", Level::Fail, "an earlier sync did not finish (pending WAL frames)", Some("nexspec sync --resume"))
    } else {
        check("wal", Level::Ok, "no pending WAL frames", None)
    }
}

fn hooks_check(repo: &Path) -> Check {
    match hooks::status(repo) {
        Ok(states) => {
            let missing: Vec<_> = states.iter().filter(|(_, s)| *s == HookState::NotInstalled).map(|(n, _)| *n).collect();
            if missing.is_empty() {
                check("hooks", Level::Ok, "post-commit, post-merge and post-checkout keep the index fresh", None)
            } else {
                check("hooks", Level::Warn, format!("not installed: {}", missing.join(", ")), Some("nexspec hook install"))
            }
        }
        Err(e) => check("hooks", Level::Warn, e.to_string(), None),
    }
}

fn mcp_checks(repo: &Path, home: Option<&Path>) -> Vec<Check> {
    let mut configured = Vec::new();
    for platform in Platform::ALL {
        let scope = if platform == Platform::Codex { Some(Scope::User) } else { None };
        // An install that would change nothing means the entry is already there.
        if let Ok(plan) = install::plan_install(platform, scope, repo, home)
            && plan.action == Action::Unchanged
        {
            configured.push(platform.name());
        }
    }
    if configured.is_empty() {
        vec![check(
            "mcp",
            Level::Warn,
            "no coding agent is configured to use `nexspec mcp`",
            Some("nexspec install --platform claude   (or gemini, cursor, vscode; codex with --scope user)"),
        )]
    } else {
        vec![check("mcp", Level::Ok, format!("configured for: {}", configured.join(", ")), None)]
    }
}

fn gitignore_check(repo: &Path) -> Check {
    let ignored = std::fs::read_to_string(repo.join(".gitignore"))
        .map(|t| t.lines().any(|l| matches!(l.trim().trim_end_matches('/'), ".specs/.index" | "/.specs/.index" | ".specs/.index/*")))
        .unwrap_or(false);
    if ignored {
        check("gitignore", Level::Ok, ".specs/.index/ is ignored by git", None)
    } else {
        check(
            "gitignore",
            Level::Warn,
            ".specs/.index/ is not in .gitignore (the index would show up as changes)",
            Some("echo '.specs/.index/' >> .gitignore"),
        )
    }
}

pub fn render(checks: &[Check]) -> String {
    let mut out = String::new();
    for c in checks {
        let tag = match c.level {
            Level::Ok => "ok  ",
            Level::Warn => "warn",
            Level::Fail => "FAIL",
        };
        out.push_str(&format!("[{tag}] {}: {}\n", c.name, c.detail));
        if let Some(fix) = &c.fix {
            out.push_str(&format!("       fix: {fix}\n"));
        }
    }
    out
}

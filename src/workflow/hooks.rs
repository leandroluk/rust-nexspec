//! Git hooks that keep the index fresh (REQ-1602 in
//! `.specs/features/workflow-integration/spec.md`).
//!
//! The hook gets a marked block *appended* to whatever is already there, so a
//! repository's own `post-commit` keeps working; uninstalling removes only that
//! block. The block runs `nexspec sync` in the background and only if the
//! binary is on the `PATH`, so a commit never waits for, or fails because of,
//! the index.

use std::path::{Path, PathBuf};

pub const HOOK_NAMES: [&str; 3] = ["post-commit", "post-merge", "post-checkout"];
const BEGIN: &str = "# >>> nexspec >>>";
const END: &str = "# <<< nexspec <<<";
const SHEBANG: &str = "#!/bin/sh";

#[derive(Debug, thiserror::Error)]
pub enum HookError {
    #[error("cannot open the git repository at {path}: {message}")]
    Open { path: String, message: String },
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// What happened (or would happen) to one hook file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookChange {
    Added,
    Unchanged,
    Removed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookState {
    Installed,
    /// The hook file exists without our block, or does not exist.
    NotInstalled,
}

fn block() -> String {
    format!(
        "{BEGIN}\n# Keeps the nexspec index up to date; remove with `nexspec hook uninstall`.\n\
         command -v nexspec >/dev/null 2>&1 && (nexspec sync >/dev/null 2>&1 &)\n{END}\n"
    )
}

/// `<common git dir>/hooks`: worktrees share it with the main checkout.
pub fn hooks_dir(repo: &Path) -> Result<PathBuf, HookError> {
    let opened = gix::open(repo).map_err(|e| HookError::Open { path: repo.display().to_string(), message: e.to_string() })?;
    Ok(opened.common_dir().join("hooks"))
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> HookError + '_ {
    move |source| HookError::Io { path: path.display().to_string(), source }
}

fn read(path: &Path) -> Result<Option<String>, HookError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text.replace("\r\n", "\n"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io(path)(e)),
    }
}

fn has_block(text: &str) -> bool {
    text.contains(BEGIN) && text.contains(END)
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), HookError> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path).map_err(io(path))?.permissions();
    permissions.set_mode(permissions.mode() | 0o111);
    std::fs::set_permissions(path, permissions).map_err(io(path))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), HookError> {
    Ok(())
}

pub fn install(repo: &Path) -> Result<Vec<(&'static str, HookChange)>, HookError> {
    let dir = hooks_dir(repo)?;
    std::fs::create_dir_all(&dir).map_err(io(&dir))?;
    let mut changes = Vec::new();
    for name in HOOK_NAMES {
        let path = dir.join(name);
        let change = match read(&path)? {
            Some(text) if has_block(&text) => HookChange::Unchanged,
            Some(text) => {
                let mut updated = text;
                if !updated.ends_with('\n') {
                    updated.push('\n');
                }
                updated.push('\n');
                updated.push_str(&block());
                std::fs::write(&path, updated).map_err(io(&path))?;
                make_executable(&path)?;
                HookChange::Added
            }
            None => {
                std::fs::write(&path, format!("{SHEBANG}\n\n{}", block())).map_err(io(&path))?;
                make_executable(&path)?;
                HookChange::Added
            }
        };
        changes.push((name, change));
    }
    Ok(changes)
}

/// Removes only the marked block; a hook that held nothing but our block (and the
/// shebang we wrote) is deleted.
pub fn uninstall(repo: &Path) -> Result<Vec<(&'static str, HookChange)>, HookError> {
    let dir = hooks_dir(repo)?;
    let mut changes = Vec::new();
    for name in HOOK_NAMES {
        let path = dir.join(name);
        let change = match read(&path)? {
            Some(text) if has_block(&text) => {
                let remaining = strip_block(&text);
                if remaining.trim().is_empty() || remaining.trim() == SHEBANG {
                    std::fs::remove_file(&path).map_err(io(&path))?;
                } else {
                    std::fs::write(&path, remaining).map_err(io(&path))?;
                }
                HookChange::Removed
            }
            _ => HookChange::Unchanged,
        };
        changes.push((name, change));
    }
    Ok(changes)
}

fn strip_block(text: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line.trim_end() == BEGIN {
            inside = true;
            // The blank line `install` put before the block goes with it.
            while kept.last().is_some_and(|l| l.trim().is_empty()) {
                kept.pop();
            }
            continue;
        }
        if inside {
            if line.trim_end() == END {
                inside = false;
            }
            continue;
        }
        kept.push(line);
    }
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

pub fn status(repo: &Path) -> Result<Vec<(&'static str, HookState)>, HookError> {
    let dir = hooks_dir(repo)?;
    HOOK_NAMES
        .iter()
        .map(|name| {
            let installed = read(&dir.join(name))?.is_some_and(|text| has_block(&text));
            Ok((*name, if installed { HookState::Installed } else { HookState::NotInstalled }))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_block_keeps_everything_else() {
        let text = format!("#!/bin/sh\necho mine\n\n{}", block());
        assert_eq!(strip_block(&text), "#!/bin/sh\necho mine\n");
    }

    #[test]
    fn strip_block_of_a_hook_we_created_leaves_only_the_shebang() {
        let text = format!("{SHEBANG}\n\n{}", block());
        assert_eq!(strip_block(&text).trim(), SHEBANG);
    }
}

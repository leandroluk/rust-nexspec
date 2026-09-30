//! Registering the MCP server with coding agents (REQ-1604 in
//! `.specs/features/workflow-integration/spec.md`).
//!
//! Each platform keeps the list of MCP servers in its own file. `plan_*` only
//! computes what would change, so `--dry-run` and the real run share one code
//! path; `apply` writes a timestamped backup before touching an existing file.
//! Entries other than `nexspec` are never modified. Files that cannot be
//! parsed are left alone and reported.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

pub const SERVER_NAME: &str = "nexspec";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Claude,
    Gemini,
    Cursor,
    Vscode,
    Codex,
}

impl Platform {
    pub const ALL: [Platform; 5] = [Self::Claude, Self::Gemini, Self::Cursor, Self::Vscode, Self::Codex];

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "claude" | "claude-code" => Some(Self::Claude),
            "gemini" => Some(Self::Gemini),
            "cursor" => Some(Self::Cursor),
            "vscode" | "vs-code" => Some(Self::Vscode),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Gemini => "gemini",
            Self::Cursor => "cursor",
            Self::Vscode => "vscode",
            Self::Codex => "codex",
        }
    }

    fn is_user_level(self) -> bool {
        self == Self::Codex
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Project,
    User,
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("{0}")]
    Invalid(String),
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "{path} could not be parsed ({reason}); it was left untouched. Add the `nexspec` server by hand: {snippet}"
    )]
    Unparseable { path: String, reason: String, snippet: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Create,
    Update,
    Remove,
    /// Nothing to do: already as requested.
    Unchanged,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub platform: Platform,
    pub path: PathBuf,
    pub action: Action,
    /// Full new content of the file (`None` when unchanged).
    pub content: Option<String>,
    pub existed: bool,
    /// Remarks for the user (e.g. comments in a rewritten file).
    pub notes: Vec<String>,
}

fn invalid(message: impl Into<String>) -> InstallError {
    InstallError::Invalid(message.into())
}

fn config_path(platform: Platform, scope: Option<Scope>, repo: &Path, home: Option<&Path>) -> Result<PathBuf, InstallError> {
    match (platform.is_user_level(), scope) {
        (true, Some(Scope::User)) => {
            let home = home.ok_or_else(|| invalid("cannot find the home directory (set HOME)"))?;
            Ok(home.join(".codex").join("config.toml"))
        }
        (true, _) => Err(invalid("codex only has a user-level configuration (~/.codex/config.toml): pass `--scope user`")),
        (false, Some(Scope::User)) => Err(invalid(format!(
            "{} is configured per project by nexspec; drop `--scope user`",
            platform.name()
        ))),
        (false, _) => Ok(repo.join(match platform {
            Platform::Claude => ".mcp.json",
            Platform::Gemini => ".gemini/settings.json",
            Platform::Cursor => ".cursor/mcp.json",
            Platform::Vscode => ".vscode/mcp.json",
            Platform::Codex => unreachable!(),
        })),
    }
}

/// Key that holds the server map in the platform's JSON file.
fn servers_key(platform: Platform) -> &'static str {
    if platform == Platform::Vscode { "servers" } else { "mcpServers" }
}

fn server_entry(platform: Platform) -> Value {
    let mut entry = json!({ "command": "nexspec", "args": ["mcp", "--watch"] });
    if platform == Platform::Vscode {
        entry["type"] = json!("stdio");
    }
    entry
}

fn snippet(platform: Platform) -> String {
    if platform == Platform::Codex {
        return "[mcp_servers.nexspec]\ncommand = \"nexspec\"\nargs = [\"mcp\", \"--watch\"]".to_string();
    }
    json!({ servers_key(platform): { SERVER_NAME: server_entry(platform) } }).to_string()
}

fn read_existing(path: &Path) -> Result<Option<String>, InstallError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(InstallError::Io { path: path.display().to_string(), source }),
    }
}

pub fn plan_install(platform: Platform, scope: Option<Scope>, repo: &Path, home: Option<&Path>) -> Result<Plan, InstallError> {
    plan(platform, scope, repo, home, true)
}

pub fn plan_uninstall(platform: Platform, scope: Option<Scope>, repo: &Path, home: Option<&Path>) -> Result<Plan, InstallError> {
    plan(platform, scope, repo, home, false)
}

fn plan(platform: Platform, scope: Option<Scope>, repo: &Path, home: Option<&Path>, install: bool) -> Result<Plan, InstallError> {
    let path = config_path(platform, scope, repo, home)?;
    let existing = read_existing(&path)?;
    let unparseable = |reason: String| InstallError::Unparseable {
        path: path.display().to_string(),
        reason,
        snippet: snippet(platform),
    };
    let mut notes = Vec::new();

    let (changed, rendered) = if platform == Platform::Codex {
        let mut table: toml::Table = match existing.as_deref() {
            Some(text) if !text.trim().is_empty() => text.parse().map_err(|e: toml::de::Error| unparseable(e.to_string()))?,
            _ => toml::Table::new(),
        };
        let servers = table
            .entry("mcp_servers")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .ok_or_else(|| unparseable("`mcp_servers` is not a table".to_string()))?;
        let desired = toml::Value::try_from(json!({ "command": "nexspec", "args": ["mcp", "--watch"] })).expect("static value");
        let changed = if install {
            servers.insert(SERVER_NAME.to_string(), desired.clone()).as_ref() != Some(&desired)
        } else {
            servers.remove(SERVER_NAME).is_some()
        };
        if servers.is_empty() && !install {
            table.remove("mcp_servers");
        }
        if changed && existing.as_deref().is_some_and(|t| t.contains('#')) {
            notes.push("comments in the file are not preserved (a backup is written)".to_string());
        }
        (changed, toml::to_string_pretty(&table).map_err(|e| unparseable(e.to_string()))?)
    } else {
        let mut root: Value = match existing.as_deref() {
            Some(text) if !text.trim().is_empty() => serde_json::from_str(text).map_err(|e| unparseable(e.to_string()))?,
            _ => Value::Object(Map::new()),
        };
        let object = root.as_object_mut().ok_or_else(|| unparseable("the top level is not an object".to_string()))?;
        let key = servers_key(platform);
        let servers = object
            .entry(key)
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .ok_or_else(|| unparseable(format!("`{key}` is not an object")))?;
        let changed = if install {
            let desired = server_entry(platform);
            servers.insert(SERVER_NAME.to_string(), desired.clone()).as_ref() != Some(&desired)
        } else {
            servers.remove(SERVER_NAME).is_some()
        };
        if servers.is_empty() && !install {
            object.remove(key);
        }
        let mut text = serde_json::to_string_pretty(&root).expect("serialisable");
        text.push('\n');
        (changed, text)
    };

    let existed = existing.is_some();
    let (action, content) = match (install, changed, existed) {
        (_, false, _) => (Action::Unchanged, None),
        (true, true, false) => (Action::Create, Some(rendered)),
        (true, true, true) => (Action::Update, Some(rendered)),
        (false, true, _) => (Action::Remove, Some(rendered)),
    };
    Ok(Plan { platform, path, action, content, existed, notes })
}

/// Writes the plan; returns the backup path when an existing file was replaced.
pub fn apply(plan: &Plan) -> Result<Option<PathBuf>, InstallError> {
    let Some(content) = &plan.content else { return Ok(None) };
    let io = |source| InstallError::Io { path: plan.path.display().to_string(), source };
    if let Some(parent) = plan.path.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    let backup = if plan.existed {
        let mut name = plan.path.file_name().unwrap_or_default().to_os_string();
        name.push(format!(".bak-{}", timestamp()));
        let backup = plan.path.with_file_name(name);
        std::fs::copy(&plan.path, &backup).map_err(io)?;
        Some(backup)
    } else {
        None
    };
    std::fs::write(&plan.path, content).map_err(io)?;
    Ok(backup)
}

/// `YYYYMMDDhhmmss` in UTC (no date crate: civil-from-days by Howard Hinnant).
fn timestamp() -> String {
    let seconds = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rest) = ((seconds / 86_400) as i64, seconds % 86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}{month:02}{day:02}{:02}{:02}{:02}", rest / 3600, rest % 3600 / 60, rest % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_has_fourteen_digits() {
        let t = timestamp();
        assert_eq!(t.len(), 14);
        assert!(t.chars().all(|c| c.is_ascii_digit()));
        assert!(t.starts_with("20"));
    }
}

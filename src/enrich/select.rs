//! Which files `enrich` may send, what of them, and in which order
//! (REQ-1903, REQ-1906 in `.specs/features/retrieval-enrichment/spec.md`).
//!
//! Nothing here touches the network. The rule of thumb: when in doubt, the
//! file does not leave the machine.

use std::path::Path;

use ignore::WalkBuilder;

/// Characters of a file sent by default (REQ-1903).
pub const DEFAULT_SNIPPET_CHARS: usize = 2500;

const EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "go", "rs", "md"];
const SKIPPED_DIRS: &[&str] = &[".git", ".specs/.index", ".specs/.cache", ".models", "target", "node_modules", "dist", "build", "coverage", ".next", "vendor"];

/// Files that never leave the machine, whatever their extension says.
fn is_sensitive_name(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    lower == ".env"
        || lower.starts_with(".env.")
        || lower.ends_with(".pem")
        || lower.ends_with(".key")
        || lower.ends_with(".p12")
        || lower.ends_with(".pfx")
        || lower.starts_with("id_rsa")
        || lower.starts_with("id_ed25519")
        || lower.starts_with("id_ecdsa")
        || lower.starts_with("id_dsa")
}

fn is_generated_name(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    lower.ends_with(".min.js") || lower.ends_with(".d.ts") || lower.contains(".generated.") || lower.ends_with(".pb.go") || lower.ends_with("_generated.go")
}

/// Repository-relative paths (forward slashes) of the files worth summarising,
/// sorted. Honours `.gitignore`.
pub fn eligible_files(repo: &Path) -> Vec<String> {
    let mut files = Vec::new();
    for entry in WalkBuilder::new(repo).hidden(false).git_global(false).build().flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(repo) else { continue };
        let relative = relative.to_string_lossy().replace('\\', "/");
        let Some(name) = relative.rsplit('/').next() else { continue };
        if is_sensitive_name(name) || is_generated_name(name) {
            continue;
        }
        let Some(extension) = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()) else { continue };
        if !EXTENSIONS.contains(&extension.as_str()) {
            continue;
        }
        let in_skipped_dir = SKIPPED_DIRS.iter().any(|dir| relative == *dir || relative.starts_with(&format!("{dir}/")))
            || relative.split('/').any(|part| part == "node_modules" || part == ".git");
        if !in_skipped_dir {
            files.push(relative);
        }
    }
    files.sort();
    files
}

/// Names the kind of secret found in `text`, if any. Deliberately blunt: a false
/// positive costs one summary, a false negative costs a credential.
pub fn find_secret(text: &str) -> Option<&'static str> {
    if text.contains("-----BEGIN") && text.contains("PRIVATE KEY-----") {
        return Some("private key");
    }
    for token in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-')) {
        let has_prefix_body = |prefix: &str, min: usize| token.strip_prefix(prefix).is_some_and(|rest| rest.len() >= min);
        if has_prefix_body("AKIA", 16) && token.len() == 20 && token[4..].chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
            return Some("AWS access key");
        }
        if has_prefix_body("AIza", 35) {
            return Some("Google API key");
        }
        if has_prefix_body("ghp_", 30) || has_prefix_body("gho_", 30) || has_prefix_body("ghs_", 30) || has_prefix_body("github_pat_", 30) {
            return Some("GitHub token");
        }
        if has_prefix_body("sk-", 20) {
            return Some("API secret key");
        }
        if token.starts_with("xox") && token.len() > 20 && token.as_bytes().get(3).is_some_and(|c| b"baprs".contains(c)) {
            return Some("Slack token");
        }
    }
    for line in text.lines() {
        if let Some(kind) = secret_assignment(line) {
            return Some(kind);
        }
    }
    None
}

/// `password = "hunter2hunter2"`, `apiKey: 'abcdef123456'`, … with a literal, not a reference.
fn secret_assignment(line: &str) -> Option<&'static str> {
    const NAMES: &[&str] = &["password", "passwd", "secret", "api_key", "apikey", "api-key", "private_key", "access_token", "auth_token", "client_secret"];
    let lower = line.to_ascii_lowercase();
    let name = NAMES.iter().find(|n| lower.contains(*n))?;
    let after = &line[lower.find(name)? + name.len()..];
    let mut chars = after.trim_start_matches(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '"' || c == '\'' || c == ' ');
    chars = chars.trim_start();
    let rest = chars.strip_prefix('=').or_else(|| chars.strip_prefix(':'))?;
    let rest = rest.trim_start();
    let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'' || *c == '`')?;
    let literal = rest[1..].split(quote).next()?;
    let looks_like_reference = literal.contains("${") || literal.contains("process.env") || literal.contains("os.environ") || literal.contains("{{");
    if literal.len() >= 8 && !looks_like_reference && !literal.contains(' ') {
        Some("hard-coded credential")
    } else {
        None
    }
}

/// The first `max_chars` characters, cut at a line boundary when one is close.
pub fn snippet(content: &str, max_chars: usize) -> &str {
    if content.chars().count() <= max_chars {
        return content;
    }
    let cut = content.char_indices().nth(max_chars).map_or(content.len(), |(i, _)| i);
    let head = &content[..cut];
    match head.rfind('\n') {
        Some(newline) if newline > cut / 2 => &head[..newline],
        _ => head,
    }
}

/// What the importance order is made of (REQ-1906).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub path: String,
    /// Edges of the file and its symbols, never counting co-change.
    pub degree: u32,
    /// Requirements/tasks that point at the file.
    pub requirement_links: u32,
    /// Number of co-change partners.
    pub cochange: u32,
}

impl Candidate {
    pub fn score(&self) -> u64 {
        u64::from(self.degree) + 50 * u64::from(self.requirement_links) + u64::from(self.cochange)
    }
}

/// Most important first; equal scores keep path order so the pass is repeatable.
pub fn order_by_importance(mut candidates: Vec<Candidate>) -> Vec<Candidate> {
    candidates.sort_by(|a, b| b.score().cmp(&a.score()).then_with(|| a.path.cmp(&b.path)));
    candidates
}

/// `--top` in both spellings: `20%` of the files, or an absolute `300`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Top {
    Percent(u8),
    Count(usize),
}

impl Top {
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        if let Some(percent) = text.strip_suffix('%') {
            let value: u8 = percent.trim().parse().map_err(|_| format!("`{text}` is not a percentage"))?;
            if value == 0 || value > 100 {
                return Err("--top percentage must be between 1 and 100".to_string());
            }
            return Ok(Self::Percent(value));
        }
        let count: usize = text.parse().map_err(|_| format!("`{text}` is neither N nor P%"))?;
        if count == 0 {
            return Err("--top must be at least 1".to_string());
        }
        Ok(Self::Count(count))
    }

    pub fn limit(self, total: usize) -> usize {
        match self {
            Self::Percent(p) => (total * usize::from(p)).div_ceil(100).max(1).min(total),
            Self::Count(n) => n.min(total),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(dir: &Path, rel: &str, content: &str) {
        let full = dir.join(rel);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, content).unwrap();
    }

    #[test]
    fn only_supported_non_sensitive_non_generated_files_are_eligible() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        write(dir.path(), ".gitignore", "ignored/\n");
        for rel in [
            "src/a.ts", "src/b.tsx", "README.md", "svc/main.go", "lib/x.py", "src/lib.rs", ".env", ".env.local", "certs/server.pem", "certs/tls.key", "id_rsa",
            "dist/bundle.js", "node_modules/p/index.js", "ignored/c.ts", "src/types.d.ts", "src/app.min.js", "logo.png", "data.json", "api/user.generated.ts",
        ] {
            write(dir.path(), rel, "x");
        }
        assert_eq!(eligible_files(dir.path()), ["README.md", "lib/x.py", "src/a.ts", "src/b.tsx", "src/lib.rs", "svc/main.go"]);
    }

    #[test]
    fn secrets_are_found_and_ordinary_code_is_not() {
        assert_eq!(find_secret("-----BEGIN RSA PRIVATE KEY-----\nabc\n-----END RSA PRIVATE KEY-----"), Some("private key"));
        // Built at run time: a literal that looks like a real key is blocked by push protection.
        let aws = format!("const k = '{}{}';", "AKIA", "ABCDEFGHIJKLMNOP");
        let google = format!("key: {}{}", "AIza", "SyA1234567890abcdefghijklmnopqrstuvw");
        let github = format!("token={}{}", "ghp_", "abcdefghijklmnopqrstuvwxyz0123456789");
        assert_eq!(find_secret(&aws), Some("AWS access key"));
        assert_eq!(find_secret(&google), Some("Google API key"));
        assert_eq!(find_secret(&github), Some("GitHub token"));
        assert_eq!(find_secret("const password = \"hunter2hunter2\";"), Some("hard-coded credential"));
        assert_eq!(find_secret("apiKey: 'abcdef123456'"), Some("hard-coded credential"));

        assert_eq!(find_secret("const password = process.env.DB_PASSWORD;"), None);
        assert_eq!(find_secret("password = \"${DB_PASSWORD}\""), None);
        assert_eq!(find_secret("function hashPassword(password: string) { return hash(password); }"), None);
        assert_eq!(find_secret("label: 'Reset your password'"), None, "a sentence is not a credential");
        assert_eq!(find_secret("// the sky is blue\nexport class Invoice {}"), None);
    }

    #[test]
    fn the_snippet_is_bounded_and_prefers_a_line_boundary() {
        let text = "aaaa\nbbbb\ncccc\ndddd\n";
        assert_eq!(snippet(text, 100), text);
        assert_eq!(snippet(text, 12), "aaaa\nbbbb");
        assert_eq!(snippet("ééééé", 3), "ééé", "cuts on a character boundary");
    }

    #[test]
    fn importance_is_degree_plus_requirement_links_plus_cochange_with_a_stable_tie_break() {
        let c = |path: &str, degree, req, co| Candidate { path: path.into(), degree, requirement_links: req, cochange: co };
        let ordered = order_by_importance(vec![c("z.ts", 10, 0, 0), c("a.ts", 10, 0, 0), c("spec-linked.ts", 1, 1, 0), c("hub.ts", 60, 0, 5), c("leaf.ts", 0, 0, 0)]);
        let paths: Vec<_> = ordered.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(paths, ["hub.ts", "spec-linked.ts", "a.ts", "z.ts", "leaf.ts"]);
    }

    #[test]
    fn top_accepts_a_percentage_or_a_count() {
        assert_eq!(Top::parse("20%").unwrap(), Top::Percent(20));
        assert_eq!(Top::parse("300").unwrap(), Top::Count(300));
        assert!(Top::parse("0").is_err() && Top::parse("101%").is_err() && Top::parse("x").is_err());
        assert_eq!(Top::Percent(20).limit(10), 2);
        assert_eq!(Top::Percent(1).limit(10), 1, "never rounds down to nothing");
        assert_eq!(Top::Count(300).limit(10), 10);
    }
}

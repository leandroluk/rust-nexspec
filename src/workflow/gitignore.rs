//! `.gitignore` entries nexspec needs: the index and the enrichment cache are
//! local, derived data and must not show up as changes (REQ-1905, REQ-1605).

use std::path::Path;

pub const INDEX_ENTRY: &str = ".specs/.index/";
pub const CACHE_ENTRY: &str = ".specs/.cache/";
/// Raw notes of the work memory stay local; `LESSONS.md` next to them is what gets shared.
pub const NOTES_ENTRY: &str = ".specs/.memory/notes/";

/// Whether `.gitignore` already covers `entry` (with or without the trailing slash or a leading `/`).
pub fn covers(repo: &Path, entry: &str) -> bool {
    let wanted = entry.trim_matches('/');
    std::fs::read_to_string(repo.join(".gitignore")).is_ok_and(|text| {
        text.lines().any(|line| {
            let line = line.trim().trim_start_matches('/').trim_end_matches("/*").trim_end_matches('/');
            line == wanted
        })
    })
}

/// Appends the entries that are missing (creating `.gitignore` if needed); returns the ones added.
pub fn ensure(repo: &Path, entries: &[&str]) -> std::io::Result<Vec<String>> {
    let missing: Vec<&str> = entries.iter().copied().filter(|e| !covers(repo, e)).collect();
    if missing.is_empty() {
        return Ok(Vec::new());
    }
    let path = repo.join(".gitignore");
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    for entry in &missing {
        text.push_str(entry);
        text.push('\n');
    }
    std::fs::write(&path, text)?;
    Ok(missing.into_iter().map(str::to_string).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn ensure_adds_only_what_is_missing_and_is_idempotent() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "target\n/.specs/.index\n").unwrap();
        let added = ensure(dir.path(), &[INDEX_ENTRY, CACHE_ENTRY]).unwrap();
        assert_eq!(added, [CACHE_ENTRY]);
        assert!(ensure(dir.path(), &[INDEX_ENTRY, CACHE_ENTRY]).unwrap().is_empty());
        let text = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert_eq!(text, "target\n/.specs/.index\n.specs/.cache/\n");
    }

    #[test]
    fn a_missing_gitignore_is_created_and_a_missing_final_newline_is_handled() {
        let dir = TempDir::new().unwrap();
        assert_eq!(ensure(dir.path(), &[INDEX_ENTRY]).unwrap(), [INDEX_ENTRY]);
        std::fs::write(dir.path().join(".gitignore"), "a").unwrap();
        ensure(dir.path(), &[CACHE_ENTRY]).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join(".gitignore")).unwrap(), "a\n.specs/.cache/\n");
    }
}

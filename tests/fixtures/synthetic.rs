//! Deterministic synthetic repository generator (REQ-901 in
//! `.specs/features/performance-guard/spec.md`). Builds a large Git history
//! in one `git fast-import` process (design.md D1) so the ~1.3k-file /
//! ~160-commit reference shape takes seconds, not minutes.
//!
//! Determinism covers content and topology (paths, blobs, commit order and
//! sizes) -- only commit timestamps depend on `now`, because the co-change
//! window is relative to the current time (design.md D2).
#![allow(dead_code)]

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use super::FixtureRepo;

#[derive(Debug, Clone)]
pub struct SyntheticParams {
    /// Total files (TypeScript + Markdown) present at HEAD.
    pub files: usize,
    /// How many of `files` are Markdown specs (the rest are `.ts`).
    pub markdown_files: usize,
    /// Total commits, including the big one.
    pub commits: usize,
    /// Files added by the first ("big") commit.
    pub big_commit_files: usize,
    pub seed: u64,
}

impl Default for SyntheticParams {
    /// Mirrors the reference repository: ~1.3k files, ~160 commits, one
    /// commit touching ~800 files.
    fn default() -> Self {
        Self {
            files: 1300,
            markdown_files: 100,
            commits: 160,
            big_commit_files: 800,
            seed: 0x5eed_0001,
        }
    }
}

impl SyntheticParams {
    /// Proportionally smaller (or larger) shape; `1.0` is the default.
    pub fn scaled(&self, factor: f64) -> Self {
        let scale = |n: usize, min: usize| (((n as f64) * factor).round() as usize).max(min);
        let files = scale(self.files, 8);
        let big = scale(self.big_commit_files, 2).min(files - 1);
        Self {
            files,
            markdown_files: scale(self.markdown_files, 1).min(files - 1),
            commits: scale(self.commits, 3),
            big_commit_files: big,
            seed: self.seed,
        }
    }

    /// Reads `NEXSPEC_SYNTH_SCALE` (default `1.0`).
    pub fn from_env() -> Self {
        let factor = std::env::var("NEXSPEC_SYNTH_SCALE")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(1.0);
        Self::default().scaled(factor)
    }
}

pub struct SyntheticRepo {
    pub repo: FixtureRepo,
    pub params: SyntheticParams,
}

impl SyntheticRepo {
    pub fn path(&self) -> &Path {
        self.repo.path()
    }

    pub fn generate(params: &SyntheticParams) -> Self {
        assert!(params.commits >= 2, "need the big commit plus at least one more");
        assert!(params.big_commit_files >= 1 && params.big_commit_files < params.files);
        assert!(params.markdown_files < params.files);

        let repo = FixtureRepo::init();
        run_git(repo.path(), &["config", "core.autocrlf", "false"]);

        let stream = build_stream(params);
        let mut child = Command::new("git")
            .args(["fast-import", "--quiet", "--force"])
            .current_dir(repo.path())
            .stdin(Stdio::piped())
            .spawn()
            .expect("spawn git fast-import");
        child
            .stdin
            .take()
            .expect("fast-import stdin")
            .write_all(&stream)
            .expect("write fast-import stream");
        let status = child.wait().expect("wait for git fast-import");
        assert!(status.success(), "git fast-import failed");

        // fast-import only writes objects and refs; materialize the working tree.
        run_git(repo.path(), &["reset", "--hard", "--quiet", "HEAD"]);

        Self {
            repo,
            params: params.clone(),
        }
    }
}

fn run_git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap_or_else(|e| panic!("failed to run git {args:?}: {e}"));
    assert!(status.success(), "git {args:?} failed");
}

/// xorshift64* -- tiny, dependency-free, stable across platforms.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn is_markdown(index: usize, params: &SyntheticParams) -> bool {
    // Spread the Markdown files evenly through the index space.
    let stride = params.files / params.markdown_files.max(1);
    params.markdown_files > 0
        && stride > 0
        && index.is_multiple_of(stride)
        && index / stride < params.markdown_files
}

fn path_of(index: usize, params: &SyntheticParams) -> String {
    if is_markdown(index, params) {
        format!(".specs/features/feat{index:04}/spec.md")
    } else {
        format!("src/m{:02}/file{index:04}.ts", index / 50)
    }
}

fn initial_content(index: usize, params: &SyntheticParams) -> String {
    if is_markdown(index, params) {
        return format!(
            "# Spec: synthetic feature {index}\n\n## Requirements\n\n- REQ-{index:04}: synthetic requirement number {index}\n"
        );
    }
    // Import an earlier non-Markdown file (already created: lower index).
    let import = (0..index)
        .rev()
        .step_by(7)
        .find(|j| !is_markdown(*j, params))
        .map(|j| {
            format!(
                "import {{ Cls{j:04} }} from '../m{:02}/file{j:04}';\n",
                j / 50
            )
        })
        .unwrap_or_default();
    format!(
        "{import}export interface Iface{index:04} {{ id: number }}\n\nexport class Cls{index:04} {{\n  method{index:04}(): number {{\n    return {index};\n  }}\n}}\n"
    )
}

fn modified_content(index: usize, params: &SyntheticParams, revision: usize) -> String {
    let mut content = initial_content(index, params);
    content.push_str(&format!("// revision {revision}\n"));
    content
}

fn build_stream(params: &SyntheticParams) -> Vec<u8> {
    let mut rng = Rng(params.seed | 1);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time after epoch")
        .as_secs();
    // One commit per hour, ending an hour ago: always inside the 6-month window.
    let first_ts = now - (params.commits as u64 + 1) * 3600;

    let mut out: Vec<u8> = Vec::new();
    let mut created = 0usize; // files 0..created exist
    let mut revision = vec![0usize; params.files];

    for commit in 0..params.commits {
        let mut changes: Vec<(usize, usize)> = Vec::new(); // (file index, revision)
        if commit == 0 {
            for i in 0..params.big_commit_files {
                changes.push((i, 0));
            }
            created = params.big_commit_files;
        } else {
            let remaining_commits = params.commits - commit;
            let remaining_files = params.files - created;
            let add = remaining_files.div_ceil(remaining_commits);
            for i in created..created + add {
                changes.push((i, 0));
            }
            created += add;
            // A few edits to existing files; skew toward a hot subset so
            // repeated co-change pairs exist.
            let edits = 1 + rng.below(3);
            for _ in 0..edits {
                let hot = created.min(40);
                let target = if rng.below(2) == 0 { rng.below(hot) } else { rng.below(created) };
                if !changes.iter().any(|(i, _)| *i == target) {
                    revision[target] += 1;
                    changes.push((target, revision[target]));
                }
            }
        }

        let ts = first_ts + commit as u64 * 3600;
        let message = if commit == 0 {
            "chore: initial import".to_string()
        } else {
            format!("feat: synthetic change {commit} (REQ-{:04})", commit % 97)
        };
        out.extend_from_slice(b"commit refs/heads/main\n");
        out.extend_from_slice(format!("mark :{}\n", commit + 1).as_bytes());
        out.extend_from_slice(format!("author Synthetic <synthetic@nexspec.test> {ts} +0000\n").as_bytes());
        out.extend_from_slice(format!("committer Synthetic <synthetic@nexspec.test> {ts} +0000\n").as_bytes());
        out.extend_from_slice(format!("data {}\n{}\n", message.len(), message).as_bytes());
        if commit > 0 {
            out.extend_from_slice(format!("from :{commit}\n").as_bytes());
        }
        for (index, rev) in changes {
            let content = if rev == 0 {
                initial_content(index, params)
            } else {
                modified_content(index, params, rev)
            };
            out.extend_from_slice(format!("M 100644 inline {}\n", path_of(index, params)).as_bytes());
            out.extend_from_slice(format!("data {}\n{}\n", content.len(), content).as_bytes());
        }
        out.push(b'\n');
    }
    out
}

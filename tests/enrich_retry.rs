//! A batch the model answers badly is asked again, one file at a time (REQ-1909): a crowded request is the usual
//! reason an answer skips a file, and a single-file request is far more reliable.

mod fixtures;

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

use fixtures::FixtureRepo;
use nexspec::Engine;
use nexspec::enrich::cache::EnrichmentCache;
use nexspec::enrich::provider::{BatchOutcome, EnrichProvider, FileRequest, ProviderError, Summarised, Usage};
use nexspec::enrich::run::{EnrichOptions, plan, run};

/// Answers every file when asked for one at a time, and only the first file of a bigger batch.
struct Crowded {
    sizes: Mutex<Vec<usize>>,
}

impl EnrichProvider for Crowded {
    fn name(&self) -> &str {
        "crowded"
    }

    fn model(&self) -> &str {
        "crowded-model"
    }

    fn summarize(&self, files: &[FileRequest], langs: &[String]) -> Result<BatchOutcome, ProviderError> {
        self.sizes.lock().unwrap().push(files.len());
        let mut outcome = BatchOutcome { usage: Usage { input_tokens: 10, output_tokens: 5 }, ..BatchOutcome::default() };
        for (i, file) in files.iter().enumerate() {
            if files.len() == 1 || i == 0 {
                let summaries: BTreeMap<String, String> = langs.iter().map(|l| (l.clone(), format!("summary of {}", file.path))).collect();
                outcome.items.push(Summarised { path: file.path.clone(), summaries });
            } else {
                outcome.failed.push((file.path.clone(), "no answer for this file".to_string()));
            }
        }
        Ok(outcome)
    }
}

#[test]
fn files_a_crowded_batch_missed_are_asked_again_alone() {
    let repo = FixtureRepo::init();
    repo.write_file(".gitignore", ".specs/.index/\n.specs/.cache/\n");
    for name in ["a", "b", "c", "d"] {
        repo.write_file(&format!("src/{name}.ts"), &format!("export function {name}Fn() {{ return 1; }}\n"));
    }
    repo.commit("init");
    let index = tempfile::TempDir::new().unwrap();
    let engine = Engine::open(index.path(), repo.path()).unwrap();
    engine.sync().unwrap();

    let options = EnrichOptions { batch: 4, concurrency: 1, ..EnrichOptions::default() };
    let provider = Crowded { sizes: Mutex::new(Vec::new()) };
    let cache = EnrichmentCache::load(repo.path()).unwrap();
    let planned = plan(&engine, repo.path(), &cache, &options, provider.model()).unwrap();
    assert_eq!(planned.files.len(), 4);
    let report = run(&engine, repo.path(), &provider, &options, planned, &AtomicBool::new(false), |_, _| {}).unwrap();

    assert_eq!(report.enriched_files, 4, "all four ended up with a summary");
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(report.retried, 3, "the three the big batch missed");
    assert_eq!(*provider.sizes.lock().unwrap(), vec![4, 1, 1, 1], "one crowded request, then three single-file ones");
    assert_eq!(EnrichmentCache::load(repo.path()).unwrap().len(), 4);
}

/// A provider that never answers a file: it is retried once and then reported, not retried forever.
struct Deaf {
    calls: Mutex<usize>,
}

impl EnrichProvider for Deaf {
    fn name(&self) -> &str {
        "deaf"
    }

    fn model(&self) -> &str {
        "deaf-model"
    }

    fn summarize(&self, files: &[FileRequest], _langs: &[String]) -> Result<BatchOutcome, ProviderError> {
        *self.calls.lock().unwrap() += 1;
        Ok(BatchOutcome { failed: files.iter().map(|f| (f.path.clone(), "no answer for this file".to_string())).collect(), ..BatchOutcome::default() })
    }
}

#[test]
fn a_file_that_is_never_answered_is_retried_once_and_then_reported() {
    let repo = FixtureRepo::init();
    repo.write_file(".gitignore", ".specs/.index/\n.specs/.cache/\n");
    repo.write_file("src/a.ts", "export const a = 1;\n");
    repo.commit("init");
    let index = tempfile::TempDir::new().unwrap();
    let engine = Engine::open(index.path(), repo.path()).unwrap();
    engine.sync().unwrap();
    let options = EnrichOptions { concurrency: 1, ..EnrichOptions::default() };
    let provider = Deaf { calls: Mutex::new(0) };
    let planned = plan(&engine, repo.path(), &EnrichmentCache::default(), &options, provider.model()).unwrap();
    let report = run(&engine, repo.path(), &provider, &options, planned, &AtomicBool::new(false), |_, _| {}).unwrap();
    assert_eq!((report.enriched_files, report.failed.len(), report.retried), (0, 1, 1));
    assert_eq!(*provider.calls.lock().unwrap(), 2, "the first try and one retry, no more");
}

/// Remembers every snippet it was sent.
struct Spy {
    seen: Mutex<Vec<(String, String)>>,
}

impl EnrichProvider for Spy {
    fn name(&self) -> &str {
        "spy"
    }

    fn model(&self) -> &str {
        "spy-model"
    }

    fn summarize(&self, files: &[FileRequest], langs: &[String]) -> Result<BatchOutcome, ProviderError> {
        let mut outcome = BatchOutcome::default();
        for file in files {
            self.seen.lock().unwrap().push((file.path.clone(), file.snippet.clone()));
            outcome.items.push(Summarised { path: file.path.clone(), summaries: langs.iter().map(|l| (l.clone(), "ok".to_string())).collect() });
        }
        Ok(outcome)
    }
}

#[test]
fn the_provider_never_sees_a_secret_value_only_the_masked_file() {
    let repo = FixtureRepo::init();
    repo.write_file(".gitignore", ".specs/.index/\n.specs/.cache/\n");
    let token = format!("{}{}", "gh", "p_abcdefghijklmnopqrstuvwxyz0123456789");
    repo.write_file("src/config.ts", &format!("export const password = \"hunter2hunter2\";\nexport const token = '{token}';\nexport const url = process.env.URL;\n"));
    repo.commit("init");
    let index = tempfile::TempDir::new().unwrap();
    let engine = Engine::open(index.path(), repo.path()).unwrap();
    engine.sync().unwrap();
    let provider = Spy { seen: Mutex::new(Vec::new()) };
    let options = EnrichOptions::default();
    let planned = plan(&engine, repo.path(), &EnrichmentCache::default(), &options, provider.model()).unwrap();
    assert_eq!((planned.files.len(), planned.redacted.len(), planned.omitted_secret.len()), (1, 1, 0));
    run(&engine, repo.path(), &provider, &options, planned, &AtomicBool::new(false), |_, _| {}).unwrap();
    let seen = provider.seen.lock().unwrap();
    let snippet = &seen[0].1;
    assert!(!snippet.contains("hunter2hunter2") && !snippet.contains(&token), "{snippet}");
    assert!(snippet.contains("password") && snippet.contains("[REDACTED]") && snippet.contains("process.env.URL"), "the code stays readable: {snippet}");

    let strict = EnrichOptions { redact_secrets: false, ..EnrichOptions::default() };
    let strict_plan = plan(&engine, repo.path(), &EnrichmentCache::default(), &strict, provider.model()).unwrap();
    assert_eq!((strict_plan.files.len(), strict_plan.omitted_secret.len()), (0, 1), "--omit-secrets keeps it home");
}

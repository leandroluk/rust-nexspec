//! The LLM behind `enrich` (REQ-1902, REQ-1912 in
//! `.specs/features/retrieval-enrichment/spec.md`).
//!
//! [`EnrichProvider`] takes a batch of file snippets and returns one summary per
//! language per file, plus the tokens the provider says it used. [`GeminiProvider`]
//! talks to Gemini through an [`HttpTransport`], so its request building, retry
//! and response handling are tested without the network. The API key travels in a
//! header and is never part of a URL, an error message or a log line.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{Value, json};

use crate::search::schema::summary_language_supported;

/// Bump when the prompt changes: cached summaries of another version are re-made (REQ-1904).
pub const PROMPT_VERSION: u32 = 1;
pub const DEFAULT_MODEL: &str = "gemini-flash-lite-latest";
const MAX_WORDS: usize = 40;

#[derive(Debug, Clone)]
pub struct FileRequest {
    pub path: String,
    pub snippet: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl std::ops::AddAssign for Usage {
    fn add_assign(&mut self, other: Self) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summarised {
    pub path: String,
    /// `lang -> summary`, every requested language present.
    pub summaries: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct BatchOutcome {
    pub items: Vec<Summarised>,
    /// Files of the batch the provider did not answer usably: `(path, reason)`. Reasons never hold code or keys.
    pub failed: Vec<(String, String)>,
    pub usage: Usage,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("{0} is not set: export it to use `nexspec enrich` with this provider")]
    MissingKey(&'static str),
    #[error("unsupported summary language `{0}` (use one of the stemmable languages, for example en, pt, ru, es, fr, de)")]
    UnsupportedLanguage(String),
    #[error("network error: {0}")]
    Network(String),
    #[error("the provider answered HTTP {status}: {detail}")]
    Http { status: u16, detail: String },
    #[error("the provider's answer could not be used: {0}")]
    BadResponse(String),
}

pub trait EnrichProvider: Send + Sync {
    fn name(&self) -> &str;
    fn model(&self) -> &str;
    fn summarize(&self, files: &[FileRequest], langs: &[String]) -> Result<BatchOutcome, ProviderError>;
}

pub fn validate_languages(langs: &[String]) -> Result<(), ProviderError> {
    for lang in langs {
        if !summary_language_supported(lang) {
            return Err(ProviderError::UnsupportedLanguage(lang.clone()));
        }
    }
    Ok(())
}

/// Instructions + the files. Every language in one request, so the code is sent once.
pub fn build_prompt(files: &[FileRequest], langs: &[String]) -> String {
    let mut prompt = format!(
        "You index source files for a code search engine. For EACH file below write, for EACH of these languages ({}), \
         1-2 sentences (at most {MAX_WORDS} words) saying what the file does, the domain or business concepts it deals \
         with, and synonyms a developer might type when looking for it. Write each language natively, do not translate \
         word for word. Do not quote code. Answer only with JSON: an array with one object per file, \
         {{\"path\": <the path as given>, \"summaries\": {{<language code>: <text>}}}}.\n",
        langs.join(", ")
    );
    for file in files {
        prompt.push_str(&format!("\n=== FILE: {} ===\n{}\n", file.path, file.snippet));
    }
    prompt
}

/// Reads the JSON the model returned. A file whose answer is missing, or lacks a requested
/// language, goes to `failed`; the rest are kept (REQ-1909).
pub type ParsedAnswer = (Vec<Summarised>, Vec<(String, String)>);

pub fn parse_answer(text: &str, requested: &[FileRequest], langs: &[String]) -> Result<ParsedAnswer, ProviderError> {
    let value: Value = serde_json::from_str(text.trim()).map_err(|_| ProviderError::BadResponse("the answer is not JSON".to_string()))?;
    let array = value.as_array().ok_or_else(|| ProviderError::BadResponse("the answer is not a JSON array".to_string()))?;
    let mut answered: BTreeMap<&str, BTreeMap<String, String>> = BTreeMap::new();
    for item in array {
        let (Some(path), Some(summaries)) = (item.get("path").and_then(Value::as_str), item.get("summaries").and_then(Value::as_object)) else { continue };
        let mut per_lang = BTreeMap::new();
        for lang in langs {
            if let Some(text) = summaries.get(lang).and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()) {
                per_lang.insert(lang.clone(), text.to_string());
            }
        }
        answered.insert(path, per_lang);
    }
    let mut items = Vec::new();
    let mut failed = Vec::new();
    for file in requested {
        match answered.remove(file.path.as_str()) {
            None => failed.push((file.path.clone(), "no answer for this file".to_string())),
            Some(per_lang) if per_lang.len() < langs.len() => failed.push((file.path.clone(), "a requested language is missing".to_string())),
            Some(per_lang) => items.push(Summarised { path: file.path.clone(), summaries: per_lang }),
        }
    }
    Ok((items, failed))
}

// ---------------------------------------------------------------------------
// Gemini
// ---------------------------------------------------------------------------

/// One HTTP exchange, abstracted so tests do not need a network.
pub trait HttpTransport: Send + Sync {
    /// POSTs `body` (JSON) to `url` with the extra headers; returns `(status, body)`.
    fn post(&self, url: &str, headers: &[(&str, &str)], body: &str) -> Result<(u16, String), String>;
}

pub struct UreqTransport {
    agent: ureq::Agent,
}

impl UreqTransport {
    pub fn new() -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(120)))
            .http_status_as_error(false)
            .build()
            .into();
        Self { agent }
    }
}

impl Default for UreqTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpTransport for UreqTransport {
    fn post(&self, url: &str, headers: &[(&str, &str)], body: &str) -> Result<(u16, String), String> {
        let mut request = self.agent.post(url).header("content-type", "application/json");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        // The error text of the HTTP library never includes request headers; still, keep only its kind.
        let mut response = request.send(body).map_err(|e| e.to_string().split(':').next().unwrap_or("request failed").to_string())?;
        let status = response.status().as_u16();
        let text = response.body_mut().read_to_string().map_err(|_| "could not read the response".to_string())?;
        Ok((status, text))
    }
}

pub struct GeminiProvider {
    api_key: String,
    model: String,
    base_url: String,
    transport: Box<dyn HttpTransport>,
    /// First back-off; doubles per retry (429/5xx, at most 3 retries).
    backoff: Duration,
}

const ENDPOINT: &str = "https://generativelanguage.googleapis.com/v1beta";
const MAX_RETRIES: u32 = 3;

impl GeminiProvider {
    /// `GEMINI_API_KEY`, else `GOOGLE_API_KEY`; model from `model`, else `NEXSPEC_ENRICH_MODEL`, else the default.
    pub fn from_env(model: Option<&str>) -> Result<Self, ProviderError> {
        let api_key = std::env::var("GEMINI_API_KEY")
            .or_else(|_| std::env::var("GOOGLE_API_KEY"))
            .ok()
            .filter(|k| !k.trim().is_empty())
            .ok_or(ProviderError::MissingKey("GEMINI_API_KEY"))?;
        let model = model
            .map(str::to_string)
            .or_else(|| std::env::var("NEXSPEC_ENRICH_MODEL").ok().filter(|m| !m.trim().is_empty()))
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
        Ok(Self::with_transport(api_key, model, Box::new(UreqTransport::new())))
    }

    pub fn with_transport(api_key: String, model: String, transport: Box<dyn HttpTransport>) -> Self {
        Self { api_key, model, base_url: ENDPOINT.to_string(), transport, backoff: Duration::from_secs(1) }
    }

    pub fn with_backoff(mut self, backoff: Duration) -> Self {
        self.backoff = backoff;
        self
    }

    fn url(&self) -> String {
        format!("{}/models/{}:generateContent", self.base_url, self.model)
    }

    fn body(&self, prompt: &str) -> String {
        json!({
            "contents": [{ "parts": [{ "text": prompt }] }],
            "generationConfig": {
                "temperature": 0,
                "responseMimeType": "application/json",
                "responseSchema": {
                    "type": "ARRAY",
                    "items": {
                        "type": "OBJECT",
                        "properties": { "path": { "type": "STRING" }, "summaries": { "type": "OBJECT" } },
                        "required": ["path", "summaries"]
                    }
                }
            }
        })
        .to_string()
    }

    fn send_with_retry(&self, body: &str) -> Result<String, ProviderError> {
        let url = self.url();
        let mut delay = self.backoff;
        for attempt in 0..=MAX_RETRIES {
            let outcome = self.transport.post(&url, &[("x-goog-api-key", self.api_key.as_str())], body);
            let retriable = match &outcome {
                Ok((status, _)) => *status == 429 || *status >= 500,
                Err(_) => true,
            };
            if retriable && attempt < MAX_RETRIES {
                std::thread::sleep(delay);
                delay *= 2;
                continue;
            }
            return match outcome {
                Ok((200..=299, text)) => Ok(text),
                Ok((status, text)) => Err(ProviderError::Http { status, detail: self.redact(&short_error(&text)) }),
                Err(message) => Err(ProviderError::Network(self.redact(&message))),
            };
        }
        unreachable!("the loop returns on its last attempt")
    }

    /// Belt and braces: the key must not appear in anything we report.
    fn redact(&self, text: &str) -> String {
        text.replace(&self.api_key, "[key]")
    }
}

/// The provider's own error message, trimmed; never the request.
fn short_error(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| "no detail".to_string())
        .chars()
        .take(200)
        .collect()
}

impl EnrichProvider for GeminiProvider {
    fn name(&self) -> &str {
        "gemini"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn summarize(&self, files: &[FileRequest], langs: &[String]) -> Result<BatchOutcome, ProviderError> {
        validate_languages(langs)?;
        let text = self.send_with_retry(&self.body(&build_prompt(files, langs)))?;
        let response: Value = serde_json::from_str(&text).map_err(|_| ProviderError::BadResponse("the response is not JSON".to_string()))?;
        let usage = Usage {
            input_tokens: response.pointer("/usageMetadata/promptTokenCount").and_then(Value::as_u64).unwrap_or(0),
            output_tokens: response.pointer("/usageMetadata/candidatesTokenCount").and_then(Value::as_u64).unwrap_or(0),
        };
        let answer = response
            .pointer("/candidates/0/content/parts/0/text")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::BadResponse("no candidate in the response".to_string()))?;
        let (items, failed) = parse_answer(answer, files, langs)?;
        Ok(BatchOutcome { items, failed, usage })
    }
}

// ---------------------------------------------------------------------------
// Fake (tests, and `bench` fixtures)
// ---------------------------------------------------------------------------

/// Answers from a fixed table; files it does not know are reported as failed.
pub struct FakeProvider {
    answers: BTreeMap<String, BTreeMap<String, String>>,
    usage_per_batch: Usage,
    model: String,
    calls: std::sync::atomic::AtomicUsize,
}

impl FakeProvider {
    pub fn new(answers: &[(&str, &[(&str, &str)])]) -> Self {
        Self {
            answers: answers
                .iter()
                .map(|(path, langs)| (path.to_string(), langs.iter().map(|(l, s)| (l.to_string(), s.to_string())).collect()))
                .collect(),
            usage_per_batch: Usage { input_tokens: 100, output_tokens: 20 },
            model: "fake-model".to_string(),
            calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl EnrichProvider for FakeProvider {
    fn name(&self) -> &str {
        "fake"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn summarize(&self, files: &[FileRequest], langs: &[String]) -> Result<BatchOutcome, ProviderError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        validate_languages(langs)?;
        let mut outcome = BatchOutcome { usage: self.usage_per_batch, ..BatchOutcome::default() };
        for file in files {
            let per_lang: Option<BTreeMap<String, String>> = self.answers.get(&file.path).and_then(|known| {
                langs.iter().map(|l| known.get(l).map(|s| (l.clone(), s.clone()))).collect()
            });
            match per_lang {
                Some(summaries) => outcome.items.push(Summarised { path: file.path.clone(), summaries }),
                None => outcome.failed.push((file.path.clone(), "no recorded answer".to_string())),
            }
        }
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn request(path: &str) -> FileRequest {
        FileRequest { path: path.to_string(), snippet: "export class X {}".to_string() }
    }

    type Seen = (String, Vec<(String, String)>, String);

    struct ScriptedTransport {
        replies: Mutex<Vec<Result<(u16, String), String>>>,
        seen: Mutex<Vec<Seen>>,
    }

    impl ScriptedTransport {
        fn new(replies: Vec<Result<(u16, String), String>>) -> std::sync::Arc<Self> {
            std::sync::Arc::new(Self { replies: Mutex::new(replies), seen: Mutex::new(Vec::new()) })
        }
    }

    impl HttpTransport for std::sync::Arc<ScriptedTransport> {
        fn post(&self, url: &str, headers: &[(&str, &str)], body: &str) -> Result<(u16, String), String> {
            self.seen.lock().unwrap().push((url.to_string(), headers.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(), body.to_string()));
            self.replies.lock().unwrap().remove(0)
        }
    }

    fn gemini(transport: std::sync::Arc<ScriptedTransport>) -> GeminiProvider {
        GeminiProvider::with_transport("SECRET-KEY-123".into(), "m".into(), Box::new(transport)).with_backoff(Duration::from_millis(1))
    }

    fn ok_response(answer: &str) -> (u16, String) {
        (200, json!({ "candidates": [{ "content": { "parts": [{ "text": answer }] } }], "usageMetadata": { "promptTokenCount": 321, "candidatesTokenCount": 45 } }).to_string())
    }

    #[test]
    fn the_key_goes_in_a_header_never_in_the_url_or_the_body() {
        let transport = ScriptedTransport::new(vec![Ok(ok_response(r#"[{"path":"a.ts","summaries":{"en":"Does a thing"}}]"#))]);
        let outcome = gemini(transport.clone()).summarize(&[request("a.ts")], &["en".into()]).unwrap();
        assert_eq!(outcome.usage, Usage { input_tokens: 321, output_tokens: 45 });
        assert_eq!(outcome.items[0].summaries["en"], "Does a thing");

        let seen = transport.seen.lock().unwrap();
        let (url, headers, body) = &seen[0];
        assert!(!url.contains("SECRET-KEY-123") && !body.contains("SECRET-KEY-123"));
        assert!(url.ends_with("/models/m:generateContent"));
        assert!(headers.contains(&("x-goog-api-key".to_string(), "SECRET-KEY-123".to_string())));
    }

    #[test]
    fn rate_limits_and_server_errors_are_retried_then_given_up() {
        let transport = ScriptedTransport::new(vec![Ok((429, "{}".into())), Ok((503, "{}".into())), Ok(ok_response(r#"[{"path":"a.ts","summaries":{"en":"ok"}}]"#))]);
        assert!(gemini(transport.clone()).summarize(&[request("a.ts")], &["en".into()]).is_ok());
        assert_eq!(transport.seen.lock().unwrap().len(), 3);

        let always_429 = ScriptedTransport::new((0..4).map(|_| Ok((429, "{}".into()))).collect());
        let err = gemini(always_429.clone()).summarize(&[request("a.ts")], &["en".into()]).unwrap_err();
        assert!(matches!(err, ProviderError::Http { status: 429, .. }), "{err}");
        assert_eq!(always_429.seen.lock().unwrap().len(), 4, "one try and three retries");

        let forbidden = ScriptedTransport::new(vec![Ok((403, json!({"error":{"message":"API key not valid SECRET-KEY-123"}}).to_string()))]);
        let err = gemini(forbidden.clone()).summarize(&[request("a.ts")], &["en".into()]).unwrap_err();
        assert_eq!(forbidden.seen.lock().unwrap().len(), 1, "a 4xx other than 429 is not retried");
        assert!(!err.to_string().contains("SECRET-KEY-123"), "the key is redacted from errors: {err}");
    }

    #[test]
    fn an_incomplete_answer_fails_only_the_affected_files() {
        let answer = r#"[{"path":"a.ts","summaries":{"en":"A","pt":"A em português"}},{"path":"b.ts","summaries":{"en":"B only"}}]"#;
        let transport = ScriptedTransport::new(vec![Ok(ok_response(answer))]);
        let outcome = gemini(transport).summarize(&[request("a.ts"), request("b.ts"), request("c.ts")], &["en".into(), "pt".into()]).unwrap();
        assert_eq!(outcome.items.iter().map(|i| i.path.as_str()).collect::<Vec<_>>(), ["a.ts"]);
        let failed: Vec<_> = outcome.failed.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(failed, ["b.ts", "c.ts"]);
    }

    #[test]
    fn garbage_is_a_bad_response_not_a_panic() {
        let transport = ScriptedTransport::new(vec![Ok(ok_response("this is not json"))]);
        assert!(matches!(gemini(transport).summarize(&[request("a.ts")], &["en".into()]), Err(ProviderError::BadResponse(_))));
    }

    #[test]
    fn unsupported_languages_are_refused_before_any_request() {
        let transport = ScriptedTransport::new(vec![]);
        let err = gemini(transport.clone()).summarize(&[request("a.ts")], &["zh".into()]).unwrap_err();
        assert!(matches!(err, ProviderError::UnsupportedLanguage(_)));
        assert!(transport.seen.lock().unwrap().is_empty());
    }

    #[test]
    fn the_prompt_carries_every_language_and_file_once() {
        let prompt = build_prompt(&[request("a.ts"), request("b.ts")], &["en".into(), "ru".into()]);
        assert!(prompt.contains("en, ru") && prompt.matches("=== FILE:").count() == 2);
        assert!(prompt.contains("natively"));
    }

    #[test]
    fn the_fake_provider_answers_from_its_table_and_counts_calls() {
        let fake = FakeProvider::new(&[("a.ts", &[("en", "A"), ("pt", "Um A")])]);
        let outcome = fake.summarize(&[request("a.ts"), request("b.ts")], &["en".into(), "pt".into()]).unwrap();
        assert_eq!((outcome.items.len(), outcome.failed.len(), fake.calls()), (1, 1, 1));
    }

    /// Needs a real key and the network: `GEMINI_API_KEY=… cargo test -- --ignored gemini_live`.
    #[test]
    #[ignore]
    fn gemini_live_answers_in_two_languages() {
        let provider = GeminiProvider::from_env(None).expect("GEMINI_API_KEY");
        let file = FileRequest { path: "src/invoice.ts".into(), snippet: "export function chargeResident(invoice: Invoice) { return billing.charge(invoice) }".into() };
        let outcome = provider.summarize(&[file], &["en".into(), "pt".into()]).expect("a live answer");
        assert_eq!(outcome.items.len(), 1, "{:?}", outcome.failed);
        assert!(outcome.usage.input_tokens > 0);
    }
}

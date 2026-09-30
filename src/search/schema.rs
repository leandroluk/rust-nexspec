//! Tantivy schema (REQ-307 in `.specs/features/ast-lexical-search/spec.md`):
//! `id` (exact-match fast-path), `kind`, `text` (BM25), `path`.

use std::collections::BTreeMap;

use tantivy::TantivyDocument;
use tantivy::schema::{Field, IndexRecordOption, STORED, STRING, Schema, TextFieldIndexing, TextOptions};
use tantivy::tokenizer::{Language, LowerCaser, Stemmer, TextAnalyzer};

use crate::search::ident::{IDENT_TOKENIZER, IdentTokenizer};

use crate::graph::node::NodePayload;
use crate::sync::mutation::StableId;

/// Languages a file summary can be written in (REQ-1904): the ones Tantivy
/// can stem. One field per language is declared up front, because an index
/// schema is fixed; an unused field costs nothing.
pub const SUMMARY_LANGUAGES: [(&str, Language); 18] = [
    ("ar", Language::Arabic),
    ("da", Language::Danish),
    ("nl", Language::Dutch),
    ("en", Language::English),
    ("fi", Language::Finnish),
    ("fr", Language::French),
    ("de", Language::German),
    ("el", Language::Greek),
    ("hu", Language::Hungarian),
    ("it", Language::Italian),
    ("no", Language::Norwegian),
    ("pt", Language::Portuguese),
    ("ro", Language::Romanian),
    ("ru", Language::Russian),
    ("es", Language::Spanish),
    ("sv", Language::Swedish),
    ("ta", Language::Tamil),
    ("tr", Language::Turkish),
];

pub fn summary_language_supported(iso: &str) -> bool {
    SUMMARY_LANGUAGES.iter().any(|(code, _)| *code == iso)
}

pub fn summary_field_name(iso: &str) -> String {
    format!("summary_{iso}")
}

fn summary_tokenizer_name(iso: &str) -> String {
    format!("ident_{iso}")
}

/// Registers the identifier tokenizer and one stemming analyzer per summary language.
pub fn register_tokenizers(index: &tantivy::Index) {
    let manager = index.tokenizers();
    manager.register(IDENT_TOKENIZER, IdentTokenizer);
    for (iso, language) in SUMMARY_LANGUAGES {
        let analyzer = TextAnalyzer::builder(IdentTokenizer).filter(LowerCaser).filter(Stemmer::new(language)).build();
        manager.register(&summary_tokenizer_name(iso), analyzer);
    }
}

/// Where the file summaries come from when a document is built
/// (`enrich::cache::EnrichmentView` in production).
pub trait SummarySource: Send + Sync {
    /// Summaries per language for the file at `path`, only while the file still has
    /// the content they were written for (the source decides how it knows).
    fn summaries(&self, path: &str) -> BTreeMap<String, String>;
}

#[derive(Clone)]
pub struct TantivySchema {
    pub schema: Schema,
    pub id_field: Field,
    pub kind_field: Field,
    pub text_field: Field,
    pub path_field: Field,
    /// `(iso, field)` for every language in [`SUMMARY_LANGUAGES`].
    pub summary_fields: Vec<(&'static str, Field)>,
}

impl TantivySchema {
    pub fn new() -> Self {
        let mut builder = Schema::builder();
        let id_field = builder.add_text_field("id", STRING | STORED);
        let kind_field = builder.add_text_field("kind", STRING | STORED);
        // Identifier-aware tokenization: `CsrDelta` is found by "csr delta".
        let text_options = TextOptions::default()
            .set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer(IDENT_TOKENIZER)
                    .set_index_option(IndexRecordOption::WithFreqsAndPositions),
            )
            .set_stored();
        let text_field = builder.add_text_field("text", text_options);
        let path_field = builder.add_text_field("path", STRING | STORED);
        let summary_fields = SUMMARY_LANGUAGES
            .iter()
            .map(|(iso, _)| {
                let options = TextOptions::default().set_indexing_options(
                    TextFieldIndexing::default()
                        .set_tokenizer(&summary_tokenizer_name(iso))
                        .set_index_option(IndexRecordOption::WithFreqsAndPositions),
                );
                (*iso, builder.add_text_field(&summary_field_name(iso), options))
            })
            .collect();
        Self {
            schema: builder.build(),
            id_field,
            kind_field,
            text_field,
            path_field,
            summary_fields,
        }
    }

    /// Build the searchable document for one node — `id` as lowercase hex
    /// (so exact-match queries are plain string comparisons), `kind` from
    /// the payload variant, `text` the human-readable content worth
    /// ranking on, `path` when the variant carries one.
    pub fn document_for(&self, id: &StableId, payload: &NodePayload) -> TantivyDocument {
        self.document_with_summaries(id, payload, &BTreeMap::new())
    }

    /// [`Self::document_for`] plus the per-language summaries of a file (REQ-1907).
    pub fn document_with_summaries(&self, id: &StableId, payload: &NodePayload, summaries: &BTreeMap<String, String>) -> TantivyDocument {
        let id_hex = hex(id);
        let (kind, text, path) = describe(payload);

        let mut doc = TantivyDocument::default();
        doc.add_text(self.id_field, &id_hex);
        doc.add_text(self.kind_field, kind);
        doc.add_text(self.text_field, &text);
        if let Some(path) = path {
            doc.add_text(self.path_field, &path);
        }
        for (iso, field) in &self.summary_fields {
            if let Some(summary) = summaries.get(*iso) {
                doc.add_text(*field, summary);
            }
        }
        doc
    }
}

impl Default for TantivySchema {
    fn default() -> Self {
        Self::new()
    }
}

pub fn hex(id: &StableId) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}

/// Inverse of [`hex`] — `None` for anything that isn't exactly 64 valid hex
/// characters (a `StableId` is always 32 bytes).
pub fn unhex(s: &str) -> Option<StableId> {
    if s.len() != 64 {
        return None;
    }
    let mut id = [0u8; 32];
    for (i, byte) in id.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(id)
}

fn describe(payload: &NodePayload) -> (&'static str, String, Option<String>) {
    match payload {
        NodePayload::Requirement { title, body, .. } => {
            ("requirement", format!("{title} {body}"), None)
        }
        NodePayload::Task { title, body, .. } => ("task", format!("{title} {body}"), None),
        NodePayload::Adr { title, body, .. } => ("adr", format!("{title} {body}"), None),
        NodePayload::DocSection { title, .. } => ("doc_section", title.clone(), None),
        NodePayload::Symbol { name, .. } => ("symbol", name.clone(), None),
        NodePayload::File { path, .. } => ("file", path.clone(), Some(path.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_languages_are_the_stemmable_ones() {
        assert!(summary_language_supported("ru") && summary_language_supported("pt"));
        assert!(!summary_language_supported("zh"), "no tokenizer for Chinese in v1");
        assert_eq!(summary_field_name("en"), "summary_en");
    }

    #[test]
    fn hex_is_lowercase_and_64_chars() {
        let id: StableId = [0xABu8; 32];
        let s = hex(&id);
        assert_eq!(s.len(), 64);
        assert_eq!(s, "ab".repeat(32));
    }
}

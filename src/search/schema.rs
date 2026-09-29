//! Tantivy schema (REQ-307 in `.specs/features/ast-lexical-search/spec.md`):
//! `id` (exact-match fast-path), `kind`, `text` (BM25), `path`.

use tantivy::TantivyDocument;
use tantivy::schema::{Field, STORED, STRING, Schema, TEXT};

use crate::graph::node::NodePayload;
use crate::sync::mutation::StableId;

#[derive(Clone)]
pub struct TantivySchema {
    pub schema: Schema,
    pub id_field: Field,
    pub kind_field: Field,
    pub text_field: Field,
    pub path_field: Field,
}

impl TantivySchema {
    pub fn new() -> Self {
        let mut builder = Schema::builder();
        let id_field = builder.add_text_field("id", STRING | STORED);
        let kind_field = builder.add_text_field("kind", STRING | STORED);
        let text_field = builder.add_text_field("text", TEXT | STORED);
        let path_field = builder.add_text_field("path", STRING | STORED);
        Self {
            schema: builder.build(),
            id_field,
            kind_field,
            text_field,
            path_field,
        }
    }

    /// Build the searchable document for one node — `id` as lowercase hex
    /// (so exact-match queries are plain string comparisons), `kind` from
    /// the payload variant, `text` the human-readable content worth
    /// ranking on, `path` when the variant carries one.
    pub fn document_for(&self, id: &StableId, payload: &NodePayload) -> TantivyDocument {
        let id_hex = hex(id);
        let (kind, text, path) = describe(payload);

        let mut doc = TantivyDocument::default();
        doc.add_text(self.id_field, &id_hex);
        doc.add_text(self.kind_field, kind);
        doc.add_text(self.text_field, &text);
        if let Some(path) = path {
            doc.add_text(self.path_field, &path);
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
    fn hex_is_lowercase_and_64_chars() {
        let id: StableId = [0xABu8; 32];
        let s = hex(&id);
        assert_eq!(s.len(), 64);
        assert_eq!(s, "ab".repeat(32));
    }
}

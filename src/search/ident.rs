//! Identifier-aware tokenizer for the `text` field (REQ-806 in
//! `.specs/features/retrieval-benchmark/spec.md`, T-808a).
//!
//! The stock tokenizer keeps `CsrDelta` and `find_markers` as opaque words
//! (`csrdelta`), so a query about "csr delta" or "markers" cannot reach them.
//! This one splits every alphanumeric run at camelCase / PascalCase / digit
//! boundaries, lowercases the parts and also keeps the whole identifier, so
//! both `CsrDelta` and `csr delta` match the same symbol. `snake_case` and
//! `kebab-case` already arrive as separate runs (`_` and `-` are not
//! alphanumeric).

use tantivy::tokenizer::{Token, TokenStream, Tokenizer};

/// Name under which the tokenizer is registered in the index.
pub const IDENT_TOKENIZER: &str = "ident";

#[derive(Clone, Default)]
pub struct IdentTokenizer;

pub struct IdentTokenStream {
    tokens: Vec<Token>,
    next: usize,
}

impl Tokenizer for IdentTokenizer {
    type TokenStream<'a> = IdentTokenStream;

    fn token_stream<'a>(&'a mut self, text: &'a str) -> IdentTokenStream {
        IdentTokenStream { tokens: tokenize(text), next: 0 }
    }
}

impl TokenStream for IdentTokenStream {
    fn advance(&mut self) -> bool {
        if self.next < self.tokens.len() {
            self.next += 1;
            true
        } else {
            false
        }
    }

    fn token(&self) -> &Token {
        &self.tokens[self.next - 1]
    }

    fn token_mut(&mut self) -> &mut Token {
        &mut self.tokens[self.next - 1]
    }
}

fn tokenize(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut position = 0usize;
    let mut run_start: Option<usize> = None;
    let mut push_run = |from: usize, to: usize, tokens: &mut Vec<Token>| {
        let run = &text[from..to];
        let parts = split_identifier(run);
        if parts.len() > 1 {
            tokens.push(make_token(run.to_lowercase(), from, to, position));
            position += 1;
        }
        for (offset, part) in parts {
            tokens.push(make_token(part.to_lowercase(), from + offset, from + offset + part.len(), position));
            position += 1;
        }
    };
    for (offset, c) in text.char_indices() {
        if c.is_alphanumeric() {
            run_start.get_or_insert(offset);
        } else if let Some(start) = run_start.take() {
            push_run(start, offset, &mut tokens);
        }
    }
    if let Some(start) = run_start {
        push_run(start, text.len(), &mut tokens);
    }
    tokens
}

fn make_token(text: String, offset_from: usize, offset_to: usize, position: usize) -> Token {
    Token { offset_from, offset_to, position, text, position_length: 1 }
}

/// `(byte offset, part)` for each piece of an alphanumeric run, split at
/// lower->Upper (`csrDelta`), Upper->Upper+lower (`HTTPServer` -> `HTTP`,
/// `Server`) and letter<->digit (`sha256` -> `sha`, `256`) boundaries.
fn split_identifier(run: &str) -> Vec<(usize, &str)> {
    let chars: Vec<(usize, char)> = run.char_indices().collect();
    let mut parts = Vec::new();
    let mut start = 0usize;
    for i in 1..chars.len() {
        let (_, prev) = chars[i - 1];
        let (_, cur) = chars[i];
        let next = chars.get(i + 1).map(|&(_, c)| c);
        let boundary = (prev.is_lowercase() && cur.is_uppercase())
            || (prev.is_uppercase() && cur.is_uppercase() && next.is_some_and(|n| n.is_lowercase()))
            || (prev.is_alphabetic() && cur.is_numeric())
            || (prev.is_numeric() && cur.is_alphabetic());
        if boundary {
            parts.push((chars[start].0, &run[chars[start].0..chars[i].0]));
            start = i;
        }
    }
    parts.push((chars[start].0, &run[chars[start].0..]));
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use tantivy::tokenizer::TextAnalyzer;

    fn words(text: &str) -> Vec<String> {
        let mut analyzer = TextAnalyzer::from(IdentTokenizer);
        let mut stream = analyzer.token_stream(text);
        let mut out = Vec::new();
        while stream.advance() {
            out.push(stream.token().text.clone());
        }
        out
    }

    #[test]
    fn pascal_and_camel_case_keep_the_whole_word_and_its_parts() {
        assert_eq!(words("CsrDelta"), vec!["csrdelta", "csr", "delta"]);
        assert_eq!(words("handleInvoice"), vec!["handleinvoice", "handle", "invoice"]);
    }

    #[test]
    fn acronyms_and_digits_split_sensibly() {
        assert_eq!(words("HTTPServer"), vec!["httpserver", "http", "server"]);
        assert_eq!(words("sha256"), vec!["sha256", "sha", "256"]);
        assert_eq!(words("REQ-021b"), vec!["req", "021b", "021", "b"]);
    }

    #[test]
    fn snake_kebab_paths_and_plain_words_split_on_punctuation() {
        assert_eq!(words("find_markers"), vec!["find", "markers"]);
        assert_eq!(words("src/sync/wal.rs"), vec!["src", "sync", "wal", "rs"]);
        assert_eq!(words("outbox.decorator.ts"), vec!["outbox", "decorator", "ts"]);
        assert_eq!(words("plain words"), vec!["plain", "words"]);
        assert!(words("  ,.; ").is_empty());
    }

    #[test]
    fn offsets_point_into_the_original_text() {
        let text = "xCsrDelta y";
        let mut analyzer = TextAnalyzer::from(IdentTokenizer);
        let mut stream = analyzer.token_stream(text);
        while stream.advance() {
            let t = stream.token();
            assert_eq!(text[t.offset_from..t.offset_to].to_lowercase(), t.text, "{t:?}");
        }
    }
}

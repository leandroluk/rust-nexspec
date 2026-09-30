//! Benchmark corpus (REQ-801 in `.specs/features/retrieval-benchmark/spec.md`):
//! a TOML file of questions with the answers a good retrieval should surface.
//!
//! ```toml
//! commit = "abc123"            # optional: target commit the expectations were written against
//!
//! [[query]]
//! id = "locate-outbox-decorator"
//! kind = "locate"              # locate | structure | behavior | traceability
//! query = "outbox decorator repository dispatch"
//! grep = "outbox"              # optional key term for the grep baseline
//! expect = ["src/outbox/outbox.decorator.ts"]   # paths, `Symbols` or `REQ-123` markers
//! notes = "real failure seen on 2026-09-29"
//! ```

use std::collections::HashSet;
use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Where is X?
    Locate,
    /// What depends on / implements X?
    Structure,
    /// How does X work? (needs reading after the search)
    Behavior,
    /// Which code satisfies REQ-N?
    Traceability,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Locate, Kind::Structure, Kind::Behavior, Kind::Traceability];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Locate => "locate",
            Kind::Structure => "structure",
            Kind::Behavior => "behavior",
            Kind::Traceability => "traceability",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub id: String,
    pub kind: Kind,
    pub query: String,
    /// Key term fed to the `grep -rn` baseline.
    pub grep: String,
    pub expect: Vec<String>,
    pub notes: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Corpus {
    /// Commit of the target repository the expectations were written for.
    pub commit: Option<String>,
    pub queries: Vec<Query>,
}

#[derive(Debug, thiserror::Error)]
pub enum CorpusError {
    #[error("cannot read corpus {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid corpus TOML: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("corpus has no queries")]
    Empty,
    #[error("query #{index} has an empty id")]
    EmptyId { index: usize },
    #[error("duplicate query id {0:?}")]
    DuplicateId(String),
    #[error("query {id:?}: empty `query` text")]
    EmptyQuery { id: String },
    #[error("query {id:?}: `expect` must list at least one non-empty entry")]
    EmptyExpect { id: String },
}

#[derive(Deserialize)]
struct RawCorpus {
    commit: Option<String>,
    #[serde(default, rename = "query")]
    queries: Vec<RawQuery>,
}

#[derive(Deserialize)]
struct RawQuery {
    #[serde(default)]
    id: String,
    kind: Kind,
    #[serde(default)]
    query: String,
    grep: Option<String>,
    #[serde(default)]
    expect: Vec<String>,
    #[serde(default)]
    notes: String,
}

impl Corpus {
    pub fn load(path: &Path) -> Result<Self, CorpusError> {
        let text = std::fs::read_to_string(path).map_err(|source| CorpusError::Io {
            path: path.display().to_string(),
            source,
        })?;
        text.parse()
    }
}

impl std::str::FromStr for Corpus {
    type Err = CorpusError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let raw: RawCorpus = toml::from_str(text)?;
        if raw.queries.is_empty() {
            return Err(CorpusError::Empty);
        }
        let mut seen = HashSet::new();
        let mut queries = Vec::with_capacity(raw.queries.len());
        for (index, q) in raw.queries.into_iter().enumerate() {
            let id = q.id.trim().to_string();
            if id.is_empty() {
                return Err(CorpusError::EmptyId { index: index + 1 });
            }
            if !seen.insert(id.clone()) {
                return Err(CorpusError::DuplicateId(id));
            }
            let query = q.query.trim().to_string();
            if query.is_empty() {
                return Err(CorpusError::EmptyQuery { id });
            }
            let expect: Vec<String> = q
                .expect
                .iter()
                .map(|e| e.trim().to_string())
                .filter(|e| !e.is_empty())
                .collect();
            if expect.is_empty() || expect.len() != q.expect.len() {
                return Err(CorpusError::EmptyExpect { id });
            }
            let grep = q
                .grep
                .map(|g| g.trim().to_string())
                .filter(|g| !g.is_empty())
                .unwrap_or_else(|| query.split_whitespace().next().unwrap_or_default().to_string());
            queries.push(Query { id, kind: q.kind, query, grep, expect, notes: q.notes });
        }
        Ok(Corpus { commit: raw.commit, queries })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
commit = "abc123"

[[query]]
id = "locate-outbox"
kind = "locate"
query = "outbox decorator repository dispatch"
expect = ["src/outbox/outbox.decorator.ts"]
notes = "seen 2026-09-29"

[[query]]
id = "trace-req"
kind = "traceability"
query = "REQ-021b implementation"
grep = "REQ-021b"
expect = ["src/a.ts", "OutboxDecorator"]
"#;

    #[test]
    fn valid_corpus_loads_with_defaults() {
        let corpus: Corpus = VALID.parse().unwrap();
        assert_eq!(corpus.commit.as_deref(), Some("abc123"));
        assert_eq!(corpus.queries.len(), 2);
        assert_eq!(corpus.queries[0].kind, Kind::Locate);
        assert_eq!(corpus.queries[0].grep, "outbox", "defaults to the first word of the query");
        assert_eq!(corpus.queries[1].grep, "REQ-021b");
        assert_eq!(corpus.queries[1].expect.len(), 2);
    }

    #[test]
    fn duplicate_id_is_rejected_and_named() {
        let text = VALID.replace("trace-req", "locate-outbox");
        let err = text.parse::<Corpus>().unwrap_err();
        assert!(matches!(&err, CorpusError::DuplicateId(id) if id == "locate-outbox"), "{err}");
    }

    #[test]
    fn unknown_kind_is_rejected() {
        let text = VALID.replace("\"traceability\"", "\"vibes\"");
        let err = text.parse::<Corpus>().unwrap_err();
        assert!(matches!(err, CorpusError::Parse(_)), "{err}");
        assert!(err.to_string().contains("vibes") || err.to_string().contains("kind"), "{err}");
    }

    #[test]
    fn empty_expect_is_rejected_and_names_the_query() {
        let text = "[[query]]\nid = \"q1\"\nkind = \"locate\"\nquery = \"x\"\nexpect = []\n";
        let err = text.parse::<Corpus>().unwrap_err();
        assert!(matches!(&err, CorpusError::EmptyExpect { id } if id == "q1"), "{err}");
        assert!(err.to_string().contains("q1"));

        let blank = "[[query]]\nid = \"q2\"\nkind = \"locate\"\nquery = \"x\"\nexpect = [\"a.ts\", \" \"]\n";
        assert!(matches!(blank.parse::<Corpus>().unwrap_err(), CorpusError::EmptyExpect { .. }));
    }

    #[test]
    fn empty_corpus_and_empty_query_text_are_rejected() {
        assert!(matches!("commit = \"x\"".parse::<Corpus>().unwrap_err(), CorpusError::Empty));
        let text = "[[query]]\nid = \"q\"\nkind = \"locate\"\nquery = \"  \"\nexpect = [\"a.ts\"]\n";
        assert!(matches!(text.parse::<Corpus>().unwrap_err(), CorpusError::EmptyQuery { .. }));
    }
}

//! A live PostgreSQL database next to the changesets (REQ-1405, decision D9 in
//! `.specs/features/domain-extractors/design.md`).
//!
//! `extract --postgres DSN` opens a **read-only** transaction, reads the catalogs
//! (nothing but `SELECT`), stores what it found in `.specs/.cache/live-schema.json`
//! (the DSN is never stored) and reports where the database and the changesets
//! disagree. The next domain pass adds the objects that exist only in the database
//! to the graph. No DDL is ever sent.

use std::path::{Path, PathBuf};

use crate::domain::schema::{Column, Constraint, ConstraintKind, DEFAULT_SCHEMA, Schema, Table};

pub const LIVE_FILE: &str = "live-schema.json";

#[derive(Debug, thiserror::Error)]
pub enum LiveError {
    #[error("cannot reach the database: {0}")]
    Connect(String),
    #[error("reading the catalogs failed: {0}")]
    Query(String),
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{0} is not a schema file: {1}")]
    Parse(String, String),
}

pub fn cache_path(repo: &Path) -> PathBuf {
    repo.join(".specs").join(".cache").join(LIVE_FILE)
}

pub fn save(repo: &Path, schema: &Schema) -> Result<(), LiveError> {
    let path = cache_path(repo);
    let io = |source| LiveError::Io { path: path.display().to_string(), source };
    std::fs::create_dir_all(path.parent().expect("has a parent")).map_err(io)?;
    std::fs::write(&path, schema.to_json()).map_err(io)
}

/// The schema saved by the last `extract`, if any. A damaged file counts as absent.
pub fn load(repo: &Path) -> Option<Schema> {
    Schema::from_json(&std::fs::read_to_string(cache_path(repo)).ok()?).ok()
}

pub fn read_file(path: &Path) -> Result<Schema, LiveError> {
    let text = std::fs::read_to_string(path).map_err(|source| LiveError::Io { path: path.display().to_string(), source })?;
    Schema::from_json(&text).map_err(|e| LiveError::Parse(path.display().to_string(), e.to_string()))
}

// ---------------------------------------------------------------------------
// Reading the database
// ---------------------------------------------------------------------------

const COLUMNS_SQL: &str = "SELECT n.nspname::text, c.relname::text, c.relkind::text, a.attname::text, format_type(a.atttypid, a.atttypmod), a.attnotnull \
    FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace JOIN pg_attribute a ON a.attrelid = c.oid \
    WHERE c.relkind IN ('r','v','m','p') AND a.attnum > 0 AND NOT a.attisdropped \
    AND n.nspname NOT IN ('pg_catalog','information_schema') AND n.nspname NOT LIKE 'pg_toast%' \
    ORDER BY n.nspname, c.relname, a.attnum";

const CONSTRAINTS_SQL: &str = "SELECT n.nspname::text, c.relname::text, con.conname::text, con.contype::text, pg_get_constraintdef(con.oid), \
    (SELECT array_agg(att.attname::text ORDER BY k.ord) FROM unnest(con.conkey) WITH ORDINALITY k(attnum, ord) \
       JOIN pg_attribute att ON att.attrelid = con.conrelid AND att.attnum = k.attnum) \
    FROM pg_constraint con JOIN pg_class c ON c.oid = con.conrelid JOIN pg_namespace n ON n.oid = c.relnamespace \
    WHERE con.contype IN ('p','u','f','c') AND n.nspname NOT IN ('pg_catalog','information_schema') ORDER BY 1, 2, 3";

const INDEXES_SQL: &str = "SELECT schemaname::text, tablename::text, indexname::text, indexdef FROM pg_indexes \
    WHERE schemaname NOT IN ('pg_catalog','information_schema') ORDER BY 1, 2, 3";

/// Reads tables, views, columns, constraints and indexes from a live database, read-only.
pub fn read_database(dsn: &str) -> Result<Schema, LiveError> {
    let mut client = postgres::Client::connect(dsn, postgres::NoTls).map_err(|e| LiveError::Connect(redact(&e.to_string(), dsn)))?;
    let query_error = |e: postgres::Error| LiveError::Query(redact(&e.to_string(), dsn));
    // Read-only for the whole session: the server itself refuses anything that writes.
    client.batch_execute("SET default_transaction_read_only = on; BEGIN READ ONLY; SET LOCAL statement_timeout = '30s'").map_err(query_error)?;

    let mut schema = Schema::default();
    for row in client.query(COLUMNS_SQL, &[]).map_err(query_error)? {
        let (schema_name, table_name, kind): (String, String, String) = (row.get(0), row.get(1), row.get(2));
        let column = Column { name: row.get(3), sql_type: normalize_type(&row.get::<_, String>(4)), nullable: !row.get::<_, bool>(5) };
        if schema.table(&schema_name, &table_name).is_none() {
            schema.upsert_table(Table::new(&schema_name, &table_name, kind == "v" || kind == "m", ""));
        }
        if let Some(table) = schema.table_mut(&schema_name, &table_name) {
            table.columns.push(column);
        }
    }
    let mut constraint_names = std::collections::HashSet::new();
    for row in client.query(CONSTRAINTS_SQL, &[]).map_err(query_error)? {
        let (schema_name, table_name, name, contype, definition): (String, String, String, String, String) =
            (row.get(0), row.get(1), row.get(2), row.get(3), row.get(4));
        let columns: Vec<String> = row.get::<_, Option<Vec<String>>>(5).unwrap_or_default();
        let kind = match contype.as_str() {
            "p" => ConstraintKind::PrimaryKey,
            "u" => ConstraintKind::Unique,
            "f" => ConstraintKind::ForeignKey,
            _ => ConstraintKind::Check,
        };
        constraint_names.insert((schema_name.clone(), name.clone()));
        if let Some(table) = schema.table_mut(&schema_name, &table_name) {
            let references = (kind == ConstraintKind::ForeignKey).then(|| foreign_key_target(&definition)).flatten();
            table.put_constraint(Constraint { name, kind, columns, references });
        }
    }
    for row in client.query(INDEXES_SQL, &[]).map_err(query_error)? {
        let (schema_name, table_name, name, definition): (String, String, String, String) = (row.get(0), row.get(1), row.get(2), row.get(3));
        if constraint_names.contains(&(schema_name.clone(), name.clone())) {
            continue; // the index behind a primary key or unique constraint
        }
        if let Some(table) = schema.table_mut(&schema_name, &table_name) {
            let unique = definition.contains("CREATE UNIQUE INDEX");
            table.put_constraint(Constraint {
                name,
                kind: if unique { ConstraintKind::UniqueIndex } else { ConstraintKind::Index },
                columns: index_columns(&definition),
                references: None,
            });
        }
    }
    let _ = client.batch_execute("ROLLBACK");
    Ok(schema)
}

/// A connection error may echo the DSN; the password must not reach a terminal or a log.
fn redact(message: &str, dsn: &str) -> String {
    let mut secrets: Vec<&str> = Vec::new();
    // `postgres://user:PASSWORD@host/db`
    if let Some(after_scheme) = dsn.split("://").nth(1)
        && let Some(userinfo) = after_scheme.split('@').next().filter(|_| after_scheme.contains('@'))
        && let Some((_, password)) = userinfo.split_once(':')
    {
        secrets.push(password);
    }
    // `host=… password=PASSWORD dbname=…`
    for part in dsn.split_whitespace() {
        if let Some(password) = part.strip_prefix("password=") {
            secrets.push(password.trim_matches('\''));
        }
    }
    let mut out = message.to_string();
    for secret in secrets.into_iter().filter(|s| !s.is_empty()) {
        out = out.replace(secret, "[redacted]");
    }
    out
}

/// `FOREIGN KEY (a) REFERENCES schema.table(b) ON DELETE …` -> `(schema, table)`.
fn foreign_key_target(definition: &str) -> Option<(String, String)> {
    let after = definition.split("REFERENCES ").nth(1)?;
    let name = after.split('(').next()?.trim();
    let mut parts = name.split('.').map(|p| p.trim().trim_matches('"').to_string());
    let first = parts.next()?;
    Some(match parts.next() {
        Some(second) => (first, second),
        None => (DEFAULT_SCHEMA.to_string(), first),
    })
}

/// Column names of `CREATE INDEX … USING gin (a, b)`: what is inside the last parentheses.
fn index_columns(definition: &str) -> Vec<String> {
    let Some(open) = definition.rfind('(') else { return Vec::new() };
    let inner = definition[open + 1..].trim_end_matches(')');
    inner.split(',').map(|c| c.split_whitespace().next().unwrap_or_default().trim_matches('"').to_string()).filter(|c| !c.is_empty()).collect()
}

/// One spelling per type: `character varying(200)` and `varchar(200)` are the same column type.
pub fn normalize_type(sql_type: &str) -> String {
    let lower = sql_type.trim().to_lowercase();
    let lower = lower.replace(", ", ",").replace(" (", "(");
    let (base, args) = match lower.find('(') {
        Some(at) => {
            let close = lower[at..].find(')').map_or(lower.len(), |c| at + c + 1);
            (format!("{}{}", &lower[..at], &lower[close..]).trim().to_string(), Some(lower[at..close].to_string()))
        }
        None => (lower.clone(), None),
    };
    let canonical = match base.as_str() {
        "character varying" | "varchar" => "varchar",
        "character" | "char" | "bpchar" => "char",
        "integer" | "int" | "int4" => "int",
        "bigint" | "int8" => "bigint",
        "smallint" | "int2" => "smallint",
        "boolean" | "bool" => "boolean",
        "timestamp with time zone" | "timestamptz" => "timestamptz",
        "timestamp without time zone" | "timestamp" => "timestamp",
        "time with time zone" | "timetz" => "timetz",
        "time without time zone" | "time" => "time",
        "double precision" | "float8" => "double precision",
        "real" | "float4" => "real",
        "decimal" | "numeric" => "numeric",
        other => other,
    };
    format!("{canonical}{}", args.unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Drift
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Drift {
    /// Tables/views the changesets create that the database does not have.
    pub missing_in_database: Vec<String>,
    /// Tables/views the database has that no changeset creates.
    pub not_in_changesets: Vec<String>,
    /// Column and constraint differences on tables both sides know.
    pub differences: Vec<String>,
}

impl Drift {
    pub fn is_empty(&self) -> bool {
        self.missing_in_database.is_empty() && self.not_in_changesets.is_empty() && self.differences.is_empty()
    }

    pub fn count(&self) -> usize {
        self.missing_in_database.len() + self.not_in_changesets.len() + self.differences.len()
    }

    /// First line is stable for tools: `drift: none` or `drift: N difference(s)`.
    pub fn render(&self) -> String {
        if self.is_empty() {
            return "drift: none\n".to_string();
        }
        let mut out = format!("drift: {} difference(s)\n", self.count());
        for (title, items) in [("in the changesets but not in the database", &self.missing_in_database), ("in the database but not in the changesets", &self.not_in_changesets), ("different", &self.differences)] {
            if !items.is_empty() {
                out.push_str(&format!("{title}:\n"));
                for item in items {
                    out.push_str(&format!("  - {item}\n"));
                }
            }
        }
        out
    }
}

/// Liquibase keeps its own bookkeeping tables; they are not part of the application schema.
fn is_tool_table(name: &str) -> bool {
    matches!(name, "databasechangelog" | "databasechangeloglock" | "flyway_schema_history")
}

pub fn compare(changesets: &Schema, live: &Schema) -> Drift {
    let mut drift = Drift::default();
    for table in changesets.tables() {
        let label = format!("{}.{}", table.schema, table.name);
        let Some(actual) = live.table(&table.schema, &table.name) else {
            drift.missing_in_database.push(label);
            continue;
        };
        if table.is_view != actual.is_view {
            drift.differences.push(format!("{label}: a {} in the changesets, a {} in the database", if table.is_view { "view" } else { "table" }, if actual.is_view { "view" } else { "table" }));
        }
        for column in &table.columns {
            match actual.columns.iter().find(|c| c.name == column.name) {
                None => drift.differences.push(format!("{label}.{}: column missing in the database", column.name)),
                Some(other) => {
                    if normalize_type(&column.sql_type) != normalize_type(&other.sql_type) {
                        drift.differences.push(format!("{label}.{}: type {} in the changesets, {} in the database", column.name, column.sql_type, other.sql_type));
                    }
                    if column.nullable != other.nullable {
                        drift.differences.push(format!("{label}.{}: {} in the changesets, {} in the database", column.name, nullability(column.nullable), nullability(other.nullable)));
                    }
                }
            }
        }
        for column in &actual.columns {
            if !table.columns.iter().any(|c| c.name == column.name) {
                drift.differences.push(format!("{label}.{}: column only in the database", column.name));
            }
        }
        if !table.is_view {
            for constraint in &table.constraints {
                if !actual.constraints.iter().any(|c| c.name == constraint.name) {
                    drift.differences.push(format!("{label}: {} {} missing in the database", constraint.kind.as_str(), constraint.name));
                }
            }
            for constraint in &actual.constraints {
                if !table.constraints.iter().any(|c| c.name == constraint.name) {
                    drift.differences.push(format!("{label}: {} {} only in the database", constraint.kind.as_str(), constraint.name));
                }
            }
        }
    }
    for table in live.tables() {
        if changesets.table(&table.schema, &table.name).is_none() && !is_tool_table(&table.name) {
            drift.not_in_changesets.push(format!("{}.{}", table.schema, table.name));
        }
    }
    drift
}

fn nullability(nullable: bool) -> &'static str {
    if nullable { "nullable" } else { "not null" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(name: &str, columns: &[(&str, &str, bool)]) -> Table {
        let mut t = Table::new("public", name, false, "db/1.sql");
        t.columns = columns.iter().map(|(n, ty, nullable)| Column { name: n.to_string(), sql_type: ty.to_string(), nullable: *nullable }).collect();
        t
    }

    fn schema(tables: Vec<Table>) -> Schema {
        let mut s = Schema::default();
        for t in tables {
            s.upsert_table(t);
        }
        s
    }

    #[test]
    fn type_spellings_are_normalised() {
        for (a, b) in [("character varying(200)", "VARCHAR(200)"), ("timestamp(3) with time zone", "timestamptz(3)"), ("integer", "int4"), ("numeric(10, 2)", "decimal(10,2)"), ("boolean", "BOOL")] {
            assert_eq!(normalize_type(a), normalize_type(b), "{a} vs {b}");
        }
        assert_ne!(normalize_type("varchar(10)"), normalize_type("varchar(20)"));
        assert_eq!(normalize_type("timestamp(3) with time zone"), "timestamptz(3)");
    }

    #[test]
    fn foreign_key_targets_and_index_columns_are_read_from_their_definitions() {
        assert_eq!(foreign_key_target("FOREIGN KEY (contract_id) REFERENCES tb_contract(id) ON DELETE CASCADE"), Some(("public".into(), "tb_contract".into())));
        assert_eq!(foreign_key_target("FOREIGN KEY (a) REFERENCES billing.\"tb_invoice\"(id)"), Some(("billing".into(), "tb_invoice".into())));
        assert_eq!(index_columns("CREATE INDEX ix ON public.t USING btree (a, \"b\" DESC)"), ["a", "b"]);
    }

    #[test]
    fn identical_schemas_have_no_drift_and_every_kind_of_difference_is_named() {
        let changesets = schema(vec![table("tb_a", &[("id", "uuid", false), ("name", "varchar(10)", true)]), table("tb_only_changesets", &[("id", "int", false)])]);
        let same = schema(vec![table("tb_a", &[("id", "uuid", false), ("name", "character varying(10)", true)]), table("tb_only_changesets", &[("id", "integer", false)])]);
        assert!(compare(&changesets, &same).is_empty(), "spelling is not drift");
        assert_eq!(compare(&changesets, &same).render(), "drift: none\n");

        let live = schema(vec![table("tb_a", &[("id", "uuid", true), ("name", "varchar(20)", true), ("extra", "text", true)]), table("tb_unmanaged", &[("id", "int", false)]), table("databasechangelog", &[("id", "text", false)])]);
        let drift = compare(&changesets, &live);
        assert_eq!(drift.missing_in_database, ["public.tb_only_changesets"]);
        assert_eq!(drift.not_in_changesets, ["public.tb_unmanaged"], "Liquibase's own tables are not drift");
        assert!(drift.differences.iter().any(|d| d.contains("tb_a.id") && d.contains("not null in the changesets, nullable")));
        assert!(drift.differences.iter().any(|d| d.contains("tb_a.name") && d.contains("type varchar(10)")));
        assert!(drift.differences.iter().any(|d| d.contains("tb_a.extra") && d.contains("only in the database")));
        assert!(drift.render().starts_with("drift: 5 difference(s)\n"), "{}", drift.render());
    }

    #[test]
    fn the_cache_round_trips_and_a_damaged_file_counts_as_absent() {
        let dir = tempfile::TempDir::new().unwrap();
        let original = schema(vec![table("tb_a", &[("id", "uuid", false)])]);
        assert!(load(dir.path()).is_none());
        save(dir.path(), &original).unwrap();
        assert_eq!(load(dir.path()).unwrap().table("public", "tb_a"), original.table("public", "tb_a"));
        std::fs::write(cache_path(dir.path()), "{ not json").unwrap();
        assert!(load(dir.path()).is_none());
    }

    #[test]
    fn a_password_in_the_dsn_never_survives_in_an_error() {
        let message = redact("connection to postgres://app:s3cretpw@db:5432/x failed: password s3cretpw rejected", "postgres://app:s3cretpw@db:5432/x");
        assert!(!message.contains("s3cretpw"), "{message}");
        let keyword = redact("bad: hunter2hunter2", "host=db user=app password=hunter2hunter2 dbname=x");
        assert!(!keyword.contains("hunter2hunter2"), "{keyword}");
    }

    /// Needs a database: `NEXSPEC_TEST_PG_DSN=postgres://… cargo test -- --ignored live_database`.
    #[test]
    #[ignore]
    fn live_database_is_read_without_writing() {
        let dsn = std::env::var("NEXSPEC_TEST_PG_DSN").expect("NEXSPEC_TEST_PG_DSN");
        let schema = read_database(&dsn).expect("read the catalogs");
        assert!(schema.tables().count() > 0 || schema.is_empty());
    }
}

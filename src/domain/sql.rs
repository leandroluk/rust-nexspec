//! A small PostgreSQL DDL reader (REQ-1402, decision D1 in
//! `.specs/features/domain-extractors/design.md`): it understands what a
//! migration folder is made of — `CREATE TABLE/VIEW/INDEX`, `ALTER TABLE`,
//! `DROP` — and skips the rest (functions, triggers, extensions, comments).
//! An unrecognised statement is counted, never fatal.

use crate::domain::schema::{Column, Constraint, ConstraintKind, DEFAULT_SCHEMA, Schema, Table};

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    /// Unquoted identifier or keyword, folded to lower case like PostgreSQL does.
    Word(String),
    /// `"Quoted"` identifier, case kept.
    Quoted(String),
    /// String literal or dollar-quoted body (content is irrelevant for DDL).
    Str,
    Num(String),
    Punct(char),
}

impl Tok {
    fn is_kw(&self, kw: &str) -> bool {
        matches!(self, Tok::Word(w) if w == kw)
    }

    fn ident(&self) -> Option<&str> {
        match self {
            Tok::Word(w) | Tok::Quoted(w) => Some(w),
            _ => None,
        }
    }

    fn text(&self) -> String {
        match self {
            Tok::Word(w) | Tok::Quoted(w) | Tok::Num(w) => w.clone(),
            Tok::Str => "'…'".to_string(),
            Tok::Punct(c) => c.to_string(),
        }
    }
}

fn tokenize(sql: &str) -> Vec<Tok> {
    let chars: Vec<char> = sql.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '-' && chars.get(i + 1) == Some(&'-') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else if c == '\'' {
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if chars.get(i + 1) == Some(&'\'') {
                        i += 2;
                        continue;
                    }
                    break;
                }
                i += 1;
            }
            i += 1;
            tokens.push(Tok::Str);
        } else if c == '"' {
            i += 1;
            let mut name = String::new();
            while i < chars.len() {
                if chars[i] == '"' {
                    if chars.get(i + 1) == Some(&'"') {
                        name.push('"');
                        i += 2;
                        continue;
                    }
                    break;
                }
                name.push(chars[i]);
                i += 1;
            }
            i += 1;
            tokens.push(Tok::Quoted(name));
        } else if c == '$' && dollar_tag(&chars, i).is_some() {
            let tag = dollar_tag(&chars, i).expect("checked");
            let tag_chars: Vec<char> = tag.chars().collect();
            i += tag_chars.len();
            while i < chars.len() && !chars[i..].starts_with(&tag_chars) {
                i += 1;
            }
            i += tag_chars.len();
            tokens.push(Tok::Str);
        } else if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            tokens.push(Tok::Num(chars[start..i].iter().collect()));
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$') {
                i += 1;
            }
            tokens.push(Tok::Word(chars[start..i].iter().collect::<String>().to_lowercase()));
        } else {
            tokens.push(Tok::Punct(c));
            i += 1;
        }
    }
    tokens
}

/// `$$` or `$tag$` starting at `i`, if that is what is there.
fn dollar_tag(chars: &[char], i: usize) -> Option<String> {
    let mut j = i + 1;
    while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
        j += 1;
    }
    (chars.get(j) == Some(&'$')).then(|| chars[i..=j].iter().collect())
}

fn statements(tokens: Vec<Tok>) -> Vec<Vec<Tok>> {
    let mut out = Vec::new();
    let mut current = Vec::new();
    for token in tokens {
        if token == Tok::Punct(';') {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
        } else {
            current.push(token);
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

struct Cursor<'a> {
    toks: &'a [Tok],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(toks: &'a [Tok]) -> Self {
        Self { toks, pos: 0 }
    }

    fn peek(&self) -> Option<&'a Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<&'a Tok> {
        let token = self.toks.get(self.pos);
        self.pos += 1;
        token
    }

    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.peek().is_some_and(|t| t.is_kw(kw)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn eat_kws(&mut self, kws: &[&str]) -> bool {
        let start = self.pos;
        for kw in kws {
            if !self.eat_kw(kw) {
                self.pos = start;
                return false;
            }
        }
        true
    }

    fn eat_punct(&mut self, c: char) -> bool {
        if self.peek() == Some(&Tok::Punct(c)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn done(&self) -> bool {
        self.pos >= self.toks.len()
    }

    /// `name`, `schema.name` or `"s"."n"` -> `(schema, name)`.
    fn qualified_name(&mut self) -> Option<(String, String)> {
        let first = self.next()?.ident()?.to_string();
        if self.eat_punct('.') {
            let second = self.next()?.ident()?.to_string();
            Some((first, second))
        } else {
            Some((DEFAULT_SCHEMA.to_string(), first))
        }
    }

    /// A parenthesised list of names: `(a, "b")`.
    fn name_list(&mut self) -> Vec<String> {
        let mut names = Vec::new();
        if !self.eat_punct('(') {
            return names;
        }
        let mut depth = 1;
        while let Some(token) = self.next() {
            match token {
                Tok::Punct('(') => depth += 1,
                Tok::Punct(')') => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                Tok::Word(_) | Tok::Quoted(_) if depth == 1 => {
                    if let Some(name) = token.ident() {
                        names.push(name.to_string());
                    }
                }
                _ => {}
            }
        }
        names
    }

    /// Skips a balanced `( … )` if one starts here.
    fn skip_parens(&mut self) {
        if !self.eat_punct('(') {
            return;
        }
        let mut depth = 1;
        while let Some(token) = self.next() {
            match token {
                Tok::Punct('(') => depth += 1,
                Tok::Punct(')') => {
                    depth -= 1;
                    if depth == 0 {
                        return;
                    }
                }
                _ => {}
            }
        }
    }
}

/// Splits tokens at top-level commas (outside parentheses).
fn split_commas(toks: &[Tok]) -> Vec<&[Tok]> {
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    for (i, token) in toks.iter().enumerate() {
        match token {
            Tok::Punct('(') => depth += 1,
            Tok::Punct(')') => depth -= 1,
            Tok::Punct(',') if depth == 0 => {
                parts.push(&toks[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&toks[start..]);
    parts.into_iter().filter(|p| !p.is_empty()).collect()
}

/// Reads SQL text and applies every statement it understands to `schema`.
pub fn apply_sql(schema: &mut Schema, sql: &str, file: &str) {
    for statement in statements(tokenize(sql)) {
        if !apply_statement(schema, &statement, file) {
            schema.skipped += 1;
        }
    }
}

fn apply_statement(schema: &mut Schema, toks: &[Tok], file: &str) -> bool {
    let mut cursor = Cursor::new(toks);
    if cursor.eat_kw("create") {
        let replace = cursor.eat_kws(&["or", "replace"]);
        let _ = replace;
        let unique = cursor.eat_kw("unique");
        let _ = cursor.eat_kw("temporary") || cursor.eat_kw("temp") || cursor.eat_kw("unlogged");
        if cursor.eat_kw("table") {
            return create_table(schema, &mut cursor, toks, file);
        }
        let materialized = cursor.eat_kw("materialized");
        if cursor.eat_kw("view") {
            let _ = materialized;
            return create_view(schema, &mut cursor, toks, file);
        }
        if cursor.eat_kw("index") {
            return create_index(schema, &mut cursor, unique);
        }
        return false;
    }
    if cursor.eat_kw("alter") {
        if cursor.eat_kw("table") {
            return alter_table(schema, &mut cursor, toks, file);
        }
        return false;
    }
    if cursor.eat_kw("drop") {
        return drop_object(schema, &mut cursor);
    }
    false
}

fn create_table(schema: &mut Schema, cursor: &mut Cursor, toks: &[Tok], file: &str) -> bool {
    let _ = cursor.eat_kws(&["if", "not", "exists"]);
    let Some((schema_name, name)) = cursor.qualified_name() else { return false };
    if cursor.peek() != Some(&Tok::Punct('(')) {
        return false;
    }
    let body_start = cursor.pos + 1;
    cursor.skip_parens();
    let body = &toks[body_start..cursor.pos.saturating_sub(1).max(body_start)];
    let mut table = Table::new(&schema_name, &name, false, file);
    for item in split_commas(body) {
        table_item(&mut table, item);
    }
    schema.upsert_table(table);
    true
}

/// A column definition or a table constraint inside `CREATE TABLE ( … )`.
fn table_item(table: &mut Table, item: &[Tok]) {
    let mut cursor = Cursor::new(item);
    let constraint_name = cursor.eat_kw("constraint").then(|| cursor.next().and_then(Tok::ident).map(str::to_string)).flatten();
    if let Some(constraint) = table_constraint(&table.name, &mut cursor, constraint_name.clone()) {
        table.put_constraint(constraint);
        return;
    }
    if constraint_name.is_some() || cursor.eat_kw("like") || cursor.eat_kw("exclude") {
        return;
    }
    cursor.pos = 0;
    let Some(name) = cursor.next().and_then(Tok::ident).map(str::to_string) else { return };
    let sql_type = parse_type(&mut cursor);
    let mut nullable = true;
    let mut inline: Vec<Constraint> = Vec::new();
    while !cursor.done() {
        let named = cursor.eat_kw("constraint").then(|| cursor.next().and_then(Tok::ident).map(str::to_string)).flatten();
        if cursor.eat_kws(&["not", "null"]) {
            nullable = false;
        } else if cursor.eat_kws(&["primary", "key"]) {
            nullable = false;
            inline.push(Constraint { name: named.unwrap_or_else(|| format!("{}_pkey", table.name)), kind: ConstraintKind::PrimaryKey, columns: vec![name.clone()], references: None });
        } else if cursor.eat_kw("unique") {
            inline.push(Constraint { name: named.unwrap_or_else(|| format!("{}_{}_key", table.name, name)), kind: ConstraintKind::Unique, columns: vec![name.clone()], references: None });
        } else if cursor.eat_kw("references") {
            let target = cursor.qualified_name();
            inline.push(Constraint { name: named.unwrap_or_else(|| format!("{}_{}_fkey", table.name, name)), kind: ConstraintKind::ForeignKey, columns: vec![name.clone()], references: target });
        } else if cursor.eat_kw("check") {
            cursor.skip_parens();
            inline.push(Constraint { name: named.unwrap_or_else(|| format!("{}_{}_check", table.name, name)), kind: ConstraintKind::Check, columns: vec![name.clone()], references: None });
        } else {
            cursor.next();
        }
    }
    table.columns.retain(|c| c.name != name);
    table.columns.push(Column { name, sql_type, nullable });
    for constraint in inline {
        table.put_constraint(constraint);
    }
}

/// `PRIMARY KEY (…)`, `UNIQUE (…)`, `FOREIGN KEY (…) REFERENCES t (…)`, `CHECK (…)` at the cursor.
fn table_constraint(table: &str, cursor: &mut Cursor, name: Option<String>) -> Option<Constraint> {
    if cursor.eat_kws(&["primary", "key"]) {
        let columns = cursor.name_list();
        return Some(Constraint { name: name.unwrap_or_else(|| format!("{table}_pkey")), kind: ConstraintKind::PrimaryKey, columns, references: None });
    }
    if cursor.eat_kw("unique") {
        let columns = cursor.name_list();
        let default = format!("{table}_{}_key", columns.join("_"));
        return Some(Constraint { name: name.unwrap_or(default), kind: ConstraintKind::Unique, columns, references: None });
    }
    if cursor.eat_kws(&["foreign", "key"]) {
        let columns = cursor.name_list();
        let references = cursor.eat_kw("references").then(|| cursor.qualified_name()).flatten();
        let default = format!("{table}_{}_fkey", columns.join("_"));
        return Some(Constraint { name: name.unwrap_or(default), kind: ConstraintKind::ForeignKey, columns, references });
    }
    if cursor.eat_kw("check") {
        cursor.skip_parens();
        return Some(Constraint { name: name.unwrap_or_else(|| format!("{table}_check")), kind: ConstraintKind::Check, columns: Vec::new(), references: None });
    }
    None
}

/// Type tokens up to the first column-constraint keyword: `TIMESTAMPTZ(3)`, `numeric(10,2)`, `text[]`, `double precision`.
fn parse_type(cursor: &mut Cursor) -> String {
    const STOP: &[&str] = &["not", "null", "default", "primary", "unique", "references", "check", "constraint", "generated", "collate"];
    let mut parts: Vec<String> = Vec::new();
    while let Some(token) = cursor.peek() {
        if let Tok::Word(w) = token
            && STOP.contains(&w.as_str())
        {
            break;
        }
        cursor.pos += 1;
        match token {
            Tok::Punct('(') => {
                let mut inner = String::from("(");
                let mut depth = 1;
                while let Some(t) = cursor.next() {
                    match t {
                        Tok::Punct('(') => depth += 1,
                        Tok::Punct(')') => depth -= 1,
                        _ => {}
                    }
                    inner.push_str(&t.text());
                    if depth == 0 {
                        break;
                    }
                }
                match parts.last_mut() {
                    Some(last) => last.push_str(&inner),
                    None => parts.push(inner),
                }
            }
            Tok::Punct(c @ ('[' | ']')) => match parts.last_mut() {
                Some(last) => last.push(*c),
                None => parts.push(c.to_string()),
            },
            other => parts.push(other.text()),
        }
    }
    parts.join(" ").to_lowercase()
}

fn create_view(schema: &mut Schema, cursor: &mut Cursor, toks: &[Tok], file: &str) -> bool {
    let Some((schema_name, name)) = cursor.qualified_name() else { return false };
    // Optional column list, then AS.
    cursor.skip_parens();
    while !cursor.done() && !cursor.eat_kw("as") {
        cursor.next();
    }
    let body = &toks[cursor.pos.min(toks.len())..];
    let mut table = Table::new(&schema_name, &name, true, file);
    let mut i = 0;
    while i < body.len() {
        if (body[i].is_kw("from") || body[i].is_kw("join"))
            && let Some(Tok::Word(_) | Tok::Quoted(_)) = body.get(i + 1)
        {
            let mut c = Cursor::new(&body[i + 1..]);
            if let Some((_, source)) = c.qualified_name()
                && !table.view_sources.contains(&source)
            {
                table.view_sources.push(source);
            }
        }
        i += 1;
    }
    schema.upsert_table(table);
    true
}

fn create_index(schema: &mut Schema, cursor: &mut Cursor, unique: bool) -> bool {
    let _ = cursor.eat_kw("concurrently");
    let _ = cursor.eat_kws(&["if", "not", "exists"]);
    let Some(name) = cursor.next().and_then(Tok::ident).map(str::to_string) else { return false };
    if !cursor.eat_kw("on") {
        return false;
    }
    let _ = cursor.eat_kw("only");
    let Some((schema_name, table_name)) = cursor.qualified_name() else { return false };
    let Some(table) = schema.table_mut(&schema_name, &table_name) else { return true };
    let _ = cursor.eat_kw("using") && cursor.next().is_some();
    let columns = cursor.name_list();
    table.put_constraint(Constraint { name, kind: if unique { ConstraintKind::UniqueIndex } else { ConstraintKind::Index }, columns, references: None });
    true
}

fn alter_table(schema: &mut Schema, cursor: &mut Cursor, toks: &[Tok], file: &str) -> bool {
    let _ = cursor.eat_kws(&["if", "exists"]);
    let _ = cursor.eat_kw("only");
    let Some((schema_name, table_name)) = cursor.qualified_name() else { return false };
    let actions = &toks[cursor.pos.min(toks.len())..];
    let _ = file;
    let mut understood = true;
    for action in split_commas(actions) {
        understood &= alter_action(schema, &schema_name, &table_name, action);
    }
    understood
}

fn alter_action(schema: &mut Schema, schema_name: &str, table_name: &str, action: &[Tok]) -> bool {
    let mut cursor = Cursor::new(action);
    if cursor.eat_kws(&["rename", "to"]) {
        if let Some(new_name) = cursor.next().and_then(Tok::ident) {
            schema.rename_table(schema_name, table_name, new_name);
        }
        return true;
    }
    let Some(table) = schema.table_mut(schema_name, table_name) else { return true };
    if cursor.eat_kw("add") {
        if cursor.eat_kw("constraint") {
            let name = cursor.next().and_then(Tok::ident).map(str::to_string);
            if let Some(constraint) = table_constraint(table_name, &mut cursor, name) {
                table.put_constraint(constraint);
            }
            return true;
        }
        if let Some(constraint) = table_constraint(table_name, &mut cursor, None) {
            table.put_constraint(constraint);
            return true;
        }
        let _ = cursor.eat_kw("column");
        let _ = cursor.eat_kws(&["if", "not", "exists"]);
        let rest = &action[cursor.pos.min(action.len())..];
        table_item(table, rest);
        return true;
    }
    if cursor.eat_kw("drop") {
        if cursor.eat_kw("constraint") {
            let _ = cursor.eat_kws(&["if", "exists"]);
            if let Some(name) = cursor.next().and_then(Tok::ident) {
                let name = name.to_string();
                table.constraints.retain(|c| c.name != name);
            }
            return true;
        }
        let _ = cursor.eat_kw("column");
        let _ = cursor.eat_kws(&["if", "exists"]);
        if let Some(name) = cursor.next().and_then(Tok::ident) {
            let name = name.to_string();
            table.columns.retain(|c| c.name != name);
            table.constraints.retain(|c| !c.columns.contains(&name) || c.columns.len() > 1);
        }
        return true;
    }
    if cursor.eat_kw("rename") {
        let _ = cursor.eat_kw("column");
        let from = cursor.next().and_then(Tok::ident).map(str::to_string);
        let to = cursor.eat_kw("to").then(|| cursor.next().and_then(Tok::ident).map(str::to_string)).flatten();
        if let (Some(from), Some(to)) = (from, to)
            && let Some(column) = table.column_mut(&from)
        {
            column.name = to;
        }
        return true;
    }
    if cursor.eat_kw("alter") {
        let _ = cursor.eat_kw("column");
        let Some(name) = cursor.next().and_then(Tok::ident).map(str::to_string) else { return false };
        let Some(column) = table.column_mut(&name) else { return true };
        if cursor.eat_kws(&["set", "not", "null"]) {
            column.nullable = false;
        } else if cursor.eat_kws(&["drop", "not", "null"]) {
            column.nullable = true;
        } else if cursor.eat_kw("type") || cursor.eat_kws(&["set", "data", "type"]) {
            column.sql_type = parse_type(&mut cursor);
        }
        return true;
    }
    false
}

fn drop_object(schema: &mut Schema, cursor: &mut Cursor) -> bool {
    let is_table = cursor.eat_kw("table");
    let is_view = !is_table && (cursor.eat_kw("view") || cursor.eat_kws(&["materialized", "view"]));
    let is_index = !is_table && !is_view && cursor.eat_kw("index");
    if !(is_table || is_view || is_index) {
        return false;
    }
    let _ = cursor.eat_kw("concurrently");
    let _ = cursor.eat_kws(&["if", "exists"]);
    while let Some((schema_name, name)) = cursor.qualified_name() {
        if is_index {
            schema.drop_constraint_anywhere(&name);
        } else {
            schema.remove_table(&schema_name, &name);
        }
        if !cursor.eat_punct(',') {
            break;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema_of(sql: &str) -> Schema {
        let mut schema = Schema::default();
        apply_sql(&mut schema, sql, "db/001.sql");
        schema
    }

    #[test]
    fn create_table_reads_columns_types_nullability_and_named_constraints() {
        let schema = schema_of(
            r#"CREATE TABLE "public"."tb_system_outbox" (
                 "id" UUID NOT NULL DEFAULT uuidv7(),
                 "created_at" TIMESTAMPTZ(3) NOT NULL DEFAULT NOW(),
                 "deleted_at" TIMESTAMPTZ(3) NULL,
                 "payload" JSONB NOT NULL,
                 "amount" NUMERIC(10, 2),
                 "tags" text[],
                 "contract_id" UUID NOT NULL,
                 CONSTRAINT fk_tb_system_outbox_contract FOREIGN KEY ("contract_id") REFERENCES "public"."tb_contract" ("id"),
                 CONSTRAINT uq_tb_system_outbox_payload UNIQUE ("payload"),
                 PRIMARY KEY ("id")
               );"#,
        );
        let table = schema.table("public", "tb_system_outbox").expect("table");
        let names: Vec<_> = table.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["id", "created_at", "deleted_at", "payload", "amount", "tags", "contract_id"]);
        let by = |n: &str| table.columns.iter().find(|c| c.name == n).unwrap();
        assert_eq!((by("id").sql_type.as_str(), by("id").nullable), ("uuid", false));
        assert_eq!(by("created_at").sql_type, "timestamptz(3)");
        assert!(by("deleted_at").nullable && by("amount").nullable);
        assert_eq!(by("amount").sql_type, "numeric(10,2)");
        assert_eq!(by("tags").sql_type, "text[]");
        let kinds: Vec<_> = table.constraints.iter().map(|c| (c.name.as_str(), c.kind)).collect();
        assert!(kinds.contains(&("fk_tb_system_outbox_contract", ConstraintKind::ForeignKey)));
        assert!(kinds.contains(&("uq_tb_system_outbox_payload", ConstraintKind::Unique)));
        assert!(kinds.contains(&("tb_system_outbox_pkey", ConstraintKind::PrimaryKey)));
        let fk = table.constraints.iter().find(|c| c.kind == ConstraintKind::ForeignKey).unwrap();
        assert_eq!(fk.references, Some(("public".to_string(), "tb_contract".to_string())));
        assert_eq!(schema.skipped, 0);
    }

    #[test]
    fn inline_constraints_get_postgres_default_names() {
        let schema = schema_of("create table t (id serial primary key, email text not null unique, owner_id int references users(id));");
        let table = schema.find("t").unwrap();
        let names: Vec<_> = table.constraints.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["t_pkey", "t_email_key", "t_owner_id_fkey"]);
        assert!(!table.columns.iter().find(|c| c.name == "id").unwrap().nullable, "a primary key is not null");
    }

    #[test]
    fn indexes_attach_to_their_table_and_keep_their_names() {
        let schema = schema_of(
            "create table t (a int, b int); \
             CREATE UNIQUE INDEX uq_t_a ON public.t (a); \
             CREATE INDEX CONCURRENTLY IF NOT EXISTS ix_t_b ON t USING gin (b);",
        );
        let table = schema.find("t").unwrap();
        assert!(table.constraints.iter().any(|c| c.name == "uq_t_a" && c.kind == ConstraintKind::UniqueIndex && c.columns == ["a"]));
        assert!(table.constraints.iter().any(|c| c.name == "ix_t_b" && c.kind == ConstraintKind::Index));
    }

    #[test]
    fn views_remember_the_tables_they_read() {
        let schema = schema_of(
            r#"CREATE OR REPLACE VIEW "public"."vw_access_membership" AS
                 SELECT a.id FROM "public"."tb_access" a
                 JOIN tb_member m ON m.access_id = a.id
                 LEFT JOIN (SELECT 1) x ON true;"#,
        );
        let view = schema.find("vw_access_membership").unwrap();
        assert!(view.is_view);
        assert_eq!(view.view_sources, ["tb_access", "tb_member"]);
    }

    #[test]
    fn alter_and_drop_keep_the_schema_in_step_with_the_changesets() {
        let schema = schema_of(
            "create table t (a int, b int, c int); \
             alter table t add column d text not null; \
             alter table t drop column b; \
             alter table t rename column c to cc; \
             alter table t alter column a set not null, alter column d type varchar(10); \
             alter table t add constraint uq_t_a unique (a); \
             create table gone (x int); drop table if exists gone; \
             create index ix on t (a); drop index ix; \
             alter table t rename to t2;",
        );
        assert!(schema.find("t").is_none() && schema.find("gone").is_none());
        let t2 = schema.find("t2").unwrap();
        let cols: Vec<_> = t2.columns.iter().map(|c| (c.name.as_str(), c.sql_type.as_str(), c.nullable)).collect();
        assert_eq!(cols, [("a", "int", false), ("cc", "int", true), ("d", "varchar(10)", false)]);
        assert!(t2.constraints.iter().any(|c| c.name == "uq_t_a"));
        assert!(!t2.constraints.iter().any(|c| c.name == "ix"));
    }

    #[test]
    fn what_is_not_understood_is_counted_and_never_breaks_the_rest() {
        let schema = schema_of(
            "CREATE EXTENSION IF NOT EXISTS pg_trgm; \
             CREATE FUNCTION f() RETURNS trigger AS $$ BEGIN RETURN NEW; END; $$ LANGUAGE plpgsql; \
             -- a comment; with a semicolon\n\
             create table ok (id int); \
             COMMENT ON TABLE ok IS 'it''s; fine';",
        );
        assert!(schema.find("ok").is_some());
        assert_eq!(schema.skipped, 3);
    }
}

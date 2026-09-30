//! Liquibase changelogs and plain `.sql` files (REQ-1402, decision D2 in
//! `.specs/features/domain-extractors/design.md`).
//!
//! Every change is turned into SQL text and fed to [`crate::domain::sql`], so
//! there is one schema logic for all three sources. `<rollback>` blocks are
//! skipped: they describe how to undo a change, not the schema.

use crate::domain::schema::Schema;
use crate::domain::sql::apply_sql;

/// Whether a tracked path can hold schema changes worth reading: any `.sql`, and XML/YAML
/// files that live where migrations usually do.
pub fn is_candidate(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".sql") {
        return true;
    }
    let structured = lower.ends_with(".xml") || lower.ends_with(".yaml") || lower.ends_with(".yml");
    structured && ["changelog", "changeset", "migration", "liquibase", "/db/", "db/"].iter().any(|hint| lower.contains(hint))
}

/// Applies the schema changes in `content` (a file at `path`) to `schema`.
pub fn apply_file(schema: &mut Schema, path: &str, content: &str) {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".sql") {
        apply_sql(schema, content, path);
    } else if lower.ends_with(".xml") && content.contains("databaseChangeLog") {
        for sql in xml_changes(content) {
            apply_sql(schema, &sql, path);
        }
    } else if (lower.ends_with(".yaml") || lower.ends_with(".yml")) && content.contains("databaseChangeLog") {
        for sql in yaml_changes(content) {
            apply_sql(schema, &sql, path);
        }
    }
}

// ---------------------------------------------------------------------------
// XML
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Event {
    Open { name: String, attrs: Vec<(String, String)>, self_closing: bool },
    Close(String),
    Text(String),
}

fn decode_entities(text: &str) -> String {
    text.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

fn local_name(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).to_string()
}

fn parse_tag(inner: &str) -> Event {
    let self_closing = inner.ends_with('/');
    let inner = inner.trim_end_matches('/').trim();
    let name_end = inner.find(|c: char| c.is_whitespace()).unwrap_or(inner.len());
    let name = local_name(&inner[..name_end]);
    let mut attrs = Vec::new();
    let mut rest = inner[name_end..].trim_start();
    while let Some(eq) = rest.find('=') {
        let key = rest[..eq].trim().to_string();
        let after = rest[eq + 1..].trim_start();
        let Some(quote) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else { break };
        let Some(end) = after[1..].find(quote) else { break };
        attrs.push((local_name(&key), decode_entities(&after[1..1 + end])));
        rest = after[end + 2..].trim_start();
    }
    Event::Open { name, attrs, self_closing }
}

fn scan_xml(text: &str) -> Vec<Event> {
    let mut events = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            events.push(Event::Text(decode_entities(rest)));
            break;
        };
        if lt > 0 {
            events.push(Event::Text(decode_entities(&rest[..lt])));
        }
        rest = &rest[lt..];
        if let Some(body) = rest.strip_prefix("<![CDATA[") {
            let end = body.find("]]>").unwrap_or(body.len());
            events.push(Event::Text(body[..end].to_string()));
            rest = &body[(end + 3).min(body.len())..];
        } else if let Some(body) = rest.strip_prefix("<!--") {
            let end = body.find("-->").unwrap_or(body.len());
            rest = &body[(end + 3).min(body.len())..];
        } else if rest.starts_with("<?") || rest.starts_with("<!") {
            let end = rest.find('>').unwrap_or(rest.len() - 1);
            rest = &rest[end + 1..];
        } else if let Some(body) = rest.strip_prefix("</") {
            let end = body.find('>').unwrap_or(body.len());
            events.push(Event::Close(local_name(body[..end].trim())));
            rest = &body[(end + 1).min(body.len())..];
        } else {
            let body = &rest[1..];
            let end = body.find('>').unwrap_or(body.len());
            events.push(parse_tag(&body[..end]));
            rest = &body[(end + 1).min(body.len())..];
        }
    }
    events
}

fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn truthy(value: Option<&str>) -> bool {
    value.is_some_and(|v| v.eq_ignore_ascii_case("true"))
}

fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn qualified(schema: Option<&str>, table: &str) -> String {
    match schema {
        Some(s) if !s.is_empty() => format!("{}.{}", quote(s), quote(table)),
        _ => quote(table),
    }
}

/// One column of `createTable`/`addColumn`, as a fragment of a column definition.
#[derive(Debug, Default, Clone)]
struct ColumnDef {
    name: String,
    sql_type: String,
    not_null: bool,
    primary_key: Option<String>,
    unique: Option<String>,
    references: Option<(String, String, String)>, // (constraint name, table, column)
}

impl ColumnDef {
    fn definition(&self) -> String {
        let mut text = format!("{} {}", quote(&self.name), if self.sql_type.is_empty() { "text" } else { &self.sql_type });
        if self.not_null {
            text.push_str(" NOT NULL");
        }
        text
    }
}

fn column_def(attrs: &[(String, String)]) -> ColumnDef {
    ColumnDef { name: attr(attrs, "name").unwrap_or_default().to_string(), sql_type: attr(attrs, "type").unwrap_or_default().to_string(), ..ColumnDef::default() }
}

fn apply_constraints(column: &mut ColumnDef, attrs: &[(String, String)], table: &str) {
    if attr(attrs, "nullable") == Some("false") || truthy(attr(attrs, "primaryKey")) {
        column.not_null = true;
    }
    if truthy(attr(attrs, "primaryKey")) {
        column.primary_key = Some(attr(attrs, "primaryKeyName").map_or_else(|| format!("{table}_pkey"), str::to_string));
    }
    if truthy(attr(attrs, "unique")) {
        column.unique = Some(attr(attrs, "uniqueConstraintName").map_or_else(|| format!("{table}_{}_key", column.name), str::to_string));
    }
    if let Some(reference_table) = attr(attrs, "referencedTableName") {
        let reference_column = attr(attrs, "referencedColumnNames").unwrap_or("id");
        let name = attr(attrs, "foreignKeyName").map_or_else(|| format!("{table}_{}_fkey", column.name), str::to_string);
        column.references = Some((name, reference_table.to_string(), reference_column.to_string()));
    } else if let Some(references) = attr(attrs, "references") {
        // `references="tb_contract(id)"`
        let (table_name, column_name) = references.split_once('(').map_or((references, "id"), |(t, c)| (t, c.trim_end_matches(')')));
        let name = attr(attrs, "foreignKeyName").map_or_else(|| format!("{table}_{}_fkey", column.name), str::to_string);
        column.references = Some((name, table_name.trim().to_string(), column_name.trim().to_string()));
    }
}

fn create_table_sql(schema: Option<&str>, table: &str, columns: &[ColumnDef]) -> String {
    let mut items: Vec<String> = columns.iter().map(ColumnDef::definition).collect();
    for column in columns {
        if let Some(name) = &column.primary_key {
            items.push(format!("CONSTRAINT {} PRIMARY KEY ({})", quote(name), quote(&column.name)));
        }
        if let Some(name) = &column.unique {
            items.push(format!("CONSTRAINT {} UNIQUE ({})", quote(name), quote(&column.name)));
        }
        if let Some((name, target, target_column)) = &column.references {
            items.push(format!("CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({})", quote(name), quote(&column.name), quote(target), quote(target_column)));
        }
    }
    format!("CREATE TABLE {} ({})", qualified(schema, table), items.join(", "))
}

#[derive(Default)]
struct Pending {
    kind: String,
    attrs: Vec<(String, String)>,
    columns: Vec<ColumnDef>,
    text: String,
}

fn xml_changes(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip_rollback = 0usize;
    let mut in_sql = false;
    let mut sql = String::new();
    let mut pending: Option<Pending> = None;
    for event in scan_xml(text) {
        if skip_rollback > 0 {
            match &event {
                Event::Open { name, self_closing: false, .. } if name == "rollback" => skip_rollback += 1,
                Event::Close(name) if name == "rollback" => skip_rollback -= 1,
                _ => {}
            }
            continue;
        }
        match event {
            Event::Open { name, self_closing, .. } if name == "rollback" => {
                if !self_closing {
                    skip_rollback = 1;
                }
            }
            Event::Open { name, .. } if name == "sql" => {
                in_sql = true;
                sql.clear();
            }
            Event::Close(name) if name == "sql" => {
                in_sql = false;
                out.push(std::mem::take(&mut sql));
            }
            Event::Text(t) if in_sql => sql.push_str(&t),
            Event::Open { name, attrs, self_closing } => match name.as_str() {
                "createTable" | "addColumn" | "createIndex" | "createView" | "dropTable" | "dropView" | "dropColumn" | "addForeignKeyConstraint" => {
                    let done = Pending { kind: name, attrs, ..Pending::default() };
                    if self_closing {
                        if let Some(statement) = finish(&done) {
                            out.push(statement);
                        }
                    } else {
                        pending = Some(done);
                    }
                }
                "column" => {
                    if let Some(p) = pending.as_mut() {
                        p.columns.push(column_def(&attrs));
                    }
                }
                "constraints" => {
                    if let Some(p) = pending.as_mut() {
                        let table = attr(&p.attrs, "tableName").unwrap_or_default().to_string();
                        if let Some(column) = p.columns.last_mut() {
                            apply_constraints(column, &attrs, &table);
                        }
                    }
                }
                _ => {}
            },
            Event::Text(t) => {
                if let Some(p) = pending.as_mut() {
                    p.text.push_str(&t);
                }
            }
            Event::Close(name) => {
                if pending.as_ref().is_some_and(|p| p.kind == name)
                    && let Some(done) = pending.take()
                    && let Some(statement) = finish(&done)
                {
                    out.push(statement);
                }
            }
        }
    }
    out
}

fn finish(p: &Pending) -> Option<String> {
    let schema = attr(&p.attrs, "schemaName");
    match p.kind.as_str() {
        "createTable" => Some(create_table_sql(schema, attr(&p.attrs, "tableName")?, &p.columns)),
        "addColumn" => {
            let table = qualified(schema, attr(&p.attrs, "tableName")?);
            Some(p.columns.iter().map(|c| format!("ALTER TABLE {table} ADD COLUMN {};", c.definition())).collect::<Vec<_>>().join(" "))
        }
        "dropColumn" => Some(format!("ALTER TABLE {} DROP COLUMN {}", qualified(schema, attr(&p.attrs, "tableName")?), quote(attr(&p.attrs, "columnName")?))),
        "dropTable" => Some(format!("DROP TABLE {}", qualified(schema, attr(&p.attrs, "tableName")?))),
        "dropView" => Some(format!("DROP VIEW {}", qualified(schema, attr(&p.attrs, "viewName")?))),
        "createView" => Some(format!("CREATE VIEW {} AS {}", qualified(schema, attr(&p.attrs, "viewName")?), p.text)),
        "createIndex" => {
            let columns: Vec<String> = p.columns.iter().map(|c| quote(&c.name)).collect();
            Some(format!(
                "CREATE {}INDEX {} ON {} ({})",
                if truthy(attr(&p.attrs, "unique")) { "UNIQUE " } else { "" },
                quote(attr(&p.attrs, "indexName")?),
                qualified(schema, attr(&p.attrs, "tableName")?),
                columns.join(", ")
            ))
        }
        "addForeignKeyConstraint" => {
            let list = |key: &str| attr(&p.attrs, key).unwrap_or_default().split(',').map(|c| quote(c.trim())).collect::<Vec<_>>().join(", ");
            Some(format!(
                "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({})",
                qualified(attr(&p.attrs, "baseTableSchemaName"), attr(&p.attrs, "baseTableName")?),
                quote(attr(&p.attrs, "constraintName")?),
                list("baseColumnNames"),
                qualified(attr(&p.attrs, "referencedTableSchemaName"), attr(&p.attrs, "referencedTableName")?),
                list("referencedColumnNames"),
            ))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// YAML
// ---------------------------------------------------------------------------

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    match (value.chars().next(), value.chars().last()) {
        (Some(a @ ('"' | '\'')), Some(b)) if a == b && value.len() >= 2 => value[1..value.len() - 1].to_string(),
        _ => value.to_string(),
    }
}

/// `sql:` blocks and `createTable` of a YAML changelog (rollbacks skipped).
fn yaml_changes(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start().trim_start_matches("- ");
        let base = indent_of(line);
        if trimmed.starts_with("rollback:") {
            i += 1;
            while i < lines.len() && (lines[i].trim().is_empty() || indent_of(lines[i]) > base) {
                i += 1;
            }
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("sql:") {
            let value = value.trim();
            if value.starts_with('|') || value.starts_with('>') {
                let mut block = Vec::new();
                i += 1;
                while i < lines.len() && (lines[i].trim().is_empty() || indent_of(lines[i]) > base) {
                    block.push(lines[i].trim());
                    i += 1;
                }
                out.push(block.join("\n"));
                continue;
            }
            out.push(unquote(value));
        } else if trimmed.starts_with("createTable:") {
            let mut table = String::new();
            let mut schema: Option<String> = None;
            let mut columns: Vec<ColumnDef> = Vec::new();
            i += 1;
            while i < lines.len() && (lines[i].trim().is_empty() || indent_of(lines[i]) > base) {
                let item = lines[i].trim_start().trim_start_matches("- ");
                if let Some((key, value)) = item.split_once(':') {
                    let value = unquote(value);
                    match key.trim() {
                        "tableName" => table = value,
                        "schemaName" => schema = Some(value),
                        "name" => columns.push(ColumnDef { name: value, ..ColumnDef::default() }),
                        "type" => {
                            if let Some(c) = columns.last_mut() {
                                c.sql_type = value;
                            }
                        }
                        "nullable" if value == "false" => {
                            if let Some(c) = columns.last_mut() {
                                c.not_null = true;
                            }
                        }
                        "primaryKey" if value == "true" => {
                            if let Some(c) = columns.last_mut() {
                                c.not_null = true;
                                c.primary_key = Some(format!("{table}_pkey"));
                            }
                        }
                        "unique" if value == "true" => {
                            if let Some(c) = columns.last_mut() {
                                c.unique = Some(format!("{table}_{}_key", c.name));
                            }
                        }
                        _ => {}
                    }
                }
                i += 1;
            }
            if !table.is_empty() {
                out.push(create_table_sql(schema.as_deref(), &table, &columns));
            }
            continue;
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema_of(path: &str, content: &str) -> Schema {
        let mut schema = Schema::default();
        apply_file(&mut schema, path, content);
        schema
    }

    const SQL_CHANGESET: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
  <changeSet id="create-table-x" author="a">
    <sql splitStatements="false" endDelimiter=";"><![CDATA[
      CREATE TABLE "public"."tb_x" (
        "id" UUID NOT NULL,
        "name" VARCHAR(10) NULL,
        PRIMARY KEY ("id")
      );
      CREATE INDEX ix_tb_x_name ON "public"."tb_x" ("name");
    ]]></sql>
    <rollback><sql><![CDATA[ DROP TABLE "public"."tb_x"; ]]></sql></rollback>
  </changeSet>
</databaseChangeLog>"#;

    #[test]
    fn sql_inside_a_changeset_builds_the_schema_and_the_rollback_does_not_undo_it() {
        let schema = schema_of("db/changeset/001.xml", SQL_CHANGESET);
        let table = schema.find("tb_x").expect("rollback must not drop it");
        assert_eq!(table.columns.len(), 2);
        assert!(table.constraints.iter().any(|c| c.name == "ix_tb_x_name"));
        assert_eq!(table.defined_in, "db/changeset/001.xml");
    }

    #[test]
    fn xml_change_elements_become_the_same_schema() {
        let xml = r#"<databaseChangeLog xmlns="x">
          <changeSet id="1" author="a">
            <createTable tableName="tb_a">
              <column name="id" type="uuid"><constraints primaryKey="true" nullable="false"/></column>
              <column name="b_id" type="uuid"><constraints nullable="false" foreignKeyName="fk_tb_a_b" referencedTableName="tb_b" referencedColumnNames="id"/></column>
              <column name="code" type="varchar(5)"><constraints unique="true" uniqueConstraintName="uq_tb_a_code"/></column>
            </createTable>
            <createIndex indexName="ix_tb_a_code" tableName="tb_a" unique="true"><column name="code"/></createIndex>
            <addColumn tableName="tb_a"><column name="note" type="text"/></addColumn>
            <dropColumn tableName="tb_a" columnName="code"/>
            <rollback><dropTable tableName="tb_a"/></rollback>
          </changeSet>
        </databaseChangeLog>"#;
        let schema = schema_of("db/changelog-1.xml", xml);
        let table = schema.find("tb_a").unwrap();
        let columns: Vec<_> = table.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(columns, ["id", "b_id", "note"]);
        assert!(!table.columns[0].nullable);
        let fk = table.constraints.iter().find(|c| c.name == "fk_tb_a_b").expect("named foreign key");
        assert_eq!(fk.references, Some(("public".to_string(), "tb_b".to_string())));
        assert!(table.constraints.iter().any(|c| c.name == "tb_a_pkey"));
    }

    #[test]
    fn yaml_changelogs_give_sql_blocks_and_create_table() {
        let yaml = "databaseChangeLog:\n  - changeSet:\n      id: 1\n      changes:\n        - sql:\n            sql: |\n              CREATE TABLE tb_y (id int);\n        - createTable:\n            tableName: tb_z\n            columns:\n              - column:\n                  name: id\n                  type: uuid\n                  constraints:\n                    primaryKey: true\n      rollback:\n        - sql:\n            sql: DROP TABLE tb_y;\n";
        let schema = schema_of("db/changelog.yaml", yaml);
        assert!(schema.find("tb_y").is_some(), "the rollback must not drop it");
        assert!(!schema.find("tb_z").unwrap().columns[0].nullable);
    }

    #[test]
    fn plain_sql_files_and_unrelated_xml_are_handled() {
        let schema = schema_of("migrations/V1__init.sql", "--liquibase formatted sql\n--changeset a:1\ncreate table t (id int);\n--rollback drop table t;");
        assert!(schema.find("t").is_some());
        assert!(schema_of("pom.xml", "<project><sql>create table no (id int)</sql></project>").is_empty(), "not a changelog");
    }

    #[test]
    fn candidates_are_sql_files_and_structured_files_where_migrations_live() {
        assert!(is_candidate("db/changeset/001.xml") && is_candidate("x/schema.sql") && is_candidate("src/db/changelog.yaml"));
        assert!(!is_candidate("pom.xml") && !is_candidate("src/app.ts") && !is_candidate("docs/readme.md"));
    }
}

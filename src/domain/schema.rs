//! The database schema as the changesets leave it (REQ-1402 in
//! `.specs/features/domain-extractors/spec.md`): tables, views, columns and named
//! constraints, built up statement by statement in the order they are applied.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const DEFAULT_SCHEMA: &str = "public";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    pub sql_type: String,
    pub nullable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConstraintKind {
    PrimaryKey,
    Unique,
    ForeignKey,
    Check,
    Index,
    UniqueIndex,
}

impl ConstraintKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PrimaryKey => "primary_key",
            Self::Unique => "unique",
            Self::ForeignKey => "foreign_key",
            Self::Check => "check",
            Self::Index => "index",
            Self::UniqueIndex => "unique_index",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Constraint {
    pub name: String,
    pub kind: ConstraintKind,
    pub columns: Vec<String>,
    /// For a foreign key: `(schema, table)` it points at.
    pub references: Option<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Table {
    pub schema: String,
    pub name: String,
    pub is_view: bool,
    pub columns: Vec<Column>,
    pub constraints: Vec<Constraint>,
    /// Repository-relative path of the file that created it.
    pub defined_in: String,
    /// For a view: table and view names found after `FROM`/`JOIN` (textual, resolved later).
    pub view_sources: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Schema {
    tables: BTreeMap<(String, String), Table>,
    /// Statements that were recognised as SQL but not understood (functions, triggers, …).
    pub skipped: usize,
}

#[derive(Serialize, Deserialize)]
struct SchemaFile {
    tables: Vec<Table>,
}

impl Schema {
    /// JSON form, for the live-schema cache (`tables` in a stable order).
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&SchemaFile { tables: self.tables.values().cloned().collect() }).expect("schema serialises")
    }

    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        let file: SchemaFile = serde_json::from_str(text)?;
        let mut schema = Schema::default();
        for table in file.tables {
            schema.upsert_table(table);
        }
        Ok(schema)
    }

    pub fn tables(&self) -> impl Iterator<Item = &Table> {
        self.tables.values()
    }

    pub fn table(&self, schema: &str, name: &str) -> Option<&Table> {
        self.tables.get(&(schema.to_string(), name.to_string()))
    }

    /// A table or view by bare name, in any schema (`public` first).
    pub fn find(&self, name: &str) -> Option<&Table> {
        self.table(DEFAULT_SCHEMA, name).or_else(|| self.tables.values().find(|t| t.name == name))
    }

    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tables.len()
    }

    /// Adds what `other` has and this schema lacks: whole tables, and columns or constraints of
    /// tables both know. Nothing this schema already says is overwritten (the changesets win).
    pub fn merge_missing(&mut self, other: &Schema) {
        for theirs in other.tables() {
            match self.table_mut(&theirs.schema, &theirs.name) {
                None => self.upsert_table(theirs.clone()),
                Some(ours) => {
                    for column in &theirs.columns {
                        if !ours.columns.iter().any(|c| c.name == column.name) {
                            ours.columns.push(column.clone());
                        }
                    }
                    for constraint in &theirs.constraints {
                        if !ours.constraints.iter().any(|c| c.name == constraint.name) {
                            ours.constraints.push(constraint.clone());
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn upsert_table(&mut self, table: Table) {
        self.tables.insert((table.schema.clone(), table.name.clone()), table);
    }

    pub(crate) fn table_mut(&mut self, schema: &str, name: &str) -> Option<&mut Table> {
        self.tables.get_mut(&(schema.to_string(), name.to_string()))
    }

    pub(crate) fn remove_table(&mut self, schema: &str, name: &str) {
        self.tables.remove(&(schema.to_string(), name.to_string()));
    }

    pub(crate) fn rename_table(&mut self, schema: &str, from: &str, to: &str) {
        if let Some(mut table) = self.tables.remove(&(schema.to_string(), from.to_string())) {
            table.name = to.to_string();
            self.tables.insert((schema.to_string(), to.to_string()), table);
        }
    }

    pub(crate) fn drop_constraint_anywhere(&mut self, name: &str) {
        for table in self.tables.values_mut() {
            table.constraints.retain(|c| c.name != name);
        }
    }
}

impl Table {
    pub fn new(schema: &str, name: &str, is_view: bool, defined_in: &str) -> Self {
        Self { schema: schema.to_string(), name: name.to_string(), is_view, columns: Vec::new(), constraints: Vec::new(), defined_in: defined_in.to_string(), view_sources: Vec::new() }
    }

    pub fn column_mut(&mut self, name: &str) -> Option<&mut Column> {
        self.columns.iter_mut().find(|c| c.name == name)
    }

    /// Adds or replaces a constraint by name (`CREATE INDEX` of an existing name replaces it).
    pub fn put_constraint(&mut self, constraint: Constraint) {
        match self.constraints.iter_mut().find(|c| c.name == constraint.name) {
            Some(existing) => *existing = constraint,
            None => self.constraints.push(constraint),
        }
    }
}

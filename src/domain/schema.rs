//! The database schema as the changesets leave it (REQ-1402 in
//! `.specs/features/domain-extractors/spec.md`): tables, views, columns and named
//! constraints, built up statement by statement in the order they are applied.

use std::collections::BTreeMap;

pub const DEFAULT_SCHEMA: &str = "public";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    pub sql_type: String,
    pub nullable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Constraint {
    pub name: String,
    pub kind: ConstraintKind,
    pub columns: Vec<String>,
    /// For a foreign key: `(schema, table)` it points at.
    pub references: Option<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

impl Schema {
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

//! Domain extractors (Fase 14): database schema, ORM bridge and package manifests.
//! See `.specs/features/domain-extractors/`.

pub mod schema;
pub mod sql;
pub mod liquibase;
pub mod manifest;
pub mod orm;
pub mod graph;

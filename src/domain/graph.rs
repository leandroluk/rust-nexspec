//! Assembles the domain subgraph (REQ-1402..1404) from the raw files, and works out
//! what has to change in the index to match it (decision D8 in
//! `.specs/features/domain-extractors/design.md`).

use std::collections::{BTreeMap, HashSet};

use crate::code::parser::edge_id;
use crate::domain::schema::Schema;
use crate::domain::{liquibase, manifest, orm};
use crate::graph::edge::{Confidence, EdgeContext, EdgeType, encode_meta};
use crate::graph::node::{NodePayload, column_node_id, constraint_node_id, file_node_id, package_node_id, symbol_node_id, table_node_id};
use crate::sync::mutation::{EdgeMutation, MutationSet, NodeMutation, StableId};

/// Everything the pass reads, already loaded.
#[derive(Debug, Default, Clone)]
pub struct DomainInput {
    /// `(path, content)` of SQL/Liquibase candidates.
    pub schema_files: Vec<(String, String)>,
    /// `(path, content)` of manifests (`package.json`, `Cargo.toml`, `tsconfig*.json`).
    pub manifests: Vec<(String, String)>,
    /// `(path, content)` of code files that hold an entity marker (and every `.prisma` file).
    pub entity_files: Vec<(String, String)>,
    /// Tracked repository paths, to keep edges to files that exist.
    pub tracked: HashSet<String>,
    /// What `extract --postgres` last read from a live database (REQ-1405): objects that exist only
    /// there are added to the graph.
    pub live: Option<Schema>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DomainStats {
    pub tables: usize,
    pub views: usize,
    pub columns: usize,
    pub constraints: usize,
    pub packages: usize,
    pub entity_links: usize,
    /// SQL statements recognised as SQL but not understood.
    pub skipped_statements: usize,
}

#[derive(Debug, Default, Clone)]
pub struct DomainGraph {
    pub nodes: Vec<NodeMutation>,
    pub edges: Vec<EdgeMutation>,
    pub stats: DomainStats,
    /// The schema the changesets add up to (for `extract`, drift and tests).
    pub schema: Schema,
}

fn node(payload: NodePayload, id: StableId) -> NodeMutation {
    NodeMutation::Upsert { id, payload: rkyv::to_bytes::<rkyv::rancor::Error>(&payload).expect("NodePayload must always serialize").to_vec() }
}

struct Edges(BTreeMap<StableId, EdgeMutation>);

impl Edges {
    fn add(&mut self, kind: &str, edge_type: EdgeType, from: StableId, to: StableId, confidence: Confidence) {
        let id = edge_id(kind, &from, &to);
        self.0.insert(
            id,
            EdgeMutation::Upsert { id, from, to, edge_type: edge_type.to_code(), payload: vec![encode_meta(confidence, EdgeContext::Runtime)] },
        );
    }
}

pub fn build(input: &DomainInput) -> DomainGraph {
    let mut schema = Schema::default();
    let mut schema_files: Vec<&(String, String)> = input.schema_files.iter().collect();
    schema_files.sort_by(|a, b| a.0.cmp(&b.0));
    for (path, content) in schema_files {
        liquibase::apply_file(&mut schema, path, content);
    }

    if let Some(live) = &input.live {
        schema.merge_missing(live);
    }

    let mut nodes: BTreeMap<StableId, NodeMutation> = BTreeMap::new();
    let mut edges = Edges(BTreeMap::new());
    let mut stats = DomainStats { skipped_statements: schema.skipped, ..DomainStats::default() };

    for table in schema.tables() {
        let table_id = table_node_id(&table.schema, &table.name);
        nodes.insert(table_id, node(NodePayload::Table { schema: table.schema.clone(), name: table.name.clone(), is_view: table.is_view }, table_id));
        if table.is_view {
            stats.views += 1;
        } else {
            stats.tables += 1;
        }
        if input.tracked.contains(&table.defined_in) {
            edges.add("domain-defined-in", EdgeType::DefinedIn, table_id, file_node_id(&table.defined_in), Confidence::Extracted);
        }
        for column in &table.columns {
            let id = column_node_id(&table.schema, &table.name, &column.name);
            nodes.insert(id, node(NodePayload::Column { table: table.name.clone(), name: column.name.clone(), sql_type: column.sql_type.clone(), nullable: column.nullable }, id));
            edges.add("domain-defined-in", EdgeType::DefinedIn, id, table_id, Confidence::Extracted);
            stats.columns += 1;
        }
        for constraint in &table.constraints {
            let id = constraint_node_id(&table.schema, &table.name, &constraint.name);
            nodes.insert(id, node(NodePayload::Constraint { table: table.name.clone(), name: constraint.name.clone(), kind: constraint.kind.as_str().to_string() }, id));
            edges.add("domain-defined-in", EdgeType::DefinedIn, id, table_id, Confidence::Extracted);
            stats.constraints += 1;
            if let Some((target_schema, target)) = &constraint.references
                && let Some(referenced) = schema.table(target_schema, target).or_else(|| schema.find(target))
            {
                edges.add("domain-references", EdgeType::References, table_id, table_node_id(&referenced.schema, &referenced.name), Confidence::Extracted);
            }
        }
        for source in &table.view_sources {
            if let Some(referenced) = schema.find(source)
                && referenced.name != table.name
            {
                edges.add("domain-references", EdgeType::References, table_id, table_node_id(&referenced.schema, &referenced.name), Confidence::Inferred);
            }
        }
    }

    // Packages.
    let mut packages: Vec<manifest::PackageInfo> = Vec::new();
    let mut manifests: Vec<&(String, String)> = input.manifests.iter().collect();
    manifests.sort_by(|a, b| a.0.cmp(&b.0));
    for (path, content) in manifests {
        let name = path.rsplit('/').next().unwrap_or(path);
        if name == "package.json" {
            packages.extend(manifest::parse_package_json(path, content));
        } else if name == "Cargo.toml" {
            packages.extend(manifest::parse_cargo_toml(path, content));
        } else {
            for target in manifest::tsconfig_extends(path, content) {
                if input.tracked.contains(&target) {
                    edges.add("domain-tsconfig-extends", EdgeType::DependsOn, file_node_id(path), file_node_id(&target), Confidence::Extracted);
                }
            }
        }
    }
    // Two manifests with the same name (a package and its example) keep the first, by path.
    let mut seen = HashSet::new();
    packages.retain(|p| seen.insert(p.name.clone()));
    for package in &packages {
        let id = package_node_id(&package.name);
        nodes.insert(id, node(NodePayload::Package { name: package.name.clone(), version: package.version.clone(), dir: package.dir.clone() }, id));
        stats.packages += 1;
        if input.tracked.contains(&package.manifest) {
            edges.add("domain-defined-in", EdgeType::DefinedIn, id, file_node_id(&package.manifest), Confidence::Extracted);
        }
    }
    for (from, to) in manifest::workspace_dependencies(&packages) {
        edges.add("domain-depends-on", EdgeType::DependsOn, package_node_id(&from), package_node_id(&to), Confidence::Extracted);
    }

    // ORM bridge.
    for (path, content) in &input.entity_files {
        for link in orm::scan(path, content) {
            let table = match &link.schema {
                Some(schema_name) => schema.table(schema_name, &link.table),
                None => schema.find(&link.table),
            };
            let Some(table) = table else { continue };
            let from = if link.has_symbol { symbol_node_id(&link.path, &link.name, link.ordinal) } else { file_node_id(&link.path) };
            edges.add("domain-implements", EdgeType::Implements, from, table_node_id(&table.schema, &table.name), Confidence::Extracted);
            stats.entity_links += 1;
        }
    }

    DomainGraph { nodes: nodes.into_values().collect(), edges: edges.0.into_values().collect(), stats, schema }
}

/// What has to change in the index so that its domain part equals `graph`:
/// upserts of everything new, removal of domain nodes that disappeared and of the
/// domain-owned edges that are no longer produced.
///
/// `existing_nodes` are the domain nodes already indexed; `existing_edges` are the
/// indexed edges as `(id, from, to, is_domain_type)` where `is_domain_type` marks
/// edges no other pass produces (generic `DependsOn`).
pub fn reconcile(graph: &DomainGraph, existing_nodes: &HashSet<StableId>, existing_edges: &[(StableId, StableId, StableId, bool)]) -> MutationSet {
    let new_nodes: HashSet<StableId> = graph
        .nodes
        .iter()
        .filter_map(|n| match n {
            NodeMutation::Upsert { id, .. } => Some(*id),
            NodeMutation::Remove { .. } => None,
        })
        .collect();
    let new_edges: HashSet<StableId> = graph
        .edges
        .iter()
        .filter_map(|e| match e {
            EdgeMutation::Upsert { id, .. } => Some(*id),
            EdgeMutation::Remove { .. } => None,
        })
        .collect();
    let mut set = MutationSet::default();
    set.nodes.extend(graph.nodes.iter().cloned());
    set.edges.extend(graph.edges.iter().cloned());
    let mut removed_nodes: Vec<StableId> = existing_nodes.difference(&new_nodes).copied().collect();
    removed_nodes.sort();
    set.nodes.extend(removed_nodes.into_iter().map(|id| NodeMutation::Remove { id }));
    let mut removed_edges: Vec<StableId> = existing_edges
        .iter()
        .filter(|(id, from, to, domain_type)| !new_edges.contains(id) && (*domain_type || existing_nodes.contains(from) || existing_nodes.contains(to)))
        .map(|(id, ..)| *id)
        .collect();
    removed_edges.sort();
    removed_edges.dedup();
    set.edges.extend(removed_edges.into_iter().map(|id| EdgeMutation::Remove { id }));
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    const CREATE_CONTRACT: &str = r#"<databaseChangeLog><changeSet id="1" author="a"><sql><![CDATA[
        CREATE TABLE "public"."tb_contract" ("id" UUID NOT NULL, PRIMARY KEY ("id"));
        CREATE TABLE "public"."tb_contract_reminder" (
          "id" UUID NOT NULL, "contract_id" UUID NOT NULL,
          CONSTRAINT fk_tb_contract_reminder_contract FOREIGN KEY ("contract_id") REFERENCES "public"."tb_contract" ("id"),
          PRIMARY KEY ("id"));
        CREATE OR REPLACE VIEW "public"."vw_contract_reminder" AS SELECT r.id FROM tb_contract_reminder r JOIN tb_contract c ON c.id = r.contract_id;
    ]]></sql></changeSet></databaseChangeLog>"#;

    fn input() -> DomainInput {
        DomainInput {
            schema_files: vec![("db/changeset/001.xml".into(), CREATE_CONTRACT.into())],
            manifests: vec![
                ("apps/web/package.json".into(), r#"{ "name": "web", "dependencies": { "ui": "*" } }"#.into()),
                ("pkgs/ui/package.json".into(), r#"{ "name": "ui" }"#.into()),
                ("apps/web/tsconfig.json".into(), r#"{ "extends": "../../tsconfig.base.json" }"#.into()),
            ],
            entity_files: vec![("src/reminder.entity.ts".into(), "@Entity({name: 'tb_contract_reminder'})\nexport class ContractReminderEntity {}\n".into())],
            tracked: ["db/changeset/001.xml", "apps/web/package.json", "pkgs/ui/package.json", "apps/web/tsconfig.json", "tsconfig.base.json", "src/reminder.entity.ts"].iter().map(|s| s.to_string()).collect(),
            live: None,
        }
    }

    fn edge_set(graph: &DomainGraph) -> Vec<(u16, StableId, StableId)> {
        graph
            .edges
            .iter()
            .filter_map(|e| match e {
                EdgeMutation::Upsert { from, to, edge_type, .. } => Some((*edge_type, *from, *to)),
                EdgeMutation::Remove { .. } => None,
            })
            .collect()
    }

    #[test]
    fn the_schema_becomes_tables_columns_constraints_and_their_relations() {
        let graph = build(&input());
        assert_eq!((graph.stats.tables, graph.stats.views, graph.stats.columns, graph.stats.constraints, graph.stats.packages), (2, 1, 3, 3, 2));
        let edges = edge_set(&graph);
        let (contract, reminder, view) = (table_node_id("public", "tb_contract"), table_node_id("public", "tb_contract_reminder"), table_node_id("public", "vw_contract_reminder"));
        assert!(edges.contains(&(EdgeType::References.to_code(), reminder, contract)), "foreign key");
        assert!(edges.contains(&(EdgeType::References.to_code(), view, reminder)) && edges.contains(&(EdgeType::References.to_code(), view, contract)), "view sources");
        assert!(edges.contains(&(EdgeType::DefinedIn.to_code(), reminder, file_node_id("db/changeset/001.xml"))));
        assert!(edges.contains(&(EdgeType::DefinedIn.to_code(), column_node_id("public", "tb_contract_reminder", "contract_id"), reminder)));
        assert!(edges.contains(&(EdgeType::DefinedIn.to_code(), constraint_node_id("public", "tb_contract_reminder", "fk_tb_contract_reminder_contract"), reminder)));
    }

    #[test]
    fn entities_packages_and_tsconfig_chains_are_linked() {
        let graph = build(&input());
        let edges = edge_set(&graph);
        let entity = symbol_node_id("src/reminder.entity.ts", "ContractReminderEntity", 0);
        assert!(edges.contains(&(EdgeType::Implements.to_code(), entity, table_node_id("public", "tb_contract_reminder"))), "entity -> table");
        assert!(edges.contains(&(EdgeType::DependsOn.to_code(), package_node_id("web"), package_node_id("ui"))), "workspace dependency");
        assert!(edges.contains(&(EdgeType::DependsOn.to_code(), file_node_id("apps/web/tsconfig.json"), file_node_id("tsconfig.base.json"))), "extends chain");
        assert_eq!(graph.stats.entity_links, 1);
    }

    #[test]
    fn objects_that_exist_only_in_the_live_database_join_the_graph() {
        use crate::domain::schema::{Column, Table};
        let mut live = Schema::default();
        let mut only_live = Table::new("public", "tb_only_live", false, "");
        only_live.columns.push(Column { name: "id".into(), sql_type: "uuid".into(), nullable: false });
        live.upsert_table(only_live);
        let mut i = input();
        i.live = Some(live);
        let graph = build(&i);
        assert_eq!(graph.stats.tables, 3, "the two from the changesets and the live-only one");
        assert!(graph.nodes.iter().any(|n| matches!(n, NodeMutation::Upsert { id, .. } if *id == table_node_id("public", "tb_only_live"))));
        assert!(!edge_set(&graph).iter().any(|(t, from, _)| *t == EdgeType::DefinedIn.to_code() && *from == table_node_id("public", "tb_only_live")), "no file defines it");
    }

    #[test]
    fn an_entity_for_a_table_that_does_not_exist_is_not_linked() {
        let mut i = input();
        i.entity_files = vec![("a.entity.ts".into(), "@Entity('tb_nowhere')\nclass A {}\n".into())];
        assert_eq!(build(&i).stats.entity_links, 0);
    }

    #[test]
    fn the_output_is_deterministic_whatever_the_input_order() {
        let mut shuffled = input();
        shuffled.manifests.reverse();
        let (a, b) = (build(&input()), build(&shuffled));
        assert_eq!(edge_set(&a), edge_set(&b));
        assert_eq!(a.nodes.len(), b.nodes.len());
    }

    #[test]
    fn reconcile_upserts_everything_and_removes_what_disappeared() {
        let graph = build(&input());
        let old_table = table_node_id("public", "tb_gone");
        let old_edge = edge_id("domain-references", &old_table, &table_node_id("public", "tb_contract"));
        let untouched_edge = edge_id("imports", &[1u8; 32], &[2u8; 32]);
        let existing_nodes: HashSet<StableId> = [old_table, table_node_id("public", "tb_contract")].into();
        let existing_edges = vec![(old_edge, old_table, table_node_id("public", "tb_contract"), false), (untouched_edge, [1u8; 32], [2u8; 32], false)];
        let set = reconcile(&graph, &existing_nodes, &existing_edges);

        assert!(set.nodes.iter().any(|n| matches!(n, NodeMutation::Remove { id } if *id == old_table)), "the dropped table goes");
        assert!(set.edges.iter().any(|e| matches!(e, EdgeMutation::Remove { id } if *id == old_edge)), "and so do its edges");
        assert!(!set.edges.iter().any(|e| matches!(e, EdgeMutation::Remove { id } if *id == untouched_edge)), "edges the pass does not own stay");
        assert!(!set.nodes.iter().any(|n| matches!(n, NodeMutation::Remove { id } if *id == table_node_id("public", "tb_contract"))), "a surviving table is not removed");

        let again = reconcile(&graph, &graph_node_ids(&graph), &graph_edges(&graph));
        assert!(again.nodes.iter().all(|n| matches!(n, NodeMutation::Upsert { .. })) && again.edges.iter().all(|e| matches!(e, EdgeMutation::Upsert { .. })), "idempotent: nothing to remove");
    }

    fn graph_node_ids(graph: &DomainGraph) -> HashSet<StableId> {
        graph.nodes.iter().filter_map(|n| if let NodeMutation::Upsert { id, .. } = n { Some(*id) } else { None }).collect()
    }

    fn graph_edges(graph: &DomainGraph) -> Vec<(StableId, StableId, StableId, bool)> {
        graph.edges.iter().filter_map(|e| if let EdgeMutation::Upsert { id, from, to, .. } = e { Some((*id, *from, *to, false)) } else { None }).collect()
    }
}

//! Domain pass end to end (T-1405, REQ-1402..1404): a Liquibase schema, an ORM entity and workspace
//! manifests become nodes and edges that `search`, `explain` and `affected` can use.

mod fixtures;

use std::path::Path;
use std::process::{Command, Output};

use fixtures::FixtureRepo;

fn run(repo: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nexspec")).arg("--repo").arg(repo).args(args).output().unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn ok(o: &Output) -> &Output {
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    o
}

const CHANGESET: &str = r#"<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
  <changeSet id="create-contract" author="a">
    <sql splitStatements="false"><![CDATA[
      CREATE TABLE "public"."tb_contract" ("id" UUID NOT NULL, PRIMARY KEY ("id"));
      CREATE TABLE "public"."tb_contract_reminder" (
        "id" UUID NOT NULL,
        "contract_id" UUID NOT NULL,
        CONSTRAINT fk_tb_contract_reminder_contract FOREIGN KEY ("contract_id") REFERENCES "public"."tb_contract" ("id"),
        PRIMARY KEY ("id")
      );
      CREATE INDEX ix_tb_contract_reminder_contract ON "public"."tb_contract_reminder" ("contract_id");
    ]]></sql>
  </changeSet>
</databaseChangeLog>"#;

fn repo() -> FixtureRepo {
    let repo = FixtureRepo::init();
    repo.write_file(".gitignore", ".specs/.index/\n");
    repo.write_file("db/changeset/001-create-contract.xml", CHANGESET);
    repo.write_file(
        "src/contract/reminder.entity.ts",
        "@Entity({name: 'tb_contract_reminder'})\nexport class ContractReminderEntity {\n  id: string;\n}\n",
    );
    repo.write_file("src/contract/reminder.repository.ts", "import { ContractReminderEntity } from './reminder.entity';\nexport class ReminderRepository { find(): ContractReminderEntity | null { return null; } }\n");
    repo.write_file("apps/web/package.json", r#"{ "name": "web", "dependencies": { "ui": "*" } }"#);
    repo.write_file("pkgs/ui/package.json", r#"{ "name": "ui" }"#);
    repo.commit("init");
    repo
}

#[test]
fn tables_are_found_explained_and_reach_entity_and_repository() {
    let repo = repo();
    ok(&run(repo.path(), &["sync"]));

    let found = out(ok(&run(repo.path(), &["search", "contract reminder"])));
    assert!(found.contains("table tb_contract_reminder"), "a table is findable by its words: {found}");

    let explained = out(ok(&run(repo.path(), &["explain", "tb_contract_reminder"])));
    assert!(explained.contains("tb_contract_reminder"), "{explained}");
    assert!(explained.contains("fk_tb_contract_reminder_contract") || explained.contains("tb_contract"), "{explained}");

    let affected = out(ok(&run(repo.path(), &["affected", "tb_contract_reminder"])));
    assert!(affected.contains("reminder.entity.ts") || affected.contains("ContractReminderEntity"), "the entity is affected by its table: {affected}");
    assert!(affected.contains("reminder.repository.ts") || affected.contains("ReminderRepository"), "and through it the repository: {affected}");

    let packages = out(ok(&run(repo.path(), &["affected", "ui"])));
    assert!(packages.contains("web"), "a workspace package is affected by the one it depends on: {packages}");
}

#[test]
fn a_dropped_table_leaves_the_graph_and_an_unchanged_sync_does_nothing() {
    let repo = repo();
    ok(&run(repo.path(), &["sync"]));
    let again = out(ok(&run(repo.path(), &["sync"])));
    assert!(again.contains("target_version=None"), "nothing changed, nothing staged: {again}");

    repo.write_file(
        "db/changeset/002-drop.xml",
        r#"<databaseChangeLog><changeSet id="drop" author="a"><sql>DROP TABLE "public"."tb_contract_reminder";</sql></changeSet></databaseChangeLog>"#,
    );
    repo.commit("drop the reminder table");
    ok(&run(repo.path(), &["sync"]));
    let found = out(ok(&run(repo.path(), &["search", "contract reminder"])));
    assert!(!found.contains("table tb_contract_reminder"), "the table is gone: {found}");
    assert!(out(ok(&run(repo.path(), &["search", "tb_contract"]))).contains("table tb_contract"), "the other table stays");
}

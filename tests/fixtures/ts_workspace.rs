//! The small TypeScript workspace used by the dependency-edge tests: path
//! aliases, a barrel with re-exports, a type-only import, an import cycle, an
//! external package, an unresolved specifier and a spec file.
#![allow(dead_code)]

use super::FixtureRepo;
use std::sync::Arc;

use nexspec::GitSource;
use nexspec::graph::csr::{Csr, CsrBase, CsrParticipant};
use nexspec::graph::edge::{Edge, EdgeType};
use nexspec::graph::node::{file_node_id, symbol_node_id};
use nexspec::sync::mutation::StableId;
use nexspec::sync::{Coordinator, RedbParticipant, VersionPointer};
use nexspec::sync_orchestrator::SyncOrchestrator;
use redb::Database;
use tempfile::NamedTempFile;

pub fn build() -> FixtureRepo {
    let repo = FixtureRepo::init();
    repo.write_file(
        "tsconfig.json",
        "{\n  // aliases\n  \"compilerOptions\": { \"baseUrl\": \".\", \"paths\": { \"@app/*\": [\"src/app/*\"] } },\n}\n",
    );
    repo.write_file("src/cache/cache.port.ts", "export interface CachePort {\n  get(key: string): string;\n}\n");
    repo.write_file(
        "src/cache/redis.adapter.ts",
        "import type { CachePort } from './cache.port';\nexport class RedisAdapter implements CachePort {\n  get(key: string) {\n    return key;\n  }\n}\n",
    );
    repo.write_file("src/cache/index.ts", "export * from './cache.port';\nexport { RedisAdapter } from './redis.adapter';\n");
    repo.write_file("src/app/util.ts", "export function helper() {}\n");
    repo.write_file(
        "src/app/service.ts",
        "import { CachePort, RedisAdapter } from '../cache';\nimport { helper } from '@app/util';\nimport { Missing } from 'some-external-pkg';\nimport { Nope } from './does-not-exist';\n\nexport class Service {\n  constructor(private cache: CachePort) {}\n  run() {\n    helper();\n    return new RedisAdapter();\n  }\n}\n",
    );
    repo.write_file("src/app/a.ts", "import { b } from './b';\nexport function a() {\n  return b();\n}\n");
    repo.write_file("src/app/b.ts", "import { a } from './a';\nexport function b() {\n  return a();\n}\n");
    repo.write_file(
        "src/app/service.spec.ts",
        "import { Service } from './service';\ndescribe('service', () => {\n  it('builds', () => {\n    new Service(null as any);\n  });\n});\n",
    );
    repo.commit("feat: workspace");
    repo
}

/// An orchestrator wired to a queryable CSR, for tests that sync repeatedly.
pub struct Workspace {
    pub repo: FixtureRepo,
    pub csr: Arc<Csr>,
    pub orchestrator: SyncOrchestrator<'static>,
    _db: Box<Database>,
    _files: Vec<NamedTempFile>,
}

impl Workspace {
    pub fn new(repo: FixtureRepo) -> Self {
        let db_file = NamedTempFile::new().unwrap();
        let db = Box::new(Database::create(db_file.path()).unwrap());
        // The orchestrator borrows the database; it lives exactly as long as
        // this struct, which owns the box.
        let db_ref: &'static Database = unsafe { &*(db.as_ref() as *const Database) };
        let csr_file = NamedTempFile::new().unwrap();
        CsrBase::build(&[], csr_file.path()).unwrap();
        let base = CsrBase::open(csr_file.path()).unwrap();
        let participant = CsrParticipant::new(Arc::new(Csr::new(base)), csr_file.path().to_path_buf());
        let csr = participant.csr_handle();
        let wal_file = NamedTempFile::new().unwrap();
        let wal = nexspec::sync::Wal::open(wal_file.path()).unwrap();
        let coordinator = Coordinator::new(
            wal,
            VersionPointer::new(db_ref),
            vec![Box::new(RedbParticipant::new(db_ref)), Box::new(participant)],
        );
        let git = GitSource::open(repo.path()).unwrap();
        let orchestrator = SyncOrchestrator::new(git, coordinator, VersionPointer::new(db_ref)).with_csr(Arc::clone(&csr));
        Self { repo, csr, orchestrator, _db: db, _files: vec![db_file, csr_file, wal_file] }
    }

    pub fn sync(&mut self) {
        self.orchestrator.run_once().expect("sync");
    }

    pub fn edges(&self, from: &StableId, edge_type: EdgeType) -> Vec<Edge> {
        self.csr.edges_from(from, edge_type)
    }

    pub fn has_edge(&self, from: &StableId, to: &StableId, edge_type: EdgeType) -> Option<Edge> {
        self.edges(from, edge_type).into_iter().find(|e| e.to == *to)
    }
}

pub fn file(path: &str) -> StableId {
    file_node_id(path)
}

pub fn symbol(path: &str, name: &str) -> StableId {
    symbol_node_id(path, name, 0)
}


//! The small TypeScript workspace used by the dependency-edge tests: path
//! aliases, a barrel with re-exports, a type-only import, an import cycle, an
//! external package, an unresolved specifier and a spec file.
#![allow(dead_code)]

use super::FixtureRepo;

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


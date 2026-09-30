//! File-level imports for Rust, Python and Go (T-709, REQ-705 in
//! `.specs/features/dependency-edges/spec.md`).

mod fixtures;

use fixtures::FixtureRepo;
use fixtures::ts_workspace::{Workspace, file};
use nexspec::graph::edge::EdgeType;

fn imports(ws: &Workspace, from: &str) -> Vec<String> {
    // Targets as paths, recovered by matching against the known candidates.
    let edges = ws.edges(&file(from), EdgeType::Imports);
    let mut out: Vec<String> = Vec::new();
    for candidate in [
        "src/app.rs", "src/util.rs", "src/lib.rs", "src/model/mod.rs", "src/model/user.rs", "app/main.py", "app/sibling.py",
        "app/pkg/__init__.py", "app/pkg/core.py", "util/util.go", "store/store.go", "store/cache.go", "main.go",
    ] {
        if edges.iter().any(|e| e.to == file(candidate)) {
            out.push(candidate.to_string());
        }
    }
    out.sort();
    out
}

#[test]
fn rust_use_and_mod_become_import_edges_and_externals_do_not() {
    let repo = FixtureRepo::init();
    repo.write_file("Cargo.toml", "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n");
    repo.write_file("src/lib.rs", "pub mod app;\npub mod util;\npub mod model;\n");
    repo.write_file("src/util.rs", "pub fn helper() {}\n");
    repo.write_file("src/model/mod.rs", "pub mod user;\n");
    repo.write_file("src/model/user.rs", "pub struct User;\n");
    repo.write_file(
        "src/app.rs",
        "use crate::util::helper;\nuse crate::model::user::User;\nuse std::collections::HashMap;\nuse serde::Serialize;\nuse super::util as u;\n",
    );
    repo.write_file("tests/it.rs", "use demo::app::run;\nuse demo::util::helper;\n");
    repo.commit("feat: crate");
    let mut ws = Workspace::new(repo);
    ws.sync();

    assert_eq!(imports(&ws, "src/app.rs"), vec!["src/model/user.rs", "src/util.rs"], "std and serde add nothing");
    assert_eq!(imports(&ws, "src/lib.rs"), vec!["src/app.rs", "src/model/mod.rs", "src/util.rs"], "`mod x;` links the file");
    assert_eq!(imports(&ws, "src/model/mod.rs"), vec!["src/model/user.rs"]);
    let from_tests = ws.edges(&file("tests/it.rs"), EdgeType::Imports);
    assert!(from_tests.iter().any(|e| e.to == file("src/app.rs")), "integration tests import by crate name");
    assert!(from_tests.iter().any(|e| e.to == file("src/util.rs")));
}

#[test]
fn python_relative_and_unique_absolute_imports_resolve() {
    let repo = FixtureRepo::init();
    repo.write_file("app/main.py", "import os\nimport requests\nfrom . import sibling\nfrom .pkg import core\nfrom pkg.core import thing\n");
    repo.write_file("app/sibling.py", "x = 1\n");
    repo.write_file("app/pkg/__init__.py", "");
    repo.write_file("app/pkg/core.py", "thing = 1\n");
    repo.commit("feat: python");
    let mut ws = Workspace::new(repo);
    ws.sync();
    assert_eq!(imports(&ws, "app/main.py"), vec!["app/pkg/__init__.py", "app/pkg/core.py", "app/sibling.py"]);
}

#[test]
fn go_imports_link_every_file_of_the_imported_package_through_go_mod() {
    let repo = FixtureRepo::init();
    repo.write_file("go.mod", "module example.com/app\n\ngo 1.22\n");
    repo.write_file("main.go", "package main\n\nimport (\n\t\"fmt\"\n\t\"example.com/app/store\"\n\t\"example.com/app/util\"\n)\n\nfunc main() { fmt.Println(store.X, util.Y) }\n");
    repo.write_file("store/store.go", "package store\n\nvar X = 1\n");
    repo.write_file("store/cache.go", "package store\n");
    repo.write_file("store/store_test.go", "package store\n");
    repo.write_file("util/util.go", "package util\n\nvar Y = 2\n");
    repo.commit("feat: go");
    let mut ws = Workspace::new(repo);
    ws.sync();
    assert_eq!(imports(&ws, "main.go"), vec!["store/cache.go", "store/store.go", "util/util.go"]);
}

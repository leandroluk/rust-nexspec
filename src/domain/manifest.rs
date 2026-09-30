//! Package manifests (REQ-1404 in `.specs/features/domain-extractors/spec.md`):
//! `package.json`, `Cargo.toml` and the `extends` chain of `tsconfig*.json`.
//!
//! JSON goes through the same tolerant parser the import resolver uses
//! (comments and trailing commas), so a manifest is read the same way whether
//! it feeds a `Package` node or a module resolution.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::code::resolve::parse_jsonc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
    /// Repository-relative directory of the manifest (`""` for the root).
    pub dir: String,
    /// Repository-relative path of the manifest file.
    pub manifest: String,
    /// Every dependency name (runtime, dev, peer, build), sorted.
    pub dependencies: BTreeSet<String>,
    /// `path = "../x"` dependencies of a Cargo manifest, as directories relative to the repository.
    pub path_dependencies: BTreeSet<String>,
}

fn dir_of(path: &str) -> String {
    path.rsplit_once('/').map(|(dir, _)| dir.to_string()).unwrap_or_default()
}

fn join_dir(dir: &str, relative: &str) -> Option<String> {
    let mut parts: Vec<&str> = if dir.is_empty() { Vec::new() } else { dir.split('/').collect() };
    for part in relative.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

/// Whether `path` is a manifest nexspec reads.
pub fn is_manifest(path: &str) -> bool {
    if path.split('/').any(|part| part == "node_modules" || part == "target") {
        return false;
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    name == "package.json" || name == "Cargo.toml" || (name.starts_with("tsconfig") && name.ends_with(".json"))
}

pub fn parse_package_json(path: &str, text: &str) -> Option<PackageInfo> {
    let value = parse_jsonc(text)?;
    let name = value.get("name")?.as_str()?.to_string();
    let mut dependencies = BTreeSet::new();
    for section in ["dependencies", "devDependencies", "peerDependencies", "optionalDependencies"] {
        if let Some(map) = value.get(section).and_then(Value::as_object) {
            dependencies.extend(map.keys().cloned());
        }
    }
    Some(PackageInfo {
        name,
        version: value.get("version").and_then(Value::as_str).unwrap_or_default().to_string(),
        dir: dir_of(path),
        manifest: path.to_string(),
        dependencies,
        path_dependencies: BTreeSet::new(),
    })
}

/// `[package]` of a `Cargo.toml` (a virtual workspace manifest has none and yields `None`).
pub fn parse_cargo_toml(path: &str, text: &str) -> Option<PackageInfo> {
    let table: toml::Table = text.parse().ok()?;
    let package = table.get("package")?.as_table()?;
    let name = package.get("name")?.as_str()?.to_string();
    let version = package.get("version").and_then(toml::Value::as_str).unwrap_or_default().to_string();
    let dir = dir_of(path);
    let mut dependencies = BTreeSet::new();
    let mut path_dependencies = BTreeSet::new();
    for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
        let Some(deps) = table.get(section).and_then(toml::Value::as_table) else { continue };
        for (dep, spec) in deps {
            let real_name = spec.get("package").and_then(toml::Value::as_str).unwrap_or(dep);
            dependencies.insert(real_name.to_string());
            if let Some(relative) = spec.get("path").and_then(toml::Value::as_str)
                && let Some(target) = join_dir(&dir, relative)
            {
                path_dependencies.insert(target);
            }
        }
    }
    Some(PackageInfo { name, version, dir, manifest: path.to_string(), dependencies, path_dependencies })
}

/// `(from package, to package)` for dependencies between packages of the repository, by
/// name or (Cargo) by directory. Sorted and de-duplicated.
pub fn workspace_dependencies(packages: &[PackageInfo]) -> Vec<(String, String)> {
    let mut edges = BTreeSet::new();
    for from in packages {
        for to in packages {
            if from.name == to.name {
                continue;
            }
            if from.dependencies.contains(&to.name) || from.path_dependencies.contains(&to.dir) {
                edges.insert((from.name.clone(), to.name.clone()));
            }
        }
    }
    edges.into_iter().collect()
}

/// The file a `tsconfig*.json` extends, as a repository-relative path, when it is a relative path
/// (a package name such as `@tsconfig/node20/tsconfig.json` is not in the repository).
pub fn tsconfig_extends(path: &str, text: &str) -> Vec<String> {
    let Some(value) = parse_jsonc(text) else { return Vec::new() };
    let targets: Vec<&str> = match value.get("extends") {
        Some(Value::String(one)) => vec![one.as_str()],
        Some(Value::Array(many)) => many.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    targets
        .into_iter()
        .filter(|t| t.starts_with('.'))
        .filter_map(|t| {
            let file = if t.ends_with(".json") { t.to_string() } else { format!("{t}.json") };
            join_dir(&dir_of(path), &file)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_json_gives_name_version_and_every_dependency_kind() {
        let info = parse_package_json(
            "apps/web/package.json",
            r#"{ "name": "@acme/web", "version": "1.2.3", // comment
                 "dependencies": { "react": "^18", "@acme/ui": "workspace:*" },
                 "devDependencies": { "vitest": "^1" }, "peerDependencies": { "typescript": "*" }, }"#,
        )
        .unwrap();
        assert_eq!((info.name.as_str(), info.version.as_str(), info.dir.as_str()), ("@acme/web", "1.2.3", "apps/web"));
        assert_eq!(info.dependencies.iter().map(String::as_str).collect::<Vec<_>>(), ["@acme/ui", "react", "typescript", "vitest"]);
        assert!(parse_package_json("package.json", r#"{ "private": true }"#).is_none(), "no name, no package");
    }

    #[test]
    fn cargo_toml_reads_package_dependencies_and_path_dependencies() {
        let info = parse_cargo_toml(
            "crates/app/Cargo.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\nserde = \"1\"\ncore-lib = { path = \"../core\" }\n[dev-dependencies]\ntempfile = \"3\"\n",
        )
        .unwrap();
        assert_eq!(info.name, "app");
        assert!(info.dependencies.contains("serde") && info.dependencies.contains("tempfile") && info.dependencies.contains("core-lib"));
        assert_eq!(info.path_dependencies.iter().map(String::as_str).collect::<Vec<_>>(), ["crates/core"]);
        assert!(parse_cargo_toml("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n").is_none(), "a virtual manifest has no package");
    }

    #[test]
    fn dependencies_between_packages_of_the_repository_are_edges_and_external_ones_are_not() {
        let web = parse_package_json("apps/web/package.json", r#"{ "name": "web", "dependencies": { "ui": "*", "react": "*" } }"#).unwrap();
        let ui = parse_package_json("pkgs/ui/package.json", r#"{ "name": "ui", "devDependencies": { "core": "*" } }"#).unwrap();
        let core = parse_package_json("pkgs/core/package.json", r#"{ "name": "core" }"#).unwrap();
        let edges = workspace_dependencies(&[web, ui, core]);
        assert_eq!(edges, [("ui".to_string(), "core".to_string()), ("web".to_string(), "ui".to_string())]);

        let app = parse_cargo_toml("crates/app/Cargo.toml", "[package]\nname=\"app\"\n[dependencies]\nlib = { path = \"../core\" }\n").unwrap();
        let core = parse_cargo_toml("crates/core/Cargo.toml", "[package]\nname=\"core\"\n").unwrap();
        assert_eq!(workspace_dependencies(&[app, core]), [("app".to_string(), "core".to_string())], "a path dependency counts even when renamed");
    }

    #[test]
    fn tsconfig_extends_follows_relative_paths_only() {
        assert_eq!(tsconfig_extends("apps/web/tsconfig.json", r#"{ "extends": "../../tsconfig.base" }"#), ["tsconfig.base.json"]);
        assert_eq!(tsconfig_extends("a/tsconfig.json", r#"{ "extends": ["./base.json", "@tsconfig/node20/tsconfig.json"] }"#), ["a/base.json"]);
        assert!(tsconfig_extends("tsconfig.json", r#"{ "compilerOptions": {} }"#).is_empty());
    }

    #[test]
    fn manifests_are_recognised_outside_dependency_folders_only() {
        assert!(is_manifest("package.json") && is_manifest("crates/a/Cargo.toml") && is_manifest("apps/w/tsconfig.app.json"));
        assert!(!is_manifest("node_modules/x/package.json") && !is_manifest("target/debug/Cargo.toml") && !is_manifest("src/a.json"));
    }
}

//! Import specifier resolution (REQ-702 in
//! `.specs/features/dependency-edges/spec.md`): turns the string in
//! `import ... from "x"` into the tracked file it names, or nothing.
//!
//! Order: relative paths, `package.json#imports` (`#/*`), `tsconfig` `paths`
//! and `baseUrl` (nearest `tsconfig.json`, following `extends`), workspace
//! packages (`@scope/pkg`, with `exports`/`source`/`main`), and finally
//! "external" -> `None`. Only files in the tracked set can be returned, so a
//! specifier that points outside the repository, or at `node_modules`,
//! never produces an edge.
//!
//! JSON is read directly (comments and trailing commas tolerated) with
//! `serde_json`, which the crate already depends on (Q1 in the spec).

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::Value;

const CODE_EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts"];

/// Resolves specifiers against a fixed set of tracked files.
pub struct SpecifierResolver {
    files: HashSet<String>,
    /// `tsconfig.json` directory -> its merged compiler options.
    tsconfigs: HashMap<String, TsConfig>,
    /// `package.json` directory -> what the package declares.
    packages: HashMap<String, Package>,
    /// Cargo crates and Go modules (Rust/Go imports).
    modules: crate::code::modules::ModuleContext,
    /// Package name -> directory (workspace packages only: those tracked here).
    by_name: HashMap<String, String>,
}

#[derive(Debug, Default, Clone)]
struct TsConfig {
    /// Directory (repo-relative) that `paths` targets are relative to.
    paths_base: String,
    /// `baseUrl` as a repo-relative directory, when set.
    base_url: Option<String>,
    paths: Vec<(String, Vec<String>)>,
}

#[derive(Debug, Default, Clone)]
struct Package {
    name: Option<String>,
    imports: BTreeMap<String, Vec<String>>,
    exports: BTreeMap<String, Vec<String>>,
    entries: Vec<String>,
}

impl SpecifierResolver {
    /// `tracked` are repo-relative paths; `read` returns a tracked file's text.
    pub fn new(tracked: impl IntoIterator<Item = String>, read: impl Fn(&str) -> Option<String>) -> Self {
        let files: HashSet<String> = tracked.into_iter().map(|p| p.replace('\\', "/")).collect();
        let mut tsconfigs = HashMap::new();
        let mut packages = HashMap::new();
        let mut by_name = HashMap::new();
        let mut modules = crate::code::modules::ModuleContext::default();
        let mut sorted: Vec<&String> = files.iter().collect();
        sorted.sort();
        for path in sorted {
            let (dir, name) = split_dir(path);
            if name == "tsconfig.json" {
                if let Some(config) = load_tsconfig(path, &read, 0) {
                    tsconfigs.insert(dir.to_string(), config);
                }
            } else if name == "Cargo.toml" {
                if let Some(text) = read(path) {
                    modules.add_cargo_toml(dir, &text);
                }
            } else if name == "go.mod" {
                if let Some(text) = read(path) {
                    modules.add_go_mod(dir, &text);
                }
            } else if name == "package.json"
                && let Some(text) = read(path)
                && let Some(value) = parse_jsonc(&text)
            {
                let package = parse_package(&value, dir);
                if let Some(pkg_name) = &package.name {
                    by_name.entry(pkg_name.clone()).or_insert_with(|| dir.to_string());
                }
                packages.insert(dir.to_string(), package);
            }
        }
        Self { files, tsconfigs, packages, modules, by_name }
    }

    /// Every tracked file `specifier` may refer to: at most one for TS/JS,
    /// Rust and Python; a Go import names a package, i.e. all its files.
    pub fn resolve_all(&self, from_file: &str, specifier: &str) -> Vec<String> {
        let from_file = from_file.replace('\\', "/");
        let ecmascript = from_file
            .rsplit_once('.')
            .is_some_and(|(_, ext)| matches!(ext, "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts"));
        if ecmascript {
            self.resolve(&from_file, specifier).into_iter().collect()
        } else {
            crate::code::modules::resolve(&self.modules, &self.files, &from_file, specifier)
        }
    }

    /// The tracked file `specifier` (written in `from_file`) refers to.
    pub fn resolve(&self, from_file: &str, specifier: &str) -> Option<String> {
        let from_file = from_file.replace('\\', "/");
        let from_dir = split_dir(&from_file).0;
        let specifier = specifier.trim();
        if specifier.is_empty() {
            return None;
        }

        if specifier == "." || specifier == ".." || specifier.starts_with("./") || specifier.starts_with("../") {
            return self.probe(&join(from_dir, specifier)?);
        }
        if specifier.starts_with('#') {
            return self.resolve_package_imports(from_dir, specifier);
        }
        if let Some(found) = self.resolve_tsconfig(from_dir, specifier) {
            return Some(found);
        }
        self.resolve_workspace_package(specifier)
    }

    fn resolve_tsconfig(&self, from_dir: &str, specifier: &str) -> Option<String> {
        let config = nearest(from_dir, &self.tsconfigs)?;
        for (pattern, targets) in &config.paths {
            let Some(captured) = match_pattern(pattern, specifier) else { continue };
            for target in targets {
                let substituted = target.replacen('*', captured, 1);
                if let Some(path) = join(&config.paths_base, &substituted)
                    && let Some(found) = self.probe(&path)
                {
                    return Some(found);
                }
            }
        }
        let base = config.base_url.as_deref()?;
        self.probe(&join(base, specifier)?)
    }

    fn resolve_package_imports(&self, from_dir: &str, specifier: &str) -> Option<String> {
        let (dir, package) = nearest_entry(from_dir, &self.packages, |p| !p.imports.is_empty())?;
        for (key, targets) in &package.imports {
            let Some(captured) = match_pattern(key, specifier) else { continue };
            for target in targets {
                let substituted = target.replacen('*', captured, 1);
                if let Some(path) = join(dir, &substituted)
                    && let Some(found) = self.probe(&path)
                {
                    return Some(found);
                }
            }
        }
        None
    }

    fn resolve_workspace_package(&self, specifier: &str) -> Option<String> {
        let (name, subpath) = split_package_specifier(specifier);
        let dir = self.by_name.get(&name)?;
        let package = self.packages.get(dir)?;
        let export_key = if subpath.is_empty() { ".".to_string() } else { format!("./{subpath}") };

        if let Some(targets) = package.exports.get(&export_key) {
            for target in targets {
                if let Some(found) = self.probe_package_target(dir, target) {
                    return Some(found);
                }
            }
        }
        if subpath.is_empty() {
            for entry in &package.entries {
                if let Some(found) = self.probe_package_target(dir, entry) {
                    return Some(found);
                }
            }
            for fallback in ["src/index", "index"] {
                if let Some(found) = self.probe(&join(dir, fallback)?) {
                    return Some(found);
                }
            }
            None
        } else {
            self.probe(&join(dir, &format!("src/{subpath}"))?).or_else(|| self.probe(&join(dir, &subpath)?))
        }
    }

    /// `target` comes from `exports`/`main`/`source`: usually built output
    /// (`dist/index.js`), whose source lives under `src/`.
    fn probe_package_target(&self, package_dir: &str, target: &str) -> Option<String> {
        let direct = join(package_dir, target)?;
        if let Some(found) = self.probe(&direct) {
            return Some(found);
        }
        for build_dir in ["dist/", "build/", "lib/", "out/"] {
            let relative = target.trim_start_matches("./");
            if let Some(rest) = relative.strip_prefix(build_dir) {
                let stem = strip_extension(rest);
                if let Some(found) = self.probe(&join(package_dir, &format!("src/{stem}"))?) {
                    return Some(found);
                }
            }
        }
        None
    }

    /// A tracked code file for `candidate`: itself, a TS sibling of a `.js`
    /// specifier, the path plus an extension, or a directory's `index`.
    fn probe(&self, candidate: &str) -> Option<String> {
        let has_code_extension = |p: &str| p.rsplit_once('.').is_some_and(|(_, ext)| CODE_EXTENSIONS.contains(&ext));
        if has_code_extension(candidate) && self.files.contains(candidate) {
            return Some(candidate.to_string());
        }
        // `./x.js` in TS sources means `./x.ts`.
        if let Some((stem, ext)) = candidate.rsplit_once('.') {
            let ts_twins: &[&str] = match ext {
                "js" => &["ts", "tsx"],
                "jsx" => &["tsx"],
                "mjs" => &["mts"],
                "cjs" => &["cts"],
                _ => &[],
            };
            for twin in ts_twins {
                let path = format!("{stem}.{twin}");
                if self.files.contains(&path) {
                    return Some(path);
                }
            }
        }
        for ext in CODE_EXTENSIONS {
            let path = format!("{candidate}.{ext}");
            if self.files.contains(&path) {
                return Some(path);
            }
        }
        for ext in CODE_EXTENSIONS {
            let path = format!("{candidate}/index.{ext}");
            if self.files.contains(&path) {
                return Some(path);
            }
        }
        None
    }
}

/// `(directory, file name)` of a `/`-separated path; the directory is `""` at the root.
fn split_dir(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

fn strip_extension(path: &str) -> &str {
    path.rsplit_once('.').map_or(path, |(stem, _)| stem)
}

/// `dir` joined with `relative`, with `.`/`..` resolved; `None` when it escapes the root.
fn join(dir: &str, relative: &str) -> Option<String> {
    let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    for segment in relative.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

/// The value of the deepest entry whose directory contains `dir`.
fn nearest<'a, V>(dir: &str, map: &'a HashMap<String, V>) -> Option<&'a V> {
    nearest_entry(dir, map, |_| true).map(|(_, v)| v)
}

fn nearest_entry<'a, V>(dir: &str, map: &'a HashMap<String, V>, accept: impl Fn(&V) -> bool) -> Option<(&'a str, &'a V)> {
    let mut current = dir;
    loop {
        if let Some((key, value)) = map.get_key_value(current)
            && accept(value)
        {
            return Some((key.as_str(), value));
        }
        if current.is_empty() {
            return None;
        }
        current = current.rsplit_once('/').map_or("", |(parent, _)| parent);
    }
}

/// Matches `specifier` against a pattern with at most one `*`; returns what `*` captured.
fn match_pattern<'s>(pattern: &str, specifier: &'s str) -> Option<&'s str> {
    match pattern.split_once('*') {
        None => (pattern == specifier).then_some(""),
        Some((prefix, suffix)) => {
            let rest = specifier.strip_prefix(prefix)?;
            rest.strip_suffix(suffix)
        }
    }
}

/// `@scope/name/sub/path` -> (`@scope/name`, `sub/path`).
fn split_package_specifier(specifier: &str) -> (String, String) {
    let mut parts = specifier.split('/');
    let first = parts.next().unwrap_or_default();
    let name = if first.starts_with('@') {
        match parts.next() {
            Some(second) => format!("{first}/{second}"),
            None => first.to_string(),
        }
    } else {
        first.to_string()
    };
    (name, parts.collect::<Vec<_>>().join("/"))
}

/// JSON with `//` and `/* */` comments and trailing commas (tsconfig style).
fn parse_jsonc(text: &str) -> Option<Value> {
    serde_json::from_str(&strip_jsonc(text)).ok()
}

fn strip_jsonc(text: &str) -> String {
    let chars: Vec<char> = text.trim_start_matches('\u{feff}').chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut in_string = false;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() {
                out.push(chars[i + 1]);
                i += 1;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
            out.push(c);
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        } else {
            out.push(c);
        }
        i += 1;
    }
    // Trailing commas before `}` or `]`.
    let mut cleaned = String::with_capacity(out.len());
    let bytes: Vec<char> = out.chars().collect();
    let mut in_str = false;
    for (idx, &c) in bytes.iter().enumerate() {
        if in_str {
            cleaned.push(c);
            if c == '\\' {
                continue;
            }
            if c == '"' && bytes.get(idx.wrapping_sub(1)) != Some(&'\\') {
                in_str = false;
            }
            continue;
        }
        if c == '"' {
            in_str = true;
        }
        if c == ',' {
            let next = bytes[idx + 1..].iter().find(|n| !n.is_whitespace());
            if matches!(next, Some('}') | Some(']')) {
                continue;
            }
        }
        cleaned.push(c);
    }
    cleaned
}

/// Loads `tsconfig.json` at `path`, merging the `extends` chain (relative
/// paths only, depth-limited). Options in the child override the parent's.
fn load_tsconfig(path: &str, read: &impl Fn(&str) -> Option<String>, depth: usize) -> Option<TsConfig> {
    if depth > 5 {
        return None;
    }
    let value = parse_jsonc(&read(path)?)?;
    let dir = split_dir(path).0;
    let mut config = TsConfig { paths_base: dir.to_string(), ..TsConfig::default() };

    if let Some(extends) = value.get("extends").and_then(Value::as_str)
        && (extends.starts_with("./") || extends.starts_with("../"))
    {
        let mut parent_path = join(dir, extends)?;
        if !parent_path.ends_with(".json") {
            parent_path.push_str(".json");
        }
        if let Some(parent) = load_tsconfig(&parent_path, read, depth + 1) {
            config = parent;
        }
    }

    if let Some(options) = value.get("compilerOptions") {
        if let Some(base_url) = options.get("baseUrl").and_then(Value::as_str) {
            let resolved = join(dir, base_url)?;
            config.base_url = Some(resolved.clone());
            // With a `baseUrl`, `paths` targets are relative to it.
            config.paths_base = resolved;
        }
        if let Some(paths) = options.get("paths").and_then(Value::as_object) {
            if options.get("baseUrl").is_none() {
                config.paths_base = dir.to_string();
            }
            config.paths = paths
                .iter()
                .map(|(pattern, targets)| {
                    let list = targets
                        .as_array()
                        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                        .unwrap_or_default();
                    (pattern.clone(), list)
                })
                .collect();
        }
    }
    Some(config)
}

fn parse_package(value: &Value, dir: &str) -> Package {
    let mut package = Package { name: value.get("name").and_then(Value::as_str).map(str::to_string), ..Package::default() };
    if let Some(imports) = value.get("imports").and_then(Value::as_object) {
        for (key, target) in imports {
            package.imports.insert(key.clone(), condition_targets(target));
        }
    }
    match value.get("exports") {
        Some(Value::Object(map)) if map.keys().any(|k| k.starts_with('.')) => {
            for (key, target) in map {
                package.exports.insert(key.clone(), condition_targets(target));
            }
        }
        Some(other) => {
            package.exports.insert(".".to_string(), condition_targets(other));
        }
        None => {}
    }
    for field in ["source", "module", "main", "types"] {
        if let Some(entry) = value.get(field).and_then(Value::as_str) {
            package.entries.push(entry.to_string());
        }
    }
    let _ = dir;
    package
}

/// Flattens an `exports`/`imports` target (string, array or condition object)
/// into candidate paths, preferring source-ish conditions.
fn condition_targets(target: &Value) -> Vec<String> {
    match target {
        Value::String(s) => vec![s.clone()],
        Value::Array(items) => items.iter().flat_map(condition_targets).collect(),
        Value::Object(map) => {
            let mut ordered: Vec<&str> = ["source", "import", "module", "default", "require", "types"]
                .into_iter()
                .filter(|k| map.contains_key(*k))
                .collect();
            for key in map.keys() {
                if !ordered.contains(&key.as_str()) {
                    ordered.push(key);
                }
            }
            ordered.into_iter().flat_map(|k| condition_targets(&map[k])).collect()
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver(files: &[(&str, &str)]) -> SpecifierResolver {
        let map: HashMap<String, String> = files.iter().map(|(p, c)| (p.to_string(), c.to_string())).collect();
        SpecifierResolver::new(map.keys().cloned().collect::<Vec<_>>(), move |p| map.get(p).cloned())
    }

    #[test]
    fn relative_specifiers_try_extensions_index_and_js_to_ts() {
        let r = resolver(&[
            ("src/a.ts", ""),
            ("src/util/index.ts", ""),
            ("src/lib/x.ts", ""),
            ("src/ui/button.tsx", ""),
            ("src/old.js", ""),
        ]);
        assert_eq!(r.resolve("src/a.ts", "./util").as_deref(), Some("src/util/index.ts"));
        assert_eq!(r.resolve("src/a.ts", "./lib/x").as_deref(), Some("src/lib/x.ts"));
        assert_eq!(r.resolve("src/a.ts", "./lib/x.js").as_deref(), Some("src/lib/x.ts"), ".js means the .ts twin");
        assert_eq!(r.resolve("src/a.ts", "./ui/button").as_deref(), Some("src/ui/button.tsx"));
        assert_eq!(r.resolve("src/a.ts", "./old.js").as_deref(), Some("src/old.js"));
        assert_eq!(r.resolve("src/lib/x.ts", "../a").as_deref(), Some("src/a.ts"));
        assert_eq!(r.resolve("src/lib/x.ts", "../../../escape"), None, "cannot leave the repository");
        assert_eq!(r.resolve("src/a.ts", "./missing"), None);
    }

    #[test]
    fn external_packages_resolve_to_nothing() {
        let r = resolver(&[("src/a.ts", "")]);
        assert_eq!(r.resolve("src/a.ts", "react"), None);
        assert_eq!(r.resolve("src/a.ts", "@nestjs/common"), None);
        assert_eq!(r.resolve("src/a.ts", "node:fs"), None);
        assert_eq!(r.resolve("src/a.ts", ""), None);
    }

    #[test]
    fn tsconfig_paths_and_base_url_with_comments_and_trailing_commas() {
        let r = resolver(&[
            (
                "tsconfig.json",
                "{\n  // aliases\n  \"compilerOptions\": {\n    \"baseUrl\": \".\",\n    /* block */\n    \"paths\": { \"@app/*\": [\"src/app/*\"], \"@core\": [\"src/core/index.ts\"], },\n  },\n}\n",
            ),
            ("src/app/feature/thing.ts", ""),
            ("src/core/index.ts", ""),
            ("src/main.ts", ""),
            ("lib/helper.ts", ""),
        ]);
        assert_eq!(r.resolve("src/main.ts", "@app/feature/thing").as_deref(), Some("src/app/feature/thing.ts"));
        assert_eq!(r.resolve("src/main.ts", "@core").as_deref(), Some("src/core/index.ts"));
        assert_eq!(r.resolve("src/main.ts", "lib/helper").as_deref(), Some("lib/helper.ts"), "bare path through baseUrl");
        assert_eq!(r.resolve("src/main.ts", "@app/nope"), None);
    }

    #[test]
    fn nearest_tsconfig_wins_and_extends_is_followed() {
        let r = resolver(&[
            ("tsconfig.base.json", "{\"compilerOptions\":{\"paths\":{\"@shared/*\":[\"packages/shared/src/*\"]}}}"),
            ("apps/web/tsconfig.json", "{\"extends\":\"../../tsconfig.base.json\",\"compilerOptions\":{\"baseUrl\":\"src\"}}"),
            ("apps/mobile/tsconfig.json", "{\"extends\":\"../../tsconfig.base.json\"}"),
            ("apps/web/src/page.ts", ""),
            ("apps/web/src/components/card.ts", ""),
            ("apps/mobile/src/screen.ts", ""),
            ("packages/shared/src/money.ts", ""),
        ]);
        // The app's own baseUrl applies to files under it ...
        assert_eq!(r.resolve("apps/web/src/page.ts", "components/card").as_deref(), Some("apps/web/src/components/card.ts"));
        assert_eq!(r.resolve("apps/mobile/src/screen.ts", "components/card"), None, "another app has no such baseUrl");
        // ... and `paths` inherited from the base stay relative to the base's directory.
        assert_eq!(r.resolve("apps/mobile/src/screen.ts", "@shared/money").as_deref(), Some("packages/shared/src/money.ts"));
    }

    #[test]
    fn package_json_imports_hash_aliases() {
        let r = resolver(&[
            ("apps/api/package.json", "{\"name\":\"api\",\"imports\":{\"#/*\":\"./src/*\"}}"),
            ("apps/api/src/module/user.ts", ""),
            ("apps/api/src/main.ts", ""),
        ]);
        assert_eq!(r.resolve("apps/api/src/main.ts", "#/module/user").as_deref(), Some("apps/api/src/module/user.ts"));
        assert_eq!(r.resolve("apps/api/src/main.ts", "#/nothing"), None);
    }

    #[test]
    fn workspace_packages_resolve_through_source_exports_main_and_dist() {
        let r = resolver(&[
            ("pkgs/nest-cache/package.json", "{\"name\":\"@acme/nest-cache\",\"main\":\"./dist/index.js\"}"),
            ("pkgs/nest-cache/src/index.ts", ""),
            ("pkgs/nest-cache/src/cache.port.ts", ""),
            ("pkgs/shared/package.json", "{\"name\":\"@acme/shared\",\"exports\":{\".\":{\"source\":\"./src/main.ts\",\"default\":\"./dist/main.js\"},\"./money\":\"./dist/money.js\"}}"),
            ("pkgs/shared/src/main.ts", ""),
            ("pkgs/shared/src/money.ts", ""),
            ("apps/api/src/x.ts", ""),
        ]);
        assert_eq!(r.resolve("apps/api/src/x.ts", "@acme/nest-cache").as_deref(), Some("pkgs/nest-cache/src/index.ts"), "dist main maps back to src");
        assert_eq!(r.resolve("apps/api/src/x.ts", "@acme/nest-cache/cache.port").as_deref(), Some("pkgs/nest-cache/src/cache.port.ts"));
        assert_eq!(r.resolve("apps/api/src/x.ts", "@acme/shared").as_deref(), Some("pkgs/shared/src/main.ts"), "the `source` condition wins");
        assert_eq!(r.resolve("apps/api/src/x.ts", "@acme/shared/money").as_deref(), Some("pkgs/shared/src/money.ts"));
        assert_eq!(r.resolve("apps/api/src/x.ts", "@acme/unknown"), None);
    }

    #[test]
    fn jsonc_stripping_keeps_strings_intact() {
        let v = parse_jsonc("{\"url\": \"http://x//y\", // c\n \"a\": [1, 2,],}").unwrap();
        assert_eq!(v["url"], "http://x//y");
        assert_eq!(v["a"], serde_json::json!([1, 2]));
    }

    #[test]
    fn join_and_pattern_helpers() {
        assert_eq!(join("a/b", "../c").as_deref(), Some("a/c"));
        assert_eq!(join("", "./x").as_deref(), Some("x"));
        assert_eq!(join("a", "../../x"), None);
        assert_eq!(match_pattern("@app/*", "@app/x/y"), Some("x/y"));
        assert_eq!(match_pattern("@core", "@core"), Some(""));
        assert_eq!(match_pattern("@app/*", "other"), None);
        assert_eq!(split_package_specifier("@s/n/a/b"), ("@s/n".to_string(), "a/b".to_string()));
        assert_eq!(split_package_specifier("pkg/sub"), ("pkg".to_string(), "sub".to_string()));
    }
}

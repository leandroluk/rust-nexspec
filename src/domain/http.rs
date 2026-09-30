//! HTTP endpoints (REQ-1304 in `.specs/features/multi-repo-graph/spec.md`, decision D5): the ones an
//! OpenAPI file declares, and the ones client code calls with a literal path. Nothing is executed and no
//! types are inferred: a call whose URL is not spelled out in the source is simply not seen.

use serde_json::Value;

const METHODS: [&str; 7] = ["get", "post", "put", "patch", "delete", "head", "options"];

/// First segments too generic to say anything about which service is on the other end.
const GENERIC_FIRST_SEGMENTS: [&str; 10] = ["health", "healthz", "status", "ping", "metrics", "ready", "live", "version", "login", "logout"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointDef {
    pub method: String,
    pub path: String,
    pub operation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientCall {
    pub method: String,
    pub path: String,
}

pub fn is_openapi_file(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    let ext_ok = name.ends_with(".json") || name.ends_with(".yaml") || name.ends_with(".yml");
    ext_ok && (name.starts_with("openapi") || name.starts_with("swagger") || name.contains(".openapi.") || name.contains(".swagger."))
}

/// Cheap byte check before scanning a code file for client calls.
pub fn has_call_marker(text: &str) -> bool {
    text.contains("fetch(") || text.contains("axios") || text.contains("http") || text.contains("requests.") || text.contains("httpx") || text.contains("client.")
}

/// `{id}`, `:id`, `<id>`, `${expr}` -> `{}`; no scheme, host, query, fragment or trailing slash.
pub fn normalize_path(raw: &str) -> String {
    let mut text = raw.trim();
    if let Some(after) = text.split_once("://").map(|(_, rest)| rest) {
        text = after.find('/').map_or("", |i| &after[i..]);
    }
    // A template literal that starts with a base URL variable: `${BASE}/users` -> `/users`.
    let mut owned = text.to_string();
    if owned.starts_with("${")
        && let Some(end) = owned.find('}')
        && owned[end + 1..].starts_with('/')
    {
        owned = owned[end + 1..].to_string();
    }
    let owned = owned.split(['?', '#']).next().unwrap_or("").to_string();
    let mut out = Vec::new();
    for segment in owned.split('/') {
        if segment.is_empty() {
            continue;
        }
        let is_param = segment.starts_with('{') || segment.starts_with(':') || (segment.starts_with('<') && segment.ends_with('>')) || segment.contains("${");
        out.push(if is_param { "{}".to_string() } else { segment.to_string() });
    }
    format!("/{}", out.join("/"))
}

/// Whether a call to `client_path` reaches an endpoint served at `defined_path`: equal, or differing by a
/// leading prefix made only of `api` and version segments (`/api/v1/contracts/{}` vs `/contracts/{}`).
pub fn paths_match(client_path: &str, defined_path: &str) -> bool {
    if client_path == defined_path {
        return true;
    }
    let Some(prefix) = client_path.strip_suffix(defined_path) else { return false };
    !prefix.is_empty()
        && prefix.split('/').filter(|s| !s.is_empty()).all(|s| s == "api" || (s.len() > 1 && s.starts_with('v') && s[1..].chars().all(|c| c.is_ascii_digit())))
}

/// An endpoint worth linking across repositories: more than one segment, or one that is not generic.
pub fn is_specific(path: &str) -> bool {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty() && *s != "{}").collect();
    match segments.as_slice() {
        [] => false,
        [one] => !GENERIC_FIRST_SEGMENTS.contains(one),
        _ => true,
    }
}

// ---------------------------------------------------------------------------
// OpenAPI
// ---------------------------------------------------------------------------

pub fn parse_openapi(path: &str, text: &str) -> Vec<EndpointDef> {
    let lower = path.to_ascii_lowercase();
    let mut defs = if lower.ends_with(".json") { openapi_json(text) } else { openapi_yaml(text) };
    defs.sort_by(|a, b| (&a.path, &a.method).cmp(&(&b.path, &b.method)));
    defs.dedup();
    defs
}

fn openapi_json(text: &str) -> Vec<EndpointDef> {
    let Ok(value) = serde_json::from_str::<Value>(text) else { return Vec::new() };
    let Some(paths) = value.get("paths").and_then(Value::as_object) else { return Vec::new() };
    let mut out = Vec::new();
    for (route, item) in paths {
        let Some(item) = item.as_object() else { continue };
        for method in METHODS {
            if let Some(operation) = item.get(method) {
                out.push(EndpointDef {
                    method: method.to_ascii_uppercase(),
                    path: normalize_path(route),
                    operation_id: operation.get("operationId").and_then(Value::as_str).unwrap_or_default().to_string(),
                });
            }
        }
    }
    out
}

/// The `paths:` block of a YAML OpenAPI file: a route at one indent, its methods one level deeper.
fn openapi_yaml(text: &str) -> Vec<EndpointDef> {
    let mut out = Vec::new();
    let mut in_paths = false;
    let mut route: Option<(String, usize)> = None;
    let mut current: Option<usize> = None;
    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() || trimmed.trim_start().starts_with('#') {
            continue;
        }
        let indent = trimmed.len() - trimmed.trim_start().len();
        let body = trimmed.trim_start();
        if indent == 0 {
            in_paths = body.starts_with("paths:");
            route = None;
            current = None;
            continue;
        }
        if !in_paths {
            continue;
        }
        if body.starts_with('/') && body.ends_with(':') {
            route = Some((body.trim_end_matches(':').trim_matches(['"', '\'']).to_string(), indent));
            current = None;
            continue;
        }
        let Some((route_name, route_indent)) = &route else { continue };
        if indent > *route_indent
            && let Some(method) = body.strip_suffix(':').map(str::to_ascii_lowercase)
            && METHODS.contains(&method.as_str())
            && current.is_none_or(|c| indent <= c || indent == route_indent + (indent - route_indent))
        {
            out.push(EndpointDef { method: method.to_ascii_uppercase(), path: normalize_path(route_name), operation_id: String::new() });
            current = Some(indent);
        } else if let Some(id) = body.strip_prefix("operationId:")
            && let Some(last) = out.last_mut()
            && last.operation_id.is_empty()
        {
            last.operation_id = id.trim().trim_matches(['"', '\'']).to_string();
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Client calls
// ---------------------------------------------------------------------------

/// Calls like `fetch('/x')`, `axios.post(\`${base}/y\`)`, `this.http.get('/z')`, `requests.get("…")`
/// whose first argument is a string literal (or template literal) that looks like a path or URL.
pub fn scan_client_calls(text: &str) -> Vec<ClientCall> {
    let mut calls = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // A call is `name(`; find the next opening parenthesis after an identifier.
        let Some(open) = text[i..].find('(').map(|p| i + p) else { break };
        i = open + 1;
        let before = text[..open].trim_end();
        let (receiver, name) = match before.rsplit_once('.') {
            Some((receiver, name)) => (receiver, name),
            None => ("", before.rsplit(|c: char| !(c.is_alphanumeric() || c == '_')).next().unwrap_or("")),
        };
        let name_lower = name.to_ascii_lowercase();
        let method = if receiver.is_empty() && name == "fetch" {
            None // method from the options object, default GET
        } else if METHODS.contains(&name_lower.as_str()) && is_client_receiver(receiver) {
            Some(name_lower.to_ascii_uppercase())
        } else {
            continue;
        };
        let rest = text[open + 1..].trim_start();
        let Some(quote) = rest.chars().next().filter(|c| matches!(c, '\'' | '"' | '`')) else { continue };
        let Some(end) = rest[1..].find(quote) else { continue };
        let literal = &rest[1..1 + end];
        if !looks_like_route(literal) {
            continue;
        }
        let method = method.unwrap_or_else(|| fetch_method(&rest[1 + end..]));
        calls.push(ClientCall { method, path: normalize_path(literal) });
    }
    calls.sort_by(|a, b| (&a.path, &a.method).cmp(&(&b.path, &b.method)));
    calls.dedup();
    calls
}

fn is_client_receiver(receiver: &str) -> bool {
    let last = receiver.rsplit('.').next().unwrap_or(receiver).to_ascii_lowercase();
    ["axios", "http", "httpclient", "client", "api", "request", "requests", "got", "ky", "httpx", "session", "fetcher"].iter().any(|r| last == *r || last.ends_with(r))
}

fn looks_like_route(literal: &str) -> bool {
    let starts_ok = literal.starts_with('/') || literal.starts_with("http://") || literal.starts_with("https://") || literal.starts_with("${");
    starts_ok && literal.chars().any(|c| c.is_ascii_alphabetic()) && !literal.contains(' ') && literal.len() <= 200
}

/// `fetch(url, { method: 'POST' })` -> `POST`; anything else is a `GET`. Only the call's own arguments are read.
fn fetch_method(after_url: &str) -> String {
    let mut depth = 1;
    let mut quote: Option<char> = None;
    let mut end = after_url.len();
    // `after_url` starts at the closing quote of the URL literal; skip it, then stop at the call's `)`.
    for (i, c) in after_url.char_indices().skip(1) {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '\'' | '"' | '`' => quote = Some(c),
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = i;
                        break;
                    }
                }
                _ => {}
            },
        }
    }
    let args = &after_url[..end];
    if let Some(at) = args.find("method") {
        let rest = args[at + 6..].trim_start().trim_start_matches(':').trim_start();
        if let Some(q) = rest.chars().next().filter(|c| matches!(c, '\'' | '"')) {
            let value: String = rest[1..].chars().take_while(|c| *c != q).collect();
            if METHODS.contains(&value.to_ascii_lowercase().as_str()) {
                return value.to_ascii_uppercase();
            }
        }
    }
    "GET".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_normalised_to_one_spelling() {
        for (raw, want) in [
            ("/contracts/{id}/items/", "/contracts/{}/items"),
            ("/contracts/:id", "/contracts/{}"),
            ("/contracts/<int:id>", "/contracts/{}"),
            ("https://svc-b.internal:8080/api/v1/contracts?page=2", "/api/v1/contracts"),
            ("${this.baseUrl}/contracts/${id}", "/contracts/{}"),
            ("/", "/"),
        ] {
            assert_eq!(normalize_path(raw), want, "{raw}");
        }
    }

    #[test]
    fn a_version_or_api_prefix_is_tolerated_and_nothing_else() {
        assert!(paths_match("/contracts/{}", "/contracts/{}"));
        assert!(paths_match("/api/v1/contracts/{}", "/contracts/{}"));
        assert!(paths_match("/v2/contracts", "/contracts"));
        assert!(!paths_match("/billing/contracts", "/contracts"), "another service's prefix is not a version");
        assert!(!paths_match("/contracts", "/api/contracts"));
    }

    #[test]
    fn generic_endpoints_are_not_worth_linking() {
        assert!(!is_specific("/health") && !is_specific("/") && !is_specific("/{}"));
        assert!(is_specific("/contracts") && is_specific("/health/deep/check"));
    }

    #[test]
    fn openapi_json_gives_one_endpoint_per_operation() {
        let json = r#"{ "openapi": "3.0.0", "paths": {
            "/contracts/{id}": { "get": { "operationId": "getContract" }, "delete": {}, "parameters": [] },
            "/contracts": { "post": { "operationId": "createContract" } } } }"#;
        let defs = parse_openapi("api/openapi.json", json);
        let got: Vec<_> = defs.iter().map(|d| (d.method.as_str(), d.path.as_str(), d.operation_id.as_str())).collect();
        assert_eq!(got, [("POST", "/contracts", "createContract"), ("DELETE", "/contracts/{}", ""), ("GET", "/contracts/{}", "getContract")]);
    }

    #[test]
    fn openapi_yaml_paths_are_read_without_a_yaml_parser() {
        let yaml = "openapi: 3.0.0\ninfo:\n  title: x\npaths:\n  /invoices/{id}:\n    get:\n      operationId: getInvoice\n      responses: {}\n    put:\n      operationId: putInvoice\n  /invoices:\n    post:\n      summary: create\ncomponents:\n  schemas:\n    get:\n      type: object\n";
        let defs = parse_openapi("openapi.yaml", yaml);
        let got: Vec<_> = defs.iter().map(|d| (d.method.as_str(), d.path.as_str(), d.operation_id.as_str())).collect();
        assert_eq!(got, [("POST", "/invoices", ""), ("GET", "/invoices/{}", "getInvoice"), ("PUT", "/invoices/{}", "putInvoice")]);
    }

    #[test]
    fn client_calls_with_a_literal_path_are_found() {
        let code = r#"
            const a = await fetch('/api/v1/invoices');
            await fetch(`${BASE}/invoices/${id}`, { method: 'POST', body });
            axios.get('https://billing.internal/contracts/' + id);
            this.http.delete(`/contracts/${id}`);
            requests.put("/items/42")
            map.get('/not-a-client');
            cache.get("/x");
            client.post('not a path');
            fetch(dynamicUrl);
        "#;
        let calls: Vec<_> = scan_client_calls(code).into_iter().map(|c| format!("{} {}", c.method, c.path)).collect();
        // Sorted by path, then method.
        assert_eq!(calls, ["GET /api/v1/invoices", "GET /contracts", "DELETE /contracts/{}", "POST /invoices/{}", "PUT /items/42"]);
    }
}

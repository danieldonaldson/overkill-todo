//! Workspace dependency & target gate (OT-012 design D6).
//!
//! Reads `cargo metadata --no-deps --offline --locked` and enforces:
//! 1. Every workspace crate is listed in `RANKS` and `ALLOWED_EDGES`.
//! 2. Every workspace->workspace dependency (normal, dev, build) is in
//!    `ALLOWED_EDGES` and points strictly inward (`rank(to) < rank(from)`).
//!    (`testkit`, if introduced later, is allowed only as a dev-dependency.)
//! 3. `domain` and `application` third-party dependencies (split by normal,
//!    optional-normal, dev, and build) stay within their allowlists.
//! 4. `sqlx` is confined to `persistence-postgres` in normal dependencies and
//!    never appears outside `{sqlx::test, sqlx::PgPool, sqlx::postgres::PgPoolOptions}`
//!    in other workspace crates' source, tests, benches, or examples.
//! 5. Every `pub fn` / `pub async fn` in `persistence-postgres/src/testing.rs`
//!    is referenced by another crate's tests or benches (zero dead fixtures).
//! 6. Every workspace lib and bin target name (with `-` -> `_`) is in
//!    `server::APP_TARGETS`, so no crate's logs or spans are silently dropped.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const RANKS: &[(&str, u8)] = &[
    ("shared", 0),
    ("domain", 0),
    ("application", 1),
    ("platform", 1),
    ("persistence-postgres", 3),
    ("services", 3),
    ("api", 3),
    ("server", 4),
];

const ALLOWED_EDGES: &[(&str, &[&str])] = &[
    ("shared", &[]),
    ("domain", &[]),
    ("application", &["domain", "shared"]),
    ("platform", &["shared"]),
    (
        "persistence-postgres",
        &["domain", "application", "platform"],
    ),
    ("services", &["domain", "application", "platform"]),
    ("api", &["shared", "domain", "application", "platform"]),
    (
        "server",
        &[
            "shared",
            "domain",
            "application",
            "platform",
            "persistence-postgres",
            "services",
            "api",
        ],
    ),
];

const ALLOWED_SQLX_PATHS: &[&str] = &[
    "sqlx::test",
    "sqlx::PgPool",
    "sqlx::postgres::PgPoolOptions",
];

const DOMAIN_NORMAL_DEPS: &[&str] = &["jiff", "thiserror", "uuid"];
const DOMAIN_DEV_DEPS: &[&str] = &["proptest"];

const APPLICATION_NORMAL_DEPS: &[&str] = &[
    "async-trait",
    "hex",
    "jiff",
    "serde",
    "sha2",
    "tokio",
    "tracing",
    "utoipa",
    "uuid",
    "validator",
];
const APPLICATION_OPTIONAL_NORMAL_DEPS: &[&str] = &["serde_json", "tracing-subscriber"];
const APPLICATION_DEV_DEPS: &[&str] = &[
    "mockall",
    "proptest",
    "serde_json",
    "tokio",
    "tracing-subscriber",
];

fn cargo_metadata() -> Value {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("server manifest sits at backend/crates/server");

    let output = Command::new(env!("CARGO"))
        .current_dir(workspace_root)
        .args([
            "metadata",
            "--no-deps",
            "--offline",
            "--locked",
            "--format-version",
            "1",
        ])
        .output()
        .expect("run cargo metadata");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("parse cargo metadata JSON")
}

fn check_edge_table(
    workspace_names: &BTreeSet<&str>,
    ranks: &BTreeMap<&str, u8>,
    allowed: &BTreeMap<&str, &[&str]>,
) {
    for name in workspace_names {
        assert!(
            ranks.contains_key(name),
            "workspace crate `{name}` has no entry in RANKS in tests/crate_graph.rs"
        );
        assert!(
            allowed.contains_key(name),
            "workspace crate `{name}` has no entry in ALLOWED_EDGES in tests/crate_graph.rs"
        );
    }
    for name in ranks.keys() {
        assert!(
            workspace_names.contains(name),
            "RANKS entry `{name}` is not a workspace member"
        );
    }
    for name in allowed.keys() {
        assert!(
            workspace_names.contains(name),
            "ALLOWED_EDGES entry `{name}` is not a workspace member"
        );
    }

    for (&from, &targets) in allowed {
        let from_rank = ranks[from];
        for &to in targets {
            let to_rank = *ranks
                .get(to)
                .unwrap_or_else(|| panic!("ALLOWED_EDGES[{from}] names unknown crate `{to}`"));
            assert!(
                to_rank < from_rank,
                "ALLOWED_EDGES cannot allow `{from}` (rank {from_rank}) -> `{to}` (rank {to_rank}): edges must point strictly inward"
            );
        }
    }
}

fn check_inner_third_party_dep(
    layer: &str,
    dep_name: &str,
    kind: Option<&str>,
    optional: bool,
    normal_allow: &[&str],
    optional_allow: &[&str],
    dev_allow: &[&str],
) {
    match (kind, optional) {
        (None, false) => assert!(
            normal_allow.contains(&dep_name),
            "layer `{layer}` cannot add third-party normal dependency `{dep_name}` (allowed: {normal_allow:?})"
        ),
        (None, true) => assert!(
            optional_allow.contains(&dep_name),
            "layer `{layer}` cannot add optional third-party dependency `{dep_name}` (allowed: {optional_allow:?})"
        ),
        (Some("dev"), _) => assert!(
            dev_allow.contains(&dep_name),
            "layer `{layer}` cannot add third-party dev-dependency `{dep_name}` (allowed: {dev_allow:?})"
        ),
        (Some(other), _) => {
            panic!("layer `{layer}` cannot add third-party {other}-dependency `{dep_name}`")
        }
    }
}

fn check_declared_dependencies(
    packages: &[&Value],
    workspace_names: &BTreeSet<&str>,
    ranks: &BTreeMap<&str, u8>,
    allowed: &BTreeMap<&str, &[&str]>,
) {
    for pkg in packages {
        let from = pkg["name"].as_str().unwrap();
        let from_rank = ranks[from];
        let deps = pkg["dependencies"].as_array().expect("dependencies array");

        for dep in deps {
            let dep_name = dep["name"].as_str().expect("dep name");
            let kind = dep["kind"].as_str();
            let optional = dep["optional"].as_bool().unwrap_or(false);

            if workspace_names.contains(dep_name) {
                if dep_name == "testkit" {
                    assert_eq!(
                        kind,
                        Some("dev"),
                        "workspace crate `{from}` may depend on `testkit` only in [dev-dependencies]"
                    );
                    continue;
                }
                assert!(
                    allowed[from].contains(&dep_name),
                    "workspace crate `{from}` has unlisted dependency on `{dep_name}` (kind={kind:?}); allowed: {:?}",
                    allowed[from]
                );
                let to_rank = ranks[dep_name];
                assert!(
                    to_rank < from_rank,
                    "forbidden outward/same-rank edge: `{from}` (rank {from_rank}) -> `{dep_name}` (rank {to_rank})"
                );
                if from == "server" && dep_name == "api" {
                    assert!(
                        !dep["uses_default_features"].as_bool().unwrap_or(true),
                        "`server` must depend on `api` with `default-features = false`"
                    );
                }
                continue;
            }

            if dep_name == "sqlx" {
                match kind {
                    None => assert_eq!(
                        from, "persistence-postgres",
                        "crate `{from}` cannot depend on `sqlx` in [dependencies] (only `persistence-postgres` may)"
                    ),
                    Some("dev") => assert!(
                        matches!(from, "persistence-postgres" | "server"),
                        "crate `{from}` cannot depend on `sqlx` in [dev-dependencies] (only `persistence-postgres` and `server` may)"
                    ),
                    Some(other) => {
                        panic!("crate `{from}` cannot add `sqlx` as a {other}-dependency")
                    }
                }
            }

            match from {
                "domain" => check_inner_third_party_dep(
                    from,
                    dep_name,
                    kind,
                    optional,
                    DOMAIN_NORMAL_DEPS,
                    &[],
                    DOMAIN_DEV_DEPS,
                ),
                "application" => check_inner_third_party_dep(
                    from,
                    dep_name,
                    kind,
                    optional,
                    APPLICATION_NORMAL_DEPS,
                    APPLICATION_OPTIONAL_NORMAL_DEPS,
                    APPLICATION_DEV_DEPS,
                ),
                _ => {}
            }
        }

        if matches!(
            from,
            "application" | "persistence-postgres" | "services" | "api"
        ) && let Some(defaults) = pkg["features"].get("default").and_then(Value::as_array)
        {
            assert!(
                defaults.is_empty(),
                "`{from}` default features must be empty, found {defaults:?}"
            );
        }
    }
}

fn check_app_targets(packages: &[&Value]) {
    let mut expected_targets = BTreeSet::new();
    for pkg in packages {
        for t in pkg["targets"].as_array().expect("targets array") {
            let kinds = t["kind"].as_array().expect("target kinds");
            if kinds
                .iter()
                .any(|k| matches!(k.as_str(), Some("lib" | "bin")))
            {
                let name = t["name"].as_str().expect("target name").replace('-', "_");
                expected_targets.insert(name);
            }
        }
    }

    let listed_targets: BTreeSet<String> = server::APP_TARGETS
        .iter()
        .map(|s| (*s).to_string())
        .collect();

    for target in &expected_targets {
        assert!(
            listed_targets.contains(target),
            "workspace target `{target}` is missing from server::APP_TARGETS (its logs/spans would be dropped)"
        );
    }
    for target in &listed_targets {
        assert!(
            expected_targets.contains(target),
            "server::APP_TARGETS lists `{target}`, which is not a workspace lib or bin target"
        );
    }
}

fn try_raw_string_end(source: &str, i: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let prefix_len = if bytes.get(i) == Some(&b'r') {
        1
    } else if matches!(bytes.get(i), Some(b'b' | b'c')) && bytes.get(i + 1) == Some(&b'r') {
        2
    } else {
        return None;
    };
    if i > 0 && is_ident_char(bytes[i - 1]) {
        return None;
    }
    let mut hashes = 0usize;
    while bytes.get(i + prefix_len + hashes) == Some(&b'#') {
        hashes += 1;
    }
    if bytes.get(i + prefix_len + hashes) != Some(&b'"') {
        return None;
    }
    let mut cursor = i + prefix_len + hashes + 1;
    while cursor < bytes.len() {
        if bytes[cursor] == b'"'
            && bytes
                .get(cursor + 1..cursor + 1 + hashes)
                .is_some_and(|s| s.iter().all(|b| *b == b'#'))
        {
            return Some(cursor + 1 + hashes);
        }
        cursor += source[cursor..].chars().next()?.len_utf8();
    }
    Some(bytes.len())
}

fn try_char_literal_end(source: &str, i: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let prefix_len = if bytes[i] == b'\'' {
        0
    } else if bytes[i] == b'b' && bytes.get(i + 1) == Some(&b'\'') {
        if i > 0 && is_ident_char(bytes[i - 1]) {
            return None;
        }
        1
    } else {
        return None;
    };
    let body_start = i + prefix_len + 1;
    if bytes.get(body_start) == Some(&b'\\') {
        let mut scan = body_start + 1;
        while scan < bytes.len() && !matches!(bytes[scan], b'\'' | b'\n') && scan - body_start <= 8
        {
            scan += 1;
        }
        return (bytes.get(scan) == Some(&b'\'')).then_some(scan + 1);
    }
    let ch = source.get(body_start..)?.chars().next()?;
    (ch != '\n' && ch != '\'' && bytes.get(body_start + ch.len_utf8()) == Some(&b'\''))
        .then_some(body_start + ch.len_utf8() + 1)
}

fn strip_rust_comments(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut i = 0;
    let mut block_depth = 0usize;
    let mut in_string = false;

    while i < bytes.len() {
        if block_depth > 0 {
            if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                block_depth += 1;
                i += 2;
            } else if bytes[i] == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                block_depth -= 1;
                i += 2;
            } else {
                if bytes[i] == b'\n' {
                    out.push('\n');
                }
                i += 1;
            }
            continue;
        }

        if in_string {
            if bytes[i] == b'\\' && i + 1 < bytes.len() {
                let mut chars = source[i..].chars();
                if let Some(first) = chars.next() {
                    out.push(first);
                    if let Some(second) = chars.next() {
                        out.push(second);
                        i += first.len_utf8() + second.len_utf8();
                        continue;
                    }
                }
            }
            if bytes[i] == b'"' {
                in_string = false;
            }
            let ch = source[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }

        if let Some(end) = try_raw_string_end(source, i).or_else(|| try_char_literal_end(source, i))
        {
            out.push_str(&source[i..end]);
            i = end;
            continue;
        }

        if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            block_depth = 1;
            i += 2;
            continue;
        }
        if bytes[i] == b'"' {
            in_string = true;
        }
        let ch = source[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    if !dir.is_dir() {
        return;
    }
    for entry in fs::read_dir(dir).expect("read directory") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "target") {
                continue;
            }
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn crate_dir(pkg: &Value) -> &Path {
    Path::new(pkg["manifest_path"].as_str().expect("manifest_path"))
        .parent()
        .expect("manifest parent")
}

const fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn contains_ident(haystack: &str, ident: &str) -> bool {
    let bytes = haystack.as_bytes();
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(ident) {
        let abs = start + pos;
        let before_ok = abs == 0 || !is_ident_char(bytes[abs - 1]);
        let after = abs + ident.len();
        let after_ok = after == bytes.len() || !is_ident_char(bytes[after]);
        if before_ok && after_ok {
            return true;
        }
        start = abs + ident.len();
    }
    false
}

fn check_no_sql_outside_postgres(packages: &[&Value]) {
    for pkg in packages {
        let name = pkg["name"].as_str().expect("pkg name");
        if name == "persistence-postgres" {
            continue;
        }
        let root = crate_dir(pkg);
        let mut files = Vec::new();
        for sub in ["src", "tests", "benches", "examples"] {
            collect_rs_files(&root.join(sub), &mut files);
        }
        for path in files {
            // `crate_graph.rs` defines the `"use sqlx"` / `"sqlx::"` scan patterns.
            if name == "server" && path.ends_with("tests/crate_graph.rs") {
                continue;
            }
            let raw = fs::read_to_string(&path).expect("read rust source");
            let stripped = strip_rust_comments(&raw);
            for (idx, line) in stripped.lines().enumerate() {
                let line_no = idx + 1;
                if let Some(use_pos) = line.find("use sqlx") {
                    let tail = line[use_pos + "use sqlx".len()..].trim();
                    assert!(
                        tail.starts_with("::") && !tail.contains('{') && !tail.contains('*'),
                        "{}:{line_no}: forbidden `use sqlx` outside persistence-postgres (only `{:?}` allowed): {line}",
                        path.display(),
                        ALLOWED_SQLX_PATHS
                    );
                }
                let mut pos = 0;
                while let Some(found) = line[pos..].find("sqlx::") {
                    let abs = pos + found;
                    let before_ok = abs == 0 || !is_ident_char(line.as_bytes()[abs - 1]);
                    if before_ok {
                        let sqlx_path: String = line[abs..]
                            .chars()
                            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == ':')
                            .collect();
                        assert!(
                            ALLOWED_SQLX_PATHS.contains(&sqlx_path.as_str()),
                            "{}:{line_no}: forbidden `sqlx` path `{sqlx_path}` outside persistence-postgres (only `{:?}` allowed)",
                            path.display(),
                            ALLOWED_SQLX_PATHS
                        );
                    }
                    pos = abs + "sqlx::".len();
                }
            }
        }
    }
}

fn check_all_testing_fixtures_are_used(packages: &[&Value]) {
    let pg_pkg = packages
        .iter()
        .copied()
        .find(|p| p["name"].as_str() == Some("persistence-postgres"))
        .expect("persistence-postgres workspace package");
    let testing_rs = crate_dir(pg_pkg).join("src/testing.rs");
    let testing_src =
        fs::read_to_string(&testing_rs).expect("read persistence-postgres/src/testing.rs");
    let stripped_testing = strip_rust_comments(&testing_src);

    let mut fixture_names = Vec::new();
    for line in stripped_testing.lines() {
        let trimmed = line.trim_start();
        let after = trimmed
            .strip_prefix("pub async fn ")
            .or_else(|| trimmed.strip_prefix("pub fn "));
        if let Some(rest) = after {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                fixture_names.push(name);
            }
        }
    }
    assert!(
        !fixture_names.is_empty(),
        "expected public fixtures in {}",
        testing_rs.display()
    );

    let mut consumer_sources = String::new();
    for pkg in packages {
        if pkg["name"].as_str() == Some("persistence-postgres") {
            continue;
        }
        let root = crate_dir(pkg);
        let mut files = Vec::new();
        for sub in ["tests", "benches", "examples"] {
            collect_rs_files(&root.join(sub), &mut files);
        }
        for path in files {
            if path.ends_with("tests/crate_graph.rs") {
                continue;
            }
            let raw = fs::read_to_string(&path).expect("read consumer source");
            consumer_sources.push_str(&strip_rust_comments(&raw));
            consumer_sources.push('\n');
        }
    }

    for name in fixture_names {
        assert!(
            contains_ident(&consumer_sources, &name),
            "unused public fixture `{name}` in {}: remove it or call it from another crate's tests/benches",
            testing_rs.display()
        );
    }
}

#[test]
fn workspace_crate_graph_and_inner_deps_follow_hexagonal_rules() {
    let metadata = cargo_metadata();
    let ranks: BTreeMap<&str, u8> = RANKS.iter().copied().collect();
    let allowed: BTreeMap<&str, &[&str]> = ALLOWED_EDGES.iter().copied().collect();

    let workspace_ids: BTreeSet<&str> = metadata["workspace_members"]
        .as_array()
        .expect("workspace_members array")
        .iter()
        .map(|v| v.as_str().expect("member id string"))
        .collect();

    let packages: Vec<&Value> = metadata["packages"]
        .as_array()
        .expect("packages array")
        .iter()
        .filter(|p| workspace_ids.contains(p["id"].as_str().expect("pkg id")))
        .collect();

    let workspace_names: BTreeSet<&str> = packages
        .iter()
        .map(|p| p["name"].as_str().expect("pkg name"))
        .collect();

    check_edge_table(&workspace_names, &ranks, &allowed);
    check_declared_dependencies(&packages, &workspace_names, &ranks, &allowed);
    check_no_sql_outside_postgres(&packages);
    check_all_testing_fixtures_are_used(&packages);
    check_app_targets(&packages);
}

//! Workspace dependency & target gate (OT-012 design D6).
//!
//! Reads `cargo metadata --no-deps --offline --locked` and enforces:
//! 1. Every workspace crate is listed in `RANKS` and `ALLOWED_EDGES`.
//! 2. Every workspace->workspace dependency (normal, dev, build) is in
//!    `ALLOWED_EDGES` and points strictly inward (`rank(to) < rank(from)`).
//!    (`testkit`, if introduced later, is allowed only as a dev-dependency.)
//! 3. `domain` and `application` third-party dependencies (split by normal,
//!    optional-normal, dev, and build) stay within their allowlists.
//! 4. Every workspace lib and bin target name (with `-` -> `_`) is in
//!    `server::APP_TARGETS`, so no crate's logs or spans are silently dropped.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use serde_json::Value;

const RANKS: &[(&str, u8)] = &[
    ("shared", 0),
    ("domain", 0),
    ("application", 1),
    ("platform", 2),
    ("server", 4),
];

const ALLOWED_EDGES: &[(&str, &[&str])] = &[
    ("shared", &[]),
    ("domain", &[]),
    ("application", &["domain", "shared"]),
    ("platform", &["shared", "domain", "application"]),
    ("server", &["shared", "domain", "application", "platform"]),
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
                continue;
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

        if from == "application"
            && let Some(defaults) = pkg["features"].get("default").and_then(Value::as_array)
        {
            assert!(
                defaults.is_empty(),
                "`application` default features must be empty, found {defaults:?}"
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
    check_app_targets(&packages);
}

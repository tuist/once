use std::collections::BTreeSet;

use once_frontend::{ResolutionRecord, ResolverInputs};

use super::touched;

fn record(package: &str, patterns: &[&str], paths: &[&str]) -> ResolutionRecord {
    ResolutionRecord {
        resolvers: vec![ResolverInputs {
            package: package.to_string(),
            patterns: patterns.iter().map(|value| (*value).to_string()).collect(),
            excludes: Vec::new(),
            paths: paths.iter().map(|value| (*value).to_string()).collect(),
        }],
        observations: once_frontend::analysis::AnalysisObservations::default(),
    }
}

fn changed(paths: &[&str]) -> BTreeSet<String> {
    paths.iter().map(|value| (*value).to_string()).collect()
}

#[test]
fn an_edited_source_file_does_not_touch_the_derivation() {
    let record = record(
        "",
        &["Cargo.lock", "**/Cargo.toml"],
        &["Cargo.toml", "Cargo.lock"],
    );

    assert!(!touched(&record, &changed(&["crates/core/main.rs"])));
}

#[test]
fn editing_a_file_the_derivation_read_touches_it() {
    let record = record(
        "",
        &["Cargo.lock", "**/Cargo.toml"],
        &["Cargo.toml", "Cargo.lock"],
    );

    assert!(touched(&record, &changed(&["Cargo.lock"])));
}

/// The case a list of read files misses on its own: a manifest that did not
/// exist when the graph was derived is not in the list, and yet it changes what
/// the derivation would produce.
#[test]
fn a_manifest_that_appears_touches_the_derivation() {
    let record = record(
        "",
        &["Cargo.lock", "**/Cargo.toml"],
        &["Cargo.toml", "Cargo.lock"],
    );

    assert!(touched(&record, &changed(&["crates/new/Cargo.toml"])));
}

#[test]
fn a_resolver_in_a_package_matches_paths_under_that_package() {
    let record = record("apps/tool", &["**/Cargo.toml"], &["Cargo.toml"]);

    assert!(touched(&record, &changed(&["apps/tool/nested/Cargo.toml"])));
    assert!(!touched(&record, &changed(&["apps/other/Cargo.toml"])));
    assert!(
        touched(&record, &changed(&["apps/tool/Cargo.toml"])),
        "the package-relative path a resolver read is named without its package prefix"
    );
}

#[test]
fn nothing_changed_touches_nothing() {
    let record = record("", &["**/Cargo.toml"], &["Cargo.toml"]);

    assert!(!touched(&record, &changed(&[])));
}

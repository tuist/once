use serde::Deserialize;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DependencyEcosystem {
    Rust,
    Swift,
    Go,
    Elixir,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyEntry {
    pub package: String,
    pub name: String,
    pub ecosystem: DependencyEcosystem,
    pub manifest: String,
    pub lockfile: Option<String>,
    pub output: Option<String>,
}

pub(crate) fn into_entries(
    entries: Vec<DependencyEntryToml>,
    package: &str,
) -> Vec<DependencyEntry> {
    entries
        .into_iter()
        .map(|entry| DependencyEntry {
            package: package.to_string(),
            name: entry.name,
            ecosystem: entry.ecosystem,
            manifest: entry.manifest,
            lockfile: entry.lockfile,
            output: entry.output,
        })
        .collect()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DependencyEntryToml {
    name: String,
    ecosystem: DependencyEcosystem,
    manifest: String,
    lockfile: Option<String>,
    output: Option<String>,
}

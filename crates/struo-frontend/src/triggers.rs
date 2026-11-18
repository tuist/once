//! Data-only root manifest triggers and atomic declaration edits.

mod editor;
mod references;
mod validation;

use serde::{Deserialize, Serialize};

use crate::{BuildConfiguration, Diagnostic};

pub use editor::apply_trigger_operations;
pub use validation::validate_triggers;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TriggerSpec {
    pub id: String,
    pub on: TriggerOn,
    pub invoke: Vec<TriggerInvocation>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TriggerOn {
    pub event: String,
    pub refs: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TriggerInvocation {
    pub target: String,
    pub capability: String,
    pub configuration: TriggerConfiguration,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TriggerConfiguration {
    pub os: String,
    pub arch: String,
    pub tokens: Vec<String>,
}

impl TriggerConfiguration {
    /// Normalize an explicit platform and selection tokens without host defaults.
    pub fn resolve(&self) -> crate::Result<BuildConfiguration> {
        BuildConfiguration::from_explicit(&self.os, &self.arch, self.tokens.clone())
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum TriggerEditOperation {
    Create { trigger: TriggerSpec },
    Update { id: String, trigger: TriggerSpec },
    Delete { id: String },
}

#[derive(Deserialize)]
struct TriggerManifest {
    #[serde(default)]
    trigger: Vec<TriggerSpec>,
}

/// Read trigger declarations without loading target kinds or project modules.
/// `path` is used for diagnostics; the caller supplies the root manifest source.
pub fn load_triggers_toml_str(path: &str, src: &str) -> Result<Vec<TriggerSpec>, Vec<Diagnostic>> {
    let manifest = toml::from_str::<TriggerManifest>(src)
        .map_err(|error| vec![parse_diagnostic(path, error)])?;
    let diagnostics = validate_triggers(&manifest.trigger);
    if diagnostics.is_empty() {
        Ok(manifest.trigger)
    } else {
        Err(diagnostics)
    }
}

fn parse_diagnostic(path: &str, error: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::new(
        "trigger_parse_error",
        format!("could not parse trigger declarations in `{path}`: {error}"),
    )
    .with_attribute("trigger")
    .with_repair("Use [[trigger]] with id, on = { event, refs }, and invoke entries containing target, capability, and explicit configuration = { os, arch, tokens }")
}

pub(crate) fn root_only_diagnostic() -> Diagnostic {
    Diagnostic::new(
        "trigger_root_only",
        "trigger declarations are only allowed in the root struo.toml",
    )
    .with_attribute("trigger")
    .with_repair("Move [[trigger]] declarations to the workspace root struo.toml")
}

#[cfg(test)]
mod tests;

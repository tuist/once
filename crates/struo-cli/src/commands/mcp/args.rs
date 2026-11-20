//! Typed arguments for MCP tools.

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};

pub(super) fn tool_args(args: &Value) -> Value {
    if args.is_null() {
        json!({})
    } else {
        args.clone()
    }
}

/// Deserialize tool arguments in place; a missing `arguments` object reads as
/// `{}`.
pub(super) fn parse_args<T: DeserializeOwned>(args: &Value) -> serde_json::Result<T> {
    if args.is_null() {
        T::deserialize(&json!({}))
    } else {
        T::deserialize(args)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TriggerListArgs {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TriggerActivationArgs {
    pub(super) revision: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TriggerEventArgs {
    pub(super) id: String,
    pub(super) event: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TargetExecutionArgs {
    pub(super) target: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BuildTargetArgs {
    pub(super) target: String,
    #[serde(default)]
    pub(super) verify_reproducible: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct VerificationQueryArgs {
    pub(super) id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ValidateActionsArgs {
    pub(super) target: String,
    #[serde(default = "default_build_capability")]
    pub(super) capability: String,
    #[serde(default)]
    pub(super) action: Option<usize>,
}

pub(super) fn default_build_capability() -> String {
    "build".to_string()
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct ChangedPathsArgs {
    #[serde(default)]
    pub(super) changed_paths: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct TestPlanQueryArgs {
    #[serde(default)]
    pub(super) changed_paths: Vec<String>,
    #[serde(default)]
    pub(super) target: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct EvidenceQueryArgs {
    #[serde(default)]
    pub(super) subject: Option<String>,
    #[serde(default = "default_evidence_limit")]
    pub(super) limit: usize,
}

pub(super) fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GraphFingerprintArgs {
    #[serde(default = "default_true")]
    pub(super) include_sources: bool,
    #[serde(default = "default_true")]
    pub(super) include_toolchain: bool,
    #[serde(default = "default_true")]
    pub(super) include_manifest: bool,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct TestAttemptQueryArgs {
    #[serde(default)]
    pub(super) target: Option<String>,
    #[serde(default = "default_test_attempt_limit")]
    pub(super) limit: usize,
}

pub(super) const fn default_test_attempt_limit() -> usize {
    20
}

pub(super) const fn default_evidence_limit() -> usize {
    5
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExternalSourceArgs {
    pub(super) url: String,
    #[serde(default = "default_external_source_max_bytes")]
    pub(super) max_bytes: usize,
}

pub(super) const fn default_external_source_max_bytes() -> usize {
    256 * 1024
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ValidateModuleArgs {
    pub(super) path: String,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct TargetKindFilterArgs {
    #[serde(default)]
    pub(super) query: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MaterializeExampleArgs {
    pub(super) kind: String,
    pub(super) slug: String,
    #[serde(default)]
    pub(super) destination: String,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct RunTestsArgs {
    #[serde(default)]
    pub(super) target: Option<String>,
    #[serde(default)]
    pub(super) targets: Vec<String>,
    #[serde(default)]
    pub(super) changed_paths: Vec<String>,
    #[serde(default)]
    pub(super) jobs: Option<usize>,
    #[serde(default)]
    pub(super) test_unit: Option<String>,
    #[serde(default)]
    pub(super) test_batch: Option<String>,
    #[serde(default)]
    pub(super) summary_only: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RunTargetArgs {
    pub(super) target: String,
    #[serde(default)]
    pub(super) visible: bool,
    #[serde(default)]
    pub(super) arguments: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StartTargetArgs {
    pub(super) target: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RuntimeSessionArgs {
    pub(super) session_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RuntimeLogsArgs {
    pub(super) session_id: String,
    #[serde(default)]
    pub(super) source: Option<String>,
    #[serde(default)]
    pub(super) cursor: Option<String>,
    #[serde(default)]
    pub(super) limit: Option<usize>,
}

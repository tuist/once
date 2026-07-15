//! Once manifest frontend.
//!
//! Loads declarative `once.toml` package files into a shared
//! [`Target`] IR. TOML keeps target declarations literal and
//! straightforward for humans and agents to patch.

pub mod analysis;
mod cache_provider;
mod error;
mod examples;
mod graph;
mod manifest;
mod manifest_editor;
mod modules;
mod script;
mod target;
mod target_ref;
mod target_validator;
mod workspace;

/// The declarative per-package manifest file.
pub const TOML_BUILD_FILE_NAME: &str = "once.toml";

pub const BUILD_FILE_NAME: &str = TOML_BUILD_FILE_NAME;

pub use cache_provider::{
    CacheProviderConfig, ExecutionProviderConfig, InfrastructureConfig,
    InfrastructureProviderConfig, NamedCacheProviderConfig, TuistCacheProviderConfig,
    DEFAULT_TUIST_URL,
};
pub use error::{Error, Result};
pub use examples::{load_example_bundle, load_target_kind_example};
pub use graph::{
    built_in_target_kind_schema, built_in_target_kind_schemas, built_in_target_kind_schemas_result,
    graph_from_targets, graph_from_targets_result, load_graph_workspace,
    target_kind_schemas_for_workspace, AttrSchema, Capability, DepSchema, Diagnostic, GraphTarget,
    TargetKindExample, TargetKindExampleBundle, TargetKindExampleFile, TargetKindExampleRoot,
    TargetKindExampleSource, TargetKindSchema, TargetLabel, ToolRequirement,
};
pub use manifest::{load_cache_provider_toml_str, load_infrastructure_toml_str, load_toml_str};
pub use manifest_editor::{
    apply_operations, apply_operations_with_schemas, EditOperation, TargetSpec, TargetUpdate,
};
pub use script::{parse_script_annotations, ScriptAnnotations};
pub use target::{AttrValue, Target};
pub use target_ref::{
    absolutize, normalize_cli_target, normalize_cli_target_from, normalize_manifest_target,
    target_id, validate_target_name, TargetIdError,
};
pub use target_validator::validate_target;
pub use workspace::{
    load_cache_provider, load_cache_provider_override, load_file, load_infrastructure_config,
    load_workspace,
};

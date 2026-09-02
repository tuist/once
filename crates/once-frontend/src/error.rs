//! Frontend error type. Distinguishes I/O, walk, parse, and evaluation
//! failures so callers can react differently (e.g. surface a parse
//! error with a file path while suppressing a transient walk error).
//!
//! Typed sub-variants (`ScriptHeader`, `ManifestSchema`, etc.) carry
//! structured detail so the CLI can render targeted messages with
//! suggestions rather than forcing every rewrite to piggy-back on a
//! stringly-typed `message` field. Their outer Display expression is
//! deliberately just `{path}: {kind}` (or an operation-context phrase
//! for wrappers) so the CLI-edge error frame does not print the same
//! text twice when it collapses the anyhow chain.

use crate::target_ref::TargetIdError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to read {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to walk {root}: {source}")]
    Walk {
        root: String,
        #[source]
        source: walkdir::Error,
    },
    #[error("parse error in {path}:\n{message}")]
    Parse { path: String, message: String },
    #[error("evaluation error in {path}:\n{message}")]
    Eval { path: String, message: String },
    #[error("{path}: {kind}")]
    ScriptHeader { path: String, kind: ScriptHeaderError },
    #[error("{path}: {kind}")]
    ManifestSchema {
        path: String,
        kind: ManifestSchemaError,
    },
    #[error("{path}: {kind}")]
    CacheProvider {
        path: String,
        kind: CacheProviderError,
    },
    #[error("{path}: {kind}")]
    NativeProject {
        path: String,
        kind: NativeProjectError,
    },
    #[error("validating target `{target}` name in {path}")]
    TargetNameInvalid {
        path: String,
        target: String,
        #[source]
        source: TargetIdError,
    },
    #[error("resolving dep of target `{target}` in {path}")]
    DepReference {
        path: String,
        target: String,
        #[source]
        source: TargetIdError,
    },
}

/// Structured detail for a `# once` header failure. Kept as a typed
/// enum (rather than a free-form message) so renderers can attach
/// suggestions like the accepted directive list without re-parsing a
/// string.
#[derive(Debug, thiserror::Error)]
pub enum ScriptHeaderError {
    #[error("script is empty; add a shebang so Once knows how to run it")]
    Empty,
    #[error("script must start with a shebang so Once knows how to run it")]
    MissingShebang,
    #[error("shebang is empty; write it as `#!/usr/bin/env <runtime>`")]
    EmptyShebang,
    #[error("shebang must name a runtime after `env`, for example `#!/usr/bin/env python3`")]
    MissingRuntime,
    #[error(
        "unknown once directive `{directive}`; \
         known directives: input, needs, fingerprint, output, env, cwd, remote, output-symlinks"
    )]
    UnknownDirective { directive: String },
    #[error(
        "once {directive} expects a quoted string, for example `# once {directive} \"value\"`"
    )]
    DirectiveExpectedQuote { directive: String },
    #[error("once {directive} is missing a closing quote")]
    DirectiveMissingClosingQuote { directive: String },
    #[error("once {directive} only accepts one quoted string")]
    DirectiveExtraArgument { directive: String },
}

/// Structured detail for a schema failure in a `once.toml` manifest.
/// Each variant is one thing a person could get wrong when writing the
/// file. Named fields carry the offending target/field so a renderer
/// can quote them back or offer a fix.
#[derive(Debug, thiserror::Error)]
pub enum ManifestSchemaError {
    #[error("workspace configuration operating system and architecture must be non-empty")]
    ConfigurationPlatformEmpty,
    #[error("workspace configuration tokens must be non-empty")]
    ConfigurationTokenEmpty,
    #[error("module paths are only loaded from the root once.toml")]
    ModulePathsInPackage,
    #[error("use either [modules] or [rules], not both")]
    ModulesAndRulesBothSet,
    #[error("target name is required")]
    TargetNameMissing,
    #[error("target `{target}` kind is required")]
    TargetKindMissing { target: String },
    #[error(
        "target `{target}` dependency role `deps` must use the top-level `deps` field \
         instead of a nested table"
    )]
    DepsRoleReserved { target: String },
    #[error("target `{target}` deps must be an array of strings or a select table")]
    DepsWrongShape { target: String },
    #[error("target `{target}` deps entries must be strings")]
    DepsEntryNotString { target: String },
    #[error("target `{target}` deps select must be a table")]
    DepsSelectWrongShape { target: String },
    #[error(
        "target `{target}` deps select has no matching branch and no `default`; \
         add a `default = [...]` entry"
    )]
    DepsSelectNoMatch { target: String },
}

/// Structured detail for a cache-provider configuration failure.
/// Fires on the onboarding path (invalid workspace binding, missing
/// user infrastructure, malformed Tuist handle) where clarity matters
/// most because the user is often setting up caching for the first
/// time.
#[derive(Debug, thiserror::Error)]
pub enum CacheProviderError {
    #[error(
        "infrastructure `{name}` was not defined in this config; \
         add an `[infrastructures.{name}]` block or run `once auth login` for the default"
    )]
    InfrastructureNotFound { name: String },
    #[error(
        "infrastructure `{name}` is an execution provider, not a cache; \
         wire it under `[infrastructure.execution]`, not `[infrastructure.cache]`"
    )]
    ExecutionProviderNotCacheable { name: String },
    #[error("Tuist project handle `{raw}` must have the form `account/project`")]
    TuistHandleShape { raw: String },
    #[error(
        "cache infrastructure binding has an empty `name`; \
         set it to the name of an `[infrastructures.<name>]` block"
    )]
    EmptyInfrastructureName,
}

/// Structured detail for the two recurring native-project lookup
/// failures. Fires when a user names a native project that doesn't
/// exist in the prelude, or names one whose discovered package
/// doesn't match what was asked for. Other native-project errors
/// still flow through `Error::Eval`; they're one-off shapes that do
/// not benefit from typed variants until B3 rewrites want them.
#[derive(Debug, thiserror::Error)]
pub enum NativeProjectError {
    #[error("unknown native project `{name}`")]
    Unknown { name: String },
    #[error(
        "native project `{name}` does not match package `{package}`"
    )]
    PackageMismatch { name: String, package: String },
}

pub type Result<T> = std::result::Result<T, Error>;

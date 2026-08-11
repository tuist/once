//! Invocation-scoped build configuration.
//!
//! Parses `--config` overrides, merges them over the workspace-declared
//! configuration, and derives the configuration identity that scopes
//! configured targets. The effective configuration drives dependency
//! selection and Starlark analysis; its digest scopes output directories
//! and build receipts so two invocations with different overrides never
//! share results. When the overrides reproduce the workspace-default
//! configuration the path suffix is empty, keeping output paths
//! byte-identical to invocations that pass no overrides.

use std::path::Path;

use anyhow::{bail, Context, Result};
use once_cas::Digest;
use once_frontend::{load_workspace_configuration, BuildConfiguration, ConfigurationOverrides};

/// Number of configuration-digest hex characters folded into an output
/// path segment. Long enough to make collisions between two distinct
/// configurations negligible.
const PATH_DIGEST_HEX_LEN: usize = 16;

/// Recognized `--config` keys.
const KEY_OS: &str = "os";
const KEY_ARCH: &str = "arch";
const KEY_TOKEN: &str = "token";

/// The effective configuration for one invocation plus the identity
/// derived from it.
#[derive(Debug, Clone)]
pub struct ResolvedConfiguration {
    pub configuration: BuildConfiguration,
    /// Segment appended after a target id in output and receipt paths.
    /// Empty for the workspace-default configuration.
    pub path_suffix: String,
    pub digest: Digest,
}

/// Parse repeated `KEY=VALUE` override strings into typed overrides.
///
/// Recognized keys are `os`, `arch`, and `token` (repeatable).
pub fn parse_overrides(raw: &[String]) -> Result<ConfigurationOverrides> {
    let mut overrides = ConfigurationOverrides::default();
    for entry in raw {
        let (key, value) = entry
            .split_once('=')
            .with_context(|| format!("--config expects `KEY=VALUE`, got `{entry}`"))?;
        let key = key.trim();
        let value = value.trim();
        if value.is_empty() {
            bail!("--config `{key}` has an empty value");
        }
        match key {
            KEY_OS => overrides.os = Some(value.to_string()),
            KEY_ARCH => overrides.arch = Some(value.to_string()),
            KEY_TOKEN => overrides.tokens.push(value.to_string()),
            other => bail!(
                "unknown --config key `{other}` (expected `{KEY_OS}`, `{KEY_ARCH}`, or `{KEY_TOKEN}`)"
            ),
        }
    }
    Ok(overrides)
}

/// Merge invocation overrides over the workspace-declared configuration
/// and derive the identity that scopes configured targets.
pub fn resolve(
    workspace: &Path,
    overrides: &ConfigurationOverrides,
) -> Result<ResolvedConfiguration> {
    let baseline = load_workspace_configuration(workspace)?;
    let effective = baseline.merged_with(overrides);
    let effective_digest = Digest::of_bytes(&effective.canonical_bytes());
    let baseline_digest = Digest::of_bytes(&baseline.canonical_bytes());
    let path_suffix = if effective_digest == baseline_digest {
        String::new()
    } else {
        format!("/cfg-{}", &effective_digest.to_hex()[..PATH_DIGEST_HEX_LEN])
    };
    tracing::debug!(
        baseline_configuration = %baseline_digest,
        effective_configuration = %effective_digest,
        path_suffix = %path_suffix,
        "resolved invocation configuration"
    );
    Ok(ResolvedConfiguration {
        configuration: effective,
        path_suffix,
        digest: effective_digest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_os_arch_and_token_overrides() {
        let overrides = parse_overrides(&[
            "os=ios".to_string(),
            "arch=arm64".to_string(),
            "token=release".to_string(),
            "token=simulator".to_string(),
        ])
        .unwrap();
        assert_eq!(overrides.os.as_deref(), Some("ios"));
        assert_eq!(overrides.arch.as_deref(), Some("arm64"));
        assert_eq!(overrides.tokens, vec!["release", "simulator"]);
    }

    #[test]
    fn rejects_unknown_config_key() {
        let error = parse_overrides(&["mode=release".to_string()]).unwrap_err();
        assert!(error.to_string().contains("unknown --config key"));
    }

    #[test]
    fn rejects_config_without_equals() {
        let error = parse_overrides(&["release".to_string()]).unwrap_err();
        assert!(error.to_string().contains("KEY=VALUE"));
    }

    #[test]
    fn rejects_empty_config_value() {
        let error = parse_overrides(&["os=".to_string()]).unwrap_err();
        assert!(error.to_string().contains("empty value"));
    }
}

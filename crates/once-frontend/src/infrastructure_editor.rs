//! Writes an infrastructure binding into a root `once.toml`.
//!
//! `once connect` provisions a project with a provider and then records the
//! binding so later runs reuse the same cache or execution provider. This
//! module owns the manifest half of that workflow: it writes the canonical
//! `[infrastructures.<name>]` provider entry plus the
//! `[infrastructure.cache]` binding, preserving formatting and unrelated
//! sections. Callers pass the provisioned account and project; nothing here
//! knows which provider produced them.

use toml_edit::{value, DocumentMut, Item, Table};

use crate::graph::Diagnostic;

/// A provider binding ready to be written into a root `once.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfrastructureBinding {
    /// Instance name used under `[infrastructures.<name>]`.
    pub provider_name: String,
    /// Provider kind, such as `tuist`.
    pub kind: String,
    /// Server URL the provider talks to.
    pub url: String,
    /// Account or organization that owns the provisioned project.
    pub account: String,
    /// Project handle within the account.
    pub project: String,
    /// OAuth client id to pin for the provider, when one is configured.
    pub oauth_client_id: Option<String>,
}

/// Apply `binding` to `toml_src` and return the resulting manifest body.
///
/// The write is canonical and idempotent: the provider table always carries
/// `kind`, `url`, `account`, and `project`, while `[infrastructure.cache]`
/// only references the provider by name so the provider's account and project
/// win in resolution. An inline `[infrastructure.cache]` (`kind = "..."`) is
/// rejected with a repair rather than silently shadowed.
pub fn apply_infrastructure_binding(
    toml_src: &str,
    binding: &InfrastructureBinding,
) -> Result<String, Vec<Diagnostic>> {
    let provider_name = binding.provider_name.trim();
    if provider_name.is_empty() {
        return Err(vec![Diagnostic::new(
            "infrastructure_provider_name_required",
            "the provider name for an infrastructure binding must not be empty",
        )]);
    }
    if binding.account.trim().is_empty() {
        return Err(vec![Diagnostic::new(
            "infrastructure_account_required",
            "the account for an infrastructure binding must not be empty",
        )]);
    }
    if binding.project.trim().is_empty() {
        return Err(vec![Diagnostic::new(
            "infrastructure_project_required",
            "the project for an infrastructure binding must not be empty",
        )]);
    }

    let mut doc: DocumentMut = toml_src.parse().map_err(|err: toml_edit::TomlError| {
        vec![Diagnostic::new(
            "toml_parse_error",
            format!("could not parse `once.toml`: {err}"),
        )]
    })?;

    set_cache_binding(&mut doc, provider_name)?;
    set_provider(&mut doc, provider_name, binding)?;

    Ok(doc.to_string())
}

fn set_cache_binding(doc: &mut DocumentMut, provider_name: &str) -> Result<(), Vec<Diagnostic>> {
    if !doc.contains_key("infrastructure") {
        let mut table = Table::new();
        table.set_implicit(true);
        doc.insert("infrastructure", Item::Table(table));
    }
    let infrastructure = doc["infrastructure"].as_table_mut().ok_or_else(|| {
        vec![Diagnostic::new(
            "infrastructure_invalid",
            "`infrastructure` must be a table when recording a provider binding",
        )
        .with_repair("replace the `infrastructure` value with a `[infrastructure]` table")]
    })?;

    match infrastructure.get_mut("cache") {
        None => {
            let mut cache = Table::new();
            cache["provider"] = value(provider_name);
            infrastructure["cache"] = Item::Table(cache);
            Ok(())
        }
        Some(item) => {
            let cache = item.as_table_mut().ok_or_else(|| {
                vec![Diagnostic::new(
                    "infrastructure_cache_invalid",
                    "`infrastructure.cache` must be a table when recording a provider binding",
                )
                .with_repair(
                    "replace the inline `[infrastructure.cache]` value with a `provider = \"<name>\"` binding",
                )]
            })?;
            if cache.contains_key("kind") {
                return Err(vec![Diagnostic::new(
                    "infrastructure_cache_inline_provider",
                    "`[infrastructure.cache]` names a provider inline, which conflicts with a provisioned binding",
                )
                .with_repair(format!(
                    "remove `kind` from `[infrastructure.cache]` and set `provider = \"{provider_name}\"`"
                ))]);
            }
            // The provider table owns account and project. Drop any stale
            // overrides on the binding so resolution uses the provisioned
            // values rather than a leftover from a previous provider. Both
            // `name` and its `provider` alias are removed so the table never
            // carries two spellings of the same key.
            cache.remove("name");
            cache.remove("account");
            cache.remove("project");
            cache["provider"] = value(provider_name);
            Ok(())
        }
    }
}

fn set_provider(
    doc: &mut DocumentMut,
    provider_name: &str,
    binding: &InfrastructureBinding,
) -> Result<(), Vec<Diagnostic>> {
    if !doc.contains_key("infrastructures") {
        let mut table = Table::new();
        table.set_implicit(true);
        doc.insert("infrastructures", Item::Table(table));
    }
    let infrastructures = doc["infrastructures"].as_table_mut().ok_or_else(|| {
        vec![Diagnostic::new(
            "infrastructures_invalid",
            "`infrastructures` must be a table when recording a provider binding",
        )
        .with_repair("replace the `infrastructures` value with an `[infrastructures]` table")]
    })?;

    let mut provider = match infrastructures.get_mut(provider_name) {
        Some(item) => item.as_table_mut().map(std::mem::take).ok_or_else(|| {
            vec![Diagnostic::new(
                "infrastructure_provider_invalid",
                format!("`infrastructures.{provider_name}` must be a table"),
            )
            .with_repair(format!(
                "replace the `infrastructures.{provider_name}` value with a `[infrastructures.{provider_name}]` table"
            ))]
        })?,
        None => Table::new(),
    };
    provider["kind"] = value(&binding.kind);
    provider["url"] = value(&binding.url);
    provider["account"] = value(&binding.account);
    provider["project"] = value(&binding.project);
    if let Some(oauth_client_id) = &binding.oauth_client_id {
        provider["oauth_client_id"] = value(oauth_client_id);
    }
    infrastructures[provider_name] = Item::Table(provider);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache_resolution::{resolve_cache_provider, ResolvedCacheProviderConfig};

    fn binding() -> InfrastructureBinding {
        InfrastructureBinding {
            provider_name: "tuist".to_string(),
            kind: "tuist".to_string(),
            url: "https://tuist.dev".to_string(),
            account: "acme".to_string(),
            project: "app".to_string(),
            oauth_client_id: None,
        }
    }

    #[test]
    fn writes_canonical_binding_and_resolves() {
        let out = apply_infrastructure_binding("", &binding()).unwrap();

        assert!(out.contains("[infrastructures.tuist]"));
        assert!(out.contains("kind = \"tuist\""));
        assert!(out.contains("account = \"acme\""));
        assert!(out.contains("project = \"app\""));
        assert!(out.contains("[infrastructure.cache]"));
        assert!(out.contains("provider = \"tuist\""));

        let resolved = resolve_cache_provider_str(&out);
        assert_eq!(resolved, ("acme".to_string(), "app".to_string()));
    }

    #[test]
    fn preserves_unrelated_sections_and_comments() {
        let source = r#"# keep me
[[target]]
name = "app"
kind = "plain"
"#;
        let out = apply_infrastructure_binding(source, &binding()).unwrap();
        assert!(out.starts_with("# keep me\n"));
        assert!(out.contains("name = \"app\""));
    }

    #[test]
    fn is_idempotent() {
        let once = apply_infrastructure_binding("", &binding()).unwrap();
        let twice = apply_infrastructure_binding(&once, &binding()).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn updates_existing_provider_in_place() {
        let source = apply_infrastructure_binding("", &binding()).unwrap();
        let updated = InfrastructureBinding {
            project: "other".to_string(),
            ..binding()
        };
        let out = apply_infrastructure_binding(&source, &updated).unwrap();
        assert!(out.contains("project = \"other\""));
        assert!(!out.contains("project = \"app\""));
    }

    #[test]
    fn clears_stale_binding_overrides() {
        let source = r#"
[infrastructures.tuist]
kind = "tuist"
account = "old"
project = "old"

[infrastructure.cache]
provider = "tuist"
account = "stale"
project = "stale"
"#;
        let out = apply_infrastructure_binding(source, &binding()).unwrap();
        assert!(!out.contains("stale"));
        assert!(out.contains("account = \"acme\""));
        assert!(out.contains("project = \"app\""));
    }

    #[test]
    fn rejects_inline_cache_provider() {
        let source = r#"
[infrastructure.cache]
kind = "tuist"
account = "acme"
project = "app"
"#;
        let diagnostics = apply_infrastructure_binding(source, &binding()).unwrap_err();
        assert_eq!(diagnostics[0].code, "infrastructure_cache_inline_provider");
        assert!(!diagnostics[0].repairs.is_empty());
    }

    #[test]
    fn removes_name_alias_from_cache_binding() {
        let source = r#"
[infrastructures.tuist]
kind = "tuist"

[infrastructure.cache]
name = "tuist"
"#;
        let out = apply_infrastructure_binding(source, &binding()).unwrap();
        assert!(out.contains("provider = \"tuist\""));
        assert!(!out.contains("name = \"tuist\""));
        // The result must still parse, which it would not with both keys.
        resolve_cache_provider_str(&out);
    }

    #[test]
    fn writes_oauth_client_id_when_pinned() {
        let pinned = InfrastructureBinding {
            oauth_client_id: Some("client-123".to_string()),
            ..binding()
        };
        let out = apply_infrastructure_binding("", &pinned).unwrap();
        assert!(out.contains("oauth_client_id = \"client-123\""));
    }

    fn resolve_cache_provider_str(source: &str) -> (String, String) {
        let root = tempfile::TempDir::new().unwrap();
        std::fs::write(root.path().join("once.toml"), source).unwrap();
        let config = resolve_cache_provider(root.path(), &root.path().join("missing.toml"), None)
            .expect("resolve");
        match config {
            ResolvedCacheProviderConfig::Tuist(config) => (
                config.account.expect("account"),
                config.project.expect("project"),
            ),
            ResolvedCacheProviderConfig::Local => panic!("expected tuist provider"),
        }
    }
}

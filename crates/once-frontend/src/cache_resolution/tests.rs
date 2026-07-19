use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::{resolve_cache_provider, ResolvedCacheProviderConfig};

fn user_config_path(root: &Path) -> PathBuf {
    root.join("config").join("once").join("config.toml")
}

fn write_user_config(root: &Path, body: &str) -> PathBuf {
    let path = user_config_path(root);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, body).unwrap();
    path
}

#[test]
fn defaults_to_local_without_configuration() {
    let tmp = TempDir::new().unwrap();

    let config = resolve_cache_provider(tmp.path(), &user_config_path(tmp.path()), None).unwrap();

    assert_eq!(config, ResolvedCacheProviderConfig::Local);
}

#[test]
fn workspace_provider_beats_user_default() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("once.toml"),
        r#"
[infrastructure.cache]
provider = "local"

[infrastructures.local]
kind = "local"
"#,
    )
    .unwrap();
    let user_config = write_user_config(
        tmp.path(),
        r#"
[infrastructure.cache]
name = "tuist"

[infrastructures.tuist]
kind = "tuist"
account = "acme"
"#,
    );

    let config = resolve_cache_provider(tmp.path(), &user_config, None).unwrap();

    assert_eq!(config, ResolvedCacheProviderConfig::Local);
}

#[test]
fn override_beats_workspace_provider() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("once.toml"),
        r#"
[cache_provider]
kind = "tuist"
account = "acme"
"#,
    )
    .unwrap();

    let config = resolve_cache_provider(
        tmp.path(),
        &user_config_path(tmp.path()),
        Some("local".to_string()),
    )
    .unwrap();

    assert_eq!(config, ResolvedCacheProviderConfig::Local);
}

#[test]
fn resolves_named_workspace_provider_scope() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("once.toml"),
        r#"
[infrastructure.cache]
provider = "shared"
project = "app"

[infrastructures.shared]
kind = "tuist"
url = "https://cache.example.com"
account = "acme"
project = "default"
"#,
    )
    .unwrap();

    let config = resolve_cache_provider(tmp.path(), &user_config_path(tmp.path()), None).unwrap();
    let ResolvedCacheProviderConfig::Tuist(config) = config else {
        panic!("expected remote cache provider");
    };

    assert_eq!(config.provider_name, "shared");
    assert_eq!(config.url, "https://cache.example.com");
    assert_eq!(config.account.as_deref(), Some("acme"));
    assert_eq!(config.project.as_deref(), Some("app"));
}

#[test]
fn resolves_user_default_provider() {
    let tmp = TempDir::new().unwrap();
    let user_config = write_user_config(
        tmp.path(),
        r#"
[infrastructure.cache]
name = "shared"
project = "app"

[infrastructures.shared]
kind = "tuist"
url = "https://cache.example.com"
account = "acme"
"#,
    );

    let config = resolve_cache_provider(tmp.path(), &user_config, None).unwrap();
    let ResolvedCacheProviderConfig::Tuist(config) = config else {
        panic!("expected remote cache provider");
    };

    assert_eq!(config.provider_name, "shared");
    assert_eq!(config.project.as_deref(), Some("app"));
}

#[test]
fn resolves_legacy_tuist_manifest() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("Tuist.swift"),
        r#"
let ignored = "fullHandle: \"wrong/value\""
let tuist = Tuist(
    url: "https://cache.example.com",
    fullHandle: "acme/app"
)
"#,
    )
    .unwrap();

    let config = resolve_cache_provider(tmp.path(), &user_config_path(tmp.path()), None).unwrap();
    let ResolvedCacheProviderConfig::Tuist(config) = config else {
        panic!("expected remote cache provider");
    };

    assert_eq!(config.account.as_deref(), Some("acme"));
    assert_eq!(config.project.as_deref(), Some("app"));
    assert_eq!(config.url, "https://cache.example.com");
}

#[test]
fn rejects_malformed_legacy_handle() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("tuist.toml"), "project = \"invalid\"\n").unwrap();

    let error =
        resolve_cache_provider(tmp.path(), &user_config_path(tmp.path()), None).unwrap_err();

    assert!(error.to_string().contains("account/project"));
}

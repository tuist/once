use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::{resolve_cache_provider, ResolvedCacheProviderConfig};

fn user_config_path(root: &Path) -> PathBuf {
    root.join("config").join("struo").join("config.toml")
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
        tmp.path().join("struo.toml"),
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
        tmp.path().join("struo.toml"),
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
        tmp.path().join("struo.toml"),
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

#[test]
fn object_store_providers_resolve_from_workspace_direct_and_user_config() {
    use super::ResolvedObjectStoreCacheProviderConfig;
    use crate::ObjectStoreCacheProviderConfig;
    use std::collections::BTreeMap;

    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("struo.toml"),
        r#"
[infrastructure.cache]
provider = "bucket"

[infrastructures.bucket]
kind = "object_store"
url = "s3://team-cache/struo"
read_only = true
options = { endpoint = "http://localhost:9000", region = "us-east-1" }
"#,
    )
    .unwrap();
    let config = resolve_cache_provider(tmp.path(), &user_config_path(tmp.path()), None).unwrap();
    assert_eq!(
        config,
        ResolvedCacheProviderConfig::ObjectStore(ResolvedObjectStoreCacheProviderConfig {
            provider_name: "bucket".to_string(),
            store: ObjectStoreCacheProviderConfig {
                url: "s3://team-cache/struo".to_string(),
                options: BTreeMap::from([
                    ("endpoint".to_string(), "http://localhost:9000".to_string()),
                    ("region".to_string(), "us-east-1".to_string()),
                ]),
                read_only: true,
            },
        })
    );

    std::fs::write(
        tmp.path().join("struo.toml"),
        "[cache_provider]\nkind = \"object_store\"\nurl = \"gs://team-cache\"\n",
    )
    .unwrap();
    let ResolvedCacheProviderConfig::ObjectStore(direct) =
        resolve_cache_provider(tmp.path(), &user_config_path(tmp.path()), None).unwrap()
    else {
        panic!("expected object store");
    };
    assert_eq!(direct.store.url, "gs://team-cache");
    assert!(!direct.store.read_only);

    std::fs::remove_file(tmp.path().join("struo.toml")).unwrap();
    let user_config = write_user_config(
        tmp.path(),
        "[infrastructure.cache]\nname = \"dev\"\n\n[infrastructures.dev]\nkind = \"object_store\"\nurl = \"az://cache\"\n",
    );
    let ResolvedCacheProviderConfig::ObjectStore(user) =
        resolve_cache_provider(tmp.path(), &user_config, None).unwrap()
    else {
        panic!("expected object store");
    };
    assert_eq!(
        (user.provider_name.as_str(), user.store.url.as_str()),
        ("dev", "az://cache")
    );
}

#[test]
fn object_store_providers_reject_unknown_fields() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("struo.toml"),
        "[cache_provider]\nkind = \"object_store\"\nurl = \"s3://b\"\nsecret_access_key = \"x\"\n",
    )
    .unwrap();
    assert!(resolve_cache_provider(tmp.path(), &user_config_path(tmp.path()), None).is_err());
}

#[test]
fn http_cache_providers_resolve_and_keep_credentials_out_of_the_manifest() {
    use super::ResolvedHttpCacheProviderConfig;
    use crate::HttpCacheProviderConfig;

    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("struo.toml"),
        r#"
[infrastructure.cache]
provider = "depot"

[infrastructures.depot]
kind = "http_cache"
url = "https://cache.depot.dev"
username_env = "DEPOT_ORG_ID"
password_env = "DEPOT_CACHE_TOKEN"
read_only = true
"#,
    )
    .unwrap();
    assert_eq!(
        resolve_cache_provider(tmp.path(), &user_config_path(tmp.path()), None).unwrap(),
        ResolvedCacheProviderConfig::HttpCache(ResolvedHttpCacheProviderConfig {
            provider_name: "depot".to_string(),
            cache: HttpCacheProviderConfig {
                url: "https://cache.depot.dev".to_string(),
                key_prefix: None,
                token_env: None,
                username_env: Some("DEPOT_ORG_ID".to_string()),
                password_env: Some("DEPOT_CACHE_TOKEN".to_string()),
                read_only: true,
            },
        })
    );

    std::fs::write(
        tmp.path().join("struo.toml"),
        "[cache_provider]\nkind = \"http_cache\"\nurl = \"https://cache.example.com\"\nkey_prefix = \"team/\"\n",
    )
    .unwrap();
    let ResolvedCacheProviderConfig::HttpCache(direct) =
        resolve_cache_provider(tmp.path(), &user_config_path(tmp.path()), None).unwrap()
    else {
        panic!("expected http cache");
    };
    assert_eq!(direct.provider_name, "http_cache");
    assert_eq!(direct.cache.key_prefix.as_deref(), Some("team/"));

    std::fs::write(
        tmp.path().join("struo.toml"),
        "[cache_provider]\nkind = \"http_cache\"\nurl = \"https://c\"\ntoken = \"literal\"\n",
    )
    .unwrap();
    assert!(resolve_cache_provider(tmp.path(), &user_config_path(tmp.path()), None).is_err());

    std::fs::remove_file(tmp.path().join("struo.toml")).unwrap();
    let user_config = write_user_config(
        tmp.path(),
        "[infrastructure.cache]\nname = \"ns\"\n\n[infrastructures.ns]\nkind = \"http_cache\"\nurl = \"https://httpcache.ord.namespaceapis.com/main\"\ntoken_env = \"NSC_CACHE_TOKEN\"\n",
    );
    let ResolvedCacheProviderConfig::HttpCache(user) =
        resolve_cache_provider(tmp.path(), &user_config, None).unwrap()
    else {
        panic!("expected http cache");
    };
    assert_eq!(
        (user.provider_name.as_str(), user.cache.token_env.as_deref()),
        ("ns", Some("NSC_CACHE_TOKEN"))
    );
}

use std::collections::BTreeMap;

use super::*;

#[test]
fn default_cache_uses_xdg_cas_root() {
    let _env_lock = crate::TEST_ENV_LOCK.lock().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    std::env::set_var("XDG_CACHE_HOME", tmp.path());
    let cache = Cache::new();

    assert_eq!(cache.root(), tmp.path().join("once").join("cas"));
    std::env::remove_var("XDG_CACHE_HOME");
}

#[tokio::test]
async fn stores_and_reads_blobs() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cache = Cache::with_provider(CacheProvider::open_local(tmp.path()));

    let digest = cache.put_blob(b"hello").await.unwrap();

    assert!(cache.has_blob(digest).await.unwrap());
    assert_eq!(cache.get_blob(digest).await.unwrap(), b"hello");
}

#[tokio::test]
async fn stores_files_and_materializes_blobs() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cache = Cache::local(tmp.path().join("cache"));
    let input = tmp.path().join("inputs/payload.bin");
    let output = tmp.path().join("outputs/nested/payload.bin");
    std::fs::create_dir_all(input.parent().unwrap()).unwrap();
    std::fs::write(&input, b"file payload").unwrap();

    let digest = cache.put_file(&input).await.unwrap();
    let bytes = cache.get_blob_to_file(digest, &output).await.unwrap();

    assert_eq!(bytes, 12);
    assert_eq!(std::fs::read(output).unwrap(), b"file payload");
}

#[test]
fn workspace_constructor_uses_explicit_local_provider() {
    let _env_lock = crate::TEST_ENV_LOCK.lock().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    std::env::set_var("XDG_CACHE_HOME", tmp.path().join("cache-home"));
    std::fs::write(
        tmp.path().join("once.toml"),
        "[cache_provider]\nkind = \"local\"\n",
    )
    .unwrap();

    let cache = Cache::from_workspace(tmp.path()).unwrap();

    assert_eq!(
        cache.root(),
        tmp.path().join("cache-home").join("once").join("cas")
    );
    std::env::remove_var("XDG_CACHE_HOME");
}

#[tokio::test]
async fn stores_and_reads_action_results() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cache = Cache::with_provider(CacheProvider::open_local(tmp.path()));
    let stdout = cache.put_blob(b"out").await.unwrap();
    let action = Digest::of_bytes(b"action");
    let result = ActionResult {
        exit_code: 0,
        stdout: Some(stdout),
        stderr: None,
        outputs: BTreeMap::new(),
    };

    cache.put_action_result(action, &result).await.unwrap();

    assert_eq!(cache.get_action_result(action).await.unwrap(), Some(result));
    assert!(cache.forget_action(action).await.unwrap());
    assert_eq!(cache.get_action_result(action).await.unwrap(), None);
}

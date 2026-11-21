use std::collections::HashSet;
use std::sync::Arc;
use struo_cas::{CacheProvider, Digest};
use struo_core::{
    Action, ExecutorResources, RemoteIoLimits, ResourceLimits, ResourcePool, RunOpts, Runner,
};

#[test]
fn legacy_command_encoding_uses_the_composed_input_digest_domain() {
    let wire = br#"{"kind":"run_command","argv":["tool"]}"#;
    let action: Action = serde_json::from_slice(wire).unwrap();
    assert_eq!(serde_json::to_vec(&action).unwrap(), wire);
    let mut expected = b"struo.action.v12\0".to_vec();
    expected.extend_from_slice(wire);
    assert_eq!(action.digest(), Digest::of_bytes(&expected));
}

#[test]
fn large_literal_artifact_hashes_match_the_legacy_json_encoding() {
    let action = Action::WriteFile {
        path: struo_core::WorkspacePath::try_from("out/large").unwrap(),
        bytes: vec![b'x'; 2 * 1024 * 1024].into(),
        input_digest: None,
    };
    let mut legacy = b"struo.action.v12\0".to_vec();
    legacy.extend_from_slice(&serde_json::to_vec(&action).unwrap());
    assert_eq!(action.digest(), Digest::of_bytes(&legacy));
}

#[test]
fn provider_environment_and_tenant_boundaries_remain_in_cache_identity() {
    let mut digests = HashSet::new();
    for remote in [
        r#"{"provider":"fleet-a","executor":"e2b","environment":"template-v1"}"#,
        r#"{"provider":"fleet-b","executor":"e2b","environment":"template-v1"}"#,
        r#"{"provider":"fleet-a","executor":"daytona","environment":"template-v1"}"#,
        r#"{"provider":"fleet-a","executor":"e2b","environment":"template-v2"}"#,
        r#"{"provider":"fleet-a","executor":"e2b","environment":"template-v1","account":"other"}"#,
        r#"{"provider":"fleet-a","executor":"e2b","environment":"template-v1","project":"other"}"#,
    ] {
        let wire = format!(r#"{{"kind":"run_command","argv":["tool"],"remote":{remote}}}"#);
        let action: Action = serde_json::from_str(&wire).unwrap();
        assert!(
            digests.insert(action.digest()),
            "execution scopes shared a key"
        );
        let restored: Action =
            serde_json::from_slice(&serde_json::to_vec(&action).unwrap()).unwrap();
        assert_eq!(action.digest(), restored.digest());
    }
}

#[tokio::test]
async fn runtime_io_limits_do_not_change_action_or_cache_identity() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::open_local(workspace.path().join("cas"));
    let action: Action =
        serde_json::from_str(r#"{"kind":"write_file","path":"out/value","bytes":[104,105]}"#)
            .unwrap();
    let runner = Runner::with_cache(cache, workspace.path(), RunOpts::default());
    let first = runner.run(&action).await.unwrap();
    for bytes in [1, 1 << 20] {
        let resources =
            ExecutorResources::new(Arc::new(ResourcePool::new(ResourceLimits::new(1, 1 << 20))))
                .with_remote_io_limits(RemoteIoLimits {
                    max_response_bytes: bytes,
                    max_archive_bytes: bytes,
                    ..RemoteIoLimits::default()
                });
        let hit = runner
            .clone()
            .with_executor_resources(resources)
            .run(&action)
            .await
            .unwrap();
        assert_eq!(hit.action, first.action);
        assert_eq!(hit.cache, struo_core::CacheState::Hit);
    }
}

/// Placement never widens result sharing. Every binding below, including a
/// named local binding and the absence of a binding, keys a distinct cache
/// slot until immutable environment identity is established.
#[test]
fn no_execution_scope_shares_a_key_with_another() {
    let command = |extra: &str| -> Action {
        serde_json::from_str(&format!(
            r#"{{"kind":"run_command","argv":["tool"]{extra}}}"#
        ))
        .unwrap()
    };
    let scopes = [
        String::new(),
        r#","remote":{"provider":"local"}"#.to_string(),
        r#","remote":{"provider":"native","executor":"local"}"#.to_string(),
        r#","remote":{"provider":"vm","executor":"microsandbox"}"#.to_string(),
        r#","remote":{"provider":"vm","executor":"microsandbox","environment":"alpine"}"#
            .to_string(),
        r#","remote":{"provider":"fleet","executor":"e2b"}"#.to_string(),
        r#","remote":{"provider":"fleet","executor":"e2b","environment":"template"}"#.to_string(),
        r#","remote":{"provider":"fleet","executor":"daytona","environment":"template"}"#
            .to_string(),
        r#","resources":{"cpu_slots":2,"memory_bytes":0}"#.to_string(),
        r#","resources":{"cpu_slots":1,"memory_bytes":1048576}"#.to_string(),
    ];
    let mut digests = HashSet::new();
    for scope in &scopes {
        assert!(
            digests.insert(command(scope).digest()),
            "scope `{scope}` shared a key"
        );
    }
}

#[test]
fn runtime_capture_and_staging_limits_are_not_part_of_the_action() {
    let action: Action = serde_json::from_str(
        r#"{"kind":"run_command","argv":["tool"],"remote":{"provider":"fleet","executor":"e2b"}}"#,
    )
    .unwrap();
    let before = action.digest();
    let _resources =
        ExecutorResources::new(Arc::new(ResourcePool::new(ResourceLimits::new(1, 1 << 20))))
            .with_remote_io_limits(RemoteIoLimits {
                max_capture_bytes: 1,
                max_capture_staging_bytes: 1,
                max_staging_bytes: 1,
                command_idle_timeout: Some(std::time::Duration::from_secs(1)),
                ..RemoteIoLimits::default()
            });
    assert_eq!(action.digest(), before);
    assert!(!serde_json::to_string(&action).unwrap().contains("capture"));
}

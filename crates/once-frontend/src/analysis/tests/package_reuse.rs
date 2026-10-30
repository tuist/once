use super::*;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

fn executable(path: &Path, source: &str) {
    std::fs::write(path, source).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}

fn commit(root: &Path, contents: &str) -> String {
    std::fs::write(root.join("Package.swift"), contents).unwrap();
    git(root, &["add", "."]);
    git(
        root,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-m",
            contents,
        ],
    );
    git(root, &["rev-parse", "HEAD"])
}

fn acquire(
    root: &Path,
    tool: &Path,
    remote: &Path,
    revision: &str,
    name: &str,
) -> Result<(), String> {
    let store = AnalysisStore::with_host_cache(
        root.into(),
        String::new(),
        String::new(),
        HostCache::with_tool_paths(BTreeMap::from([("git".into(), tool.display().to_string())])),
    );
    let source = format!(
        "host_git_checkout({:?}, {:?}, {:?}, {:?})",
        remote.display().to_string(),
        revision,
        root.join(name).display().to_string(),
        root.join("cache").display().to_string()
    );
    let (_, result) = with_active_store(store, || run(&source));
    result.map_err(|error| format!("{error:#}"))
}

#[test]
fn shared_git_preserves_origin_identity_when_git_rewrites_repository_urls() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let remote = root.join("remote");
    std::fs::create_dir(&remote).unwrap();
    git(&remote, &["init"]);
    let revision = commit(&remote, "first");
    let tool = Path::new("/usr/bin/git");
    acquire(root, tool, &remote, &revision, "checkout").unwrap();
    git(
        &root.join("checkout"),
        &[
            "config",
            "url.unused-mirror.insteadOf",
            remote.to_str().unwrap(),
        ],
    );
    acquire(root, tool, &remote, &revision, "checkout").unwrap();
    assert_eq!(
        git(&root.join("checkout"), &["rev-parse", "HEAD"]),
        revision
    );
}

#[test]
fn shared_git_reuses_objects_and_preserves_pins_across_workspaces_and_failed_fetches() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let remote = root.join("remote");
    std::fs::create_dir(&remote).unwrap();
    git(&remote, &["init"]);
    let first = commit(&remote, "first");
    let tool = root.join("git-wrapper");
    executable(
        &tool,
        "#!/bin/sh\nfor arg do [ \"$arg\" != fetch ] || exit 91; done\nexec /usr/bin/git \"$@\"\n",
    );
    acquire(root, &tool, &remote, &first, "one/.once/packages/identity").unwrap();
    let one = root.join("one/.once/packages/identity");
    std::thread::scope(|scope| {
        for index in 0..4 {
            let tool = &tool;
            let remote = &remote;
            let first = &first;
            scope.spawn(move || {
                acquire(
                    root,
                    tool,
                    remote,
                    first,
                    &format!("concurrent-{}/identity", index % 2),
                )
                .unwrap();
            });
        }
    });
    std::fs::rename(&remote, root.join("offline")).unwrap();
    acquire(root, &tool, &remote, &first, "two/.once/packages/identity").unwrap();
    acquire(root, &tool, &remote, &first, "one/.once/packages/identity").unwrap();
    let two = root.join("two/.once/packages/identity");
    assert_eq!(
        std::fs::read_to_string(two.join("Package.swift")).unwrap(),
        "first"
    );
    std::fs::rename(root.join("offline"), &remote).unwrap();
    let second = commit(&remote, "second");
    // Reject shallow clients, but allow the ordinary fallback and local object transfer.
    executable(&tool, "#!/bin/sh\nfor arg do [ \"$arg\" != --depth ] || exit 92; done\nexec /usr/bin/git \"$@\"\n");
    acquire(root, &tool, &remote, &second, "one/.once/packages/identity").unwrap();
    assert_eq!(
        std::fs::read_to_string(one.join("Package.swift")).unwrap(),
        "second"
    );
    assert_eq!(git(&two, &["rev-parse", "HEAD"]), first);
    let error = acquire(
        root,
        &tool,
        &remote,
        &"0".repeat(40),
        "one/.once/packages/identity",
    )
    .unwrap_err();
    assert!(error.contains("repository acquisition failed"), "{error}");
    assert_eq!(git(&one, &["rev-parse", "HEAD"]), second);
    assert!(acquire(root, &tool, &remote, &"0".repeat(40), "failed").is_err());
    assert!(!root.join("failed").exists());
    acquire(
        root,
        &tool,
        &remote,
        &first,
        "three/.once/packages/identity",
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("three/.once/packages/identity/Package.swift")).unwrap(),
        "first"
    );
    let third = commit(&remote, "third");
    executable(&tool, "#!/bin/sh\nif [ \"$3\" = fetch ] && [ \"$4\" != --depth ] && [ \"$4\" != --no-tags ]; then exit 93; fi\nexec /usr/bin/git \"$@\"\n");
    acquire(root, &tool, &remote, &third, "one/.once/packages/identity").unwrap();
    assert_eq!(
        std::fs::read_to_string(one.join("Package.swift")).unwrap(),
        "third"
    );
    let other = root.join("other-remote");
    let error = acquire(root, &tool, &other, &third, "one/.once/packages/identity").unwrap_err();
    assert!(error.contains("different repository"), "{error}");
    assert!(acquire(root, &tool, &other, &third, "retry/identity").is_err());
    assert!(!root.join("retry/identity").exists());
    std::os::unix::fs::symlink(&remote, &other).unwrap();
    acquire(root, &tool, &other, &third, "retry/identity").unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("retry/identity/Package.swift")).unwrap(),
        "third"
    );
    std::fs::write(one.join("Package.swift"), "uncommitted").unwrap();
    assert!(acquire(root, &tool, &remote, &second, "one/.once/packages/identity").is_err());
    assert_eq!(git(&one, &["rev-parse", "HEAD"]), third);
    assert_eq!(
        std::fs::read_to_string(one.join("Package.swift")).unwrap(),
        "uncommitted"
    );
}

#[test]
fn cached_discovery_reuses_success_and_invalidates_file_tool_and_environment_changes() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let input = root.join("input");
    let tool = root.join("metadata");
    std::fs::write(&input, "first").unwrap();
    executable(
        &tool,
        "#!/bin/sh\nprintf x >> \"$1/count\"\ncat \"$1/input\"\nprintf %s \"$MODE\"\n",
    );
    let invoke = |mode: &str| {
        let source = format!(
            "value = host_cached_command([{:?}, {:?}], inputs = [{:?}], env = {{\"MODE\": {:?}}})",
            tool.display().to_string(),
            root.display().to_string(),
            input.display().to_string(),
            mode
        );
        let (_, result) = with_active_store(
            AnalysisStore::new(root.into(), String::new(), String::new()),
            || eval_string(&source),
        );
        result.unwrap()
    };
    assert_eq!(invoke("a"), "firsta");
    assert_eq!(invoke("a"), "firsta");
    assert_eq!(std::fs::read_to_string(root.join("count")).unwrap(), "x");
    std::fs::write(&input, "changed").unwrap();
    assert_eq!(invoke("a"), "changeda");
    assert_eq!(invoke("b"), "changedb");
    executable(
        &tool,
        "#!/bin/sh\nprintf x >> \"$1/count\"\nprintf replaced\n",
    );
    assert_eq!(invoke("b"), "replaced");
    assert_eq!(std::fs::read_to_string(root.join("count")).unwrap(), "xxxx");
    executable(&tool, "#!/bin/sh\nprintf x >> \"$1/count\"\nexit 17\n");
    let source = format!(
        "host_cached_command([{:?}, {:?}], inputs = [{:?}])",
        tool.display().to_string(),
        root.display().to_string(),
        input.display().to_string()
    );
    for _ in 0..2 {
        let (_, result) = with_active_store(
            AnalysisStore::new(root.into(), String::new(), String::new()),
            || run(&source),
        );
        assert!(result.is_err());
    }
    assert_eq!(
        std::fs::read_to_string(root.join("count")).unwrap(),
        "xxxxxx"
    );
    let unchecked = format!("value = {}", source.replace(')', ", check = False)"));
    for _ in 0..2 {
        let (store, result) = with_active_store(
            AnalysisStore::new(root.into(), String::new(), String::new()),
            || eval_string(&unchecked),
        );
        assert_eq!(result.unwrap(), "");
        assert!(!AnalysisEngine::new().unwrap().observations_hold(
            root,
            &store.observations,
            CommandPolicy::TrustDeclaredInputs,
            &UnchangedWorkspace::Unknown,
        ));
    }
    assert_eq!(
        std::fs::read_to_string(root.join("count")).unwrap(),
        "xxxxxxxx"
    );
}

#[test]
fn shared_git_ignores_repository_location_inherited_from_the_callers_checkout() {
    const CHILD_ROOT: &str = "ONCE_GIT_TEST_ROOT";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = Path::new(&root);
        let revision = std::env::var("ONCE_GIT_TEST_REVISION").unwrap();
        acquire(
            root,
            Path::new("/usr/bin/git"),
            &root.join("remote"),
            &revision,
            "checkout",
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("checkout/Package.swift")).unwrap(),
            "pinned"
        );
        return;
    }
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let remote = root.join("remote");
    std::fs::create_dir(&remote).unwrap();
    git(&remote, &["init"]);
    let revision = commit(&remote, "pinned");
    std::fs::write(remote.join("Package.swift"), "user edits").unwrap();
    let output = Command::new(std::env::current_exe().unwrap()).args(["--exact", "analysis::tests::package_reuse::shared_git_ignores_repository_location_inherited_from_the_callers_checkout"]).env(CHILD_ROOT, root).env("ONCE_GIT_TEST_REVISION", revision).env("GIT_DIR", remote.join(".git")).env("GIT_WORK_TREE", &remote).env("GIT_INDEX_FILE", remote.join(".git/index")).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        std::fs::read_to_string(remote.join("Package.swift")).unwrap(),
        "user edits"
    );
}

#[test]
fn cached_discovery_invalidates_inherited_environment_without_persisting_values() {
    const CHILD_ROOT: &str = "ONCE_METADATA_TEST_ROOT";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = Path::new(&root);
        let source = format!(
            "value = host_cached_command([{:?}], inputs = [])",
            root.join("tool").display().to_string()
        );
        let (_, result) = with_active_store(
            AnalysisStore::new(root.into(), String::new(), String::new()),
            || eval_string(&source),
        );
        std::fs::write(root.join("answer"), result.unwrap()).unwrap();
        return;
    }
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    executable(
        &root.join("tool"),
        "#!/bin/sh\nprintf x >> count\nprintf %s \"$ONCE_METADATA_TEST_VALUE\"\n",
    );
    for (value, count) in [
        ("one-secret", "x"),
        ("one-secret", "x"),
        ("two-secret", "xx"),
    ] {
        let output = Command::new(std::env::current_exe().unwrap()).args(["--exact", "analysis::tests::package_reuse::cached_discovery_invalidates_inherited_environment_without_persisting_values"]).env(CHILD_ROOT, root).env("ONCE_METADATA_TEST_VALUE", value).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(std::fs::read_to_string(root.join("answer")).unwrap(), value);
        assert_eq!(std::fs::read_to_string(root.join("count")).unwrap(), count);
    }
    // The answer can contain a value the command deliberately prints; the key must not.
    for entry in std::fs::read_dir(root.join(".once/metadata-v1")).unwrap() {
        let stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap();
        assert!(!stored["key"].as_str().unwrap().contains("secret"));
    }
}

fn counted_git(workspace: &Path, expected: usize) -> std::path::PathBuf {
    std::fs::create_dir(workspace.join("started")).unwrap();
    let tool = workspace.join("git-wrapper");
    executable(
        &tool,
        &format!(
            r#"#!/bin/sh
if [ "$1" != clone ] || [ "$2" != --mirror ]; then exec /usr/bin/git "$@"; fi
printf 'start %s\n' "$$" >> '{workspace}/events'
mkdir '{workspace}/started/'"$$"
attempt=0
while [ "$(ls '{workspace}/started' | wc -l)" -lt {expected} ]; do
    attempt=$((attempt + 1))
    [ "$attempt" -lt 500 ] || exit 98
    sleep 0.01
done
/usr/bin/git "$@"
status=$?
printf 'end %s\n' "$$" >> '{workspace}/events'
exit "$status"
"#,
            workspace = workspace.display()
        ),
    );
    tool
}

#[test]
fn batch_git_acquisition_shares_memory_and_cpu_admission_and_reuses_offline_checkouts() {
    use once_core::{ResourceLimits, ResourcePool, ResourceRequest};
    use std::sync::Arc;
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let remotes: Vec<_> = (0..3)
        .map(|index| {
            let path = root.join(format!("remote-{index}"));
            std::fs::create_dir(&path).unwrap();
            git(&path, &["init"]);
            let revision = commit(&path, &format!("package-{index}"));
            (path, revision)
        })
        .collect();
    for (cpu, memory_mib, held, expected) in
        [(4, 500, false, 2), (1, 2000, false, 1), (4, 750, true, 2)]
    {
        let workspace = TempDir::new_in(root).unwrap();
        let workspace = workspace.path();
        let tool = counted_git(workspace, expected);
        let resources = Arc::new(ResourcePool::new(ResourceLimits::new(
            cpu,
            memory_mib * 1024 * 1024,
        )));
        let reservation = held
            .then(|| futures::executor::block_on(resources.acquire(ResourceRequest::default())));
        let requests: Vec<_> = remotes.iter().enumerate().map(|(index, (remote, revision))| serde_json::json!({
            "url": remote, "revision": revision, "path": workspace.join(format!("checkout-{index}")),
        })).collect();
        let source = format!(
            "host_git_checkouts({}, cache_root = {:?})",
            serde_json::to_string(&requests).unwrap(),
            workspace.join("cache").display().to_string()
        );
        let invoke = |source: &str| {
            with_analysis_resources_blocking(Arc::clone(&resources), || {
                let store = AnalysisStore::with_host_cache(
                    workspace.into(),
                    String::new(),
                    String::new(),
                    HostCache::with_tool_paths(BTreeMap::from([(
                        "git".into(),
                        tool.display().to_string(),
                    )])),
                );
                with_active_store(store, || run(source)).1
            })
        };
        invoke(&source).unwrap();
        let events = std::fs::read_to_string(workspace.join("events")).unwrap();
        let (mut active, mut peak) = (0, 0);
        for event in events.lines() {
            if event.starts_with("start ") {
                active += 1;
            } else {
                active -= 1;
            }
            peak = peak.max(active);
        }
        assert_eq!((active, peak), (0, expected));
        executable(&tool, "#!/bin/sh\nfor arg do [ \"$arg\" != fetch ] && [ \"$arg\" != clone ] || exit 91; done\nexec /usr/bin/git \"$@\"\n");
        invoke(&source).unwrap();
        let failed = source.replace(&remotes[2].1, &"f".repeat(40));
        assert!(invoke(&failed)
            .unwrap_err()
            .to_string()
            .contains("repository acquisition failed"));
        for (index, (_, revision)) in remotes.iter().enumerate() {
            let checkout = workspace.join(format!("checkout-{index}"));
            assert_eq!(git(&checkout, &["rev-parse", "HEAD"]), *revision);
            assert_eq!(
                std::fs::read_to_string(checkout.join("Package.swift")).unwrap(),
                format!("package-{index}")
            );
        }
        drop(reservation);
        let permits: Vec<_> = (0..resources.max_parallel_actions())
            .map(|_| {
                use futures::FutureExt;
                resources
                    .acquire(ResourceRequest::default())
                    .now_or_never()
                    .expect("batch leaked a reservation")
            })
            .collect();
        drop(permits);
    }
}

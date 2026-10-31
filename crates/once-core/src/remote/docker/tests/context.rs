use std::time::Duration;

use super::*;

#[path = "context_clock.rs"]
mod clock;
#[path = "context_fixture.rs"]
mod fixture;

const CHILD_ROOT: &str = "ONCE_DOCKER_CONTEXT_TEST_ROOT";
const TEST: &str =
    "remote::docker::tests::context::real_docker_context_switch_retains_public_execution_admission";

#[tokio::test]
#[ignore = "requires Docker, Python 3 and ONCE_DOCKER_TEST_IMAGE"]
async fn real_docker_context_switch_retains_public_execution_admission() {
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        public_execution_child(Path::new(&root)).await;
        return;
    }
    let binary = std::env::var_os("ONCE_DOCKER_BIN").unwrap_or_else(|| "docker".into());
    let client = process::Client::new(binary.clone(), RemoteIoLimits::default())
        .connect()
        .await
        .unwrap();
    let archive = client
        .control(&[
            "context".into(),
            "export".into(),
            "once-pinned".into(),
            "-".into(),
        ])
        .await
        .unwrap();
    for switch in [false, true] {
        let fixture = fixture::Fixture::new(&binary, &archive).await;
        if switch {
            std::fs::write(fixture.root.path().join("switch"), b"").unwrap();
        }
        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap());
        fixture::clear_connection_environment(&mut child);
        let output = child
            .args(["--exact", TEST, "--ignored", "--nocapture"])
            .env(CHILD_ROOT, fixture.root.path())
            .env("ONCE_DOCKER_REAL_BIN", &binary)
            .env(
                "ONCE_DOCKER_BIN",
                fixture.root.path().join("docker-wrapper"),
            )
            .env("DOCKER_CONFIG", fixture.root.path().join("config"))
            .kill_on_drop(true)
            .output()
            .await
            .unwrap();
        // Always clean the real container, including when the child assertion fails.
        if let Ok(id) = std::fs::read_to_string(fixture.root.path().join("container-id")) {
            let _ = client
                .control(&[
                    "rm".into(),
                    "--force".into(),
                    "--volumes".into(),
                    id.trim().into(),
                ])
                .await;
            let remaining = client
                .control(&[
                    "ps".into(),
                    "--all".into(),
                    "--filter".into(),
                    format!("id={}", id.trim()),
                    "--format".into(),
                    "{{.ID}}".into(),
                ])
                .await
                .unwrap();
            assert!(remaining.is_empty(), "test container was not removed");
        }
        assert!(
            output.status.success(),
            "switch={switch}\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(
            fixture.requests(),
            0,
            "a pinned operation contacted the second daemon"
        );
        eprintln!(
            "switch={switch}: public timeout, live 64 MiB container, host and exclusive slot retained, original-daemon recovery released admission; second-daemon requests=0"
        );
    }
}

async fn public_execution_child(root: &Path) {
    let request = ResourceRequest::new(1, 64 * 1024 * 1024).with_slot("exclusive", 1);
    let host = ExecutorResources::new(Arc::new(ResourcePool::new(ResourceLimits::new(
        2,
        128 * 1024 * 1024,
    ))));
    let cache = CacheProvider::open_local(root.join("cache"));
    let script = "touch /tmp/once-context-ready; sleep 60";
    let mut action = action(&["/bin/sh", "-c", script], real_image());
    if let Action::RunCommand {
        resources,
        timeout_ms,
        ..
    } = &mut action
    {
        *resources = request.clone();
        *timeout_ms = Some(4000);
    }
    let lease = host.acquire(Executor::Docker, &request).await;
    let execution = crate::run_uncached_admitted(&action, root, &cache, false, None, lease);
    clock::timeout_after_ready(root, execution).await;
    let id = std::fs::read_to_string(root.join("container-id")).unwrap();
    let inspected = fixture::original(
        root,
        &[
            "inspect",
            "--format",
            "{{.State.Running}} {{.HostConfig.Memory}}",
            id.trim(),
        ],
    )
    .await;
    assert!(inspected.status.success());
    assert_eq!(
        String::from_utf8_lossy(&inspected.stdout).trim(),
        "true 67108864"
    );
    let full_host = ResourceRequest::new(2, 128 * 1024 * 1024);
    let slot_only = ResourceRequest::new(1, 1).with_slot("exclusive", 1);
    let (memory, slot) = tokio::join!(
        tokio::time::timeout(
            Duration::from_secs(5),
            host.acquire(Executor::Local, &full_host)
        ),
        tokio::time::timeout(
            Duration::from_secs(5),
            host.acquire(Executor::Local, &slot_only)
        ),
    );
    assert!(memory.is_err(), "live container lost its host reservation");
    assert!(slot.is_err(), "live container lost its exclusive slot");
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("config/config.json")).unwrap()).unwrap();
    let expected = if root.join("switch").exists() {
        "empty"
    } else {
        "original"
    };
    assert_eq!(config["currentContext"], expected);
    assert!(
        std::fs::read_to_string(root.join("listings"))
            .unwrap()
            .lines()
            .count()
            > 0,
        "retained recovery never performed a lookup"
    );
    std::fs::write(root.join("allow-delete"), b"").unwrap();
    let readmit = full_host.with_slot("exclusive", 1);
    let _readmitted = tokio::time::timeout(
        Duration::from_secs(75),
        host.acquire(Executor::Local, &readmit),
    )
    .await
    .unwrap();
    let remaining = fixture::original(
        root,
        &[
            "ps",
            "--all",
            "--filter",
            &format!("id={}", id.trim()),
            "--format",
            "{{.ID}}",
        ],
    )
    .await;
    assert!(remaining.status.success());
    assert!(
        remaining.stdout.is_empty(),
        "admission released before real container deletion"
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        while !Journal::new(root).scan(100).unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

use std::collections::BTreeMap;

use struo_cas::{CacheProvider, Cas};
use tempfile::TempDir;

use super::{execute_command, Invocation, Redirect};
use crate::{Error, NetworkPolicy};

fn setup() -> (TempDir, CacheProvider) {
    let workspace = TempDir::new().unwrap();
    let cache = CacheProvider::Local(Cas::open(workspace.path().join(".struo/cas")));
    (workspace, cache)
}

fn shell(script: &str) -> Vec<String> {
    vec!["/bin/sh".into(), "-c".into(), script.into()]
}

fn invocation<'a>(
    argv: &'a [String],
    env: &'a BTreeMap<String, String>,
    timeout_ms: Option<u64>,
    network: NetworkPolicy,
) -> Invocation<'a> {
    Invocation {
        argv,
        env,
        cwd: None,
        timeout_ms,
        redirect: Redirect::default(),
        network,
    }
}

#[cfg(unix)]
mod processes {
    use std::path::Path;
    use std::time::{Duration, Instant};

    use super::super::supervise::DRAIN_GRACE;
    use super::*;

    fn alive(pid: &str) -> bool {
        let output = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", pid])
            .output()
            .unwrap();
        let stat = String::from_utf8_lossy(&output.stdout);
        let stat = stat.trim();
        !stat.is_empty() && !stat.starts_with('Z')
    }

    async fn wait_for_exit(pid: &str) -> bool {
        for _ in 0..100 {
            if !alive(pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    fn kill(pid: &str) {
        let _ = std::process::Command::new("kill")
            .args(["-9", pid])
            .status();
    }

    async fn read_pids(path: &Path, count: usize) -> Vec<String> {
        for _ in 0..200 {
            if let Ok(contents) = std::fs::read_to_string(path) {
                let pids = contents
                    .split_whitespace()
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                if pids.len() >= count {
                    return pids;
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("{} never listed {count} pids", path.display());
    }

    async fn assert_all_exit(pids: &[String]) {
        for pid in pids {
            let exited = wait_for_exit(pid).await;
            if !exited {
                kill(pid);
            }
            assert!(exited, "process {pid} outlived its action");
        }
    }

    #[tokio::test]
    async fn timeout_kills_descendants_that_hold_the_output_pipes() {
        let (workspace, cache) = setup();
        let pids = workspace.path().join("pids");
        let argv = shell(&format!(
            "sleep 30 & echo $! >> {pids}; (sleep 30; true) & echo $! >> {pids}; echo $$ >> {pids}; wait",
            pids = pids.display()
        ));
        let env = BTreeMap::new();
        let started = Instant::now();
        let result = execute_command(
            invocation(&argv, &env, Some(300), NetworkPolicy::Unrestricted),
            workspace.path(),
            &cache,
        )
        .await;
        assert!(matches!(result, Err(Error::Timeout(_))), "{result:?}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "timeout waited on inherited pipes: {:?}",
            started.elapsed()
        );
        assert_all_exit(&read_pids(&pids, 3).await).await;
    }

    #[tokio::test]
    async fn cancellation_kills_the_whole_process_group() {
        let (workspace, cache) = setup();
        let pids = workspace.path().join("pids");
        let argv = shell(&format!(
            "sleep 30 & echo $! >> {pids}; echo $$ >> {pids}; wait",
            pids = pids.display()
        ));
        let env = BTreeMap::new();
        let run = execute_command(
            invocation(&argv, &env, None, NetworkPolicy::Unrestricted),
            workspace.path(),
            &cache,
        );
        let pid_file = pids.clone();
        let observed = tokio::select! {
            result = run => panic!("the action finished on its own: {result:?}"),
            pids = async move {
                let pids = read_pids(&pid_file, 2).await;
                let leader: u32 = pids[1].parse().unwrap();
                assert!(super::super::process_group::registered(leader));
                pids
            } => pids,
        };
        assert_all_exit(&observed).await;
        let leader: u32 = observed[1].parse().unwrap();
        assert!(!super::super::process_group::registered(leader));
    }

    #[tokio::test]
    async fn exited_leader_does_not_wait_forever_on_a_background_holder() {
        let (workspace, cache) = setup();
        let pids = workspace.path().join("pids");
        let argv = shell(&format!(
            "echo started; sleep 30 & echo $! >> {pids}; exit 0",
            pids = pids.display()
        ));
        let env = BTreeMap::new();
        let started = Instant::now();
        let result = execute_command(
            invocation(&argv, &env, None, NetworkPolicy::Unrestricted),
            workspace.path(),
            &cache,
        )
        .await
        .unwrap();
        assert!(
            started.elapsed() < DRAIN_GRACE * 3,
            "drain was not bounded: {:?}",
            started.elapsed()
        );
        assert_eq!(result.exit_code, 0);
        let stdout = cache.get_blob(&result.stdout.unwrap()).await.unwrap();
        assert_eq!(stdout, b"started\n");
        assert_all_exit(&read_pids(&pids, 1).await).await;
    }

    #[tokio::test]
    async fn the_leader_stays_unreaped_while_its_group_holds_the_output() {
        let (workspace, cache) = setup();
        let pids = workspace.path().join("pids");
        let argv = shell(&format!(
            "echo $$ >> {pids}; sleep 1 & exit 0",
            pids = pids.display()
        ));
        let env = BTreeMap::new();
        let run = execute_command(
            invocation(&argv, &env, None, NetworkPolicy::Unrestricted),
            workspace.path(),
            &cache,
        );
        let observe = async {
            let leader = read_pids(&pids, 1).await.remove(0);
            for _ in 0..40 {
                let stat = std::process::Command::new("ps")
                    .args(["-o", "stat=", "-p", &leader])
                    .output()
                    .unwrap();
                if String::from_utf8_lossy(&stat.stdout)
                    .trim()
                    .starts_with('Z')
                {
                    let pgid: u32 = leader.parse().unwrap();
                    assert!(super::super::process_group::registered(pgid));
                    return leader;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            panic!("leader {leader} was reaped while its group still held the output");
        };
        let (result, leader) = tokio::join!(run, observe);
        assert_eq!(result.unwrap().exit_code, 0);
        assert!(!alive(&leader));
        assert!(!super::super::process_group::registered(
            leader.parse().unwrap()
        ));
    }

    #[tokio::test]
    async fn background_processes_that_release_the_output_die_with_the_action() {
        let (workspace, cache) = setup();
        let argv = shell("sleep 30 >/dev/null 2>&1 & echo $!");
        let env = BTreeMap::new();
        let started = Instant::now();
        let result = execute_command(
            invocation(&argv, &env, None, NetworkPolicy::Unrestricted),
            workspace.path(),
            &cache,
        )
        .await
        .unwrap();
        assert!(started.elapsed() < DRAIN_GRACE);
        let stdout = cache.get_blob(&result.stdout.unwrap()).await.unwrap();
        let pid = String::from_utf8(stdout).unwrap();
        let pid = pid.trim();
        assert_all_exit(&[pid.to_string()]).await;
    }

    const FORWARD_MODE: &str = "STRUO_LOCAL_FORWARD_TEST_CHILD";

    /// Forwards a signal to every live action group. Runs in a re-executed
    /// test binary because it signals every group this process started.
    #[test]
    #[ignore = "run by forwarded_signals_reach_the_action_and_its_descendants"]
    fn forward_child() {
        let Ok(workspace) = std::env::var(FORWARD_MODE) else {
            return;
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let workspace = Path::new(&workspace);
            let cache = CacheProvider::Local(Cas::open(workspace.join(".struo/cas")));
            let pids = workspace.join("pids");
            let argv = shell(&format!(
                "sleep 30 & echo $! >> {pids}; echo $$ >> {pids}; wait",
                pids = pids.display()
            ));
            let env = BTreeMap::new();
            let run = execute_command(
                invocation(&argv, &env, None, NetworkPolicy::Unrestricted),
                workspace,
                &cache,
            );
            let forward = async {
                read_pids(&pids, 2).await;
                super::super::process_group::signal_action_process_groups(libc::SIGTERM);
                std::future::pending::<()>().await;
            };
            let result = tokio::select! {
                result = run => result.unwrap(),
                () = forward => unreachable!(),
            };
            println!("exit_code={}", result.exit_code);
        });
    }

    #[tokio::test]
    async fn forwarded_signals_reach_the_action_and_its_descendants() {
        let workspace = TempDir::new().unwrap();
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "local::tests::processes::forward_child",
                "--ignored",
                "--nocapture",
            ])
            .env(FORWARD_MODE, workspace.path())
            .output()
            .await
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{stdout}");
        assert!(stdout.contains("exit_code=-1"), "{stdout}");
        assert_all_exit(&read_pids(&workspace.path().join("pids"), 2).await).await;
    }

    #[tokio::test]
    async fn completed_actions_keep_their_exit_code_and_output() {
        let (workspace, cache) = setup();
        let argv = shell("printf out; printf err >&2; exit 3");
        let env = BTreeMap::new();
        let result = execute_command(
            invocation(&argv, &env, Some(10_000), NetworkPolicy::Unrestricted),
            workspace.path(),
            &cache,
        )
        .await
        .unwrap();
        assert_eq!(result.exit_code, 3);
        assert_eq!(
            cache.get_blob(&result.stdout.unwrap()).await.unwrap(),
            b"out"
        );
        assert_eq!(
            cache.get_blob(&result.stderr.unwrap()).await.unwrap(),
            b"err"
        );
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod network {
    use std::time::Duration;

    use super::*;

    const CHILD_MODE: &str = "STRUO_LOCAL_NETWORK_TEST_CHILD";

    /// Probes network access from inside an action that declared `deny`.
    #[test]
    #[ignore = "run by the tests below as a network-denied action"]
    fn child() {
        use std::io::{Read, Write};
        use std::os::unix::net::{UnixListener, UnixStream};

        if std::env::var(CHILD_MODE).is_err() {
            return;
        }
        let report = |name: &str, result: std::io::Result<()>| match result {
            Ok(()) => println!("{name}=ok"),
            Err(error) => println!("{name}=denied:{:?}", error.kind()),
        };
        let port = std::env::var("STRUO_TEST_PORT").unwrap();
        report(
            "loopback_connect",
            std::net::TcpStream::connect_timeout(
                &format!("127.0.0.1:{port}").parse().unwrap(),
                Duration::from_secs(2),
            )
            .map(drop),
        );
        report(
            "loopback_bind",
            std::net::TcpListener::bind("127.0.0.1:0").map(drop),
        );
        report(
            "udp_send",
            std::net::UdpSocket::bind("127.0.0.1:0")
                .and_then(|socket| socket.send_to(b"x", format!("127.0.0.1:{port}")))
                .map(drop),
        );
        report(
            "unix_stream",
            (|| {
                let dir = tempfile::tempdir()?;
                let path = dir.path().join("socket");
                let listener = UnixListener::bind(&path)?;
                let mut client = UnixStream::connect(&path)?;
                let (mut server, _) = listener.accept()?;
                client.write_all(b"hi")?;
                let mut buffer = [0; 2];
                server.read_exact(&mut buffer)?;
                Ok(())
            })(),
        );
        report("unix_pair", UnixStream::pair().map(drop));
    }

    async fn probe(network: NetworkPolicy) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (workspace, cache) = setup();
        let argv = vec![
            std::env::current_exe().unwrap().display().to_string(),
            "--exact".into(),
            "local::tests::network::child".into(),
            "--ignored".into(),
            "--nocapture".into(),
            "--test-threads=1".into(),
        ];
        let env = BTreeMap::from([
            (CHILD_MODE.to_string(), "1".to_string()),
            ("STRUO_TEST_PORT".to_string(), port.to_string()),
            ("TMPDIR".to_string(), "/tmp".to_string()),
        ]);
        let result = execute_command(
            invocation(&argv, &env, Some(60_000), network),
            workspace.path(),
            &cache,
        )
        .await
        .unwrap();
        drop(listener);
        let stdout = cache.get_blob(&result.stdout.unwrap()).await.unwrap();
        String::from_utf8(stdout).unwrap()
    }

    fn line<'a>(stdout: &'a str, name: &str) -> &'a str {
        stdout
            .lines()
            .find_map(|line| line.split_once(&format!("{name}=")).map(|(_, value)| value))
            .unwrap_or_else(|| panic!("probe {name} missing from {stdout}"))
    }

    #[tokio::test]
    async fn deny_blocks_loopback_and_keeps_unix_ipc() {
        let stdout = probe(NetworkPolicy::Deny).await;
        for name in ["loopback_connect", "loopback_bind", "udp_send"] {
            assert!(
                line(&stdout, name).starts_with("denied"),
                "{name} reached the network: {stdout}"
            );
        }
        for name in ["unix_stream", "unix_pair"] {
            assert_eq!(line(&stdout, name), "ok", "{name}: {stdout}");
        }
    }

    #[tokio::test]
    async fn unrestricted_actions_keep_loopback() {
        let stdout = probe(NetworkPolicy::Unrestricted).await;
        for name in [
            "loopback_connect",
            "loopback_bind",
            "unix_stream",
            "unix_pair",
        ] {
            assert_eq!(line(&stdout, name), "ok", "{name}: {stdout}");
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[tokio::test]
async fn deny_fails_closed_without_an_enforcement_mechanism() {
    let (workspace, cache) = setup();
    let argv = shell("exit 0");
    let env = BTreeMap::new();
    let result = execute_command(
        invocation(&argv, &env, None, NetworkPolicy::Deny),
        workspace.path(),
        &cache,
    )
    .await;
    assert!(
        matches!(result, Err(Error::UnsupportedExecutorRequirement { .. })),
        "{result:?}"
    );
}

use std::os::unix::process::CommandExt as _;
use std::process::{Child, Command};
use std::sync::mpsc;
use std::time::Duration;

use super::registry::{self, Registration};
use super::GroupChild;

const SETTLE: Duration = Duration::from_millis(200);

/// A real process group to publish, so signal 0 has a target.
fn sleeping_group() -> Child {
    Command::new("sleep")
        .arg("30")
        .process_group(0)
        .spawn()
        .unwrap()
}

#[test]
fn a_signal_waits_for_a_reserved_slot_to_publish_its_group() {
    let mut registration = Registration::reserve().unwrap();
    let (done, finished) = mpsc::channel();
    let walker = std::thread::spawn(move || {
        registry::signal_all(0);
        done.send(()).unwrap();
    });
    assert!(
        finished.recv_timeout(SETTLE).is_err(),
        "the walk skipped a reserved slot"
    );
    let mut leader = sleeping_group();
    registration.assign(Some(leader.id())).unwrap();
    finished.recv_timeout(Duration::from_secs(5)).unwrap();
    walker.join().unwrap();
    drop(registration);
    leader.kill().unwrap();
    leader.wait().unwrap();
}

#[test]
fn unregistering_waits_for_a_signal_in_flight() {
    let mut leader = sleeping_group();
    let mut registration = Registration::reserve().unwrap();
    registration.assign(Some(leader.id())).unwrap();
    let held = registry::hold_busy(registration.slot());
    let (done, finished) = mpsc::channel();
    let unregister = std::thread::spawn(move || {
        drop(registration);
        done.send(()).unwrap();
    });
    assert!(
        finished.recv_timeout(SETTLE).is_err(),
        "the group left the table while a handler could still signal it"
    );
    drop(held);
    finished.recv_timeout(Duration::from_secs(5)).unwrap();
    unregister.join().unwrap();
    assert!(!registry::registered(
        libc::pid_t::try_from(leader.id()).unwrap()
    ));
    leader.kill().unwrap();
    leader.wait().unwrap();
}

#[tokio::test]
async fn dropping_an_exited_child_unregisters_before_tokio_reaps_it() {
    let mut command = tokio::process::Command::new("/bin/sh");
    command.args(["-c", "exit 0"]);
    let mut group = GroupChild::spawn(&mut command).unwrap();
    let leader = group.child().id().unwrap();
    group.exited().await.unwrap();
    assert!(super::registered(leader));
    drop(group);
    assert!(!super::registered(leader));
}

#[tokio::test]
async fn spawned_actions_start_with_the_caller_signal_mask() {
    let mut command = tokio::process::Command::new("/bin/sh");
    command.args(["-c", "kill -TERM $$; sleep 5"]);
    let mut group = GroupChild::spawn(&mut command).unwrap();
    let status = tokio::time::timeout(Duration::from_secs(3), async {
        group.exited().await.unwrap();
        group.reap().await.unwrap()
    })
    .await
    .expect("the action ignored SIGTERM because spawn left it held");
    assert_eq!(
        std::os::unix::process::ExitStatusExt::signal(&status),
        Some(libc::SIGTERM)
    );
}

#[tokio::test]
async fn cancelling_a_reap_of_a_live_leader_still_kills_the_whole_group() {
    use tokio::io::AsyncBufReadExt as _;

    let mut command = tokio::process::Command::new("/bin/sh");
    command
        .args(["-c", "sleep 60 & echo $!; wait"])
        .stdout(std::process::Stdio::piped());
    let mut group = GroupChild::spawn(&mut command).unwrap();
    let leader = group.child().id().unwrap();
    let mut lines = tokio::io::BufReader::new(group.take_stdout().unwrap()).lines();
    let background = lines.next_line().await.unwrap().unwrap();

    let reaped = tokio::time::timeout(SETTLE, group.reap()).await;
    assert!(reaped.is_err(), "reap returned while the leader was alive");
    assert!(
        super::registered(leader),
        "reap unregistered the group before the leader exited"
    );

    drop(group);
    assert!(!super::registered(leader));
    let exited = wait_for_exit(&background).await;
    if !exited {
        let _ = Command::new("kill").args(["-9", &background]).status();
    }
    assert!(
        exited,
        "the background process outlived its cancelled action"
    );
}

#[tokio::test]
async fn reaping_kills_background_processes_the_caller_never_killed() {
    use tokio::io::AsyncBufReadExt as _;

    let mut command = tokio::process::Command::new("/bin/sh");
    command
        .args(["-c", "sleep 60 >/dev/null 2>&1 & echo $!; exit 0"])
        .stdout(std::process::Stdio::piped());
    let mut group = GroupChild::spawn(&mut command).unwrap();
    let mut lines = tokio::io::BufReader::new(group.take_stdout().unwrap()).lines();
    let background = lines.next_line().await.unwrap().unwrap();

    let status = group.reap().await.unwrap();
    assert!(status.success());
    let exited = wait_for_exit(&background).await;
    if !exited {
        let _ = Command::new("kill").args(["-9", &background]).status();
    }
    assert!(exited, "a background process outlived its reaped action");
}

fn alive(pid: &str) -> bool {
    let output = Command::new("ps")
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

const CHILD_MODE: &str = "STRUO_PROCESS_GROUP_TEST_CHILD";

/// Scenarios that latch process-wide state, run in a re-executed test binary.
#[test]
#[ignore = "run by the tests below in a child process"]
fn child() {
    let Ok(mode) = std::env::var(CHILD_MODE) else {
        return;
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let error = match mode.as_str() {
            "terminating" => {
                super::signal_action_process_groups(0);
                spawn_error()
            }
            "full" => {
                let held = (0..registry::SLOTS)
                    .map(|_| Registration::reserve().unwrap())
                    .collect::<Vec<_>>();
                let error = spawn_error();
                drop(held);
                error
            }
            other => panic!("unknown mode {other}"),
        };
        println!("spawn_error={error}");
    });
}

fn spawn_error() -> std::io::Error {
    let mut command = tokio::process::Command::new("/bin/sh");
    command.args(["-c", "exit 0"]);
    match GroupChild::spawn(&mut command) {
        Ok(_) => panic!("spawn succeeded"),
        Err(error) => error,
    }
}

fn run_child(mode: &str) -> String {
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "local::process_group::tests::child",
            "--ignored",
            "--nocapture",
        ])
        .env(CHILD_MODE, mode)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(output.status.success(), "{stdout}");
    stdout
}

#[test]
fn no_action_starts_struo_termination_began() {
    let stdout = run_child("terminating");
    assert!(stdout.contains("termination signal"), "{stdout}");
}

#[test]
fn a_full_table_fails_the_spawn_instead_of_hiding_the_group() {
    let stdout = run_child("full");
    assert!(stdout.contains("admission control"), "{stdout}");
}

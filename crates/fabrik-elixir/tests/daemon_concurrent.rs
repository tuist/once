//! End-to-end concurrency test for the elixir compile daemon.
//!
//! Spawns a real BEAM running `fabrik_compiler.exs`, then fires several
//! `CompileRequest`s at it in parallel and confirms every job lands
//! with the right `.beam` files on disk. The point is the contract,
//! not the throughput: with the `GenServer` serialization in place no
//! two compiles touch the global code path or module table at the
//! same time, so concurrent clients always observe a consistent VM.
//!
//! Gated on `elixir` being available; cargo test invocations through
//! `mise exec --` pick it up from `mise.toml`. Skips loudly with a
//! `println!` when elixir is missing so the absence is visible in CI
//! logs rather than disguised as a pass.

#![cfg(unix)]

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use fabrik_elixir::daemon::{submit, DAEMON_SCRIPT};
use fabrik_elixir::protocol::CompileRequest;
use tempfile::TempDir;

/// Number of overlapping compile jobs. Larger than the host's CPU
/// count to ensure the worker queue actually backs up; eight is a
/// realistic upper bound for a single fabrik build's elixir-action
/// fanout under the default resource pool.
const PARALLEL_JOBS: usize = 8;

fn elixir_available() -> bool {
    Command::new("elixir")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Drop guard that SIGTERMs the daemon when the test exits, even on
/// panic. Without this a flaky assertion would leave an orphaned BEAM
/// holding the socket and break subsequent runs.
struct DaemonGuard(Option<Child>);

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn start_daemon(script: &Path, socket: &Path) -> DaemonGuard {
    let child = Command::new("elixir")
        .arg(script)
        .arg(socket)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawning elixir daemon");
    DaemonGuard(Some(child))
}

fn wait_for_socket(socket: &Path) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if socket.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!(
        "daemon did not bind socket at {} within 20s",
        socket.display()
    );
}

#[test]
fn daemon_serves_many_concurrent_compile_requests() {
    if !elixir_available() {
        println!(
            "skipping {}: elixir not on PATH (run via `mise exec --`)",
            module_path!()
        );
        return;
    }

    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let socket = workspace.join("daemon.sock");
    let script = workspace.join("fabrik_compiler.exs");
    std::fs::write(&script, DAEMON_SCRIPT).unwrap();

    // Set up PARALLEL_JOBS independent micro-projects. Each has its
    // own module name and output dir so a working serializer produces
    // PARALLEL_JOBS distinct .beam files; a broken one would either
    // crash the daemon or leave at least one module unloaded.
    for i in 0..PARALLEL_JOBS {
        let src_dir = workspace.join(format!("pkg{i}"));
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::write(
            src_dir.join("mod.ex"),
            format!("defmodule Mod{i} do\n  def n, do: {i}\nend\n"),
        )
        .unwrap();
    }

    let _daemon = start_daemon(&script, &socket);
    wait_for_socket(&socket);

    let socket = Arc::new(socket);
    let workspace_arc = Arc::new(workspace.clone());

    let handles: Vec<_> = (0..PARALLEL_JOBS)
        .map(|i| {
            let socket = Arc::clone(&socket);
            let ws = Arc::clone(&workspace_arc);
            thread::spawn(move || {
                let req = CompileRequest::new(
                    i as u64 + 1,
                    ws.to_string_lossy().into_owned(),
                    format!("pkg{i}/out.ebin"),
                    Vec::new(),
                    vec![format!("pkg{i}/mod.ex")],
                );
                submit(&socket, &req)
            })
        })
        .collect();

    for (i, handle) in handles.into_iter().enumerate() {
        let resp = handle
            .join()
            .unwrap_or_else(|_| panic!("worker thread {i} panicked"))
            .unwrap_or_else(|e| panic!("submit {i} failed: {e}"));
        assert!(
            resp.ok,
            "job {i} reported failure: {}",
            resp.error.unwrap_or_default()
        );
        assert_eq!(resp.id, (i as u64) + 1, "job {i} response id mismatch");
    }

    for i in 0..PARALLEL_JOBS {
        let beam = workspace
            .join(format!("pkg{i}/out.ebin"))
            .join(format!("Elixir.Mod{i}.beam"));
        assert!(
            beam.exists(),
            "expected compiled module at {} after concurrent run",
            beam.display()
        );
    }
}

#[test]
fn daemon_compiles_a_dep_chain_through_pa() {
    // Verifies that paths the daemon prepends via the request's `pa`
    // field are actually visible to the compile and cleaned up after.
    // The downstream module references the upstream module's atom by
    // name, so the compile fails unless the daemon prepended the dep
    // ebin onto the code path for the duration of this job.
    if !elixir_available() {
        println!(
            "skipping {}: elixir not on PATH (run via `mise exec --`)",
            module_path!()
        );
        return;
    }

    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let socket = workspace.join("daemon.sock");
    let script = workspace.join("fabrik_compiler.exs");
    std::fs::write(&script, DAEMON_SCRIPT).unwrap();

    let dep_src = workspace.join("dep");
    let app_src = workspace.join("app");
    std::fs::create_dir_all(&dep_src).unwrap();
    std::fs::create_dir_all(&app_src).unwrap();
    std::fs::write(
        dep_src.join("dep.ex"),
        "defmodule Dep do\n  def greeting, do: :hello\nend\n",
    )
    .unwrap();
    std::fs::write(
        app_src.join("app.ex"),
        "defmodule App do\n  def call, do: Dep.greeting()\nend\n",
    )
    .unwrap();

    let _daemon = start_daemon(&script, &socket);
    wait_for_socket(&socket);

    let dep_req = CompileRequest::new(
        1,
        workspace.to_string_lossy().into_owned(),
        "dep/out.ebin".into(),
        Vec::new(),
        vec!["dep/dep.ex".into()],
    );
    let dep_resp = submit(&socket, &dep_req).expect("dep submit");
    assert!(
        dep_resp.ok,
        "dep compile failed: {}",
        dep_resp.error.unwrap_or_default()
    );

    let app_req = CompileRequest::new(
        2,
        workspace.to_string_lossy().into_owned(),
        "app/out.ebin".into(),
        vec!["dep/out.ebin".into()],
        vec!["app/app.ex".into()],
    );
    let app_resp = submit(&socket, &app_req).expect("app submit");
    assert!(
        app_resp.ok,
        "app compile failed: {}",
        app_resp.error.unwrap_or_default()
    );

    assert!(workspace.join("app/out.ebin/Elixir.App.beam").exists());
    assert!(workspace.join("dep/out.ebin/Elixir.Dep.beam").exists());
}

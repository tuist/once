#![cfg(unix)]

use std::process::{Command, Output};

use tonic::Code;

#[path = "support/cache_failure.rs"]
mod cache_failure;

const MODULE: &str = r#"
def _impl(ctx):
    run_action(
        argv = ["/bin/sh", "-c", "printf correct > output.txt; printf run >> runs.log"],
        outputs = ["output.txt"],
        identifier = "produce",
    )
    return {}

def _repair_impl(ctx):
    _impl(ctx)
    run_action(
        argv = ["/bin/rm", "-rf", "CAS_BLOBS"],
        clean_paths = ["output.txt"],
        cacheable = False,
        identifier = "evict",
    )
    run_action(
        argv = ["/bin/cp", "output.txt", "consumed.txt"],
        inputs = ["output.txt"],
        outputs = ["consumed.txt"],
        identifier = "consume",
    )
    return {}

repair_outage_test = target_kind(
    capabilities = [capability("build", ["default"])],
    impl = _repair_impl,
)

outage_test = target_kind(
    capabilities = [capability("build", ["default"])],
    impl = _impl,
)
"#;

struct Workspace {
    root: tempfile::TempDir,
    home: tempfile::TempDir,
    endpoint: String,
}

impl Workspace {
    async fn new(code: Code) -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let blobs = home.path().join("cache/struo/cas/cas");
        std::fs::write(
            root.path().join("kinds.star"),
            MODULE.replace("CAS_BLOBS", blobs.to_str().unwrap()),
        )
        .unwrap();
        std::fs::write(
            root.path().join("struo.toml"),
            "[modules]\npaths = [\"kinds.star\"]\n\n[[target]]\nname = \"app\"\nkind = \"outage_test\"\n\n[infrastructure.cache]\nprovider = \"tuist\"\n\n[infrastructures.tuist]\nkind = \"tuist\"\nurl = \"http://127.0.0.1:9\"\naccount = \"acme\"\nproject = \"app\"\n",
        )
        .unwrap();
        Self {
            root,
            home,
            endpoint: cache_failure::endpoint(code).await,
        }
    }

    async fn run(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_struo"));
        command
            .args(args)
            .current_dir(self.root.path())
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.home.path())
            .env("XDG_CACHE_HOME", self.home.path().join("cache"))
            .env("XDG_CONFIG_HOME", self.home.path().join("config"))
            .env("XDG_DATA_HOME", self.home.path().join("data"))
            .env("XDG_STATE_HOME", self.home.path().join("state"))
            .env("STRUO_CHANGE_TRACKER", "0")
            .env("TUIST_TOKEN", "test-token")
            .env("TUIST_CACHE_ENDPOINT", &self.endpoint);
        tokio::task::spawn_blocking(move || command.output().unwrap())
            .await
            .unwrap()
    }

    fn assert_executed(&self, output: &Output) {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            std::fs::read(self.root.path().join("output.txt")).unwrap(),
            b"correct"
        );
        assert_eq!(
            std::fs::read_to_string(self.root.path().join("runs.log")).unwrap(),
            "run"
        );
    }
}

#[tokio::test]
async fn explicit_cache_reads_still_report_outages() {
    let workspace = Workspace::new(Code::Unavailable).await;
    let digest = struo_cas::Digest::of_bytes(b"explicit cache read").to_hex();
    let output = workspace
        .run(&["--format", "json", "cache", "action", "get", &digest])
        .await;
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("get action result"), "{stderr}");
    assert!(!workspace.root.path().join("runs.log").exists());
}

#[tokio::test]
async fn exec_recomputes_during_remote_cache_outages_and_reuses_local_results() {
    for code in [Code::Unavailable, Code::Internal] {
        let workspace = Workspace::new(code).await;
        let args = [
            "exec",
            "--",
            "/bin/sh",
            "-c",
            "printf correct > output.txt; printf run >> runs.log",
        ];
        let first = workspace.run(&args).await;
        workspace.assert_executed(&first);
        let second = workspace.run(&args).await;
        workspace.assert_executed(&second);
    }
}

#[tokio::test]
async fn graph_recomputes_during_remote_cache_outages_and_reuses_local_results() {
    for code in [Code::Unavailable, Code::Internal] {
        let workspace = Workspace::new(code).await;
        let first = workspace.run(&["build", "app"]).await;
        workspace.assert_executed(&first);
        let second = workspace.run(&["build", "app"]).await;
        workspace.assert_executed(&second);
    }
}

#[tokio::test]
async fn a_deferred_output_is_recomputed_during_a_cache_outage() {
    let workspace = Workspace::new(Code::Unavailable).await;
    let manifest = workspace.root.path().join("struo.toml");
    let contents = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(
        &manifest,
        contents.replace("kind = \"outage_test\"", "kind = \"repair_outage_test\""),
    )
    .unwrap();
    let output = workspace.run(&["build", "app"]).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read(workspace.root.path().join("consumed.txt")).unwrap(),
        b"correct"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.root.path().join("runs.log")).unwrap(),
        "runrun"
    );
}

#[tokio::test]
async fn cache_refusals_and_corruption_do_not_execute_actions() {
    for code in [
        Code::PermissionDenied,
        Code::Unauthenticated,
        Code::DataLoss,
        Code::InvalidArgument,
    ] {
        for args in [
            vec!["build", "app"],
            vec![
                "exec",
                "--",
                "/bin/sh",
                "-c",
                "printf correct > output.txt; printf run >> runs.log",
            ],
        ] {
            let workspace = Workspace::new(code).await;
            let output = workspace.run(&args).await;
            assert!(!output.status.success(), "{code:?}: unexpectedly succeeded");
            assert!(
                !workspace.root.path().join("output.txt").exists(),
                "{code:?}: executed despite cache error"
            );
        }
    }
}

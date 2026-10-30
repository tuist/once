//! A build that reuses the persistent cache after the process exits must put
//! back what it needs without rerunning work the cache still answers for,
//! and must recover an output whose bytes vanished after the cache probe
//! said it could be reused. Spawns the real binary against an isolated cache.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MODULE: &str = r#"
def _two_step_impl(ctx):
    out = ctx["build_dir"]
    run_action(
        argv = ["/bin/sh", "-c", "printf object > " + out + "/main.o; printf compile >> runs.log"],
        outputs = [out + "/main.o"],
        create_dirs = [out],
        identifier = "compile",
        intermediate = True,
    )
    run_action(
        argv = ["/bin/sh", "-c", "cat " + out + "/main.o > " + out + "/app; printf link >> runs.log"],
        inputs = [out + "/main.o"],
        outputs = [out + "/app"],
        identifier = "link",
    )
    return {}

def _recovering_impl(ctx):
    out = ctx["build_dir"]
    run_action(
        argv = ["/bin/sh", "-c", "printf first > " + out + "/produced.txt; printf produce >> runs.log"],
        outputs = [out + "/produced.txt"],
        create_dirs = [out],
        identifier = "produce",
    )
    run_action(
        argv = ["/bin/sh", "-c", "printf cleaned > " + out + "/cleaned.txt"],
        outputs = [out + "/cleaned.txt"],
        clean_paths = [out + "/produced.txt"],
        cacheable = False,
        identifier = "clean",
    )
    run_action(
        argv = ["/bin/sh", "-c", "cat " + out + "/produced.txt > " + out + "/consumed.txt"],
        inputs = [out + "/produced.txt"],
        outputs = [out + "/consumed.txt"],
        cacheable = False,
        identifier = "consume",
    )
    return {}

two_step = target_kind(
    capabilities = [capability("build", ["default"])],
    impl = _two_step_impl,
)

recovering = target_kind(
    capabilities = [capability("build", ["default"])],
    impl = _recovering_impl,
)

def _receipt_root_impl(ctx):
    out = declare_output("app")
    run_action(
        argv = ["/bin/sh", "-c", "printf app > \"$1\"", "sh", out],
        outputs = [out],
    )
    return {}

def _mandatory_check_impl(ctx):
    run_action(
        argv = ["/bin/sh", "-c", "printf check >> \"$1\"", "sh", host_env("HOME") + "/checks"],
        cacheable = ctx["attr"]["cacheable"],
    )
    return {}

receipt_root = target_kind(
    deps = [dep("plugins", ["check"])],
    capabilities = [capability("build", ["default"])],
    impl = _receipt_root_impl,
)

mandatory_check = target_kind(
    attrs = [attr("cacheable", "bool", required = True)],
    providers = ["check"],
    capabilities = [capability("build", ["default"])],
    impl = _mandatory_check_impl,
)
"#;

struct Workspace {
    root: tempfile::TempDir,
    home: tempfile::TempDir,
}

impl Workspace {
    fn new(kind: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let modules = root.path().join("modules");
        std::fs::create_dir_all(&modules).unwrap();
        std::fs::write(modules.join("kinds.star"), MODULE).unwrap();
        std::fs::write(
            root.path().join("once.toml"),
            format!(
                "[modules]\npaths = [\"modules/*.star\"]\n\n[[target]]\nname = \"app\"\nkind = \"{kind}\"\n"
            ),
        )
        .unwrap();
        Self {
            root,
            home: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self) -> &Path {
        self.root.path()
    }

    fn build(&self) -> Output {
        self.command().arg("build").arg("app").output().unwrap()
    }

    fn command(&self) -> Command {
        let home = self.home.path();
        let mut command = Command::new(env!("CARGO_BIN_EXE_once"));
        command
            .arg("-C")
            .arg(self.path())
            .arg("--format")
            .arg("json")
            .env("HOME", home)
            .env("XDG_CACHE_HOME", home.join("cache"))
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_DATA_HOME", home.join("data"))
            .env("XDG_STATE_HOME", home.join("state"))
            .env("ONCE_CHANGE_TRACKER", "0")
            .env("ONCE_CACHE_PROVIDER", "local")
            .env_remove("TUIST_TOKEN");
        command
    }

    fn build_succeeds(&self) -> serde_json::Value {
        let output = self.build();
        assert!(
            output.status.success(),
            "build failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "build record did not decode: {error}; stdout: {}",
                String::from_utf8_lossy(&output.stdout)
            )
        })
    }

    fn runs(&self) -> String {
        std::fs::read_to_string(self.path().join("runs.log")).unwrap_or_default()
    }

    fn tracked_build(&self) -> Output {
        let output = self
            .command()
            .env("ONCE_CHANGE_TRACKER", "1")
            .env("RUST_LOG", "once::commands::graph=trace")
            .args(["build", "app"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "tracked build failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn output(&self, name: &str) -> PathBuf {
        self.path().join(".once/out/app").join(name)
    }

    /// Where the cache keeps the blob an action result names for `output`.
    fn cached_output_blob(&self, output: &str) -> PathBuf {
        let cache = self.home.path().join("cache/once/cas");
        let digest = walk(&cache.join("actions"))
            .into_iter()
            .find_map(|path| {
                let result: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
                result["outputs"][output].as_str().map(str::to_string)
            })
            .unwrap_or_else(|| panic!("no cached action result names `{output}`"));
        let (prefix, rest) = digest.split_at(2);
        cache.join("cas").join(prefix).join(rest)
    }
}

fn walk(directory: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(directory).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}

#[test]
fn a_restarted_build_restores_removed_outputs_without_rerunning_actions() {
    let workspace = Workspace::new("two_step");
    workspace.build_succeeds();
    assert_eq!(workspace.runs(), "compilelink");

    std::fs::remove_dir_all(workspace.path().join(".once/out")).unwrap();
    let record = workspace.build_succeeds();

    assert_eq!(workspace.runs(), "compilelink", "no action ran again");
    assert_eq!(record["cache"], "hit");
    assert_eq!(std::fs::read(workspace.output("app")).unwrap(), b"object");
    assert!(
        !workspace.output("main.o").exists(),
        "the intermediate stays in the cache when nothing needs it on disk"
    );
}

#[test]
fn an_action_cache_build_reruns_after_gc_evicts_its_intermediate() {
    let workspace = Workspace::new("two_step");
    workspace.build_succeeds();
    assert_eq!(workspace.runs(), "compilelink");
    let output = workspace
        .command()
        .args(["cache", "gc", "--max-size", "0"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "GC failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_file(workspace.output("main.o")).unwrap();
    workspace.build_succeeds();
    assert_eq!(workspace.runs(), "compilelinkcompilelink");
    assert_eq!(std::fs::read(workspace.output("app")).unwrap(), b"object");
    workspace.build_succeeds();
    assert_eq!(workspace.runs(), "compilelinkcompilelink");
}

#[test]
fn an_output_replaced_after_its_producer_ran_is_restored_for_its_reader() {
    let workspace = Workspace::new("recovering");

    workspace.build_succeeds();

    assert_eq!(workspace.runs(), "produce");
    assert_eq!(
        std::fs::read(workspace.output("consumed.txt")).unwrap(),
        b"first"
    );
}

#[test]
fn an_output_evicted_after_the_cache_probe_reruns_only_its_producer() {
    let workspace = Workspace::new("recovering");
    workspace.build_succeeds();
    let blob = workspace.cached_output_blob(".once/out/app/produced.txt");
    assert!(blob.exists(), "the produced bytes were cached");
    std::fs::remove_file(&blob).unwrap();

    workspace.build_succeeds();

    assert_eq!(workspace.runs(), "produceproduce");
    assert_eq!(
        std::fs::read(workspace.output("consumed.txt")).unwrap(),
        b"first"
    );
    assert!(blob.exists(), "running the producer cached its bytes again");

    // With the bytes back, the next build needs no repair.
    workspace.build_succeeds();
    assert_eq!(workspace.runs(), "produceproduce");
}

#[test]
fn a_corrupt_cached_output_fails_instead_of_rerunning_its_producer() {
    let workspace = Workspace::new("recovering");
    workspace.build_succeeds();
    std::fs::write(
        workspace.cached_output_blob(".once/out/app/produced.txt"),
        b"not the cached bytes",
    )
    .unwrap();

    let output = workspace.build();

    assert!(!output.status.success());
    assert_eq!(workspace.runs(), "produce", "corruption is not absence");
}

fn receipt_workspace(cacheable: bool) -> Workspace {
    let workspace = Workspace::new("receipt_root");
    std::fs::write(
        workspace.path().join("once.toml"),
        format!(
            r#"[modules]
paths = ["modules/*.star"]

[[target]]
name = "app"
kind = "receipt_root"

[target.dependencies]
plugins = ["./check"]

[[target]]
name = "check"
kind = "mandatory_check"

[target.attrs]
cacheable = {cacheable}
"#
        ),
    )
    .unwrap();
    workspace
}

#[test]
fn tracked_build_receipts_do_not_skip_mandatory_dependency_actions() {
    let workspace = receipt_workspace(false);
    for invocation in 1..=4 {
        let output = workspace.tracked_build();
        assert_eq!(
            std::fs::read_to_string(workspace.home.path().join("checks")).unwrap(),
            "check".repeat(invocation),
            "mandatory dependency action must run on invocation {invocation}"
        );
        if invocation > 1 {
            let record: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(record["cache"], "hit", "the root action stays cached");
        }
    }
}

#[test]
fn tracked_cacheable_builds_still_reuse_whole_build_receipts() {
    let workspace = receipt_workspace(true);
    let mut reused_receipt = false;
    for _ in 0..4 {
        let output = workspace.tracked_build();
        reused_receipt |=
            String::from_utf8_lossy(&output.stderr).contains("reused unchanged build receipt");
    }
    assert_eq!(
        std::fs::read_to_string(workspace.home.path().join("checks")).unwrap(),
        "check"
    );
    assert!(reused_receipt, "the cacheable control must reuse a receipt");
}

#[test]
fn tracked_builds_clear_existing_receipts_when_a_dependency_becomes_uncacheable() {
    let workspace = receipt_workspace(true);
    for _ in 0..4 {
        workspace.tracked_build();
    }
    let receipts = workspace.path().join(".once/build-receipts");
    assert!(!walk(&receipts).is_empty());

    let manifest = workspace.path().join("once.toml");
    let content = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(
        &manifest,
        content.replace("cacheable = true", "cacheable = false"),
    )
    .unwrap();
    for checks in 2..=3 {
        workspace.tracked_build();
        assert_eq!(
            std::fs::read_to_string(workspace.home.path().join("checks")).unwrap(),
            "check".repeat(checks)
        );
        assert!(walk(&receipts).is_empty(), "the stale receipt was cleared");
    }
}

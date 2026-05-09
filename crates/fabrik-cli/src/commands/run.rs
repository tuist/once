//! `fabrik run` - execute the action(s) that produce a target.
//!
//! For a `rust_binary`, that action is the rustc invocation. The verb
//! is the same regardless of target type: target-specific composition
//! lives in the build-file declarations, not in the CLI.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use fabrik_cas::{Cas, Digest};
use fabrik_core::{Action, CacheState, ResourceRequest, RunOpts, Runner, WorkspacePath};
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::cli::{exit_from, Format, CACHE_DIR};
use crate::render;

#[derive(Serialize)]
struct RunRecord<'a> {
    label: &'a str,
    kind: &'a str,
    action_digest: String,
    cache: &'a str,
    exit_code: i32,
    output: String,
}

struct ActionPlan {
    action: Action,
    output: String,
    output_dir: Option<PathBuf>,
}

pub async fn run(workspace: &Path, cas: &Cas, label: &str, format: Format) -> Result<ExitCode> {
    let targets = fabrik_frontend::load_workspace(workspace).context("loading workspace")?;
    let target = targets
        .iter()
        .find(|t| t.label() == label)
        .ok_or_else(|| anyhow::anyhow!("no target matches `{label}`"))?;

    if target.kind == "apple_ios_app" {
        return run_apple_ios_app(workspace, cas, label, &targets, target, format).await;
    }

    let plan = action_for(workspace, target)?;
    if let Some(out_dir) = &plan.output_dir {
        tokio::fs::create_dir_all(out_dir)
            .await
            .with_context(|| format!("creating output directory {}", out_dir.display()))?;
    }

    let outcome = fabrik_core::run(&plan.action, workspace, cas, RunOpts::default())
        .await
        .context("executing action")?;

    render_run_output(cas, &outcome, label, target, &plan.output, format).await?;
    Ok(exit_from(outcome.result.exit_code))
}

async fn run_apple_ios_app(
    workspace: &Path,
    cas: &Cas,
    label: &str,
    targets: &[fabrik_frontend::Target],
    target: &fabrik_frontend::Target,
    format: Format,
) -> Result<ExitCode> {
    let built = fabrik_apple::build_plan(targets, label, workspace).context("building app plan")?;
    let runner = Runner::new(cas.clone(), workspace.to_path_buf(), RunOpts::default());
    let _build_outcomes = runner
        .run_plan(&built.plan)
        .await
        .with_context(|| format!("building app target {label}"))?;
    let launch = fabrik_apple::launch_ios_app(target, workspace)?;
    let outcome = runner
        .run(&launch.action)
        .await
        .with_context(|| format!("launching app target {label}"))?;

    render_run_output(cas, &outcome, label, target, &launch.output, format).await?;
    Ok(exit_from(outcome.result.exit_code))
}

async fn render_run_output(
    cas: &Cas,
    outcome: &fabrik_core::Outcome,
    label: &str,
    target: &fabrik_frontend::Target,
    output: &str,
    format: Format,
) -> Result<()> {
    let stdout_blob = cas.get_blob(&outcome.result.stdout).await?;
    let stderr_blob = cas.get_blob(&outcome.result.stderr).await?;
    let cache_tag = match outcome.cache {
        CacheState::Hit => "hit",
        CacheState::Miss => "miss",
    };
    let record = RunRecord {
        label,
        kind: &target.kind,
        action_digest: outcome.action.to_string(),
        cache: cache_tag,
        exit_code: outcome.result.exit_code,
        output: output.to_string(),
    };

    match format {
        Format::Human => {
            let mut out = tokio::io::stdout();
            out.write_all(&stdout_blob).await?;
            out.flush().await?;
            let mut err = tokio::io::stderr();
            err.write_all(&stderr_blob).await?;
            let trailer = format!(
                "fabrik: ran {label} (cache {cache_tag}, exit={})\n",
                outcome.result.exit_code
            );
            err.write_all(trailer.as_bytes()).await?;
            err.flush().await?;
        }
        Format::Json | Format::Toon => {
            // Subprocess stderr stays on stderr (so e.g. rustc's
            // diagnostics still flow to the terminal); the structured
            // outcome record goes to stdout where agents pick it up.
            let mut err = tokio::io::stderr();
            err.write_all(&stderr_blob).await?;
            err.flush().await?;
            let mut out = tokio::io::stdout();
            out.write_all(render::structured(format, &record)?.as_bytes())
                .await?;
            out.flush().await?;
        }
    }
    Ok(())
}

fn action_for(workspace: &Path, target: &fabrik_frontend::Target) -> Result<ActionPlan> {
    match target.kind.as_str() {
        "rust_binary" => rust_binary_action(workspace, target),
        "cargo_binary" => cargo_binary_action(workspace, target),
        "task" => task_action(workspace, target),
        other => anyhow::bail!("running `{other}` targets is not yet supported"),
    }
}

fn rust_binary_action(workspace: &Path, target: &fabrik_frontend::Target) -> Result<ActionPlan> {
    let main_src = target
        .srcs
        .first()
        .ok_or_else(|| anyhow::anyhow!("rust_binary {} has no srcs", target.label()))?;
    let src_rel = source_path(target, main_src)?;
    let out_rel = if target.package.is_empty() {
        format!("{CACHE_DIR}/out/{}", target.name)
    } else {
        format!("{CACHE_DIR}/out/{}/{}", target.package, target.name)
    };
    let input_digest = input_digest(workspace, target)?;
    let output_dir = workspace.join(CACHE_DIR).join("out").join(&target.package);

    Ok(ActionPlan {
        action: Action::RunCommand {
            argv: vec![
                "rustc".into(),
                "--edition=2021".into(),
                format!("--crate-name={}", target.name),
                "--crate-type=bin".into(),
                "-o".into(),
                out_rel.clone(),
                src_rel.as_str().to_string(),
            ],
            env: tool_env(),
            cwd: None,
            input_digest,
            outputs: vec![],
            resources: ResourceRequest::default(),
            timeout_ms: Some(120_000),
        },
        output: out_rel,
        output_dir: Some(output_dir),
    })
}

fn cargo_binary_action(workspace: &Path, target: &fabrik_frontend::Target) -> Result<ActionPlan> {
    if target.srcs.is_empty() {
        anyhow::bail!("cargo_binary {} has no srcs", target.label());
    }
    let cargo_package = target
        .attrs
        .get("cargo_package")
        .ok_or_else(|| anyhow::anyhow!("cargo_binary {} has no cargo_package", target.label()))?;
    let bin = target.attrs.get("bin").unwrap_or(&target.name);
    let input_digest = input_digest(workspace, target)?;
    let output = format!("target/debug/{bin}{}", std::env::consts::EXE_SUFFIX);

    Ok(ActionPlan {
        action: Action::RunCommand {
            argv: vec![
                "cargo".into(),
                "build".into(),
                "--locked".into(),
                "--package".into(),
                cargo_package.to_string(),
                "--bin".into(),
                bin.to_string(),
            ],
            env: tool_env(),
            cwd: None,
            input_digest,
            outputs: vec![],
            resources: ResourceRequest::new(2, 0),
            timeout_ms: Some(300_000),
        },
        output,
        output_dir: None,
    })
}

fn task_action(workspace: &Path, target: &fabrik_frontend::Target) -> Result<ActionPlan> {
    let argv_json = target
        .attrs
        .get("argv_json")
        .ok_or_else(|| anyhow::anyhow!("task {} has no argv", target.label()))?;
    let argv: Vec<String> = serde_json::from_str(argv_json)
        .with_context(|| format!("parsing argv for task {}", target.label()))?;
    if argv.is_empty() {
        anyhow::bail!("task {} has empty argv", target.label());
    }
    let env = match target.attrs.get("env_json") {
        Some(raw) => serde_json::from_str(raw)
            .with_context(|| format!("parsing env for task {}", target.label()))?,
        None => BTreeMap::new(),
    };
    let cwd = match target.attrs.get("cwd") {
        Some(raw) => Some(
            WorkspacePath::try_from(raw.as_str())
                .with_context(|| format!("invalid cwd for task {}", target.label()))?,
        ),
        None => None,
    };
    let outputs = match target.attrs.get("outputs_json") {
        Some(raw) => {
            let values: Vec<String> = serde_json::from_str(raw)
                .with_context(|| format!("parsing outputs for task {}", target.label()))?;
            values
                .iter()
                .map(|value| {
                    WorkspacePath::try_from(value.as_str())
                        .with_context(|| format!("invalid output `{value}` in {}", target.label()))
                })
                .collect::<Result<_>>()?
        }
        None => Vec::new(),
    };
    let cache = target
        .attrs
        .get("cache")
        .map_or(Ok(true), |raw| raw.parse::<bool>())
        .with_context(|| format!("parsing cache setting for task {}", target.label()))?;
    let timeout_ms = parse_attr::<u64>(target, "timeout_ms")?;
    let cpu_slots = parse_attr::<usize>(target, "cpu_slots")?.unwrap_or(1);
    let memory_bytes = parse_attr::<u64>(target, "memory_bytes")?.unwrap_or(0);
    let input_digest = if cache {
        input_digest(workspace, target)?
    } else {
        Some(uncached_task_digest(target))
    };

    Ok(ActionPlan {
        action: Action::RunCommand {
            argv,
            env,
            cwd,
            input_digest,
            outputs,
            resources: ResourceRequest::new(cpu_slots, memory_bytes),
            timeout_ms,
        },
        output: String::new(),
        output_dir: None,
    })
}

fn source_path(target: &fabrik_frontend::Target, src: &str) -> Result<WorkspacePath> {
    let rel = if target.package.is_empty() {
        src.to_string()
    } else {
        format!("{}/{src}", target.package)
    };
    WorkspacePath::try_from(rel.as_str())
        .with_context(|| format!("invalid source path `{src}` in {}", target.label()))
}

fn parse_attr<T>(target: &fabrik_frontend::Target, name: &str) -> Result<Option<T>>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    target
        .attrs
        .get(name)
        .map(|value| {
            value
                .parse::<T>()
                .with_context(|| format!("parsing {name} for {}", target.label()))
        })
        .transpose()
}

fn uncached_task_digest(target: &fabrik_frontend::Target) -> Digest {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut buf = Vec::new();
    buf.extend_from_slice(b"fabrik.task.uncached.v1\0");
    buf.extend_from_slice(target.label().as_bytes());
    buf.push(0);
    buf.extend_from_slice(&nonce.to_le_bytes());
    Digest::of_bytes(&buf)
}

/// Hash the declared sources of a target into a single digest the
/// action depends on. Two targets that disagree on either the set of
/// source paths or on any source's contents get different digests, so
/// the action cache invalidates correctly.
///
/// Encoding (per source, after sorting by workspace path):
/// `path_bytes || 0x00 || file_digest || 0x00`. Sorting before hashing
/// makes the result stable across iteration orders. The terminator
/// guards against ambiguity even though declared paths never contain
/// NULs in practice.
fn input_digest(workspace: &Path, target: &fabrik_frontend::Target) -> Result<Option<Digest>> {
    if target.srcs.is_empty() {
        return Ok(None);
    }

    let mut paths: Vec<_> = target
        .srcs
        .iter()
        .map(|src| source_path(target, src))
        .collect::<Result<_>>()?;
    paths.sort_by(|a, b| a.as_str().cmp(b.as_str()));

    let mut buf = Vec::new();
    for path in paths {
        let file = std::fs::File::open(path.resolve(workspace))
            .with_context(|| format!("reading source input `{path}`"))?;
        let digest = Digest::of_reader(std::io::BufReader::new(file))
            .with_context(|| format!("hashing source input `{path}`"))?;
        buf.extend_from_slice(path.as_str().as_bytes());
        buf.push(0);
        buf.extend_from_slice(digest.as_bytes());
        buf.push(0);
    }
    Ok(Some(Digest::of_bytes(&buf)))
}

/// Environment variables forwarded verbatim to spawned tool actions
/// (rustc, cargo, ...). Anything not on this list, and not a `MISE_*`
/// variable, is dropped: actions must declare every variable they
/// depend on, or the cache key lies.
const FORWARDED_TOOL_ENV: &[&str] = &["PATH", "HOME", "CARGO_HOME", "RUSTUP_HOME"];

fn tool_env() -> BTreeMap<String, String> {
    select_tool_env(std::env::vars())
}

fn select_tool_env<I>(vars: I) -> BTreeMap<String, String>
where
    I: IntoIterator<Item = (String, String)>,
{
    vars.into_iter()
        .filter(|(k, _)| FORWARDED_TOOL_ENV.contains(&k.as_str()) || k.starts_with("MISE_"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabrik_frontend::Target;
    use tempfile::TempDir;

    fn target(package: &str, kind: &str, srcs: &[&str]) -> Target {
        Target {
            package: package.into(),
            kind: kind.into(),
            name: "demo".into(),
            srcs: srcs.iter().map(|s| (*s).into()).collect(),
            deps: vec![],
            attrs: BTreeMap::new(),
        }
    }

    #[test]
    fn select_tool_env_keeps_allowlisted_keys_and_mise_prefix() {
        let env = select_tool_env([
            ("PATH".into(), "/usr/bin".into()),
            ("CARGO_HOME".into(), "/cargo".into()),
            ("MISE_TRUSTED_CONFIG_PATHS".into(), "/ws".into()),
            ("MISE_YES".into(), "1".into()),
            ("UNRELATED".into(), "leaked".into()),
            ("FABRIK_PROBE".into(), "leaked".into()),
        ]);
        assert_eq!(env.get("PATH").map(String::as_str), Some("/usr/bin"));
        assert_eq!(env.get("CARGO_HOME").map(String::as_str), Some("/cargo"));
        assert_eq!(
            env.get("MISE_TRUSTED_CONFIG_PATHS").map(String::as_str),
            Some("/ws")
        );
        assert_eq!(env.get("MISE_YES").map(String::as_str), Some("1"));
        assert!(!env.contains_key("UNRELATED"));
        assert!(!env.contains_key("FABRIK_PROBE"));
    }

    #[test]
    fn source_path_joins_package_and_rejects_escapes() {
        let t = target("crates/foo", "rust_binary", &["src/main.rs"]);
        assert_eq!(
            source_path(&t, "src/main.rs").unwrap().as_str(),
            "crates/foo/src/main.rs"
        );
        let root = target("", "rust_binary", &["main.rs"]);
        assert_eq!(source_path(&root, "main.rs").unwrap().as_str(), "main.rs");

        // A `..` in the declared src must not let the target escape its
        // own package. The frontend collects whatever the user wrote;
        // path validation lives here.
        let escape = target("pkg", "rust_binary", &["../escape.rs"]);
        assert!(source_path(&escape, "../escape.rs").is_err());
    }

    #[test]
    fn input_digest_is_none_when_no_srcs_are_declared() {
        let tmp = TempDir::new().unwrap();
        let t = target("", "rust_binary", &[]);
        assert!(input_digest(tmp.path(), &t).unwrap().is_none());
    }

    #[test]
    fn input_digest_changes_with_content_and_is_stable_across_runs() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("pkg")).unwrap();
        std::fs::write(tmp.path().join("pkg/main.rs"), b"fn main() {}").unwrap();
        let t = target("pkg", "rust_binary", &["main.rs"]);

        let first = input_digest(tmp.path(), &t).unwrap().unwrap();
        let second = input_digest(tmp.path(), &t).unwrap().unwrap();
        assert_eq!(first, second, "stable for identical content");

        // Edit the file: digest must change.
        std::fs::write(tmp.path().join("pkg/main.rs"), b"fn main() { /*!*/ }").unwrap();
        let third = input_digest(tmp.path(), &t).unwrap().unwrap();
        assert_ne!(first, third, "content change must invalidate the digest");
    }

    #[test]
    fn input_digest_is_independent_of_declared_src_order() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("pkg")).unwrap();
        std::fs::write(tmp.path().join("pkg/a.rs"), b"a").unwrap();
        std::fs::write(tmp.path().join("pkg/b.rs"), b"b").unwrap();
        let forward = target("pkg", "rust_binary", &["a.rs", "b.rs"]);
        let reversed = target("pkg", "rust_binary", &["b.rs", "a.rs"]);
        assert_eq!(
            input_digest(tmp.path(), &forward).unwrap().unwrap(),
            input_digest(tmp.path(), &reversed).unwrap().unwrap()
        );
    }

    #[test]
    fn rust_binary_action_has_expected_argv_and_output() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("pkg")).unwrap();
        std::fs::write(tmp.path().join("pkg/main.rs"), b"fn main() {}").unwrap();
        let t = target("pkg", "rust_binary", &["main.rs"]);
        let plan = rust_binary_action(tmp.path(), &t).unwrap();

        assert_eq!(plan.output, format!("{CACHE_DIR}/out/pkg/demo"));
        assert_eq!(
            plan.output_dir.as_deref(),
            Some(tmp.path().join(CACHE_DIR).join("out").join("pkg")).as_deref()
        );
        match plan.action {
            Action::RunCommand {
                argv,
                input_digest,
                cwd,
                ..
            } => {
                assert_eq!(argv[0], "rustc");
                assert!(argv.contains(&"--crate-type=bin".to_string()));
                assert!(argv.iter().any(|a| a == "--crate-name=demo"));
                assert_eq!(argv.last().map(String::as_str), Some("pkg/main.rs"));
                assert!(input_digest.is_some(), "rust_binary must hash its inputs");
                assert!(cwd.is_none(), "rust_binary runs at the workspace root");
            }
        }
    }

    #[test]
    fn rust_binary_action_root_package_drops_separator() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("main.rs"), b"fn main() {}").unwrap();
        let t = target("", "rust_binary", &["main.rs"]);
        let plan = rust_binary_action(tmp.path(), &t).unwrap();
        assert_eq!(plan.output, format!("{CACHE_DIR}/out/demo"));
    }

    #[test]
    fn cargo_binary_action_uses_attrs_and_falls_back_to_name_for_bin() {
        let mut t = target("", "cargo_binary", &["Cargo.toml"]);
        t.attrs.insert("cargo_package".into(), "fabrik-cli".into());
        // No explicit `bin`: defaults to `name`.
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), b"[package]\nname=\"x\"\n").unwrap();
        let plan = cargo_binary_action(tmp.path(), &t).unwrap();
        match plan.action {
            Action::RunCommand { argv, .. } => {
                assert_eq!(argv[0], "cargo");
                assert!(argv.contains(&"--locked".to_string()));
                assert!(argv
                    .windows(2)
                    .any(|w| w[0] == "--package" && w[1] == "fabrik-cli"));
                assert!(argv.windows(2).any(|w| w[0] == "--bin" && w[1] == "demo"));
            }
        }
        assert!(plan.output_dir.is_none());
    }

    #[test]
    fn cargo_binary_action_requires_cargo_package_attr() {
        let t = target("", "cargo_binary", &["Cargo.toml"]);
        let tmp = TempDir::new().unwrap();
        let err = cargo_binary_action(tmp.path(), &t)
            .err()
            .expect("missing cargo_package must error")
            .to_string();
        assert!(err.contains("cargo_package"), "got {err}");
    }

    #[test]
    fn action_for_rejects_unsupported_kinds() {
        let tmp = TempDir::new().unwrap();
        let t = target("", "rust_library", &["lib.rs"]);
        let err = action_for(tmp.path(), &t)
            .err()
            .expect("rust_library is unsupported")
            .to_string();
        assert!(err.contains("rust_library"), "got {err}");
    }
}

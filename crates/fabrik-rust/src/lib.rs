//! Rust language integration for Fabrik.
//!
//! Currently exposes one capability: build a `RunCommand` action that
//! invokes `cargo` against a Rust workspace, with the workspace's
//! source tree and toolchain versions folded into the action's cache
//! key. A second invocation with no source change is a cache hit.
//!
//! This is opaque-mode integration: cargo runs end-to-end, and Fabrik
//! caches the (stdout, stderr, exit) tuple. Subsequent phases will
//! replace this with cooperative resolution (`cargo metadata`) and
//! reimplemented per-crate `rustc` actions.

use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use fabrik_cas::Digest;
use fabrik_core::Action;
use ignore::WalkBuilder;
use tracing::instrument;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a Rust workspace: {0:?} has no Cargo.toml")]
    NotARustWorkspace(PathBuf),
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to run `{program} {arg}`: {source}")]
    ToolProbe {
        program: String,
        arg: String,
        #[source]
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

/// Snapshot of the Rust toolchain a cargo invocation will see.
///
/// Captured by shelling out to `cargo --version` and `rustc --version`
/// before constructing the action so the cache key reflects toolchain
/// drift, not just source drift.
#[derive(Debug, Clone)]
pub struct Toolchain {
    pub cargo_version: String,
    pub rustc_version: String,
}

impl Toolchain {
    /// Probe the toolchain on `PATH`.
    pub fn detect() -> Result<Self> {
        Ok(Self {
            cargo_version: probe("cargo", "--version")?,
            rustc_version: probe("rustc", "--version")?,
        })
    }

    fn fingerprint(&self) -> String {
        format!("cargo={}|rustc={}", self.cargo_version, self.rustc_version)
    }
}

fn probe(program: &str, arg: &str) -> Result<String> {
    let output = Command::new(program)
        .arg(arg)
        .output()
        .map_err(|source| Error::ToolProbe {
            program: program.into(),
            arg: arg.into(),
            source,
        })?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Compute a deterministic digest of the workspace's Rust source.
///
/// Includes `Cargo.toml`, `Cargo.lock`, and every `*.rs` file the
/// `ignore` crate considers visible (respects `.gitignore`,
/// `.ignore`, hidden-file rules). Files are sorted by their
/// workspace-relative path before hashing, so the digest is stable
/// across machines and filesystems with different directory-walk
/// orders.
///
/// Specifically excludes `target/` because cargo writes there and we
/// don't want a successful build to invalidate its own cache key.
#[instrument(skip(workspace))]
pub fn workspace_digest(workspace: &Path) -> Result<Digest> {
    let cargo_toml = workspace.join("Cargo.toml");
    if !cargo_toml.exists() {
        return Err(Error::NotARustWorkspace(workspace.to_path_buf()));
    }

    let mut entries: Vec<(String, Digest)> = Vec::new();
    let walker = WalkBuilder::new(workspace)
        .standard_filters(true)
        // Honor .gitignore even when the workspace isn't a git repo
        // (e.g. fixtures, tarballed sources). Without this, the ignore
        // crate skips .gitignore entirely outside a repo.
        .require_git(false)
        .filter_entry(|entry| {
            // Always traverse the root.
            if entry.depth() == 0 {
                return true;
            }
            // Drop target/ at any depth — cargo's output directory.
            entry.file_name() != "target"
        })
        .build();

    for result in walker {
        let entry = result.map_err(|e| Error::Io {
            path: workspace.to_path_buf(),
            source: std::io::Error::other(e.to_string()),
        })?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if !is_rust_source(path) {
            continue;
        }
        let rel = path
            .strip_prefix(workspace)
            .expect("walked path is under workspace");
        // Forward-slash path string so digests match across Windows
        // and unix.
        let rel_str = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let bytes = std::fs::read(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        entries.push((rel_str, Digest::of_bytes(&bytes)));
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = blake3::Hasher::new();
    hasher.update(b"fabrik.rust.workspace_digest.v1\0");
    for (rel, digest) in &entries {
        hasher.update(rel.as_bytes());
        hasher.update(b"\0");
        hasher.update(digest.as_bytes());
        hasher.update(b"\n");
    }
    Ok(Digest::from_bytes(*hasher.finalize().as_bytes()))
}

fn is_rust_source(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if name == "Cargo.toml" || name == "Cargo.lock" {
        return true;
    }
    matches!(path.extension().and_then(|e| e.to_str()), Some("rs"))
}

/// Build a `RunCommand` action that runs `cargo <args>` against
/// `workspace`, with the workspace source digest and toolchain
/// fingerprint folded into the cache key.
///
/// The action runs with `cwd = workspace_root` (the executor will
/// resolve `None` to it). The environment is the minimum cargo
/// expects: `PATH`, `HOME`, plus opt-in passthrough of `RUSTUP_HOME`,
/// `CARGO_HOME`, `CARGO_TARGET_DIR`, `USER`, `TERM` when the parent
/// has them set. A synthetic `FABRIK_CARGO_KEY` env var carries the
/// source digest + toolchain fingerprint so any change to either
/// invalidates the cache without affecting cargo's behavior.
pub fn cargo_action(workspace: &Path, args: &[String], toolchain: &Toolchain) -> Result<Action> {
    let digest = workspace_digest(workspace)?;
    let key = format!(
        "src={src}|tool={tool}",
        src = digest,
        tool = toolchain.fingerprint(),
    );

    let mut env: BTreeMap<String, String> = BTreeMap::new();
    env.insert("FABRIK_CARGO_KEY".into(), key);
    // Always-required env. Without PATH cargo can't find rustc; without
    // HOME it can't locate ~/.cargo.
    for k in ["PATH", "HOME"] {
        if let Ok(v) = env::var(k) {
            env.insert(k.into(), v);
        }
    }
    // Best-effort passthrough — included in the cache key so changes
    // to e.g. CARGO_TARGET_DIR force a rebuild.
    for k in [
        "RUSTUP_HOME",
        "RUSTUP_TOOLCHAIN",
        "CARGO_HOME",
        "CARGO_TARGET_DIR",
        "USER",
        "TERM",
    ] {
        if let Ok(v) = env::var(k) {
            env.insert(k.into(), v);
        }
    }

    let mut argument_vec = vec!["cargo".to_string()];
    argument_vec.extend(args.iter().cloned());

    Ok(Action::RunCommand {
        argv: argument_vec,
        env,
        cwd: None,
        timeout_ms: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn fixture() -> TempDir {
        let tmp = TempDir::new().unwrap();
        write(
            &tmp.path().join("Cargo.toml"),
            "[workspace]\nmembers = []\n",
        );
        write(&tmp.path().join("Cargo.lock"), "# locked\n");
        write(&tmp.path().join("src/main.rs"), "fn main() {}\n");
        tmp
    }

    #[test]
    fn workspace_digest_is_stable_across_calls() {
        let tmp = fixture();
        let a = workspace_digest(tmp.path()).unwrap();
        let b = workspace_digest(tmp.path()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn workspace_digest_rejects_non_rust_workspace() {
        let tmp = TempDir::new().unwrap();
        let err = workspace_digest(tmp.path()).unwrap_err();
        assert!(matches!(err, Error::NotARustWorkspace(_)));
    }

    #[test]
    fn workspace_digest_changes_when_a_source_file_changes() {
        let tmp = fixture();
        let before = workspace_digest(tmp.path()).unwrap();
        write(
            &tmp.path().join("src/main.rs"),
            "fn main() { println!(\"hi\"); }\n",
        );
        let after = workspace_digest(tmp.path()).unwrap();
        assert_ne!(before, after);
    }

    #[test]
    fn workspace_digest_changes_when_cargo_toml_changes() {
        let tmp = fixture();
        let before = workspace_digest(tmp.path()).unwrap();
        write(
            &tmp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"foo\"]\n",
        );
        let after = workspace_digest(tmp.path()).unwrap();
        assert_ne!(before, after);
    }

    #[test]
    fn workspace_digest_ignores_target_dir() {
        let tmp = fixture();
        let before = workspace_digest(tmp.path()).unwrap();
        // Drop a fake build artifact in target/ — must not affect digest.
        write(
            &tmp.path().join("target/release/fabrik-cli"),
            "\x7fELF... pretend",
        );
        write(&tmp.path().join("target/release/.rs"), "noise");
        write(
            &tmp.path().join("target/debug/build/foo-1234/output.rs"),
            "fn unrelated() {}",
        );
        let after = workspace_digest(tmp.path()).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn workspace_digest_respects_gitignore() {
        let tmp = fixture();
        // Put a file the user has ignored — should not affect digest.
        write(&tmp.path().join(".gitignore"), "ignored.rs\n");
        let before = workspace_digest(tmp.path()).unwrap();
        write(&tmp.path().join("ignored.rs"), "fn ignored() {}");
        let after = workspace_digest(tmp.path()).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn workspace_digest_changes_when_a_new_rs_file_is_added() {
        let tmp = fixture();
        let before = workspace_digest(tmp.path()).unwrap();
        write(&tmp.path().join("src/lib.rs"), "pub fn lib() {}\n");
        let after = workspace_digest(tmp.path()).unwrap();
        assert_ne!(before, after);
    }

    #[test]
    fn cargo_action_includes_synthetic_key_in_env() {
        let tmp = fixture();
        let toolchain = Toolchain {
            cargo_version: "cargo 1.86.0".into(),
            rustc_version: "rustc 1.86.0".into(),
        };
        let action = cargo_action(
            tmp.path(),
            &["build".to_string(), "--release".to_string()],
            &toolchain,
        )
        .unwrap();
        let Action::RunCommand { argv, env, .. } = action;
        assert_eq!(argv, vec!["cargo", "build", "--release"]);
        let key = env.get("FABRIK_CARGO_KEY").expect("synthetic key set");
        assert!(key.starts_with("src="), "{key}");
        assert!(key.contains("tool=cargo=cargo 1.86.0"), "{key}");
    }

    #[test]
    fn cargo_action_changes_digest_when_source_changes() {
        let tmp = fixture();
        let toolchain = Toolchain {
            cargo_version: "cargo 1.86.0".into(),
            rustc_version: "rustc 1.86.0".into(),
        };
        let a = cargo_action(tmp.path(), &["build".into()], &toolchain).unwrap();
        write(
            &tmp.path().join("src/main.rs"),
            "fn main() { /* edit */ }\n",
        );
        let b = cargo_action(tmp.path(), &["build".into()], &toolchain).unwrap();
        assert_ne!(a.digest(), b.digest());
    }

    #[test]
    fn cargo_action_changes_digest_when_toolchain_changes() {
        let tmp = fixture();
        let t1 = Toolchain {
            cargo_version: "cargo 1.86.0".into(),
            rustc_version: "rustc 1.86.0".into(),
        };
        let t2 = Toolchain {
            cargo_version: "cargo 1.87.0".into(),
            rustc_version: "rustc 1.87.0".into(),
        };
        let a = cargo_action(tmp.path(), &["build".into()], &t1).unwrap();
        let b = cargo_action(tmp.path(), &["build".into()], &t2).unwrap();
        assert_ne!(a.digest(), b.digest());
    }

    #[test]
    fn cargo_action_changes_digest_when_args_change() {
        let tmp = fixture();
        let toolchain = Toolchain {
            cargo_version: "cargo 1.86.0".into(),
            rustc_version: "rustc 1.86.0".into(),
        };
        let a = cargo_action(tmp.path(), &["build".into()], &toolchain).unwrap();
        let b = cargo_action(tmp.path(), &["test".into()], &toolchain).unwrap();
        assert_ne!(a.digest(), b.digest());
    }
}

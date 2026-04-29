//! Action types and cache-aware execution.
//!
//! Currently exposes one action kind ([`Action::RunCommand`]) and a
//! straight-line executor that consults a [`Cas`] for memoization
//! before spawning a subprocess.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use fabrik_cas::{ActionResult, Cas, Digest};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cas error: {0}")]
    Cas(#[from] fabrik_cas::Error),
    #[error("failed to spawn {program}: {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("action requires a non-empty argv")]
    EmptyArgv,
}

pub type Result<T> = std::result::Result<T, Error>;

/// All actions Fabrik can execute.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    RunCommand {
        argv: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        #[serde(default)]
        cwd: Option<PathBuf>,
    },
}

impl Action {
    /// Canonical, content-addressed key for this action. Two actions with
    /// the same canonical JSON encoding share a cache slot.
    pub fn digest(&self) -> Digest {
        // BTreeMap gives deterministic key ordering in serde_json output;
        // Vec<String> is intrinsically ordered. The canonical encoding
        // is therefore stable across processes.
        let canonical = serde_json::to_vec(self).expect("Action is serializable");
        Digest::of_bytes(&canonical)
    }
}

/// Whether a result came from cache or fresh execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheState {
    Hit,
    Miss,
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub action: Digest,
    pub result: ActionResult,
    pub cache: CacheState,
}

/// Run an action, consulting the CAS first. On a miss, executes the
/// underlying command and stores the result.
pub fn run(action: &Action, cas: &Cas) -> Result<Outcome> {
    let key = action.digest();
    if let Some(result) = cas.get_action_result(&key)? {
        return Ok(Outcome {
            action: key,
            result,
            cache: CacheState::Hit,
        });
    }
    let result = execute(action, cas)?;
    cas.put_action_result(&key, &result)?;
    Ok(Outcome {
        action: key,
        result,
        cache: CacheState::Miss,
    })
}

fn execute(action: &Action, cas: &Cas) -> Result<ActionResult> {
    match action {
        Action::RunCommand { argv, env, cwd } => {
            let (program, rest) = argv.split_first().ok_or(Error::EmptyArgv)?;
            let mut cmd = Command::new(program);
            cmd.args(rest);
            // Don't inherit the parent's env: actions must declare every
            // variable they depend on, or the cache key lies.
            cmd.env_clear();
            for (k, v) in env {
                cmd.env(k, v);
            }
            if let Some(cwd) = cwd {
                cmd.current_dir(cwd);
            }
            let output = cmd.output().map_err(|source| Error::Spawn {
                program: program.clone(),
                source,
            })?;
            let stdout = cas.put_blob(&output.stdout)?;
            let stderr = cas.put_blob(&output.stderr)?;
            Ok(ActionResult {
                exit_code: output.status.code().unwrap_or(-1),
                stdout,
                stderr,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fresh_cas() -> (TempDir, Cas) {
        let tmp = TempDir::new().unwrap();
        let cas = Cas::open(tmp.path()).unwrap();
        (tmp, cas)
    }

    fn echo_action(msg: &str) -> Action {
        // Use /bin/sh -c so the test is portable across the limited PATH
        // we set explicitly.
        Action::RunCommand {
            argv: vec!["/bin/sh".into(), "-c".into(), format!("printf '{msg}'")],
            env: BTreeMap::new(),
            cwd: None,
        }
    }

    #[test]
    fn first_run_is_miss_second_is_hit() {
        let (_tmp, cas) = fresh_cas();
        let action = echo_action("hello");
        let first = run(&action, &cas).unwrap();
        assert_eq!(first.cache, CacheState::Miss);
        assert_eq!(first.result.exit_code, 0);
        assert_eq!(cas.get_blob(&first.result.stdout).unwrap(), b"hello");

        let second = run(&action, &cas).unwrap();
        assert_eq!(second.cache, CacheState::Hit);
        assert_eq!(second.result, first.result);
    }

    #[test]
    fn different_argv_gets_different_cache_slot() {
        let (_tmp, cas) = fresh_cas();
        let a = run(&echo_action("a"), &cas).unwrap();
        let b = run(&echo_action("b"), &cas).unwrap();
        assert_ne!(a.action, b.action);
    }

    #[test]
    fn env_is_part_of_the_cache_key() {
        let (_tmp, cas) = fresh_cas();
        let mut env_a = BTreeMap::new();
        env_a.insert("X".into(), "1".into());
        let mut env_b = BTreeMap::new();
        env_b.insert("X".into(), "2".into());
        let argv = vec!["/bin/sh".into(), "-c".into(), "true".into()];
        let a = Action::RunCommand {
            argv: argv.clone(),
            env: env_a,
            cwd: None,
        };
        let b = Action::RunCommand {
            argv,
            env: env_b,
            cwd: None,
        };
        assert_ne!(a.digest(), b.digest());
    }

    #[test]
    fn nonzero_exit_still_caches() {
        let (_tmp, cas) = fresh_cas();
        let action = Action::RunCommand {
            argv: vec!["/bin/sh".into(), "-c".into(), "exit 7".into()],
            env: BTreeMap::new(),
            cwd: None,
        };
        let first = run(&action, &cas).unwrap();
        assert_eq!(first.result.exit_code, 7);
        let second = run(&action, &cas).unwrap();
        assert_eq!(second.cache, CacheState::Hit);
    }
}

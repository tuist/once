//! Lifecycle of a command an uncacheable action spawns directly.
//!
//! Killing only the spawned process leaves whatever it started running, and a
//! descendant that inherited the stderr pipe keeps the capture from ever
//! seeing end of file. The command runs as an [`ActionProcessGroup`], the
//! same supervised group local actions use: its leader is observed exiting
//! without being reaped so its group id stays reserved, the group is in the
//! table a forwarded termination signal reaches, and dropping it before it
//! was reaped kills the group. A descendant holding stderr past a short grace
//! after the command exits gets its group killed so the capture can finish.

use std::process::ExitStatus;
use std::time::Duration;

use anyhow::{Context, Result};
use struo_cas::{CacheProvider, Digest};
use struo_core::ActionProcessGroup;
use tokio::process::{ChildStderr, Command};

/// How long a descendant may keep stderr open after the command exits.
const STDERR_GRACE: Duration = Duration::from_secs(2);

/// A spawned command leading its own supervised process group.
pub(super) struct ProcessGroup {
    group: ActionProcessGroup,
}

impl ProcessGroup {
    pub(super) fn spawn(command: &mut Command, context: &'static str) -> Result<Self> {
        let group = ActionProcessGroup::spawn(command).context(context)?;
        Ok(Self { group })
    }

    pub(super) fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.group.take_stderr()
    }

    /// Wait for the command and, when given, capture its stderr into the
    /// cache, killing the group if a descendant holds stderr open past the
    /// grace period.
    pub(super) async fn wait(
        mut self,
        stderr: Option<ChildStderr>,
        cache: &CacheProvider,
    ) -> Result<(ExitStatus, Option<Digest>)> {
        let drain = async {
            match stderr {
                Some(pipe) => cache.put_stream(pipe).await.map(Some),
                None => Ok(None),
            }
        };
        let mut drain = std::pin::pin!(drain);
        let mut drained = None;
        loop {
            tokio::select! {
                result = &mut drain, if drained.is_none() => {
                    drained = Some(result?);
                }
                exited = self.group.exited() => {
                    exited.context("waiting for declared action")?;
                    break;
                }
            }
        }
        let stderr = if let Some(stderr) = drained {
            stderr
        } else if let Ok(result) = tokio::time::timeout(STDERR_GRACE, &mut drain).await {
            result?
        } else {
            tracing::debug!(
                "a process the declared action started kept stderr open; killing its group"
            );
            self.group.kill_group();
            drain.await?
        };
        let status = self.group.reap().await.context("reaping declared action")?;
        Ok((status, stderr))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn command(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(script)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped());
        command
    }

    #[tokio::test]
    async fn a_descendant_holding_stderr_does_not_hang_the_action() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(struo_cas::Cas::open(workspace.path().join("cas")));
        let mut group = ProcessGroup::spawn(
            &mut command("printf started >&2; sleep 30 & exit 3"),
            "spawning test",
        )
        .unwrap();
        let stderr = group.take_stderr();
        let started = std::time::Instant::now();

        let (status, stderr) = group.wait(stderr, &cache).await.unwrap();

        assert_eq!(status.code(), Some(3));
        assert!(started.elapsed() < Duration::from_secs(20));
        let stderr = cache.get_blob(&stderr.unwrap()).await.unwrap();
        assert_eq!(stderr, b"started");
    }

    #[tokio::test]
    async fn dropping_an_unfinished_group_kills_its_descendants() {
        let workspace = tempfile::tempdir().unwrap();
        let marker = workspace.path().join("survived");
        let mut command = command(&format!("(sleep 1; touch {}) & wait", marker.display()));
        command.stderr(std::process::Stdio::null());
        let group = ProcessGroup::spawn(&mut command, "spawning test").unwrap();

        drop(group);
        tokio::time::sleep(Duration::from_millis(1500)).await;

        assert!(!marker.exists());
    }
}

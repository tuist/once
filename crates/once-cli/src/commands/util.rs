//! Cross-verb helpers: workspace target lookup and cache-state
//! rendering. These are tiny on their own; the value is forcing every
//! verb through one shape so adding a new verb doesn't reinvent the
//! same boilerplate slightly differently.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use once_cas::{CacheProvider, Digest};
use once_core::CacheState;
use once_frontend::Target;
use tokio::io::AsyncWrite;

pub const CHILD_OUTPUT_LIMIT: u64 = 16 * 1024 * 1024;

pub async fn write_cache_blob<W: AsyncWrite + Unpin>(
    cache: &CacheProvider,
    digest: &Digest,
    writer: &mut W,
) -> Result<()> {
    let directory = tempfile::tempdir().context("creating output staging directory")?;
    let path = directory.path().join("output");
    cache.copy_blob_to_file(digest, &path).await?;
    let mut file = tokio::fs::File::open(&path)
        .await
        .with_context(|| format!("opening staged output {}", path.display()))?;
    tokio::io::copy(&mut file, writer).await?;
    Ok(())
}

pub struct CapturedOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

pub fn capture_command_output(command: &mut Command) -> Result<CapturedOutput> {
    let stdout = tempfile::tempfile().context("creating child stdout staging file")?;
    let stderr = tempfile::tempfile().context("creating child stderr staging file")?;
    command.stdout(Stdio::from(
        stdout
            .try_clone()
            .context("cloning child stdout staging file")?,
    ));
    command.stderr(Stdio::from(
        stderr
            .try_clone()
            .context("cloning child stderr staging file")?,
    ));
    let status = command.status().context("running child process")?;
    let (stdout, stdout_truncated) = read_staged_output(stdout, false)?;
    let (stderr, stderr_truncated) = read_staged_output(stderr, true)?;
    Ok(CapturedOutput {
        status,
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
    })
}

pub async fn capture_tokio_command_output(
    command: &mut tokio::process::Command,
    timeout: Option<Duration>,
) -> Result<CapturedOutput> {
    let stdout = tempfile::tempfile().context("creating child stdout staging file")?;
    let stderr = tempfile::tempfile().context("creating child stderr staging file")?;
    command.stdout(Stdio::from(
        stdout
            .try_clone()
            .context("cloning child stdout staging file")?,
    ));
    command.stderr(Stdio::from(
        stderr
            .try_clone()
            .context("cloning child stderr staging file")?,
    ));
    let status = match timeout {
        Some(duration) => tokio::time::timeout(duration, command.status())
            .await
            .context("child process timed out")??,
        None => command.status().await?,
    };
    let (stdout, stdout_truncated) = read_staged_output(stdout, false)?;
    let (stderr, stderr_truncated) = read_staged_output(stderr, true)?;
    Ok(CapturedOutput {
        status,
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
    })
}

fn read_staged_output(mut file: std::fs::File, from_end: bool) -> Result<(Vec<u8>, bool)> {
    read_staged_output_with_limit(&mut file, from_end, CHILD_OUTPUT_LIMIT)
}

fn read_staged_output_with_limit(
    file: &mut std::fs::File,
    from_end: bool,
    limit: u64,
) -> Result<(Vec<u8>, bool)> {
    let len = file.metadata().context("reading child output size")?.len();
    let truncated = len > limit;
    if from_end && truncated {
        file.seek(SeekFrom::Start(len - limit))
            .context("seeking child output")?;
    } else {
        file.rewind().context("rewinding child output")?;
    }
    let mut bytes = Vec::with_capacity(
        usize::try_from(len.min(limit)).context("child output limit exceeds addressable memory")?,
    );
    file.take(limit)
        .read_to_end(&mut bytes)
        .context("reading child output")?;
    Ok((bytes, truncated))
}

/// Load every target declared in the workspace and pick the one whose
/// id matches `target_id`. Returns an error if the workspace fails to
/// load or no target has the requested id. Verbs that operate on a
/// single target funnel through this so the error wording is uniform.
pub fn find_target(workspace: &Path, target_id: &str) -> Result<(Vec<Target>, usize)> {
    let targets = once_frontend::load_workspace(workspace).context("loading workspace")?;
    let idx = targets
        .iter()
        .position(|t| t.id() == target_id)
        .ok_or_else(|| anyhow!("no target matches `{target_id}`"))?;
    Ok((targets, idx))
}

/// The short string Once prints for a [`CacheState`]. Repeated
/// in every verb that emits structured output, so it lives here to
/// keep the spelling uniform (`hit` / `miss`, no trailing space).
#[must_use]
pub fn cache_tag(cache: CacheState) -> &'static str {
    match cache {
        CacheState::Hit => "hit",
        CacheState::Miss => "miss",
    }
}

#[must_use]
pub fn relative_path(from: &str, to: &str) -> String {
    if from.is_empty() {
        return to.to_string();
    }
    let from_parts = from
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let to_parts = to
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let mut shared = 0;
    while shared < from_parts.len()
        && shared < to_parts.len()
        && from_parts[shared] == to_parts[shared]
    {
        shared += 1;
    }

    let mut parts = Vec::new();
    for _ in shared..from_parts.len() {
        parts.push("..".to_string());
    }
    for part in &to_parts[shared..] {
        parts.push((*part).to_string());
    }
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staged_output_keeps_a_prefix_or_suffix_with_bounded_memory() {
        let mut file = tempfile::tempfile().unwrap();
        std::io::Write::write_all(&mut file, b"abcdefgh").unwrap();

        let (prefix, prefix_truncated) =
            read_staged_output_with_limit(&mut file, false, 4).unwrap();
        let (suffix, suffix_truncated) = read_staged_output_with_limit(&mut file, true, 4).unwrap();

        assert_eq!(prefix, b"abcd");
        assert_eq!(suffix, b"efgh");
        assert!(prefix_truncated);
        assert!(suffix_truncated);
    }
}

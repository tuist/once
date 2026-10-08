use std::collections::HashSet;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use once_events_client::SourceFileSnapshot;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

const MAX_TREE_BYTES: usize = 32 * 1024 * 1024;
const MAX_TREE_ENTRIES: usize = 200_000;
const GIT_TIMEOUT: Duration = Duration::from_secs(3);

pub(super) async fn revision(workspace: &Path) -> String {
    git_output(workspace, &["rev-parse", "--verify", "HEAD"], 128)
        .await
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|revision| revision.trim().to_string())
        .unwrap_or_default()
}

pub(super) async fn collect(workspace: &Path, revision: &str) -> SourceFileSnapshot {
    collect_complete(workspace, revision)
        .await
        .unwrap_or_else(|| {
            tracing::debug!("repository source classification unavailable");
            SourceFileSnapshot::default()
        })
}

async fn collect_complete(workspace: &Path, revision: &str) -> Option<SourceFileSnapshot> {
    if !matches!(revision.len(), 40 | 64) || !revision.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    let deadline = tokio::time::Instant::now() + GIT_TIMEOUT;
    let root =
        git_output_until(workspace, &["rev-parse", "--show-toplevel"], 4096, deadline).await?;
    let root = std::str::from_utf8(&root)
        .ok()?
        .trim_end_matches(['\r', '\n']);
    let expected_root = if let Some(work_tree) = std::env::var_os("GIT_WORK_TREE") {
        workspace.join(work_tree)
    } else {
        workspace
            .ancestors()
            .find(|path| path.join(".git").exists())?
            .to_path_buf()
    };
    if Path::new(root).canonicalize().ok()? != expected_root.canonicalize().ok()? {
        return None;
    }
    let superproject = git_output_until(
        workspace,
        &["rev-parse", "--show-superproject-working-tree"],
        4096,
        deadline,
    )
    .await?;
    if superproject
        .iter()
        .any(|byte| !matches!(byte, b'\r' | b'\n'))
    {
        return None;
    }
    let tree = format!("{revision}^{{tree}}");
    let output = git_output_until(
        workspace,
        &["ls-tree", "-r", "-z", "--full-tree", &tree, "--"],
        MAX_TREE_BYTES,
        deadline,
    )
    .await?;
    let paths = parse_tree(&output, MAX_TREE_ENTRIES)?;
    (tokio::time::Instant::now() < deadline)
        .then(|| SourceFileSnapshot::from_committed_paths(paths))
}

async fn git_output(workspace: &Path, args: &[&str], max_bytes: usize) -> Option<Vec<u8>> {
    git_output_until(
        workspace,
        args,
        max_bytes,
        tokio::time::Instant::now() + GIT_TIMEOUT,
    )
    .await
}

async fn git_output_until(
    workspace: &Path,
    args: &[&str],
    max_bytes: usize,
    deadline: tokio::time::Instant,
) -> Option<Vec<u8>> {
    let remaining = deadline.checked_duration_since(tokio::time::Instant::now())?;
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(workspace)
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command_output(command, max_bytes, remaining).await
}

async fn command_output(
    mut command: Command,
    max_bytes: usize,
    timeout: Duration,
) -> Option<Vec<u8>> {
    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let result = tokio::time::timeout(timeout, async {
        let mut bytes = Vec::new();
        stdout
            .take(u64::try_from(max_bytes).ok()?.saturating_add(1))
            .read_to_end(&mut bytes)
            .await
            .ok()?;
        if bytes.len() > max_bytes || !child.wait().await.ok()?.success() {
            return None;
        }
        Some(bytes)
    })
    .await
    .ok()
    .flatten();
    if result.is_none() {
        let _ = child.kill().await;
    }
    result
}

fn parse_tree(bytes: &[u8], max_entries: usize) -> Option<HashSet<String>> {
    if !bytes.is_empty() && bytes.last() != Some(&0) {
        return None;
    }
    let mut paths = HashSet::new();
    for (index, entry) in bytes
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .enumerate()
    {
        if index >= max_entries {
            return None;
        }
        let tab = entry.iter().position(|byte| *byte == b'\t')?;
        let header = std::str::from_utf8(&entry[..tab]).ok()?;
        let mut columns = header.split(' ');
        let mode = columns.next()?;
        let kind = columns.next()?;
        let object = columns.next()?;
        if columns.next().is_some() || !object.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        match (mode, kind) {
            ("100644" | "100755" | "120000", "blob") => {
                if let Ok(path) = std::str::from_utf8(&entry[tab + 1..]) {
                    paths.insert(path.to_string());
                }
            }
            ("160000", "commit") => {}
            _ => return None,
        }
    }
    Some(paths)
}

#[cfg(test)]
mod tests;

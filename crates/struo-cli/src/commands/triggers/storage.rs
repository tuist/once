use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use fd_lock::RwLock;

const INPUT_LIMIT: u64 = 1024 * 1024;

pub(super) fn read_bounded(reader: impl Read) -> Result<Vec<u8>> {
    let mut raw = Vec::new();
    reader.take(INPUT_LIMIT + 1).read_to_end(&mut raw)?;
    if raw.len() as u64 > INPUT_LIMIT {
        bail!("trigger input exceeds {INPUT_LIMIT} bytes");
    }
    Ok(raw)
}

pub(super) fn read(path: &Path) -> Result<Vec<u8>> {
    read_bounded(File::open(path).with_context(|| format!("opening {}", path.display()))?)
}

pub(super) fn manifest(workspace: &Path) -> Result<String> {
    match read(&workspace.join("struo.toml")) {
        Ok(raw) => String::from_utf8(raw).context("root manifest is not UTF-8"),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            Ok(String::new())
        }
        Err(error) => Err(error),
    }
}

pub(super) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if bytes.len() as u64 > INPUT_LIMIT {
        bail!("trigger record exceeds {INPUT_LIMIT} bytes");
    }
    let parent = path
        .parent()
        .context("trigger record has no parent directory")?;
    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

pub(super) fn lock(workspace: &Path) -> Result<RwLock<File>> {
    let directory = workspace.join(".struo/triggers");
    std::fs::create_dir_all(&directory)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join("worker.lock"))?;
    Ok(RwLock::new(file))
}

pub(super) fn git(workspace: &Path, args: &[&str]) -> Result<String> {
    let output = crate::commands::util::capture_command_output(
        Command::new("git")
            .arg("--no-pager")
            .arg("--no-replace-objects")
            .arg("-C")
            .arg(workspace)
            .args(args)
            .env("GIT_OPTIONAL_LOCKS", "0"),
    )?;
    if !output.status.success() {
        bail!(
            "Git could not read the requested trigger source object: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    if output.stdout_truncated {
        bail!("Git trigger source output exceeds the capture limit");
    }
    String::from_utf8(output.stdout).context("Git trigger source is not UTF-8")
}

pub(super) fn repository(workspace: &Path) -> Result<String> {
    let root = git(workspace, &["rev-parse", "--show-toplevel"])?;
    let root = Path::new(root.trim()).canonicalize()?;
    if root != workspace.canonicalize()? {
        bail!("trigger operations require the Git repository root");
    }
    Ok(root.to_string_lossy().into_owned())
}

pub(super) fn revision(workspace: &Path, revision: &str) -> Result<()> {
    if !matches!(revision.len(), 40 | 64)
        || !revision
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        bail!("revision must be a full lowercase commit object id, not a ref or abbreviation");
    }
    let resolved = git(
        workspace,
        &["rev-parse", "--verify", &format!("{revision}^{{commit}}")],
    )?;
    if resolved.trim() != revision {
        bail!("revision does not identify the exact commit object");
    }
    Ok(())
}

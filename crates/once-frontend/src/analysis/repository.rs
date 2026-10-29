use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

use super::globals::sha256_hex;
use super::store::{capture_host_command_output, HOST_COMMAND_OUTPUT_LIMIT};

pub(super) fn checkout(
    git: &str,
    url: &str,
    revision: &str,
    path: &Path,
    cache: &Path,
) -> Result<()> {
    if revision.len() != 40 && revision.len() != 64
        || !revision.bytes().all(|b| b.is_ascii_hexdigit())
    {
        bail!("host_git_checkout requires a full hexadecimal commit revision");
    }
    if !path.is_absolute() || !cache.is_absolute() {
        bail!("host_git_checkout requires absolute checkout and cache paths");
    }
    if url.is_empty() || url.starts_with('-') {
        bail!("host_git_checkout requires a repository URL or path");
    }
    std::fs::create_dir_all(cache)?;
    let _checkout_lock = lock(&cache.join(format!(
        "checkout-{}.lock",
        sha256_hex(path.as_os_str().as_encoded_bytes())
    )))?;
    if path.exists() {
        let origin = run(
            git,
            [
                OsStr::new("-C"),
                path.as_os_str(),
                OsStr::new("config"),
                OsStr::new("--get"),
                OsStr::new("remote.origin.url"),
            ],
            true,
        )?;
        if origin.trim() != url {
            bail!("package checkout at `{}` belongs to a different repository; remove this cached checkout and reload", path.display());
        }
        let current = run(
            git,
            [
                OsStr::new("-C"),
                path.as_os_str(),
                OsStr::new("rev-parse"),
                OsStr::new("HEAD"),
            ],
            true,
        )?;
        if current.trim() == revision {
            return Ok(());
        }
    }
    let repository = cache.join(sha256_hex(url.as_bytes()));
    let _repository_lock = lock(&repository.with_extension("lock"))?;
    ensure_revision(git, url, revision, &repository, cache)?;
    materialize(git, url, revision, path, &repository)
}

fn ensure_revision(
    git: &str,
    url: &str,
    revision: &str,
    repository: &Path,
    cache: &Path,
) -> Result<()> {
    if !repository.exists() {
        let temporary = tempfile::tempdir_in(cache)?;
        let staging = temporary.path().join("repository");
        run(
            git,
            [
                OsStr::new("clone"),
                OsStr::new("--mirror"),
                OsStr::new("--"),
                OsStr::new(url),
                staging.as_os_str(),
            ],
            true,
        )?;
        std::fs::rename(staging, repository)?;
    }
    let has_revision = || -> Result<bool> {
        Ok(run(
            git,
            [
                OsStr::new("-C"),
                repository.as_os_str(),
                OsStr::new("cat-file"),
                OsStr::new("-t"),
                OsStr::new(&format!("{revision}^{{commit}}")),
            ],
            false,
        )?
        .trim()
            == "commit")
    };
    if !has_revision()? {
        run(
            git,
            [
                OsStr::new("-C"),
                repository.as_os_str(),
                OsStr::new("fetch"),
                OsStr::new("--depth"),
                OsStr::new("1"),
                OsStr::new("origin"),
                OsStr::new(revision),
            ],
            false,
        )?;
        if !has_revision()? {
            run(
                git,
                [
                    OsStr::new("-C"),
                    repository.as_os_str(),
                    OsStr::new("fetch"),
                    OsStr::new("origin"),
                    OsStr::new(revision),
                ],
                true,
            )?;
        }
    }
    // Keep fetched commits reachable even when another worktree fetches a different pin.
    run(
        git,
        [
            OsStr::new("-C"),
            repository.as_os_str(),
            OsStr::new("update-ref"),
            OsStr::new(&format!("refs/once/{revision}")),
            OsStr::new(revision),
        ],
        true,
    )?;
    Ok(())
}

fn materialize(git: &str, url: &str, revision: &str, path: &Path, repository: &Path) -> Result<()> {
    if path.exists() {
        run(
            git,
            [
                OsStr::new("-C"),
                path.as_os_str(),
                OsStr::new("fetch"),
                OsStr::new("--no-tags"),
                repository.as_os_str(),
                OsStr::new(revision),
            ],
            true,
        )?;
        run(
            git,
            [
                OsStr::new("-C"),
                path.as_os_str(),
                OsStr::new("checkout"),
                OsStr::new("--detach"),
                OsStr::new(revision),
            ],
            true,
        )?;
    } else {
        let parent = path.parent().context("checkout has no parent")?;
        std::fs::create_dir_all(parent)?;
        let temporary = tempfile::tempdir_in(parent)?;
        let staging = temporary.path().join("checkout");
        run(
            git,
            [
                OsStr::new("clone"),
                OsStr::new("--shared"),
                OsStr::new("--no-checkout"),
                OsStr::new("--"),
                repository.as_os_str(),
                staging.as_os_str(),
            ],
            true,
        )?;
        run(
            git,
            [
                OsStr::new("-C"),
                staging.as_os_str(),
                OsStr::new("remote"),
                OsStr::new("set-url"),
                OsStr::new("origin"),
                OsStr::new(url),
            ],
            true,
        )?;
        run(
            git,
            [
                OsStr::new("-C"),
                staging.as_os_str(),
                OsStr::new("checkout"),
                OsStr::new("--detach"),
                OsStr::new(revision),
            ],
            true,
        )?;
        std::fs::rename(staging, path)?;
    }
    Ok(())
}

fn lock(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    file.lock().context("locking repository cache")?;
    Ok(file)
}

fn run<I, S>(git: &str, args: I, check: bool) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new(git);
    command.args(args).env("GIT_TERMINAL_PROMPT", "0");
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_NAMESPACE",
        "GIT_SHALLOW_FILE",
    ] {
        command.env_remove(name);
    }
    let (status, stdout, _) = capture_host_command_output(&mut command, HOST_COMMAND_OUTPUT_LIMIT)
        .context("running repository acquisition command")?;
    if check && !status.success() {
        // Git's stderr and argv can contain credentials embedded in repository URLs.
        bail!("repository acquisition failed ({status}); check repository access and the pinned revision");
    }
    Ok(String::from_utf8(stdout)?)
}

use std::path::Path;

use anyhow::{Context, Result};
use struo_cas::Digest;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::commands::graph::analysis) enum FingerprintKind {
    Directory,
    Symlink,
}

pub(super) fn reads_fingerprints(
    declared: &struo_frontend::analysis::DeclaredAction,
    inputs: &std::collections::BTreeMap<String, super::AvailableInput>,
    producers: &super::Producers,
) -> bool {
    (declared.depends_on_prior_actions
        && inputs.iter().any(|(path, input)| {
            input.same_target && producers.has_fingerprint(path, input.blob_digest)
        }))
        || declared
            .inputs
            .iter()
            .filter_map(|path| super::enclosing_available_output(inputs, path))
            .any(|(path, input)| producers.has_fingerprint(path, input.blob_digest))
}

pub(super) struct Fingerprint {
    pub(super) kind: FingerprintKind,
    pub(super) bytes: Vec<u8>,
}

pub(super) fn fingerprint(path: &Path) -> Result<Option<Fingerprint>> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        let target = std::fs::read_link(path)
            .with_context(|| format!("reading declared output symlink `{}`", path.display()))?;
        return Ok(Some(Fingerprint {
            kind: FingerprintKind::Symlink,
            bytes: format!("struo.symlink_output.v1\n{}\n", target.to_string_lossy()).into_bytes(),
        }));
    }
    if metadata.is_dir() {
        return Ok(Some(Fingerprint {
            kind: FingerprintKind::Directory,
            bytes: directory_manifest_bytes(path)?,
        }));
    }
    Ok(None)
}

pub(super) async fn verify(
    workspace: &Path,
    path: &str,
    digest: Digest,
    kind: FingerprintKind,
    lease: &struo_core::ExecutionPermit,
) -> Result<()> {
    let absolute = workspace.join(path);
    let matched = capture_step(Some(lease), move || match fingerprint(&absolute) {
        Ok(Some(current)) => Ok(current.kind == kind && Digest::of_bytes(&current.bytes) == digest),
        Ok(None) => Ok(false),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    })
    .await??;
    let description = match kind {
        FingerprintKind::Directory => "directory",
        FingerprintKind::Symlink => "symlink",
    };
    anyhow::ensure!(matched,
        "cannot restore fingerprint-only output `{path}`: its uncached {description} producer recorded a fingerprint, not restorable contents; keep the output unchanged until it is consumed, or make the producer cacheable"
    );
    Ok(())
}

pub(super) async fn capture_step<T, F>(
    lease: Option<&struo_core::ExecutionPermit>,
    work: F,
) -> std::result::Result<T, tokio::task::JoinError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    match lease {
        Some(lease) => lease.run_blocking(work).await,
        None => tokio::task::spawn_blocking(work).await,
    }
}

fn directory_manifest_bytes(root: &Path) -> Result<Vec<u8>> {
    let mut entries = Vec::new();
    collect_directory_manifest(root, root, &mut entries)?;
    let mut manifest = b"struo.directory_output.v1\n".to_vec();
    for entry in entries {
        manifest.extend_from_slice(entry.as_bytes());
        manifest.push(b'\n');
    }
    Ok(manifest)
}

fn collect_directory_manifest(root: &Path, dir: &Path, entries: &mut Vec<String>) -> Result<()> {
    let mut children = std::fs::read_dir(dir)
        .with_context(|| format!("reading declared directory output `{}`", dir.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("reading declared directory output `{}`", dir.display()))?;
    children.sort_by_key(std::fs::DirEntry::file_name);
    for child in children {
        let path = child.path();
        let relative = path
            .strip_prefix(root)
            .unwrap_or(path.as_path())
            .to_string_lossy()
            .replace('\\', "/");
        let metadata = std::fs::symlink_metadata(&path)
            .with_context(|| format!("reading declared output metadata `{}`", path.display()))?;
        if metadata.is_dir() {
            entries.push(format!("dir\t{relative}"));
            collect_directory_manifest(root, &path, entries)?;
        } else if metadata.is_file() {
            let file = std::fs::File::open(&path)
                .with_context(|| format!("reading declared output file `{}`", path.display()))?;
            let digest = Digest::of_reader(std::io::BufReader::new(file))
                .with_context(|| format!("hashing declared output file `{}`", path.display()))?;
            entries.push(format!("file\t{relative}\t{digest}"));
        } else if metadata.file_type().is_symlink() {
            let target = std::fs::read_link(&path)
                .with_context(|| format!("reading declared output symlink `{}`", path.display()))?;
            entries.push(format!(
                "symlink\t{relative}\t{}",
                Digest::of_bytes(target.to_string_lossy().as_bytes())
            ));
        }
    }
    Ok(())
}

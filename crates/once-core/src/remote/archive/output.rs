use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::archive_error;
use super::limits::check_entries;
use crate::remote::output_install::OutputStaging;
use crate::remote::path::{path_is_declared, Directory};
use crate::remote::PhasePermit;
use crate::{Error, RemoteIoLimits, Result, WorkspacePath};

#[path = "output/budget.rs"]
mod budget;

#[cfg(all(test, any(target_os = "linux", target_vendor = "apple")))]
#[path = "output/budget_tests.rs"]
mod budget_tests;

#[cfg(all(test, any(target_os = "linux", target_vendor = "apple")))]
#[path = "output/symlinks.rs"]
mod symlink_tests;

#[cfg(all(test, any(target_os = "linux", target_vendor = "apple")))]
async fn install_output_archive(
    archive_path: &Path,
    workspace_root: &Path,
    outputs: &[WorkspacePath],
    provider: &'static str,
) -> Result<()> {
    install_output_archive_limited(
        archive_path,
        workspace_root,
        outputs,
        provider,
        RemoteIoLimits::default(),
        PhasePermit::default(),
    )
    .await
}

pub(in crate::remote) async fn install_output_archive_limited(
    archive_path: &Path,
    workspace_root: &Path,
    outputs: &[WorkspacePath],
    provider: &'static str,
    limits: RemoteIoLimits,
    lease: PhasePermit,
) -> Result<()> {
    let archive = archive_path.to_path_buf();
    let workspace = workspace_root.to_path_buf();
    let outputs = outputs.to_vec();
    let staged_outputs = outputs.clone();
    let extraction_lease = lease.clone();
    let staging = tokio::task::spawn_blocking(move || {
        let _lease = extraction_lease;
        validate_headers(&archive, limits, provider)?;
        let mut staging = OutputStaging::create_limited(&workspace, limits)?;
        let modes = extract_output_archive(
            &archive,
            staging.files_directory(),
            &staged_outputs,
            provider,
            limits,
        )?;
        staging.set_directory_modes(modes);
        Ok::<_, Error>(staging)
    })
    .await
    .map_err(|source| archive_error(provider, format!("output archive task failed: {source}")))??;
    staging
        .install_admitted(outputs.as_slice(), workspace_root, lease)
        .await
}

fn validate_headers(path: &Path, limits: RemoteIoLimits, provider: &'static str) -> Result<()> {
    let file = std::fs::File::open(path)
        .map_err(|source| super::file_error("open_remote_output_archive", path, source))?;
    let size = file
        .metadata()
        .map_err(|source| super::file_error("read_remote_output_archive", path, source))?
        .len();
    super::super::http::check_size(size, limits.max_archive_bytes, provider, "archive bytes")?;
    let mut archive = tar::Archive::new(file);
    let entries = archive
        .entries()
        .map_err(|source| archive_error(provider, source.to_string()))?;
    let mut extension_bytes = 0_u64;
    for (index, entry) in entries.raw(true).enumerate() {
        check_entries(
            index.saturating_add(1),
            limits.max_archive_entries,
            provider,
        )?;
        let entry = entry.map_err(|source| archive_error(provider, source.to_string()))?;
        let kind = entry.header().entry_type();
        if kind.is_gnu_longname()
            || kind.is_gnu_longlink()
            || kind.is_pax_local_extensions()
            || kind.is_pax_global_extensions()
        {
            extension_bytes = extension_bytes.saturating_add(entry.size());
            super::super::http::check_size(
                extension_bytes,
                limits.max_response_bytes,
                provider,
                "archive metadata bytes",
            )?;
        }
    }
    Ok(())
}

fn extract_output_archive(
    archive_path: &Path,
    staging_root: &Directory,
    outputs: &[WorkspacePath],
    provider: &'static str,
    limits: RemoteIoLimits,
) -> Result<BTreeMap<PathBuf, u32>> {
    let declared_roots = outputs
        .iter()
        .map(|output| PathBuf::from(output.as_str()))
        .collect::<Vec<_>>();
    let file = std::fs::File::open(archive_path).map_err(|source| Error::FileAction {
        action: "open_remote_output_archive",
        path: archive_path.display().to_string(),
        source,
    })?;
    let mut archive = tar::Archive::new(file);
    let mut total_bytes = 0_u64;
    let mut directory_modes = BTreeMap::new();
    let mut budget = budget::ExtractionBudget::new(limits, provider);
    for (index, entry) in archive
        .entries()
        .map_err(|source| {
            archive_error(
                provider,
                format!("cannot read remote output archive: {source}"),
            )
        })?
        .enumerate()
    {
        check_entries(
            index.saturating_add(1),
            limits.max_archive_entries,
            provider,
        )?;
        let mut entry = entry.map_err(|source| {
            archive_error(
                provider,
                format!("cannot read remote output entry: {source}"),
            )
        })?;
        total_bytes = total_bytes.saturating_add(entry.size());
        super::super::http::check_size(
            total_bytes,
            limits.max_archive_bytes,
            provider,
            "extracted archive bytes",
        )?;
        if let Some((path, mode)) = extract_output_entry(
            &mut entry,
            staging_root,
            &declared_roots,
            provider,
            &mut budget,
            &directory_modes,
        )? {
            directory_modes.insert(path, mode);
        }
    }
    for output in outputs {
        staging_root
            .metadata(Path::new(output.as_str()))
            .map_err(|source| {
                archive_error(
                    provider,
                    format!(
                        "declared output `{}` was not returned: {source}",
                        output.as_str()
                    ),
                )
            })?;
    }
    Ok(directory_modes)
}

fn extract_output_entry(
    entry: &mut tar::Entry<'_, std::fs::File>,
    staging_root: &Directory,
    declared_roots: &[PathBuf],
    provider: &'static str,
    budget: &mut budget::ExtractionBudget,
    directory_modes: &BTreeMap<PathBuf, u32>,
) -> Result<Option<(PathBuf, u32)>> {
    let raw_path = entry.path().map_err(|source| {
        archive_error(
            provider,
            format!("remote output has an invalid path: {source}"),
        )
    })?;
    let relative = {
        let raw_path = raw_path.as_ref();
        let raw = raw_path.to_str().ok_or_else(|| {
            archive_error(
                provider,
                "remote output path is not valid UTF-8".to_string(),
            )
        })?;
        let path = WorkspacePath::try_from(raw)
            .map_err(|source| archive_error(provider, source.to_string()))?;
        let relative = PathBuf::from(path.as_str());
        if relative.as_os_str().is_empty() || !path_is_declared(&relative, declared_roots) {
            return Err(archive_error(
                provider,
                format!(
                    "remote output `{}` is outside declared output trees",
                    raw_path.display()
                ),
            ));
        }
        relative
    };
    let kind = entry.header().entry_type();
    if !(kind.is_file() || kind.is_dir() || kind.is_symlink()) {
        return Err(archive_error(
            provider,
            format!(
                "remote output `{}` has an unsupported type",
                relative.display()
            ),
        ));
    }
    let directory_mode = if kind.is_dir() {
        Some(
            entry
                .header()
                .mode()
                .map_err(|source| archive_error(provider, source.to_string()))?,
        )
    } else {
        None
    };
    let target = if kind.is_symlink() {
        Some(
            entry
                .link_name()
                .map_err(|source| archive_error(provider, source.to_string()))?
                .ok_or_else(|| {
                    archive_error(provider, "symbolic link has no target".to_string())
                })?,
        )
    } else {
        None
    };
    budget.charge(
        staging_root,
        &relative,
        kind.is_dir() && !directory_modes.contains_key(&relative),
        target.as_deref(),
    )?;
    let extracted = if let Some(target) = target {
        staging_root.symlink(&relative, &target)
    } else if kind.is_dir() {
        staging_root.descend(&relative, true).map(|_| ())
    } else {
        let mode = entry
            .header()
            .mode()
            .map_err(|source| archive_error(provider, source.to_string()))?;
        staging_root
            .create_file(&relative, mode)
            .and_then(|mut file| std::io::copy(entry, &mut file).map(|_| ()))
    };
    extracted.map_err(|source| {
        archive_error(
            provider,
            format!(
                "cannot extract remote output `{}`: {source}",
                relative.display()
            ),
        )
    })?;
    Ok(directory_mode.map(|mode| (relative, mode)))
}

#[cfg(all(test, any(target_os = "linux", target_vendor = "apple")))]
mod tests {
    use super::*;

    pub(super) fn write_archive(path: &Path, entries: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let mut archive = tar::Builder::new(file);
        for (entry_path, contents) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(u64::try_from(contents.len()).unwrap());
            header.set_mode(0o644);
            header.set_cksum();
            archive
                .append_data(&mut header, entry_path, *contents)
                .unwrap();
        }
        archive.finish().unwrap();
    }

    #[tokio::test]
    async fn output_limits_fail_before_publication() {
        let workspace = tempfile::tempdir().unwrap();
        let archive = workspace.path().join("outputs.tar");
        write_archive(&archive, &[("./reports/a", b"a"), ("./reports/b", b"b")]);
        std::fs::create_dir(workspace.path().join("reports")).unwrap();
        std::fs::write(workspace.path().join("reports/original"), b"original").unwrap();
        let outputs = [WorkspacePath::try_from("reports").unwrap()];
        for limits in [
            RemoteIoLimits {
                max_archive_bytes: 1,
                ..RemoteIoLimits::default()
            },
            RemoteIoLimits {
                max_archive_entries: 1,
                ..RemoteIoLimits::default()
            },
            RemoteIoLimits {
                max_metadata_bytes: 1,
                ..RemoteIoLimits::default()
            },
        ] {
            let error = install_output_archive_limited(
                &archive,
                workspace.path(),
                &outputs,
                "test",
                limits,
                PhasePermit::default(),
            )
            .await
            .unwrap_err();
            assert!(matches!(error, Error::ExecutorResourceLimit { .. }));
            assert_eq!(
                std::fs::read(workspace.path().join("reports/original")).unwrap(),
                b"original"
            );
            assert!(!workspace.path().join("reports/a").exists());
        }
    }

    #[tokio::test]
    async fn rejects_large_extension_headers_before_tar_buffers_them() {
        let workspace = tempfile::tempdir().unwrap();
        let archive = workspace.path().join("outputs.tar");
        let file = std::fs::File::create(&archive).unwrap();
        let mut builder = tar::Builder::new(file);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::XHeader);
        header.set_size(1024);
        header.set_cksum();
        builder
            .append_data(&mut header, "extension", &vec![b'x'; 1024][..])
            .unwrap();
        builder.finish().unwrap();
        let limits = RemoteIoLimits {
            max_response_bytes: 64,
            ..RemoteIoLimits::default()
        };
        let error = install_output_archive_limited(
            &archive,
            workspace.path(),
            &[],
            "test",
            limits,
            PhasePermit::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            Error::ExecutorResourceLimit {
                resource: "archive metadata bytes",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn installs_only_declared_outputs() {
        let workspace = tempfile::tempdir().unwrap();
        let archive = workspace.path().join("outputs.tar");
        write_archive(&archive, &[("./reports/result.txt", b"passed\n")]);
        let outputs = vec![WorkspacePath::try_from("reports").unwrap()];

        install_output_archive(&archive, workspace.path(), &outputs, "test")
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(workspace.path().join("reports/result.txt")).unwrap(),
            "passed\n"
        );
    }

    #[tokio::test]
    async fn rejects_outputs_outside_declared_trees() {
        let workspace = tempfile::tempdir().unwrap();
        let archive = workspace.path().join("outputs.tar");
        write_archive(&archive, &[("secrets.txt", b"no")]);
        let outputs = vec![WorkspacePath::try_from("reports").unwrap()];

        let error = install_output_archive(&archive, workspace.path(), &outputs, "test")
            .await
            .unwrap_err();

        assert!(error.to_string().contains("outside declared output trees"));
        assert!(!workspace.path().join("secrets.txt").exists());
    }
}

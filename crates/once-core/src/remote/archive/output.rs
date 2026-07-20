use std::path::{Path, PathBuf};

use super::archive_error;
use crate::remote::output_install::OutputStaging;
use crate::remote::path::{path_is_declared, relative_link_stays_within};
use crate::{Error, Result, WorkspacePath};

pub(in crate::remote) async fn install_output_archive(
    archive_path: &Path,
    workspace_root: &Path,
    outputs: &[WorkspacePath],
    provider: &'static str,
) -> Result<()> {
    let archive = archive_path.to_path_buf();
    let workspace = workspace_root.to_path_buf();
    let outputs = outputs.to_vec();
    let staged_outputs = outputs.clone();
    let staging = tokio::task::spawn_blocking(move || {
        let staging = OutputStaging::create(&workspace)?;
        extract_output_archive(&archive, staging.files(), &staged_outputs, provider)?;
        Ok::<_, Error>(staging)
    })
    .await
    .map_err(|source| archive_error(provider, format!("output archive task failed: {source}")))??;
    staging.install(outputs.as_slice(), workspace_root).await
}

fn extract_output_archive(
    archive_path: &Path,
    staging_root: &Path,
    outputs: &[WorkspacePath],
    provider: &'static str,
) -> Result<()> {
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
    for entry in archive.entries().map_err(|source| {
        archive_error(
            provider,
            format!("cannot read remote output archive: {source}"),
        )
    })? {
        let mut entry = entry.map_err(|source| {
            archive_error(
                provider,
                format!("cannot read remote output entry: {source}"),
            )
        })?;
        extract_output_entry(&mut entry, staging_root, &declared_roots, provider)?;
    }
    for output in outputs {
        let path = output.resolve(staging_root);
        std::fs::symlink_metadata(&path).map_err(|source| {
            archive_error(
                provider,
                format!(
                    "declared output `{}` was not returned: {source}",
                    output.as_str()
                ),
            )
        })?;
    }
    Ok(())
}

fn extract_output_entry(
    entry: &mut tar::Entry<'_, std::fs::File>,
    staging_root: &Path,
    declared_roots: &[PathBuf],
    provider: &'static str,
) -> Result<()> {
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
    if kind.is_symlink() {
        let target = entry
            .link_name()
            .map_err(|source| archive_error(provider, source.to_string()))?
            .ok_or_else(|| archive_error(provider, "symbolic link has no target".to_string()))?;
        if !relative_link_stays_within(&relative, &target, declared_roots) {
            return Err(archive_error(
                provider,
                format!(
                    "output symbolic link `{}` targets `{}` outside declared output trees",
                    relative.display(),
                    target.display()
                ),
            ));
        }
    }
    let unpacked = entry.unpack_in(staging_root).map_err(|source| {
        archive_error(
            provider,
            format!(
                "cannot extract remote output `{}`: {source}",
                relative.display()
            ),
        )
    })?;
    if !unpacked {
        return Err(archive_error(
            provider,
            format!("remote output `{}` escaped staging", relative.display()),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_archive(path: &Path, entries: &[(&str, &[u8])]) {
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

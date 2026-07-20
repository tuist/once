use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::{archive_error, file_error, TempArchive};
use crate::remote::path::relative_link_stays_within;
use crate::{Result, WorkspacePath};

pub(in crate::remote) async fn create_input_archive(
    workspace_root: &Path,
    inputs: &[WorkspacePath],
    provider: &'static str,
) -> Result<TempArchive> {
    let workspace = workspace_root.to_path_buf();
    let inputs = inputs.to_vec();
    tokio::task::spawn_blocking(move || build_input_archive(&workspace, &inputs, provider))
        .await
        .map_err(|source| archive_error(provider, format!("input archive task failed: {source}")))?
}

fn build_input_archive(
    workspace_root: &Path,
    inputs: &[WorkspacePath],
    provider: &'static str,
) -> Result<TempArchive> {
    let parent = workspace_root.join(".once/tmp");
    std::fs::create_dir_all(&parent).map_err(|source| crate::Error::FileAction {
        action: "create_remote_archive_directory",
        path: ".once/tmp".to_string(),
        source,
    })?;
    let file =
        tempfile::NamedTempFile::new_in(parent).map_err(|source| crate::Error::FileAction {
            action: "create_remote_input_archive",
            path: ".once/tmp".to_string(),
            source,
        })?;
    let declared_roots = inputs
        .iter()
        .map(|input| PathBuf::from(input.as_str()))
        .collect::<Vec<_>>();
    let mut entries = BTreeMap::new();
    for input in inputs {
        collect_entries(
            &input.resolve(workspace_root),
            Path::new(input.as_str()),
            &mut entries,
        )?;
    }

    let mut builder = tar::Builder::new(file);
    builder.follow_symlinks(false);
    for (relative, source) in entries {
        if relative.as_os_str().is_empty() {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&source)
            .map_err(|source_error| file_error("read_remote_input", &relative, source_error))?;
        if metadata.file_type().is_symlink() {
            let target = std::fs::read_link(&source).map_err(|source_error| {
                file_error("read_remote_input_link", &relative, source_error)
            })?;
            if !relative_link_stays_within(&relative, &target, &declared_roots) {
                return Err(archive_error(
                    provider,
                    format!(
                        "input symbolic link `{}` targets `{}` outside declared input trees",
                        relative.display(),
                        target.display()
                    ),
                ));
            }
        }
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            builder
                .append_dir(&relative, &source)
                .map_err(|source_error| {
                    file_error("archive_remote_input", &relative, source_error)
                })?;
        } else if metadata.is_file() || metadata.file_type().is_symlink() {
            builder
                .append_path_with_name(&source, &relative)
                .map_err(|source_error| {
                    file_error("archive_remote_input", &relative, source_error)
                })?;
        } else {
            return Err(archive_error(
                provider,
                format!(
                    "input `{}` is not a file, directory, or symbolic link",
                    relative.display()
                ),
            ));
        }
    }
    builder.finish().map_err(|source| {
        file_error(
            "finish_remote_input_archive",
            Path::new(".once/tmp"),
            source,
        )
    })?;
    let file = builder.into_inner().map_err(|source| {
        file_error(
            "finish_remote_input_archive",
            Path::new(".once/tmp"),
            source,
        )
    })?;
    let len = file
        .as_file()
        .metadata()
        .map_err(|source| file_error("read_remote_input_archive", Path::new(".once/tmp"), source))?
        .len();
    Ok(TempArchive {
        path: file.into_temp_path(),
        len,
    })
}

fn collect_entries(
    source: &Path,
    relative: &Path,
    entries: &mut BTreeMap<PathBuf, PathBuf>,
) -> Result<()> {
    let metadata = std::fs::symlink_metadata(source)
        .map_err(|source_error| file_error("read_remote_input", relative, source_error))?;
    entries.insert(relative.to_path_buf(), source.to_path_buf());
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Ok(());
    }
    let mut children = std::fs::read_dir(source)
        .map_err(|source_error| file_error("read_remote_input_directory", relative, source_error))?
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|source_error| {
            file_error("read_remote_input_directory", relative, source_error)
        })?;
    children.sort_by_key(std::fs::DirEntry::file_name);
    for child in children {
        let name = child.file_name();
        let child_relative = relative.join(name);
        collect_entries(&child.path(), &child_relative, entries)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn input_archive_preserves_a_dependency_tree() {
        let workspace = tempfile::tempdir().unwrap();
        let bin = workspace.path().join("node_modules/.bin");
        let package = workspace.path().join("node_modules/vitest");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("runner.mjs"), "console.log('ok')").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("../vitest/runner.mjs", bin.join("vitest")).unwrap();

        let archive = create_input_archive(
            workspace.path(),
            &[WorkspacePath::try_from("node_modules").unwrap()],
            "test",
        )
        .await
        .unwrap();
        let destination = tempfile::tempdir().unwrap();
        tar::Archive::new(std::fs::File::open(archive.path()).unwrap())
            .unpack(destination.path())
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(destination.path().join("node_modules/vitest/runner.mjs"))
                .unwrap(),
            "console.log('ok')"
        );
        #[cfg(unix)]
        assert_eq!(
            std::fs::read_link(destination.path().join("node_modules/.bin/vitest")).unwrap(),
            PathBuf::from("../vitest/runner.mjs")
        );
    }
}

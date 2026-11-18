use std::path::{Path, PathBuf};

use super::{validate_links, Directory};
use crate::{RemoteIoLimits, Result, WorkspacePath};

pub(in crate::remote) fn validate_output_tree(
    directory: &Directory,
    outputs: &[WorkspacePath],
    limits: RemoteIoLimits,
    retained_metadata_bytes: u64,
) -> Result<()> {
    let roots = outputs
        .iter()
        .map(|output| PathBuf::from(output.as_str()))
        .collect::<Vec<_>>();
    let mut links = Vec::new();
    let mut metadata_bytes = retained_metadata_bytes;
    let mut entries = 0_usize;
    for root in &roots {
        collect_links(
            directory,
            root,
            &mut links,
            &mut metadata_bytes,
            &mut entries,
            limits,
        )?;
    }
    validate_links(directory, &links, &roots).map_err(|source| error(Path::new(""), source))
}

fn collect_links(
    directory: &Directory,
    path: &Path,
    links: &mut Vec<PathBuf>,
    metadata_bytes: &mut u64,
    entries: &mut usize,
    limits: RemoteIoLimits,
) -> Result<()> {
    *entries = entries.saturating_add(1);
    crate::remote::http::check_size(
        *entries as u64,
        limits.max_archive_entries as u64,
        "remote",
        "archive entries",
    )?;
    *metadata_bytes = metadata_bytes
        .saturating_add(path.as_os_str().len() as u64)
        .saturating_add(256);
    let metadata = directory
        .metadata(path)
        .map_err(|source| error(path, source))?;
    let target = if metadata.is_symlink() {
        let target = directory
            .read_link(path)
            .map_err(|source| error(path, source))?;
        *metadata_bytes = metadata_bytes.saturating_add(target.as_os_str().len() as u64);
        Some(target)
    } else {
        None
    };
    crate::remote::http::check_size(
        *metadata_bytes,
        limits.max_metadata_bytes,
        "remote",
        "output metadata bytes",
    )?;
    if target.is_some() {
        links.push(path.to_path_buf());
    } else if metadata.is_dir() {
        let children = directory
            .descend(path, false)
            .map_err(|source| error(path, source))?;
        for child in children.entries().map_err(|source| error(path, source))? {
            collect_links(
                directory,
                &path.join(child.map_err(|source| error(path, source))?),
                links,
                metadata_bytes,
                entries,
                limits,
            )?;
        }
    } else if !metadata.is_file() {
        return Err(error(
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "unsupported remote output type",
            ),
        ));
    }
    Ok(())
}

fn error(path: &Path, source: std::io::Error) -> crate::Error {
    crate::Error::FileAction {
        action: "validate_remote_output_links",
        path: path.display().to_string(),
        source,
    }
}

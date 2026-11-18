use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use crate::ArchiveEntry;

use super::{invalid_input, ResolvedEntry, ResolvedSource};

pub(super) fn reject_output_overlap(
    entries: &[ArchiveEntry],
    resolved: &BTreeMap<String, ResolvedEntry>,
    outputs: &[&Path],
    workspace_root: &Path,
) -> io::Result<()> {
    let sources = entries
        .iter()
        .filter_map(|entry| entry.source.as_ref())
        .map(|source| source.resolve(workspace_root))
        .chain(resolved.values().filter_map(|entry| match &entry.source {
            ResolvedSource::File(path) => Some(path.clone()),
            _ => None,
        }))
        .map(|source| source.canonicalize())
        .collect::<io::Result<Vec<_>>>()?;
    let mut destinations = Vec::new();
    for output in outputs {
        if std::fs::symlink_metadata(output).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Err(invalid_input("archive outputs must not be symbolic links"));
        }
        let output = crate::execute::canonicalize_existing_prefix(output);
        for other in sources.iter().chain(&destinations) {
            if output.starts_with(other)
                || other.starts_with(&output)
                || (output.exists() && other.exists() && same_file::is_same_file(&output, other)?)
            {
                return Err(invalid_input(format!(
                    "archive output `{}` overlaps an input or another output `{}`",
                    output.display(),
                    other.display(),
                )));
            }
        }
        destinations.push(output);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArchiveEntryKind, ArchiveFormat, WorkspacePath};

    #[test]
    fn archives_reject_overlapping_outputs_without_mutating_inputs() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("data")).unwrap();
        std::fs::write(root.path().join("data/a"), b"original").unwrap();
        let entries = [ArchiveEntry {
            kind: ArchiveEntryKind::Tree,
            source: Some(WorkspacePath::try_from("data").unwrap()),
            path: String::new(),
            target: None,
            mode: 0o644,
            directory_mode: 0o755,
            owner_id: 0,
            group_id: 0,
            mtime: 0,
        }];
        let archive = root.path().join("out.zip");
        let inside = root.path().join("data/out.zip");
        let input = root.path().join("data/a");
        let alias = root.path().join("hardlink");
        std::fs::hard_link(&input, &alias).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.path().join("data"), root.path().join("alias"))
                .unwrap();
            std::os::unix::fs::symlink(
                root.path().join("data/missing"),
                root.path().join("dangling"),
            )
            .unwrap();
        }
        let cases = vec![
            (inside.clone(), None, None),
            (archive.clone(), Some(inside.clone()), None),
            (archive.clone(), None, Some(inside.clone())),
            (archive.clone(), Some(archive.clone()), None),
            (input.clone(), None, None),
            (alias, None, None),
        ];
        #[cfg(unix)]
        let cases = {
            let mut cases = cases;
            cases.extend([
                (root.path().join("alias/out.zip"), None, None),
                (root.path().join("dangling"), None, None),
            ]);
            cases
        };
        for format in [ArchiveFormat::Zip, ArchiveFormat::Tar, ArchiveFormat::TarGz] {
            for (output, digest, uncompressed) in &cases {
                let result = super::super::write_blocking(
                    &entries,
                    output,
                    digest.as_deref(),
                    uncompressed.as_deref(),
                    format,
                    root.path(),
                );
                assert!(result.is_err(), "accepted {}", output.display());
                assert_eq!(std::fs::read(&input).unwrap(), b"original");
                assert!(!archive.exists());
                assert!(!inside.exists());
            }
        }
        std::fs::write(&inside, b"previous output").unwrap();
        assert!(super::super::write_blocking(
            &entries,
            &inside,
            None,
            None,
            ArchiveFormat::Zip,
            root.path(),
        )
        .is_err());
        assert_eq!(std::fs::read(inside).unwrap(), b"previous output");
    }
}

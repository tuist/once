use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufReader, Write};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest as _, Sha256};
use tar::{Builder, EntryType, Header};

use crate::{ArchiveEntry, ArchiveEntryKind, ArchiveFormat, Error, Result, WorkspacePath};

pub(crate) async fn write(
    entries: &[ArchiveEntry],
    output: &WorkspacePath,
    sha256_output: Option<&WorkspacePath>,
    format: ArchiveFormat,
    workspace_root: &Path,
) -> Result<()> {
    let entries = entries.to_vec();
    let output_path = output.resolve(workspace_root);
    let output_label = output.as_str().to_string();
    let sha256_path = sha256_output.map(|path| path.resolve(workspace_root));
    let sha256_label = sha256_output.map(|path| path.as_str().to_string());
    let workspace_root = workspace_root.to_path_buf();
    tokio::task::spawn_blocking(move || {
        write_blocking(
            &entries,
            &output_path,
            sha256_path.as_deref(),
            format,
            &workspace_root,
        )
    })
    .await
    .map_err(|source| Error::FileAction {
        action: "write_archive",
        path: output_label.clone(),
        source: io::Error::other(source.to_string()),
    })?
    .map_err(|source| Error::FileAction {
        action: "write_archive",
        path: sha256_label.unwrap_or(output_label),
        source,
    })
}

fn write_blocking(
    entries: &[ArchiveEntry],
    output: &Path,
    sha256_output: Option<&Path>,
    format: ArchiveFormat,
    workspace_root: &Path,
) -> io::Result<()> {
    match format {
        ArchiveFormat::Tar => write_tar(entries, output, sha256_output, workspace_root),
    }
}

fn write_tar(
    entries: &[ArchiveEntry],
    output: &Path,
    sha256_output: Option<&Path>,
    workspace_root: &Path,
) -> io::Result<()> {
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let resolved = resolve_entries(entries, workspace_root)?;
    let writer = DigestWriter::new(File::create(output)?);
    let mut builder = Builder::new(writer);
    for entry in resolved.values() {
        append_entry(&mut builder, entry)?;
    }
    builder.finish()?;
    let mut writer = builder.into_inner()?;
    writer.flush()?;
    if let Some(digest_path) = sha256_output {
        if let Some(parent) = digest_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(digest_path, format!("{}\n", writer.digest_hex()))?;
    }
    Ok(())
}

#[derive(Debug)]
enum ResolvedSource {
    File(PathBuf),
    Directory,
    Symlink(PathBuf),
}

#[derive(Debug)]
struct ResolvedEntry {
    path: String,
    source: ResolvedSource,
    mode: u32,
    owner_id: u64,
    group_id: u64,
    mtime: u64,
}

fn resolve_entries(
    entries: &[ArchiveEntry],
    workspace_root: &Path,
) -> io::Result<BTreeMap<String, ResolvedEntry>> {
    let mut resolved = BTreeMap::new();
    for entry in entries {
        match entry.kind {
            ArchiveEntryKind::File => {
                let source = required_source(entry)?.resolve(workspace_root);
                if !std::fs::symlink_metadata(&source)?.is_file() {
                    return Err(invalid_input(format!(
                        "archive file source `{}` is not a regular file",
                        source.display()
                    )));
                }
                insert_entry(
                    &mut resolved,
                    ResolvedEntry {
                        path: normalize_archive_path(&entry.path, false)?,
                        source: ResolvedSource::File(source),
                        mode: entry.mode,
                        owner_id: entry.owner_id,
                        group_id: entry.group_id,
                        mtime: entry.mtime,
                    },
                )?;
            }
            ArchiveEntryKind::Directory => {
                if entry.source.is_some() {
                    return Err(invalid_input(format!(
                        "archive directory `{}` must not declare a source",
                        entry.path
                    )));
                }
                insert_entry(
                    &mut resolved,
                    ResolvedEntry {
                        path: normalize_archive_path(&entry.path, false)?,
                        source: ResolvedSource::Directory,
                        mode: entry.directory_mode,
                        owner_id: entry.owner_id,
                        group_id: entry.group_id,
                        mtime: entry.mtime,
                    },
                )?;
            }
            ArchiveEntryKind::Tree => {
                let source = required_source(entry)?.resolve(workspace_root);
                if !std::fs::symlink_metadata(&source)?.is_dir() {
                    return Err(invalid_input(format!(
                        "archive tree source `{}` is not a directory",
                        source.display()
                    )));
                }
                let destination = normalize_archive_path(&entry.path, true)?;
                if !destination.is_empty() {
                    insert_entry(
                        &mut resolved,
                        ResolvedEntry {
                            path: destination.clone(),
                            source: ResolvedSource::Directory,
                            mode: entry.directory_mode,
                            owner_id: entry.owner_id,
                            group_id: entry.group_id,
                            mtime: entry.mtime,
                        },
                    )?;
                }
                collect_tree(&source, &destination, entry, &mut resolved)?;
            }
        }
    }
    Ok(resolved)
}

fn required_source(entry: &ArchiveEntry) -> io::Result<&WorkspacePath> {
    entry.source.as_ref().ok_or_else(|| {
        invalid_input(format!(
            "archive {:?} entry `{}` requires a source",
            entry.kind, entry.path
        ))
    })
}

fn collect_tree(
    source: &Path,
    destination: &str,
    entry: &ArchiveEntry,
    resolved: &mut BTreeMap<String, ResolvedEntry>,
) -> io::Result<()> {
    let mut children = std::fs::read_dir(source)?.collect::<io::Result<Vec<_>>>()?;
    children.sort_by_key(std::fs::DirEntry::file_name);
    for child in children {
        let child_source = child.path();
        let name = child.file_name().into_string().map_err(|name| {
            invalid_input(format!(
                "archive tree contains a non-UTF-8 path component: {}",
                name.to_string_lossy()
            ))
        })?;
        if name.contains('\\') {
            return Err(invalid_input(format!(
                "archive tree path component `{name}` contains a backslash"
            )));
        }
        let child_destination = if destination.is_empty() {
            name
        } else {
            format!("{destination}/{name}")
        };
        let metadata = std::fs::symlink_metadata(&child_source)?;
        let (source, mode) = if metadata.file_type().is_symlink() {
            (
                ResolvedSource::Symlink(std::fs::read_link(&child_source)?),
                0o777,
            )
        } else if metadata.is_dir() {
            (ResolvedSource::Directory, entry.directory_mode)
        } else if metadata.is_file() {
            (ResolvedSource::File(child_source.clone()), entry.mode)
        } else {
            return Err(invalid_input(format!(
                "archive tree entry `{}` has an unsupported file type",
                child_source.display()
            )));
        };
        insert_entry(
            resolved,
            ResolvedEntry {
                path: child_destination.clone(),
                source,
                mode,
                owner_id: entry.owner_id,
                group_id: entry.group_id,
                mtime: entry.mtime,
            },
        )?;
        if metadata.is_dir() {
            collect_tree(&child_source, &child_destination, entry, resolved)?;
        }
    }
    Ok(())
}

fn insert_entry(
    entries: &mut BTreeMap<String, ResolvedEntry>,
    entry: ResolvedEntry,
) -> io::Result<()> {
    if entries.contains_key(&entry.path) {
        return Err(invalid_input(format!(
            "archive path `{}` is declared more than once",
            entry.path
        )));
    }
    entries.insert(entry.path.clone(), entry);
    Ok(())
}

fn normalize_archive_path(path: &str, allow_empty: bool) -> io::Result<String> {
    if path.contains('\\') {
        return Err(invalid_input(format!(
            "archive path `{path}` contains a backslash"
        )));
    }
    if path.starts_with('/') {
        return Err(invalid_input(format!(
            "archive path `{path}` must be relative"
        )));
    }
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        return if allow_empty {
            Ok(String::new())
        } else {
            Err(invalid_input("archive path must not be empty"))
        };
    }
    let mut parts = Vec::new();
    for component in Path::new(trimmed).components() {
        match component {
            Component::Normal(value) => {
                parts.push(value.to_str().ok_or_else(|| {
                    invalid_input(format!("archive path `{path}` contains non-UTF-8 text"))
                })?);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) | Component::RootDir => {
                return Err(invalid_input(format!(
                    "archive path `{path}` must stay relative"
                )));
            }
        }
    }
    if parts.is_empty() && !allow_empty {
        return Err(invalid_input("archive path must not be empty"));
    }
    Ok(parts.join("/"))
}

fn append_entry<W: Write>(builder: &mut Builder<W>, entry: &ResolvedEntry) -> io::Result<()> {
    let mut header = Header::new_gnu();
    header.set_mode(entry.mode);
    header.set_uid(entry.owner_id);
    header.set_gid(entry.group_id);
    header.set_mtime(entry.mtime);
    match &entry.source {
        ResolvedSource::File(source) => {
            let file = File::open(source)?;
            header.set_entry_type(EntryType::Regular);
            header.set_size(file.metadata()?.len());
            header.set_cksum();
            builder.append_data(&mut header, &entry.path, BufReader::new(file))?;
        }
        ResolvedSource::Directory => {
            header.set_entry_type(EntryType::Directory);
            header.set_size(0);
            header.set_cksum();
            builder.append_data(
                &mut header,
                format!("{}/", entry.path.trim_end_matches('/')),
                io::empty(),
            )?;
        }
        ResolvedSource::Symlink(target) => {
            header.set_entry_type(EntryType::Symlink);
            header.set_size(0);
            header.set_link_name(target)?;
            header.set_cksum();
            builder.append_data(&mut header, &entry.path, io::empty())?;
        }
    }
    Ok(())
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

struct DigestWriter<W> {
    inner: W,
    hasher: Sha256,
}

impl<W> DigestWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
        }
    }

    fn digest_hex(&self) -> String {
        hex_lower(&self.hasher.clone().finalize())
    }
}

impl<W: Write> Write for DigestWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buffer)?;
        self.hasher.update(&buffer[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WorkspacePath;

    #[test]
    fn deterministic_tar_uses_declared_metadata_and_digest() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("hello"), b"hello\n").unwrap();
        let entries = vec![
            ArchiveEntry {
                kind: ArchiveEntryKind::Directory,
                source: None,
                path: "usr/local/bin".to_string(),
                mode: 0,
                directory_mode: 0o755,
                owner_id: 7,
                group_id: 8,
                mtime: 9,
            },
            ArchiveEntry {
                kind: ArchiveEntryKind::File,
                source: Some(WorkspacePath::try_from("hello").unwrap()),
                path: "usr/local/bin/hello".to_string(),
                mode: 0o755,
                directory_mode: 0o755,
                owner_id: 7,
                group_id: 8,
                mtime: 9,
            },
        ];
        let output = temporary.path().join("layer.tar");
        let digest = temporary.path().join("layer.sha256");

        write_blocking(
            &entries,
            &output,
            Some(&digest),
            ArchiveFormat::Tar,
            temporary.path(),
        )
        .unwrap();
        let first = std::fs::read(&output).unwrap();
        write_blocking(
            &entries,
            &output,
            Some(&digest),
            ArchiveFormat::Tar,
            temporary.path(),
        )
        .unwrap();
        assert_eq!(first, std::fs::read(&output).unwrap());
        assert_eq!(
            std::fs::read_to_string(&digest).unwrap().trim(),
            hex_lower(&Sha256::digest(&first))
        );

        let mut archive = tar::Archive::new(first.as_slice());
        let entries = archive
            .entries()
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.path().unwrap().to_string_lossy().into_owned(),
                    entry.header().mode().unwrap(),
                    entry.header().uid().unwrap(),
                    entry.header().gid().unwrap(),
                    entry.header().mtime().unwrap(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            entries,
            vec![
                ("usr/local/bin/".to_string(), 0o755, 7, 8, 9),
                ("usr/local/bin/hello".to_string(), 0o755, 7, 8, 9),
            ]
        );
    }

    #[test]
    fn archive_paths_cannot_escape() {
        assert!(normalize_archive_path("../escape", false).is_err());
        assert!(normalize_archive_path("/escape", false).is_err());
        assert!(normalize_archive_path("windows\\escape", false).is_err());
    }
}

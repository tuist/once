//! The content-addressed input root of a remote action.

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use once_cas::{reapi, ReapiMessage as _};
use sha2::{Digest as _, Sha256};

use crate::remote::archive::input::plan::InputPlan;
use crate::remote::path::relative_link_stays_within;
use crate::{Error, RemoteIoLimits, Result, WorkspacePath};

use super::PROVIDER;

/// A blob the input root references and where its bytes come from.
pub(super) enum Blob {
    /// A workspace-relative file, reopened without following links to upload.
    File(PathBuf),
    Bytes(Vec<u8>),
}

pub(super) struct InputTree {
    pub root: reapi::Digest,
    pub blobs: BTreeMap<String, (reapi::Digest, Blob)>,
}

#[derive(Default)]
struct Node {
    files: BTreeMap<String, (reapi::Digest, bool)>,
    directories: BTreeSet<String>,
    symlinks: BTreeMap<String, String>,
}

/// Paths the input root must contain besides the declared inputs.
pub(super) struct Layout<'a> {
    pub inputs: &'a [WorkspacePath],
    /// Removed from the declared inputs before the command runs.
    pub clean_paths: &'a [WorkspacePath],
    /// Present even when they fall under a cleaned path.
    pub restored_inputs: &'a [WorkspacePath],
    /// Empty directories that exist before the command runs.
    pub directories: Vec<&'a str>,
}

pub(super) fn build(
    workspace: &Path,
    layout: &Layout<'_>,
    limits: RemoteIoLimits,
) -> Result<InputTree> {
    let mut nodes = BTreeMap::<PathBuf, Node>::new();
    nodes.insert(PathBuf::new(), Node::default());
    let mut blobs = BTreeMap::new();
    let cleaned = |path: &Path| {
        layout
            .clean_paths
            .iter()
            .any(|clean| path.starts_with(clean.as_str()))
    };
    for (inputs, keep_cleaned) in [(layout.inputs, false), (layout.restored_inputs, true)] {
        let plan = InputPlan::collect(workspace, inputs, PROVIDER, limits)?;
        for relative in plan.entries.keys() {
            if relative.as_os_str().is_empty() || (!keep_cleaned && cleaned(relative)) {
                continue;
            }
            add_entry(&plan, relative, &mut nodes, &mut blobs)?;
        }
    }
    for directory in &layout.directories {
        ensure_directory(&mut nodes, Path::new(directory))?;
    }
    let root = encode_directories(nodes, &mut blobs)?;
    Ok(InputTree { root, blobs })
}

fn add_entry(
    plan: &InputPlan,
    relative: &Path,
    nodes: &mut BTreeMap<PathBuf, Node>,
    blobs: &mut BTreeMap<String, (reapi::Digest, Blob)>,
) -> Result<()> {
    let metadata = plan
        .workspace
        .metadata(relative)
        .map_err(|source| file_error("read_remote_input", relative, source))?;
    if metadata.is_dir() {
        return ensure_directory(nodes, relative);
    }
    let parent = relative.parent().unwrap_or(Path::new(""));
    ensure_directory(nodes, parent)?;
    let name = file_name(relative)?;
    if metadata.is_symlink() {
        let target = plan
            .workspace
            .read_link(relative)
            .map_err(|source| file_error("read_remote_input_link", relative, source))?;
        if !relative_link_stays_within(relative, &target, &plan.declared_roots) {
            return Err(api_error(format!(
                "input symbolic link `{}` targets `{}` outside declared input trees",
                relative.display(),
                target.display()
            )));
        }
        let target = target.to_str().ok_or_else(|| {
            api_error(format!("input link `{}` is not UTF-8", relative.display()))
        })?;
        node(nodes, parent)
            .symlinks
            .insert(name, target.to_string());
        return Ok(());
    }
    if !metadata.is_file() {
        return Err(api_error(format!(
            "input `{}` is not a file, directory, or symbolic link",
            relative.display()
        )));
    }
    let mut file = plan
        .workspace
        .open_file(relative)
        .map_err(|source| file_error("open_remote_input", relative, source))?;
    let executable = file
        .metadata()
        .map_err(|source| file_error("read_remote_input", relative, source))?
        .permissions()
        .mode()
        & 0o111
        != 0;
    let digest = hash_reader(&mut file)
        .map_err(|source| file_error("hash_remote_input", relative, source))?;
    blobs
        .entry(key(&digest))
        .or_insert_with(|| (digest.clone(), Blob::File(relative.to_path_buf())));
    node(nodes, parent).files.insert(name, (digest, executable));
    Ok(())
}

fn ensure_directory(nodes: &mut BTreeMap<PathBuf, Node>, path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        let std::path::Component::Normal(part) = component else {
            return Err(api_error(format!(
                "invalid input directory `{}`",
                path.display()
            )));
        };
        let name = part
            .to_str()
            .ok_or_else(|| api_error(format!("input path `{}` is not UTF-8", path.display())))?
            .to_string();
        node(nodes, &current).directories.insert(name);
        current.push(part);
        nodes.entry(current.clone()).or_default();
    }
    Ok(())
}

fn node<'a>(nodes: &'a mut BTreeMap<PathBuf, Node>, path: &Path) -> &'a mut Node {
    nodes.entry(path.to_path_buf()).or_default()
}

/// Encode directories children first, so each parent can reference its
/// children's digests. Returns the root's digest.
fn encode_directories(
    nodes: BTreeMap<PathBuf, Node>,
    blobs: &mut BTreeMap<String, (reapi::Digest, Blob)>,
) -> Result<reapi::Digest> {
    let mut paths = nodes.keys().cloned().collect::<Vec<_>>();
    paths.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    let mut digests = BTreeMap::<PathBuf, reapi::Digest>::new();
    let mut nodes = nodes;
    for path in paths {
        let node = nodes.remove(&path).expect("every path has a node");
        for name in node.files.keys().chain(node.symlinks.keys()) {
            if node.directories.contains(name) {
                return Err(api_error(format!(
                    "input `{}` is both a directory and a file",
                    path.join(name).display()
                )));
            }
        }
        let directory = reapi::Directory {
            files: node
                .files
                .into_iter()
                .map(|(name, (digest, is_executable))| reapi::FileNode {
                    name,
                    digest: Some(digest),
                    is_executable,
                    node_properties: None,
                })
                .collect(),
            directories: node
                .directories
                .into_iter()
                .map(|name| {
                    let digest = digests
                        .remove(&path.join(&name))
                        .expect("children are encoded before their parent");
                    reapi::DirectoryNode {
                        name,
                        digest: Some(digest),
                    }
                })
                .collect(),
            symlinks: node
                .symlinks
                .into_iter()
                .map(|(name, target)| reapi::SymlinkNode {
                    name,
                    target,
                    node_properties: None,
                })
                .collect(),
            node_properties: None,
        };
        let bytes = directory.encode_to_vec();
        let digest = digest_bytes(&bytes);
        blobs
            .entry(key(&digest))
            .or_insert_with(|| (digest.clone(), Blob::Bytes(bytes)));
        digests.insert(path, digest);
    }
    Ok(digests
        .remove(Path::new(""))
        .expect("the root directory is always encoded"))
}

pub(super) fn digest_bytes(bytes: &[u8]) -> reapi::Digest {
    reapi::Digest {
        hash: hex::encode(Sha256::digest(bytes)),
        size_bytes: i64::try_from(bytes.len()).expect("blob length fits a digest"),
    }
}

pub(super) fn hash_reader(reader: &mut impl std::io::Read) -> std::io::Result<reapi::Digest> {
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; 256 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    Ok(reapi::Digest {
        hash: hex::encode(hasher.finalize()),
        size_bytes: i64::try_from(size).map_err(std::io::Error::other)?,
    })
}

pub(super) fn key(digest: &reapi::Digest) -> String {
    format!("{}/{}", digest.hash, digest.size_bytes)
}

fn file_name(path: &Path) -> Result<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .ok_or_else(|| api_error(format!("input path `{}` is not UTF-8", path.display())))
}

fn file_error(action: &'static str, path: &Path, source: std::io::Error) -> Error {
    Error::FileAction {
        action,
        path: path.display().to_string(),
        source,
    }
}

pub(super) fn api_error(message: String) -> Error {
    Error::RemoteProviderApi {
        provider: PROVIDER.to_string(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(tree: &InputTree, digest: &reapi::Digest) -> reapi::Directory {
        let Blob::Bytes(bytes) = &tree.blobs[&key(digest)].1 else {
            panic!("directories are encoded in memory");
        };
        reapi::Directory::decode(bytes.as_slice()).unwrap()
    }

    #[test]
    fn cleaned_inputs_are_omitted_unless_restored_and_setup_directories_exist() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join("out/keep")).unwrap();
        std::fs::write(workspace.path().join("src.rs"), b"fn main() {}").unwrap();
        std::fs::write(workspace.path().join("out/stale"), b"stale").unwrap();
        std::fs::write(workspace.path().join("out/keep/args.rsp"), b"--edition").unwrap();
        let path = |value: &str| WorkspacePath::try_from(value).unwrap();
        let tree = build(
            workspace.path(),
            &Layout {
                inputs: &[path("src.rs"), path("out")],
                clean_paths: &[path("out")],
                restored_inputs: &[path("out/keep/args.rsp")],
                directories: vec!["out", "build/tmp"],
            },
            RemoteIoLimits::default(),
        )
        .unwrap();
        let root = decode(&tree, &tree.root);
        assert_eq!(
            root.directories
                .iter()
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>(),
            ["build", "out"]
        );
        assert_eq!(root.files[0].name, "src.rs");
        let out = decode(&tree, root.directories[1].digest.as_ref().unwrap());
        assert!(out.files.is_empty(), "cleaned output must not be staged");
        let keep = decode(&tree, out.directories[0].digest.as_ref().unwrap());
        assert_eq!(keep.files[0].name, "args.rsp");
        assert_eq!(
            keep.files[0].digest.as_ref().unwrap(),
            &digest_bytes(b"--edition")
        );
        let build = decode(&tree, root.directories[0].digest.as_ref().unwrap());
        assert_eq!(build.directories[0].name, "tmp");
    }

    #[test]
    fn identical_trees_share_one_root_digest_and_modes_partition_it() {
        let build_root = |executable: bool| {
            let workspace = tempfile::tempdir().unwrap();
            let file = workspace.path().join("tool");
            std::fs::write(&file, b"#!/bin/sh\n").unwrap();
            let mode = if executable { 0o755 } else { 0o644 };
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
            build(
                workspace.path(),
                &Layout {
                    inputs: &[WorkspacePath::try_from("tool").unwrap()],
                    clean_paths: &[],
                    restored_inputs: &[],
                    directories: Vec::new(),
                },
                RemoteIoLimits::default(),
            )
            .unwrap()
            .root
        };
        assert_eq!(build_root(true), build_root(true));
        assert_ne!(build_root(true), build_root(false));
    }
}

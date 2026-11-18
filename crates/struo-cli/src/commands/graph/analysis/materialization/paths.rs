use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use struo_cas::ActionResult;
use struo_core::WorkspacePath;

pub(super) fn batch_len(results: &[ActionResult], workspace: &Path, limit: usize) -> usize {
    // Missing Windows paths can acquire short-name aliases only after creation.
    if cfg!(windows) || results.len() == 1 || limit <= 1 {
        return 1;
    }
    let mut paths = BTreeSet::new();
    let mut count = 0;
    for result in results.iter().take(limit.max(1)) {
        let Some(footprint) = footprint(result, workspace) else {
            break;
        };
        if footprint.iter().any(|path| overlaps(&paths, path)) {
            break;
        }
        paths.extend(footprint);
        count += 1;
    }
    count.max(1)
}

fn overlaps(paths: &BTreeSet<PathBuf>, candidate: &Path) -> bool {
    candidate.ancestors().any(|parent| paths.contains(parent))
        || paths
            .range(candidate.to_path_buf()..)
            .next()
            .is_some_and(|path| path.starts_with(candidate))
}

pub(super) fn disjoint(
    workspace: &Path,
    left: &BTreeSet<String>,
    right: &BTreeSet<String>,
) -> bool {
    if left.is_empty() || right.is_empty() {
        return true;
    }
    if cfg!(windows) {
        return false;
    }
    let Some(left) = footprint_paths(left.iter().map(String::as_str), workspace) else {
        return false;
    };
    let Some(right) = footprint_paths(right.iter().map(String::as_str), workspace) else {
        return false;
    };
    right.iter().all(|path| !overlaps(&left, path))
}

pub(super) fn disjoint_from_results<'a>(
    workspace: &Path,
    candidate: &ActionResult,
    active: impl Iterator<Item = &'a ActionResult>,
) -> bool {
    if cfg!(windows) {
        return false;
    }
    let Some(candidate) = footprint(candidate, workspace) else {
        return false;
    };
    active.into_iter().all(|result| {
        footprint(result, workspace)
            .is_some_and(|other| candidate.iter().all(|path| !overlaps(&other, path)))
    })
}

pub(super) fn independent_members<'a>(
    workspace: &Path,
    paths: &BTreeSet<String>,
    candidates: impl Iterator<Item = &'a str>,
) -> BTreeSet<&'a str> {
    let mut candidates = candidates.peekable();
    if candidates.peek().is_none() {
        return BTreeSet::new();
    }
    if paths.len() == 1 {
        return candidates.filter(|path| paths.contains(*path)).collect();
    }
    if cfg!(windows) {
        return BTreeSet::new();
    }
    let footprints = paths
        .iter()
        .map(|path| {
            footprint_paths(std::iter::once(path.as_str()), workspace)
                .map(|footprint| (path.as_str(), footprint))
        })
        .collect::<Option<std::collections::BTreeMap<_, _>>>();
    let Some(footprints) = footprints else {
        return BTreeSet::new();
    };
    candidates
        .filter(|path| {
            footprints.get(*path).is_some_and(|own| {
                footprints.iter().all(|(other, footprint)| {
                    *other == *path || own.iter().all(|path| !overlaps(footprint, path))
                })
            })
        })
        .collect()
}

pub(super) fn creation_conflicts(
    workspace: &Path,
    created: &str,
    pending: &BTreeSet<String>,
) -> bool {
    if pending.is_empty() {
        return false;
    }
    if cfg!(windows) {
        return true;
    }
    let Some(created) = footprint_paths(std::iter::once(created), workspace) else {
        return true;
    };
    let Some(pending) = footprint_paths(pending.iter().map(String::as_str), workspace) else {
        return true;
    };
    created
        .iter()
        .any(|path| path.ancestors().any(|parent| pending.contains(parent)))
}

fn footprint(result: &ActionResult, workspace: &Path) -> Option<BTreeSet<PathBuf>> {
    footprint_paths(result.outputs.keys().map(String::as_str), workspace)
}

fn footprint_paths<'a>(
    outputs: impl Iterator<Item = &'a str>,
    workspace: &Path,
) -> Option<BTreeSet<PathBuf>> {
    let mut paths = BTreeSet::new();
    for output in outputs {
        let logical = WorkspacePath::try_from(output).ok()?.resolve(workspace);
        let canonical = resolve_existing_ancestors(&logical)?;
        let logical = logical.to_str()?;
        let canonical = canonical.to_str()?;
        if !logical.is_ascii() || !canonical.is_ascii() {
            return None;
        }
        paths.insert(PathBuf::from(logical.to_ascii_lowercase()));
        paths.insert(PathBuf::from(canonical.to_ascii_lowercase()));
    }
    Some(paths)
}

fn resolve_existing_ancestors(path: &Path) -> Option<PathBuf> {
    #[cfg(test)]
    RESOLUTIONS.set(RESOLUTIONS.get() + 1);
    let mut ancestor = path;
    let mut suffix = Vec::new();
    loop {
        match ancestor.canonicalize() {
            Ok(mut canonical) => {
                for component in suffix.iter().rev() {
                    canonical.push(component);
                }
                return Some(canonical);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match ancestor.symlink_metadata() {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    _ => return None,
                }
                suffix.push(ancestor.file_name()?.to_os_string());
                ancestor = ancestor.parent()?;
            }
            Err(_) => return None,
        }
    }
}

#[cfg(test)]
thread_local! {
    static RESOLUTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
#[path = "paths_tests.rs"]
mod tests;

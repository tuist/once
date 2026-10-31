use std::collections::VecDeque;
use std::path::{Component, Path, PathBuf};

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
mod boundary;
#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
#[path = "path/unsupported.rs"]
mod boundary;
pub(super) use boundary::{Directory, Parent};
mod tree;
pub(super) use tree::validate_output_tree;

pub(super) fn validate_links(
    directory: &Directory,
    links: &[PathBuf],
    declared_roots: &[PathBuf],
) -> std::io::Result<()> {
    for link in links {
        let mut pending = components(link)?;
        let mut resolved = PathBuf::new();
        let mut followed = 0;
        while let Some(component) = pending.pop_front() {
            if component == ".." {
                if !resolved.pop() {
                    return Err(unsafe_link(link));
                }
            } else {
                resolved.push(&component);
                if !path_is_declared(&resolved, declared_roots)
                    && !declared_roots
                        .iter()
                        .any(|root| root.starts_with(&resolved))
                {
                    return Err(unsafe_link(link));
                }
                let metadata = match directory.metadata(&resolved) {
                    Ok(metadata) => Some(metadata),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(error) => return Err(error),
                };
                if metadata.is_some_and(|metadata| metadata.is_symlink()) {
                    followed += 1;
                    if followed > 40 {
                        return Err(unsafe_link(link));
                    }
                    let target = directory.read_link(&resolved)?;
                    resolved.pop();
                    let mut expanded = components(&target)?;
                    expanded.append(&mut pending);
                    pending = expanded;
                }
            }
        }
        if !path_is_declared(&resolved, declared_roots) {
            return Err(unsafe_link(link));
        }
    }
    Ok(())
}

fn components(path: &Path) -> std::io::Result<VecDeque<std::ffi::OsString>> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(Ok(part.to_owned())),
            Component::ParentDir => Some(Ok("..".into())),
            Component::CurDir => None,
            _ => Some(Err(unsafe_link(path))),
        })
        .collect()
}

fn unsafe_link(link: &Path) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        format!(
            "symbolic link `{}` escapes declared trees or contains a cycle",
            link.display()
        ),
    )
}

pub(super) fn relative_link_stays_within(
    link: &Path,
    target: &Path,
    declared_roots: &[PathBuf],
) -> bool {
    if target.is_absolute() {
        return false;
    }
    let Some(parent) = link.parent() else {
        return false;
    };
    let Some(resolved) = normalize(&parent.join(target)) else {
        return false;
    };
    declared_roots.iter().any(|root| resolved.starts_with(root))
}

pub(super) fn path_is_declared(path: &Path, declared_roots: &[PathBuf]) -> bool {
    declared_roots.iter().any(|root| path.starts_with(root))
}

fn normalize(path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return None;
                }
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_links_within_a_declared_tree() {
        assert!(relative_link_stays_within(
            Path::new("node_modules/.bin/vitest"),
            Path::new("../vitest/bin.mjs"),
            &[PathBuf::from("node_modules")],
        ));
    }

    #[test]
    fn rejects_links_that_escape_declared_trees() {
        assert!(!relative_link_stays_within(
            Path::new("node_modules/tool"),
            Path::new("../../secret"),
            &[PathBuf::from("node_modules")],
        ));
        assert!(!relative_link_stays_within(
            Path::new("node_modules/tool"),
            Path::new("/host/secret"),
            &[PathBuf::from("node_modules")],
        ));
    }
}

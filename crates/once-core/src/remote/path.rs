use std::path::{Component, Path, PathBuf};

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

//! The [`Target`] record produced by loading a `fabrik.toml` file.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::target_ref::target_id;

/// A target declared by a `fabrik.toml` file.
///
/// `package` is the workspace-relative directory holding the build file
/// (forward-slash separated, empty string for the workspace root).
/// `srcs` are package-relative paths.
/// `attrs` carries target-kind-specific string settings.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Target {
    pub package: String,
    pub kind: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub srcs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attrs: BTreeMap<String, String>,
}

impl Target {
    /// The project-root-relative id for this target.
    #[must_use]
    pub fn id(&self) -> String {
        target_id(&self.package, &self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_uses_project_relative_path() {
        let t = Target {
            package: "crates/foo".into(),
            kind: "rust_binary".into(),
            name: "bar".into(),
            srcs: vec![],
            deps: vec![],
            attrs: BTreeMap::new(),
        };
        assert_eq!(t.id(), "crates/foo/bar");
        let root_t = Target {
            package: String::new(),
            ..t
        };
        assert_eq!(root_t.id(), "bar");
    }
}

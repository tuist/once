//! The [`Target`] record produced by loading a `fabrik.toml` file.

use std::borrow::Cow;
use std::collections::BTreeMap;

use serde::Serialize;

use crate::dependency::external_target_id;
use crate::target_ref::target_id;

/// A target declared by a `fabrik.toml` file.
///
/// `package` is the workspace-relative directory holding the build file
/// (forward-slash separated, empty string for the workspace root).
/// `external_package` is set for generated external targets that are
/// loaded from `.fabrik/external` but addressed outside the workspace
/// target namespace.
/// `srcs` are package-relative paths.
/// `attrs` carries target-kind-specific string settings.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ExternalDependency {
    pub graph: String,
    pub spec: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Target {
    pub package: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_package: Option<String>,
    pub kind: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub srcs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_deps: Vec<ExternalDependency>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attrs: BTreeMap<String, String>,
}

impl Target {
    /// The canonical id for this target.
    #[must_use]
    pub fn id(&self) -> String {
        self.external_package.as_ref().map_or_else(
            || target_id(&self.package, &self.name),
            |external_package| external_target_id(external_package, &self.name),
        )
    }

    #[must_use]
    pub fn output_package(&self) -> Cow<'_, str> {
        self.external_package.as_ref().map_or_else(
            || Cow::Borrowed(self.package.as_str()),
            |external_package| Cow::Owned(format!("external/{external_package}")),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_uses_project_relative_path() {
        let t = Target {
            package: "crates/foo".into(),
            external_package: None,
            kind: "rust_binary".into(),
            name: "bar".into(),
            srcs: vec![],
            deps: vec![],
            external_deps: vec![],
            attrs: BTreeMap::new(),
        };
        assert_eq!(t.id(), "crates/foo/bar");
        let root_t = Target {
            package: String::new(),
            ..t
        };
        assert_eq!(root_t.id(), "bar");
    }

    #[test]
    fn id_uses_external_package_when_present() {
        let t = Target {
            package: ".fabrik/external/cargo".into(),
            external_package: Some("cargo".into()),
            kind: "rust_library".into(),
            name: "serde".into(),
            srcs: vec![],
            deps: vec![],
            external_deps: vec![],
            attrs: BTreeMap::new(),
        };

        assert_eq!(t.id(), "external:cargo/serde");
        assert_eq!(t.output_package(), "external/cargo");
    }
}

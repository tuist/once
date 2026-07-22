//! The [`Target`] record produced by loading a `once.toml` file.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::target_ref::target_id;

/// A script-like target declared by a `once.toml` file.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Target {
    pub package: String,
    pub kind: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependency_edges: BTreeMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub srcs: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attrs: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub typed_attrs: BTreeMap<String, AttrValue>,
}

impl Target {
    /// The canonical id for this target.
    #[must_use]
    pub fn id(&self) -> String {
        target_id(&self.package, &self.name)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum AttrValue {
    String(String),
    Integer(i64),
    Float(f64),
    Bool(bool),
    List(Vec<AttrValue>),
    Map(BTreeMap<String, AttrValue>),
}

impl AttrValue {
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }
}

impl PartialEq for AttrValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::String(lhs), Self::String(rhs)) => lhs == rhs,
            (Self::Integer(lhs), Self::Integer(rhs)) => lhs == rhs,
            (Self::Float(lhs), Self::Float(rhs)) => lhs.to_bits() == rhs.to_bits(),
            (Self::Bool(lhs), Self::Bool(rhs)) => lhs == rhs,
            (Self::List(lhs), Self::List(rhs)) => lhs == rhs,
            (Self::Map(lhs), Self::Map(rhs)) => lhs == rhs,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_uses_project_relative_path() {
        let t = Target {
            package: "crates/foo".into(),
            kind: "script".into(),
            name: "bar".into(),
            deps: vec![],
            dependency_edges: BTreeMap::new(),
            srcs: vec![],
            attrs: BTreeMap::new(),
            typed_attrs: BTreeMap::new(),
        };
        assert_eq!(t.id(), "crates/foo/bar");
        let root_t = Target {
            package: String::new(),
            ..t
        };
        assert_eq!(root_t.id(), "bar");
    }
}

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::Digest as _;
use starlark::values::list::ListRef;
use starlark::values::{Value, ValueLike};

use super::store::with_store;
use super::transitive_set::{Node, TransitiveSet, TransitiveSets};

/// Action inputs retain shared sets until a consumer needs individual paths.
/// Deserialized set references must be resolved before iteration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeclaredInputs(Vec<Entry>);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum Entry {
    String(String),
    Set {
        #[serde(rename = "$struo_transitive_set")]
        digest: String,
        #[serde(skip)]
        node: Option<Arc<Node>>,
    },
}

/// A direct path or shared node, without flattening its transitive closure.
#[derive(Debug, Clone, Copy)]
pub enum InputFragment<'a> {
    String(&'a str),
    Set(&'a Node),
}

impl DeclaredInputs {
    /// Concatenate declarations without expanding their shared sets.
    pub fn concat(inputs: impl IntoIterator<Item = Self>) -> Self {
        Self(inputs.into_iter().flat_map(|inputs| inputs.0).collect())
    }

    pub(super) fn unpack(value: Value<'_>, field: &str) -> Result<Self> {
        let entry = |value: Value<'_>| -> Result<Entry> {
            if let Some(item) = value.unpack_str() {
                return Ok(Entry::String(item.to_string()));
            }
            if let Some(set) = value.downcast_ref::<TransitiveSet>() {
                with_store(|store| {
                    if let Some(store) = store {
                        store
                            .host_cache
                            .transitive_sets
                            .export(&store.workspace_root, &set.0);
                    }
                });
                return Ok(Entry::Set {
                    digest: set.0.hex(),
                    node: Some(Arc::clone(&set.0)),
                });
            }
            anyhow::bail!(
                "expected `{field}` entries to be strings or transitive sets, got `{}`",
                value.get_type()
            );
        };
        if value.downcast_ref::<TransitiveSet>().is_some() {
            return Ok(Self(vec![entry(value)?]));
        }
        let list = ListRef::from_value(value).ok_or_else(|| {
            anyhow::anyhow!(
                "expected `{field}` to be a list of strings or a transitive_set, got `{}`",
                value.get_type()
            )
        })?;
        Ok(Self(list.iter().map(entry).collect::<Result<_>>()?))
    }

    /// Resolve serialized references using the workspace's transitive-set registry.
    pub fn resolve(&mut self, workspace: &Path) -> Result<()> {
        self.resolve_with(workspace, &TransitiveSets::default())
    }

    pub(super) fn resolve_with(
        &mut self,
        workspace: &Path,
        registry: &TransitiveSets,
    ) -> Result<()> {
        for entry in &mut self.0 {
            if let Entry::Set { digest, node } = entry {
                if node.is_none() {
                    *node = Some(registry.resolve(workspace, digest)?);
                }
            }
        }
        Ok(())
    }

    /// Visit direct entries and shared roots in declaration order, without flattening.
    /// Panics for unresolved references; call `resolve` after deserialization.
    pub fn fragments(&self) -> impl Iterator<Item = InputFragment<'_>> {
        self.0.iter().map(|entry| match entry {
            Entry::String(item) => InputFragment::String(item),
            Entry::Set { node, .. } => InputFragment::Set(
                node.as_deref()
                    .expect("resolve declared input references before consuming them"),
            ),
        })
    }

    /// Flatten lazily. List duplicates survive; each set deduplicates independently.
    pub fn iter(&self) -> impl Iterator<Item = &String> {
        self.0.iter().flat_map(|entry| match entry {
            Entry::String(item) => InputIter::direct(item),
            Entry::Set { node, .. } => InputIter::set(
                node.as_deref()
                    .expect("resolve declared input references before consuming them"),
            ),
        })
    }

    pub fn len(&self) -> usize {
        self.iter().count()
    }

    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|entry| match entry {
            Entry::Set {
                node: Some(node), ..
            } => node.direct().is_empty() && node.children().is_empty(),
            Entry::String(_) | Entry::Set { node: None, .. } => false,
        })
    }

    pub fn to_vec(&self) -> Vec<String> {
        self.iter().cloned().collect()
    }

    pub fn contains(&self, item: &String) -> bool {
        self.iter().any(|candidate| candidate == item)
    }

    pub(super) fn hash_structure(&self, hash: &mut sha2::Sha256) {
        hash.update((self.0.len() as u64).to_le_bytes());
        for entry in &self.0 {
            let (tag, value) = match entry {
                Entry::String(value) => (0, value),
                Entry::Set { digest, .. } => (1, digest),
            };
            hash.update([tag]);
            hash.update((value.len() as u64).to_le_bytes());
            hash.update(value.as_bytes());
        }
    }

    pub(super) fn any_string(&self, predicate: impl Fn(&str) -> bool) -> bool {
        let mut seen = HashSet::new();
        for fragment in self.fragments() {
            match fragment {
                InputFragment::String(value) if predicate(value) => return true,
                InputFragment::String(_) => {}
                InputFragment::Set(node) => {
                    let mut stack = vec![node];
                    while let Some(node) = stack.pop() {
                        if !seen.insert(node.digest()) {
                            continue;
                        }
                        if node.direct().iter().any(|value| predicate(value)) {
                            return true;
                        }
                        stack.extend(node.children().iter().map(AsRef::as_ref));
                    }
                }
            }
        }
        false
    }
}

struct InputIter<'a> {
    items: std::slice::Iter<'a, String>,
    pending: Vec<&'a Node>,
    nodes: HashSet<[u8; 32]>,
    seen: HashSet<&'a String>,
}

impl<'a> InputIter<'a> {
    fn direct(item: &'a String) -> Self {
        Self {
            items: std::slice::from_ref(item).iter(),
            pending: Vec::new(),
            nodes: HashSet::new(),
            seen: HashSet::new(),
        }
    }

    fn set(node: &'a Node) -> Self {
        Self {
            items: [].iter(),
            pending: vec![node],
            nodes: HashSet::new(),
            seen: HashSet::new(),
        }
    }
}

impl<'a> Iterator for InputIter<'a> {
    type Item = &'a String;

    fn next(&mut self) -> Option<Self::Item> {
        if self.nodes.is_empty() && self.pending.is_empty() {
            return self.items.next();
        }
        loop {
            for item in self.items.by_ref() {
                if self.seen.insert(item) {
                    return Some(item);
                }
            }
            let node = self.pending.pop()?;
            if self.nodes.insert(node.digest()) {
                self.pending
                    .extend(node.children().iter().rev().map(AsRef::as_ref));
                self.items = node.direct().iter();
            }
        }
    }
}

impl<'a> IntoIterator for &'a DeclaredInputs {
    type Item = &'a String;
    type IntoIter = Box<dyn Iterator<Item = &'a String> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.iter())
    }
}

impl From<Vec<String>> for DeclaredInputs {
    fn from(items: Vec<String>) -> Self {
        Self(items.into_iter().map(Entry::String).collect())
    }
}

impl FromIterator<String> for DeclaredInputs {
    fn from_iter<T: IntoIterator<Item = String>>(iter: T) -> Self {
        Self::from(iter.into_iter().collect::<Vec<_>>())
    }
}

impl PartialEq for DeclaredInputs {
    fn eq(&self, other: &Self) -> bool {
        self.0.len() == other.0.len()
            && self.0.iter().zip(&other.0).all(|(a, b)| match (a, b) {
                (Entry::String(a), Entry::String(b))
                | (Entry::Set { digest: a, .. }, Entry::Set { digest: b, .. }) => a == b,
                _ => false,
            })
    }
}

impl Eq for DeclaredInputs {}

impl<T: AsRef<str>> PartialEq<Vec<T>> for DeclaredInputs {
    fn eq(&self, other: &Vec<T>) -> bool {
        self.iter()
            .map(String::as_str)
            .eq(other.iter().map(AsRef::as_ref))
    }
}

impl<T: AsRef<str>, const N: usize> PartialEq<[T; N]> for DeclaredInputs {
    fn eq(&self, other: &[T; N]) -> bool {
        self.iter()
            .map(String::as_str)
            .eq(other.iter().map(AsRef::as_ref))
    }
}

impl super::DeclaredAction {
    pub fn resolve_inputs(&mut self, workspace: &Path) -> Result<()> {
        self.inputs.resolve(workspace)
    }
}

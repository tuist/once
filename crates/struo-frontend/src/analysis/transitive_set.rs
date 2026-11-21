//! Transitive sets: string sets a target builds from its own items and
//! references to its dependencies' sets, so building one costs the target's
//! own items rather than its whole closure.
//!
//! A set is an immutable node. Its items are a pre-order walk, direct items
//! first, then each child left to right, keeping the first occurrence of each
//! item and skipping nodes already visited. That is the list a target kind
//! gets by concatenating its own items with its dependencies' flattened lists
//! and removing duplicates, so a target kind can switch to sets without
//! changing the actions it declares.
//!
//! Sets cross target boundaries as a reference to the node's digest. The
//! session keeps every exported node, and writes it under the workspace's
//! `.struo` directory so a provider stored by one invocation still resolves in
//! the next.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::Digest as _;
use starlark::values::list::ListRef;
use starlark::values::{Value, ValueLike};

/// Key of the one-entry JSON object that stands for a set in a provider.
pub(super) const REFERENCE_KEY: &str = "$struo_transitive_set";
const DIGEST_DOMAIN: &[u8] = b"struo.transitive_set.v1\0";
const STORE_DIR: &str = ".struo/transitive-sets";

type NodeDigest = [u8; 32];

#[derive(Debug)]
pub struct Node {
    digest: NodeDigest,
    direct: Box<[String]>,
    children: Box<[Arc<Node>]>,
}

impl Node {
    pub fn digest(&self) -> [u8; 32] {
        self.digest
    }

    pub fn direct(&self) -> &[String] {
        &self.direct
    }

    pub fn children(&self) -> &[Arc<Node>] {
        &self.children
    }

    fn new(direct: Vec<String>, children: Vec<Arc<Node>>) -> Self {
        let mut hash = sha2::Sha256::new();
        hash.update(DIGEST_DOMAIN);
        hash.update((direct.len() as u64).to_le_bytes());
        for item in &direct {
            hash.update((item.len() as u64).to_le_bytes());
            hash.update(item.as_bytes());
        }
        hash.update((children.len() as u64).to_le_bytes());
        for child in &children {
            hash.update(child.digest);
        }
        Self {
            digest: hash.finalize().into(),
            direct: direct.into_boxed_slice(),
            children: children.into_boxed_slice(),
        }
    }

    /// The set of `direct` followed by `transitive`.
    ///
    /// Duplicate direct items, empty children and repeated children are
    /// dropped, and a set that adds nothing to its only child is that child, so
    /// a chain of targets that contribute nothing to a set shares one node.
    pub(super) fn build(direct: Vec<String>, transitive: Vec<Arc<Node>>) -> Arc<Node> {
        let mut seen = HashSet::new();
        let direct = direct
            .into_iter()
            .filter(|item| seen.insert(item.clone()))
            .collect::<Vec<_>>();
        let mut seen = HashSet::new();
        let mut children = transitive
            .into_iter()
            .filter(|child| !child.is_empty() && seen.insert(child.digest))
            .collect::<Vec<_>>();
        if direct.is_empty() && children.len() == 1 {
            return children.remove(0);
        }
        Arc::new(Self::new(direct, children))
    }

    /// Children are never empty, so a node without direct items or children is
    /// the only empty shape.
    fn is_empty(&self) -> bool {
        self.direct.is_empty() && self.children.is_empty()
    }

    pub(super) fn hex(&self) -> String {
        hex::encode(self.digest)
    }

    /// Visit each item once, in the set's order.
    pub(super) fn for_each_item<'a>(&'a self, mut visit: impl FnMut(&'a str)) {
        let mut visited = HashSet::new();
        let mut items = HashSet::new();
        let mut stack = vec![self];
        while let Some(node) = stack.pop() {
            if !visited.insert(node.digest) {
                continue;
            }
            for item in &node.direct {
                if items.insert(item.as_str()) {
                    visit(item);
                }
            }
            stack.extend(node.children.iter().rev().map(AsRef::as_ref));
        }
    }

    fn for_each_topological_item<'a>(&'a self, mut visit: impl FnMut(&'a str)) {
        let mut visited = HashSet::new();
        let mut ordered = Vec::new();
        let mut stack = vec![(self, false)];
        while let Some((node, ready)) = stack.pop() {
            if ready {
                ordered.push(node);
            } else if visited.insert(node.digest) {
                stack.push((node, true));
                stack.extend(node.children.iter().map(|child| (child.as_ref(), false)));
            }
        }
        let mut items = HashSet::new();
        for node in ordered.into_iter().rev() {
            for item in &node.direct {
                if items.insert(item.as_str()) {
                    visit(item);
                }
            }
        }
    }

    #[cfg(test)]
    pub(super) fn to_vec(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.for_each_item(|item| out.push(item.to_string()));
        out
    }
}

/// On-disk form of one node: its own items and its children's digests.
#[derive(Serialize, Deserialize)]
struct StoredNode {
    direct: Vec<String>,
    children: Vec<String>,
}

/// Nodes the session exported or read back, by digest.
#[derive(Clone, Debug, Default)]
pub struct TransitiveSets {
    inner: Arc<Mutex<Registry>>,
}

#[derive(Debug, Default)]
struct Registry {
    nodes: HashMap<NodeDigest, Arc<Node>>,
    /// Nodes known to be on disk, along with every node they reach.
    stored: HashSet<NodeDigest>,
}

fn node_path(workspace: &Path, hex: &str) -> PathBuf {
    workspace
        .join(STORE_DIR)
        .join(&hex[..2])
        .join(format!("{hex}.json"))
}

impl TransitiveSets {
    pub fn resolve_inputs(
        &self,
        workspace: &Path,
        inputs: &mut super::DeclaredInputs,
    ) -> Result<()> {
        inputs.resolve_with(workspace, self)
    }

    pub fn resolve_action(
        &self,
        workspace: &Path,
        action: &mut super::DeclaredAction,
    ) -> Result<()> {
        action.inputs.resolve_with(workspace, self)?;
        action.argv.resolve_with(workspace, self)?;
        for file in &mut action.arg_files {
            file.args.resolve_with(workspace, self)?;
        }
        Ok(())
    }

    #[must_use]
    pub fn provider_available(&self, workspace: &Path, value: &JsonValue) -> bool {
        match value {
            JsonValue::Array(items) => items
                .iter()
                .all(|item| self.provider_available(workspace, item)),
            JsonValue::Object(entries) => match reference(entries) {
                Some(hex) => self.resolve(workspace, hex).is_ok(),
                None => entries
                    .values()
                    .all(|value| self.provider_available(workspace, value)),
            },
            _ => true,
        }
    }

    /// Keep `node` for dependents in this session and write it, and every node
    /// it reaches that this session has not verified, for later ones. Rewrite
    /// children before parents to repair incomplete or damaged stored DAGs.
    ///
    /// Writing is best effort: a node that could not be written still resolves
    /// in this session, and a stored provider that names it is found missing
    /// and analysed again.
    pub(super) fn export(&self, workspace: &Path, node: &Arc<Node>) {
        let mut registry = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        registry
            .nodes
            .entry(node.digest)
            .or_insert_with(|| Arc::clone(node));
        let mut stack = vec![(node.as_ref(), false)];
        while let Some((node, children_stored)) = stack.pop() {
            if registry.stored.contains(&node.digest) {
                continue;
            }
            if children_stored {
                if write_node(&node_path(workspace, &node.hex()), node).is_err() {
                    return;
                }
            } else {
                stack.push((node, true));
                stack.extend(node.children.iter().map(|child| (child.as_ref(), false)));
                continue;
            }
            registry.stored.insert(node.digest);
        }
    }

    /// The node `hex` names, from this session or from disk.
    pub(super) fn resolve(&self, workspace: &Path, hex: &str) -> Result<Arc<Node>> {
        let digest = parse_digest(hex)?;
        let mut registry = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(node) = registry.nodes.get(&digest) {
            return Ok(Arc::clone(node));
        }
        let mut stack: Vec<(NodeDigest, Option<StoredNode>)> = vec![(digest, None)];
        let mut visiting = HashSet::new();
        while let Some((digest, stored)) = stack.pop() {
            if registry.nodes.contains_key(&digest) {
                continue;
            }
            let hex = hex::encode(digest);
            let Some(stored) = stored else {
                if !visiting.insert(digest) {
                    anyhow::bail!("transitive set {hex} contains a cycle");
                }
                let path = node_path(workspace, &hex);
                let raw = std::fs::read(&path).with_context(|| {
                    format!("transitive set {hex} is not stored at {}", path.display())
                })?;
                let stored: StoredNode = serde_json::from_slice(&raw)
                    .with_context(|| format!("reading transitive set {hex}"))?;
                let children = stored
                    .children
                    .iter()
                    .map(|child| parse_digest(child))
                    .collect::<Result<Vec<_>>>()?;
                stack.push((digest, Some(stored)));
                stack.extend(children.into_iter().map(|child| (child, None)));
                continue;
            };
            let children = stored
                .children
                .iter()
                .map(|child| {
                    let child = parse_digest(child)?;
                    registry
                        .nodes
                        .get(&child)
                        .cloned()
                        .ok_or_else(|| anyhow!("transitive set {hex} names a missing child"))
                })
                .collect::<Result<Vec<_>>>()?;
            let node = Node::new(stored.direct, children);
            if node.digest != digest {
                anyhow::bail!("transitive set {hex} does not match its digest");
            }
            visiting.remove(&digest);
            registry.stored.insert(digest);
            registry.nodes.insert(digest, Arc::new(node));
        }
        registry
            .nodes
            .get(&digest)
            .cloned()
            .ok_or_else(|| anyhow!("transitive set {hex} did not resolve"))
    }
}

fn parse_digest(hex: &str) -> Result<NodeDigest> {
    let mut digest = [0_u8; 32];
    hex::decode_to_slice(hex, &mut digest)
        .map_err(|_| anyhow!("`{hex}` is not a transitive set digest"))?;
    Ok(digest)
}

fn write_node(path: &Path, node: &Node) -> std::io::Result<()> {
    let stored = StoredNode {
        direct: node.direct.to_vec(),
        children: node.children.iter().map(|child| child.hex()).collect(),
    };
    let raw = serde_json::to_vec(&stored)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Named by process so two invocations writing the same node cannot read
    // each other's half-written file; either rename leaves the same bytes.
    let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
    std::fs::write(&temporary, raw)?;
    std::fs::rename(&temporary, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temporary);
    })
}

/// Whether every set a stored provider names resolves, including descendants.
/// Use a retained registry's `provider_available` to share validated nodes.
#[must_use]
pub fn transitive_sets_available(workspace: &Path, value: &JsonValue) -> bool {
    TransitiveSets::default().provider_available(workspace, value)
}

/// The digest a one-entry reference object names.
pub(super) fn reference(entries: &serde_json::Map<String, JsonValue>) -> Option<&str> {
    match (entries.len(), entries.get(REFERENCE_KEY)) {
        (1, Some(JsonValue::String(hex))) => Some(hex),
        _ => None,
    }
}

pub(super) use value::TransitiveSet;

// `ProvidesStaticType` and `starlark_simple_value!` expand to the `unsafe impl`s
// that tell starlark a value's static type, which is the only way to register a
// Starlark value type. Scoped here so the crate stays under the workspace-wide
// `unsafe_code = "deny"`; nothing in this module is hand-written unsafe code.
#[allow(unsafe_code)]
mod value {
    use std::fmt;
    use std::sync::Arc;

    use allocative::Allocative;
    use anyhow::anyhow;
    use starlark::environment::{Methods, MethodsBuilder};
    use starlark::starlark_module;
    use starlark::starlark_simple_value;
    use starlark::values::{
        starlark_value, Heap, NoSerialize, ProvidesStaticType, StarlarkPagablePanic, StarlarkValue,
        Value, ValueLike,
    };

    use super::Node;

    /// A transitive set as a Starlark value.
    #[derive(Debug, Clone, ProvidesStaticType, NoSerialize, Allocative, StarlarkPagablePanic)]
    pub(in crate::analysis) struct TransitiveSet(
        #[allocative(skip)] pub(in crate::analysis) Arc<Node>,
    );

    starlark_simple_value!(TransitiveSet);

    impl fmt::Display for TransitiveSet {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("transitive_set(")?;
            let mut first = true;
            let mut result = f.write_str("[");
            self.0.for_each_item(|item| {
                if result.is_ok() {
                    let separator = if first { "" } else { ", " };
                    first = false;
                    result = write!(f, "{separator}{item:?}");
                }
            });
            result?;
            f.write_str("])")
        }
    }

    fn flatten_required(operation: &str) -> starlark::Error {
        starlark::Error::new_other(anyhow!(
            "{operation} is not supported on a transitive_set; call `.to_list()` where the items are needed"
        ))
    }

    #[starlark_module]
    fn transitive_set_methods(builder: &mut MethodsBuilder) {
        /// The set's items, in order and without duplicates. Costs the size of
        /// the whole set, so call it only where the items are consumed.
        fn to_list<'v>(
            this: Value<'v>,
            #[starlark(require = named, default = "preorder")] order: &str,
            heap: Heap<'v>,
        ) -> anyhow::Result<Value<'v>> {
            let set = this
                .downcast_ref::<TransitiveSet>()
                .ok_or_else(|| anyhow!("expected a transitive_set"))?;
            let mut items = Vec::new();
            match order {
                "preorder" => set.0.for_each_item(|item| items.push(heap.alloc(item))),
                "topological" => set
                    .0
                    .for_each_topological_item(|item| items.push(heap.alloc(item))),
                _ => anyhow::bail!("expected to_list order to be `preorder` or `topological`"),
            }
            Ok(heap.alloc(items))
        }
    }

    starlark::methods_static!(TRANSITIVE_SET_METHODS = transitive_set_methods);

    #[starlark_value(type = "transitive_set")]
    impl<'v> StarlarkValue<'v> for TransitiveSet {
        fn get_methods() -> Option<&'static Methods> {
            Some(TRANSITIVE_SET_METHODS.methods())
        }

        fn to_bool(&self) -> bool {
            !self.0.is_empty()
        }

        fn equals(&self, other: Value<'v>) -> starlark::Result<bool> {
            Ok(other
                .downcast_ref::<TransitiveSet>()
                .is_some_and(|other| other.0.digest == self.0.digest))
        }

        fn write_hash(
            &self,
            hasher: &mut starlark::collections::StarlarkHasher,
        ) -> starlark::Result<()> {
            std::hash::Hash::hash(&self.0.digest, hasher);
            Ok(())
        }

        fn length(&self) -> starlark::Result<i32> {
            Err(flatten_required("len()"))
        }

        fn iterate_collect(&self, _heap: Heap<'v>) -> starlark::Result<Vec<Value<'v>>> {
            Err(flatten_required("iteration"))
        }

        fn is_in(&self, _other: Value<'v>) -> starlark::Result<bool> {
            Err(flatten_required("`in`"))
        }

        fn add(&self, _rhs: Value<'v>, _heap: Heap<'v>) -> Option<starlark::Result<Value<'v>>> {
            Some(Err(flatten_required("`+`")))
        }

        fn radd(&self, _lhs: Value<'v>, _heap: Heap<'v>) -> Option<starlark::Result<Value<'v>>> {
            Some(Err(flatten_required("`+`")))
        }
    }
}

/// Build a set from Starlark `direct` and `transitive` arguments. Entries of
/// `transitive` may be sets, lists of strings, or `None`.
pub(super) fn build(
    direct: Option<Value<'_>>,
    transitive: Option<Value<'_>>,
) -> Result<TransitiveSet> {
    let direct = match direct.filter(|value| !value.is_none()) {
        Some(value) => super::values::unpack_string_list(value, "transitive_set.direct")?,
        None => Vec::new(),
    };
    let mut children = Vec::new();
    if let Some(value) = transitive.filter(|value| !value.is_none()) {
        let list = ListRef::from_value(value).ok_or_else(|| {
            anyhow!(
                "expected `transitive_set.transitive` to be a list of transitive sets, lists, or None, got `{}`",
                value.get_type()
            )
        })?;
        for entry in list.iter() {
            if entry.is_none() {
                continue;
            }
            if let Some(set) = entry.downcast_ref::<TransitiveSet>() {
                children.push(Arc::clone(&set.0));
            } else if ListRef::from_value(entry).is_some() {
                let items = super::values::unpack_string_list(entry, "transitive_set.transitive")?;
                children.push(Node::build(items, Vec::new()));
            } else {
                anyhow::bail!(
                    "expected `transitive_set.transitive` entries to be transitive sets, lists, or None, got `{}`",
                    entry.get_type()
                );
            }
        }
    }
    Ok(TransitiveSet(Node::build(direct, children)))
}

#[cfg(test)]
mod tests;

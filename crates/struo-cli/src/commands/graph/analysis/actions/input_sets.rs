use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::Result;
use struo_cas::Digest;
use struo_core::{InputDigestBuilder, InputFingerprintComponent};
use struo_frontend::analysis::{DeclaredInputs, InputFragment, Node};

/// Hash shared structure without allocating a flattened path list. Resolution
/// belongs to the caller: the same node can name sources in one action and
/// generated inputs in another.
pub(super) fn compose(
    inputs: &DeclaredInputs,
    builder: &mut InputDigestBuilder,
    mut resolve: impl FnMut(&str) -> Result<InputFingerprintComponent>,
) -> Result<()> {
    let mut components = BTreeMap::new();
    let mut nodes = HashMap::new();
    let mut direct = BTreeSet::new();
    let mut roots = BTreeMap::new();
    for fragment in inputs.fragments() {
        match fragment {
            InputFragment::String(path) => {
                direct.insert(path);
            }
            InputFragment::Set(node) => {
                roots.insert(node.digest(), node);
            }
        }
    }
    for path in direct {
        let component = resolve_once(&mut components, path, &mut resolve)?;
        builder.push_keyed(b"input", &component_digest(component));
    }
    for (_, node) in roots {
        builder.push_keyed(
            b"input-set",
            &hash_node(node, &mut nodes, &mut components, &mut resolve)?,
        );
    }
    for (_, component) in components {
        builder.record_digest(component.category, component.label, component.digest);
    }
    Ok(())
}

fn resolve_once<'a, 'b>(
    components: &'b mut BTreeMap<&'a str, InputFingerprintComponent>,
    path: &'a str,
    resolve: &mut impl FnMut(&str) -> Result<InputFingerprintComponent>,
) -> Result<&'b InputFingerprintComponent> {
    use std::collections::btree_map::Entry;

    Ok(match components.entry(path) {
        Entry::Occupied(entry) => entry.into_mut(),
        Entry::Vacant(entry) => entry.insert(resolve(path)?),
    })
}

fn component_digest(component: &InputFingerprintComponent) -> Digest {
    let mut hash = InputDigestBuilder::new(b"struo.resolved-input.v1\0");
    hash.push_bytes(component.category.as_bytes());
    hash.push_keyed(component.label.as_bytes(), &component.digest);
    hash.finish()
}

fn hash_node<'a>(
    root: &'a Node,
    nodes: &mut HashMap<[u8; 32], Digest>,
    components: &mut BTreeMap<&'a str, InputFingerprintComponent>,
    resolve: &mut impl FnMut(&str) -> Result<InputFingerprintComponent>,
) -> Result<Digest> {
    let mut pending = vec![(root, false)];
    while let Some((node, ready)) = pending.pop() {
        if nodes.contains_key(&node.digest()) {
            continue;
        }
        if !ready {
            pending.push((node, true));
            pending.extend(
                node.children()
                    .iter()
                    .rev()
                    .map(|child| (child.as_ref(), false)),
            );
            continue;
        }
        let mut hash = InputDigestBuilder::new(b"struo.resolved-input-set.v1\0");
        for path in node.direct() {
            let component = resolve_once(components, path, resolve)?;
            hash.push_keyed(b"direct", &component_digest(component));
        }
        for child in node.children() {
            hash.push_keyed(b"child", &nodes[&child.digest()]);
        }
        nodes.insert(node.digest(), hash.finish());
    }
    Ok(nodes[&root.digest()])
}

#[cfg(test)]
mod tests;

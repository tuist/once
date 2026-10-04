use std::collections::BTreeMap;

use once_cas::Digest;

pub(super) fn prune_replaced_descendants<T>(
    previous: &mut BTreeMap<String, T>,
    replacements: &BTreeMap<String, Digest>,
) {
    previous.retain(|path, _| {
        let mut candidate = path.as_str();
        while let Some((parent, _)) = candidate.rsplit_once('/') {
            if replacements.contains_key(parent) {
                return false;
            }
            candidate = parent;
        }
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_tree_supersedes_deleted_and_modified_child_records() {
        let mut previous = BTreeMap::from([
            ("out/product/deleted".into(), 1),
            ("out/product/nested/modified".into(), 2),
            ("out/product-neighbor/file".into(), 3),
            ("out/product".into(), 4),
        ]);
        let replacements = BTreeMap::from([("out/product".into(), Digest::of_bytes(b"new tree"))]);
        prune_replaced_descendants(&mut previous, &replacements);
        assert_eq!(previous.len(), 2);
        assert_eq!(previous.get("out/product-neighbor/file"), Some(&3));
        assert_eq!(previous.get("out/product"), Some(&4));
    }

    #[test]
    fn replacing_a_child_preserves_its_parent_and_siblings() {
        let mut previous = BTreeMap::from([
            ("out/tree".into(), 1),
            ("out/tree/child".into(), 2),
            ("out/tree/sibling".into(), 3),
        ]);
        let replacements = BTreeMap::from([("out/tree/child".into(), Digest::of_bytes(b"new"))]);
        prune_replaced_descendants(&mut previous, &replacements);
        assert_eq!(previous.len(), 3);
    }
}

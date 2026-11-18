use std::collections::BTreeMap;

use struo_cas::Digest;

use super::actions::TouchedPath;
use super::AvailableInput;

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

/// Stop treating an output as on disk once an action that ran replaced it
/// without producing it again.
///
/// Cleaning, removing, or overwriting a path takes away whatever another
/// action left there, so a later reader has to restore it rather than trust
/// a flag that described the disk before the action ran.
pub(super) fn forget_replaced_materializations(
    inputs: &mut BTreeMap<String, AvailableInput>,
    touched: &[TouchedPath],
    produced: &BTreeMap<String, Digest>,
) {
    for (path, input) in inputs.iter_mut() {
        if !input.materialized || within_any(path, produced.keys()) {
            continue;
        }
        if touched.iter().any(|touched| match touched {
            TouchedPath::Replaced(replaced) => is_within(path, replaced),
            TouchedPath::Created(_) => false,
        }) {
            input.materialized = false;
        }
    }
}

fn within_any<'a>(path: &str, roots: impl IntoIterator<Item = &'a String>) -> bool {
    roots.into_iter().any(|root| is_within(path, root))
}

fn is_within(path: &str, root: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn available(materialized: bool) -> AvailableInput {
        AvailableInput {
            blob_digest: Digest::of_bytes(b"input"),
            same_target: true,
            materialized,
        }
    }

    #[test]
    fn replacing_a_path_forgets_outputs_at_or_under_it_but_not_its_own() {
        let mut inputs = BTreeMap::from([
            ("out/a".to_string(), available(true)),
            ("out/tree/child".to_string(), available(true)),
            ("out/tree-sibling".to_string(), available(true)),
            ("out/created/kept".to_string(), available(true)),
            ("out/new".to_string(), available(true)),
        ]);
        let touched = vec![
            TouchedPath::Replaced("out/a".to_string()),
            TouchedPath::Replaced("out/tree".to_string()),
            TouchedPath::Replaced("out/new".to_string()),
            TouchedPath::Created("out/created".to_string()),
        ];
        let produced = BTreeMap::from([("out/new".to_string(), Digest::of_bytes(b"new"))]);

        forget_replaced_materializations(&mut inputs, &touched, &produced);

        assert!(!inputs["out/a"].materialized);
        assert!(!inputs["out/tree/child"].materialized);
        assert!(inputs["out/tree-sibling"].materialized);
        assert!(inputs["out/created/kept"].materialized);
        assert!(inputs["out/new"].materialized);
    }

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

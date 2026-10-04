use std::collections::VecDeque;

use once_frontend::analysis::DeclaredAction;

pub(super) fn take_batch(
    first: DeclaredAction,
    pending: &mut VecDeque<DeclaredAction>,
    limit: usize,
) -> Vec<DeclaredAction> {
    let mut batch = vec![first];
    if batch[0].depends_on_prior_actions {
        return batch;
    }
    while batch.len() < limit {
        let Some(next) = pending.front() else { break };
        if next.depends_on_prior_actions || batch.iter().any(|prior| conflicts(prior, next)) {
            break;
        }
        if let Some(next) = pending.pop_front() {
            batch.push(next);
        }
    }
    batch
}

fn overlaps(left: &str, right: &str) -> bool {
    let left = std::path::Path::new(left);
    let right = std::path::Path::new(right);
    left.starts_with(right) || right.starts_with(left)
}

fn writes(action: &DeclaredAction) -> impl Iterator<Item = &str> {
    action
        .outputs
        .iter()
        .chain(&action.clean_paths)
        .chain(action.stdout.iter())
        .chain(action.stderr.iter())
        .map(String::as_str)
        .chain(action.arg_files.iter().map(|file| file.path.as_str()))
}

fn accesses(action: &DeclaredAction) -> impl Iterator<Item = &str> {
    writes(action).chain(action.inputs.iter().map(String::as_str))
}

fn conflicts(left: &DeclaredAction, right: &DeclaredAction) -> bool {
    writes(left).any(|path| accesses(right).any(|other| overlaps(path, other)))
        || writes(right).any(|path| accesses(left).any(|other| overlaps(path, other)))
        || left
            .clean_paths
            .iter()
            .any(|path| right.create_dirs.iter().any(|dir| overlaps(path, dir)))
        || right
            .clean_paths
            .iter()
            .any(|path| left.create_dirs.iter().any(|dir| overlaps(path, dir)))
        || left
            .create_dirs
            .iter()
            .any(|dir| writes(right).any(|path| std::path::Path::new(dir).starts_with(path)))
        || right
            .create_dirs
            .iter()
            .any(|dir| writes(left).any(|path| std::path::Path::new(dir).starts_with(path)))
        || !left.cacheable
        || !right.cacheable
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(inputs: &[&str], outputs: &[&str], ordered: bool) -> DeclaredAction {
        serde_json::from_value(serde_json::json!({
            "inputs": inputs, "outputs": outputs, "depends_on_prior_actions": ordered,
        }))
        .unwrap()
    }

    #[test]
    fn independent_compilations_share_a_bounded_batch() {
        let first = action(&["a.c"], &["a.o"], false);
        let mut queue = VecDeque::from([
            action(&["b.c"], &["b.o"], false),
            action(&["c.c"], &["c.o"], false),
        ]);
        assert_eq!(take_batch(first, &mut queue, 2).len(), 2);
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn generated_inputs_signing_and_directory_mutations_serialize() {
        for next in [
            action(&["build/module"], &["consumer.o"], false),
            action(&["build/module"], &["build/module"], false),
            action(&[], &["build"], false),
            action(&[], &["other"], true),
        ] {
            let mut queue = VecDeque::from([next]);
            assert_eq!(
                take_batch(action(&[], &["build/module"], false), &mut queue, 4).len(),
                1
            );
        }
    }

    #[test]
    fn directory_creation_does_not_serialize_sibling_outputs() {
        let mut first = action(&[], &["modules/a"], false);
        first.create_dirs = vec!["modules".into()];
        let mut second = action(&[], &["modules/b"], false);
        second.create_dirs = vec!["modules".into()];
        let mut queue = VecDeque::from([second]);
        assert_eq!(take_batch(first, &mut queue, 4).len(), 2);
    }

    #[test]
    fn captured_streams_and_file_directory_conflicts_are_serialized() {
        let mut first = action(&[], &["first"], false);
        first.stdout = Some("shared.log".into());
        let mut second = action(&[], &["second"], false);
        second.stderr = Some("shared.log".into());
        assert_eq!(take_batch(first, &mut VecDeque::from([second]), 4).len(), 1);

        let first = action(&[], &["build/path"], false);
        let mut second = action(&[], &["other"], false);
        second.create_dirs = vec!["build/path/nested".into()];
        assert_eq!(take_batch(first, &mut VecDeque::from([second]), 4).len(), 1);
    }
}

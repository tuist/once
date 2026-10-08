use std::collections::HashSet;
use std::sync::Arc;

use crate::proto::{ActionCompleted, SourceFileStatus};

/// Immutable repository file membership for the commit reported by one run.
/// The caller owns Git collection; unavailable or incomplete trees stay unknown.
#[derive(Clone, Debug, Default)]
pub struct SourceFileSnapshot {
    committed: Option<Arc<HashSet<String>>>,
}

impl SourceFileSnapshot {
    /// Construct only from a complete listing of blobs at the reported commit.
    #[must_use]
    pub fn from_committed_paths(paths: HashSet<String>) -> Self {
        Self {
            committed: Some(Arc::new(paths)),
        }
    }

    #[must_use]
    pub fn status(&self, path: &str) -> SourceFileStatus {
        match &self.committed {
            Some(paths) if paths.contains(path) => SourceFileStatus::Committed,
            Some(_) => SourceFileStatus::NotCommitted,
            None => SourceFileStatus::Unknown,
        }
    }

    pub(crate) fn classify(&self, action: &mut ActionCompleted) {
        action.source_file_statuses = action
            .source_files
            .iter()
            .map(|path| self.status(path) as i32)
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_and_unavailable_snapshots_are_distinct() {
        let snapshot = SourceFileSnapshot::from_committed_paths(HashSet::from([
            "src/main.rs".into(),
            "vendor/tracked.rs".into(),
        ]));
        assert_eq!(snapshot.status("src/main.rs"), SourceFileStatus::Committed);
        assert_eq!(
            snapshot.status("vendor/tracked.rs"),
            SourceFileStatus::Committed
        );
        assert_eq!(
            snapshot.status("src/Main.rs"),
            SourceFileStatus::NotCommitted
        );
        assert_eq!(
            snapshot.status("vendor/ignored.rs"),
            SourceFileStatus::NotCommitted
        );
        assert_eq!(
            SourceFileSnapshot::default().status("src/main.rs"),
            SourceFileStatus::Unknown
        );
    }

    #[test]
    fn retained_actions_are_reclassified_without_changing_paths() {
        let mut action = ActionCompleted {
            source_files: vec!["src/main.rs".into(), "vendor/ignored.rs".into()],
            ..Default::default()
        };
        let first = SourceFileSnapshot::from_committed_paths(HashSet::from(["src/main.rs".into()]));
        first.classify(&mut action);
        assert_eq!(action.source_file_statuses, [1, 2]);
        let next = SourceFileSnapshot::from_committed_paths(HashSet::new());
        next.classify(&mut action);
        assert_eq!(action.source_file_statuses, [2, 2]);
        assert_eq!(action.source_files, ["src/main.rs", "vendor/ignored.rs"]);
    }
}

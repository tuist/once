use prost::encoding::{encoded_len_varint, key_len, string};
use prost::Message;

use crate::proto::{run_event::Payload, RunEvent};

pub(crate) fn fit_action_presentation(event: &mut RunEvent, max_bytes: usize) {
    if !matches!(&event.payload, Some(Payload::ActionCompleted(_)))
        || event.encoded_len() <= max_bytes
    {
        return;
    }
    let Some(Payload::ActionCompleted(action)) = &mut event.payload else {
        return;
    };
    let source_files = std::mem::take(&mut action.source_files);
    let mut statuses = std::mem::take(&mut action.source_file_statuses);
    let classified = !statuses.is_empty();
    if classified && statuses.len() != source_files.len() {
        statuses = vec![crate::proto::SourceFileStatus::Unknown as i32; source_files.len()];
    }
    if event.encoded_len() > max_bytes {
        let Some(Payload::ActionCompleted(action)) = &mut event.payload else {
            return;
        };
        action.display_name = None;
    }
    let base_event_bytes = event.encoded_len();
    let Some(Payload::ActionCompleted(action)) = &mut event.payload else {
        return;
    };
    let mut action_bytes = action.encoded_len();
    let envelope_bytes = base_event_bytes
        - action_bytes
        - encoded_len_varint(u64::try_from(action_bytes).unwrap_or(u64::MAX));
    let mut status_bytes = 0_usize;
    for (index, path) in source_files.into_iter().enumerate() {
        let candidate_path_bytes = action_bytes.saturating_add(string::encoded_len(16, &path));
        let status = statuses.get(index).copied().unwrap_or_default();
        let candidate_status_bytes = status_bytes.saturating_add(encoded_len_varint(
            u64::try_from(status).unwrap_or(u64::MAX),
        ));
        let packed_bytes = if classified {
            key_len(17)
                + encoded_len_varint(u64::try_from(candidate_status_bytes).unwrap_or(u64::MAX))
                + candidate_status_bytes
        } else {
            0
        };
        let candidate_bytes = candidate_path_bytes.saturating_add(packed_bytes);
        let event_bytes = envelope_bytes
            .saturating_add(candidate_bytes)
            .saturating_add(encoded_len_varint(
                u64::try_from(candidate_bytes).unwrap_or(u64::MAX),
            ));
        if event_bytes <= max_bytes {
            action.source_files.push(path);
            action_bytes = candidate_path_bytes;
            if classified {
                action.source_file_statuses.push(status);
                status_bytes = candidate_status_bytes;
            }
        }
    }
    tracing::debug!(
        seq = event.seq,
        max_bytes,
        "limited oversized action presentation metadata"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::ActionCompleted;

    fn event(name: &str, paths: Vec<String>) -> RunEvent {
        RunEvent {
            seq: 7,
            payload: Some(Payload::ActionCompleted(ActionCompleted {
                identifier: "compile".into(),
                display_name: Some(name.into()),
                source_files: paths,
                selected_attempt: 1,
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    fn action(event: &RunEvent) -> &ActionCompleted {
        let Some(Payload::ActionCompleted(action)) = &event.payload else {
            panic!("expected action completion");
        };
        assert_eq!(event.seq, 7);
        assert_eq!(action.identifier, "compile");
        assert_eq!(action.selected_attempt, 1);
        action
    }

    #[test]
    fn small_metadata_is_unchanged() {
        let mut event = event("Compile main.c", vec!["src/main.c".into()]);
        let original = event.clone();
        fit_action_presentation(&mut event, 512);
        assert_eq!(event, original);
    }

    #[test]
    fn large_source_lists_keep_links_that_fit_without_dropping_the_action() {
        for limit in [128, 256, 512, 16_384] {
            let paths = (0..3_000)
                .map(|file| format!("src/file-{file}.c"))
                .collect();
            let mut event = event("Compile module", paths);
            fit_action_presentation(&mut event, limit);
            assert!(event.encoded_len() <= limit);
            assert_eq!(
                action(&event).display_name.as_deref(),
                Some("Compile module")
            );
            assert!(!action(&event).source_files.is_empty());
            assert!(action(&event).source_files.len() < 3_000);
            assert_eq!(action(&event).source_files[0], "src/file-0.c");
        }
    }

    #[test]
    fn classified_paths_and_statuses_are_trimmed_together() {
        for limit in [128, 256, 512, 16_384] {
            let paths = (0..3_000).map(|i| format!("src/file-{i}.c")).collect();
            let mut event = event("Compile module", paths);
            let Some(Payload::ActionCompleted(completed)) = &mut event.payload else {
                unreachable!();
            };
            completed.source_file_statuses =
                (0..3_000).map(|i| if i % 2 == 0 { 1 } else { 2 }).collect();
            fit_action_presentation(&mut event, limit);
            assert!(event.encoded_len() <= limit);
            let action = action(&event);
            assert!(!action.source_files.is_empty());
            assert_eq!(action.source_files.len(), action.source_file_statuses.len());
            for (path, status) in action.source_files.iter().zip(&action.source_file_statuses) {
                let index: usize = path
                    .strip_prefix("src/file-")
                    .unwrap()
                    .strip_suffix(".c")
                    .unwrap()
                    .parse()
                    .unwrap();
                assert_eq!(*status, if index.is_multiple_of(2) { 1 } else { 2 });
            }
        }
    }

    #[test]
    fn skipping_a_large_path_keeps_the_smaller_paths_status() {
        let mut event = event("Compile", vec!["x".repeat(1_000), "src/main.c".into()]);
        let Some(Payload::ActionCompleted(completed)) = &mut event.payload else {
            unreachable!();
        };
        completed.source_file_statuses = vec![1, 2];
        fit_action_presentation(&mut event, 128);
        assert_eq!(action(&event).source_files, ["src/main.c"]);
        assert_eq!(action(&event).source_file_statuses, [2]);
        assert!(event.encoded_len() <= 128);
    }

    #[test]
    fn individually_oversized_paths_do_not_hide_smaller_links() {
        let mut event = event(
            "Compile module",
            vec!["file.c".repeat(100), "src/main.c".into()],
        );
        fit_action_presentation(&mut event, 512);
        assert!(event.encoded_len() <= 512);
        assert_eq!(action(&event).source_files, ["src/main.c"]);
    }

    #[test]
    fn oversized_names_do_not_drop_small_source_links() {
        let mut event = event(&"x".repeat(1_000), vec!["src/main.c".into()]);
        fit_action_presentation(&mut event, 512);
        assert!(event.encoded_len() <= 512);
        assert_eq!(action(&event).display_name, None);
        assert_eq!(action(&event).source_files, ["src/main.c"]);
    }

    #[test]
    fn oversized_names_and_sources_do_not_drop_the_action() {
        let mut event = event(&"x".repeat(1_000), vec!["file.c".repeat(100); 1_000]);
        fit_action_presentation(&mut event, 512);
        assert!(event.encoded_len() <= 512);
        assert_eq!(action(&event).display_name, None);
        assert!(action(&event).source_files.is_empty());
    }
}

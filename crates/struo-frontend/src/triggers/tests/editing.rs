use super::{trigger, SOURCE};
use crate::{apply_trigger_operations, load_triggers_toml_str, TriggerEditOperation};

#[test]
fn batches_create_replace_rename_and_delete_without_changing_other_manifest_data() {
    let target = r"
# Keep the original target formatting.
[[target]]
name   = 'core' # target comment
kind = 'custom_kind'
srcs = [ 'src/**' ]
[target.attrs]
custom = { nested = [1, 2, 3] }
";
    let src = format!("[modules]\npaths = ['missing.bzl']\n{SOURCE}{target}");
    let mut replacement = trigger();
    replacement.id = "renamed-checks".to_string();
    replacement.on.refs = vec!["refs/tags/v1".to_string()];
    replacement.invoke[0].capability = "build".to_string();
    let edited = apply_trigger_operations(
        &src,
        &[
            TriggerEditOperation::Update {
                id: trigger().id,
                trigger: replacement.clone(),
            },
            TriggerEditOperation::Create { trigger: trigger() },
            TriggerEditOperation::Delete { id: trigger().id },
        ],
    )
    .unwrap();
    assert_eq!(
        load_triggers_toml_str("struo.toml", &edited).unwrap(),
        [replacement]
    );
    assert!(edited.contains(target), "{edited}");
    assert!(edited.contains("paths = ['missing.bzl']"));
    let original: toml::Value = toml::from_str(&src).unwrap();
    let updated: toml::Value = toml::from_str(&edited).unwrap();
    assert_eq!(original["target"], updated["target"]);
    assert_eq!(original["modules"], updated["modules"]);
}

#[test]
fn replacing_a_trigger_preserves_untouched_nested_trigger_tables() {
    let untouched = r"
# Keep this declaration's comments and table layout.
[[trigger]]
id = 'other-checks'
[trigger.on]
event = 'vcs.ref_changed'
refs = ['refs/heads/other']
[[trigger.invoke]]
target = 'checks/other'
capability = 'test'
[trigger.invoke.configuration]
os = 'linux'
arch = 'x86_64'
tokens = []
";
    let src = format!("{SOURCE}{untouched}");
    let mut replacement = trigger();
    replacement.invoke[0].capability = "run".to_string();
    let edited = apply_trigger_operations(
        &src,
        &[TriggerEditOperation::Update {
            id: trigger().id,
            trigger: replacement.clone(),
        }],
    )
    .unwrap();
    assert!(edited.contains(untouched), "{edited}");
    let loaded = load_triggers_toml_str("struo.toml", &edited).unwrap();
    assert_eq!(loaded.len(), 2);
    assert!(loaded.contains(&replacement));
    assert_eq!(
        loaded
            .iter()
            .find(|record| record.id == "other-checks")
            .unwrap()
            .invoke[0]
            .target,
        "checks/other"
    );
}

#[test]
fn operations_and_invalid_final_state_abort_the_entire_batch() {
    let mut created = trigger();
    created.id = "new-checks".to_string();
    for failing in [
        TriggerEditOperation::Delete {
            id: "missing".to_string(),
        },
        TriggerEditOperation::Create { trigger: trigger() },
        TriggerEditOperation::Update {
            id: "missing".to_string(),
            trigger: trigger(),
        },
    ] {
        let operations = [
            TriggerEditOperation::Create {
                trigger: created.clone(),
            },
            failing,
        ];
        assert!(apply_trigger_operations(SOURCE, &operations).is_err());
        assert_eq!(
            load_triggers_toml_str("struo.toml", SOURCE).unwrap(),
            [trigger()]
        );
    }
    created.on.refs.clear();
    let operations = [TriggerEditOperation::Create { trigger: created }];
    assert_eq!(
        apply_trigger_operations(SOURCE, &operations).unwrap_err()[0].code,
        "trigger_refs_empty"
    );
}

#[test]
fn validates_the_complete_final_list_and_allows_repairing_invalid_existing_records() {
    let invalid = SOURCE.replace("refs/heads/main", "main");
    let mut other = trigger();
    other.id = "other-checks".to_string();
    let diagnostics =
        apply_trigger_operations(&invalid, &[TriggerEditOperation::Create { trigger: other }])
            .unwrap_err();
    assert_eq!(diagnostics[0].code, "invalid_trigger_ref");
    assert!(apply_trigger_operations(&invalid, &[]).is_err());
    let repaired = apply_trigger_operations(
        &invalid,
        &[TriggerEditOperation::Update {
            id: trigger().id,
            trigger: trigger(),
        }],
    )
    .unwrap();
    assert_eq!(
        load_triggers_toml_str("struo.toml", &repaired).unwrap(),
        [trigger()]
    );
    let removed = apply_trigger_operations(
        &invalid,
        &[TriggerEditOperation::Delete { id: trigger().id }],
    )
    .unwrap();
    assert!(load_triggers_toml_str("struo.toml", &removed)
        .unwrap()
        .is_empty());
}

#[test]
fn full_replacement_or_delete_can_repair_structurally_invalid_records() {
    for invalid in [
        SOURCE.replace("id =", "unknown = true\nid ="),
        SOURCE.replace(", tokens = []", ""),
    ] {
        assert!(load_triggers_toml_str("struo.toml", &invalid).is_err());
        let repaired = apply_trigger_operations(
            &invalid,
            &[TriggerEditOperation::Update {
                id: trigger().id,
                trigger: trigger(),
            }],
        )
        .unwrap();
        assert_eq!(
            load_triggers_toml_str("struo.toml", &repaired).unwrap(),
            [trigger()]
        );
        let removed = apply_trigger_operations(
            &invalid,
            &[TriggerEditOperation::Delete { id: trigger().id }],
        )
        .unwrap();
        assert!(load_triggers_toml_str("struo.toml", &removed)
            .unwrap()
            .is_empty());
    }
}

#[test]
fn final_validation_rejects_colliding_renames_and_normalized_duplicates() {
    let mut other = trigger();
    other.id = "other-checks".to_string();
    let created = apply_trigger_operations(
        SOURCE,
        &[TriggerEditOperation::Create {
            trigger: other.clone(),
        }],
    )
    .unwrap();
    let error = apply_trigger_operations(
        &created,
        &[TriggerEditOperation::Update {
            id: other.id,
            trigger: trigger(),
        }],
    )
    .unwrap_err();
    assert_eq!(error[0].code, "duplicate_trigger_id");
    let mut duplicated = trigger();
    duplicated.invoke.push(duplicated.invoke[0].clone());
    let error = apply_trigger_operations(
        SOURCE,
        &[TriggerEditOperation::Update {
            id: trigger().id,
            trigger: duplicated,
        }],
    )
    .unwrap_err();
    assert_eq!(error[0].code, "duplicate_trigger_invocation");
}

#[test]
fn can_create_and_delete_the_first_trigger_and_noop_is_byte_preserving() {
    let src = "# existing comment\n[workspace]\ninclude = []\n";
    assert_eq!(apply_trigger_operations(src, &[]).unwrap(), src);
    let created =
        apply_trigger_operations(src, &[TriggerEditOperation::Create { trigger: trigger() }])
            .unwrap();
    assert_eq!(
        load_triggers_toml_str("struo.toml", &created).unwrap(),
        [trigger()]
    );
    assert!(created.contains(src));
    let removed = apply_trigger_operations(
        &created,
        &[TriggerEditOperation::Delete { id: trigger().id }],
    )
    .unwrap();
    assert_eq!(removed, src);
}

#[test]
fn edit_operations_use_strict_tagged_serde_contract() {
    let operation = TriggerEditOperation::Create { trigger: trigger() };
    let value = serde_json::to_value(&operation).unwrap();
    assert_eq!(value["op"], "create");
    assert_eq!(
        serde_json::from_value::<TriggerEditOperation>(value.clone()).unwrap(),
        operation
    );
    let mut unknown = value;
    unknown["extra"] = serde_json::Value::Bool(true);
    assert!(serde_json::from_value::<TriggerEditOperation>(unknown).is_err());
    assert!(
        serde_json::from_str::<TriggerEditOperation>(r#"{"op":"update","id":"core-checks"}"#)
            .is_err()
    );
}

use std::collections::BTreeSet;

use super::references::{canonical_target, valid_id, valid_ref};
use super::{TriggerInvocation, TriggerSpec};
use crate::Diagnostic;

pub fn validate_triggers(triggers: &[TriggerSpec]) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut ids = BTreeSet::new();
    for trigger in triggers {
        if !valid_id(&trigger.id) {
            diagnostics.push(problem(
                trigger,
                "invalid_trigger_id",
                "id",
                "trigger id must be a simple stable name",
                "Use an ASCII letter or digit followed by letters, digits, underscores, or hyphens",
            ));
        }
        if !ids.insert(&trigger.id) {
            diagnostics.push(problem(
                trigger,
                "duplicate_trigger_id",
                "id",
                "trigger ids must be unique within the workspace",
                "Give each trigger a distinct stable id",
            ));
        }
        validate_on(trigger, &mut diagnostics);
        if trigger.invoke.is_empty() {
            diagnostics.push(problem(
                trigger,
                "trigger_invocations_empty",
                "invoke",
                "trigger invoke must contain at least one invocation",
                "Add an invocation with target, capability, and explicit configuration",
            ));
        }
        let mut members = BTreeSet::new();
        for (index, invocation) in trigger.invoke.iter().enumerate() {
            validate_invocation(trigger, index, invocation, &mut diagnostics);
            if let (Ok(target), Ok(configuration)) = (
                canonical_target(&invocation.target),
                invocation.configuration.resolve(),
            ) {
                let key = (
                    target,
                    &invocation.capability,
                    configuration.canonical_bytes(),
                );
                if !members.insert(key) {
                    diagnostics.push(problem(
                        trigger, "duplicate_trigger_invocation", &format!("invoke[{index}]"),
                        "trigger contains duplicate invocations after configuration normalization",
                        "Keep one invocation for each target, capability, and effective configuration",
                    ));
                }
            }
        }
    }
    diagnostics
}

fn validate_on(trigger: &TriggerSpec, diagnostics: &mut Vec<Diagnostic>) {
    if trigger.on.event != "vcs.ref_changed" {
        diagnostics.push(problem(
            trigger,
            "unsupported_trigger_event",
            "on.event",
            "unsupported trigger event",
            "Set on.event = \"vcs.ref_changed\"",
        ));
    }
    if trigger.on.refs.is_empty() {
        diagnostics.push(problem(
            trigger,
            "trigger_refs_empty",
            "on.refs",
            "trigger refs must contain at least one fully qualified ref",
            "Set on.refs = [\"refs/heads/main\"] or another exact fully qualified ref",
        ));
    }
    let mut refs = BTreeSet::new();
    for (index, reference) in trigger.on.refs.iter().enumerate() {
        let attribute = format!("on.refs[{index}]");
        if !valid_ref(reference) {
            diagnostics.push(problem(
                trigger, "invalid_trigger_ref", &attribute,
                "trigger refs must be exact fully qualified refs without patterns or control characters",
                "Use a valid exact ref such as refs/heads/main or refs/tags/v1.0",
            ));
        }
        if !refs.insert(reference) {
            diagnostics.push(problem(
                trigger,
                "duplicate_trigger_ref",
                &attribute,
                "trigger contains duplicate refs",
                "Keep each exact ref only once",
            ));
        }
    }
}

fn validate_invocation(
    trigger: &TriggerSpec,
    index: usize,
    invocation: &TriggerInvocation,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let prefix = format!("invoke[{index}]");
    if let Err(message) = canonical_target(&invocation.target) {
        diagnostics.push(problem(
            trigger, "invalid_trigger_target", &format!("{prefix}.target"), &message,
            "Use an exact canonical workspace-relative target id such as checks/core, not native query shorthand",
        ));
    }
    if !matches!(invocation.capability.as_str(), "build" | "test" | "run") {
        diagnostics.push(problem(
            trigger,
            "unsupported_trigger_capability",
            &format!("{prefix}.capability"),
            "unsupported trigger capability",
            "Set capability to build, test, or run",
        ));
    }
    for (field, value) in [
        ("os", &invocation.configuration.os),
        ("arch", &invocation.configuration.arch),
    ] {
        if value.trim().is_empty() {
            diagnostics.push(problem(
                trigger, "invalid_trigger_configuration", &format!("{prefix}.configuration.{field}"),
                "trigger configuration platform values must be explicit and non-empty",
                "Set configuration.os and configuration.arch explicitly, for example linux and x86_64",
            ));
        }
    }
    let mut tokens = BTreeSet::new();
    for (index, token) in invocation.configuration.tokens.iter().enumerate() {
        let attribute = format!("{prefix}.configuration.tokens[{index}]");
        if token.trim().is_empty() {
            diagnostics.push(problem(
                trigger,
                "invalid_trigger_configuration",
                &attribute,
                "trigger configuration tokens must be non-empty",
                "Remove empty tokens; use tokens = [] when no additional tokens are needed",
            ));
        }
        if !tokens.insert(token) {
            diagnostics.push(problem(
                trigger,
                "duplicate_trigger_token",
                &attribute,
                "trigger contains duplicate configuration tokens",
                "Keep each additional token only once",
            ));
        }
    }
}

fn problem(
    trigger: &TriggerSpec,
    code: &str,
    attribute: &str,
    message: &str,
    repair: &str,
) -> Diagnostic {
    Diagnostic::new(code, message)
        .with_target(&trigger.id)
        .with_attribute(attribute)
        .with_repair(repair)
}

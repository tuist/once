use crate::normalize_manifest_target;

pub(super) fn valid_id(id: &str) -> bool {
    id.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && id
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'-'))
}

#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "Git ref components use a case-sensitive .lock suffix rule"
)]
pub(super) fn valid_ref(reference: &str) -> bool {
    reference.starts_with("refs/")
        && !reference.contains("..")
        && !reference.contains("@{")
        && !reference.ends_with('.')
        && !reference.chars().any(|ch| {
            ch.is_control() || ch == ' ' || matches!(ch, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
        })
        && reference
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}

pub(super) fn canonical_target(target: &str) -> Result<String, String> {
    let normalized = normalize_manifest_target("", target).map_err(|error| error.to_string())?;
    if normalized != target {
        return Err(format!(
            "use the canonical workspace-relative target id `{normalized}`"
        ));
    }
    if target
        .chars()
        .any(|ch| ch.is_control() || matches!(ch, '*' | '?' | '[' | ']'))
    {
        return Err("target ids must be exact, not query expressions or patterns".to_string());
    }
    Ok(normalized)
}

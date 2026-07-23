use std::collections::BTreeMap;
use std::path::Path;

pub const EXECUTION_ROOT_MARKER: &str = "{{once.execution_root}}";

#[must_use]
pub fn resolve_execution_value(value: &str, execution_root: &Path) -> String {
    if !value.contains(EXECUTION_ROOT_MARKER) {
        return value.to_string();
    }
    value.replace(
        EXECUTION_ROOT_MARKER,
        execution_root.to_string_lossy().as_ref(),
    )
}

#[must_use]
pub fn resolve_execution_argv(argv: &[String], execution_root: &Path) -> Vec<String> {
    argv.iter()
        .map(|value| resolve_execution_value(value, execution_root))
        .collect()
}

#[must_use]
pub fn resolve_execution_env(
    env: &BTreeMap<String, String>,
    execution_root: &Path,
) -> BTreeMap<String, String> {
    env.iter()
        .map(|(key, value)| (key.clone(), resolve_execution_value(value, execution_root)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_markers_without_changing_unmarked_values() {
        assert_eq!(
            resolve_execution_value("{{once.execution_root}}/.once/cache", Path::new("/sandbox")),
            "/sandbox/.once/cache"
        );
        assert_eq!(
            resolve_execution_value("plain", Path::new("/sandbox")),
            "plain"
        );
    }

    #[test]
    fn resolves_markers_inside_environment_lists() {
        assert_eq!(
            resolve_execution_value(
                "{{once.execution_root}}/bin:/usr/bin",
                Path::new("/workspace")
            ),
            "/workspace/bin:/usr/bin"
        );
    }
}

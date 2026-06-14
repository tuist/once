use std::collections::BTreeSet;
use std::path::Path;

/// Wrap a declared argv as `/bin/sh -c "mkdir -p <output dirs> && <argv>"`.
///
/// Real toolchains expect their output directories to exist. Doing
/// the `mkdir -p` inside the script keeps the whole compile expressed
/// as one cacheable action.
pub(super) fn wrap_in_script(argv: &[String], outputs: &[String]) -> String {
    let mut script = String::from("set -eu\n");
    let mut seen_dirs = BTreeSet::new();
    for output in outputs {
        if let Some(parent) = Path::new(output).parent().and_then(|path| path.to_str()) {
            if !parent.is_empty() && seen_dirs.insert(parent.to_string()) {
                script.push_str("mkdir -p ");
                script.push_str(&shell_quote(parent));
                script.push('\n');
            }
        }
    }
    let mut first = true;
    for arg in argv {
        if !first {
            script.push(' ');
        }
        first = false;
        script.push_str(&shell_quote(arg));
    }
    script.push('\n');
    script
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    let escaped = value.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("A'B"), "'A'\"'\"'B'");
    }

    #[test]
    fn wrap_script_prepends_mkdir_for_each_output_parent() {
        let outputs = vec![
            ".once/out/x/A.a".to_string(),
            ".once/out/x/A.swiftmodule".to_string(),
            ".once/out/x/sub/B.swiftdoc".to_string(),
        ];
        let script = wrap_in_script(&["swiftc".to_string(), "-o".to_string()], &outputs);
        assert!(script.contains("mkdir -p '.once/out/x'"));
        assert!(script.contains("mkdir -p '.once/out/x/sub'"));
        assert_eq!(script.matches("mkdir -p '.once/out/x'\n").count(), 1);
    }
}

//! Client-side argv/cwd redaction per RFC 0008 §Identity and redaction,
//! extended by RFC 0009 §Workspace Safe Context.
//!
//! The wire proto's `RunStarted.argv_normalized` carries a list of
//! [`ArgvToken`]s produced by walking the raw argument list once,
//! left-to-right:
//!
//! * A token on the frozen safe-literal allowlist emits `SafeLiteral`.
//! * A token whose exact value appears in the caller-supplied
//!   [`SafeContext`] (currently opted-in graph target labels) also
//!   emits `SafeLiteral`. RFC 0009 defines the opt-in policy and
//!   stamps a new allowlist version.
//! * `--flag` / `-x` boolean flags emit `FlagKey`.
//! * `--key=value` combined form splits, hashing `value` under a
//!   project-scoped BLAKE3 key. If the value is in the [`SafeContext`],
//!   `value_shape_hash` is skipped and the value is emitted as a
//!   `SafeLiteral` immediately after the `FlagKey` instead. This
//!   preserves the "no cross-token inference" rule of RFC 0008 for
//!   values that already round-trip through the manifest.
//! * Anything else hashes to `OpaqueValue` under the same key.
//!
//! Nothing is inferred across token boundaries (`-p once-core` stays two
//! independent tokens); tool-specific parsing is deliberately out of
//! scope for v1.
//!
//! The safe-literal allowlist is version-stamped so the server can
//! quarantine unrecognised safe literals from an older client under a
//! projection warning without breaking ingestion.

use blake3::Hasher;
use once_events_client::proto::{argv_token::Token, ArgvToken, NamedValue};
use std::collections::HashSet;

/// Safe-literal allowlist version. Bumped from `2026.09.03-v1` when
/// RFC 0009 landed the workspace-context extension: a v2 client
/// classifies workspace-manifest tokens as `SafeLiteral`, and the
/// server accepts them under this version only. Older clients keep
/// declaring `v1` and continue to hash those tokens.
pub const SAFE_LITERAL_ALLOWLIST_VERSION: &str = "2026.09.15-v2";
pub const BASE_SAFE_LITERAL_ALLOWLIST_VERSION: &str = "2026.09.03-v1";

/// The safe literals owned by Once's generic command surface.
///
/// Toolchain and test-framework names deliberately stay opaque. A target kind
/// can introduce a tool without requiring the Rust event client and the event
/// service to learn that tool's name.
pub const SAFE_LITERALS: &[&str] = &[
    "once", "build", "test", "run", "check", "install", "update", "lint", "format", "fmt", "bench",
    "doc", "clean", "add", "remove", "publish", "release", "debug",
];

/// Set of explicitly opted-in workspace target labels the client can
/// emit verbatim as `SafeLiteral` in addition to the frozen allowlist.
///
/// See RFC 0009 §Workspace Safe Context.
#[derive(Debug, Clone, Default)]
pub struct SafeContext {
    values: HashSet<String>,
}

impl SafeContext {
    /// Empty context. Equivalent to strict mode: the classifier
    /// falls back to the frozen allowlist behaviour of RFC 0008.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Add one workspace-known token to the safe set. Absolute
    /// filesystem paths and tokens containing `=` are dropped on
    /// principle: paths escape the workspace boundary, and `=`
    /// tokens are routed through `split_named_value` regardless.
    pub fn insert<S: Into<String>>(&mut self, value: S) {
        let value = value.into();
        if is_unsafe_workspace_token(&value) {
            return;
        }
        self.values.insert(value);
    }

    /// Bulk-insert an iterable of workspace tokens through
    /// [`Self::insert`]'s filter.
    pub fn extend<I, S>(&mut self, iter: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        for value in iter {
            self.insert(value);
        }
    }

    fn contains(&self, value: &str) -> bool {
        self.values.contains(value)
    }
}

/// Normalise an argument vector under a project-scoped key. The key
/// bytes are the response of `RunEventService::GetArgvHashKey` for the
/// current project; callers cache it for the run's lifetime.
///
/// Callers with explicit disclosure policy may use
/// [`normalize_argv_with_context`] for graph target labels. Without
/// that policy, this function is equivalent to
/// `normalize_argv_with_context(argv, key_bytes, &SafeContext::empty())`.
#[cfg(test)]
pub fn normalize_argv<T: AsRef<str>>(argv: &[T], key_bytes: &[u8]) -> Vec<ArgvToken> {
    normalize_argv_with_context(argv, key_bytes, &SafeContext::empty())
}

/// Normalise an argument vector under a project-scoped key, extended
/// with the caller's [`SafeContext`]. See module docs for the token
/// classification rules and RFC 0009 for the design rationale.
pub fn normalize_argv_with_context<T: AsRef<str>>(
    argv: &[T],
    key_bytes: &[u8],
    context: &SafeContext,
) -> Vec<ArgvToken> {
    argv.iter()
        .map(|arg| normalize_one(arg.as_ref(), key_bytes, context))
        .collect()
}

fn is_unsafe_workspace_token(value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    if value.starts_with('/') {
        return true;
    }
    if value.contains('=') {
        return true;
    }
    // Windows drive letters (`C:\...`, `D:/...`). Non-drive
    // tokens like `//foo:bar` still enter the set; they are Once
    // target labels, not paths.
    if let Some((prefix, rest)) = value.split_once(':') {
        if prefix.len() == 1
            && prefix
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
            && (rest.starts_with('/') || rest.starts_with('\\'))
        {
            return true;
        }
    }
    false
}

/// Turn a workspace-absolute cwd into a workspace-relative path. A
/// value that would traverse outside the workspace (leading `/`, a
/// `..` segment, a drive letter) is rejected as an empty string so the
/// server-side traversal check doesn't have to salvage it.
pub fn cwd_relative(cwd: &std::path::Path, workspace: &std::path::Path) -> String {
    match cwd.strip_prefix(workspace) {
        Ok(rel) => {
            let display = rel.to_string_lossy();
            if display.is_empty() {
                ".".to_string()
            } else {
                display.replace('\\', "/")
            }
        }
        Err(_) => String::new(),
    }
}

fn normalize_one(token: &str, key_bytes: &[u8], context: &SafeContext) -> ArgvToken {
    if is_safe_literal(token) || context.contains(token) {
        return ArgvToken {
            token: Some(Token::SafeLiteral(token.to_string())),
        };
    }
    if let Some((key, value)) = split_named_value(token) {
        // The combined `--key=value` form still splits before the
        // context check because the context only holds bare values.
        // If the value is workspace-known, emit `key=value` in a
        // shape the projector renders verbatim; otherwise fall
        // back to the RFC 0008 keyed hash.
        if context.contains(value) {
            return ArgvToken {
                token: Some(Token::SafeLiteral(format!("{key}={value}"))),
            };
        }
        return ArgvToken {
            token: Some(Token::NamedValue(NamedValue {
                key: key.to_string(),
                value_shape_hash: hash_value(value, key_bytes),
            })),
        };
    }
    if is_flag_key(token) {
        return ArgvToken {
            token: Some(Token::FlagKey(token.to_string())),
        };
    }
    ArgvToken {
        token: Some(Token::OpaqueValueHash(hash_value(token, key_bytes))),
    }
}

fn is_safe_literal(token: &str) -> bool {
    // Compare case-insensitively for tool names carried through argv0
    // (`Cargo.exe` on Windows lowercases to `cargo.exe`; the basename is
    // resolved elsewhere).
    let lower = token.to_ascii_lowercase();
    SAFE_LITERALS.iter().any(|literal| *literal == lower)
}

// `--flag` / `-x` boolean flags, without a value. Combined `--key=value`
// is handled by `split_named_value`. `-` alone is not a flag.
fn is_flag_key(token: &str) -> bool {
    if token.len() < 2 {
        return false;
    }
    if !token.starts_with('-') {
        return false;
    }
    // Rules out combined forms like `--key=value`; those go through
    // `split_named_value`.
    !token.contains('=')
}

fn split_named_value(token: &str) -> Option<(&str, &str)> {
    if !token.starts_with('-') {
        return None;
    }
    let (key, value) = token.split_once('=')?;
    if key.len() < 2 {
        return None;
    }
    Some((key, value))
}

fn hash_value(value: &str, key_bytes: &[u8]) -> String {
    let mut hasher = Hasher::new_keyed(&key32(key_bytes));
    hasher.update(value.as_bytes());
    let digest = hasher.finalize();
    // First 128 bits base32-encoded gives a compact, stable identifier
    // (32 hex chars would round-trip too; hex is nicer to log).
    hex::encode(&digest.as_bytes()[..16])
}

fn key32(bytes: &[u8]) -> [u8; 32] {
    let mut key = [0u8; 32];
    let take = bytes.len().min(32);
    key[..take].copy_from_slice(&bytes[..take]);
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> Vec<u8> {
        b"argv-normalize-test-key-0000000000000".to_vec()
    }

    fn context_with<S: Into<String>>(values: impl IntoIterator<Item = S>) -> SafeContext {
        let mut ctx = SafeContext::empty();
        ctx.extend(values);
        ctx
    }

    #[test]
    fn once_vocabulary_is_preserved_without_knowing_the_toolchain() {
        let tokens = normalize_argv(&["toolchain-command", "build"], &key());
        assert!(matches!(tokens[0].token, Some(Token::OpaqueValueHash(_))));
        assert!(matches!(tokens[1].token, Some(Token::SafeLiteral(ref v)) if v == "build"));
    }

    #[test]
    fn positional_after_flag_hashes_as_opaque_without_context() {
        let tokens = normalize_argv(&["toolchain-command", "test", "-p", "once-core"], &key());
        assert!(matches!(tokens[2].token, Some(Token::FlagKey(ref k)) if k == "-p"));
        assert!(matches!(tokens[3].token, Some(Token::OpaqueValueHash(_))));
    }

    #[test]
    fn workspace_known_positional_emits_safe_literal() {
        let context = context_with(["once-core", "my-app"]);
        let tokens = normalize_argv_with_context(
            &["toolchain-command", "test", "-p", "once-core"],
            &key(),
            &context,
        );
        assert!(matches!(tokens[2].token, Some(Token::FlagKey(ref k)) if k == "-p"));
        assert!(
            matches!(tokens[3].token, Some(Token::SafeLiteral(ref v)) if v == "once-core"),
            "workspace-known positional should emit as SafeLiteral, got {:?}",
            tokens[3].token
        );
    }

    #[test]
    fn workspace_unknown_positional_still_hashes_with_context() {
        let context = context_with(["once-core"]);
        let tokens = normalize_argv_with_context(
            &["once", "build", "totally-random-target"],
            &key(),
            &context,
        );
        assert!(matches!(tokens[2].token, Some(Token::OpaqueValueHash(_))));
    }

    #[test]
    fn combined_named_value_with_workspace_known_value_emits_safe_literal() {
        let context = context_with(["telemetry"]);
        let tokens = normalize_argv_with_context(
            &["toolchain-command", "build", "--features=telemetry"],
            &key(),
            &context,
        );
        match tokens[2].token.as_ref() {
            Some(Token::SafeLiteral(value)) => {
                assert_eq!(value, "--features=telemetry");
            }
            other => panic!("expected SafeLiteral, got {other:?}"),
        }
    }

    #[test]
    fn absolute_paths_are_never_added_to_context() {
        let mut context = SafeContext::empty();
        context.insert("/Users/pedro/private/build");
        context.insert("C:/Users/pedro/build");
        let tokens = normalize_argv_with_context(
            &["once", "build", "/Users/pedro/private/build"],
            &key(),
            &context,
        );
        assert!(
            matches!(tokens[2].token, Some(Token::OpaqueValueHash(_))),
            "absolute path must not be classified as SafeLiteral"
        );
    }

    #[test]
    fn combined_named_value_splits() {
        let tokens = normalize_argv(
            &["toolchain-command", "build", "--target=target-triple"],
            &key(),
        );
        match tokens[2].token.as_ref() {
            Some(Token::NamedValue(nv)) => {
                assert_eq!(nv.key, "--target");
                assert!(!nv.value_shape_hash.is_empty());
            }
            _ => panic!("expected NamedValue"),
        }
    }

    #[test]
    fn same_value_same_hash_under_same_key() {
        let a = normalize_argv(&["toolchain-command", "run", "prod"], &key());
        let b = normalize_argv(&["toolchain-command", "run", "prod"], &key());
        match (a[2].token.as_ref(), b[2].token.as_ref()) {
            (Some(Token::OpaqueValueHash(h1)), Some(Token::OpaqueValueHash(h2))) => {
                assert_eq!(h1, h2);
            }
            _ => panic!("expected OpaqueValueHash"),
        }
    }

    #[test]
    fn same_value_different_hash_under_different_key() {
        let a = normalize_argv(&["toolchain-command", "run", "prod"], &key());
        let b = normalize_argv(&["toolchain-command", "run", "prod"], b"different-key");
        match (a[2].token.as_ref(), b[2].token.as_ref()) {
            (Some(Token::OpaqueValueHash(h1)), Some(Token::OpaqueValueHash(h2))) => {
                assert_ne!(h1, h2);
            }
            _ => panic!("expected OpaqueValueHash"),
        }
    }

    #[test]
    fn cwd_traversal_rejected() {
        let workspace = std::path::Path::new("/repo");
        let cwd = std::path::Path::new("/etc/passwd");
        assert_eq!(cwd_relative(cwd, workspace), "");
    }

    #[test]
    fn cwd_relative_from_workspace() {
        let workspace = std::path::Path::new("/repo");
        let cwd = std::path::Path::new("/repo/crates/once-cli");
        assert_eq!(cwd_relative(cwd, workspace), "crates/once-cli");
    }
}

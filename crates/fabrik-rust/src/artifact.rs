//! Deterministic output paths for Rust artifacts and the bookkeeping
//! that lets one crate's outputs feed another's `--extern` flags.

use fabrik_cas::Digest;

/// The kinds of Rust target this crate knows how to compile. Kept
/// separate from the raw target `kind` string so the rest of the
/// code can match exhaustively without re-parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RustKind {
    Library,
    Binary,
    Test,
    ProcMacro,
    BuildScript,
}

impl RustKind {
    /// Recognise a target `kind` string. Unknown kinds are surfaced
    /// to the caller as `None` so callers can produce an actionable
    /// error mentioning the name.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "rust_library" => Some(Self::Library),
            "rust_binary" => Some(Self::Binary),
            "rust_test" => Some(Self::Test),
            "rust_proc_macro" => Some(Self::ProcMacro),
            "cargo_build_script" => Some(Self::BuildScript),
            _ => None,
        }
    }

    pub fn is_rustc_target(self) -> bool {
        !matches!(self, Self::BuildScript)
    }

    /// The kind's user-facing name, matching the target kind string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Library => "rust_library",
            Self::Binary => "rust_binary",
            Self::Test => "rust_test",
            Self::ProcMacro => "rust_proc_macro",
            Self::BuildScript => "cargo_build_script",
        }
    }
}

/// A built dependency, as seen by a downstream rustc invocation. The
/// `crate_name` is what dependents type into Rust source (`use foo;`).
/// `extern_path` is what gets passed to `--extern` - an `rlib` for
/// libraries, the platform `dylib` for proc-macros. The
/// `action_digest` is mixed into the dependent action's `input_digest`
/// so that a change to a transitive dep correctly invalidates this
/// crate's cache slot.
#[derive(Debug, Clone)]
pub struct DepArtifact {
    pub crate_name: String,
    /// Path passed to `--extern <crate_name>=<this>`.
    pub extern_path: String,
    /// Sibling rmeta, useful for metadata-only pipelining.
    pub rmeta_path: String,
    pub out_dir: String,
    pub action_digest: Digest,
    pub kind: RustKind,
    pub build_script_outputs: Option<String>,
}

/// Map a Cargo-style hyphenated package name to a Rust identifier by
/// replacing `-` with `_`. This is the same default rustc applies for
/// `--crate-name` derived from a package name.
pub fn default_crate_name(target_name: &str) -> String {
    target_name.replace('-', "_")
}

/// Workspace-relative output directory for a target. Mirrors the
/// existing convention used by single-action `rust_binary` runs:
/// outputs land under `.fabrik/out/<package>/`, with the file inside
/// named after the target. All build outputs share the gitignored
/// `.fabrik/` root with the cache.
pub fn out_dir(package: &str, _name: &str) -> String {
    if package.is_empty() {
        ".fabrik/out".to_string()
    } else {
        format!(".fabrik/out/{package}")
    }
}

/// Workspace-relative path to a library's rlib. Mirrors rustc's
/// default file naming (`lib<crate_name>.rlib`).
pub fn rlib_path(out_dir: &str, crate_name: &str) -> String {
    format!("{out_dir}/lib{crate_name}.rlib")
}

/// Workspace-relative path to a library's rmeta sibling.
pub fn rmeta_path(out_dir: &str, crate_name: &str) -> String {
    format!("{out_dir}/lib{crate_name}.rmeta")
}

/// Workspace-relative path to a binary executable. Adds `.exe` on
/// Windows so the output matches what rustc actually writes.
pub fn binary_path(out_dir: &str, name: &str) -> String {
    format!("{out_dir}/{name}{}", std::env::consts::EXE_SUFFIX)
}

/// Workspace-relative path to a proc-macro dynamic library. Cargo
/// names it `lib<crate_name>.{dylib,so,dll}` depending on platform.
pub fn proc_macro_path(out_dir: &str, crate_name: &str) -> String {
    let ext = std::env::consts::DLL_EXTENSION;
    format!("{out_dir}/lib{crate_name}.{ext}")
}

/// Workspace-relative path to the stdout captured from a build script.
pub fn build_script_outputs_path(out_dir: &str, name: &str) -> String {
    format!("{out_dir}/{name}_build_script.out")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_crate_name_replaces_hyphens() {
        assert_eq!(default_crate_name("fabrik-cas"), "fabrik_cas");
        assert_eq!(default_crate_name("fabrik"), "fabrik");
    }

    #[test]
    fn out_dir_handles_root_package() {
        assert_eq!(out_dir("", "hello"), ".fabrik/out");
        assert_eq!(
            out_dir("crates/fabrik-cas", "fabrik-cas"),
            ".fabrik/out/crates/fabrik-cas"
        );
    }

    #[test]
    fn rust_kind_parse_recognises_known_kinds() {
        assert_eq!(RustKind::parse("rust_library"), Some(RustKind::Library));
        assert_eq!(RustKind::parse("rust_binary"), Some(RustKind::Binary));
        assert_eq!(RustKind::parse("rust_test"), Some(RustKind::Test));
        assert_eq!(
            RustKind::parse("rust_proc_macro"),
            Some(RustKind::ProcMacro)
        );
        assert_eq!(
            RustKind::parse("cargo_build_script"),
            Some(RustKind::BuildScript)
        );
        assert_eq!(RustKind::parse("cargo_binary"), None);
    }
}

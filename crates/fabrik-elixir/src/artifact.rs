//! Deterministic output paths for Elixir artifacts and the bookkeeping
//! that lets one target's `.ebin` directory feed another's `-pa` flag.

use fabrik_cas::Digest;

/// The kinds of Elixir target this crate compiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElixirKind {
    Library,
    Binary,
}

impl ElixirKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "elixir_library" => Some(Self::Library),
            "elixir_binary" => Some(Self::Binary),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Library => "elixir_library",
            Self::Binary => "elixir_binary",
        }
    }
}

/// A built dependency, as seen by a downstream `elixirc` invocation.
/// The `ebin_dir` is what gets passed to `-pa <dir>` so the BEAM code
/// path resolves the dep's modules at compile time. The `action_digest`
/// flows into the dependent action's `input_digest` so a change to a
/// transitive dep invalidates this target's cache slot.
#[derive(Debug, Clone)]
pub struct BeamArtifact {
    pub ebin_dir: String,
    pub action_digest: Digest,
    pub kind: ElixirKind,
}

/// Workspace-relative `.ebin` directory for a target. Mirrors OTP's
/// per-application `ebin/` convention so the layout stays familiar to
/// anyone reading the cache contents on disk.
pub fn ebin_dir(package: &str, name: &str) -> String {
    if package.is_empty() {
        format!(".fabrik/out/{name}.ebin")
    } else {
        format!(".fabrik/out/{package}/{name}.ebin")
    }
}

/// Workspace-relative path to the launcher escript that an
/// `elixir.binary` produces.
pub fn escript_path(package: &str, name: &str) -> String {
    if package.is_empty() {
        format!(".fabrik/out/{name}")
    } else {
        format!(".fabrik/out/{package}/{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ebin_dir_handles_root_package() {
        assert_eq!(ebin_dir("", "hello"), ".fabrik/out/hello.ebin");
        assert_eq!(ebin_dir("apps/foo", "foo"), ".fabrik/out/apps/foo/foo.ebin");
    }

    #[test]
    fn escript_path_handles_root_package() {
        assert_eq!(escript_path("", "cli"), ".fabrik/out/cli");
        assert_eq!(escript_path("apps/foo", "cli"), ".fabrik/out/apps/foo/cli");
    }

    #[test]
    fn elixir_kind_round_trip() {
        assert_eq!(
            ElixirKind::parse("elixir_library"),
            Some(ElixirKind::Library)
        );
        assert_eq!(ElixirKind::parse("elixir_binary"), Some(ElixirKind::Binary));
        assert_eq!(ElixirKind::parse("elixir_test"), None);
        assert_eq!(ElixirKind::parse("python_library"), None);
        assert_eq!(ElixirKind::Library.as_str(), "elixir_library");
    }
}

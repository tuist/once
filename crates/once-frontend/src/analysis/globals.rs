use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde_json::Value as JsonValue;
use starlark::environment::{Globals, GlobalsBuilder};
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::none::NoneType;
use starlark::values::Value;

use super::store::{analysis_active, with_store, with_store_mut, DeclaredAction};
use super::values::{
    json_to_value, toml_value_to_starlark, unpack_byte_list, unpack_string_dict, unpack_string_list,
};

/// Globals exposed to the prelude.
///
/// The set is intentionally generic: anything platform- or
/// toolchain-specific is implemented in starlark on top of these
/// primitives. Schema parsing references the names without invoking
/// them, so the bodies short-circuit to inert values when no
/// [`AnalysisStore`] is installed.
#[must_use]
pub fn globals_for_prelude() -> Globals {
    GlobalsBuilder::standard().with(prelude_globals).build()
}

#[starlark_module]
fn prelude_globals(builder: &mut GlobalsBuilder) {
    /// Host CPU architecture as a normalized string (e.g. `"arm64"`,
    /// `"x86_64"`). Schema parsing returns `""`.
    #[allow(clippy::unnecessary_wraps)]
    fn host_arch() -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        Ok(host_arch_str().to_string())
    }

    /// Host operating system as a normalized string (e.g. `"macos"`,
    /// `"linux"`). Schema parsing returns `""`.
    #[allow(clippy::unnecessary_wraps)]
    fn host_os() -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        Ok(host_os_str().to_string())
    }

    /// Active workspace root as an absolute path. Schema parsing
    /// returns `""`.
    fn workspace_root() -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        with_store(|store| -> Result<String> {
            let store = store.ok_or_else(|| anyhow!("workspace_root called outside analysis"))?;
            Ok(store.workspace_root.to_string_lossy().into_owned())
        })
    }

    /// Find `name` on `PATH` and return its absolute path. Fails if
    /// the binary is not found. Schema parsing returns `""`.
    fn host_which(name: &str) -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        let resolved = with_store(|store| -> Result<Option<String>> {
            let store = store.ok_or_else(|| anyhow!("host_which called outside analysis"))?;
            store.host_cache.which(name)
        })?;
        resolved.ok_or_else(|| anyhow!("`{name}` not found on PATH"))
    }

    /// Run `argv[0]` with `argv[1..]` as arguments and return its
    /// stdout as a string. Fails if the process exits non-zero;
    /// includes stderr in the error message. Optional `env` is a
    /// `dict<string, string>` of environment variables overlaid on the
    /// host process env. Both `argv` and `env` participate in the
    /// cache key, so a different `DEVELOPER_DIR` resolves to a
    /// different cached result. Schema parsing returns `""`.
    fn host_command<'v>(argv: Value<'v>, env: Option<Value<'v>>) -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        let argv = unpack_string_list(argv, "argv")?;
        let env = env
            .map(|value| unpack_string_dict(value, "env"))
            .transpose()?
            .unwrap_or_default();
        with_store(|store| -> Result<String> {
            let store = store.ok_or_else(|| anyhow!("host_command called outside analysis"))?;
            store.host_cache.command(&argv, &env)
        })
    }

    /// Expand a list of glob patterns against the active target's
    /// package directory. Returns sorted, deduplicated, workspace-
    /// relative file paths. Schema parsing returns an empty list.
    fn glob<'v>(
        patterns: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let heap = eval.heap();
        if !analysis_active() {
            return Ok(heap.alloc(Vec::<String>::new()));
        }
        let patterns = unpack_string_list(patterns, "patterns")?;
        let resolved = with_store(|store| -> Result<Vec<String>> {
            let store = store.ok_or_else(|| anyhow!("glob called outside analysis"))?;
            expand_globs(&store.workspace_root, &store.package, &patterns)
        })?;
        Ok(heap.alloc(resolved))
    }

    /// Reserve a workspace-relative output path under the active
    /// target's build directory and return it. Outside analysis this
    /// returns the bare name.
    fn declare_output(name: &str) -> anyhow::Result<String> {
        with_store_mut(|store| match store {
            Some(store) => {
                let path = format!("{}/{}", store.build_dir, name);
                store.declared_outputs.push(path.clone());
                Ok(path)
            }
            None => Ok(name.to_string()),
        })
    }

    /// Declare an action that materialises `content` at the workspace-
    /// relative `path`. The content is hashed into the input digest so
    /// any edit (including in starlark that produced it) invalidates
    /// downstream consumers.
    ///
    /// Implementation note: the materialisation runs as `/bin/sh -c`
    /// with the path bound to a shell variable first; the parent
    /// directory is computed via the POSIX `${var%/*}` parameter
    /// expansion. Passing `shell_quote(path)` directly inside
    /// `$(dirname ...)` would re-tokenize the escaped quotes a path
    /// like `a'b/c.h` ends up with, so binding once and dereferencing
    /// twice keeps the action robust against single quotes in paths.
    #[allow(clippy::unnecessary_wraps)]
    fn write_file(path: &str, content: &str) -> anyhow::Result<NoneType> {
        if !analysis_active() {
            return Ok(NoneType);
        }
        let script = format!(
            "set -eu\n\
             __once_path={path_arg}\n\
             case \"$__once_path\" in */*) mkdir -p \"${{__once_path%/*}}\" ;; esac\n\
             printf '%s' {content_arg} > \"$__once_path\"\n",
            path_arg = shell_quote(path),
            content_arg = shell_quote(content),
        );
        let action = DeclaredAction {
            argv: vec!["/bin/sh".to_string(), "-c".to_string(), script],
            inputs: Vec::new(),
            outputs: vec![path.to_string()],
            env: BTreeMap::new(),
            cacheable: true,
            // Folding the literal content into the toolchain identity
            // keeps the digest pinned to what the file should contain,
            // so changing the content alone invalidates the action.
            toolchain_identity: Some(format!("once.write_file.v1\0{content}")),
            identifier: Some(format!("write_file:{path}")),
        };
        with_store_mut(|store| {
            if let Some(store) = store {
                store.actions.push(action);
            }
        });
        Ok(NoneType)
    }

    /// Declare an action that materialises raw bytes at `path`.
    /// `bytes` is a list of integers in `0..=255`. The content is
    /// base64-encoded into the generated shell command so binary
    /// payloads (including NULs) survive shell quoting, and is folded
    /// into the toolchain identity so any change invalidates
    /// downstream consumers. Domain-specific binary formats
    /// (header-maps, mach-o, etc.) are constructed in the prelude
    /// and emitted through this primitive.
    #[allow(clippy::unnecessary_wraps)]
    fn write_bytes<'v>(path: &str, bytes: Value<'v>) -> anyhow::Result<NoneType> {
        if !analysis_active() {
            return Ok(NoneType);
        }
        let bytes = unpack_byte_list(bytes, "bytes")?;
        let encoded = base64_encode(&bytes);
        let script = format!(
            "set -eu\n\
             __once_path={path_arg}\n\
             case \"$__once_path\" in */*) mkdir -p \"${{__once_path%/*}}\" ;; esac\n\
             printf '%s' {encoded_arg} | base64 -d > \"$__once_path\"\n",
            path_arg = shell_quote(path),
            encoded_arg = shell_quote(&encoded),
        );
        let action = DeclaredAction {
            argv: vec!["/bin/sh".to_string(), "-c".to_string(), script],
            inputs: Vec::new(),
            outputs: vec![path.to_string()],
            env: BTreeMap::new(),
            cacheable: true,
            toolchain_identity: Some(format!("once.write_bytes.v1\0{encoded}")),
            identifier: Some(format!("write_bytes:{path}")),
        };
        with_store_mut(|store| {
            if let Some(store) = store {
                store.actions.push(action);
            }
        });
        Ok(NoneType)
    }

    /// Record one command action declaration. Argument shape:
    /// `argv`: list of strings; `inputs`: list of workspace-relative
    /// source paths to hash into the input digest; `outputs`: list of
    /// workspace-relative paths the action produces; `env`: optional
    /// string->string dict; `cacheable`: optional bool, default true;
    /// `toolchain_identity`: optional string folded into the input
    /// digest; `identifier`: optional label for diagnostics.
    fn run_action<'v>(
        argv: Value<'v>,
        inputs: Option<Value<'v>>,
        outputs: Option<Value<'v>>,
        env: Option<Value<'v>>,
        toolchain_identity: Option<String>,
        identifier: Option<String>,
        cacheable: Option<bool>,
    ) -> anyhow::Result<NoneType> {
        let argv = unpack_string_list(argv, "argv")?;
        let inputs = inputs
            .map(|value| unpack_string_list(value, "inputs"))
            .transpose()?
            .unwrap_or_default();
        let outputs = outputs
            .map(|value| unpack_string_list(value, "outputs"))
            .transpose()?
            .unwrap_or_default();
        let env = env
            .map(|value| unpack_string_dict(value, "env"))
            .transpose()?
            .unwrap_or_default();
        let action = DeclaredAction {
            argv,
            inputs,
            outputs,
            env,
            cacheable: cacheable.unwrap_or(true),
            toolchain_identity,
            identifier,
        };
        with_store_mut(|store| {
            if let Some(store) = store {
                store.actions.push(action);
            }
        });
        Ok(NoneType)
    }

    /// Decode TOML into Starlark dictionaries/lists/scalars. This is a
    /// generic data-format primitive used by dependency resolvers; the
    /// ecosystem-specific interpretation stays in Starlark.
    fn toml_decode<'v>(src: &str, eval: &mut Evaluator<'v, '_, '_>) -> anyhow::Result<Value<'v>> {
        let value: toml::Value = toml::from_str(src)?;
        Ok(toml_value_to_starlark(eval, value))
    }

    /// Decode JSON into Starlark dictionaries/lists/scalars. Dependency
    /// resolvers use this for machine output from ecosystem-native
    /// resolution commands.
    fn json_decode<'v>(src: &str, eval: &mut Evaluator<'v, '_, '_>) -> anyhow::Result<Value<'v>> {
        let value: JsonValue = serde_json::from_str(src)?;
        Ok(json_to_value(eval, &value))
    }
}

pub(super) fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    let escaped = value.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

/// Standard base64 alphabet encoder. The output is consumed by
/// `base64 -d` in a generated shell script, so we need round-tripping
/// fidelity, not a fancy MIME-line-wrapped form.
pub(super) fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut chunks = bytes.chunks_exact(3);
    for chunk in &mut chunks {
        let bits = (u32::from(chunk[0]) << 16) | (u32::from(chunk[1]) << 8) | u32::from(chunk[2]);
        out.push(ALPHABET[((bits >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((bits >> 12) & 0x3F) as usize] as char);
        out.push(ALPHABET[((bits >> 6) & 0x3F) as usize] as char);
        out.push(ALPHABET[(bits & 0x3F) as usize] as char);
    }
    let remainder = chunks.remainder();
    match remainder.len() {
        0 => {}
        1 => {
            let bits = u32::from(remainder[0]) << 16;
            out.push(ALPHABET[((bits >> 18) & 0x3F) as usize] as char);
            out.push(ALPHABET[((bits >> 12) & 0x3F) as usize] as char);
            out.push('=');
            out.push('=');
        }
        2 => {
            let bits = (u32::from(remainder[0]) << 16) | (u32::from(remainder[1]) << 8);
            out.push(ALPHABET[((bits >> 18) & 0x3F) as usize] as char);
            out.push(ALPHABET[((bits >> 12) & 0x3F) as usize] as char);
            out.push(ALPHABET[((bits >> 6) & 0x3F) as usize] as char);
            out.push('=');
        }
        _ => unreachable!(),
    }
    out
}

fn host_arch_str() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else {
        std::env::consts::ARCH
    }
}

fn host_os_str() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        std::env::consts::OS
    }
}

/// Expand `patterns` against `package` and return workspace-relative
/// file paths.
///
/// Each match is canonicalized and required to land inside the
/// canonical workspace root, which rejects symlinks that point
/// outside the tree. The check is best-effort against the on-disk
/// state at evaluation time: a write-capable attacker on the
/// workspace could in principle swap a symlink between
/// `glob::glob` and `canonicalize`. Once treats the workspace as
/// trusted (a developer's own checkout), so this TOCTOU window is
/// out of scope for the threat model; the check exists to surface
/// honest mistakes (a stray `..` symlink), not adversarial races.
/// Windows junctions are not exercised by tests yet; the
/// `canonicalize` call covers them in production but a dedicated
/// Windows test should land alongside Windows CI.
pub(super) fn expand_globs(
    workspace_root: &Path,
    package: &str,
    patterns: &[String],
) -> Result<Vec<String>> {
    let package_dir = if package.is_empty() {
        workspace_root.to_path_buf()
    } else {
        workspace_root.join(package)
    };
    let canonical_workspace = std::fs::canonicalize(workspace_root)
        .with_context(|| format!("canonicalizing workspace `{}`", workspace_root.display()))?;
    let mut out: Vec<String> = Vec::new();
    for pattern in patterns {
        let abs_pattern = package_dir.join(pattern);
        let pattern_str = abs_pattern
            .to_str()
            .ok_or_else(|| anyhow!("non-utf8 glob pattern `{pattern}`"))?;
        for entry in
            glob::glob(pattern_str).with_context(|| format!("invalid glob pattern `{pattern}`"))?
        {
            let path = entry.with_context(|| format!("glob walk failed for `{pattern}`"))?;
            if !path.is_file() {
                continue;
            }
            let canonical = std::fs::canonicalize(&path)
                .with_context(|| format!("canonicalizing `{}`", path.display()))?;
            let stripped = canonical
                .strip_prefix(&canonical_workspace)
                .with_context(|| {
                    format!(
                        "glob result `{}` is outside the workspace `{}`",
                        canonical.display(),
                        canonical_workspace.display()
                    )
                })?;
            let ws_rel = stripped
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            if !ws_rel.is_empty() {
                out.push(ws_rel);
            }
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

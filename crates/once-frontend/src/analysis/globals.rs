use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde_json::Value as JsonValue;
use sha2::Digest as ShaDigest;
use starlark::environment::{Globals, GlobalsBuilder};
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::none::NoneType;
use starlark::values::Value;

use super::store::{
    analysis_active, with_store, with_store_mut, DeclaredAction, DeclaredActionOperation,
};
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

    /// Read one host environment variable. Missing variables return
    /// `""`. Schema parsing returns `""`.
    #[allow(clippy::unnecessary_wraps)]
    fn host_env(name: &str) -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        Ok(std::env::var(name).unwrap_or_default())
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

    /// Return the SHA-256 digest of one host file as lowercase hex.
    /// This is for host-specific tool or signing inputs that cannot be
    /// declared as workspace action inputs.
    fn host_file_sha256(path: &str) -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        file_sha256_hex(Path::new(path)).with_context(|| format!("hashing host file `{path}`"))
    }

    /// Return whether one host path currently exists as a file.
    #[allow(clippy::unnecessary_wraps)]
    fn host_file_exists(path: &str) -> anyhow::Result<bool> {
        if !analysis_active() {
            return Ok(false);
        }
        Ok(Path::new(path).is_file())
    }

    /// Return whether one host file contains `needle` as text.
    fn host_file_contains(path: &str, needle: &str) -> anyhow::Result<bool> {
        if !analysis_active() {
            return Ok(false);
        }
        let content =
            std::fs::read_to_string(path).with_context(|| format!("reading host file `{path}`"))?;
        Ok(content.contains(needle))
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

    /// Declare a portable action that materialises `content` at the
    /// workspace-relative `path`. The content is hashed into the
    /// action digest so any edit invalidates downstream consumers.
    #[allow(clippy::unnecessary_wraps)]
    fn write_file(path: &str, content: &str) -> anyhow::Result<NoneType> {
        if !analysis_active() {
            return Ok(NoneType);
        }
        let action = DeclaredAction {
            operation: Some(DeclaredActionOperation::WriteFile {
                path: path.to_string(),
                content: content.to_string(),
            }),
            argv: Vec::new(),
            inputs: Vec::new(),
            outputs: vec![path.to_string()],
            env: BTreeMap::new(),
            cacheable: true,
            toolchain_identity: None,
            identifier: Some(format!("write_file:{path}")),
        };
        with_store_mut(|store| {
            if let Some(store) = store {
                store.actions.push(action);
            }
        });
        Ok(NoneType)
    }

    /// Declare a portable action that materialises raw bytes at
    /// `path`. `bytes` is a list of integers in `0..=255`.
    /// Domain-specific binary formats are constructed in the prelude
    /// and emitted through this primitive.
    #[allow(clippy::unnecessary_wraps)]
    fn write_bytes<'v>(path: &str, bytes: Value<'v>) -> anyhow::Result<NoneType> {
        if !analysis_active() {
            return Ok(NoneType);
        }
        let bytes = unpack_byte_list(bytes, "bytes")?;
        let action = DeclaredAction {
            operation: Some(DeclaredActionOperation::WriteBytes {
                path: path.to_string(),
                bytes,
            }),
            argv: Vec::new(),
            inputs: Vec::new(),
            outputs: vec![path.to_string()],
            env: BTreeMap::new(),
            cacheable: true,
            toolchain_identity: None,
            identifier: Some(format!("write_bytes:{path}")),
        };
        with_store_mut(|store| {
            if let Some(store) = store {
                store.actions.push(action);
            }
        });
        Ok(NoneType)
    }

    /// Declare a portable action that copies one file. `inputs`
    /// should include `source` when it is a workspace source file.
    /// Generated outputs from previous actions are already covered by
    /// same-target action dependencies.
    fn copy_file<'v>(
        source: &str,
        destination: &str,
        inputs: Option<Value<'v>>,
        toolchain_identity: Option<String>,
        identifier: Option<String>,
        cacheable: Option<bool>,
    ) -> anyhow::Result<NoneType> {
        if !analysis_active() {
            return Ok(NoneType);
        }
        let inputs = inputs
            .map(|value| unpack_string_list(value, "inputs"))
            .transpose()?
            .unwrap_or_default();
        let action = DeclaredAction {
            operation: Some(DeclaredActionOperation::CopyFile {
                source: source.to_string(),
                destination: destination.to_string(),
            }),
            argv: Vec::new(),
            inputs,
            outputs: vec![destination.to_string()],
            env: BTreeMap::new(),
            cacheable: cacheable.unwrap_or(true),
            toolchain_identity,
            identifier: Some(identifier.unwrap_or_else(|| format!("copy_file:{destination}"))),
        };
        with_store_mut(|store| {
            if let Some(store) = store {
                store.actions.push(action);
            }
        });
        Ok(NoneType)
    }

    /// Declare a portable action that copies the contents of a source
    /// directory into a destination directory. `inputs` should list
    /// source files when the source tree is not produced by an earlier
    /// action or dependency.
    fn copy_tree<'v>(
        source: &str,
        destination: &str,
        inputs: Option<Value<'v>>,
        toolchain_identity: Option<String>,
        identifier: Option<String>,
        cacheable: Option<bool>,
    ) -> anyhow::Result<NoneType> {
        if !analysis_active() {
            return Ok(NoneType);
        }
        let inputs = inputs
            .map(|value| unpack_string_list(value, "inputs"))
            .transpose()?
            .unwrap_or_default();
        let action = DeclaredAction {
            operation: Some(DeclaredActionOperation::CopyTree {
                sources: vec![source.to_string()],
                destination: destination.to_string(),
            }),
            argv: Vec::new(),
            inputs,
            outputs: vec![destination.to_string()],
            env: BTreeMap::new(),
            cacheable: cacheable.unwrap_or(true),
            toolchain_identity,
            identifier: Some(identifier.unwrap_or_else(|| format!("copy_tree:{destination}"))),
        };
        with_store_mut(|store| {
            if let Some(store) = store {
                store.actions.push(action);
            }
        });
        Ok(NoneType)
    }

    /// Declare a portable action that copies the contents of several
    /// source directories into one destination directory.
    fn copy_trees<'v>(
        sources: Value<'v>,
        destination: &str,
        inputs: Option<Value<'v>>,
        toolchain_identity: Option<String>,
        identifier: Option<String>,
        cacheable: Option<bool>,
    ) -> anyhow::Result<NoneType> {
        if !analysis_active() {
            return Ok(NoneType);
        }
        let sources = unpack_string_list(sources, "sources")?;
        let inputs = inputs
            .map(|value| unpack_string_list(value, "inputs"))
            .transpose()?
            .unwrap_or_default();
        let action = DeclaredAction {
            operation: Some(DeclaredActionOperation::CopyTree {
                sources,
                destination: destination.to_string(),
            }),
            argv: Vec::new(),
            inputs,
            outputs: vec![destination.to_string()],
            env: BTreeMap::new(),
            cacheable: cacheable.unwrap_or(true),
            toolchain_identity,
            identifier: Some(identifier.unwrap_or_else(|| format!("copy_tree:{destination}"))),
        };
        with_store_mut(|store| {
            if let Some(store) = store {
                store.actions.push(action);
            }
        });
        Ok(NoneType)
    }

    /// Declare an uncached portable action that removes a workspace
    /// path if it exists. Use this before native tools that merge into
    /// existing output directories.
    #[allow(clippy::unnecessary_wraps)]
    fn remove_path(path: &str, identifier: Option<String>) -> anyhow::Result<NoneType> {
        if !analysis_active() {
            return Ok(NoneType);
        }
        let action = DeclaredAction {
            operation: Some(DeclaredActionOperation::RemovePath {
                path: path.to_string(),
            }),
            argv: Vec::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            env: BTreeMap::new(),
            cacheable: false,
            toolchain_identity: None,
            identifier: Some(identifier.unwrap_or_else(|| format!("remove_path:{path}"))),
        };
        with_store_mut(|store| {
            if let Some(store) = store {
                store.actions.push(action);
            }
        });
        Ok(NoneType)
    }

    /// Declare an uncached portable action that creates a workspace
    /// directory and any missing parents.
    #[allow(clippy::unnecessary_wraps)]
    fn ensure_dir(path: &str, identifier: Option<String>) -> anyhow::Result<NoneType> {
        if !analysis_active() {
            return Ok(NoneType);
        }
        let action = DeclaredAction {
            operation: Some(DeclaredActionOperation::EnsureDir {
                path: path.to_string(),
            }),
            argv: Vec::new(),
            inputs: Vec::new(),
            outputs: vec![path.to_string()],
            env: BTreeMap::new(),
            cacheable: false,
            toolchain_identity: None,
            identifier: Some(identifier.unwrap_or_else(|| format!("ensure_dir:{path}"))),
        };
        with_store_mut(|store| {
            if let Some(store) = store {
                store.actions.push(action);
            }
        });
        Ok(NoneType)
    }

    /// Declare a portable action that writes a deterministic digest
    /// listing for a workspace tree. Missing roots produce an empty
    /// file. `include_suffixes` filters files by path suffix when set.
    fn write_tree_digest<'v>(
        root: &str,
        output: &str,
        include_suffixes: Option<Value<'v>>,
        inputs: Option<Value<'v>>,
        identifier: Option<String>,
        cacheable: Option<bool>,
    ) -> anyhow::Result<NoneType> {
        if !analysis_active() {
            return Ok(NoneType);
        }
        let include_suffixes = include_suffixes
            .map(|value| unpack_string_list(value, "include_suffixes"))
            .transpose()?
            .unwrap_or_default();
        let inputs = inputs
            .map(|value| unpack_string_list(value, "inputs"))
            .transpose()?
            .unwrap_or_default();
        let action = DeclaredAction {
            operation: Some(DeclaredActionOperation::WriteTreeDigest {
                root: root.to_string(),
                output: output.to_string(),
                include_suffixes,
            }),
            argv: Vec::new(),
            inputs,
            outputs: vec![output.to_string()],
            env: BTreeMap::new(),
            cacheable: cacheable.unwrap_or(true),
            toolchain_identity: None,
            identifier: Some(identifier.unwrap_or_else(|| format!("write_tree_digest:{output}"))),
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
            operation: None,
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

fn file_sha256_hex(path: &Path) -> Result<String> {
    use std::io::Read;

    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file);
    let mut hasher = sha2::Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hex_lower(&hasher.finalize()))
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
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

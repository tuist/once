use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde_json::Value as JsonValue;
use sha2::Digest as ShaDigest;
use starlark::environment::{Globals, GlobalsBuilder};
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::dict::{AllocDict, DictRef};
use starlark::values::list::ListRef;
use starlark::values::none::NoneType;
use starlark::values::Value;
use walkdir::WalkDir;

use super::store::{
    analysis_active, with_store, with_store_mut, DeclaredAction, DeclaredActionOperation,
    DeclaredArchiveEntry, DeclaredArchiveEntryKind, DeclaredArchiveFormat, DeclaredArgFile,
    DeclaredArgFileFormat, DeclaredCopyPathMode, DeclaredPreparePathMode,
};
use super::values::{
    json_to_value, toml_value_to_starlark, unpack_byte_list, unpack_string_dict, unpack_string_list,
};

const CMD_ARGS_MARKER: &str = "_once_cmd_args";
const EXECUTION_ROOT_MARKER: &str = "{{once.execution_root}}";
const MAX_HOST_FILE_READ_BYTES: u64 = 16 * 1024 * 1024;

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
        with_store(|store| -> Result<String> {
            let store = store.ok_or_else(|| anyhow!("host_env called outside analysis"))?;
            Ok(store.host_cache.env(name).unwrap_or_default())
        })
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

    /// Convert a workspace-relative path into a stable command value
    /// that resolves against the actual local, sandbox, or remote
    /// execution root immediately before process launch.
    fn execution_path(path: &str) -> anyhow::Result<String> {
        let mut parts = Vec::new();
        for component in Path::new(path).components() {
            match component {
                Component::Normal(value) => parts.push(
                    value
                        .to_str()
                        .ok_or_else(|| anyhow!("execution_path contains non-UTF-8 text"))?,
                ),
                Component::CurDir => {}
                Component::ParentDir | Component::Prefix(_) | Component::RootDir => {
                    return Err(anyhow!(
                        "execution_path must stay inside the workspace, got `{path}`"
                    ));
                }
            }
        }
        let normalized = parts.join("/");
        Ok(if normalized.is_empty() {
            EXECUTION_ROOT_MARKER.to_string()
        } else {
            format!("{EXECUTION_ROOT_MARKER}/{normalized}")
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

    /// Like `host_which` but returns `""` when `name` is not on PATH
    /// instead of failing. Lets rules probe for an optional tool without
    /// a host shell. Schema parsing returns `""`.
    fn host_which_optional(name: &str) -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        let resolved = with_store(|store| -> Result<Option<String>> {
            let store =
                store.ok_or_else(|| anyhow!("host_which_optional called outside analysis"))?;
            store.host_cache.which(name)
        })?;
        Ok(resolved.unwrap_or_default())
    }

    /// Run `argv[0]` with `argv[1..]` as arguments and return its
    /// stdout as a string. Fails if the process exits non-zero;
    /// includes stderr in the error message. Optional `env` is a
    /// `dict<string, string>` of environment variables overlaid on the
    /// host process env. Both `argv` and `env` participate in the
    /// cache key, so a different `DEVELOPER_DIR` resolves to a
    /// different cached result. When set, `cwd` must be absolute.
    /// Schema parsing returns `""`.
    fn host_command<'v>(
        argv: Value<'v>,
        env: Option<Value<'v>>,
        cwd: Option<&str>,
        merge_stderr: Option<bool>,
    ) -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        let argv = unpack_string_list(argv, "argv")?;
        let env = env
            .map(|value| unpack_string_dict(value, "env"))
            .transpose()?
            .unwrap_or_default();
        let merge_stderr = merge_stderr.unwrap_or(false);
        with_store(|store| -> Result<String> {
            let store = store.ok_or_else(|| anyhow!("host_command called outside analysis"))?;
            if let Some(cwd) = cwd {
                if !Path::new(cwd).is_absolute() {
                    return Err(anyhow!("host_command cwd must be absolute, got `{cwd}`"));
                }
            }
            store
                .host_cache
                .command(&argv, &env, cwd.map(Path::new), merge_stderr)
        })
    }

    /// Return the SHA-256 digest of one host file as lowercase hex.
    /// This is for host-specific tool or signing inputs that cannot be
    /// declared as workspace action inputs.
    fn host_file_sha256(path: &str) -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        observe_host_path(Path::new(path))?;
        file_sha256_hex(Path::new(path)).with_context(|| format!("hashing host file `{path}`"))
    }

    /// Return whether one host path currently exists as a file.
    fn host_file_exists(path: &str) -> anyhow::Result<bool> {
        if !analysis_active() {
            return Ok(false);
        }
        observe_host_path(Path::new(path))?;
        Ok(Path::new(path).is_file())
    }

    /// Read one host file as UTF-8 text.
    fn host_file_read(path: &str) -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        observe_host_path(Path::new(path))?;
        let bytes = read_bounded_host_file(path)?;
        String::from_utf8(bytes).with_context(|| format!("host file `{path}` is not UTF-8 text"))
    }

    /// Return whether one host file contains `needle` as text.
    fn host_file_contains(path: &str, needle: &str) -> anyhow::Result<bool> {
        if !analysis_active() {
            return Ok(false);
        }
        observe_host_path(Path::new(path))?;
        if needle.is_empty() {
            return Ok(true);
        }
        let content = read_bounded_host_file(path)?;
        Ok(content
            .windows(needle.len())
            .any(|window| window == needle.as_bytes()))
    }

    /// Return the sorted entry names of host directory `path`. Missing or
    /// non-directory paths (and schema parsing) return an empty list.
    /// Names are bare file names, letting rules enumerate host toolchains
    /// (for example SDK version directories) without a host shell.
    fn host_read_dir<'v>(
        path: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let heap = eval.heap();
        if !analysis_active() {
            return Ok(heap.alloc(Vec::<String>::new()));
        }
        observe_host_path(Path::new(path))?;
        let mut names = match std::fs::read_dir(path) {
            Ok(entries) => entries
                .filter_map(std::result::Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            Err(_) => Vec::new(),
        };
        names.sort();
        Ok(heap.alloc(names))
    }

    /// Expand a list of glob patterns against the active target's
    /// package directory. Returns sorted, deduplicated, workspace-
    /// relative file paths. Schema parsing returns an empty list.
    fn glob<'v>(
        patterns: Value<'v>,
        exclude: Option<Value<'v>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let heap = eval.heap();
        if !analysis_active() {
            return Ok(heap.alloc(Vec::<String>::new()));
        }
        let patterns = unpack_string_list(patterns, "patterns")?;
        let excludes = exclude
            .map(|value| unpack_string_list(value, "exclude"))
            .transpose()?
            .unwrap_or_default();
        let resolved = with_store(|store| -> Result<Vec<String>> {
            let store = store.ok_or_else(|| anyhow!("glob called outside analysis"))?;
            expand_globs_with_excludes(&store.workspace_root, &store.package, &patterns, &excludes)
        })?;
        Ok(heap.alloc(resolved))
    }

    /// Walk a package-relative directory and return sorted,
    /// deduplicated, workspace-relative file and symbolic-link paths.
    /// `excluded_paths` names root-relative paths whose trees should
    /// not be traversed. `excluded_names` prunes entries with an exact
    /// file name at any depth. Schema parsing returns an empty list.
    fn walk_files<'v>(
        root: &str,
        excluded_paths: Option<Value<'v>>,
        excluded_names: Option<Value<'v>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let heap = eval.heap();
        if !analysis_active() {
            return Ok(heap.alloc(Vec::<String>::new()));
        }
        let excluded_paths = excluded_paths
            .map(|value| unpack_string_list(value, "excluded_paths"))
            .transpose()?
            .unwrap_or_default();
        let excluded_names = excluded_names
            .map(|value| unpack_string_list(value, "excluded_names"))
            .transpose()?
            .unwrap_or_default();
        let resolved = with_store(|store| -> Result<Vec<String>> {
            let store = store.ok_or_else(|| anyhow!("walk_files called outside analysis"))?;
            walk_package_files(
                &store.workspace_root,
                &store.package,
                root,
                &excluded_paths,
                &excluded_names,
            )
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

    /// Declare a portable action that writes text or bytes at the
    /// workspace-relative `path`. `content` may be a string or a list
    /// of integers in `0..=255`.
    fn write_path<'v>(path: &str, content: Value<'v>) -> anyhow::Result<NoneType> {
        if !analysis_active() {
            return Ok(NoneType);
        }
        let bytes = unpack_write_content(content)?;
        let action = DeclaredAction {
            operation: Some(DeclaredActionOperation::WriteFile {
                path: path.to_string(),
                bytes,
            }),
            argv: Vec::new(),
            arg_files: Vec::new(),
            inputs: Vec::new(),
            outputs: vec![path.to_string()],
            stdout: None,
            stderr: None,
            clean_paths: Vec::new(),
            create_dirs: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            success_exit_codes: vec![0],
            cacheable: true,
            inherit_parent_env: false,
            depends_on_prior_actions: true,
            toolchain_identity: None,
            identifier: Some(format!("write_path:{path}")),
        };
        with_store_mut(|store| {
            if let Some(store) = store {
                store.actions.push(action);
            }
        });
        Ok(NoneType)
    }

    /// Declare a portable copy action. The default copies one path by value,
    /// materializing a directory symlink at the destination. `kind = "tree"`
    /// copies directory contents and preserves their symlink layout. Tree
    /// copies accept one source string or a list of source directories.
    fn copy_path<'v>(
        source: Value<'v>,
        destination: &str,
        kind: Option<String>,
        inputs: Option<Value<'v>>,
        toolchain_identity: Option<String>,
        identifier: Option<String>,
        cacheable: Option<bool>,
    ) -> anyhow::Result<NoneType> {
        if !analysis_active() {
            return Ok(NoneType);
        }
        let mode = parse_copy_path_mode(kind.as_deref())?;
        let sources = unpack_copy_sources(source, mode)?;
        let inputs = inputs
            .map(|value| unpack_string_list(value, "inputs"))
            .transpose()?
            .unwrap_or_default();
        let action = DeclaredAction {
            operation: Some(DeclaredActionOperation::CopyPath {
                sources,
                destination: destination.to_string(),
                mode,
            }),
            argv: Vec::new(),
            arg_files: Vec::new(),
            inputs,
            outputs: vec![destination.to_string()],
            stdout: None,
            stderr: None,
            clean_paths: Vec::new(),
            create_dirs: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            success_exit_codes: vec![0],
            cacheable: cacheable.unwrap_or(true),
            inherit_parent_env: false,
            depends_on_prior_actions: true,
            toolchain_identity,
            identifier: Some(identifier.unwrap_or_else(|| format!("copy_path:{destination}"))),
        };
        with_store_mut(|store| {
            if let Some(store) = store {
                store.actions.push(action);
            }
        });
        Ok(NoneType)
    }

    /// Snapshot an absolute host toolchain file into a workspace-relative
    /// output. The source digest is captured during analysis and verified
    /// again on execution, so cache identity follows file content.
    fn materialize_host_file(source: &str, destination: &str) -> anyhow::Result<NoneType> {
        if !analysis_active() {
            return Ok(NoneType);
        }
        let source_path = Path::new(source);
        if !source_path.is_absolute() {
            return Err(anyhow!(
                "materialize_host_file source must be absolute, got `{source}`"
            ));
        }
        if !source_path.is_file() {
            return Err(anyhow!(
                "materialize_host_file source is not a file: `{source}`"
            ));
        }
        observe_host_path(source_path)?;
        let source_sha256 = file_sha256_hex(source_path)
            .with_context(|| format!("hashing host file `{source}`"))?;
        let action = DeclaredAction {
            operation: Some(DeclaredActionOperation::MaterializeHostFile {
                source: source.to_string(),
                source_sha256,
                destination: destination.to_string(),
            }),
            argv: Vec::new(),
            arg_files: Vec::new(),
            inputs: Vec::new(),
            outputs: vec![destination.to_string()],
            stdout: None,
            stderr: None,
            clean_paths: Vec::new(),
            create_dirs: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            success_exit_codes: vec![0],
            cacheable: true,
            inherit_parent_env: false,
            depends_on_prior_actions: true,
            toolchain_identity: None,
            identifier: Some(format!("materialize_host_file:{destination}")),
        };
        with_store_mut(|store| {
            if let Some(store) = store {
                store.actions.push(action);
            }
        });
        Ok(NoneType)
    }

    /// Snapshot an absolute host directory into a workspace-relative output.
    /// The source tree digest is captured during analysis and verified again
    /// on execution, so cache identity follows files, modes, and symlinks.
    fn materialize_host_tree(source: &str, destination: &str) -> anyhow::Result<NoneType> {
        if !analysis_active() {
            return Ok(NoneType);
        }
        let source_path = Path::new(source);
        if !source_path.is_absolute() {
            return Err(anyhow!(
                "materialize_host_tree source must be absolute, got `{source}`"
            ));
        }
        if !source_path.is_dir() {
            return Err(anyhow!(
                "materialize_host_tree source is not a directory: `{source}`"
            ));
        }
        let source_sha256 = once_host_tree::host_tree_sha256_hex(source_path)
            .with_context(|| format!("hashing host directory `{source}`"))?;
        let action = DeclaredAction {
            operation: Some(DeclaredActionOperation::MaterializeHostTree {
                source: source.to_string(),
                source_sha256,
                destination: destination.to_string(),
            }),
            argv: Vec::new(),
            arg_files: Vec::new(),
            inputs: Vec::new(),
            outputs: vec![destination.to_string()],
            stdout: None,
            stderr: None,
            clean_paths: Vec::new(),
            create_dirs: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            success_exit_codes: vec![0],
            cacheable: true,
            inherit_parent_env: false,
            depends_on_prior_actions: true,
            toolchain_identity: None,
            identifier: Some(format!("materialize_host_tree:{destination}")),
        };
        with_store_mut(|store| {
            if let Some(store) = store {
                store.actions.push(action);
            }
        });
        Ok(NoneType)
    }

    /// Link one workspace path to another without copying or caching
    /// the linked contents. Downstream actions still hash the linked
    /// tree when they declare it as an input.
    fn link_path(
        source: &str,
        destination: &str,
        identifier: Option<String>,
    ) -> anyhow::Result<NoneType> {
        if !analysis_active() {
            return Ok(NoneType);
        }
        if source == destination {
            return Err(anyhow!("link_path source and destination must differ"));
        }
        let action = DeclaredAction {
            operation: Some(DeclaredActionOperation::LinkPath {
                source: source.to_string(),
                destination: destination.to_string(),
            }),
            argv: Vec::new(),
            arg_files: Vec::new(),
            inputs: Vec::new(),
            outputs: vec![destination.to_string()],
            stdout: None,
            stderr: None,
            clean_paths: Vec::new(),
            create_dirs: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            success_exit_codes: vec![0],
            cacheable: false,
            inherit_parent_env: false,
            depends_on_prior_actions: true,
            toolchain_identity: None,
            identifier: Some(
                identifier.unwrap_or_else(|| format!("link_path:{source}:{destination}")),
            ),
        };
        with_store_mut(|store| {
            if let Some(store) = store {
                store.actions.push(action);
            }
        });
        Ok(NoneType)
    }

    /// Declare an uncached portable path preparation action. `kind`
    /// must be `"remove"` or `"directory"`.
    fn prepare_path(
        path: &str,
        kind: &str,
        identifier: Option<String>,
    ) -> anyhow::Result<NoneType> {
        if !analysis_active() {
            return Ok(NoneType);
        }
        let mode = parse_prepare_path_mode(kind)?;
        let outputs = match mode {
            DeclaredPreparePathMode::Remove => Vec::new(),
            DeclaredPreparePathMode::Directory => vec![path.to_string()],
        };
        let action = DeclaredAction {
            operation: Some(DeclaredActionOperation::PreparePath {
                path: path.to_string(),
                mode,
            }),
            argv: Vec::new(),
            arg_files: Vec::new(),
            inputs: Vec::new(),
            outputs,
            stdout: None,
            stderr: None,
            clean_paths: Vec::new(),
            create_dirs: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            success_exit_codes: vec![0],
            cacheable: false,
            inherit_parent_env: false,
            depends_on_prior_actions: true,
            toolchain_identity: None,
            identifier: Some(identifier.unwrap_or_else(|| format!("prepare_path:{kind}:{path}"))),
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
            arg_files: Vec::new(),
            inputs,
            outputs: vec![output.to_string()],
            stdout: None,
            stderr: None,
            clean_paths: Vec::new(),
            create_dirs: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            success_exit_codes: vec![0],
            cacheable: cacheable.unwrap_or(true),
            inherit_parent_env: false,
            depends_on_prior_actions: true,
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

    /// Declare a portable deterministic archive action. Each entry is
    /// a dict with `kind`, `path`, optional `source`, and explicit
    /// metadata. The initial `tar` format supports files, directories,
    /// and recursively expanded trees.
    fn write_archive<'v>(
        entries: Value<'v>,
        output: &str,
        sha256_output: Option<String>,
        format: Option<String>,
        inputs: Option<Value<'v>>,
        identifier: Option<String>,
        cacheable: Option<bool>,
    ) -> anyhow::Result<NoneType> {
        if !analysis_active() {
            return Ok(NoneType);
        }
        let entries = unpack_archive_entries(entries)?;
        let format = parse_archive_format(format.as_deref())?;
        let mut inputs = inputs
            .map(|value| unpack_string_list(value, "inputs"))
            .transpose()?
            .unwrap_or_default();
        inputs.extend(entries.iter().filter_map(|entry| entry.source.clone()));
        inputs.sort();
        inputs.dedup();
        let mut outputs = vec![output.to_string()];
        if let Some(path) = &sha256_output {
            outputs.push(path.clone());
        }
        let action = DeclaredAction {
            operation: Some(DeclaredActionOperation::WriteArchive {
                entries,
                output: output.to_string(),
                sha256_output,
                format,
            }),
            argv: Vec::new(),
            arg_files: Vec::new(),
            inputs,
            outputs,
            stdout: None,
            stderr: None,
            clean_paths: Vec::new(),
            create_dirs: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            success_exit_codes: vec![0],
            cacheable: cacheable.unwrap_or(true),
            inherit_parent_env: false,
            depends_on_prior_actions: true,
            toolchain_identity: None,
            identifier: Some(identifier.unwrap_or_else(|| format!("write_archive:{output}"))),
        };
        with_store_mut(|store| {
            if let Some(store) = store {
                store.actions.push(action);
            }
        });
        Ok(NoneType)
    }

    /// Build a structured command-line fragment. `args` is a list of
    /// string arguments. When `use_arg_file` is set, it must be a dict
    /// with `path` plus optional `format` and `arg_format`. The supported
    /// format is `"line-delimited"`.
    fn cmd_args<'v>(
        args: Value<'v>,
        use_arg_file: Option<Value<'v>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let args = unpack_string_list(args, "cmd_args.args")?;
        let arg_file = match use_arg_file {
            Some(value) => unpack_cmd_args_arg_file(value)?,
            None => None,
        };
        if let Some(arg_file) = &arg_file {
            validate_declared_arg_file_args(arg_file.format, &args, &arg_file.path)?;
            apply_arg_format(&arg_file.arg_format, &arg_file.path)?;
        }
        let heap = eval.heap();
        let mut pairs = vec![
            (CMD_ARGS_MARKER.to_string(), Value::new_bool(true)),
            ("args".to_string(), heap.alloc(args)),
        ];
        if let Some(arg_file) = arg_file {
            pairs.extend([
                ("arg_file_path".to_string(), heap.alloc(arg_file.path)),
                (
                    "arg_file_format".to_string(),
                    heap.alloc(arg_file.format.as_str().to_string()),
                ),
                ("arg_format".to_string(), heap.alloc(arg_file.arg_format)),
            ]);
        }
        Ok(heap.alloc(AllocDict(pairs)))
    }

    /// Record one command action declaration. Argument shape:
    /// `argv`: list of strings and `cmd_args` values; `inputs`: list
    /// of workspace-relative source paths to hash into the input
    /// digest; `outputs`: list of workspace-relative paths the action
    /// produces; `clean_paths`: optional list of workspace paths to
    /// remove before a fresh command execution; `create_dirs`: optional
    /// list of workspace directories to create before a fresh command
    /// execution; `cwd`: optional workspace-relative directory to run
    /// the command in, defaulting to the workspace root; `env`: optional
    /// string->string dict; `cacheable`: optional bool, default true;
    /// `inherit_parent_env`: optional bool for uncached local run actions,
    /// default false;
    /// `sandbox`: optional local filesystem sandbox policy, `"off"`,
    /// `"inputs"`, or `"copied-inputs"`; the copied mode materializes
    /// private input copies for tools that cannot consume links;
    /// `success_exit_codes`: optional integer list, default `[0]`, whose
    /// members mean the command completed and its outputs are valid;
    /// `toolchain_identity`: optional string folded into the input
    /// digest; `identifier`: optional label for diagnostics.
    #[allow(
        clippy::too_many_arguments,
        reason = "run_action mirrors the declared-action fields, including optional stream redirection"
    )]
    fn run_action<'v>(
        argv: Value<'v>,
        inputs: Option<Value<'v>>,
        outputs: Option<Value<'v>>,
        clean_paths: Option<Value<'v>>,
        create_dirs: Option<Value<'v>>,
        cwd: Option<Value<'v>>,
        env: Option<Value<'v>>,
        toolchain_identity: Option<String>,
        identifier: Option<String>,
        cacheable: Option<bool>,
        inherit_parent_env: Option<bool>,
        depends_on_prior_actions: Option<bool>,
        stdout: Option<String>,
        stderr: Option<String>,
        sandbox: Option<String>,
        success_exit_codes: Option<Value<'v>>,
    ) -> anyhow::Result<NoneType> {
        validate_sandbox(sandbox.as_deref())?;
        let argv = unpack_action_argv(argv, "argv")?;
        let inputs = inputs
            .map(|value| unpack_string_list(value, "inputs"))
            .transpose()?
            .unwrap_or_default();
        let outputs = outputs
            .map(|value| unpack_string_list(value, "outputs"))
            .transpose()?
            .unwrap_or_default();
        let clean_paths = clean_paths
            .map(|value| unpack_string_list(value, "clean_paths"))
            .transpose()?
            .unwrap_or_default();
        let create_dirs = create_dirs
            .map(|value| unpack_string_list(value, "create_dirs"))
            .transpose()?
            .unwrap_or_default();
        let cwd = match cwd {
            None => None,
            Some(value) if value.is_none() => None,
            Some(value) => Some(
                value
                    .unpack_str()
                    .ok_or_else(|| anyhow::anyhow!("cwd must be a string or None"))?
                    .to_string(),
            ),
        };
        let env = env
            .map(|value| unpack_string_dict(value, "env"))
            .transpose()?
            .unwrap_or_default();
        let cacheable = cacheable.unwrap_or(true);
        let inherit_parent_env = inherit_parent_env.unwrap_or(false);
        if cacheable && inherit_parent_env {
            return Err(anyhow!(
                "run_action inherit_parent_env requires cacheable = False"
            ));
        }
        if inherit_parent_env && sandbox.as_deref().is_some_and(|mode| mode != "off") {
            return Err(anyhow!(
                "run_action inherit_parent_env requires sandbox = \"off\""
            ));
        }
        let mut success_exit_codes = success_exit_codes
            .map(|value| unpack_i32_list(value, "success_exit_codes"))
            .transpose()?
            .unwrap_or_else(|| vec![0]);
        if success_exit_codes.is_empty() {
            anyhow::bail!("success_exit_codes must contain at least one exit code");
        }
        success_exit_codes.sort_unstable();
        success_exit_codes.dedup();
        let action = DeclaredAction {
            operation: None,
            argv: argv.args,
            arg_files: argv.arg_files,
            inputs,
            outputs,
            stdout,
            stderr,
            clean_paths,
            create_dirs,
            cwd,
            env,
            sandbox,
            success_exit_codes,
            cacheable,
            inherit_parent_env,
            depends_on_prior_actions: depends_on_prior_actions.unwrap_or(true),
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

fn read_bounded_host_file(path: &str) -> Result<Vec<u8>> {
    let metadata =
        std::fs::metadata(path).with_context(|| format!("reading host file `{path}`"))?;
    if metadata.len() > MAX_HOST_FILE_READ_BYTES {
        anyhow::bail!("host file `{path}` exceeds the 16 mebibyte analysis limit");
    }
    std::fs::read(path).with_context(|| format!("reading host file `{path}`"))
}

struct ActionArgv {
    args: Vec<String>,
    arg_files: Vec<DeclaredArgFile>,
}

struct CmdArgsArgFile {
    path: String,
    format: DeclaredArgFileFormat,
    arg_format: String,
}

impl DeclaredArgFileFormat {
    fn as_str(self) -> &'static str {
        match self {
            Self::LineDelimited => "line-delimited",
        }
    }
}

fn unpack_write_content(content: Value<'_>) -> Result<Vec<u8>> {
    if let Some(string) = content.unpack_str() {
        return Ok(string.as_bytes().to_vec());
    }
    unpack_byte_list(content, "content")
}

fn parse_copy_path_mode(kind: Option<&str>) -> Result<DeclaredCopyPathMode> {
    match kind.unwrap_or("file") {
        "file" => Ok(DeclaredCopyPathMode::File),
        "tree" => Ok(DeclaredCopyPathMode::Tree),
        other => Err(anyhow!(
            "expected `kind` to be `file` or `tree`, got `{other}`"
        )),
    }
}

fn unpack_copy_sources(source: Value<'_>, mode: DeclaredCopyPathMode) -> Result<Vec<String>> {
    let sources = if let Some(source) = source.unpack_str() {
        vec![source.to_string()]
    } else {
        unpack_string_list(source, "source")?
    };
    match mode {
        DeclaredCopyPathMode::File if sources.len() != 1 => Err(anyhow!(
            "`copy_path` with kind `file` requires exactly one source"
        )),
        DeclaredCopyPathMode::Tree if sources.is_empty() => Err(anyhow!(
            "`copy_path` with kind `tree` requires at least one source"
        )),
        _ => Ok(sources),
    }
}

fn parse_prepare_path_mode(kind: &str) -> Result<DeclaredPreparePathMode> {
    match kind {
        "remove" => Ok(DeclaredPreparePathMode::Remove),
        "directory" => Ok(DeclaredPreparePathMode::Directory),
        other => Err(anyhow!(
            "expected `kind` to be `remove` or `directory`, got `{other}`"
        )),
    }
}

fn parse_archive_format(format: Option<&str>) -> Result<DeclaredArchiveFormat> {
    match format.unwrap_or("tar") {
        "tar" => Ok(DeclaredArchiveFormat::Tar),
        other => Err(anyhow!(
            "expected `write_archive.format` to be `tar`, got `{other}`"
        )),
    }
}

fn unpack_archive_entries(value: Value<'_>) -> Result<Vec<DeclaredArchiveEntry>> {
    let entries = ListRef::from_value(value).ok_or_else(|| {
        anyhow!(
            "expected `write_archive.entries` to be a list of dicts, got `{}`",
            value.get_type()
        )
    })?;
    entries
        .iter()
        .enumerate()
        .map(|(index, value)| unpack_archive_entry(value, index))
        .collect()
}

fn unpack_archive_entry(value: Value<'_>, index: usize) -> Result<DeclaredArchiveEntry> {
    let field = format!("write_archive.entries[{index}]");
    let dict = DictRef::from_value(value).ok_or_else(|| {
        anyhow!(
            "expected `{field}` to be a dict, got `{}`",
            value.get_type()
        )
    })?;
    let kind = match required_string_field(&dict, &field, "kind")?.as_str() {
        "file" => DeclaredArchiveEntryKind::File,
        "directory" => DeclaredArchiveEntryKind::Directory,
        "tree" => DeclaredArchiveEntryKind::Tree,
        other => {
            return Err(anyhow!(
                "expected `{field}.kind` to be `file`, `directory`, or `tree`, got `{other}`"
            ));
        }
    };
    let source = optional_string_field(&dict, "source")?;
    match kind {
        DeclaredArchiveEntryKind::File | DeclaredArchiveEntryKind::Tree if source.is_none() => {
            return Err(anyhow!("expected `{field}` to contain `source`"));
        }
        DeclaredArchiveEntryKind::Directory if source.is_some() => {
            return Err(anyhow!(
                "expected `{field}.source` to be omitted for a directory"
            ));
        }
        _ => {}
    }
    let mode = optional_nonnegative_int_field(&dict, &field, "mode")?.unwrap_or(
        if kind == DeclaredArchiveEntryKind::Directory {
            0o755
        } else {
            0o644
        },
    );
    let directory_mode =
        optional_nonnegative_int_field(&dict, &field, "directory_mode")?.unwrap_or(0o755);
    if mode > 0o7777 {
        return Err(anyhow!("expected `{field}.mode` to be at most 4095"));
    }
    if directory_mode > 0o7777 {
        return Err(anyhow!(
            "expected `{field}.directory_mode` to be at most 4095"
        ));
    }
    Ok(DeclaredArchiveEntry {
        kind,
        source,
        path: required_string_field(&dict, &field, "path")?,
        mode,
        directory_mode,
        owner_id: u64::from(
            optional_nonnegative_int_field(&dict, &field, "owner_id")?.unwrap_or(0),
        ),
        group_id: u64::from(
            optional_nonnegative_int_field(&dict, &field, "group_id")?.unwrap_or(0),
        ),
        mtime: u64::from(optional_nonnegative_int_field(&dict, &field, "mtime")?.unwrap_or(0)),
    })
}

fn optional_nonnegative_int_field(
    dict: &DictRef<'_>,
    field: &str,
    name: &str,
) -> Result<Option<u32>> {
    dict.get_str(name)
        .map(|value| {
            let value = value
                .unpack_i32()
                .ok_or_else(|| anyhow!("expected `{field}.{name}` to be an integer"))?;
            u32::try_from(value)
                .map_err(|_| anyhow!("expected `{field}.{name}` to be non-negative"))
        })
        .transpose()
}

fn validate_sandbox(value: Option<&str>) -> Result<()> {
    match value {
        None | Some("off" | "inputs" | "copied-inputs") => Ok(()),
        Some(other) => Err(anyhow!(
            "expected `sandbox` to be `off`, `inputs`, or `copied-inputs`, got `{other}`"
        )),
    }
}

fn unpack_i32_list(value: Value<'_>, field: &str) -> anyhow::Result<Vec<i32>> {
    let list = ListRef::from_value(value).ok_or_else(|| {
        anyhow!(
            "expected `{field}` to be a list of integers, got `{}`",
            value.get_type()
        )
    })?;
    list.iter()
        .enumerate()
        .map(|(index, item)| {
            let value = item
                .unpack_i32()
                .ok_or_else(|| anyhow!("expected `{field}` entry {index} to be an integer"))?;
            if value < 0 {
                anyhow::bail!("expected `{field}` entry {index} to be non-negative");
            }
            Ok(value)
        })
        .collect()
}

fn unpack_action_argv(value: Value<'_>, field: &str) -> anyhow::Result<ActionArgv> {
    let list = ListRef::from_value(value).ok_or_else(|| {
        anyhow!(
            "expected `{field}` to be a list of strings or cmd_args values, got `{}`",
            value.get_type()
        )
    })?;
    let mut argv = ActionArgv {
        args: Vec::new(),
        arg_files: Vec::new(),
    };
    for (index, item) in list.iter().enumerate() {
        unpack_action_argv_item(item, field, index, &mut argv)?;
    }
    Ok(argv)
}

fn unpack_action_argv_item(
    value: Value<'_>,
    field: &str,
    index: usize,
    argv: &mut ActionArgv,
) -> anyhow::Result<()> {
    if let Some(arg) = value.unpack_str() {
        argv.args.push(arg.to_string());
        return Ok(());
    }
    if let Some(dict) = DictRef::from_value(value) {
        if dict
            .get_str(CMD_ARGS_MARKER)
            .and_then(Value::unpack_bool)
            .unwrap_or(false)
        {
            return unpack_cmd_args_value(&dict, field, index, argv);
        }
    }
    Err(anyhow!(
        "expected `{field}` entries to be strings or cmd_args values, got `{}`",
        value.get_type()
    ))
}

fn unpack_cmd_args_value(
    dict: &DictRef<'_>,
    field: &str,
    index: usize,
    argv: &mut ActionArgv,
) -> anyhow::Result<()> {
    let fragment_args = dict
        .get_str("args")
        .ok_or_else(|| anyhow!("expected `{field}` entry {index} cmd_args to contain `args`"))
        .and_then(|value| unpack_string_list(value, "cmd_args.args"))?;
    let Some(path) = optional_string_field(dict, "arg_file_path")? else {
        argv.args.extend(fragment_args);
        return Ok(());
    };
    let format = parse_arg_file_format(
        optional_string_field(dict, "arg_file_format")?
            .unwrap_or_else(|| "line-delimited".to_string())
            .as_str(),
        "cmd_args.use_arg_file.format",
    )?;
    validate_declared_arg_file_args(format, &fragment_args, &path)?;
    let arg_format =
        optional_string_field(dict, "arg_format")?.unwrap_or_else(|| "@{}".to_string());
    argv.args.push(apply_arg_format(&arg_format, &path)?);
    argv.arg_files.push(DeclaredArgFile {
        path,
        format,
        args: fragment_args,
    });
    Ok(())
}

fn unpack_cmd_args_arg_file(value: Value<'_>) -> anyhow::Result<Option<CmdArgsArgFile>> {
    if value.is_none() {
        return Ok(None);
    }
    let dict = DictRef::from_value(value).ok_or_else(|| {
        anyhow!(
            "expected `cmd_args.use_arg_file` to be a dict, got `{}`",
            value.get_type()
        )
    })?;
    let path = required_string_field(&dict, "cmd_args.use_arg_file", "path")?;
    let format = parse_arg_file_format(
        optional_string_field(&dict, "format")?
            .unwrap_or_else(|| "line-delimited".to_string())
            .as_str(),
        "cmd_args.use_arg_file.format",
    )?;
    let arg_format =
        optional_string_field(&dict, "arg_format")?.unwrap_or_else(|| "@{}".to_string());
    Ok(Some(CmdArgsArgFile {
        path,
        format,
        arg_format,
    }))
}

fn parse_arg_file_format(value: &str, field: &str) -> anyhow::Result<DeclaredArgFileFormat> {
    match value {
        "line-delimited" => Ok(DeclaredArgFileFormat::LineDelimited),
        other => Err(anyhow!(
            "expected `{field}` to be `line-delimited`, got `{other}`"
        )),
    }
}

fn validate_declared_arg_file_args(
    format: DeclaredArgFileFormat,
    args: &[String],
    path: &str,
) -> anyhow::Result<()> {
    match format {
        DeclaredArgFileFormat::LineDelimited => {
            for arg in args {
                if arg.contains('\n') || arg.contains('\r') {
                    return Err(anyhow!(
                        "{} arg file `{path}` contains an argument with a newline",
                        format.as_str()
                    ));
                }
            }
            Ok(())
        }
    }
}

fn apply_arg_format(format: &str, path: &str) -> anyhow::Result<String> {
    if format.matches("{}").count() != 1 {
        return Err(anyhow!(
            "expected `cmd_args.use_arg_file.arg_format` to contain exactly one `{{}}` placeholder"
        ));
    }
    Ok(format.replace("{}", path))
}

fn required_string_field(dict: &DictRef<'_>, field: &str, name: &str) -> anyhow::Result<String> {
    dict.get_str(name)
        .ok_or_else(|| anyhow!("expected `{field}` to contain `{name}`"))?
        .unpack_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow!("expected `{field}.{name}` to be a string"))
}

fn optional_string_field(dict: &DictRef<'_>, name: &str) -> anyhow::Result<Option<String>> {
    dict.get_str(name)
        .map(|value| {
            value
                .unpack_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| anyhow!("expected `{name}` to be a string"))
        })
        .transpose()
}

fn observe_host_path(path: &Path) -> Result<()> {
    with_store(|store| {
        let store = store.ok_or_else(|| anyhow!("host path observed outside analysis"))?;
        store.host_cache.observe_path(path);
        Ok(())
    })
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
/// Each match is canonicalized for containment validation but returned by its
/// logical workspace path. This keeps an internal symlink visible to actions
/// that need to materialize its value while rejecting links that point outside
/// the tree. The check is best-effort against the on-disk state at evaluation
/// time: a write-capable attacker on the workspace could in principle swap a
/// symlink between the directory walk and `canonicalize`. Once treats the workspace
/// as trusted, so this TOCTOU window is out of scope for the threat model.
/// Windows junctions are not exercised by tests yet; the `canonicalize` call
/// covers them in production but a dedicated Windows test should land
/// alongside Windows CI.
#[cfg(test)]
pub(super) fn expand_globs(
    workspace_root: &Path,
    package: &str,
    patterns: &[String],
) -> Result<Vec<String>> {
    expand_globs_with_excludes(workspace_root, package, patterns, &[])
}

pub fn expand_globs_with_excludes(
    workspace_root: &Path,
    package: &str,
    patterns: &[String],
    excludes: &[String],
) -> Result<Vec<String>> {
    let package_dir = if package.is_empty() {
        workspace_root.to_path_buf()
    } else {
        workspace_root.join(package)
    };
    let excludes = excludes
        .iter()
        .map(|pattern| {
            glob::Pattern::new(pattern)
                .with_context(|| format!("invalid exclude glob pattern `{pattern}`"))
        })
        .collect::<Result<Vec<_>>>()?;
    let canonical_workspace = std::fs::canonicalize(workspace_root)
        .with_context(|| format!("canonicalizing workspace `{}`", workspace_root.display()))?;
    let patterns = patterns
        .iter()
        .map(|pattern| {
            let normalized = pattern.strip_prefix("./").unwrap_or(pattern);
            glob::Pattern::new(normalized)
                .with_context(|| format!("invalid glob pattern `{pattern}`"))
        })
        .collect::<Result<Vec<_>>>()?;
    let walk_workspace = patterns.iter().any(|pattern| {
        Path::new(pattern.as_str())
            .components()
            .any(|component| component == Component::ParentDir)
    });
    let walk_root = if walk_workspace {
        workspace_root.to_path_buf()
    } else {
        let narrowed = narrow_walk_root(&package_dir, &patterns);
        narrowed.unwrap_or_else(|| package_dir.clone())
    };
    if !walk_root.exists() {
        return Ok(Vec::new());
    }

    let mut out = collect_glob_matches(
        workspace_root,
        package,
        walk_root.as_path(),
        &patterns,
        &excludes,
        &canonical_workspace,
    )?;
    out.sort();
    out.dedup();
    let symlink_roots = out
        .iter()
        .filter(|path| {
            std::fs::symlink_metadata(workspace_root.join(path))
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
        })
        .cloned()
        .collect::<Vec<_>>();
    out.retain(|candidate| {
        !symlink_roots.iter().any(|root| {
            candidate
                .strip_prefix(root)
                .is_some_and(|suffix| suffix.starts_with('/'))
        })
    });
    Ok(out)
}

/// Compute the deepest directory under `package_dir` that could contain
/// files matching any of the compiled `patterns`. Returns `None` when a
/// pattern starts with a wildcard or has no literal directory prefix,
/// meaning the walk cannot be narrowed below `package_dir`.
fn narrow_walk_root(package_dir: &Path, patterns: &[glob::Pattern]) -> Option<PathBuf> {
    let mut common: Option<PathBuf> = None;
    for pattern in patterns {
        let raw = pattern.as_str();
        let wildcard_pos = raw.find(['*', '?', '[']);
        let literal = match wildcard_pos {
            Some(pos) => &raw[..pos],
            None => raw,
        };
        let dir_str = if literal.is_empty() {
            return None;
        } else if let Some(stripped) = literal.strip_suffix('/') {
            stripped
        } else {
            match literal.rfind('/') {
                Some(pos) => &literal[..pos],
                None => return None,
            }
        };
        if dir_str.is_empty() {
            return None;
        }
        // An absolute literal prefix would make `join` discard `package_dir`
        // and could collapse the common ancestor to the filesystem root,
        // walking far more than the package. Fall back to the package walk.
        if Path::new(dir_str).is_absolute() {
            return None;
        }
        let candidate = package_dir.join(dir_str);
        common = Some(match common {
            None => candidate,
            Some(existing) => common_ancestor(&existing, &candidate),
        });
        if common.as_ref().is_some_and(|c| c == package_dir) {
            return None;
        }
    }
    common.filter(|path| path != package_dir)
}

fn common_ancestor(a: &Path, b: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for (comp_a, comp_b) in a.components().zip(b.components()) {
        if comp_a == comp_b {
            result.push(comp_a.as_os_str());
        } else {
            break;
        }
    }
    result
}

fn collect_glob_matches(
    workspace_root: &Path,
    package: &str,
    walk_root: &Path,
    patterns: &[glob::Pattern],
    excludes: &[glob::Pattern],
    canonical_workspace: &Path,
) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let walker = WalkDir::new(walk_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            if entry.depth() == 0 {
                return true;
            }
            let Ok(logical) = entry.path().strip_prefix(workspace_root) else {
                return true;
            };
            if logical
                .components()
                .any(|component| component.as_os_str() == ".once")
            {
                return false;
            }
            if !entry.file_type().is_dir() {
                return true;
            }
            let Ok(ws_rel) = normalize_logical_workspace_path(logical) else {
                return true;
            };
            let package_relative = workspace_path_relative_to_package(package, &ws_rel);
            !excludes.iter().any(|pattern| {
                pattern.matches(&format!("{package_relative}/__once_glob_descendant__"))
            })
        });
    for entry in walker {
        let entry = entry.with_context(|| {
            format!(
                "walking glob root `{}` without following directory symlinks",
                walk_root.display()
            )
        })?;
        if entry.depth() == 0 {
            continue;
        }
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(path)
            .with_context(|| format!("reading metadata for `{}`", path.display()))?;
        if !metadata.is_file() && !metadata.file_type().is_symlink() {
            continue;
        }
        let logical = path.strip_prefix(workspace_root).with_context(|| {
            format!(
                "glob result `{}` is outside workspace `{}`",
                path.display(),
                workspace_root.display()
            )
        })?;
        let ws_rel = normalize_logical_workspace_path(logical)?;
        let package_relative = workspace_path_relative_to_package(package, &ws_rel);
        let symlink_tree = metadata.file_type().is_symlink() && path.is_dir();
        let matches = patterns.iter().any(|pattern| {
            pattern.matches(&package_relative)
                || symlink_tree
                    && pattern.matches(&format!("{package_relative}/__once_glob_descendant__"))
        });
        if !matches {
            continue;
        }
        let is_excluded = excludes
            .iter()
            .any(|pattern| pattern.matches(&package_relative));
        let excluded_symlink_tree = symlink_tree
            && excludes.iter().any(|pattern| {
                pattern.matches(&format!("{package_relative}/__once_glob_descendant__"))
            });
        if is_excluded || excluded_symlink_tree {
            continue;
        }
        let canonical = std::fs::canonicalize(path)
            .with_context(|| format!("canonicalizing `{}`", path.display()))?;
        canonical
            .strip_prefix(canonical_workspace)
            .with_context(|| {
                format!(
                    "glob result `{}` is outside the workspace `{}`",
                    canonical.display(),
                    canonical_workspace.display()
                )
            })?;
        if !ws_rel.is_empty() {
            out.push(ws_rel);
        }
    }
    Ok(out)
}

fn workspace_path_relative_to_package(package: &str, workspace_path: &str) -> String {
    let package_parts = package
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let path_parts = workspace_path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let common = package_parts
        .iter()
        .zip(path_parts.iter())
        .take_while(|(left, right)| left == right)
        .count();
    std::iter::repeat_n("..", package_parts.len() - common)
        .chain(path_parts[common..].iter().copied())
        .collect::<Vec<_>>()
        .join("/")
}

fn normalize_logical_workspace_path(path: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => parts.push(
                value
                    .to_str()
                    .ok_or_else(|| anyhow!("glob result contains non-UTF-8 text"))?,
            ),
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.pop().is_none() {
                    return Err(anyhow!(
                        "glob result `{}` escapes the workspace",
                        path.display()
                    ));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(anyhow!(
                    "glob result `{}` is not workspace-relative",
                    path.display()
                ));
            }
        }
    }
    Ok(parts.join("/"))
}

pub(super) fn walk_package_files(
    workspace_root: &Path,
    package: &str,
    root: &str,
    excluded_paths: &[String],
    excluded_names: &[String],
) -> Result<Vec<String>> {
    let package_dir = if package.is_empty() {
        workspace_root.to_path_buf()
    } else {
        workspace_root.join(package)
    };
    let root = normalize_walk_path(root, "root", true)?;
    let exclusions = excluded_paths
        .iter()
        .map(|path| normalize_walk_path(path, "excluded path", false))
        .collect::<Result<Vec<_>>>()?;
    let excluded_names = excluded_names
        .iter()
        .map(|name| normalize_walk_name(name))
        .collect::<Result<Vec<_>>>()?;
    let canonical_workspace = std::fs::canonicalize(workspace_root)
        .with_context(|| format!("canonicalizing workspace `{}`", workspace_root.display()))?;
    let requested_root = package_dir.join(root);
    let canonical_root = std::fs::canonicalize(&requested_root)
        .with_context(|| format!("canonicalizing walk root `{}`", requested_root.display()))?;
    canonical_root
        .strip_prefix(&canonical_workspace)
        .with_context(|| {
            format!(
                "walk root `{}` is outside the workspace `{}`",
                canonical_root.display(),
                canonical_workspace.display()
            )
        })?;
    if !canonical_root.is_dir() {
        return Err(anyhow!(
            "walk root `{}` is not a directory",
            requested_root.display()
        ));
    }

    let walker = WalkDir::new(&canonical_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            let relative = entry
                .path()
                .strip_prefix(&canonical_root)
                .unwrap_or(entry.path());
            relative.as_os_str().is_empty()
                || (!excluded_names
                    .iter()
                    .any(|excluded| entry.file_name() == excluded)
                    && !exclusions
                        .iter()
                        .any(|excluded| relative == excluded || relative.starts_with(excluded)))
        });
    let mut out = Vec::new();
    for entry in walker {
        let entry =
            entry.with_context(|| format!("walking directory `{}`", requested_root.display()))?;
        let file_type = entry.file_type();
        if !file_type.is_file() && !file_type.is_symlink() {
            continue;
        }
        let workspace_relative = entry
            .path()
            .strip_prefix(&canonical_workspace)
            .with_context(|| {
                format!(
                    "walk result `{}` is outside the workspace `{}`",
                    entry.path().display(),
                    canonical_workspace.display()
                )
            })?
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        if !workspace_relative.is_empty() {
            out.push(workspace_relative);
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn normalize_walk_path(path: &str, field: &str, allow_empty: bool) -> Result<std::path::PathBuf> {
    let mut normalized = std::path::PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(value) => normalized.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) | Component::RootDir => {
                return Err(anyhow!(
                    "walk_files {field} must stay inside its package, got `{path}`"
                ));
            }
        }
    }
    if !allow_empty && normalized.as_os_str().is_empty() {
        return Err(anyhow!("walk_files {field} must not be empty"));
    }
    Ok(normalized)
}

fn normalize_walk_name(name: &str) -> Result<std::ffi::OsString> {
    if name.is_empty() || name.contains('/') || name.contains('\\') || matches!(name, "." | "..") {
        return Err(anyhow!(
            "walk_files excluded name must be one file name, got `{name}`"
        ));
    }
    Ok(std::ffi::OsString::from(name))
}

#[cfg(test)]
mod narrow_walk_root_tests {
    use super::narrow_walk_root;
    use std::path::{Path, PathBuf};

    fn compile(patterns: &[&str]) -> Vec<glob::Pattern> {
        patterns
            .iter()
            .map(|pattern| glob::Pattern::new(pattern).expect("valid glob"))
            .collect()
    }

    #[test]
    fn narrows_to_literal_prefix() {
        let package = Path::new("/ws");
        let patterns = compile(&["codex-rs/tui/**/*.rs"]);
        assert_eq!(
            narrow_walk_root(package, &patterns),
            Some(PathBuf::from("/ws/codex-rs/tui"))
        );
    }

    #[test]
    fn common_ancestor_across_patterns() {
        let package = Path::new("/ws");
        let patterns = compile(&["codex-rs/tui/**/*.rs", "codex-rs/core/**/*.rs"]);
        assert_eq!(
            narrow_walk_root(package, &patterns),
            Some(PathBuf::from("/ws/codex-rs"))
        );
    }

    #[test]
    fn leading_wildcard_cannot_narrow() {
        let package = Path::new("/ws");
        assert_eq!(narrow_walk_root(package, &compile(&["**/*.rs"])), None);
        assert_eq!(narrow_walk_root(package, &compile(&["*.rs"])), None);
    }

    #[test]
    fn absolute_pattern_does_not_escape_package() {
        // An absolute literal prefix must not make the walk root jump outside
        // the package (previously `join` discarded the package and the common
        // ancestor collapsed toward the filesystem root).
        let package = Path::new("/ws/pkg");
        let patterns = compile(&["/abs/src/*.rs", "src/**/*.rs"]);
        assert_eq!(narrow_walk_root(package, &patterns), None);
        assert_eq!(
            narrow_walk_root(package, &compile(&["/abs/src/*.rs"])),
            None
        );
    }
}

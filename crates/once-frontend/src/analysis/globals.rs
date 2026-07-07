use std::collections::BTreeMap;
use std::path::Path;

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

use super::store::{
    analysis_active, with_store, with_store_mut, DeclaredAction, DeclaredActionOperation,
    DeclaredArgFile, DeclaredArgFileFormat, DeclaredCopyPathMode, DeclaredPreparePathMode,
};
use super::values::{
    json_to_value, toml_value_to_starlark, unpack_byte_list, unpack_string_dict, unpack_string_list,
};

const CMD_ARGS_MARKER: &str = "_once_cmd_args";

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
    /// different cached result. Schema parsing returns `""`.
    fn host_command<'v>(
        argv: Value<'v>,
        env: Option<Value<'v>>,
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
            store.host_cache.command(&argv, &env, merge_stderr)
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
    #[expect(
        clippy::unnecessary_wraps,
        reason = "starlark_module native functions use Result-returning signatures"
    )]
    fn host_file_exists(path: &str) -> anyhow::Result<bool> {
        if !analysis_active() {
            return Ok(false);
        }
        Ok(Path::new(path).is_file())
    }

    /// Read one host file as UTF-8 text.
    fn host_file_read(path: &str) -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        std::fs::read_to_string(path).with_context(|| format!("reading host file `{path}`"))
    }

    /// Return whether one host file contains `needle` as text.
    fn host_file_contains(path: &str, needle: &str) -> anyhow::Result<bool> {
        if !analysis_active() {
            return Ok(false);
        }
        if needle.is_empty() {
            return Ok(true);
        }
        let content = std::fs::read(path).with_context(|| format!("reading host file `{path}`"))?;
        Ok(content
            .windows(needle.len())
            .any(|window| window == needle.as_bytes()))
    }

    /// Return the sorted entry names of host directory `path`. Missing or
    /// non-directory paths (and schema parsing) return an empty list.
    /// Names are bare file names, letting rules enumerate host toolchains
    /// (for example SDK version directories) without a host shell.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "starlark_module native functions use Result-returning signatures"
    )]
    fn host_read_dir<'v>(
        path: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let heap = eval.heap();
        if !analysis_active() {
            return Ok(heap.alloc(Vec::<String>::new()));
        }
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
            cacheable: true,
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

    /// Declare a portable copy action. `kind` is `"file"` by default
    /// or `"tree"` to copy directory contents. Tree copies accept one
    /// source string or a list of source directories.
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
            cacheable: cacheable.unwrap_or(true),
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
            cacheable: false,
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
            cacheable: cacheable.unwrap_or(true),
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
    /// string->string dict; `cacheable`:
    /// optional bool, default true;
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
        depends_on_prior_actions: Option<bool>,
        stdout: Option<String>,
        stderr: Option<String>,
    ) -> anyhow::Result<NoneType> {
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
            cacheable: cacheable.unwrap_or(true),
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

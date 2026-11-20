//! Host queries: platform, environment, commands, and files outside the workspace.

use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use starlark::environment::GlobalsBuilder;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::values::none::NoneType;
use starlark::values::Value;

use super::observation::{
    observe_host_path, observe_platform, observed_which, read_host_dir_names,
};
use super::{
    cached_host_tree_digest, file_sha256_hex, git_checkouts, host_arch_str, host_os_str,
    inherited_environment_sha256, read_bounded_host_file, selected_execution_context, sha256_hex,
    EXECUTION_ROOT_MARKER,
};
use crate::analysis::action_history::make_history;
use crate::analysis::store::{
    analysis_active, observe, observe_unrecordable, with_store, Observation,
    HOST_COMMAND_OUTPUT_LIMIT,
};
use crate::analysis::values::{unpack_string_dict, unpack_string_list, value_to_json};

#[starlark_module]
pub(super) fn host_globals(builder: &mut GlobalsBuilder) {
    #[allow(clippy::unnecessary_wraps)]
    fn execution_context<'v>(eval: &mut Evaluator<'v, '_, '_>) -> anyhow::Result<Value<'v>> {
        Ok(selected_execution_context(eval))
    }

    #[allow(clippy::unnecessary_wraps)]
    fn analysis_warning(message: &str) -> anyhow::Result<NoneType> {
        if analysis_active() {
            eprintln!("warning: {message}");
        }
        Ok(NoneType)
    }

    #[allow(clippy::unnecessary_wraps)]
    fn action_history_key<'v>(
        namespace: Value<'v>,
        components: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        Ok(make_history(namespace, components, eval.heap()))
    }

    /// Host CPU architecture as a normalized string (e.g. `"arm64"`,
    /// `"x86_64"`). Schema parsing returns `""`.
    #[allow(clippy::unnecessary_wraps)]
    fn host_arch() -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        observe_platform();
        Ok(host_arch_str().to_string())
    }

    /// Host operating system as a normalized string (e.g. `"macos"`,
    /// `"linux"`). Schema parsing returns `""`.
    #[allow(clippy::unnecessary_wraps)]
    fn host_os() -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        observe_platform();
        Ok(host_os_str().to_string())
    }

    /// Read one host environment variable. Missing variables return
    /// `""`. Schema parsing returns `""`.
    #[allow(clippy::unnecessary_wraps)]
    fn host_env(name: &str) -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        let value = with_store(|store| -> Result<Option<String>> {
            let store = store.ok_or_else(|| anyhow!("host_env called outside analysis"))?;
            Ok(store.host_cache.env(name))
        })?;
        observe(Observation::Env {
            name: name.to_string(),
            value: value.clone(),
        });
        Ok(value.unwrap_or_default())
    }

    #[allow(clippy::unnecessary_wraps)]
    fn host_environment_sha256() -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        let sha256 = inherited_environment_sha256();
        observe(Observation::EnvironmentDigest {
            sha256: sha256.clone(),
        });
        Ok(sha256)
    }

    /// Active workspace root as an absolute path, spelled for command
    /// lines (without a Windows verbatim prefix). Schema parsing
    /// returns `""`.
    fn workspace_root() -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        with_store(|store| -> Result<String> {
            let store = store.ok_or_else(|| anyhow!("workspace_root called outside analysis"))?;
            Ok(struo_core::command_path(&store.workspace_root))
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
        let resolved = observed_which(name, "host_which")?;
        resolved.ok_or_else(|| anyhow!("`{name}` not found on PATH"))
    }

    /// Like `host_which` but returns `""` when `name` is not on PATH
    /// instead of failing. Lets rules probe for an optional tool without
    /// a host shell. Schema parsing returns `""`.
    fn host_which_optional(name: &str) -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        Ok(observed_which(name, "host_which_optional")?.unwrap_or_default())
    }

    /// Run `argv[0]` with `argv[1..]` as arguments and return its
    /// stdout as a string. Fails if the process exits non-zero;
    /// includes stderr in the error message. Optional `env` is a
    /// `dict<string, string>` of environment variables overlaid on the
    /// host process env. Both `argv` and `env` participate in the
    /// cache key, so a different `DEVELOPER_DIR` resolves to a
    /// different cached result. When set, `cwd` must be absolute.
    ///
    /// The captured output is capped at the default analysis limit. Pass
    /// `max_output_bytes` to raise that bound for a command whose answer is
    /// inherently large, such as a build system's action graph. The bound is
    /// part of the recorded observation, so a later validation re-runs the
    /// command under the same bound.
    /// Schema parsing returns `""`.
    fn host_command<'v>(
        argv: Value<'v>,
        env: Option<Value<'v>>,
        cwd: Option<&str>,
        merge_stderr: Option<bool>,
        check: Option<bool>,
        max_output_bytes: Option<Value<'v>>,
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
        let check = check.unwrap_or(true);
        let max_output_bytes = match max_output_bytes {
            None => HOST_COMMAND_OUTPUT_LIMIT,
            Some(value) => {
                let bytes = value.unpack_i32().ok_or_else(|| {
                    anyhow!(
                        "expected `max_output_bytes` to be an integer, got `{}`",
                        value.get_type()
                    )
                })?;
                u64::try_from(bytes)
                    .map_err(|_| anyhow!("expected `max_output_bytes` to be positive"))?
            }
        };
        let output = with_store(|store| -> Result<String> {
            let store = store.ok_or_else(|| anyhow!("host_command called outside analysis"))?;
            if let Some(cwd) = cwd {
                if !Path::new(cwd).is_absolute() {
                    return Err(anyhow!("host_command cwd must be absolute, got `{cwd}`"));
                }
            }
            store.host_cache.command_checked(
                &argv,
                &env,
                cwd.map(Path::new),
                merge_stderr,
                check,
                max_output_bytes,
            )
        })?;
        // The output digest rather than the output: a discovery command's
        // answer can be megabytes, and replay only needs to know whether the
        // answer is the same one.
        let program = argv.first().and_then(|program| {
            with_store(|store| store.and_then(|store| store.host_cache.program_identity(program)))
        });
        observe(Observation::Command {
            argv,
            env,
            cwd: cwd.map(ToOwned::to_owned),
            merge_stderr,
            check,
            output_sha256: sha256_hex(output.as_bytes()),
            program,
            max_output_bytes: (max_output_bytes != HOST_COMMAND_OUTPUT_LIMIT)
                .then_some(max_output_bytes),
        });
        Ok(output)
    }

    fn host_git_checkout(
        url: &str,
        revision: &str,
        path: &str,
        cache_root: &str,
    ) -> anyhow::Result<NoneType> {
        if !analysis_active() {
            return Ok(NoneType);
        }
        git_checkouts(
            &[crate::analysis::repository::Checkout {
                url: url.into(),
                revision: revision.into(),
                path: path.into(),
            }],
            cache_root,
        )?;
        Ok(NoneType)
    }

    fn host_git_checkouts(repositories: Value<'_>, cache_root: &str) -> anyhow::Result<NoneType> {
        if !analysis_active() {
            return Ok(NoneType);
        }
        let repositories: Vec<crate::analysis::repository::Checkout> =
            serde_json::from_value(value_to_json(repositories))
                .context("host_git_checkouts expects a list of {url, revision, path} records")?;
        git_checkouts(&repositories, cache_root)?;
        Ok(NoneType)
    }

    fn host_cached_command<'v>(
        argv: Value<'v>,
        inputs: Value<'v>,
        env: Option<Value<'v>>,
        cwd: Option<&str>,
        check: Option<bool>,
    ) -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        let argv = unpack_string_list(argv, "argv")?;
        let inputs = unpack_string_list(inputs, "inputs")?;
        let env = env
            .map(|value| unpack_string_dict(value, "env"))
            .transpose()?
            .unwrap_or_default();
        if cwd.is_some_and(|path| !Path::new(path).is_absolute()) {
            anyhow::bail!("host_cached_command cwd must be absolute");
        }
        let cwd = cwd.map_or_else(
            || with_store(|store| store.unwrap().workspace_root.clone()),
            PathBuf::from,
        );
        let program = argv
            .first()
            .context("host_cached_command requires a non-empty argv")?;
        let resolved = observed_which(program, "host_cached_command")?
            .context("host_cached_command executable not found")?;
        let environment = inherited_environment_sha256();
        observe(Observation::EnvironmentDigest {
            sha256: environment.clone(),
        });
        let ((output, success), digests) = with_store(|store| -> Result<_> {
            let store = store.context("host_cached_command called outside analysis")?;
            let mut digests = Vec::new();
            for path in std::iter::once(&resolved).chain(inputs.iter()) {
                if !Path::new(path).is_absolute() {
                    anyhow::bail!("host_cached_command inputs must be absolute file paths");
                }
                store.host_cache.observe_path(Path::new(path));
                let digest = store
                    .host_cache
                    .host_file_digest(Path::new(path), || file_sha256_hex(Path::new(path)))?;
                digests.push((path.clone(), digest));
            }
            let slot = sha256_hex(&serde_json::to_vec(&(&argv, &env, &cwd))?);
            let key = sha256_hex(&serde_json::to_vec(&(&digests, &environment))?);
            let output =
                crate::analysis::metadata::command(&store.workspace_root, &slot, &key, || {
                    store.host_cache.run_command(
                        &argv,
                        &env,
                        Some(&cwd),
                        false,
                        check.unwrap_or(true),
                        HOST_COMMAND_OUTPUT_LIMIT,
                    )
                })?;
            Ok((output, digests))
        })?;
        for (path, sha256) in digests {
            observe(Observation::FileDigest { path, sha256 });
        }
        if !success {
            observe_unrecordable();
        }
        Ok(output)
    }

    /// Return the SHA-256 digest of one host file as lowercase hex.
    /// This is for host-specific tool or signing inputs that cannot be
    /// declared as workspace action inputs.
    fn host_file_sha256(path: &str) -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        observe_host_path(Path::new(path))?;
        let digest = with_store(|store| {
            let store = store.ok_or_else(|| anyhow!("host_file_sha256 called outside analysis"))?;
            store.host_cache.host_file_digest(Path::new(path), || {
                file_sha256_hex(Path::new(path))
                    .with_context(|| format!("hashing host file `{path}`"))
            })
        })?;
        observe(Observation::FileDigest {
            path: path.to_string(),
            sha256: digest.clone(),
        });
        Ok(digest)
    }

    /// Return the size in bytes of one host file. This lets a deferred
    /// planner describe a produced artifact, such as an archive layer, without
    /// reading it into memory.
    fn host_file_size(path: &str) -> anyhow::Result<i64> {
        if !analysis_active() {
            return Ok(0);
        }
        observe_host_path(Path::new(path))?;
        let bytes = std::fs::metadata(path)
            .with_context(|| format!("reading the size of host file `{path}`"))?
            .len();
        let bytes = i64::try_from(bytes).context("host file is larger than the supported size")?;
        observe(Observation::FileSize {
            path: path.to_string(),
            bytes,
        });
        Ok(bytes)
    }

    /// Return where a host symbolic link points, exactly as written, or an
    /// empty string when the path is not a symbolic link. A target kind uses
    /// it to declare the files a linked input depends on.
    fn host_symlink_target(path: &str) -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        observe_host_path(Path::new(path))?;
        let target = match std::fs::read_link(path) {
            Ok(target) => target
                .into_os_string()
                .into_string()
                .map_err(|_| anyhow!("host symbolic link `{path}` has a non-UTF-8 target"))?,
            Err(_) => String::new(),
        };
        observe(Observation::SymlinkTarget {
            path: path.to_string(),
            target: target.clone(),
        });
        Ok(target)
    }

    #[allow(clippy::unnecessary_wraps)]
    fn content_sha256(text: &str) -> anyhow::Result<String> {
        Ok(sha256_hex(text.as_bytes()))
    }

    fn host_tree_sha256(path: &str) -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        let source = Path::new(path);
        if !source.is_absolute() || !source.is_dir() {
            anyhow::bail!("host_tree_sha256 requires an absolute directory path");
        }
        let digest = with_store(|store| -> anyhow::Result<String> {
            let store = store.context("host_tree_sha256 called outside analysis")?;
            cached_host_tree_digest(&store.host_cache, &store.workspace_root, source)
        })?;
        observe(Observation::TreeDigest {
            path: path.to_string(),
            sha256: digest.clone(),
        });
        Ok(digest)
    }

    /// Return whether one host path currently exists as a file.
    fn host_file_exists(path: &str) -> anyhow::Result<bool> {
        if !analysis_active() {
            return Ok(false);
        }
        observe_host_path(Path::new(path))?;
        let exists = Path::new(path).is_file();
        observe(Observation::FileExists {
            path: path.to_string(),
            exists,
        });
        Ok(exists)
    }

    /// Return whether one host path currently exists, including directories
    /// and symbolic links. Use this when a resolver needs to probe a source
    /// tree rather than read a regular file.
    fn host_path_exists(path: &str) -> anyhow::Result<bool> {
        if !analysis_active() {
            return Ok(false);
        }
        observe_host_path(Path::new(path))?;
        let exists = Path::new(path).exists();
        observe(Observation::PathExists {
            path: path.to_string(),
            exists,
        });
        Ok(exists)
    }

    /// Return whether one existing host path resolves within another existing
    /// host path. Symbolic links are resolved before containment is checked.
    fn host_path_is_within(path: &str, root: &str) -> anyhow::Result<bool> {
        if !analysis_active() {
            return Ok(false);
        }
        let path = Path::new(path);
        let root = Path::new(root);
        observe_host_path(path)?;
        observe_host_path(root)?;
        let canonical_path = std::fs::canonicalize(path)
            .with_context(|| format!("canonicalizing host path `{}`", path.display()))?;
        let canonical_root = std::fs::canonicalize(root)
            .with_context(|| format!("canonicalizing host root `{}`", root.display()))?;
        // Both sides are resolved through their symlinks, so the answer follows
        // from where those links point and not only from the two strings.
        let within = canonical_path.starts_with(canonical_root);
        observe(Observation::PathWithin {
            path: path.to_string_lossy().into_owned(),
            root: root.to_string_lossy().into_owned(),
            within,
        });
        Ok(within)
    }

    /// Read one host file as UTF-8 text.
    fn host_file_read(path: &str) -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        observe_host_path(Path::new(path))?;
        let bytes = read_bounded_host_file(path)?;
        observe(Observation::FileText {
            path: path.to_string(),
            sha256: sha256_hex(&bytes),
        });
        String::from_utf8(bytes).with_context(|| format!("host file `{path}` is not UTF-8 text"))
    }

    /// Read the encapsulated content of one host CMS `SignedData` file, such
    /// as a signed configuration document, as UTF-8 text. The signature is
    /// not verified.
    fn host_file_cms_content(path: &str) -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        observe_host_path(Path::new(path))?;
        let bytes = read_bounded_host_file(path)?;
        observe(Observation::FileText {
            path: path.to_string(),
            sha256: sha256_hex(&bytes),
        });
        let content = crate::analysis::property_list::cms_signed_content(&bytes)
            .with_context(|| format!("reading CMS content of host file `{path}`"))?;
        String::from_utf8(content)
            .with_context(|| format!("CMS content of host file `{path}` is not UTF-8 text"))
    }

    /// Return whether one host file contains `needle` as text.
    fn host_file_contains(path: &str, needle: &str) -> anyhow::Result<bool> {
        if !analysis_active() {
            return Ok(false);
        }
        observe_host_path(Path::new(path))?;
        let found = if needle.is_empty() {
            true
        } else {
            let content = read_bounded_host_file(path)?;
            content
                .windows(needle.len())
                .any(|window| window == needle.as_bytes())
        };
        observe(Observation::FileContains {
            path: path.to_string(),
            needle: needle.to_string(),
            found,
        });
        Ok(found)
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
        let names = read_host_dir_names(path);
        observe(Observation::ReadDir {
            path: path.to_string(),
            names: names.clone(),
        });
        Ok(heap.alloc(names))
    }
}

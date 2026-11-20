//! Lowering of Starlark-declared actions into executable core [`Action`]s,
//! including argument files, archive entries, and the input digest and
//! fingerprint that key each action.

use std::collections::BTreeMap;
use std::env;
use std::path::Path;

use anyhow::{Context, Result};
use struo_cas::Digest;
use struo_core::{
    Action, ArchiveEntry, ArchiveEntryKind, ArchiveFormat, CopyPathMode, InputDigestBuilder,
    InputFingerprintManifest, NetworkPolicy, OutputSymlinkMode, PreparePathMode, SandboxMode,
    WorkspacePath,
};
use struo_frontend::analysis::{
    DeclaredAction, DeclaredActionOperation, DeclaredArchiveEntryKind, DeclaredArchiveFormat,
    DeclaredArgFile, DeclaredArgFileFormat, DeclaredCopyPathMode, DeclaredPreparePathMode,
};

use crate::commands::graph::analysis::source_digest_cache::SourceDigestCache;
use crate::commands::graph::analysis::AvailableInput;

pub(super) struct PreparedDeclaredAction {
    pub(super) action: Action,
    pub(super) input_fingerprint: InputFingerprintManifest,
}

pub(super) fn declared_to_action(
    workspace: &Path,
    declared: &DeclaredAction,
    module_source_digest: Digest,
    dep_action_digests: &[(String, Digest)],
    sandbox_override: SandboxMode,
) -> Result<Action> {
    Ok(prepare_declared_action_with_inputs(
        workspace,
        declared,
        module_source_digest,
        dep_action_digests,
        &BTreeMap::new(),
        None,
        sandbox_override,
    )?
    .action)
}

pub(super) fn prepare_declared_action_with_inputs(
    workspace: &Path,
    declared: &DeclaredAction,
    module_source_digest: Digest,
    dep_action_digests: &[(String, Digest)],
    available_inputs: &BTreeMap<String, AvailableInput>,
    source_digest_cache: Option<&SourceDigestCache>,
    sandbox_override: SandboxMode,
) -> Result<PreparedDeclaredAction> {
    let inherited;
    let declared = if declared.cacheable && declared.inherit_parent_env {
        anyhow::ensure!(
            effective_sandbox(declared.sandbox.as_deref(), sandbox_override)? == SandboxMode::Off,
            "inherit_parent_env requires sandbox = \"off\""
        );
        let mut effective = declared.clone();
        effective.env = env::vars_os()
            .map(|(key, value)| {
                Ok((
                    key.into_string().map_err(|_| {
                        anyhow::anyhow!("inherited environment names must be UTF-8")
                    })?,
                    value.into_string().map_err(|_| {
                        anyhow::anyhow!("inherited environment values must be UTF-8")
                    })?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        effective.env.extend(declared.env.clone());
        inherited = effective;
        &inherited
    } else {
        declared
    };
    let env_keys = declared.env.keys().cloned().collect::<Vec<_>>();
    tracing::trace!(
        identifier = ?declared.identifier,
        argv_len = declared.argv.len(),
        env_keys = ?env_keys,
        inputs = declared.inputs.len(),
        outputs = declared.outputs.len(),
        "declared graph action"
    );
    let input_fingerprint = compose_input_fingerprint_with_available(
        workspace,
        declared,
        module_source_digest,
        dep_action_digests,
        available_inputs,
        source_digest_cache,
    )?;
    let input_digest = input_fingerprint.input_digest;
    let inputs = declared_action_inputs(declared, true)?;
    let outputs = declared
        .outputs
        .iter()
        .map(|path| {
            WorkspacePath::try_from(path.as_str())
                .with_context(|| format!("invalid declared output path `{path}`"))
        })
        .collect::<Result<Vec<_>>>()?;
    let stdout_path = declared
        .stdout
        .as_deref()
        .map(|path| workspace_path(path, "run_action stdout path"))
        .transpose()?
        .map(Box::new);
    let stderr_path = declared
        .stderr
        .as_deref()
        .map(|path| workspace_path(path, "run_action stderr path"))
        .transpose()?
        .map(Box::new);
    let action = match &declared.operation {
        None => Action::RunCommand {
            argv: declared.argv.clone(),
            env: declared.env.clone(),
            cwd: declared
                .cwd
                .as_deref()
                .map(|cwd| workspace_path(cwd, "run_action cwd"))
                .transpose()?,
            input_digest: Some(input_digest),
            inputs,
            outputs,
            stdout_path,
            stderr_path,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: declared.resources.clone(),
            sandbox: effective_sandbox(declared.sandbox.as_deref(), sandbox_override)?,
            network: effective_network(declared.network.as_deref())?,
            timeout_ms: None,
            success_exit_codes: declared.success_exit_codes.clone(),
            remote: None,
        },
        Some(operation) => operation_to_action(operation.clone(), input_digest)?,
    };
    Ok(PreparedDeclaredAction {
        action,
        input_fingerprint,
    })
}

pub(super) fn materialize_declared_arg_files(
    workspace: &Path,
    arg_files: &[DeclaredArgFile],
) -> Result<()> {
    for arg_file in arg_files {
        let path = workspace_path(&arg_file.path, "arg_files path")?;
        let absolute = path.resolve(workspace);
        if let Some(parent) = absolute.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("creating parent directory for arg file `{}`", path.as_str())
            })?;
        }
        let content = declared_arg_file_content(arg_file)?;
        std::fs::write(&absolute, content)
            .with_context(|| format!("writing arg file `{}`", path.as_str()))?;
    }
    Ok(())
}

pub(super) fn declared_arg_file_content(arg_file: &DeclaredArgFile) -> Result<Vec<u8>> {
    match arg_file.format {
        DeclaredArgFileFormat::LineDelimited => declared_arg_file_lines(arg_file, |arg| {
            validate_arg_file_line(arg_file, arg)?;
            Ok(arg.to_string())
        }),
    }
}

pub(super) fn declared_arg_file_lines(
    arg_file: &DeclaredArgFile,
    format: impl Fn(&str) -> Result<String>,
) -> Result<Vec<u8>> {
    let mut content = Vec::new();
    for arg in &arg_file.args {
        let line = format(arg)?;
        content.extend_from_slice(line.as_bytes());
        content.push(b'\n');
    }
    Ok(content)
}

pub(super) fn validate_arg_file_line(arg_file: &DeclaredArgFile, arg: &str) -> Result<()> {
    if arg.contains('\n') || arg.contains('\r') {
        anyhow::bail!(
            "{} arg file `{}` contains an argument with a newline",
            declared_arg_file_format_name(arg_file.format),
            arg_file.path
        );
    }
    Ok(())
}

pub(super) fn declared_arg_file_format_name(format: DeclaredArgFileFormat) -> &'static str {
    match format {
        DeclaredArgFileFormat::LineDelimited => "line-delimited",
    }
}

#[allow(clippy::too_many_lines)]
pub(super) fn operation_to_action(
    operation: DeclaredActionOperation,
    input_digest: Digest,
) -> Result<Action> {
    Ok(match operation {
        DeclaredActionOperation::ExpandActions { .. } => {
            anyhow::bail!("deferred actions must be expanded before execution")
        }
        DeclaredActionOperation::WriteFile { path, bytes } => Action::WriteFile {
            path: workspace_path(&path, "write_path path")?,
            bytes: bytes.into(),
            input_digest: Some(input_digest),
        },
        DeclaredActionOperation::CopyPath {
            sources,
            destination,
            mode,
        } => Action::CopyPath {
            sources: sources
                .iter()
                .map(|source| workspace_path(source, "copy_path source"))
                .collect::<Result<Vec<_>>>()?,
            destination: workspace_path(&destination, "copy_path destination")?,
            mode: match mode {
                DeclaredCopyPathMode::File => CopyPathMode::File,
                DeclaredCopyPathMode::Tree => CopyPathMode::Tree,
                DeclaredCopyPathMode::Files { strip_prefix } => CopyPathMode::Files {
                    strip_prefix: workspace_path(&strip_prefix, "copy_path strip_prefix")?,
                },
            },
            input_digest: Some(input_digest),
        },
        DeclaredActionOperation::LinkPath {
            source,
            destination,
        } => Action::LinkPath {
            source: workspace_path(&source, "link_path source")?,
            destination: workspace_path(&destination, "link_path destination")?,
            input_digest: Some(input_digest),
        },
        DeclaredActionOperation::MaterializeHostFile {
            source,
            source_sha256,
            source_mode,
            destination,
        } => {
            let source = std::path::PathBuf::from(source);
            if !source.is_absolute() {
                anyhow::bail!(
                    "invalid materialize_host_file source `{}`: expected an absolute path",
                    source.display()
                );
            }
            Action::MaterializeHostFile {
                source,
                source_sha256,
                source_mode,
                destination: workspace_path(&destination, "materialize_host_file destination")?,
                input_digest: Some(input_digest),
            }
        }
        DeclaredActionOperation::MaterializeHostTree {
            source,
            source_sha256,
            destination,
        } => {
            let source = std::path::PathBuf::from(source);
            if !source.is_absolute() {
                anyhow::bail!(
                    "invalid materialize_host_tree source `{}`: expected an absolute path",
                    source.display()
                );
            }
            Action::MaterializeHostTree {
                source,
                source_sha256,
                destination: workspace_path(&destination, "materialize_host_tree destination")?,
                input_digest: Some(input_digest),
            }
        }
        DeclaredActionOperation::PreparePath { path, mode } => Action::PreparePath {
            path: workspace_path(&path, "prepare_path path")?,
            mode: match mode {
                DeclaredPreparePathMode::Remove => PreparePathMode::Remove,
                DeclaredPreparePathMode::Directory => PreparePathMode::Directory,
            },
            input_digest: Some(input_digest),
        },
        DeclaredActionOperation::WriteTreeDigest {
            root,
            output,
            include_suffixes,
        } => Action::WriteTreeDigest {
            root: workspace_path(&root, "write_tree_digest root")?,
            output: workspace_path(&output, "write_tree_digest output")?,
            include_suffixes,
            input_digest: Some(input_digest),
        },
        DeclaredActionOperation::WriteArchive {
            entries,
            output,
            sha256_output,
            uncompressed_sha256_output,
            format,
        } => archive_to_action(
            entries,
            &output,
            sha256_output.as_deref(),
            uncompressed_sha256_output.as_deref(),
            format,
            input_digest,
        )?,
        DeclaredActionOperation::DownloadAndExtract {
            url,
            sha256,
            destination,
            authorization_env,
        } => Action::DownloadAndExtract {
            url,
            sha256,
            destination: workspace_path(&destination, "download_and_extract destination")?,
            authorization_env,
            input_digest: Some(input_digest),
        },
    })
}

pub(super) fn archive_to_action(
    entries: Vec<struo_frontend::analysis::DeclaredArchiveEntry>,
    output: &str,
    sha256_output: Option<&str>,
    uncompressed_sha256_output: Option<&str>,
    format: DeclaredArchiveFormat,
    input_digest: Digest,
) -> Result<Action> {
    let entries = entries
        .into_iter()
        .map(|entry| {
            Ok(ArchiveEntry {
                kind: match entry.kind {
                    DeclaredArchiveEntryKind::File => ArchiveEntryKind::File,
                    DeclaredArchiveEntryKind::Directory => ArchiveEntryKind::Directory,
                    DeclaredArchiveEntryKind::Tree => ArchiveEntryKind::Tree,
                    DeclaredArchiveEntryKind::Symlink => ArchiveEntryKind::Symlink,
                },
                source: entry
                    .source
                    .as_deref()
                    .map(|source| workspace_path(source, "write_archive entry source"))
                    .transpose()?,
                path: entry.path,
                target: entry.target,
                mode: entry.mode,
                directory_mode: entry.directory_mode,
                owner_id: entry.owner_id,
                group_id: entry.group_id,
                mtime: entry.mtime,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Action::WriteArchive {
        entries,
        output: workspace_path(output, "write_archive output")?,
        sha256_output: sha256_output
            .map(|path| workspace_path(path, "write_archive sha256_output"))
            .transpose()?,
        uncompressed_sha256_output: uncompressed_sha256_output
            .map(|path| workspace_path(path, "write_archive uncompressed_sha256_output"))
            .transpose()?,
        format: match format {
            DeclaredArchiveFormat::Tar => ArchiveFormat::Tar,
            DeclaredArchiveFormat::TarGz => ArchiveFormat::TarGz,
            DeclaredArchiveFormat::Zip => ArchiveFormat::Zip,
        },
        input_digest: Some(input_digest),
    })
}

pub(super) fn workspace_path(path: &str, context: &str) -> Result<WorkspacePath> {
    WorkspacePath::try_from(path).with_context(|| format!("invalid {context} `{path}`"))
}

/// Declared inputs that do not exist in the workspace. During isolated action
/// validation these are outputs a prior action or dependency target would have
/// produced, which the probe never runs. Malformed paths are left out so
/// `declared_to_action` can surface them as the usual invalid-path error.
pub(super) fn missing_declared_inputs(workspace: &Path, declared: &DeclaredAction) -> Vec<String> {
    declared
        .inputs
        .iter()
        .filter(|input| {
            WorkspacePath::try_from(input.as_str())
                .is_ok_and(|path| !path.resolve(workspace).exists())
        })
        .cloned()
        .collect()
}

pub(super) fn declared_action_inputs(
    declared: &DeclaredAction,
    include_setup: bool,
) -> Result<Vec<WorkspacePath>> {
    let mut inputs = declared
        .inputs
        .iter()
        .map(|path| workspace_path(path, "run_action input"))
        .collect::<Result<Vec<_>>>()?;
    for arg_file in &declared.arg_files {
        inputs.push(workspace_path(&arg_file.path, "run_action arg file")?);
    }
    for path in &declared.create_dirs {
        // Native input sandboxes stage private directories. Providers receive
        // setup separately instead of inheriting mutable scratch contents.
        if include_setup && (path == ".struo/tmp" || path.starts_with(".struo/tmp/")) {
            inputs.push(workspace_path(path, "run_action create_dirs entry")?);
        }
    }
    inputs.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    inputs.dedup_by(|a, b| a.as_str() == b.as_str());
    Ok(inputs)
}

pub(super) fn effective_sandbox(
    declared: Option<&str>,
    sandbox_override: SandboxMode,
) -> Result<SandboxMode> {
    let declared = match declared {
        Some(raw) => raw
            .parse::<SandboxMode>()
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("parsing sandbox policy `{raw}`"))?,
        None => SandboxMode::Off,
    };
    Ok(declared.stronger(sandbox_override))
}

pub(super) fn effective_network(declared: Option<&str>) -> Result<NetworkPolicy> {
    match declared {
        Some(raw) => raw
            .parse::<NetworkPolicy>()
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("parsing network policy `{raw}`")),
        // Actions that do not declare a network policy default to
        // unrestricted. The reproducible-build effort meant to switch
        // this to `Deny`, but doing so broke every action that spawns
        // a subprocess needing a socketpair for stdout/stderr (the
        // Linux seccomp filter denies related syscalls) and every
        // action that in turn wraps itself in a macOS Seatbelt profile
        // (nested `sandbox_apply` calls are refused on recent runners).
        // Actions that genuinely need network isolation opt in with
        // `network = "deny"` explicitly.
        None => Ok(NetworkPolicy::Unrestricted),
    }
}

pub(super) fn ensure_output_parent_dirs(workspace: &Path, outputs: &[WorkspacePath]) -> Result<()> {
    for output in outputs {
        let absolute = output.resolve(workspace);
        if let Some(parent) = absolute.parent() {
            let mut directories = parent
                .ancestors()
                .take_while(|directory| *directory != workspace)
                .collect::<Vec<_>>();
            directories.reverse();
            for directory in directories {
                if directory.is_dir() {
                    continue;
                }
                let blocker = std::fs::symlink_metadata(directory).ok();
                if blocker.as_ref().is_some_and(std::fs::Metadata::is_dir) {
                    continue;
                }
                if blocker.is_some() {
                    let path = output.as_str();
                    let managed = path.starts_with(".struo/out/")
                        || path.contains("/.struo/out/")
                        || path.starts_with(".struo/tmp/")
                        || path.contains("/.struo/tmp/");
                    if !managed {
                        anyhow::bail!(
                            "output `{}` has a non-directory parent `{}`",
                            output.as_str(),
                            directory.display()
                        );
                    }
                    std::fs::remove_file(directory).with_context(|| {
                        format!(
                            "removing stale non-directory parent for output `{}`",
                            output.as_str()
                        )
                    })?;
                }
                match std::fs::create_dir(directory) {
                    Ok(()) => {}
                    Err(error)
                        if error.kind() == std::io::ErrorKind::AlreadyExists
                            && directory.is_dir() => {}
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!("creating parent directory for output `{}`", output.as_str())
                        });
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn compose_input_digest(
    workspace: &Path,
    declared: &DeclaredAction,
    module_source_digest: Digest,
    dep_action_digests: &[(String, Digest)],
) -> Result<Digest> {
    compose_input_digest_with_available(
        workspace,
        declared,
        module_source_digest,
        dep_action_digests,
        &BTreeMap::new(),
        None,
    )
}

#[cfg(test)]
pub(super) fn compose_input_digest_with_available(
    workspace: &Path,
    declared: &DeclaredAction,
    module_source_digest: Digest,
    dep_action_digests: &[(String, Digest)],
    available_inputs: &BTreeMap<String, AvailableInput>,
    source_digest_cache: Option<&SourceDigestCache>,
) -> Result<Digest> {
    Ok(compose_input_fingerprint_with_available(
        workspace,
        declared,
        module_source_digest,
        dep_action_digests,
        available_inputs,
        source_digest_cache,
    )?
    .input_digest)
}

/// Find the action output a declared input comes from: an exact match, or the
/// closest enclosing directory output.
///
/// A rule may declare one file inside a directory that another action emits,
/// such as the build script inside a snapshot of a package's sources. That
/// file is only on disk once the producing action's outputs are materialized,
/// and a cache hit defers materialization until some later action misses. The
/// digest therefore has to come from the producer, not from the filesystem,
/// or hashing the input fails outright on a hit and disagrees with the miss
/// path when it succeeds.
pub(super) fn resolve_available_input<'a>(
    available_inputs: &'a BTreeMap<String, AvailableInput>,
    input: &str,
) -> Option<&'a AvailableInput> {
    if let Some(available) = available_inputs.get(input) {
        return Some(available);
    }
    let mut candidate = input;
    while let Some((parent, _)) = candidate.rsplit_once('/') {
        if let Some(available) = available_inputs.get(parent) {
            return Some(available);
        }
        candidate = parent;
    }
    None
}

/// The output path whose materialization puts `input` on disk, paired with its
/// blob digest. Mirrors [`resolve_available_input`] for the staging step.
pub(super) fn enclosing_available_output<'a>(
    available_inputs: &'a BTreeMap<String, AvailableInput>,
    input: &str,
) -> Option<(&'a String, &'a AvailableInput)> {
    if let Some((path, available)) = available_inputs.get_key_value(input) {
        return Some((path, available));
    }
    let mut candidate = input;
    while let Some((parent, _)) = candidate.rsplit_once('/') {
        if let Some((path, available)) = available_inputs.get_key_value(parent) {
            return Some((path, available));
        }
        candidate = parent;
    }
    None
}

pub(super) fn generated_input_children<'a>(
    available: &'a BTreeMap<String, AvailableInput>,
    input: &str,
) -> impl Iterator<Item = (&'a String, &'a AvailableInput)> {
    let prefix = format!("{input}/");
    available
        .range(prefix.clone()..)
        .take_while(move |(path, _)| path.starts_with(&prefix))
}

pub(super) fn compose_input_fingerprint_with_available(
    workspace: &Path,
    declared: &DeclaredAction,
    _module_source_digest: Digest,
    dep_action_digests: &[(String, Digest)],
    available_inputs: &BTreeMap<String, AvailableInput>,
    source_digest_cache: Option<&SourceDigestCache>,
) -> Result<InputFingerprintManifest> {
    let mut builder = InputDigestBuilder::new(b"struo.declared_action.input.v5\0");
    push_declared_action_metadata(&mut builder, declared, workspace)?;

    let mut sorted_inputs = declared
        .inputs
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    sorted_inputs.sort_unstable();
    sorted_inputs.dedup();
    for input in &sorted_inputs {
        if let Some(available) = resolve_available_input(available_inputs, input) {
            let digest = &available.blob_digest;
            let category = if available.same_target {
                "generated-input"
            } else {
                "dependency-output"
            };
            builder.push_keyed_component(category, *input, input.as_bytes(), digest);
        } else {
            let generated_directory = generated_input_children(available_inputs, input)
                .next()
                .is_some();
            let digest = match source_digest_cache {
                Some(cache) if !generated_directory => cache.digest(workspace, input),
                _ => struo_core::digest_source_path(workspace, input),
            }
            .with_context(|| format!("hashing declared input `{input}`"))?;
            builder.push_keyed_component("source", *input, input.as_bytes(), &digest);
        }
    }

    for (label, digest) in dep_action_digests {
        if label.starts_with("same-target:") {
            let key = format!("dep:{label}");
            builder.push_keyed_component("dependency", label, key.as_bytes(), digest);
        }
    }

    Ok(builder.finish_with_fingerprint())
}

pub(super) fn push_declared_action_metadata(
    builder: &mut InputDigestBuilder,
    declared: &DeclaredAction,
    workspace: &Path,
) -> Result<()> {
    let identities = serde_json::to_vec(&(&declared.toolchain_identity, &declared.identifier))
        .context("serializing declared action identities")?;
    builder.push_bytes(&identities);
    if let Some(identity) = &declared.toolchain_identity {
        builder.record_bytes("toolchain", "identity", identity.as_bytes());
    }
    if let Some(identifier) = &declared.identifier {
        builder.record_bytes("action", "identifier", identifier.as_bytes());
    }
    if let Some(operation) = &declared.operation {
        let encoded =
            serde_json::to_vec(operation).context("serializing declared action operation")?;
        builder.push_bytes_component("command", "operation", &encoded);
    }
    let canonical_argv = declared
        .argv
        .iter()
        .map(|arg| canonical_action_value(arg, workspace))
        .collect::<Vec<_>>();
    for arg in &canonical_argv {
        builder.push_bytes(arg.as_bytes());
    }
    if !canonical_argv.is_empty() {
        let encoded =
            serde_json::to_vec(&canonical_argv).context("serializing declared action arguments")?;
        builder.record_bytes("command", "arguments", &encoded);
    }
    if let Some(stdout) = &declared.stdout {
        builder.push_keyed_component(
            "command",
            "stdout-path",
            b"stdout",
            &Digest::of_bytes(stdout.as_bytes()),
        );
    }
    if let Some(stderr) = &declared.stderr {
        builder.push_keyed_component(
            "command",
            "stderr-path",
            b"stderr",
            &Digest::of_bytes(stderr.as_bytes()),
        );
    }
    let encoded_arg_files =
        serde_json::to_vec(&declared.arg_files).context("serializing declared action arg files")?;
    builder.push_bytes(&encoded_arg_files);
    if !declared.arg_files.is_empty() {
        builder.record_bytes("command", "argument-files", &encoded_arg_files);
    }
    let canonical_env = declared
        .env
        .iter()
        .map(|(key, value)| (key, canonical_action_value(value, workspace)))
        .collect::<BTreeMap<_, _>>();
    for (key, value) in &canonical_env {
        builder.push_bytes(key.as_bytes());
        builder.push_bytes(value.as_bytes());
        builder.record_bytes("environment", format!("variable:{key}"), value.as_bytes());
    }
    if !canonical_env.is_empty() {
        let encoded =
            serde_json::to_vec(&canonical_env).context("serializing declared environment")?;
        builder.record_bytes("environment", "declared", &encoded);
    }
    for path in &declared.clean_paths {
        builder.push_bytes(b"clean_path");
        builder.push_bytes(path.as_bytes());
    }
    for path in &declared.create_dirs {
        builder.push_bytes(b"create_dir");
        builder.push_bytes(path.as_bytes());
    }
    if !declared.clean_paths.is_empty() || !declared.create_dirs.is_empty() {
        let encoded = serde_json::to_vec(&(&declared.clean_paths, &declared.create_dirs))
            .context("serializing declared path setup")?;
        builder.record_bytes("command", "path-setup", &encoded);
    }
    if let Some(cwd) = &declared.cwd {
        builder.push_bytes(b"cwd").push_bytes(cwd.as_bytes());
        builder.record_bytes("command", "working-directory", cwd.as_bytes());
    }
    Ok(())
}

pub(super) fn canonical_action_value(value: &str, workspace: &Path) -> String {
    let workspace = workspace.to_string_lossy();
    if workspace.is_empty() {
        return value.to_owned();
    }
    value.replace(workspace.as_ref(), "{{struo.execution_root}}")
}

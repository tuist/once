//! Portable filesystem actions that need no command.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{anyhow, Context};
use starlark::environment::GlobalsBuilder;
use starlark::starlark_module;
use starlark::values::none::NoneType;
use starlark::values::Value;

use super::observation::observe_host_path;
use super::{
    copy_path_outputs, file_sha256_hex, host_tree_digest_cache, parse_archive_format,
    parse_copy_path_mode, parse_prepare_path_mode, unpack_archive_entries, unpack_copy_sources,
    unpack_write_content,
};
use crate::analysis::presentation::{record_action, unpack_presentation, unpack_source_files};
use crate::analysis::store::{
    analysis_active, observe, DeclaredAction, DeclaredActionOperation, DeclaredArchiveFormat,
    DeclaredPreparePathMode, Observation,
};
use crate::analysis::values::unpack_string_list;

#[starlark_module]
pub(super) fn file_action_globals(builder: &mut GlobalsBuilder) {
    /// Declare a portable action that writes text or bytes at the
    /// workspace-relative `path`. `content` may be a string or a list
    /// of integers in `0..=255`.
    fn write_path<'v>(
        path: &str,
        content: Value<'v>,
        #[starlark(require = named)] presentation: Option<Value<'v>>,
        #[starlark(require = named)] display_name: Option<String>,
        #[starlark(require = named)] source_files: Option<Value<'v>>,
    ) -> anyhow::Result<NoneType> {
        if !analysis_active() {
            return Ok(NoneType);
        }
        let bytes = unpack_write_content(content)?;
        let action = DeclaredAction {
            presentation: unpack_presentation(presentation),
            history: None,
            resources: struo_core::ResourceRequest::default(),
            operation: Some(DeclaredActionOperation::WriteFile {
                path: path.to_string(),
                bytes,
            }),
            argv: Vec::new().into(),
            arg_files: Vec::new(),
            inputs: Vec::new().into(),
            outputs: vec![path.to_string()],
            stdout: None,
            stderr: None,
            clean_paths: Vec::new(),
            create_dirs: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            network: None,
            success_exit_codes: vec![0],
            cacheable: true,
            inherit_parent_env: false,
            depends_on_prior_actions: false,
            intermediate: false,
            toolchain_identity: None,
            identifier: Some(format!("write_path:{path}")),
            display_name: display_name
                .or_else(|| Some(format!("Write {}", path.rsplit('/').next().unwrap_or(path)))),
            source_files: unpack_source_files(source_files)?,
        };
        record_action(action);
        Ok(NoneType)
    }

    /// Declare a portable copy action. The default copies one path by value,
    /// materializing a directory symlink at the destination. `kind = "tree"`
    /// copies directory contents and preserves their symlink layout. Tree
    /// copies accept one source string or a list of source directories.
    /// `kind = "files"` copies a list of files into the `destination`
    /// directory as one action, each at its path below `strip_prefix`.
    #[allow(clippy::too_many_arguments)]
    fn copy_path<'v>(
        source: Value<'v>,
        destination: &str,
        kind: Option<String>,
        inputs: Option<Value<'v>>,
        toolchain_identity: Option<String>,
        identifier: Option<String>,
        cacheable: Option<bool>,
        #[starlark(require = named)] presentation: Option<Value<'v>>,
        #[starlark(require = named)] display_name: Option<String>,
        #[starlark(require = named)] source_files: Option<Value<'v>>,
        #[starlark(require = named)] strip_prefix: Option<String>,
    ) -> anyhow::Result<NoneType> {
        if !analysis_active() {
            return Ok(NoneType);
        }
        let mode = parse_copy_path_mode(kind.as_deref(), strip_prefix)?;
        let sources = unpack_copy_sources(source, &mode)?;
        let outputs = copy_path_outputs(&sources, destination, &mode)?;
        let mut inputs = inputs
            .map(|value| unpack_string_list(value, "inputs"))
            .transpose()?
            .unwrap_or_default();
        inputs.extend(sources.iter().cloned());
        inputs.sort();
        inputs.dedup();
        let action = DeclaredAction {
            presentation: unpack_presentation(presentation),
            history: None,
            resources: struo_core::ResourceRequest::default(),
            operation: Some(DeclaredActionOperation::CopyPath {
                sources,
                destination: destination.to_string(),
                mode,
            }),
            argv: Vec::new().into(),
            arg_files: Vec::new(),
            inputs: inputs.into(),
            outputs,
            stdout: None,
            stderr: None,
            clean_paths: Vec::new(),
            create_dirs: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            network: None,
            success_exit_codes: vec![0],
            cacheable: cacheable.unwrap_or(true),
            inherit_parent_env: false,
            depends_on_prior_actions: false,
            intermediate: false,
            toolchain_identity,
            identifier: Some(identifier.unwrap_or_else(|| format!("copy_path:{destination}"))),
            display_name: display_name.or_else(|| {
                Some(format!(
                    "Copy {}",
                    destination.rsplit('/').next().unwrap_or(destination)
                ))
            }),
            source_files: unpack_source_files(source_files)?,
        };
        record_action(action);
        Ok(NoneType)
    }

    /// Snapshot an absolute host toolchain file into a workspace-relative
    /// output. The source digest is captured during analysis and verified
    /// again on execution, so cache identity follows file content.
    fn materialize_host_file<'v>(
        source: &str,
        destination: &str,
        #[starlark(require = named)] presentation: Option<Value<'v>>,
        #[starlark(require = named)] display_name: Option<String>,
        #[starlark(require = named)] source_files: Option<Value<'v>>,
    ) -> anyhow::Result<NoneType> {
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
        // The digest is baked into the action, so a replay of this analysis
        // would hand the executor a digest for content that may have moved on.
        // Record it as the answer it is, so the replay is refused instead.
        observe(Observation::FileDigest {
            path: source.to_string(),
            sha256: source_sha256.clone(),
        });
        let source_mode = struo_host_tree::host_file_mode(&std::fs::metadata(source_path)?);
        observe(Observation::FileMode {
            path: source.to_string(),
            mode: source_mode,
        });
        let action = DeclaredAction {
            presentation: unpack_presentation(presentation),
            history: None,
            resources: struo_core::ResourceRequest::default(),
            operation: Some(DeclaredActionOperation::MaterializeHostFile {
                source: source.to_string(),
                source_sha256,
                source_mode: Some(source_mode),
                destination: destination.to_string(),
            }),
            argv: Vec::new().into(),
            arg_files: Vec::new(),
            inputs: Vec::new().into(),
            outputs: vec![destination.to_string()],
            stdout: None,
            stderr: None,
            clean_paths: Vec::new(),
            create_dirs: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            network: None,
            success_exit_codes: vec![0],
            cacheable: true,
            inherit_parent_env: false,
            depends_on_prior_actions: false,
            intermediate: false,
            toolchain_identity: None,
            identifier: Some(format!("materialize_host_file:{destination}")),
            display_name: display_name.or_else(|| {
                Some(format!(
                    "Snapshot toolchain file {}",
                    destination.rsplit('/').next().unwrap_or(destination)
                ))
            }),
            source_files: unpack_source_files(source_files)?,
        };
        record_action(action);
        Ok(NoneType)
    }

    /// Snapshot an absolute host directory into a workspace-relative output.
    /// The source tree digest is captured during analysis and verified again
    /// on execution, so cache identity follows files, modes, and symlinks.
    fn materialize_host_tree<'v>(
        source: &str,
        destination: &str,
        #[starlark(require = named)] presentation: Option<Value<'v>>,
        #[starlark(require = named)] display_name: Option<String>,
        #[starlark(require = named)] source_files: Option<Value<'v>>,
    ) -> anyhow::Result<NoneType> {
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
        observe_host_path(source_path)?;
        // The trees this snapshots are package sources in a dependency
        // cache: large, numerous, and immutable once unpacked. Hashing every
        // one of them on every graph load is the bulk of what an unchanged
        // build spends its time on, so consult the stat-described cache
        // first and only read bytes when the tree looks different.
        let source_sha256 = host_tree_digest_cache()
            .digest("host-tree", source_path, || {
                struo_host_tree::host_tree_sha256_hex(source_path)
            })
            .with_context(|| format!("hashing host directory `{source}`"))?;
        observe(Observation::TreeDigest {
            path: source.to_string(),
            sha256: source_sha256.clone(),
        });
        let action = DeclaredAction {
            presentation: unpack_presentation(presentation),
            history: None,
            resources: struo_core::ResourceRequest::default(),
            operation: Some(DeclaredActionOperation::MaterializeHostTree {
                source: source.to_string(),
                source_sha256,
                destination: destination.to_string(),
            }),
            argv: Vec::new().into(),
            arg_files: Vec::new(),
            inputs: Vec::new().into(),
            outputs: vec![destination.to_string()],
            stdout: None,
            stderr: None,
            clean_paths: Vec::new(),
            create_dirs: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            network: None,
            success_exit_codes: vec![0],
            cacheable: true,
            inherit_parent_env: false,
            depends_on_prior_actions: false,
            intermediate: false,
            toolchain_identity: None,
            identifier: Some(format!("materialize_host_tree:{destination}")),
            display_name: display_name.or_else(|| {
                Some(format!(
                    "Snapshot toolchain tree {}",
                    destination.rsplit('/').next().unwrap_or(destination)
                ))
            }),
            source_files: unpack_source_files(source_files)?,
        };
        record_action(action);
        Ok(NoneType)
    }

    /// Link one workspace path to another without copying or caching
    /// the linked contents. Downstream actions still hash the linked
    /// tree when they declare it as an input.
    fn link_path<'v>(
        source: &str,
        destination: &str,
        identifier: Option<String>,
        #[starlark(require = named)] presentation: Option<Value<'v>>,
        #[starlark(require = named)] display_name: Option<String>,
        #[starlark(require = named)] source_files: Option<Value<'v>>,
    ) -> anyhow::Result<NoneType> {
        if !analysis_active() {
            return Ok(NoneType);
        }
        if source == destination {
            return Err(anyhow!("link_path source and destination must differ"));
        }
        let action = DeclaredAction {
            presentation: unpack_presentation(presentation),
            history: None,
            resources: struo_core::ResourceRequest::default(),
            operation: Some(DeclaredActionOperation::LinkPath {
                source: source.to_string(),
                destination: destination.to_string(),
            }),
            argv: Vec::new().into(),
            arg_files: Vec::new(),
            inputs: vec![source.to_string()].into(),
            outputs: vec![destination.to_string()],
            stdout: None,
            stderr: None,
            clean_paths: Vec::new(),
            create_dirs: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            network: None,
            success_exit_codes: vec![0],
            cacheable: false,
            inherit_parent_env: false,
            depends_on_prior_actions: false,
            intermediate: false,
            toolchain_identity: None,
            identifier: Some(
                identifier.unwrap_or_else(|| format!("link_path:{source}:{destination}")),
            ),
            display_name: display_name.or_else(|| {
                Some(format!(
                    "Link {}",
                    destination.rsplit('/').next().unwrap_or(destination)
                ))
            }),
            source_files: unpack_source_files(source_files)?,
        };
        record_action(action);
        Ok(NoneType)
    }

    /// Declare an uncached portable path preparation action. `kind`
    /// must be `"remove"` or `"directory"`.
    fn prepare_path<'v>(
        path: &str,
        kind: &str,
        identifier: Option<String>,
        #[starlark(require = named)] presentation: Option<Value<'v>>,
        #[starlark(require = named)] display_name: Option<String>,
        #[starlark(require = named)] source_files: Option<Value<'v>>,
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
            presentation: unpack_presentation(presentation),
            history: None,
            resources: struo_core::ResourceRequest::default(),
            operation: Some(DeclaredActionOperation::PreparePath {
                path: path.to_string(),
                mode,
            }),
            argv: Vec::new().into(),
            arg_files: Vec::new(),
            inputs: Vec::new().into(),
            outputs,
            stdout: None,
            stderr: None,
            clean_paths: Vec::new(),
            create_dirs: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            network: None,
            success_exit_codes: vec![0],
            cacheable: false,
            inherit_parent_env: false,
            depends_on_prior_actions: false,
            intermediate: false,
            toolchain_identity: None,
            identifier: Some(identifier.unwrap_or_else(|| format!("prepare_path:{kind}:{path}"))),
            display_name: display_name.or_else(|| {
                Some(format!(
                    "{} {}",
                    if kind == "remove" {
                        "Remove"
                    } else {
                        "Create directory"
                    },
                    path.rsplit('/').next().unwrap_or(path)
                ))
            }),
            source_files: unpack_source_files(source_files)?,
        };
        record_action(action);
        Ok(NoneType)
    }

    /// Declare a portable action that writes a deterministic digest
    /// listing for a workspace tree. Missing roots produce an empty
    /// file. `include_suffixes` filters files by path suffix when set.
    #[allow(clippy::too_many_arguments)]
    fn write_tree_digest<'v>(
        root: &str,
        output: &str,
        include_suffixes: Option<Value<'v>>,
        inputs: Option<Value<'v>>,
        identifier: Option<String>,
        cacheable: Option<bool>,
        #[starlark(require = named)] presentation: Option<Value<'v>>,
        #[starlark(require = named)] display_name: Option<String>,
        #[starlark(require = named)] source_files: Option<Value<'v>>,
    ) -> anyhow::Result<NoneType> {
        if !analysis_active() {
            return Ok(NoneType);
        }
        let include_suffixes = include_suffixes
            .map(|value| unpack_string_list(value, "include_suffixes"))
            .transpose()?
            .unwrap_or_default();
        let mut inputs = inputs
            .map(|value| unpack_string_list(value, "inputs"))
            .transpose()?
            .unwrap_or_default();
        inputs.push(root.to_string());
        inputs.sort();
        inputs.dedup();
        let action = DeclaredAction {
            presentation: unpack_presentation(presentation),
            history: None,
            resources: struo_core::ResourceRequest::default(),
            operation: Some(DeclaredActionOperation::WriteTreeDigest {
                root: root.to_string(),
                output: output.to_string(),
                include_suffixes,
            }),
            argv: Vec::new().into(),
            arg_files: Vec::new(),
            inputs: inputs.into(),
            outputs: vec![output.to_string()],
            stdout: None,
            stderr: None,
            clean_paths: Vec::new(),
            create_dirs: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            network: None,
            success_exit_codes: vec![0],
            cacheable: cacheable.unwrap_or(true),
            inherit_parent_env: false,
            depends_on_prior_actions: false,
            intermediate: false,
            toolchain_identity: None,
            identifier: Some(identifier.unwrap_or_else(|| format!("write_tree_digest:{output}"))),
            display_name: display_name.or_else(|| {
                Some(format!(
                    "Fingerprint {}",
                    root.rsplit('/').next().unwrap_or(root)
                ))
            }),
            source_files: unpack_source_files(source_files)?,
        };
        record_action(action);
        Ok(NoneType)
    }

    /// Declare a portable deterministic archive action. Each entry is
    /// a dict with `kind`, `path`, optional `source`, and explicit
    /// metadata. The initial `tar` format supports files, directories,
    /// and recursively expanded trees.
    #[allow(clippy::too_many_arguments)]
    fn write_archive<'v>(
        entries: Value<'v>,
        output: &str,
        sha256_output: Option<String>,
        format: Option<String>,
        inputs: Option<Value<'v>>,
        identifier: Option<String>,
        cacheable: Option<bool>,
        uncompressed_sha256_output: Option<String>,
        #[starlark(require = named)] presentation: Option<Value<'v>>,
        #[starlark(require = named)] display_name: Option<String>,
        #[starlark(require = named)] source_files: Option<Value<'v>>,
    ) -> anyhow::Result<NoneType> {
        if !analysis_active() {
            return Ok(NoneType);
        }
        let entries = unpack_archive_entries(entries)?;
        let format = parse_archive_format(format.as_deref())?;
        if uncompressed_sha256_output.is_some() && format != DeclaredArchiveFormat::TarGz {
            anyhow::bail!(
                "`write_archive.uncompressed_sha256_output` requires `format = \"tar.gz\"`"
            );
        }
        let mut inputs = inputs
            .map(|value| unpack_string_list(value, "inputs"))
            .transpose()?
            .unwrap_or_default();
        inputs.extend(entries.iter().filter_map(|entry| entry.source.clone()));
        inputs.sort();
        inputs.dedup();
        let mut outputs = vec![output.to_string()];
        for path in [&sha256_output, &uncompressed_sha256_output]
            .into_iter()
            .flatten()
        {
            outputs.push(path.clone());
        }
        let action = DeclaredAction {
            presentation: unpack_presentation(presentation),
            history: None,
            resources: struo_core::ResourceRequest::default(),
            operation: Some(DeclaredActionOperation::WriteArchive {
                entries,
                output: output.to_string(),
                sha256_output,
                uncompressed_sha256_output,
                format,
            }),
            argv: Vec::new().into(),
            arg_files: Vec::new(),
            inputs: inputs.into(),
            outputs,
            stdout: None,
            stderr: None,
            clean_paths: Vec::new(),
            create_dirs: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            network: None,
            success_exit_codes: vec![0],
            cacheable: cacheable.unwrap_or(true),
            inherit_parent_env: false,
            depends_on_prior_actions: false,
            intermediate: false,
            toolchain_identity: None,
            identifier: Some(identifier.unwrap_or_else(|| format!("write_archive:{output}"))),
            display_name: display_name.or_else(|| {
                Some(format!(
                    "Archive {}",
                    output.rsplit('/').next().unwrap_or(output)
                ))
            }),
            source_files: unpack_source_files(source_files)?,
        };
        record_action(action);
        Ok(NoneType)
    }

    /// Download a checksum-pinned ZIP archive and expand it into a declared
    /// directory output. `authorization_env` is the name of an environment
    /// variable read only when the action executes; its value is never stored
    /// in the action declaration, action digest, or evidence.
    #[allow(clippy::too_many_arguments)]
    fn download_and_extract<'v>(
        url: &str,
        sha256: &str,
        destination: &str,
        authorization_env: Option<String>,
        identifier: Option<String>,
        cacheable: Option<bool>,
        #[starlark(require = named)] presentation: Option<Value<'v>>,
        #[starlark(require = named)] display_name: Option<String>,
        #[starlark(require = named)] source_files: Option<Value<'v>>,
    ) -> anyhow::Result<NoneType> {
        if !analysis_active() {
            return Ok(NoneType);
        }
        if url.trim().is_empty() {
            return Err(anyhow!("download_and_extract url must not be empty"));
        }
        if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(anyhow!(
                "download_and_extract sha256 must be exactly 64 hexadecimal characters"
            ));
        }
        if authorization_env
            .as_deref()
            .is_some_and(|name| name.trim().is_empty())
        {
            return Err(anyhow!(
                "download_and_extract authorization_env must not be empty"
            ));
        }
        let action = DeclaredAction {
            presentation: unpack_presentation(presentation),
            history: None,
            resources: struo_core::ResourceRequest::default(),
            operation: Some(DeclaredActionOperation::DownloadAndExtract {
                url: url.to_string(),
                sha256: sha256.to_string(),
                destination: destination.to_string(),
                authorization_env,
            }),
            argv: Vec::new().into(),
            arg_files: Vec::new(),
            inputs: Vec::new().into(),
            outputs: vec![destination.to_string()],
            stdout: None,
            stderr: None,
            clean_paths: Vec::new(),
            create_dirs: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            network: None,
            success_exit_codes: vec![0],
            cacheable: cacheable.unwrap_or(true),
            inherit_parent_env: false,
            depends_on_prior_actions: false,
            intermediate: false,
            toolchain_identity: None,
            identifier: Some(
                identifier.unwrap_or_else(|| format!("download_and_extract:{destination}")),
            ),
            display_name: display_name.or_else(|| {
                Some(format!(
                    "Fetch and extract {}",
                    destination.rsplit('/').next().unwrap_or(destination)
                ))
            }),
            source_files: unpack_source_files(source_files)?,
        };
        record_action(action);
        Ok(NoneType)
    }
}

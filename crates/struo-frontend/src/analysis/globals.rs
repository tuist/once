use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{anyhow, Context, Result};
use sha2::Digest as ShaDigest;
use starlark::environment::{Globals, GlobalsBuilder};
use starlark::eval::Evaluator;
use starlark::values::dict::DictRef;
use starlark::values::list::ListRef;
use starlark::values::Value;

use super::store::{
    observe, with_store, DeclaredArchiveEntry, DeclaredArchiveEntryKind, DeclaredArchiveFormat,
    DeclaredArgFile, DeclaredArgFileFormat, DeclaredCopyPathMode, DeclaredPreparePathMode,
    HostCache, Observation,
};
use super::values::{json_to_value, unpack_byte_list, unpack_string_list};

mod actions;
mod codecs;
mod expansion;
mod file_actions;
mod host;
mod observation;
mod paths;

pub use observation::{expansion_could_differ, UnchangedWorkspace};
pub(super) use observation::{first_stale_observation, observations_hold};
use observation::{observe_absent_link_target, observe_host_path, observed_which};

pub use expansion::expand_globs_with_excludes;
#[cfg(test)]
pub(super) use expansion::{expand_globs, walk_package_files};

const CMD_ARGS_MARKER: &str = "_struo_cmd_args";
const EXECUTION_ROOT_MARKER: &str = "{{struo.execution_root}}";
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
    GlobalsBuilder::standard()
        .with(host::host_globals)
        .with(paths::path_globals)
        .with(actions::action_globals)
        .with(file_actions::file_action_globals)
        .with(codecs::codec_globals)
        .build()
}

pub(super) fn selected_execution_context<'v>(eval: &Evaluator<'v, '_, '_>) -> Value<'v> {
    with_store(|store| {
        store
            .and_then(|store| store.execution.as_ref())
            .map_or(Value::new_none(), |execution| {
                json_to_value(
                    eval,
                    &serde_json::to_value(execution).expect("execution is JSON"),
                )
            })
    })
}

fn unpack_resources(value: Option<Value<'_>>) -> Result<struo_core::ResourceRequest> {
    let mut request = struo_core::ResourceRequest::default();
    let Some(value) = value.filter(|value| !value.is_none()) else {
        return Ok(request);
    };
    let dict = DictRef::from_value(value)
        .ok_or_else(|| anyhow!("run_action resources must be a dictionary or None"))?;
    for (key, value) in dict.iter() {
        let key = key
            .unpack_str()
            .ok_or_else(|| anyhow!("run_action resources keys must be strings"))?;
        match key {
            "cpu_slots" => {
                request.cpu_slots = usize::try_from(unpack_resource_count(value, key)?)
                    .context("run_action resources.cpu_slots is too large")?;
                if request.cpu_slots == 0 {
                    anyhow::bail!("run_action resources.cpu_slots must be positive");
                }
            }
            "memory_bytes" => request.memory_bytes = unpack_resource_count(value, key)?,
            "slots" => {
                let slots = DictRef::from_value(value)
                    .ok_or_else(|| anyhow!("run_action resources.slots must be a dictionary"))?;
                for (name, count) in slots.iter() {
                    let name = name.unpack_str().ok_or_else(|| {
                        anyhow!("run_action resources.slots keys must be strings")
                    })?;
                    let count = usize::try_from(unpack_resource_count(count, "slots")?)
                        .context("run_action resources.slots count is too large")?;
                    request = request.with_slot(name, count);
                }
            }
            _ => anyhow::bail!(
                "unknown run_action resources key `{key}`; expected cpu_slots, memory_bytes, or slots"
            ),
        }
    }
    Ok(request)
}

fn unpack_resource_count(value: Value<'_>, field: &str) -> Result<u64> {
    value
        .to_json_value()
        .ok()
        .and_then(|value| value.as_u64())
        .ok_or_else(|| {
            anyhow!("run_action resources.{field} must be a nonnegative integer within u64 range")
        })
}

fn git_checkouts(repositories: &[super::repository::Checkout], cache_root: &str) -> Result<()> {
    if repositories.is_empty() {
        return Ok(());
    }
    let (workspace, resources) = with_store(|store| {
        let store = store.unwrap();
        (
            store.workspace_root.clone(),
            Arc::clone(&store.host_cache.resources),
        )
    });
    for (index, repository) in repositories.iter().enumerate() {
        let path = &repository.path;
        if !path.is_absolute()
            || !path.starts_with(&workspace)
            || path.components().any(|part| part == Component::ParentDir)
        {
            anyhow::bail!("host_git_checkout path must stay inside the workspace");
        }
        if repositories[..index]
            .iter()
            .any(|other| path.starts_with(&other.path) || other.path.starts_with(path))
        {
            anyhow::bail!("host_git_checkouts requires distinct, non-overlapping checkout paths");
        }
    }
    let git = observed_which("git", "host_git_checkout")?
        .context("host_git_checkout requires git on PATH")?;
    super::repository::checkout_many(&git, repositories, Path::new(cache_root), &resources)?;
    for repository in repositories {
        let head = repository.path.join(".git/HEAD");
        observe(Observation::FileDigest {
            path: head.to_string_lossy().into_owned(),
            sha256: file_sha256_hex(&head)?,
        });
    }
    Ok(())
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

fn parse_copy_path_mode(
    kind: Option<&str>,
    strip_prefix: Option<String>,
) -> Result<DeclaredCopyPathMode> {
    match (kind.unwrap_or("file"), strip_prefix) {
        ("files", Some(strip_prefix)) => Ok(DeclaredCopyPathMode::Files { strip_prefix }),
        ("files", None) => Err(anyhow!(
            "`copy_path` with kind `files` requires `strip_prefix`"
        )),
        (_, Some(_)) => Err(anyhow!(
            "`strip_prefix` applies only to `copy_path` with kind `files`"
        )),
        ("file", None) => Ok(DeclaredCopyPathMode::File),
        ("tree", None) => Ok(DeclaredCopyPathMode::Tree),
        (other, None) => Err(anyhow!(
            "expected `kind` to be `file`, `tree`, or `files`, got `{other}`"
        )),
    }
}

fn copy_path_outputs(
    sources: &[String],
    destination: &str,
    mode: &DeclaredCopyPathMode,
) -> Result<Vec<String>> {
    let DeclaredCopyPathMode::Files { strip_prefix } = mode else {
        return Ok(vec![destination.to_string()]);
    };
    let sources = sources
        .iter()
        .map(|source| struo_core::WorkspacePath::try_from(source.as_str()))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let destinations = struo_core::copy_files_destinations(
        &sources,
        &struo_core::WorkspacePath::try_from(destination)?,
        &struo_core::WorkspacePath::try_from(strip_prefix.as_str())?,
    )
    .map_err(|reason| anyhow!("`copy_path`: {reason}"))?;
    Ok(destinations
        .into_iter()
        .map(|path| path.as_str().to_string())
        .collect())
}

fn unpack_copy_sources(source: Value<'_>, mode: &DeclaredCopyPathMode) -> Result<Vec<String>> {
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
        DeclaredCopyPathMode::Files { .. } if sources.is_empty() => Err(anyhow!(
            "`copy_path` with kind `files` requires at least one source"
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
        "tar.gz" => Ok(DeclaredArchiveFormat::TarGz),
        "zip" => Ok(DeclaredArchiveFormat::Zip),
        other => Err(anyhow!(
            "expected `write_archive.format` to be `tar`, `tar.gz` or `zip`, got `{other}`"
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
        "symlink" => DeclaredArchiveEntryKind::Symlink,
        other => {
            return Err(anyhow!(
                "expected `{field}.kind` to be `file`, `directory`, `tree`, or `symlink`, got `{other}`"
            ));
        }
    };
    let source = optional_string_field(&dict, "source")?;
    let target = optional_string_field(&dict, "target")?;
    if (kind == DeclaredArchiveEntryKind::Symlink) != target.is_some() {
        return Err(anyhow!(
            "expected `{field}.target` only on, and always on, a symlink entry"
        ));
    }
    match kind {
        DeclaredArchiveEntryKind::File | DeclaredArchiveEntryKind::Tree if source.is_none() => {
            return Err(anyhow!("expected `{field}` to contain `source`"));
        }
        DeclaredArchiveEntryKind::Directory | DeclaredArchiveEntryKind::Symlink
            if source.is_some() =>
        {
            return Err(anyhow!(
                "expected `{field}.source` to be omitted for a directory or symlink"
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
        target,
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

fn validate_network(value: Option<&str>) -> Result<()> {
    match value {
        None | Some("unrestricted" | "deny") => Ok(()),
        Some(other) => Err(anyhow!(
            "expected `network` to be `unrestricted` or `deny`, got `{other}`"
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

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(sha2::Sha256::digest(bytes))
}

#[must_use]
pub fn inherited_environment_sha256() -> String {
    let mut hasher = sha2::Sha256::new();
    for (name, value) in std::env::vars_os().collect::<BTreeMap<_, _>>() {
        for part in [name, value] {
            let bytes = part.as_encoded_bytes();
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
        }
    }
    hex::encode(hasher.finalize())
}

pub(super) fn file_sha256_hex(path: &Path) -> Result<String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)?;
    let mut hasher = sha2::Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match file.read(&mut buf)? {
            0 => return Ok(hex::encode(hasher.finalize())),
            read => hasher.update(&buf[..read]),
        }
    }
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

static HOST_TREE_DIGEST_CACHES: OnceLock<
    Mutex<BTreeMap<PathBuf, Arc<struo_host_tree::TreeDigestCache>>>,
> = OnceLock::new();

/// Write every tree digest learned during this process back to disk.
///
/// The caches hang off a process-wide map that nothing drops, so the run has
/// to hand back what it learned before it ends.
pub fn flush_host_tree_digest_caches() {
    let Some(caches) = HOST_TREE_DIGEST_CACHES.get() else {
        return;
    };
    let caches = caches
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for cache in caches.values() {
        cache.save();
    }
}

/// The trees `materialize_host_tree` snapshots, such as unpacked dependency
/// packages, live outside any one workspace and are shared by every checkout
/// and worktree on the host. Their digests are remembered in the user cache so
/// a fresh checkout does not re-read them; each entry is still checked against
/// the tree's stat description before it is reused.
fn host_tree_digest_cache() -> Arc<struo_host_tree::TreeDigestCache> {
    let path = struo_core::Xdg::from_env()
        .cache_home
        .join("struo")
        .join("host-tree-digests");
    let caches = HOST_TREE_DIGEST_CACHES.get_or_init(|| Mutex::new(BTreeMap::new()));
    let mut caches = caches
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Arc::clone(
        caches.entry(path).or_insert_with_key(|path| {
            Arc::new(struo_host_tree::TreeDigestCache::open(path.clone()))
        }),
    )
}

fn cached_host_tree_digest(
    host_cache: &HostCache,
    workspace: &Path,
    source: &Path,
) -> Result<String> {
    host_cache.host_tree_digest(source, || {
        let cache = struo_host_tree::TreeDigestCache::open(
            workspace
                .join(".struo/host-tree-identities")
                .join(sha256_hex(source.to_string_lossy().as_bytes())),
        );
        Ok(cache.digest("host-tree", source, || {
            struo_host_tree::host_tree_sha256_hex(source)
        })?)
    })
}

#[cfg(test)]
mod observation_completeness_tests {
    use super::globals_for_prelude;

    /// Globals that cannot tell an implementation anything about the host.
    ///
    /// Either they only declare work for later, or their result follows from
    /// their arguments. Nothing about them needs recording, because replaying
    /// the same call with the same arguments cannot produce a different answer.
    const PURE: &[&str] = &[
        "action_history_key",
        "analysis_warning",
        "cmd_args",
        "content_sha256",
        "copy_path",
        "declare_output",
        "download_and_extract",
        "execution_context",
        "execution_path",
        "expand_actions",
        "json_decode",
        "link_path",
        "plist_decode",
        "plist_encode",
        "prepare_path",
        "run_action",
        "toml_decode",
        "workspace_root",
        "write_archive",
        "write_path",
        "write_tree_digest",
    ];

    /// Globals whose answer comes from the host and is written into the
    /// observation ledger, so a replay can ask again and compare.
    const RECORDED: &[&str] = &[
        "host_arch",
        "host_cached_command",
        "host_command",
        "host_git_checkout",
        "host_git_checkouts",
        "host_env",
        "host_environment_sha256",
        "host_file_cms_content",
        "host_file_contains",
        "host_file_exists",
        "host_file_read",
        "host_file_sha256",
        "host_file_size",
        "host_os",
        "host_symlink_target",
        "host_path_exists",
        "host_path_is_within",
        "host_read_dir",
        "host_tree_sha256",
        "host_which",
        "host_which_optional",
        "glob",
        "walk_files",
        "walk_symlinks",
        "walk_workspace_files",
        "materialize_host_file",
        "materialize_host_tree",
    ];

    /// A global that is neither pure nor recorded is a way for the host to
    /// change without a stored analysis noticing, which would make a replay
    /// hand back actions built from state that has since moved. Adding one
    /// fails here until its author has said which it is.
    #[test]
    fn every_global_is_classified() {
        let globals = globals_for_prelude();
        let standard = starlark::environment::Globals::standard();
        let standard_names = standard
            .names()
            .map(|name| name.as_str().to_string())
            .collect::<std::collections::BTreeSet<_>>();
        let unclassified = globals
            .names()
            .map(|name| name.as_str().to_string())
            .filter(|name| !standard_names.contains(name))
            .filter(|name| !PURE.contains(&name.as_str()) && !RECORDED.contains(&name.as_str()))
            .collect::<Vec<_>>();

        assert!(
            unclassified.is_empty(),
            "these globals are neither listed as pure nor as recorded: {unclassified:?}. \
             A global that reads host state must write an Observation, or call \
             observe_unrecordable so analyses that use it are never stored."
        );
    }

    /// The other direction: a name listed here that no longer exists means the
    /// lists have drifted from the globals and are no longer evidence of
    /// anything.
    #[test]
    fn no_classification_names_a_global_that_is_gone() {
        let globals = globals_for_prelude();
        let names = globals
            .names()
            .map(|name| name.as_str().to_string())
            .collect::<std::collections::BTreeSet<_>>();
        let missing = PURE
            .iter()
            .chain(RECORDED.iter())
            .filter(|name| !names.contains(**name))
            .collect::<Vec<_>>();

        assert!(
            missing.is_empty(),
            "classified globals that do not exist: {missing:?}"
        );
    }
}

#[cfg(test)]
mod resource_tests {
    use super::super::store::DeclaredAction;
    use super::*;
    use starlark::environment::Module;
    use starlark::syntax::{AstModule, Dialect};

    fn analyze(source: &str) -> Result<Vec<DeclaredAction>> {
        let workspace = tempfile::tempdir()?;
        let store = super::super::AnalysisStore::new(
            workspace.path().to_path_buf(),
            "pkg".to_string(),
            ".struo/out/pkg".to_string(),
        );
        let (store, result) = super::super::with_active_store(store, || {
            Module::with_temp_heap(|module| -> Result<()> {
                let ast =
                    AstModule::parse("resources.star", source.to_string(), &Dialect::Standard)
                        .map_err(|error| anyhow!("{error:?}"))?;
                Evaluator::new(&module)
                    .eval_module(ast, &globals_for_prelude())
                    .map_err(|error| anyhow!("{error:?}"))?;
                Ok(())
            })
        });
        result?;
        Ok(store.actions)
    }

    #[test]
    fn run_action_resources_survive_analysis_and_legacy_serialization() {
        let actions = analyze(
            r#"
run_action(["tool"], resources = {"cpu_slots": 2, "memory_bytes": 4 * 1024 * 1024 * 1024, "slots": {"device": 1, "unused": 0}})
run_action(["tool"])
run_action(["tool"], resources = {})
run_action(["tool"], resources = None)
run_action(["tool"], resources = {"cpu_slots": 1, "memory_bytes": 0, "slots": {"unused": 0}})
run_action(["tool"], resources = {"memory_bytes": 18446744073709551615})
"#,
        )
        .unwrap();
        let expected =
            struo_core::ResourceRequest::new(2, 4 * 1024 * 1024 * 1024).with_slot("device", 1);
        assert_eq!(actions[0].resources, expected);
        let json = serde_json::to_value(&actions[0]).unwrap();
        let decoded: DeclaredAction = serde_json::from_value(json).unwrap();
        assert_eq!(decoded.resources, expected);
        let legacy_json = serde_json::to_value(&actions[1]).unwrap();
        assert!(legacy_json.get("resources").is_none());
        for action in &actions[1..5] {
            assert_eq!(action.resources, struo_core::ResourceRequest::default());
            assert_eq!(serde_json::to_value(action).unwrap(), legacy_json);
        }
        let legacy: DeclaredAction = serde_json::from_value(legacy_json).unwrap();
        assert_eq!(legacy.resources, struo_core::ResourceRequest::default());
        assert_eq!(actions[5].resources.memory_bytes, u64::MAX);
    }

    #[test]
    fn run_action_resources_reject_invalid_shapes_and_counts() {
        for resources in [
            "[]",
            "1",
            "{1: 2}",
            "{\"unknown\": 1}",
            "{\"cpu_slots\": 0}",
            "{\"cpu_slots\": -1}",
            "{\"cpu_slots\": True}",
            "{\"cpu_slots\": 1.0}",
            "{\"memory_bytes\": -1}",
            "{\"memory_bytes\": False}",
            "{\"memory_bytes\": 1.0}",
            "{\"memory_bytes\": \"1024\"}",
            "{\"memory_bytes\": 18446744073709551616}",
            "{\"slots\": []}",
            "{\"slots\": {1: 1}}",
            "{\"slots\": {\"device\": -1}}",
            "{\"slots\": {\"device\": True}}",
            "{\"slots\": {\"device\": 1.0}}",
        ] {
            let error =
                analyze(&format!("run_action([\"tool\"], resources = {resources})")).unwrap_err();
            assert!(
                error.to_string().contains("resources"),
                "{resources}: {error:#}"
            );
        }
    }
}

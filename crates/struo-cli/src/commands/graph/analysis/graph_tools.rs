//! Resolving the executables of the tools graph targets declare, with a
//! workspace-local cache keyed by the tool environment.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use struo_cas::Digest;
use struo_frontend::analysis::CachedToolCommand;
use struo_frontend::GraphTarget;

use super::actions;

const GRAPH_TOOL_CACHE_SCHEMA: &str = "struo.graph-tool-paths.v5";
pub(super) const GRAPH_TOOL_ENVIRONMENT_KEYS: &[&str] = &[
    "DEVELOPER_DIR",
    "ELIXIR_ERL_OPTIONS",
    "ERL_AFLAGS",
    "ERL_FLAGS",
    "GEM_HOME",
    "GEM_PATH",
    "GOENV",
    "GOFLAGS",
    "GOROOT",
    "HOME",
    "JAVA_HOME",
    "JAVA_TOOL_OPTIONS",
    "JDK_JAVA_OPTIONS",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "MISE_CACHE_DIR",
    "MISE_CONFIG_DIR",
    "MISE_DATA_DIR",
    "MISE_ENABLE_TOOLS",
    "MISE_ENV",
    "MISE_PROFILE",
    "MIX_ENV",
    "NODE_OPTIONS",
    "PATH",
    "PYTHONHOME",
    "PYTHONPATH",
    "PYTHONWARNINGS",
    "RUBYOPT",
    "RUSTFLAGS",
    "RUSTUP_TOOLCHAIN",
    "SDKROOT",
    "USERPROFILE",
    "ZIG_GLOBAL_CACHE_DIR",
    "_JAVA_OPTIONS",
];

/// Resolve the executables declared by the graph's tools to concrete
/// paths through the workspace's mise toolchain.
///
/// Resolution is scoped and best-effort so declaring a tool never makes a
/// build worse off than the host toolchain would:
///
/// * Workspaces without `mise.toml` resolve every executable from the
///   host `PATH`, so an empty map is returned and `host_which` keeps
///   walking `PATH` (and verifying existence) as before.
/// * A declared tool the workspace does not actually pin (for example a
///   rust target in a node-only workspace) is not managed by mise. Its
///   preparation and resolution failures are logged and the executable
///   resolves from the host `PATH`, skipping mise shims, so the target
///   falls back to the host toolchain instead of aborting the whole
///   session.
pub(super) async fn resolve_graph_tools(
    workspace: &Path,
    graph: &[GraphTarget],
) -> Result<ResolvedGraphTools> {
    if !struo_core::workspace_has_mise_config(workspace) {
        return Ok(ResolvedGraphTools::default());
    }

    let tool_names = graph
        .iter()
        .flat_map(|target| target.tools.iter().map(|tool| tool.name.clone()))
        .collect::<BTreeSet<_>>();
    let executable_names = graph
        .iter()
        .flat_map(|target| {
            target
                .tools
                .iter()
                .flat_map(|tool| tool.executables.iter().cloned())
        })
        .collect::<BTreeSet<_>>();
    let tool_names = tool_names.into_iter().collect::<Vec<_>>();
    let executable_names = executable_names.into_iter().collect::<Vec<_>>();
    let fingerprint = graph_tool_cache_fingerprint(workspace, &tool_names, &executable_names)?;
    if let Some(cached) = read_graph_tool_cache(workspace, fingerprint) {
        tracing::debug!(
            tools = cached.paths.len(),
            commands = cached.commands.len(),
            "reused validated graph tool paths"
        );
        return Ok(ResolvedGraphTools {
            paths: cached.paths,
            fingerprint: Some(fingerprint),
            commands: cached.commands,
        });
    }

    let tool_names = Arc::<[String]>::from(tool_names);
    let (mut paths, mut failures) =
        resolve_graph_tool_executables(workspace, &tool_names, &executable_names).await?;
    if !failures.is_empty() {
        let tool_name_refs = tool_names.iter().map(String::as_str).collect::<Vec<_>>();
        if let Err(error) = struo_core::workspace_prepare_tools(workspace, &tool_name_refs).await {
            tracing::debug!(
                %error,
                "preparing graph tools through mise failed; falling back to the host toolchain for tools it could not install"
            );
        }
        // mise installs every tool it can even when another one fails. Without
        // resolving again, whether an installable tool resolves through mise or
        // the host would depend on whether an earlier or concurrent invocation
        // happened to install it first, and the resulting PATH changes the
        // cache key of every action that uses the tool.
        (paths, failures) =
            resolve_graph_tool_executables(workspace, &tool_names, &executable_names).await?;
    }
    let all_managed = failures.is_empty();
    for (executable, error) in failures {
        if let Some(path) = actions::host_executable_path(&executable) {
            paths.insert(executable.clone(), path.to_string_lossy().into_owned());
        }
        tracing::trace!(
            executable,
            %error,
            "graph tool executable is not ready"
        );
    }
    // Host fallbacks must not be cached: a later run would reuse them and
    // skip mise resolution until the configuration changes. Leaving the
    // session without a fingerprint also stops `persist_graph_tool_cache`
    // from writing them after the build.
    let cacheable = all_managed && paths.len() == executable_names.len();
    if cacheable {
        if let Err(error) = write_graph_tool_cache(workspace, fingerprint, &paths, &[]) {
            tracing::debug!(%error, "failed to persist graph tool paths");
        }
    }
    Ok(ResolvedGraphTools {
        paths,
        fingerprint: cacheable.then_some(fingerprint),
        commands: Vec::new(),
    })
}

/// Install the graph's tools before several processes analyse it at once.
///
/// `mise install` installs what it can even when another requested tool
/// fails, so a process that resolved an executable from the host before a
/// sibling's install finished would analyse the same actions as that sibling
/// into different digests that write the same outputs.
pub(crate) async fn prepare_graph_tools(workspace: &Path, graph: &[GraphTarget]) -> Result<()> {
    resolve_graph_tools(workspace, graph).await.map(drop)
}

async fn resolve_graph_tool_executables(
    workspace: &Path,
    tool_names: &Arc<[String]>,
    executable_names: &[String],
) -> Result<(
    BTreeMap<String, String>,
    Vec<(String, struo_core::ToolEnvError)>,
)> {
    let mut tasks = tokio::task::JoinSet::new();
    for executable in executable_names {
        let workspace = workspace.to_path_buf();
        let executable = executable.clone();
        let tool_names = Arc::clone(tool_names);
        tasks.spawn(async move {
            let tool_name_refs = tool_names.iter().map(String::as_str).collect::<Vec<_>>();
            let path =
                struo_core::workspace_executable(&workspace, &executable, &tool_name_refs).await;
            (executable, path)
        });
    }

    let mut paths = BTreeMap::new();
    let mut failures = Vec::new();
    while let Some(result) = tasks.join_next().await {
        let (executable, path) = result.context("joining graph tool resolution")?;
        match path {
            Ok(path) => {
                paths.insert(executable, path);
            }
            Err(error) => {
                failures.push((executable, error));
            }
        }
    }
    Ok((paths, failures))
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct GraphToolCache {
    pub(super) schema: String,
    pub(super) fingerprint: String,
    pub(super) paths_fingerprint: String,
    pub(super) paths: BTreeMap<String, String>,
    pub(super) commands: Vec<CachedToolCommand>,
}

#[derive(Debug, Default)]
pub(super) struct ResolvedGraphTools {
    pub(super) paths: BTreeMap<String, String>,
    pub(super) fingerprint: Option<Digest>,
    pub(super) commands: Vec<CachedToolCommand>,
}

pub(super) fn graph_tool_cache_fingerprint(
    workspace: &Path,
    tool_names: &[String],
    executable_names: &[String],
) -> Result<Digest> {
    let mut bytes = Vec::new();
    push_fingerprint_part(&mut bytes, GRAPH_TOOL_CACHE_SCHEMA.as_bytes());
    push_fingerprint_part(&mut bytes, struo_core::MANAGED_MISE_VERSION.as_bytes());
    push_fingerprint_part(&mut bytes, std::env::consts::OS.as_bytes());
    push_fingerprint_part(&mut bytes, std::env::consts::ARCH.as_bytes());
    for path in ["mise.toml", "mise.lock"] {
        push_fingerprint_part(&mut bytes, path.as_bytes());
        match std::fs::read(workspace.join(path)) {
            Ok(contents) => push_fingerprint_part(&mut bytes, &contents),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                push_fingerprint_part(&mut bytes, b"missing");
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("reading graph tool configuration `{path}`"));
            }
        }
    }
    for tool in tool_names {
        push_fingerprint_part(&mut bytes, tool.as_bytes());
    }
    for executable in executable_names {
        push_fingerprint_part(&mut bytes, executable.as_bytes());
    }
    for key in GRAPH_TOOL_ENVIRONMENT_KEYS {
        push_fingerprint_part(&mut bytes, key.as_bytes());
        match std::env::var_os(key) {
            Some(value) => push_fingerprint_part(&mut bytes, value.to_string_lossy().as_bytes()),
            None => push_fingerprint_part(&mut bytes, b"missing"),
        }
    }
    Ok(Digest::of_bytes(&bytes))
}

fn push_fingerprint_part(bytes: &mut Vec<u8>, part: &[u8]) {
    bytes.extend_from_slice(&(part.len() as u64).to_le_bytes());
    bytes.extend_from_slice(part);
}

fn read_graph_tool_cache(workspace: &Path, expected_fingerprint: Digest) -> Option<GraphToolCache> {
    read_graph_tool_cache_at(&graph_tool_cache_path(workspace), expected_fingerprint)
}

pub(super) fn read_graph_tool_cache_at(
    path: &Path,
    expected_fingerprint: Digest,
) -> Option<GraphToolCache> {
    let raw = match std::fs::read(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::debug!(%error, path = %path.display(), "failed to read graph tool paths");
            return None;
        }
    };
    let cached = match serde_json::from_slice::<GraphToolCache>(&raw) {
        Ok(cached) => cached,
        Err(error) => {
            tracing::debug!(%error, path = %path.display(), "failed to parse graph tool paths");
            return None;
        }
    };
    if cached.schema != GRAPH_TOOL_CACHE_SCHEMA
        || cached.fingerprint != expected_fingerprint.to_string()
        || graph_tool_paths_fingerprint(&cached.paths)
            .is_none_or(|fingerprint| fingerprint.to_string() != cached.paths_fingerprint)
    {
        return None;
    }
    Some(cached)
}

pub(super) fn write_graph_tool_cache(
    workspace: &Path,
    fingerprint: Digest,
    paths: &BTreeMap<String, String>,
    commands: &[CachedToolCommand],
) -> Result<()> {
    write_graph_tool_cache_at(
        &graph_tool_cache_path(workspace),
        fingerprint,
        paths,
        commands,
    )
}

pub(super) fn write_graph_tool_cache_at(
    path: &Path,
    fingerprint: Digest,
    paths: &BTreeMap<String, String>,
    commands: &[CachedToolCommand],
) -> Result<()> {
    let parent = path
        .parent()
        .expect("graph tool cache path always has a parent");
    std::fs::create_dir_all(parent)
        .with_context(|| format!("creating graph tool cache directory `{}`", parent.display()))?;
    let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
    let paths_fingerprint =
        graph_tool_paths_fingerprint(paths).context("fingerprinting graph tool paths")?;
    let record = GraphToolCache {
        schema: GRAPH_TOOL_CACHE_SCHEMA.to_string(),
        fingerprint: fingerprint.to_string(),
        paths_fingerprint: paths_fingerprint.to_string(),
        paths: paths.clone(),
        commands: commands.to_vec(),
    };
    let raw = serde_json::to_vec(&record).context("serializing graph tool paths")?;
    std::fs::write(&temporary, raw)
        .with_context(|| format!("writing graph tool cache `{}`", temporary.display()))?;
    if let Err(error) = std::fs::rename(&temporary, path) {
        if !path.is_file() {
            return Err(error)
                .with_context(|| format!("activating graph tool cache `{}`", path.display()));
        }
        std::fs::remove_file(path)
            .with_context(|| format!("replacing graph tool cache `{}`", path.display()))?;
        std::fs::rename(&temporary, path)
            .with_context(|| format!("activating graph tool cache `{}`", path.display()))?;
    }
    Ok(())
}

fn graph_tool_cache_path(workspace: &Path) -> PathBuf {
    graph_tool_cache_path_from(&struo_core::Xdg::from_env().struo_toolchains(), workspace)
}

pub(super) fn graph_tool_cache_path_from(toolchain_root: &Path, workspace: &Path) -> PathBuf {
    let workspace = std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    let workspace_id = Digest::of_bytes(workspace.to_string_lossy().as_bytes());
    toolchain_root.join(format!("{workspace_id}.json"))
}

pub(super) fn graph_tool_paths_fingerprint(paths: &BTreeMap<String, String>) -> Option<Digest> {
    if !cfg!(unix) {
        return None;
    }
    let mut bytes = Vec::new();
    for (name, path) in paths {
        if struo_core::is_mise_shim(Path::new(path)) {
            return None;
        }
        let metadata = std::fs::metadata(path).ok()?;
        if !metadata.is_file() {
            return None;
        }
        let modified = metadata
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?;
        push_fingerprint_part(&mut bytes, name.as_bytes());
        push_fingerprint_part(&mut bytes, path.as_bytes());
        push_fingerprint_part(&mut bytes, &metadata.len().to_le_bytes());
        push_fingerprint_part(&mut bytes, &modified.as_nanos().to_le_bytes());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            for value in [metadata.ctime(), metadata.ctime_nsec()] {
                push_fingerprint_part(&mut bytes, &value.to_le_bytes());
            }
            for value in [metadata.dev(), metadata.ino(), u64::from(metadata.mode())] {
                push_fingerprint_part(&mut bytes, &value.to_le_bytes());
            }
        }
        if let Some(stamp) = struo_host_tree::unsettled_stamp(&metadata) {
            push_fingerprint_part(&mut bytes, &stamp.to_le_bytes());
        }
    }
    Some(Digest::of_bytes(&bytes))
}

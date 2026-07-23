use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use once_cas::CacheProvider;
use once_core::{RemoteExecution, SandboxMode, Xdg};

use crate::cli::{self, Cli, Cmd, Output};
use crate::commands;

pub(crate) async fn dispatch(cli: Cli) -> Result<ExitCode> {
    let output = Output::new(cli.format, cli.quiet);
    if cli.list {
        return commands::surface::print(&cli.surface_path(), output)
            .await
            .map(|()| ExitCode::SUCCESS);
    }

    let Some(command) = cli.command else {
        return Ok(ExitCode::SUCCESS);
    };

    let workspace = resolve_workspace(cli.directory)?;
    let xdg = Xdg::from_env();
    Box::pin(run_command(&workspace, &xdg, output, command)).await
}

fn resolve_workspace(directory: Option<PathBuf>) -> Result<PathBuf> {
    let workspace = match directory {
        Some(directory) => {
            once_frontend::absolutize(directory).context("resolving workspace root")?
        }
        None => env::current_dir().context("resolving workspace root")?,
    };

    // A typo'd `-C` filename used to surface as a CAS write failure
    // because the CAS lived under `<workspace>/.once`. With the CAS
    // moved to `$XDG_CACHE_HOME/once/cas`, the bogus path would
    // silently run; explicit validation here keeps the loud error.
    if !workspace.is_dir() {
        anyhow::bail!(
            "once: workspace `{}` is not a directory",
            workspace.display()
        );
    }
    Ok(workspace)
}

async fn run_command(
    workspace: &Path,
    xdg: &Xdg,
    output: Output,
    command: Cmd,
) -> Result<ExitCode> {
    match command {
        Cmd::Auth { cmd } => run_auth_command(workspace, xdg, output, cmd).await,
        Cmd::Build { target, sandbox } => {
            dispatch_build(workspace, xdg, output, target, sandbox).await
        }
        Cmd::Run {
            target,
            sandbox,
            visible,
            runtime_rpc,
            runtime_rpc_socket,
            remote,
            compute,
        } => {
            Box::pin(dispatch_run(
                workspace,
                xdg,
                RunDispatchArgs {
                    output,
                    target,
                    visible,
                    runtime_rpc,
                    runtime_rpc_socket,
                    sandbox,
                    remote: resolve_remote_execution(workspace, xdg, remote, compute.as_deref())?,
                },
            ))
            .await
        }
        Cmd::Exec {
            sandbox,
            script,
            env,
            cwd,
            timeout_ms,
            cache_failures,
            remote,
            compute,
            argv,
        } => {
            dispatch_exec(
                workspace,
                xdg,
                output,
                commands::exec::ExecArgs {
                    sandbox,
                    script,
                    env,
                    cwd,
                    timeout_ms,
                    cache_failures,
                    remote: resolve_remote_execution(workspace, xdg, remote, compute.as_deref())?,
                    argv,
                },
            )
            .await
        }
        Cmd::Cache { cmd } => run_cache_command(workspace, xdg, output, cmd).await,
        Cmd::Test {
            target,
            sandbox,
            jobs,
            all,
            changed_paths,
            test_unit,
            batch_test_units,
            test_batch_id,
        } => {
            Box::pin(dispatch_test(
                workspace,
                xdg,
                TestDispatchArgs {
                    output,
                    target,
                    sandbox,
                    jobs,
                    all,
                    changed_paths,
                    test_unit,
                    batch_test_units,
                    test_batch_id,
                },
            ))
            .await
        }
        Cmd::Toolchain { cmd } => run_toolchain_command(workspace, output, cmd).await,
        Cmd::Query { expression, cmd } => {
            run_query_command(workspace, output, expression.as_deref(), cmd).await
        }
        Cmd::Edit { cmd } => run_edit_command(workspace, output, cmd).await,
        Cmd::Runtime { cmd } => run_runtime_command(workspace, output, cmd).await,
        Cmd::Mcp {
            workspace: workspace_override,
            allow_run,
        } => run_mcp_command(workspace, workspace_override, allow_run).await,
        Cmd::Reference { out } => crate::reference::generate(&out),
    }
}

async fn run_mcp_command(
    workspace: &Path,
    workspace_override: Option<PathBuf>,
    allow_run: bool,
) -> Result<ExitCode> {
    let workspace = resolve_mcp_workspace(workspace, workspace_override)?;
    commands::mcp::serve(workspace, allow_run)
        .await
        .map(|()| ExitCode::SUCCESS)
}

fn resolve_mcp_workspace(workspace: &Path, workspace_override: Option<PathBuf>) -> Result<PathBuf> {
    match workspace_override {
        Some(workspace) => resolve_workspace(Some(workspace)),
        None => Ok(workspace.to_path_buf()),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn mcp_workspace_override_is_canonicalized() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        let alias = temporary.path().join("alias");
        std::fs::create_dir(&workspace).unwrap();
        std::os::unix::fs::symlink(&workspace, &alias).unwrap();

        assert_eq!(
            resolve_mcp_workspace(temporary.path(), Some(alias)).unwrap(),
            std::fs::canonicalize(workspace).unwrap()
        );
    }
}

async fn dispatch_build(
    workspace: &Path,
    xdg: &Xdg,
    output: Output,
    target: Option<String>,
    sandbox: SandboxMode,
) -> Result<ExitCode> {
    let target = resolve_required_target(workspace, target)?;
    let cache = crate::cache_provider::resolve(workspace, xdg)?;
    Box::pin(commands::graph::build(
        workspace, &cache, output, &target, sandbox,
    ))
    .await
}

struct TestDispatchArgs {
    output: Output,
    target: Option<String>,
    sandbox: SandboxMode,
    jobs: Option<usize>,
    all: bool,
    changed_paths: Vec<String>,
    test_unit: Option<String>,
    batch_test_units: Vec<String>,
    test_batch_id: Option<String>,
}

async fn dispatch_test(workspace: &Path, xdg: &Xdg, args: TestDispatchArgs) -> Result<ExitCode> {
    if !args.batch_test_units.is_empty() {
        let target = resolve_required_target(workspace, args.target)?;
        let cache = crate::cache_provider::resolve(workspace, xdg)?;
        return Box::pin(commands::graph::test_with_filters(
            workspace,
            &cache,
            args.output,
            &target,
            args.sandbox,
            &args.batch_test_units,
            args.test_batch_id.as_deref(),
        ))
        .await;
    }
    if !args.all && args.changed_paths.is_empty() && args.jobs.is_none() && args.test_unit.is_none()
    {
        let target = resolve_required_target(workspace, args.target)?;
        let cache = crate::cache_provider::resolve(workspace, xdg)?;
        return Box::pin(commands::graph::test(
            workspace,
            &cache,
            args.output,
            &target,
            args.sandbox,
        ))
        .await;
    }
    let graph = once_frontend::load_graph_workspace(workspace).context("loading graph")?;
    let plan = match args.target {
        Some(target) => {
            let target = resolve_target_arg(workspace, &target)?;
            if let Some(test_unit) = args.test_unit {
                commands::query::explicit_test_unit_plan_with_graph(
                    workspace, &graph, &target, &test_unit,
                )?
            } else {
                commands::query::explicit_test_plan_with_graph(workspace, &graph, &[target])?
            }
        }
        None => {
            commands::query::test_plan_for_paths_with_graph(workspace, &graph, &args.changed_paths)?
        }
    };
    Box::pin(commands::test_schedule::run(
        workspace,
        Some(graph),
        args.output,
        plan,
        args.jobs,
        args.sandbox,
    ))
    .await
}

async fn run_toolchain_command(
    workspace: &Path,
    output: Output,
    command: Option<cli::ToolchainCmd>,
) -> Result<ExitCode> {
    match command {
        Some(cli::ToolchainCmd::Inspect { platform }) => {
            commands::toolchain::inspect(workspace, output, platform.as_deref())
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        None => anyhow::bail!("toolchain subcommand required"),
    }
}

#[allow(clippy::too_many_lines)]
async fn run_query_command(
    workspace: &Path,
    output: Output,
    expression: Option<&str>,
    command: Option<cli::QueryCmd>,
) -> Result<ExitCode> {
    match command {
        Some(cli::QueryCmd::Workspace) => commands::query::workspace(workspace, output)
            .await
            .map(|()| ExitCode::SUCCESS),
        Some(cli::QueryCmd::Targets { kind }) => {
            commands::query::targets(workspace, output, kind.as_deref())
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        Some(cli::QueryCmd::Capabilities { target }) => {
            commands::query::capabilities(workspace, output, &target)
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        Some(cli::QueryCmd::Schema { kind }) => commands::query::schema(workspace, output, &kind)
            .await
            .map(|()| ExitCode::SUCCESS),
        Some(cli::QueryCmd::Example { kind, slug }) => {
            commands::query::example(workspace, output, &kind, &slug)
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        Some(cli::QueryCmd::TargetKinds { query }) => {
            commands::query::target_kinds(workspace, output, query.as_deref())
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        Some(cli::QueryCmd::ModuleContract) => commands::query::module_contract(output)
            .await
            .map(|()| ExitCode::SUCCESS),
        Some(cli::QueryCmd::ExternalSource { url, max_bytes }) => {
            commands::query::external_source(output, &url, max_bytes)
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        Some(cli::QueryCmd::Target { target }) => {
            commands::query::target(workspace, output, &target)
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        Some(cli::QueryCmd::Tests) => commands::query::tests(workspace, output)
            .await
            .map(|()| ExitCode::SUCCESS),
        Some(cli::QueryCmd::AffectedTests { changed_paths }) => {
            commands::query::affected_tests(workspace, output, &changed_paths)
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        Some(cli::QueryCmd::TestPlan {
            changed_paths,
            target,
            test_unit,
        }) => run_test_plan_query(workspace, output, &changed_paths, target, test_unit).await,
        Some(cli::QueryCmd::TestResults { target }) => {
            commands::query::test_results(workspace, output, &target)
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        Some(cli::QueryCmd::TestManifest { target }) => {
            commands::query::test_manifest(workspace, output, &target)
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        Some(cli::QueryCmd::TestAttempts { target, limit }) => {
            commands::query::test_attempts(workspace, output, target.as_deref(), limit)
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        Some(cli::QueryCmd::Evidence { subject, limit }) => {
            commands::query::evidence(workspace, output, subject.as_deref(), limit)
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        Some(cli::QueryCmd::ValidateTarget { file }) => {
            commands::query::validate_target(workspace, output, file)
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        Some(cli::QueryCmd::Script { path }) => commands::query::script(workspace, output, &path)
            .await
            .map(|()| ExitCode::SUCCESS),
        Some(cli::QueryCmd::ValidateWorkspace) => {
            commands::query::validate_workspace(workspace, output)
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        Some(cli::QueryCmd::ValidateActions {
            target,
            capability,
            action,
        }) => commands::query::validate_actions(workspace, output, &target, &capability, action)
            .await
            .map(|()| ExitCode::SUCCESS),
        Some(cli::QueryCmd::ValidateModule { path }) => {
            commands::query::validate_module(workspace, output, &path)
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        None if let Some(expression) = expression => {
            commands::query::expression(workspace, output, expression)
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        None => anyhow::bail!("query subcommand required"),
    }
}

async fn run_test_plan_query(
    workspace: &Path,
    output: Output,
    changed_paths: &[String],
    target: Option<String>,
    test_unit: Option<String>,
) -> Result<ExitCode> {
    commands::query::test_plan_request(
        workspace,
        output,
        changed_paths,
        target.as_deref(),
        test_unit.as_deref(),
    )
    .await
    .map(|()| ExitCode::SUCCESS)
}

async fn run_edit_command(
    workspace: &Path,
    output: Output,
    command: Option<cli::EditCmd>,
) -> Result<ExitCode> {
    match command {
        Some(cli::EditCmd::Apply { file }) => commands::edit::apply(workspace, output, file)
            .await
            .map(|()| ExitCode::SUCCESS),
        Some(cli::EditCmd::MaterializeExample {
            kind,
            slug,
            destination,
        }) => commands::edit::materialize_example(workspace, output, &kind, &slug, &destination)
            .await
            .map(|()| ExitCode::SUCCESS),
        None => anyhow::bail!("edit subcommand required"),
    }
}

async fn run_runtime_command(
    workspace: &Path,
    output: Output,
    command: Option<cli::RuntimeCmd>,
) -> Result<ExitCode> {
    match command {
        Some(cli::RuntimeCmd::Start { target }) => {
            let target = resolve_required_target(workspace, target)?;
            commands::runtime::start(workspace, output, &target).await
        }
        Some(cli::RuntimeCmd::Status { session }) => {
            commands::runtime::status(workspace, output, &session).await
        }
        Some(cli::RuntimeCmd::Logs {
            session,
            source,
            cursor,
            limit,
        }) => {
            commands::runtime::logs(
                workspace,
                output,
                &session,
                source.as_deref(),
                cursor.as_deref(),
                limit,
            )
            .await
        }
        Some(cli::RuntimeCmd::Stop { session }) => {
            commands::runtime::stop(workspace, output, &session).await
        }
        Some(cli::RuntimeCmd::Rpc {
            session_dir,
            socket,
        }) => commands::runtime::rpc(&session_dir, socket.as_deref())
            .await
            .map(|()| ExitCode::SUCCESS),
        Some(cli::RuntimeCmd::Supervise {
            session_dir,
            target,
        }) => commands::runtime::supervise(workspace, &session_dir, &target),
        None => anyhow::bail!("runtime subcommand required"),
    }
}

async fn run_auth_command(
    workspace: &Path,
    xdg: &Xdg,
    output: Output,
    command: Option<cli::AuthCmd>,
) -> Result<ExitCode> {
    match command {
        Some(cli::AuthCmd::Login {
            provider,
            no_browser,
        }) => commands::auth::login(workspace, xdg, &provider, !no_browser, output)
            .await
            .map(|()| ExitCode::SUCCESS),
        Some(cli::AuthCmd::Logout { provider }) => {
            commands::auth::logout(workspace, xdg, &provider, output)
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        None => anyhow::bail!("auth subcommand required"),
    }
}

async fn run_cache_command(
    workspace: &Path,
    xdg: &Xdg,
    output: Output,
    command: Option<cli::CacheCmd>,
) -> Result<ExitCode> {
    match command {
        Some(cli::CacheCmd::Stats) => {
            let cache = crate::cache_provider::resolve(workspace, xdg)?;
            commands::cache::print_stats(&cache, output)
                .await
                .map(|()| ExitCode::SUCCESS)
        }
        Some(cli::CacheCmd::Blob { cmd }) => {
            let cache = crate::cache_provider::resolve(workspace, xdg)?;
            match cmd {
                Some(cli::CacheBlobCmd::Put { path }) => {
                    commands::cache::put_blob(&cache, path.as_deref(), output)
                        .await
                        .map(|()| ExitCode::SUCCESS)
                }
                Some(cli::CacheBlobCmd::Get {
                    digest,
                    output: output_path,
                }) => commands::cache::get_blob(&cache, digest, output_path.as_deref(), output)
                    .await
                    .map(|()| ExitCode::SUCCESS),
                Some(cli::CacheBlobCmd::Exists { digest }) => {
                    commands::cache::exists_blob(&cache, digest, output).await
                }
                None => anyhow::bail!("cache blob subcommand required"),
            }
        }
        Some(cli::CacheCmd::Action { cmd }) => {
            let cache = crate::cache_provider::resolve(workspace, xdg)?;
            match cmd {
                Some(cli::CacheActionCmd::Get {
                    action,
                    inputs,
                    if_success,
                }) => commands::cache::get_action(&cache, action, inputs, if_success, output).await,
                Some(cli::CacheActionCmd::Put {
                    action,
                    inputs,
                    exit_code,
                    stdout,
                    stderr,
                    outputs,
                }) => commands::cache::put_action(
                    &cache, action, inputs, exit_code, stdout, stderr, outputs, output,
                )
                .await
                .map(|()| ExitCode::SUCCESS),
                Some(cli::CacheActionCmd::Forget { action }) => {
                    commands::cache::forget_action(&cache, action, output)
                        .await
                        .map(|()| ExitCode::SUCCESS)
                }
                None => anyhow::bail!("cache action subcommand required"),
            }
        }
        None => anyhow::bail!("cache subcommand required"),
    }
}

struct RunDispatchArgs {
    output: Output,
    target: Option<String>,
    visible: bool,
    runtime_rpc: bool,
    runtime_rpc_socket: Option<PathBuf>,
    sandbox: SandboxMode,
    remote: Option<RemoteExecution>,
}

async fn dispatch_run(workspace: &Path, xdg: &Xdg, args: RunDispatchArgs) -> Result<ExitCode> {
    let RunDispatchArgs {
        output,
        target,
        visible,
        runtime_rpc,
        runtime_rpc_socket,
        sandbox,
        remote,
    } = args;
    let resolved_target = resolve_required_target(workspace, target.clone())?;
    if let Some(graph) =
        commands::graph::load_graph_for_capability(workspace, &resolved_target, "run")?
    {
        if runtime_rpc || runtime_rpc_socket.is_some() {
            anyhow::bail!("--runtime-rpc is only supported for executable script targets");
        }
        if remote.is_some() {
            anyhow::bail!("--remote is only supported for executable script targets");
        }
        let cache = crate::cache_provider::resolve(workspace, xdg)?;
        // Keep dispatch_run's future below clippy's large_futures limit.
        return Box::pin(commands::graph::run(
            workspace,
            &cache,
            graph,
            output,
            &resolved_target,
            commands::graph::GraphRunOptions { visible },
            sandbox,
        ))
        .await;
    }
    if visible {
        anyhow::bail!("--visible is only supported for graph run targets");
    }
    let cache = crate::cache_provider::resolve(workspace, xdg)?;
    run_target_command(
        workspace,
        &cache,
        output,
        Some(resolved_target),
        runtime_rpc,
        runtime_rpc_socket,
        sandbox,
        remote,
    )
    .await
}

async fn dispatch_exec(
    workspace: &Path,
    xdg: &Xdg,
    output: Output,
    args: commands::exec::ExecArgs,
) -> Result<ExitCode> {
    let cache = crate::cache_provider::resolve(workspace, xdg)?;
    commands::exec::exec(workspace, xdg, &cache, args, output).await
}

#[allow(clippy::too_many_arguments)]
async fn run_target_command(
    workspace: &Path,
    cache: &CacheProvider,
    output: Output,
    target: Option<String>,
    runtime_rpc: bool,
    runtime_rpc_socket: Option<PathBuf>,
    sandbox: SandboxMode,
    remote: Option<RemoteExecution>,
) -> Result<ExitCode> {
    let target = resolve_required_target(workspace, target)?;
    commands::run::run(
        workspace,
        cache,
        &target,
        commands::run::RunArgs {
            output,
            runtime_rpc,
            runtime_rpc_socket,
            sandbox,
            remote,
        },
    )
    .await
}

fn resolve_remote_execution(
    workspace: &Path,
    xdg: &Xdg,
    remote: bool,
    compute: Option<&str>,
) -> Result<Option<RemoteExecution>> {
    if remote {
        crate::cache_provider::resolve_execution(workspace, xdg, compute).map(Some)
    } else {
        Ok(None)
    }
}

fn resolve_required_target(workspace: &Path, target: Option<String>) -> Result<String> {
    let raw = target.context("missing target")?;
    resolve_target_arg(workspace, &raw)
}

fn resolve_target_arg(workspace: &Path, raw: &str) -> Result<String> {
    once_frontend::normalize_cli_target(workspace, raw).context("resolving target argument")
}

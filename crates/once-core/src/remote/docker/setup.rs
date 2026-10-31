use std::path::Path;

use super::process::Client;
use super::{join_path, CommandFilesystemSetup, PreparedCommand, ROOT};
use crate::{resolve_execution_argv, resolve_execution_env, Error, ExecutionPermit, Result};

pub(super) async fn stage_inputs(client: &Client, container: &str, archive: &Path) -> Result<()> {
    client
        .control(&[
            "exec".into(),
            container.into(),
            "rm".into(),
            "-rf".into(),
            "--".into(),
            ROOT.into(),
        ])
        .await?;
    client
        .control(&[
            "exec".into(),
            container.into(),
            "mkdir".into(),
            "-p".into(),
            "--".into(),
            ROOT.into(),
        ])
        .await?;
    client
        .transfer(
            &[
                "exec".into(),
                "-i".into(),
                container.into(),
                "tar".into(),
                "-xf".into(),
                "-".into(),
                "-C".into(),
                ROOT.into(),
            ],
            Some(archive),
            None,
        )
        .await
}

pub(super) async fn apply(
    client: &Client,
    container: &str,
    command: &PreparedCommand<'_>,
    setup: CommandFilesystemSetup<'_>,
) -> Result<()> {
    if !setup.clean_paths.is_empty() {
        let mut clean = vec![
            "exec".into(),
            container.into(),
            "rm".into(),
            "-rf".into(),
            "--".into(),
        ];
        clean.extend(
            setup
                .clean_paths
                .iter()
                .map(|path| join_path(ROOT, path.as_str())),
        );
        client.control(&clean).await?;
    }
    let mut dirs = vec![ROOT.to_string()];
    dirs.extend(
        setup
            .create_dirs
            .iter()
            .map(|path| join_path(ROOT, path.as_str())),
    );
    if let Some(cwd) = command.cwd {
        dirs.push(join_path(ROOT, cwd.as_str()));
    }
    for output in command.outputs {
        if let Some(parent) = Path::new(output.as_str()).parent() {
            dirs.push(join_path(ROOT, &parent.to_string_lossy()));
        }
    }
    let mut mkdir = vec![
        "exec".into(),
        container.into(),
        "mkdir".into(),
        "-p".into(),
        "--".into(),
    ];
    mkdir.extend(dirs);
    client.control(&mkdir).await?;
    Ok(())
}

pub(super) async fn restore_inputs(
    client: &Client,
    container: &str,
    workspace: &Path,
    setup: CommandFilesystemSetup<'_>,
    lease: &ExecutionPermit,
) -> Result<()> {
    if setup.post_setup_inputs.is_empty() {
        return Ok(());
    }
    let (archive, _phase) = super::create_input_archive_limited(
        workspace,
        setup.post_setup_inputs,
        "docker",
        client.limits,
        Some(lease),
    )
    .await?;
    client
        .transfer(
            &[
                "exec".into(),
                "-i".into(),
                container.into(),
                "tar".into(),
                "-xf".into(),
                "-".into(),
                "-C".into(),
                ROOT.into(),
            ],
            Some(archive.path()),
            None,
        )
        .await
}

pub(super) fn execution_args(
    container: &str,
    command: &PreparedCommand<'_>,
) -> Result<Vec<String>> {
    let mut args = vec![
        "exec".into(),
        "--workdir".into(),
        command
            .cwd
            .map_or_else(|| ROOT.into(), |cwd| join_path(ROOT, cwd.as_str())),
    ];
    for (key, value) in resolve_execution_env(command.env, Path::new(ROOT)) {
        args.push("--env".into());
        args.push(format!("{key}={value}"));
    }
    args.push(container.into());
    let resolved = resolve_execution_argv(command.argv, Path::new(ROOT));
    if resolved.is_empty() {
        return Err(Error::EmptyArgv);
    }
    args.extend(resolved);
    Ok(args)
}

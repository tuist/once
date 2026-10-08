use std::path::{Path, PathBuf};

use anyhow::Result;
use once_frontend::NativeInvocation;

use crate::cli::Cmd;

pub(crate) fn query_graph(directory: &Path) -> Result<(PathBuf, Vec<once_frontend::GraphTarget>)> {
    if let Some(invocation) = once_frontend::native_invocation(directory)? {
        let mut resolved =
            crate::commands::graph::resolve_invocation_configuration(&invocation.workspace, &[])?;
        resolved.native_targets = Some(invocation.targets);
        let graph = resolved.load_graph(&invocation.workspace)?;
        Ok((invocation.workspace, graph))
    } else {
        Ok((
            directory.to_path_buf(),
            once_frontend::load_graph_workspace(directory)?,
        ))
    }
}

pub(crate) fn prepare(directory: &Path, command: &mut Cmd) -> Result<Option<NativeInvocation>> {
    let (target, all) = match command {
        Cmd::Build { target, all, .. }
        | Cmd::Test { target, all, .. }
        | Cmd::Lint { target, all, .. } => (target, *all),
        Cmd::Run { target, .. } => (target, false),
        _ => return Ok(None),
    };
    if directory.join(once_frontend::TOML_BUILD_FILE_NAME).exists() {
        return Ok(None);
    }
    let project_dir = target
        .as_deref()
        .and_then(|value| {
            let parent = Path::new(value).parent()?;
            (!parent.as_os_str().is_empty()
                && !parent.is_absolute()
                && !parent
                    .components()
                    .any(|part| matches!(part, std::path::Component::ParentDir)))
            .then(|| directory.join(parent))
            .filter(|path| path.is_dir())
        })
        .unwrap_or_else(|| directory.to_path_buf());
    let Some(mut invocation) = once_frontend::native_invocation(&project_dir)? else {
        return Ok(None);
    };
    if let Some(value) = target.as_mut() {
        if !value.contains('/') && !invocation.package.is_empty() {
            *value = format!("{}/{}", invocation.package, value);
        }
        if let Some(seed) = invocation
            .local_targets
            .iter()
            .find(|seed| seed.id() == *value)
        {
            if !invocation
                .targets
                .iter()
                .any(|candidate| candidate.id() == *value)
            {
                invocation.targets = vec![seed.clone()];
            }
        }
    } else if !all && matches!(command, Cmd::Build { .. }) {
        let root = invocation
            .targets
            .iter()
            .find(|seed| seed.package == invocation.package);
        if let (Some(root), Cmd::Build { target, .. }) = (root, command) {
            *target = Some(root.id());
        }
    }
    tracing::debug!(workspace = %invocation.workspace.display(), package = %invocation.package,
        "selected invocation-local native project");
    Ok(Some(invocation))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(args: &[&str]) -> Cmd {
        let args = args.iter().map(std::ffi::OsStr::new).collect::<Vec<_>>();
        crate::cli::Cli::try_parse_from(&args)
            .unwrap()
            .command
            .unwrap()
    }

    #[test]
    fn preserves_foreign_native_targets_and_repository_root_default_commands() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".git")).unwrap();
        let project = root.path().join("app");
        std::fs::create_dir(&project).unwrap();
        std::fs::write(project.join("mix.exs"), "raise \"not evaluated\"").unwrap();
        std::fs::write(project.join("Dockerfile"), "FROM scratch\n").unwrap();
        let mut default_build = command(&["once", "build"]);
        assert!(prepare(root.path(), &mut default_build).unwrap().is_none());
        let mut build = command(&["once", "build", "app/image"]);
        let selected = prepare(root.path(), &mut build).unwrap().unwrap();
        assert_eq!(selected.targets.len(), 1);
        assert_eq!(selected.targets[0].id(), "app/image");
        let mut build = command(&["once", "build", "image"]);
        let selected = prepare(&project, &mut build).unwrap().unwrap();
        assert_eq!(selected.targets.len(), 1);
        assert_eq!(selected.targets[0].id(), "app/image");
    }

    #[test]
    fn uses_the_containing_boundary_for_run_lint_and_all() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".git")).unwrap();
        let project = root.path().join("app");
        std::fs::create_dir(&project).unwrap();
        std::fs::write(project.join("mix.exs"), "raise \"not evaluated\"").unwrap();
        for args in [
            vec!["once", "run", "mix_lint"],
            vec!["once", "lint"],
            vec!["once", "build", "--all"],
            vec!["once", "test", "--all"],
        ] {
            let mut command = command(&args);
            let selected = prepare(&project, &mut command).unwrap().unwrap();
            assert_eq!(
                selected.workspace,
                std::fs::canonicalize(root.path()).unwrap()
            );
            assert_eq!(selected.targets[0].id(), "app/mix");
            if let Cmd::Build { target, .. } = command {
                assert!(target.is_none());
            }
        }
    }
}

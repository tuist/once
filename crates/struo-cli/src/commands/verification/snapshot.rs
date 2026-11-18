use std::path::Path;

use anyhow::{Context, Result};
use struo_core::WorkspacePath;

pub(super) fn copy(source: &Path, destination: &Path, excluded: &[WorkspacePath]) -> Result<()> {
    if excluded.iter().any(|path| path.as_str() == ".") {
        anyhow::bail!("cannot verify a script that declares the whole workspace as an output; declare its generated paths instead");
    }
    copy_entry(source, source, destination, destination, excluded)
}

fn copy_entry(
    root: &Path,
    source: &Path,
    destination_root: &Path,
    destination: &Path,
    excluded: &[WorkspacePath],
) -> Result<()> {
    let relative = source.strip_prefix(root)?;
    if relative
        .components()
        .any(|component| component.as_os_str() == ".struo")
        || excluded
            .iter()
            .any(|output| relative.starts_with(output.as_str()))
    {
        return Ok(());
    }
    let metadata = std::fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        let resolved = source
            .canonicalize()
            .with_context(|| format!("cannot snapshot symlink `{}`", relative.display()))?;
        let canonical_root = root.canonicalize()?;
        let target_relative = resolved.strip_prefix(&canonical_root)
            .with_context(|| format!("cannot privately snapshot symlink `{}` because it points outside the workspace; materialize it inside the workspace before verification", relative.display()))?;
        let target = std::fs::read_link(source)?;
        let target = if target.is_absolute() {
            destination_root.join(target_relative)
        } else {
            target
        };
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, destination)?;
        #[cfg(windows)]
        if resolved.is_dir() {
            std::os::windows::fs::symlink_dir(target, destination)?;
        } else {
            std::os::windows::fs::symlink_file(target, destination)?;
        }
        return Ok(());
    }
    if metadata.is_dir() {
        std::fs::create_dir_all(destination)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_entry(
                root,
                &entry.path(),
                destination_root,
                &destination.join(entry.file_name()),
                excluded,
            )?;
        }
    } else if metadata.is_file() {
        std::fs::copy(source, destination)?;
    } else {
        anyhow::bail!(
            "cannot snapshot special file `{}`; remove it from the workspace before verification",
            relative.display()
        );
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.access_mode(0x100).custom_flags(0x0200_0000);
    }
    let file = options.open(destination)?;
    let mut times = std::fs::FileTimes::new();
    if let Ok(modified) = metadata.modified() {
        times = times.set_modified(modified);
    }
    if let Ok(accessed) = metadata.accessed() {
        times = times.set_accessed(accessed);
    }
    file.set_times(times)?;
    std::fs::set_permissions(destination, metadata.permissions())?;
    Ok(())
}

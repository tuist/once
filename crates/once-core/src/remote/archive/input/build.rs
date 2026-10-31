use std::path::Path;

use super::plan::InputPlan;
use crate::remote::archive::limits::{limit_error, LimitedWriter};
use crate::remote::archive::{archive_error, file_error, TempArchive};
use crate::remote::path::relative_link_stays_within;
use crate::Result;

pub(super) fn archive(
    _workspace_root: &Path,
    plan: InputPlan,
    provider: &'static str,
) -> Result<TempArchive> {
    let file = tempfile::NamedTempFile::new().map_err(|source| {
        file_error(
            "create_remote_input_archive",
            Path::new(".once/tmp"),
            source,
        )
    })?;
    let mut builder = tar::Builder::new(LimitedWriter::new(file, plan.archive_bytes));
    builder.follow_symlinks(false);
    for (relative, _source) in plan.entries {
        if relative.as_os_str().is_empty() {
            continue;
        }
        let metadata = plan
            .workspace
            .metadata(&relative)
            .map_err(|source_error| file_error("read_remote_input", &relative, source_error))?;
        let result = if metadata.is_symlink() {
            let target = plan
                .workspace
                .read_link(&relative)
                .map_err(|source_error| {
                    file_error("read_remote_input_link", &relative, source_error)
                })?;
            if !relative_link_stays_within(&relative, &target, &plan.declared_roots) {
                return Err(archive_error(
                    provider,
                    format!(
                        "input symbolic link `{}` targets `{}` outside declared input trees",
                        relative.display(),
                        target.display()
                    ),
                ));
            }
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            header.set_mode(0o777);
            builder.append_link(&mut header, &relative, target)
        } else if metadata.is_dir() {
            let directory_metadata = plan
                .workspace
                .descend(&relative, false)
                .and_then(|directory| directory.filesystem_metadata())
                .map_err(|source| file_error("read_remote_input_directory", &relative, source))?;
            let mut header = tar::Header::new_gnu();
            header.set_metadata(&directory_metadata);
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            header.set_cksum();
            builder.append_data(&mut header, &relative, std::io::empty())
        } else if metadata.is_file() {
            let mut file = plan
                .workspace
                .open_file(&relative)
                .map_err(|source_error| file_error("open_remote_input", &relative, source_error))?;
            builder.append_file(&relative, &mut file)
        } else {
            return Err(archive_error(
                provider,
                format!(
                    "input `{}` is not a file, directory, or symbolic link",
                    relative.display()
                ),
            ));
        };
        result.map_err(|source_error| {
            limit_error(provider, &source_error)
                .unwrap_or_else(|| file_error("archive_remote_input", &relative, source_error))
        })?;
    }
    builder.finish().map_err(|source| {
        limit_error(provider, &source).unwrap_or_else(|| {
            file_error(
                "finish_remote_input_archive",
                Path::new(".once/tmp"),
                source,
            )
        })
    })?;
    let file = builder
        .into_inner()
        .map_err(|source| {
            file_error(
                "finish_remote_input_archive",
                Path::new(".once/tmp"),
                source,
            )
        })?
        .into_inner();
    let len = file
        .as_file()
        .metadata()
        .map_err(|source| file_error("read_remote_input_archive", Path::new(".once/tmp"), source))?
        .len();
    Ok(TempArchive {
        path: file.into_temp_path(),
        len,
    })
}

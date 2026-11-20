//! The `download_and_extract` action: a bounded, verified archive download
//! that unpacks a zip into the workspace.

mod zip_preflight;

#[cfg(test)]
mod tests;

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Component, Path};
use std::sync::Arc;

use futures::StreamExt;
use sha2::Digest as ShaDigest;

use crate::executor::staging::StagingPermit;
use crate::{Error, ExecutionPermit, Executor, RemoteIoLimits, Result, WorkspacePath};

use super::fs::remove_path_blocking;
use super::{file_step, spawn_admitted};

#[allow(clippy::too_many_lines)]
pub(super) async fn download_and_extract(
    url: &str,
    expected_sha256: &str,
    destination: &WorkspacePath,
    authorization_env: Option<&str>,
    workspace_root: &Path,
    lease: Option<&ExecutionPermit>,
) -> Result<()> {
    let parsed = reqwest::Url::parse(url).map_err(|source| Error::InvalidDownloadAndExtract {
        reason: format!("invalid URL: {source}"),
    })?;
    let display_url = redacted_url(&parsed);
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(Error::InvalidDownloadAndExtract {
            reason: "URL must use HTTP or HTTPS".to_string(),
        });
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(Error::InvalidDownloadAndExtract {
            reason: "URL user-info is not allowed; use authorization_env instead".to_string(),
        });
    }
    if !is_sha256(expected_sha256) {
        return Err(Error::InvalidDownloadAndExtract {
            reason: "sha256 must be exactly 64 hexadecimal characters".to_string(),
        });
    }
    let absolute_destination = destination.resolve(workspace_root);
    let destination_parent =
        absolute_destination
            .parent()
            .ok_or_else(|| Error::InvalidDownloadAndExtract {
                reason: format!(
                    "destination `{}` has no parent directory",
                    destination.as_str()
                ),
            })?;
    let parent = destination_parent.to_path_buf();
    file_step(
        lease,
        "download_and_extract",
        destination.as_str(),
        move || std::fs::create_dir_all(parent),
    )
    .await?;

    let client =
        reqwest::Client::builder()
            .build()
            .map_err(|source| Error::InvalidDownloadAndExtract {
                reason: format!("creating HTTP client: {source}"),
            })?;
    let mut request = client.get(parsed);
    if let Some(name) = authorization_env {
        let value = std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| Error::DownloadAuthorizationMissing {
                name: name.to_string(),
            })?;
        request = request.header(reqwest::header::AUTHORIZATION, value);
    }
    let limits = lease.map_or_else(RemoteIoLimits::default, ExecutionPermit::io_limits);
    // One absolute deadline covers the response headers, the whole body, and
    // its writes to the private archive, so a server that keeps trickling
    // bytes cannot hold the action forever. Running out of time drops only
    // this future: a write already on the blocking pool keeps its own handle
    // on the archive, its staging, and the lease until it stops. Extraction
    // starts after the deadline and is not covered by it.
    let deadline = tokio::time::Instant::now() + limits.artifact_download_timeout;
    let download = async {
        let response = tokio::time::timeout(limits.stall_timeout, request.send())
            .await
            .map_err(|_| download_stalled(limits))?
            .map_err(|source| Error::InvalidDownloadAndExtract {
                reason: format!("requesting `{display_url}`: {}", source.without_url()),
            })?
            .error_for_status()
            .map_err(|source| Error::InvalidDownloadAndExtract {
                reason: format!("requesting `{display_url}`: {}", source.without_url()),
            })?;
        let declared = response.content_length();
        check_download_size(declared.unwrap_or(0), limits)?;
        let staging = match lease {
            Some(lease) => Some(
                lease
                    .acquire_staging(download_staging_bytes(declared, limits))
                    .await?,
            ),
            None => None,
        };

        let archive_parent = destination_parent.to_path_buf();
        // Created on the blocking pool with its staging, so the two are
        // never apart.
        let archive = Arc::new(
            file_step(
                lease,
                "download_and_extract",
                destination.as_str(),
                move || {
                    Ok(DownloadedArchive {
                        file: tempfile::NamedTempFile::new_in(archive_parent)?,
                        _staging: staging,
                    })
                },
            )
            .await?,
        );
        let archive_label = archive.file.path().display().to_string();
        let mut digest = sha2::Sha256::new();
        let mut stream = response.bytes_stream();
        let mut size = 0_u64;
        while let Some(chunk) = tokio::time::timeout(limits.stall_timeout, stream.next())
            .await
            .map_err(|_| download_stalled(limits))?
        {
            let chunk = chunk.map_err(|source| Error::InvalidDownloadAndExtract {
                reason: format!("reading `{display_url}`: {}", source.without_url()),
            })?;
            size = size.saturating_add(chunk.len() as u64);
            check_download_size(size, limits)?;
            // The staging reservation covers the declared length only.
            if declared.is_some_and(|declared| size > declared) {
                return Err(Error::InvalidDownloadAndExtract {
                    reason: format!("`{display_url}` sent more bytes than it declared"),
                });
            }
            digest.update(&chunk);
            write_archive_chunk(lease, Arc::clone(&archive), &archive_label, chunk).await?;
        }
        Ok::<_, Error>((archive, digest, declared))
    };
    let (archive, digest, declared) = tokio::time::timeout_at(deadline, download)
        .await
        .map_err(|_| Error::Timeout(limits.artifact_download_timeout))??;

    let digest = digest.finalize();
    let mut actual_sha256 = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut actual_sha256, "{byte:02x}").expect("writing to a String cannot fail");
    }
    if !actual_sha256.eq_ignore_ascii_case(expected_sha256) {
        return Err(Error::DownloadChecksumMismatch {
            url: display_url,
            expected: expected_sha256.to_string(),
            actual: actual_sha256,
        });
    }

    let extracted_destination = absolute_destination.clone();
    let staging_parent = destination_parent.to_path_buf();
    spawn_admitted(lease, move || {
        // Owned here so the archive is deleted, and its staging released,
        // only after extraction stops.
        let archive = archive;
        let staging = tempfile::Builder::new()
            .prefix(".struo-download-")
            .tempdir_in(&staging_parent)
            .map_err(|source| Error::FileAction {
                action: "download_and_extract",
                path: staging_parent.display().to_string(),
                source,
            })?;
        let staged_output = staging.path().join("output");
        extract_zip_archive(
            archive.file.path(),
            &staged_output,
            limits,
            expansion_bound(declared, limits),
        )?;
        remove_path_blocking(&extracted_destination).map_err(|source| Error::FileAction {
            action: "download_and_extract",
            path: extracted_destination.display().to_string(),
            source,
        })?;
        std::fs::rename(&staged_output, &extracted_destination).map_err(|source| {
            Error::FileAction {
                action: "download_and_extract",
                path: extracted_destination.display().to_string(),
                source,
            }
        })
    })
    .await
    .map_err(|source| Error::FileAction {
        action: "download_and_extract",
        path: destination.as_str().to_string(),
        source: std::io::Error::other(source.to_string()),
    })??;
    Ok(())
}

/// A downloaded archive and the staging reserved for it and its extracted
/// copy. Every blocking write and the extraction own a clone, so the file is
/// deleted and the reservation released only after the last task touching
/// it stops, extraction and publication included; on Windows an open handle
/// would otherwise keep the delete from succeeding.
struct DownloadedArchive {
    file: tempfile::NamedTempFile,
    _staging: Option<StagingPermit>,
}

/// The most bytes extraction may write for a download that declared
/// `declared` bytes on the wire: `max_archive_expansion_ratio` times that,
/// capped by the archive limit. A download that declares no length may
/// expand to the whole archive limit.
fn expansion_bound(declared: Option<u64>, limits: RemoteIoLimits) -> u64 {
    declared.map_or(limits.max_archive_bytes, |declared| {
        declared
            .saturating_mul(limits.max_archive_expansion_ratio)
            .min(limits.max_archive_bytes)
    })
}

/// Staging one download needs, reserved in a single all-or-nothing request
/// before anything is written, so the download never waits for a second
/// permit while it holds the first: the archive (its declared length, or the
/// archive limit when it declares none) plus its [`expansion_bound`].
/// Downloads that declare their length reserve in proportion to it and run
/// side by side. With the default limits a download that declares none
/// reserves the whole staging pool, so such downloads run one at a time. A
/// staging limit below this sum fails the download instead of starting it.
fn download_staging_bytes(declared: Option<u64>, limits: RemoteIoLimits) -> u64 {
    declared
        .unwrap_or(limits.max_archive_bytes)
        .saturating_add(expansion_bound(declared, limits))
}

/// Append `chunk` to the archive on the blocking pool. The write owns a
/// clone of the archive, its staging, and the lease, so cancelling the
/// caller releases none of them while the write is queued or running.
async fn write_archive_chunk(
    lease: Option<&ExecutionPermit>,
    archive: Arc<DownloadedArchive>,
    label: &str,
    chunk: impl AsRef<[u8]> + Send + 'static,
) -> Result<()> {
    file_step(lease, "download_and_extract", label, move || {
        archive.file.as_file().write_all(chunk.as_ref())
    })
    .await
}

fn check_download_size(size: u64, limits: RemoteIoLimits) -> Result<()> {
    if size > limits.max_archive_bytes {
        Err(Error::ExecutorResourceLimit {
            executor: Executor::Local.name().to_string(),
            resource: "download bytes",
            limit: limits.max_archive_bytes,
        })
    } else {
        Ok(())
    }
}

fn download_stalled(limits: RemoteIoLimits) -> Error {
    Error::ExecutorStalled {
        executor: Executor::Local.name().to_string(),
        phase: "download",
        timeout: limits.stall_timeout,
    }
}

/// `url` without user-info, query, or fragment, which can carry credentials
/// the caller owns and must not reach errors or logs.
fn redacted_url(url: &reqwest::Url) -> String {
    let mut redacted = url.clone();
    let _ = redacted.set_username("");
    let _ = redacted.set_password(None);
    if redacted.query().is_some() {
        redacted.set_query(Some("redacted"));
    }
    redacted.set_fragment(None);
    redacted.to_string()
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Extract the archive at `archive_path` into `destination`, writing at
/// most `expansion` bytes of file contents.
pub(super) fn extract_zip_archive(
    archive_path: &Path,
    destination: &Path,
    limits: RemoteIoLimits,
    expansion: u64,
) -> Result<()> {
    std::fs::create_dir_all(destination).map_err(|source| Error::FileAction {
        action: "download_and_extract",
        path: destination.display().to_string(),
        source,
    })?;
    let archive_file = std::fs::File::open(archive_path).map_err(|source| Error::FileAction {
        action: "download_and_extract",
        path: archive_path.display().to_string(),
        source,
    })?;
    let mut archive = zip_preflight::open(archive_file, limits)?;
    if archive.len() > limits.max_archive_entries {
        return Err(Error::ExecutorResourceLimit {
            executor: Executor::Local.name().to_string(),
            resource: "archive entries",
            limit: u64::try_from(limits.max_archive_entries).unwrap_or(u64::MAX),
        });
    }
    let expanded_too_far = || Error::ExecutorResourceLimit {
        executor: Executor::Local.name().to_string(),
        resource: "extracted bytes",
        limit: expansion,
    };
    let mut remaining = expansion;
    for index in 0..archive.len() {
        let mut entry =
            archive
                .by_index(index)
                .map_err(|source| Error::InvalidDownloadAndExtract {
                    reason: format!("reading ZIP entry {index}: {source}"),
                })?;
        let name = entry
            .name()
            .map_err(|source| Error::InvalidDownloadAndExtract {
                reason: format!("reading ZIP entry {index} name: {source}"),
            })?
            .into_owned();
        let Some(relative) = entry.enclosed_name() else {
            return Err(Error::InvalidDownloadAndExtract {
                reason: format!("ZIP entry `{name}` escapes the destination"),
            });
        };
        if relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err(Error::InvalidDownloadAndExtract {
                reason: format!("ZIP entry `{name}` escapes the destination"),
            });
        }
        let output = destination.join(&relative);
        let unix_mode = entry.unix_mode().unwrap_or(0);
        let file_type = unix_mode & 0o170_000;
        if file_type != 0
            && file_type != 0o100_000
            && file_type != 0o040_000
            && file_type != 0o120_000
        {
            return Err(Error::InvalidDownloadAndExtract {
                reason: format!("ZIP entry `{name}` has an unsupported file type"),
            });
        }
        if traverses_extracted_symlink(destination, &relative) {
            return Err(Error::InvalidDownloadAndExtract {
                reason: format!("ZIP entry `{name}` is written through a symbolic link"),
            });
        }
        if file_type == 0o120_000 {
            let mut target = String::new();
            let mut bounded = std::io::Read::take(&mut entry, limits.max_metadata_bytes);
            std::io::Read::read_to_string(&mut bounded, &mut target).map_err(|source| {
                Error::InvalidDownloadAndExtract {
                    reason: format!("reading ZIP symbolic link `{name}`: {source}"),
                }
            })?;
            extract_zip_symlink(&relative, Path::new(&target), &output).map_err(|reason| {
                Error::InvalidDownloadAndExtract {
                    reason: format!("ZIP entry `{name}` {reason}"),
                }
            })?;
            continue;
        }
        if entry.is_dir() {
            std::fs::create_dir_all(&output).map_err(|source| Error::FileAction {
                action: "download_and_extract",
                path: output.display().to_string(),
                source,
            })?;
            continue;
        }
        // Refuse an entry that declares more than is left before creating
        // its file; `extract_bounded_file` never writes past the bound for
        // one that lies.
        if entry.size() > remaining {
            return Err(expanded_too_far());
        }
        let written = extract_bounded_file(&mut entry, &name, &output, unix_mode, remaining)?
            .ok_or_else(expanded_too_far)?;
        remaining -= written;
    }
    Ok(())
}

/// Extract one entry, writing at most `remaining` bytes. Returns `None` when
/// the entry holds more than that.
fn extract_bounded_file(
    entry: &mut impl std::io::Read,
    name: &str,
    output: &Path,
    unix_mode: u32,
    remaining: u64,
) -> Result<Option<u64>> {
    let written = extract_zip_file(
        &mut std::io::Read::take(&mut *entry, remaining),
        name,
        output,
        unix_mode,
    )?;
    let mut probe = [0_u8; 1];
    let more = entry.read(&mut probe).map_err(|source| Error::FileAction {
        action: "download_and_extract",
        path: output.display().to_string(),
        source,
    })?;
    Ok((more == 0).then_some(written))
}

fn extract_zip_file(
    entry: &mut impl std::io::Read,
    name: &str,
    output: &Path,
    unix_mode: u32,
) -> Result<u64> {
    let parent = output
        .parent()
        .ok_or_else(|| Error::InvalidDownloadAndExtract {
            reason: format!("ZIP entry `{name}` has no parent directory"),
        })?;
    std::fs::create_dir_all(parent).map_err(|source| Error::FileAction {
        action: "download_and_extract",
        path: parent.display().to_string(),
        source,
    })?;
    let mut output_file = std::fs::File::create(output).map_err(|source| Error::FileAction {
        action: "download_and_extract",
        path: output.display().to_string(),
        source,
    })?;
    let written = std::io::copy(entry, &mut output_file).map_err(|source| Error::FileAction {
        action: "download_and_extract",
        path: output.display().to_string(),
        source,
    })?;
    #[cfg(unix)]
    if unix_mode != 0 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(output, std::fs::Permissions::from_mode(unix_mode & 0o777))
            .map_err(|source| Error::FileAction {
                action: "download_and_extract",
                path: output.display().to_string(),
                source,
            })?;
    }
    #[cfg(not(unix))]
    let _ = unix_mode;
    Ok(written)
}

/// Whether `relative`, or any directory above it, is a symbolic link that an
/// earlier entry of the same archive extracted under `destination`.
fn traverses_extracted_symlink(destination: &Path, relative: &Path) -> bool {
    let mut current = destination.to_path_buf();
    relative.components().any(|component| {
        current.push(component);
        std::fs::symlink_metadata(&current).is_ok_and(|metadata| metadata.file_type().is_symlink())
    })
}

/// Creates the symbolic link an archive stores at `relative`, such as the
/// `Versions/Current` links inside a macOS framework. The target must be
/// relative and stay inside the destination. `..` is accepted only before the
/// first named component, so it climbs real directories rather than out of a
/// directory reached through another link.
fn extract_zip_symlink(
    relative: &Path,
    target: &Path,
    output: &Path,
) -> std::result::Result<(), String> {
    let mut depth = relative.components().count().saturating_sub(1);
    let mut descended = false;
    for component in target.components() {
        match component {
            Component::Normal(_) => descended = true,
            Component::CurDir => {}
            Component::ParentDir if !descended && depth > 0 => depth -= 1,
            _ => {
                return Err(format!(
                    "links to `{}` outside the destination",
                    target.display()
                ))
            }
        }
    }
    let parent = output
        .parent()
        .ok_or_else(|| "has no parent directory".to_string())?;
    std::fs::create_dir_all(parent).map_err(|source| source.to_string())?;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, output).map_err(|source| source.to_string())
    }
    #[cfg(not(unix))]
    {
        Err("is a symbolic link, which this platform cannot extract".to_string())
    }
}

#[cfg(test)]
mod redaction_tests {
    use super::redacted_url;

    #[test]
    fn download_urls_drop_credentials_before_reaching_errors() {
        let url = reqwest::Url::parse(
            "https://user:secret@example.com/archive.zip?X-Amz-Signature=abc&token=t#frag",
        )
        .unwrap();
        let redacted = redacted_url(&url);
        assert_eq!(redacted, "https://example.com/archive.zip?redacted");
        for secret in ["user", "secret", "abc", "token", "frag"] {
            assert!(!redacted.contains(secret), "{redacted} leaks {secret}");
        }
    }

    #[test]
    fn download_urls_without_credentials_are_unchanged() {
        let url = reqwest::Url::parse("https://example.com/a/b.zip").unwrap();
        assert_eq!(redacted_url(&url), "https://example.com/a/b.zip");
    }
}

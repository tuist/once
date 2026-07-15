use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use futures::StreamExt as _;
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncWriteExt as _;
use tokio::sync::Mutex;

use crate::Xdg;

use super::mise::ToolEnvError;

pub const MANAGED_MISE_VERSION: &str = "2026.7.5";

static INSTALL_LOCK: Mutex<()> = Mutex::const_new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MiseAsset {
    filename: &'static str,
    sha256: &'static str,
}

impl MiseAsset {
    fn url(self) -> String {
        format!(
            "https://github.com/jdx/mise/releases/download/v{MANAGED_MISE_VERSION}/{}",
            self.filename
        )
    }
}

pub async fn managed_mise() -> Result<PathBuf, ToolEnvError> {
    let asset = current_asset()?;
    let path = managed_mise_path();
    if path.is_file() {
        return Ok(path);
    }

    let _guard = INSTALL_LOCK.lock().await;
    if path.is_file() {
        return Ok(path);
    }

    install(asset, &path).await?;
    Ok(path)
}

pub fn managed_mise_path() -> PathBuf {
    Xdg::from_env()
        .once_data()
        .join("tools")
        .join("mise")
        .join(MANAGED_MISE_VERSION)
        .join(binary_name())
}

pub(super) fn managed_mise_data_dir() -> PathBuf {
    Xdg::from_env().once_data().join("tools").join("mise-data")
}

pub(super) fn managed_mise_config_dir() -> PathBuf {
    Xdg::from_env()
        .once_data()
        .join("tools")
        .join("mise-config")
}

pub(super) fn managed_mise_cache_dir() -> PathBuf {
    Xdg::from_env().cache_home.join("once").join("mise")
}

async fn install(asset: MiseAsset, destination: &Path) -> Result<(), ToolEnvError> {
    let parent = destination
        .parent()
        .expect("managed mise destination always has a parent");
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|source| ToolEnvError::InstallMiseIo {
            action: "create install directory",
            path: parent.to_path_buf(),
            source,
        })?;

    let temporary = temporary_path(destination);
    if let Err(error) = download(asset, &temporary).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error);
    }
    if let Err(error) = make_executable(&temporary).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error);
    }
    match tokio::fs::rename(&temporary, destination).await {
        Ok(()) => Ok(()),
        Err(_) if destination.is_file() => {
            let _ = tokio::fs::remove_file(&temporary).await;
            Ok(())
        }
        Err(source) => {
            let _ = tokio::fs::remove_file(&temporary).await;
            Err(ToolEnvError::InstallMiseIo {
                action: "activate download",
                path: destination.to_path_buf(),
                source,
            })
        }
    }
}

async fn download(asset: MiseAsset, temporary: &Path) -> Result<(), ToolEnvError> {
    let response = reqwest::get(asset.url())
        .await
        .map_err(|source| ToolEnvError::DownloadMise { source })?;
    let status = response.status();
    if !status.is_success() {
        return Err(ToolEnvError::MiseDownloadStatus { status });
    }
    let mut file =
        tokio::fs::File::create(temporary)
            .await
            .map_err(|source| ToolEnvError::InstallMiseIo {
                action: "write download",
                path: temporary.to_path_buf(),
                source,
            })?;
    let mut hasher = Sha256::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|source| ToolEnvError::DownloadMise { source })?;
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|source| ToolEnvError::InstallMiseIo {
                action: "write download",
                path: temporary.to_path_buf(),
                source,
            })?;
    }
    file.flush()
        .await
        .map_err(|source| ToolEnvError::InstallMiseIo {
            action: "flush download",
            path: temporary.to_path_buf(),
            source,
        })?;
    verify_digest(asset, &hasher.finalize())
}

#[cfg(test)]
fn verify_checksum(asset: MiseAsset, bytes: &[u8]) -> Result<(), ToolEnvError> {
    verify_digest(asset, &Sha256::digest(bytes))
}

fn verify_digest(asset: MiseAsset, digest: &[u8]) -> Result<(), ToolEnvError> {
    let mut actual = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut actual, "{byte:02x}").expect("writing to a String cannot fail");
    }
    if actual == asset.sha256 {
        return Ok(());
    }
    Err(ToolEnvError::MiseChecksum {
        asset: asset.filename,
        expected: asset.sha256,
        actual,
    })
}

fn temporary_path(destination: &Path) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    destination.with_extension(format!("download-{}-{timestamp}", std::process::id()))
}

#[cfg(unix)]
async fn make_executable(path: &Path) -> Result<(), ToolEnvError> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = tokio::fs::metadata(path)
        .await
        .map_err(|source| ToolEnvError::InstallMiseIo {
            action: "read download permissions",
            path: path.to_path_buf(),
            source,
        })?
        .permissions();
    permissions.set_mode(0o755);
    tokio::fs::set_permissions(path, permissions)
        .await
        .map_err(|source| ToolEnvError::InstallMiseIo {
            action: "set download permissions",
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(not(unix))]
async fn make_executable(_path: &Path) -> Result<(), ToolEnvError> {
    Ok(())
}

const fn binary_name() -> &'static str {
    if cfg!(windows) {
        "mise.exe"
    } else {
        "mise"
    }
}

fn current_asset() -> Result<MiseAsset, ToolEnvError> {
    asset_for(std::env::consts::OS, std::env::consts::ARCH).ok_or_else(|| {
        ToolEnvError::UnsupportedMisePlatform {
            operating_system: std::env::consts::OS,
            architecture: std::env::consts::ARCH,
        }
    })
}

fn asset_for(operating_system: &str, architecture: &str) -> Option<MiseAsset> {
    match (operating_system, architecture) {
        ("linux", "x86_64") => Some(MiseAsset {
            filename: "mise-v2026.7.5-linux-x64",
            sha256: "5f7ab76afdf0780d12edeaa67e908094e9ccf7924cfe203e415c1cfb87bbf778",
        }),
        ("linux", "aarch64") => Some(MiseAsset {
            filename: "mise-v2026.7.5-linux-arm64",
            sha256: "41fcf744050bfa27f9871e2151ac6f44b5ce2741424b3d5282b92becc71e6bc4",
        }),
        ("macos", "x86_64") => Some(MiseAsset {
            filename: "mise-v2026.7.5-macos-x64",
            sha256: "62fe1fe9dbc32c6ce1388ee23df4a0862d3d7f40a6820b40c2f1cbab995dc1d4",
        }),
        ("macos", "aarch64") => Some(MiseAsset {
            filename: "mise-v2026.7.5-macos-arm64",
            sha256: "a456c65907e8334619d77fa152bdcf9023fddc0daa03d47fbe86d032dbf565b0",
        }),
        ("windows", "x86_64") => Some(MiseAsset {
            filename: "mise-v2026.7.5-windows-x64.exe",
            sha256: "1840f167ec8b161598e08b8ede769cf9954c0239b25bb7bdf0b326124b548c32",
        }),
        ("windows", "aarch64") => Some(MiseAsset {
            filename: "mise-v2026.7.5-windows-arm64.exe",
            sha256: "27d3279d9d6a994d910561706f5ca99abcd8a03d38ad15b73ff0b5b3e148e4ea",
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_platforms_have_pinned_assets() {
        assert!(asset_for("linux", "x86_64").is_some());
        assert!(asset_for("macos", "aarch64").is_some());
        assert!(asset_for("windows", "x86_64").is_some());
    }

    #[test]
    fn checksum_verification_rejects_changed_bytes() {
        let asset = MiseAsset {
            filename: "mise-test",
            sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        };
        assert!(verify_checksum(asset, b"").is_ok());
        assert!(matches!(
            verify_checksum(asset, b"changed"),
            Err(ToolEnvError::MiseChecksum { .. })
        ));
    }
}

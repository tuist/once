use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Read;
use std::sync::Arc;

use tokio::process::Command;

use super::{error, io_error, Client};
use crate::{RemoteIoLimits, Result};

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;

pub(super) struct Connection {
    environment: BTreeMap<OsString, OsString>,
    config: Option<tempfile::TempDir>,
    daemon: String,
    credential: String,
}

fn credential(exported: &[u8]) -> Result<String> {
    // Docker exports TLS files in map iteration order. Hash their contents in
    // canonical path order so an unchanged credential survives another export.
    let mut files = BTreeMap::new();
    // The CLI can omit the last entry's block padding and end-of-archive blocks.
    // This reader is only for hashing; Docker imports the unmodified export.
    let mut archive = tar::Archive::new(exported.chain(std::io::repeat(0).take(3 * 512)));
    for entry in archive.entries().map_err(io_error)? {
        let mut entry = entry.map_err(io_error)?;
        if entry.header().entry_type().is_file() {
            let path = entry.path_bytes().into_owned();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).map_err(io_error)?;
            if files.insert(path, blake3::hash(&bytes)).is_some() {
                return Err(error("duplicate Docker context snapshot entry"));
            }
        }
    }
    let mut digest = blake3::Hasher::new();
    for (path, hash) in files {
        digest.update(&(path.len() as u64).to_le_bytes());
        digest.update(&path);
        digest.update(hash.as_bytes());
    }
    Ok(digest.finalize().to_hex().to_string())
}

impl Connection {
    pub fn configure(&self, command: &mut Command) {
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("DOCKER_") {
                command.env_remove(key);
            }
        }
        command.envs(&self.environment);
        if let Some(config) = &self.config {
            command
                .arg("--config")
                .arg(config.path())
                .args(["--context", "once-pinned"]);
        }
    }
}

impl Client {
    pub fn new(binary: OsString, limits: RemoteIoLimits) -> Self {
        Self {
            binary,
            limits,
            connection: Arc::new(Connection {
                environment: std::env::vars_os()
                    .filter(|(key, _)| key.to_string_lossy().starts_with("DOCKER_"))
                    .collect(),
                config: None,
                daemon: String::new(),
                credential: String::new(),
            }),
        }
    }

    pub async fn connect(self) -> Result<Self> {
        let mut client = self.snapshot().await?;
        let daemon = super::super::capacity::identity(&client).await?;
        Arc::get_mut(&mut client.connection)
            .expect("unshared connection")
            .daemon = daemon;
        Ok(client)
    }

    async fn snapshot(mut self) -> Result<Self> {
        let context = self.control(&["context".into(), "show".into()]).await?;
        let context = std::str::from_utf8(&context)
            .map_err(|_| error("invalid Docker context name"))?
            .trim();
        if context.is_empty() || context.starts_with('-') || context.contains('\n') {
            return Err(error("invalid Docker context name"));
        }
        // Export includes the resolved endpoint and TLS material, including
        // the environment-derived default context. Never retain a context name
        // that can subsequently be redirected by the user's configuration.
        let exported = self
            .control(&[
                "context".into(),
                "export".into(),
                context.into(),
                "-".into(),
            ])
            .await?;
        let config = tempfile::tempdir().map_err(io_error)?;
        let archive = config.path().join("context.tar");
        tokio::fs::write(&archive, &exported)
            .await
            .map_err(io_error)?;
        let credential = credential(&exported)?;
        let mut environment = self.connection.environment.clone();
        for key in [
            "DOCKER_CONTEXT",
            "DOCKER_HOST",
            "DOCKER_CONFIG",
            "DOCKER_TLS",
            "DOCKER_TLS_VERIFY",
            "DOCKER_CERT_PATH",
        ] {
            environment.remove(std::ffi::OsStr::new(key));
        }
        environment.insert("DOCKER_CONFIG".into(), config.path().as_os_str().into());
        self.connection = Arc::new(Connection {
            environment,
            config: None,
            daemon: String::new(),
            credential,
        });
        self.control(&[
            "context".into(),
            "import".into(),
            "once-pinned".into(),
            archive
                .to_str()
                .ok_or_else(|| error("invalid Docker context snapshot path"))?
                .into(),
        ])
        .await?;
        tokio::fs::remove_file(archive).await.map_err(io_error)?;
        Arc::get_mut(&mut self.connection)
            .expect("unshared connection")
            .config = Some(config);
        Ok(self)
    }

    pub fn daemon(&self) -> &str {
        assert!(
            !self.connection.daemon.is_empty(),
            "Docker client must be pinned"
        );
        &self.connection.daemon
    }

    pub fn credential(&self) -> &str {
        &self.connection.credential
    }

    pub async fn verify_daemon(&self) -> Result<()> {
        self.check_identity(&super::super::capacity::identity(self).await?)
    }

    pub fn check_identity(&self, id: &str) -> Result<()> {
        if id != self.daemon() {
            return Err(error(
                "Docker daemon identity changed; retaining container ownership",
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    pub async fn identify(mut self) -> Result<Self> {
        let daemon = super::super::capacity::identity(&self).await?;
        Arc::get_mut(&mut self.connection)
            .expect("unshared connection")
            .daemon = daemon;
        Ok(self)
    }
}

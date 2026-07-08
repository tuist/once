//! Dynamic OAuth client registration and its on-disk persistence. When a
//! workspace does not pin an `oauth_client_id`, Once registers a client
//! with Tuist and remembers it under the credentials root so later logins
//! and token refreshes reuse the same identity.

use std::fs;
use std::path::PathBuf;

use schlussel::{ClientMetadata, DynamicRegistrationClient};
use serde::{Deserialize, Serialize};

use super::{TuistAuth, PROVIDER_NAME};
use crate::{Digest, Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct RegisteredClient {
    pub(super) client_id: String,
}

impl TuistAuth {
    pub(super) fn register_client(&self, redirect_uri: &str) -> Result<RegisteredClient> {
        let client =
            DynamicRegistrationClient::new(self.registration_endpoint()?).map_err(|source| {
                Error::InvalidConfig {
                    provider: PROVIDER_NAME,
                    message: format!("invalid Tuist registration endpoint: {source}"),
                }
            })?;
        let response = client
            .register(&ClientMetadata {
                client_name: "once".to_string(),
                redirect_uris: vec![redirect_uri.to_string()],
                grant_types: vec![
                    "authorization_code".to_string(),
                    "refresh_token".to_string(),
                ],
                response_types: vec!["code".to_string()],
                token_endpoint_auth_method: Some("none".to_string()),
                ..ClientMetadata::default()
            })
            .map_err(|source| Self::remote_auth_error("register auth client", &source))?;
        let client_id = response.client_id.trim();
        if client_id.is_empty() {
            return Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation: "register auth client",
                message: "Tuist returned an empty client_id".to_string(),
            });
        }
        Ok(RegisteredClient {
            client_id: client_id.to_string(),
        })
    }

    pub(super) fn resolve_client_id(&self) -> Result<String> {
        if let Some(client_id) = self.config.oauth_client_id.clone() {
            return Ok(client_id);
        }
        let registered_client =
            self.load_registered_client()?
                .ok_or_else(|| Error::InvalidConfig {
                    provider: PROVIDER_NAME,
                    message: self.login_hint(),
                })?;
        Ok(registered_client.client_id)
    }

    fn load_registered_client(&self) -> Result<Option<RegisteredClient>> {
        let path = self.registration_path();
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(Error::Remote {
                    provider: PROVIDER_NAME,
                    operation: "load auth registration",
                    message: source.to_string(),
                });
            }
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|source| Error::InvalidConfig {
                provider: PROVIDER_NAME,
                message: format!(
                    "corrupt Tuist auth registration at {}: {source}",
                    path.display()
                ),
            })
    }

    pub(super) fn save_registered_client(
        &self,
        registered_client: &RegisteredClient,
    ) -> Result<()> {
        let path = self.registration_path();
        let Some(parent) = path.parent() else {
            return Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation: "store auth registration",
                message: "registration path had no parent directory".to_string(),
            });
        };
        fs::create_dir_all(parent).map_err(|source| Error::Remote {
            provider: PROVIDER_NAME,
            operation: "store auth registration",
            message: source.to_string(),
        })?;
        let bytes =
            serde_json::to_vec_pretty(registered_client).map_err(|source| Error::Remote {
                provider: PROVIDER_NAME,
                operation: "store auth registration",
                message: source.to_string(),
            })?;
        fs::write(path, bytes).map_err(|source| Error::Remote {
            provider: PROVIDER_NAME,
            operation: "store auth registration",
            message: source.to_string(),
        })
    }

    pub(super) fn delete_registered_client(&self) -> Result<()> {
        let path = self.registration_path();
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation: "delete auth registration",
                message: source.to_string(),
            }),
        }
    }

    fn registration_path(&self) -> PathBuf {
        let key = Digest::of_bytes(self.identity().as_bytes()).to_hex();
        self.credentials_root
            .join("registrations")
            .join(format!("{key}.json"))
    }
}

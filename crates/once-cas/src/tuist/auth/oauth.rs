//! Interactive OAuth authorization-code login and reads of the stored
//! token. This drives the browser-based `once auth login` flow (PKCE,
//! loopback callback, code exchange) and loads or refreshes the token
//! persisted for a workspace.

use schlussel::callback::{build_authorization_url, open_browser};
use schlussel::{
    CallbackServer, FileStorage, OAuthClient, OAuthConfig, PkcePair, SchlusselError,
    SessionStorage, Token, TokenRefresher,
};

use super::{TuistAuth, TuistAuthPrompt, OIDC_TOKEN_REFRESH_WINDOW_SECONDS, PROVIDER_NAME};
use crate::{Error, Result};

impl TuistAuth {
    pub(super) fn load_valid_token(&self) -> Result<Token> {
        let key = self.storage_key();
        let storage = self.storage()?;
        let client = self.oauth_client(
            storage,
            self.resolve_client_id()?,
            "http://127.0.0.1/callback".to_string(),
        )?;
        let refresher = TokenRefresher::new(client)
            .with_file_locking("once")
            .map_err(|source| Self::remote_auth_error("configure auth refresh", &source))?;
        refresher
            .get_valid_token(&key)
            .map_err(|source| self.cached_auth_error(source))
    }

    pub(super) fn load_stored_access_token(&self) -> Result<Option<String>> {
        let key = self.storage_key();
        let Some(token) = self
            .storage()?
            .load(&key)
            .map_err(|source| Self::remote_auth_error("load auth token", &source))?
        else {
            return Ok(None);
        };
        if token.expires_within(OIDC_TOKEN_REFRESH_WINDOW_SECONDS) {
            return Ok(None);
        }
        Ok(Some(token.access_token))
    }

    pub(super) fn login_with_client_id(
        &self,
        client_id: &str,
        open_browser_after_prompt: bool,
        handler: &mut dyn FnMut(TuistAuthPrompt),
    ) -> Result<()> {
        let server = CallbackServer::new(0)
            .map_err(|source| Self::remote_auth_error("start callback", &source))?;
        let redirect_uri = server.callback_url();
        self.authorize(
            client_id,
            &redirect_uri,
            &server,
            open_browser_after_prompt,
            handler,
        )
    }

    pub(super) fn authorize(
        &self,
        client_id: &str,
        redirect_uri: &str,
        server: &CallbackServer,
        open_browser_after_prompt: bool,
        handler: &mut dyn FnMut(TuistAuthPrompt),
    ) -> Result<()> {
        let pkce = PkcePair::generate();
        let state = random_state();
        let authorize_url = build_authorization_url(
            &self.authorization_endpoint()?,
            client_id,
            redirect_uri,
            None,
            &state,
            pkce.challenge(),
        )
        .map_err(|source| Self::remote_auth_error("build authorization URL", &source))?;

        handler(TuistAuthPrompt {
            authorize_url: authorize_url.clone(),
            redirect_uri: redirect_uri.to_string(),
            opens_browser: open_browser_after_prompt,
        });
        if open_browser_after_prompt {
            open_browser(&authorize_url)
                .map_err(|source| Self::remote_auth_error("open browser", &source))?;
        }

        let callback = server
            .wait_for_callback(120)
            .map_err(|source| Self::remote_auth_error("wait for auth callback", &source))?;
        if callback.state.as_deref() != Some(state.as_str()) {
            return Err(Self::remote_auth_error(
                "authorize",
                &SchlusselError::InvalidState,
            ));
        }
        if callback.error_code.is_some() {
            return Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation: "authorize",
                message: callback
                    .error_description
                    .or(callback.error_code)
                    .unwrap_or_else(|| "authorization denied".to_string()),
            });
        }
        let code = callback.code.ok_or_else(|| Error::Remote {
            provider: PROVIDER_NAME,
            operation: "authorize",
            message: "missing authorization code".to_string(),
        })?;

        let key = self.storage_key();
        let storage = self.storage()?;
        let client = self.oauth_client(storage, client_id.to_string(), redirect_uri.to_string())?;
        let token = client
            .exchange_code(&code, pkce.verifier(), redirect_uri)
            .map_err(|source| Self::remote_auth_error("exchange authorization code", &source))?;
        client
            .save_token(&key, &token)
            .map_err(|source| Self::remote_auth_error("store auth token", &source))?;
        Ok(())
    }

    fn oauth_client(
        &self,
        storage: FileStorage,
        client_id: String,
        redirect_uri: String,
    ) -> Result<OAuthClient<FileStorage>> {
        OAuthClient::new(self.oauth_config(client_id, redirect_uri)?, storage).map_err(|source| {
            Error::InvalidConfig {
                provider: PROVIDER_NAME,
                message: format!("invalid Tuist auth configuration: {source}"),
            }
        })
    }

    fn oauth_config(&self, client_id: String, redirect_uri: String) -> Result<OAuthConfig> {
        Ok(OAuthConfig {
            client_id,
            client_secret: None,
            authorization_endpoint: self.authorization_endpoint()?,
            token_endpoint: self.token_endpoint()?,
            redirect_uri,
            scope: None,
            device_authorization_endpoint: None,
        })
    }
}

fn random_state() -> String {
    PkcePair::generate().verifier()[..22].to_string()
}

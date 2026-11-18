//! What an authentication failure means for retrying.
//!
//! A credential service that cannot answer right now is unavailable, so the
//! remote cache can recover once it answers again. A credential the service
//! rejected is refused, and a missing or unusable configuration is
//! misconfigured; neither changes by asking again.

use std::fmt::Write as _;

use reqwest::StatusCode;
use schlussel::SchlusselError;

use super::super::status;
use crate::{Error, ErrorKind};

pub(super) fn schlussel_kind(error: &SchlusselError) -> ErrorKind {
    match error {
        SchlusselError::Http(_)
        | SchlusselError::Timeout
        | SchlusselError::Lock(_)
        | SchlusselError::AuthorizationPending
        | SchlusselError::SlowDown => ErrorKind::Unavailable,
        SchlusselError::Server { status, code, .. } => server_kind(*status, code.as_deref()),
        SchlusselError::AuthorizationDenied
        | SchlusselError::InvalidState
        | SchlusselError::DeviceCodeExpired
        | SchlusselError::TokenExpired
        | SchlusselError::NoRefreshToken
        | SchlusselError::TokenNotFound(_) => ErrorKind::Refused,
        SchlusselError::Storage(_) | SchlusselError::Io(_) | SchlusselError::CallbackServer(_) => {
            ErrorKind::Io
        }
        SchlusselError::Json(_) => ErrorKind::Protocol,
        SchlusselError::InvalidParameter(_)
        | SchlusselError::Configuration(_)
        | SchlusselError::UnsupportedOperation(_)
        | SchlusselError::InsecureEndpoint(_)
        | SchlusselError::MissingClientId
        | SchlusselError::MissingEndpoint(_)
        | SchlusselError::MethodNotFound(_)
        | SchlusselError::FormulaNotFound(_) => ErrorKind::Misconfigured,
    }
}

/// OAuth error codes name a rejected grant or client regardless of the HTTP
/// status the server chose; otherwise the status decides.
fn server_kind(status: Option<u16>, code: Option<&str>) -> ErrorKind {
    match code {
        Some(
            "invalid_grant"
            | "invalid_client"
            | "unauthorized_client"
            | "access_denied"
            | "invalid_token",
        ) => return ErrorKind::Refused,
        Some("temporarily_unavailable" | "server_error" | "slow_down") => {
            return ErrorKind::Unavailable
        }
        _ => {}
    }
    match status.and_then(|status| StatusCode::from_u16(status).ok()) {
        Some(status) => status::http_kind(status),
        None => ErrorKind::Unavailable,
    }
}

/// OAuth error codes are a fixed vocabulary and safe to repeat; anything
/// else a credential service wrote is not.
const OAUTH_CODES: &[&str] = &[
    "invalid_request",
    "invalid_client",
    "invalid_grant",
    "unauthorized_client",
    "unsupported_grant_type",
    "invalid_scope",
    "access_denied",
    "invalid_token",
    "server_error",
    "temporarily_unavailable",
    "slow_down",
];

/// A credential failure described without text a server or a stored
/// credential supplied, which can contain tokens.
pub(super) fn describe(error: &SchlusselError) -> String {
    match error {
        SchlusselError::Http(_) => "the credential service request failed".into(),
        SchlusselError::Timeout => "the credential service timed out".into(),
        SchlusselError::Lock(_) => "the credential refresh lock is unavailable".into(),
        SchlusselError::Server { status, code, .. } => {
            let mut message = match status {
                Some(status) => format!("the credential service answered HTTP {status}"),
                None => "the credential service returned an error".into(),
            };
            if let Some(code) = code.as_deref().filter(|code| OAUTH_CODES.contains(code)) {
                let _ = write!(message, " ({code})");
            }
            message
        }
        SchlusselError::Json(_) => "the credential service response was malformed".into(),
        SchlusselError::Storage(_) | SchlusselError::Io(_) => {
            "the credential store could not be read or written".into()
        }
        SchlusselError::CallbackServer(_) => "the authorization callback failed".into(),
        // The remaining variants carry fixed text or local configuration.
        other => other.to_string(),
    }
}

/// An OAuth callback error described by its code when it is a standard one.
pub(super) fn callback_error(code: Option<&str>) -> String {
    match code.filter(|code| OAUTH_CODES.contains(code)) {
        Some(code) => format!("authorization was not granted ({code})"),
        None => "authorization was not granted".into(),
    }
}

pub(super) fn auth_error(
    operation: &'static str,
    kind: ErrorKind,
    message: impl Into<String>,
) -> Error {
    status::remote(operation, kind, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_credential_service_failures_are_unavailable() {
        for error in [
            SchlusselError::Http("connection reset".into()),
            SchlusselError::Timeout,
            SchlusselError::Lock("refresh lock busy".into()),
            SchlusselError::server(Some(503), None, None),
            SchlusselError::server(Some(429), None, None),
            SchlusselError::server(None, None, None),
            SchlusselError::server(Some(400), Some("temporarily_unavailable".into()), None),
        ] {
            assert_eq!(schlussel_kind(&error), ErrorKind::Unavailable, "{error:?}");
        }
    }

    #[test]
    fn rejected_credentials_are_refused() {
        for error in [
            SchlusselError::server(Some(400), Some("invalid_grant".into()), None),
            SchlusselError::server(Some(401), None, None),
            SchlusselError::server(Some(403), None, None),
            SchlusselError::server(Some(500), Some("invalid_client".into()), None),
            SchlusselError::AuthorizationDenied,
            SchlusselError::TokenExpired,
        ] {
            assert_eq!(schlussel_kind(&error), ErrorKind::Refused, "{error:?}");
        }
    }

    #[test]
    fn descriptions_never_repeat_what_a_server_or_store_supplied() {
        let secret = crate::tuist::redact::SENTINEL;
        for error in [
            SchlusselError::Http(format!("https://x.test/token?code={secret}")),
            SchlusselError::server(Some(400), Some(secret.into()), Some(secret.into())),
            SchlusselError::Json(format!("invalid type: string \"{secret}\"")),
            SchlusselError::Storage(secret.into()),
            SchlusselError::Io(secret.into()),
            SchlusselError::Lock(secret.into()),
            SchlusselError::CallbackServer(secret.into()),
        ] {
            assert!(!describe(&error).contains(secret), "{error:?}");
        }
        assert_eq!(
            describe(&SchlusselError::server(
                Some(400),
                Some("invalid_grant".into()),
                None
            )),
            "the credential service answered HTTP 400 (invalid_grant)"
        );
        assert!(!callback_error(Some(secret)).contains(secret));
    }

    #[test]
    fn local_and_configuration_failures_keep_their_own_kinds() {
        assert_eq!(
            schlussel_kind(&SchlusselError::Storage("disk full".into())),
            ErrorKind::Io
        );
        assert_eq!(
            schlussel_kind(&SchlusselError::Json("bad".into())),
            ErrorKind::Protocol
        );
        assert_eq!(
            schlussel_kind(&SchlusselError::MissingClientId),
            ErrorKind::Misconfigured
        );
    }
}

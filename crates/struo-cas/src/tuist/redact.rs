//! What may be said about a remote in errors and logs.
//!
//! Endpoint URLs can carry credentials in their user information, query, or
//! path, and anything a server sends back (status messages, response bodies,
//! even the text of a decoding error about them) can echo tokens. Errors and
//! log fields therefore name endpoints by scheme, host, and port only, and
//! describe remote failures by category and status code, never by the text
//! the remote returned.

use reqwest::Url;

/// `scheme://host[:port]` of an endpoint, without user information, path,
/// query, or fragment.
pub(super) fn endpoint(raw: &str) -> String {
    let normalized = super::cache::grpc_endpoint_url(raw);
    match Url::parse(&normalized) {
        Ok(url) => match (url.host_str(), url.port()) {
            (Some(host), Some(port)) => format!("{}://{host}:{port}", url.scheme()),
            (Some(host), None) => format!("{}://{host}", url.scheme()),
            (None, _) => "an endpoint without a host".to_string(),
        },
        Err(_) => "an unparsable endpoint".to_string(),
    }
}

/// A transport failure described without the request URL it carries.
pub(super) fn transport(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "the request timed out".to_string()
    } else if error.is_connect() {
        "the connection failed".to_string()
    } else if error.is_body() || error.is_decode() {
        "the response body could not be read".to_string()
    } else {
        "the request failed".to_string()
    }
}

/// An HTTP failure described by its status alone.
pub(super) fn http_status(status: reqwest::StatusCode) -> String {
    format!("HTTP {}", status.as_u16())
}

#[cfg(test)]
pub(super) const SENTINEL: &str = "SENTINEL-c0ffee-secret";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_keep_only_scheme_host_and_port() {
        for (raw, expected) in [
            (
                "https://user:SENTINEL-c0ffee-secret@cache.example.test:8443/p/SENTINEL-c0ffee-secret?token=SENTINEL-c0ffee-secret#SENTINEL-c0ffee-secret",
                "https://cache.example.test:8443",
            ),
            ("grpcs://cache.example.test", "https://cache.example.test"),
            ("grpc://127.0.0.1:5101", "http://127.0.0.1:5101"),
            ("not a url SENTINEL-c0ffee-secret", "an unparsable endpoint"),
        ] {
            let redacted = endpoint(raw);
            assert_eq!(redacted, expected);
            assert!(!redacted.contains(SENTINEL));
        }
    }
}

//! Client for the `/.well-known/once` service discovery document.
//!
//! The discovery document is how a server that speaks the Once event
//! protocol tells clients where each protocol lives. Today we honor the
//! `events` protocol so the CLI's invocation reporter can be pointed at a
//! dedicated ingest fleet without a client release. Absent fields mean "not
//! offered" - callers fall back to whatever URL they used before discovery
//! existed.
//!
//! The convention is Once's, not the server implementation's, so the same
//! shape works whether the server is Tuist or any other backend that adopts
//! the Once event protocol.
//!
//! The endpoint list for each protocol is always a list so the client picks
//! one per its own policy. This module implements the simplest possible
//! policy - first-listed - and leaves latency probing, region hints, or
//! failover for a follow-up.
//!
//! The discovery request is a best-effort GET with a bounded timeout. Any
//! failure (server not upgraded, network hiccup, invalid JSON) surfaces as
//! `Ok(None)` so callers can transparently fall back.

use std::time::Duration;

use serde::Deserialize;

const DISCOVERY_PATH: &str = ".well-known/once";
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(2);

/// A single endpoint the server advertises for a protocol.
#[derive(Debug, Clone, Deserialize)]
pub struct Endpoint {
    /// URL a client should reach for this protocol.
    pub url: String,
    /// Optional region hint. Not consumed today; carried through so a future
    /// selector can honor it without a wire change.
    #[serde(default)]
    #[allow(dead_code, reason = "surfaced for a future region-aware selector")]
    pub region: Option<String>,
}

/// A protocol section in the discovery document.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Protocol {
    #[serde(default)]
    pub endpoints: Vec<Endpoint>,
}

impl Protocol {
    /// First advertised URL, or `None` when the list is empty.
    pub fn first_url(&self) -> Option<&str> {
        self.endpoints.first().map(|endpoint| endpoint.url.as_str())
    }
}

/// The subset of the discovery document this client consumes today.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Discovery {
    #[serde(default)]
    pub events: Protocol,
}

/// Fetch the discovery document from `base_url` best-effort. Returns
/// `Ok(None)` when the server does not implement discovery (any HTTP error,
/// non-2xx response, or malformed JSON) so callers can fall back cleanly.
pub async fn fetch(base_url: &str) -> Result<Option<Discovery>, reqwest::Error> {
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/{DISCOVERY_PATH}");
    let client = reqwest::Client::builder()
        .timeout(DISCOVERY_TIMEOUT)
        .build()?;
    let response = match client.get(&url).send().await {
        Ok(response) => response,
        Err(error) => {
            tracing::debug!(%error, "Once discovery request failed");
            return Ok(None);
        }
    };
    if !response.status().is_success() {
        tracing::debug!(
            status = response.status().as_u16(),
            "Once discovery not available at {url}"
        );
        return Ok(None);
    }
    match response.json::<Discovery>().await {
        Ok(discovery) => Ok(Some(discovery)),
        Err(error) => {
            tracing::debug!(%error, "Once discovery returned an unparseable body");
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_parses_the_events_section() {
        let body = r#"{
            "events": {
                "endpoints": [
                    {"url": "https://ingest.tuist.dev", "region": "eu-west"},
                    {"url": "https://ingest-us.tuist.dev"}
                ]
            }
        }"#;

        let discovery: Discovery = serde_json::from_str(body).unwrap();
        assert_eq!(discovery.events.endpoints.len(), 2);
        assert_eq!(
            discovery.events.first_url(),
            Some("https://ingest.tuist.dev")
        );
        assert_eq!(
            discovery.events.endpoints[0].region.as_deref(),
            Some("eu-west")
        );
    }

    #[test]
    fn discovery_defaults_missing_events() {
        let discovery: Discovery = serde_json::from_str("{}").unwrap();
        assert!(discovery.events.endpoints.is_empty());
        assert_eq!(discovery.events.first_url(), None);
    }

    #[test]
    fn protocol_first_url_returns_none_for_empty_list() {
        let protocol = Protocol::default();
        assert_eq!(protocol.first_url(), None);
    }
}

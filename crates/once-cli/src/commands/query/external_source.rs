use std::io::Read;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};

use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::Client;
use reqwest::redirect::Policy;
use serde::Serialize;

const MAX_MAX_BYTES: usize = 1024 * 1024;

#[derive(Debug, Serialize)]
pub struct ExternalSource {
    pub url: String,
    pub content_type: Option<String>,
    pub content_digest: String,
    pub byte_count: usize,
    pub truncated: bool,
    pub content: String,
}

pub fn fetch(url: &str, max_bytes: usize) -> Result<ExternalSource> {
    validate_max_bytes(max_bytes)?;
    let parsed = validate_url(url)?;
    let addresses = resolved_public_addresses(&parsed)?;
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow!("external source web address is missing a host"))?;
    let client = Client::builder()
        .redirect(Policy::none())
        .user_agent(concat!("once/", env!("CARGO_PKG_VERSION")))
        .resolve_to_addrs(host, &addresses)
        .build()
        .context("creating external source client")?;
    let response = client
        .get(parsed.clone())
        .send()
        .with_context(|| format!("fetching external source `{url}`"))?;
    if response.status().is_redirection() {
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("<missing>");
        bail!(
            "external source redirected to `{location}`; fetch that final HTTPS address explicitly"
        );
    }
    let response = response
        .error_for_status()
        .with_context(|| format!("fetching external source `{url}`"))?;
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let mut bytes = Vec::new();
    response
        .take((max_bytes + 1) as u64)
        .read_to_end(&mut bytes)
        .context("reading external source")?;
    let truncated = bytes.len() > max_bytes;
    bytes.truncate(max_bytes);
    let content = String::from_utf8(bytes.clone())
        .map_err(|_| anyhow!("external source is not UTF-8 text"))?;
    Ok(ExternalSource {
        url: parsed.to_string(),
        content_type,
        content_digest: blake3::hash(&bytes).to_hex().to_string(),
        byte_count: bytes.len(),
        truncated,
        content,
    })
}

fn validate_max_bytes(max_bytes: usize) -> Result<()> {
    if !(1..=MAX_MAX_BYTES).contains(&max_bytes) {
        bail!("`max_bytes` must be between 1 and {MAX_MAX_BYTES}");
    }
    Ok(())
}

fn validate_url(url: &str) -> Result<reqwest::Url> {
    let parsed = reqwest::Url::parse(url).context("parsing external source web address")?;
    if parsed.scheme() != "https" {
        bail!("external source web address must use HTTPS");
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow!("external source web address is missing a host"))?;
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        bail!("external source host must be public");
    }
    let address_literal = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    if let Ok(address) = address_literal.parse::<IpAddr>() {
        validate_public_ip(address)?;
    }
    Ok(parsed)
}

fn resolved_public_addresses(url: &reqwest::Url) -> Result<Vec<SocketAddr>> {
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("external source web address is missing a host"))?;
    let port = url.port_or_known_default().unwrap_or(443);
    let addresses = (host, port)
        .to_socket_addrs()
        .with_context(|| format!("resolving external source host `{host}`"))?
        .collect::<Vec<_>>();
    if addresses.is_empty() {
        bail!("external source host `{host}` did not resolve");
    }
    for address in &addresses {
        validate_public_ip(address.ip())?;
    }
    Ok(addresses)
}

fn validate_public_ip(address: IpAddr) -> Result<()> {
    let blocked = match address {
        IpAddr::V4(address) => {
            address.is_private()
                || address.is_loopback()
                || address.is_link_local()
                || address.is_broadcast()
                || address.is_documentation()
                || address.is_multicast()
                || address.is_unspecified()
        }
        IpAddr::V6(address) => {
            if let Some(address) = address.to_ipv4_mapped() {
                return validate_public_ip(IpAddr::V4(address));
            }
            let segments = address.segments();
            address.is_loopback()
                || address.is_unspecified()
                || address.is_multicast()
                || (segments[0] & 0xfe00) == 0xfc00
                || (segments[0] & 0xffc0) == 0xfe80
                || (segments[0] == 0x2001 && segments[1] == 0x0db8)
        }
    };
    if blocked {
        bail!("external source address must be public");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_https_and_local_addresses() {
        assert!(validate_url("http://example.com/rule").is_err());
        assert!(validate_url("https://localhost/rule").is_err());
        assert!(validate_url("https://127.0.0.1/rule").is_err());
        assert!(validate_url("https://10.0.0.1/rule").is_err());
        assert!(validate_url("https://[::ffff:127.0.0.1]/rule").is_err());
        assert!(validate_url("https://[2001:db8::1]/rule").is_err());
    }

    #[test]
    fn accepts_public_https_addresses() {
        assert!(validate_url("https://example.com/rule.bzl").is_ok());
    }

    #[test]
    fn validates_byte_limit() {
        assert!(validate_max_bytes(1).is_ok());
        assert!(validate_max_bytes(MAX_MAX_BYTES).is_ok());
        assert!(validate_max_bytes(0).is_err());
        assert!(validate_max_bytes(MAX_MAX_BYTES + 1).is_err());
    }
}

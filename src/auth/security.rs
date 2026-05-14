use std::net::IpAddr;

use crate::error::{Error, Result};

/// Validate token endpoints before transmitting credentials or assertions.
pub(crate) fn secure_token_endpoint_url(raw_url: &str, field: &str) -> Result<reqwest::Url> {
    let url =
        reqwest::Url::parse(raw_url).map_err(|e| Error::config(format!("invalid {field}: {e}")))?;

    if url.scheme() == "https" || is_loopback_http_url(&url) {
        return Ok(url);
    }

    Err(Error::config(format!(
        "{field} must use https unless it points to localhost or a loopback IP"
    )))
}

fn is_loopback_http_url(url: &reqwest::Url) -> bool {
    if url.scheme() != "http" {
        return false;
    }

    match url.host_str() {
        Some("localhost") => true,
        Some(host) => host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback()),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secure_token_endpoint_allows_https() {
        secure_token_endpoint_url("https://example.com/token", "token_uri").unwrap();
    }

    #[test]
    fn secure_token_endpoint_allows_loopback_http() {
        secure_token_endpoint_url("http://127.0.0.1:1234/token", "token_uri").unwrap();
        secure_token_endpoint_url("http://[::1]:1234/token", "token_uri").unwrap();
        secure_token_endpoint_url("http://localhost:1234/token", "token_uri").unwrap();
    }

    #[test]
    fn secure_token_endpoint_rejects_non_loopback_http() {
        let err = secure_token_endpoint_url("http://example.com/token", "token_uri").unwrap_err();
        assert!(err.to_string().contains("must use https"));
    }
}

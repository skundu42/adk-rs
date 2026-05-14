//! Transport-security primitives shared by every outbound HTTP code path.
//!
//! The rule is the same for every credential-bearing client (provider API
//! keys, OAuth tokens, OpenAPI bearer credentials, MCP HTTP transport, A2A
//! `Authorization` headers): the destination must be `https://` or a
//! loopback URL. Anything else — `http://api.example.com`, IP literals on
//! the public internet, scheme-relative URLs — gets rejected before the
//! request is built.
//!
//! Loopback is allowed for local development and tests:
//!
//! - `localhost`
//! - `127.0.0.0/8` (any IPv4 loopback)
//! - `::1` (the sole IPv6 loopback)
//!
//! Mirrors the auth module's token endpoint validation but is always compiled
//! (no `auth` feature gate) and exposed throughout the crate.

use std::net::IpAddr;

use crate::error::{Error, Result};

/// Return `Ok(())` if `url` is safe to send credentials to, otherwise an
/// [`Error::Config`]. `field` names the offending input in the error
/// message so callers can diagnose without dumping the URL itself (the URL
/// can contain secrets in its userinfo).
pub fn require_secure_url(url: &str, field: &str) -> Result<()> {
    if is_secure_url(url) {
        return Ok(());
    }
    Err(Error::config(format!(
        "{field} must be https:// or point to a loopback host \
         (refusing to send credentials over plaintext HTTP)"
    )))
}

/// Same as [`require_secure_url`] but returns a `bool`. Useful when the
/// caller wants to log a warning instead of failing.
#[must_use]
pub fn is_secure_url(url: &str) -> bool {
    let url = url.trim();
    if has_ascii_prefix_ci(url, "https://") {
        return true;
    }
    if let Some(rest) = strip_ascii_prefix_ci(url, "http://") {
        return is_loopback_authority(rest);
    }
    false
}

fn has_ascii_prefix_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

fn strip_ascii_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    has_ascii_prefix_ci(s, prefix).then(|| &s[prefix.len()..])
}

/// Given the part of the URL after `http://`, decide whether the authority
/// component is a loopback host. Strips userinfo, IPv6 brackets, and the
/// optional `:port` suffix before checking.
fn is_loopback_authority(rest: &str) -> bool {
    // Authority ends at the first `/`, `?`, or `#`.
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    // Strip userinfo (`user:pass@host`).
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    // Strip port.
    let host = if let Some(stripped) = host_port.strip_prefix('[') {
        // IPv6 in brackets: `[::1]:1234` → `::1`.
        match stripped.find(']') {
            Some(close) => &stripped[..close],
            None => return false,
        }
    } else {
        host_port.rsplit_once(':').map_or(host_port, |(h, _)| h)
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_is_always_secure() {
        assert!(is_secure_url("https://example.com"));
        assert!(is_secure_url("HTTPS://Example.Com/path?x=1"));
        assert!(is_secure_url("https://user:pass@example.com:8443/api"));
    }

    #[test]
    fn plaintext_public_is_rejected() {
        assert!(!is_secure_url("http://example.com"));
        assert!(!is_secure_url("http://example.com:8080/api"));
        assert!(!is_secure_url("http://user:secret@example.com"));
        // Userinfo can't smuggle loopback past the check.
        assert!(!is_secure_url("http://127.0.0.1@example.com"));
    }

    #[test]
    fn loopback_http_is_allowed() {
        assert!(is_secure_url("http://localhost"));
        assert!(is_secure_url("http://localhost:8000/api"));
        assert!(is_secure_url("http://127.0.0.1:8000"));
        assert!(is_secure_url("http://127.1.2.3"));
        assert!(is_secure_url("http://[::1]"));
        assert!(is_secure_url("http://[::1]:9000/x"));
    }

    #[test]
    fn unknown_schemes_rejected() {
        assert!(!is_secure_url("ftp://example.com"));
        assert!(!is_secure_url("file:///etc/passwd"));
        assert!(!is_secure_url(""));
        assert!(!is_secure_url("example.com"));
    }

    #[test]
    fn require_secure_url_surfaces_field_name() {
        let err =
            require_secure_url("http://api.example.com", "OpenAiConfig.base_url").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("OpenAiConfig.base_url"),
            "missing field: {msg}"
        );
        assert!(msg.contains("https"), "missing https hint: {msg}");
    }
}

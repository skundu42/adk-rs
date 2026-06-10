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
///
/// Parses with the WHATWG [`url`] crate — the same parser reqwest uses —
/// so the host this check validates is exactly the host the request goes
/// to. A hand-rolled parser here would invite differentials (e.g. `\` is a
/// path separator in WHATWG http(s) URLs, so `http://evil.com\@localhost/`
/// actually targets `evil.com`).
#[must_use]
pub fn is_secure_url(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url.trim()) else {
        return false;
    };
    match parsed.scheme() {
        "https" => true,
        "http" => match parsed.host() {
            Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        },
        _ => false,
    }
}

/// Heuristic: would shipping this header to a plaintext endpoint leak a
/// credential? Matches `Authorization`, `Cookie`, `Proxy-Authorization`,
/// and the conventional `x-api-*` / `x-auth-*` patterns case-insensitively.
#[must_use]
pub fn header_looks_credential_bearing(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "authorization" | "cookie" | "proxy-authorization"
    ) || lower.starts_with("x-api")
        || lower.starts_with("x-auth")
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

    /// Regression: WHATWG URLs treat `\` as `/` in http(s), so the authority
    /// of `http://evil.com\@localhost/` ends at the backslash and the real
    /// host is `evil.com`. A naive authority parser that only splits on
    /// `/?#` reads `localhost` instead and waves the URL through.
    #[test]
    fn backslash_authority_differential_is_rejected() {
        assert!(!is_secure_url("http://evil.com\\@localhost/"));
        assert!(!is_secure_url("http://evil.com\\localhost/"));
        assert!(!is_secure_url("http://evil.com\\@127.0.0.1/"));
        // Sanity: the parser agrees with reqwest about the real host.
        let parsed = url::Url::parse("http://evil.com\\@localhost/").unwrap();
        assert_eq!(parsed.host_str(), Some("evil.com"));
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

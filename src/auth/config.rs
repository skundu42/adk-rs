//! [`AuthConfig`] — pairs an [`AuthScheme`] with the raw/exchanged
//! [`AuthCredential`] state for one tool's auth needs.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::auth::credential::AuthCredential;
use crate::auth::scheme::AuthScheme;

/// Per-tool auth descriptor. Carried as `tool.auth_config()` on tools that
/// need authentication; the runner's `CredentialManager` resolves it before
/// invoking the tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthConfig {
    /// What the server expects.
    pub auth_scheme: AuthScheme,
    /// Initial credential (e.g. client_id + client_secret for OAuth2;
    /// service account JSON for SA).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_auth_credential: Option<AuthCredential>,
    /// Resolved credential (set after exchange / consent / refresh).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exchanged_auth_credential: Option<AuthCredential>,
    /// Stable cache key. Computed deterministically when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_key: Option<String>,
}

impl AuthConfig {
    /// Construct with just a scheme.
    #[must_use]
    pub fn new(auth_scheme: AuthScheme) -> Self {
        Self {
            auth_scheme,
            raw_auth_credential: None,
            exchanged_auth_credential: None,
            credential_key: None,
        }
    }

    /// Attach a raw credential.
    #[must_use]
    pub fn with_raw(mut self, raw: AuthCredential) -> Self {
        self.raw_auth_credential = Some(raw);
        self
    }

    /// Pin the cache key explicitly (otherwise computed from scheme + raw).
    #[must_use]
    pub fn with_key(mut self, key: impl Into<String>) -> Self {
        self.credential_key = Some(key.into());
        self
    }

    /// Resolve the cache key. Uses `credential_key` if set, else computes a
    /// stable digest from the scheme + raw credential. Mirrors Python's
    /// `_stable_model_digest` (SHA-256 over canonical JSON).
    #[must_use]
    pub fn resolve_credential_key(&self) -> String {
        if let Some(k) = &self.credential_key {
            return k.clone();
        }
        let mut h = Sha256::new();
        h.update(self.auth_scheme.kind().as_bytes());
        h.update(b":");
        if let Ok(j) = serde_json::to_vec(&self.auth_scheme) {
            h.update(&j);
        }
        h.update(b":");
        if let Some(raw) = &self.raw_auth_credential {
            if let Ok(j) = serde_json::to_vec(raw) {
                h.update(&j);
            }
        }
        let digest = h.finalize();
        let mut hex = String::with_capacity(digest.len() * 2);
        for b in digest.iter() {
            use std::fmt::Write as _;
            let _ = write!(&mut hex, "{b:02x}");
        }
        format!("{}_{}", self.auth_scheme.kind(), &hex[..16])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::credential::AuthCredential;
    use crate::auth::scheme::ApiKeyLocation;

    #[test]
    fn key_is_deterministic_for_same_inputs() {
        let cfg = AuthConfig::new(AuthScheme::ApiKey {
            location: ApiKeyLocation::Header,
            name: "X-API-Key".into(),
            description: None,
        })
        .with_raw(AuthCredential::api_key("secret"));
        let k1 = cfg.resolve_credential_key();
        let k2 = cfg.resolve_credential_key();
        assert_eq!(k1, k2);
        assert!(k1.starts_with("api_key_"));
    }

    #[test]
    fn explicit_key_overrides() {
        let cfg = AuthConfig::new(AuthScheme::ApiKey {
            location: ApiKeyLocation::Header,
            name: "X-API-Key".into(),
            description: None,
        })
        .with_key("explicit");
        assert_eq!(cfg.resolve_credential_key(), "explicit");
    }
}

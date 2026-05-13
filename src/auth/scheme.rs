//! Auth schemes — the *contract* side of authentication (what the server
//! expects). Compare to `AuthCredential` which is the *value* side.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Where an API-key value should live in the HTTP request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiKeyLocation {
    /// HTTP header (`In: header`).
    Header,
    /// Query parameter (`In: query`).
    Query,
    /// Cookie value (`In: cookie`).
    Cookie,
}

/// OAuth 2.0 grant type, matching RFC 6749 nomenclature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuthGrantType {
    /// `authorization_code` (interactive consent).
    AuthorizationCode,
    /// `client_credentials` (server-to-server).
    ClientCredentials,
    /// `implicit` (deprecated by RFC 8252).
    Implicit,
    /// `password` (resource owner password credentials).
    Password,
}

/// OAuth 2.0 flow descriptor — endpoints + scopes for one grant type.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthFlow {
    /// Authorization endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_url: Option<String>,
    /// Token endpoint.
    pub token_url: String,
    /// Refresh-token endpoint (defaults to `token_url`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_url: Option<String>,
    /// Available scopes (name → description).
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub scopes: IndexMap<String, String>,
}

/// All OAuth2 flows the server advertises. At least one must be populated.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthFlows {
    /// Interactive authorization-code flow (typically with PKCE).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_code: Option<OAuthFlow>,
    /// Server-to-server client-credentials flow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_credentials: Option<OAuthFlow>,
    /// Implicit flow (deprecated by RFC 8252).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implicit: Option<OAuthFlow>,
    /// Resource owner password credentials.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<OAuthFlow>,
}

/// Auth scheme contract.
///
/// Mirrors the subset of OpenAPI 3 `securityScheme` types we support, plus a
/// `Custom` escape hatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scheme_type", rename_all = "snake_case")]
pub enum AuthScheme {
    /// `apiKey`. Value lives in `name` (header/query/cookie key).
    ApiKey {
        /// Key location.
        location: ApiKeyLocation,
        /// Header/query/cookie name.
        name: String,
        /// Optional description.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// `http`. `scheme` is "basic" or "bearer".
    Http {
        /// `"basic"` | `"bearer"` | …
        scheme: String,
        /// Hint for bearer format (`"JWT"`, `"opaque"`, …).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bearer_format: Option<String>,
        /// Optional description.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// `oauth2`.
    OAuth2 {
        /// Flows advertised by the server.
        flows: OAuthFlows,
        /// Optional description.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// `openIdConnect`. URL is the discovery doc.
    OpenIdConnect {
        /// `.well-known/openid-configuration` URL.
        open_id_connect_url: String,
        /// Scopes to request.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        scopes: Vec<String>,
        /// Optional description.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// Vendor-specific scheme. The unstructured tag string lets a custom
    /// provider plug in via [`crate::auth::provider`].
    Custom {
        /// Vendor tag.
        tag: String,
        /// Arbitrary properties.
        #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
        properties: serde_json::Value,
    },
}

impl AuthScheme {
    /// Stable identifier used as part of the credential cache key.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::ApiKey { .. } => "api_key",
            Self::Http { .. } => "http",
            Self::OAuth2 { .. } => "oauth2",
            Self::OpenIdConnect { .. } => "oidc",
            Self::Custom { .. } => "custom",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_key_scheme_round_trip() {
        let s = AuthScheme::ApiKey {
            location: ApiKeyLocation::Header,
            name: "X-API-Key".into(),
            description: None,
        };
        let j = serde_json::to_string(&s).unwrap();
        assert!(j.contains("\"api_key\""));
        assert!(j.contains("\"header\""));
        let back: AuthScheme = serde_json::from_str(&j).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn oauth2_scheme_with_authorization_code_flow() {
        let mut scopes = IndexMap::new();
        scopes.insert("read".to_string(), "Read scope".to_string());
        let s = AuthScheme::OAuth2 {
            flows: OAuthFlows {
                authorization_code: Some(OAuthFlow {
                    authorization_url: Some("https://provider/authorize".into()),
                    token_url: "https://provider/token".into(),
                    refresh_url: None,
                    scopes,
                }),
                ..OAuthFlows::default()
            },
            description: None,
        };
        let back: AuthScheme = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(s, back);
        assert_eq!(s.kind(), "oauth2");
    }
}

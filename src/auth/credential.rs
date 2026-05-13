//! Resolved credential values that travel from the credential manager into
//! a tool's HTTP client. Wire-shape mirrors Python ADK's `AuthCredential`.

use serde::{Deserialize, Serialize};

/// Type discriminator for an [`AuthCredential`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthCredentialType {
    /// Static API key (sent as a header or query param).
    ApiKey,
    /// HTTP Basic / Bearer auth.
    Http,
    /// OAuth 2.0 (authorization-code, client-credentials, PKCE).
    OAuth2,
    /// OpenID Connect (OAuth2 + ID token).
    OpenIdConnect,
    /// Google-style service-account JSON keys.
    ServiceAccount,
}

/// HTTP-auth credential payload.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpAuth {
    /// Scheme name (`"basic"`, `"bearer"`, etc.).
    pub scheme: String,
    /// Bearer token (when `scheme == "bearer"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// Username (when `scheme == "basic"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Password (when `scheme == "basic"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

/// OAuth 2.0 credential state — both inputs (client_id/secret) and outputs
/// (access_token, refresh_token, expires_at). Persisted to the credential
/// service across requests.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuth2Auth {
    /// Public client identifier.
    pub client_id: String,
    /// Optional client secret (omit for public clients with PKCE).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// Authorization endpoint (server-provided or discovered).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_uri: Option<String>,
    /// Token endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_uri: Option<String>,
    /// Redirect URI registered with the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_uri: Option<String>,
    /// CSRF state (set during URL generation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// PKCE S256 verifier (set during URL generation; used at token exchange).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_verifier: Option<String>,
    /// Authorization code received from the redirect (set by the caller).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_code: Option<String>,
    /// Resolved access token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    /// Refresh token (when granted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// Token expiry (Unix epoch seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    /// Requested scopes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
}

/// Google-style service account JSON credentials. Fields mirror the standard
/// `service_account.json` shape so users can deserialize the file directly.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceAccountAuth {
    /// `"service_account"`.
    #[serde(rename = "type", default)]
    pub account_type: String,
    /// GCP project id.
    pub project_id: String,
    /// Key identifier.
    pub private_key_id: String,
    /// PEM-encoded RSA private key.
    pub private_key: String,
    /// Service account email.
    pub client_email: String,
    /// Numeric client id.
    #[serde(default)]
    pub client_id: String,
    /// Authorization URI.
    #[serde(default)]
    pub auth_uri: String,
    /// Token URI.
    pub token_uri: String,
    /// Scopes to request when exchanging for an access token.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
    /// When set, generate an ID token for this audience instead of an access
    /// token. Used for service-to-service auth on Cloud Run / IAP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_audience: Option<String>,
    /// Resolved access token (set after exchange).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    /// Token expiry (Unix epoch seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

/// Unified credential envelope. Exactly one of the inner fields is populated
/// for any given `auth_type`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthCredential {
    /// Discriminator: which inner field to read.
    pub auth_type: AuthCredentialType,
    /// API key value, when `auth_type == ApiKey`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// HTTP auth payload, when `auth_type == Http`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http: Option<HttpAuth>,
    /// OAuth2 payload, when `auth_type ∈ {OAuth2, OpenIdConnect}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth2: Option<OAuth2Auth>,
    /// Service account payload, when `auth_type == ServiceAccount`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_account: Option<ServiceAccountAuth>,
}

impl AuthCredential {
    /// Construct an API-key credential.
    #[must_use]
    pub fn api_key(value: impl Into<String>) -> Self {
        Self {
            auth_type: AuthCredentialType::ApiKey,
            api_key: Some(value.into()),
            http: None,
            oauth2: None,
            service_account: None,
        }
    }

    /// Construct an HTTP-Bearer credential.
    #[must_use]
    pub fn bearer(token: impl Into<String>) -> Self {
        Self {
            auth_type: AuthCredentialType::Http,
            api_key: None,
            http: Some(HttpAuth {
                scheme: "bearer".into(),
                token: Some(token.into()),
                username: None,
                password: None,
            }),
            oauth2: None,
            service_account: None,
        }
    }

    /// Construct an HTTP-Basic credential.
    #[must_use]
    pub fn basic(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            auth_type: AuthCredentialType::Http,
            api_key: None,
            http: Some(HttpAuth {
                scheme: "basic".into(),
                token: None,
                username: Some(username.into()),
                password: Some(password.into()),
            }),
            oauth2: None,
            service_account: None,
        }
    }

    /// Construct an OAuth2 credential (typically with `client_id` /
    /// `client_secret` only — tokens get filled in after exchange).
    #[must_use]
    pub fn oauth2(oauth2: OAuth2Auth) -> Self {
        Self {
            auth_type: AuthCredentialType::OAuth2,
            api_key: None,
            http: None,
            oauth2: Some(oauth2),
            service_account: None,
        }
    }

    /// Construct a service-account credential.
    #[must_use]
    pub fn service_account(sa: ServiceAccountAuth) -> Self {
        Self {
            auth_type: AuthCredentialType::ServiceAccount,
            api_key: None,
            http: None,
            oauth2: None,
            service_account: Some(sa),
        }
    }

    /// True when a usable access value is present without further exchange.
    /// (API keys and HTTP basic/bearer are always ready; OAuth2/SA need an
    /// `access_token`.)
    #[must_use]
    pub fn is_ready(&self) -> bool {
        match self.auth_type {
            AuthCredentialType::ApiKey => self.api_key.is_some(),
            AuthCredentialType::Http => self
                .http
                .as_ref()
                .is_some_and(|h| h.token.is_some() || h.username.is_some()),
            AuthCredentialType::OAuth2 | AuthCredentialType::OpenIdConnect => self
                .oauth2
                .as_ref()
                .and_then(|o| o.access_token.as_ref())
                .is_some(),
            AuthCredentialType::ServiceAccount => self
                .service_account
                .as_ref()
                .and_then(|s| s.access_token.as_ref())
                .is_some(),
        }
    }

    /// True when the credential carries an expiry and that expiry has passed
    /// (with a 60-second leeway).
    #[must_use]
    pub fn is_expired(&self, now_unix: i64) -> bool {
        let exp = match self.auth_type {
            AuthCredentialType::OAuth2 | AuthCredentialType::OpenIdConnect => {
                self.oauth2.as_ref().and_then(|o| o.expires_at)
            }
            AuthCredentialType::ServiceAccount => {
                self.service_account.as_ref().and_then(|s| s.expires_at)
            }
            _ => None,
        };
        matches!(exp, Some(e) if e <= now_unix + 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_key_round_trip() {
        let c = AuthCredential::api_key("sk-xyz");
        let s = serde_json::to_string(&c).unwrap();
        let back: AuthCredential = serde_json::from_str(&s).unwrap();
        assert_eq!(c, back);
        assert!(c.is_ready());
    }

    #[test]
    fn bearer_round_trip() {
        let c = AuthCredential::bearer("token-abc");
        let s = serde_json::to_string(&c).unwrap();
        assert!(s.contains("\"bearer\""));
        let back: AuthCredential = serde_json::from_str(&s).unwrap();
        assert_eq!(c, back);
        assert!(c.is_ready());
    }

    #[test]
    fn oauth2_unready_until_access_token() {
        let mut c = AuthCredential::oauth2(OAuth2Auth {
            client_id: "id".into(),
            ..OAuth2Auth::default()
        });
        assert!(!c.is_ready());
        c.oauth2.as_mut().unwrap().access_token = Some("at".into());
        assert!(c.is_ready());
    }

    #[test]
    fn oauth2_expiry_leeway() {
        let mut c = AuthCredential::oauth2(OAuth2Auth {
            client_id: "id".into(),
            access_token: Some("at".into()),
            expires_at: Some(1000),
            ..OAuth2Auth::default()
        });
        assert!(!c.is_expired(0));
        assert!(c.is_expired(1000)); // 1000 <= 0 + 60? No — 1000 <= 1000 + 60 = 1060, yes
        assert!(c.is_expired(2000));
        c.oauth2.as_mut().unwrap().expires_at = None;
        assert!(!c.is_expired(9_999_999));
    }
}

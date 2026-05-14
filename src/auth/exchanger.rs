//! Credential exchangers — turn a *raw* credential into a *resolved* one.
//!
//! - [`OAuth2Exchanger`] swaps `client_id + auth_code + code_verifier` (or
//!   `client_id + secret` for the `client_credentials` flow) for an access /
//!   refresh token pair via the token endpoint.
//! - [`ServiceAccountExchanger`] signs an `RS256` JWT and trades it at Google's
//!   OAuth2 token endpoint for an access token (or ID token, when
//!   `target_audience` is set).
//!
//! Implementations are stateless and plug into [`ExchangerRegistry`].

use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;

use crate::auth::config::AuthConfig;
use crate::auth::credential::{AuthCredential, AuthCredentialType, OAuth2Auth, ServiceAccountAuth};
use crate::auth::handler::AuthHandler;
use crate::auth::scheme::AuthScheme;
use crate::error::{Error, Result};

/// One credential exchange step.
#[async_trait]
pub trait CredentialExchanger: Send + Sync + std::fmt::Debug + 'static {
    /// Attempt to upgrade `raw` into a resolved credential. Returns `Ok(None)`
    /// when the exchanger can't complete (e.g. authorization-code flow needs
    /// the user to consent first).
    async fn exchange(
        &self,
        config: &AuthConfig,
        raw: &AuthCredential,
    ) -> Result<Option<AuthCredential>>;
}

/// Maps credential types to exchangers.
#[derive(Default, Debug)]
pub struct ExchangerRegistry {
    by_type: HashMap<AuthCredentialType, Arc<dyn CredentialExchanger>>,
}

impl ExchangerRegistry {
    /// Empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a registry pre-populated with the standard exchangers
    /// (OAuth2 + ServiceAccount). Used by the default [`crate::auth::CredentialManager`].
    #[must_use]
    pub fn with_defaults() -> Self {
        let mut r = Self::new();
        r.register(AuthCredentialType::OAuth2, Arc::new(OAuth2Exchanger));
        r.register(AuthCredentialType::OpenIdConnect, Arc::new(OAuth2Exchanger));
        r.register(
            AuthCredentialType::ServiceAccount,
            Arc::new(ServiceAccountExchanger),
        );
        r
    }

    /// Replace the exchanger for the given type.
    pub fn register(&mut self, ty: AuthCredentialType, exchanger: Arc<dyn CredentialExchanger>) {
        self.by_type.insert(ty, exchanger);
    }

    /// Look up an exchanger.
    #[must_use]
    pub fn get(&self, ty: AuthCredentialType) -> Option<Arc<dyn CredentialExchanger>> {
        self.by_type.get(&ty).cloned()
    }
}

/// OAuth2 exchanger. Handles three cases:
///
/// 1. raw already carries an `access_token` → pass-through (the caller pre-baked one).
/// 2. raw carries an `auth_code` + `code_verifier` → authorization-code exchange.
/// 3. scheme advertises `client_credentials` → server-to-server exchange.
#[derive(Debug, Default)]
pub struct OAuth2Exchanger;

#[async_trait]
impl CredentialExchanger for OAuth2Exchanger {
    async fn exchange(
        &self,
        config: &AuthConfig,
        raw: &AuthCredential,
    ) -> Result<Option<AuthCredential>> {
        let Some(oauth2) = raw.oauth2.as_ref() else {
            return Ok(None);
        };
        let now = Utc::now().timestamp();

        // (1) Already-baked token. If still fresh, hand it through; if expired
        // and we have a refresh token, run the refresh; otherwise refuse so the
        // caller (CredentialManager) falls through to the consent path rather
        // than serving an expired token forever (v0.2 #5).
        if oauth2.access_token.is_some() {
            if !raw.is_expired(now) {
                return Ok(Some(raw.clone()));
            }
            if oauth2.refresh_token.is_some() {
                let refresher = crate::auth::refresher::OAuth2Refresher;
                if let Some(refreshed) =
                    crate::auth::refresher::CredentialRefresher::refresh(&refresher, config, raw)
                        .await?
                {
                    return Ok(Some(refreshed));
                }
            }
            return Ok(None);
        }

        // (2) Authorization-code flow.
        if let (Some(auth_code), Some(verifier)) =
            (oauth2.auth_code.as_deref(), oauth2.code_verifier.as_deref())
        {
            let mut populated = oauth2.clone();
            attach_flow_endpoints(&mut populated, &config.auth_scheme);
            let handler = AuthHandler::from_oauth2(&populated)?;
            let tok = handler.exchange_code(auth_code, verifier).await?;
            let mut new = oauth2.clone();
            tok.apply_to(&mut new);
            return Ok(Some(AuthCredential::oauth2(new)));
        }

        // (3) Client-credentials flow.
        if matches!(
            config.auth_scheme,
            AuthScheme::OAuth2 { flows: ref f, .. }
            if f.client_credentials.is_some()
        ) {
            let mut populated = oauth2.clone();
            attach_flow_endpoints(&mut populated, &config.auth_scheme);
            let tok = client_credentials_exchange(&populated).await?;
            let mut new = oauth2.clone();
            tok.apply_to(&mut new);
            return Ok(Some(AuthCredential::oauth2(new)));
        }

        Ok(None)
    }
}

/// Fill in `auth_uri` / `token_uri` from the scheme's authorization-code flow
/// if they aren't already set. Mirrors Python's "discover endpoints from
/// scheme" step.
fn attach_flow_endpoints(oauth2: &mut OAuth2Auth, scheme: &AuthScheme) {
    if let AuthScheme::OAuth2 { flows, .. } = scheme {
        if let Some(ac) = flows.authorization_code.as_ref() {
            if oauth2.auth_uri.is_none() {
                oauth2.auth_uri.clone_from(&ac.authorization_url);
            }
            if oauth2.token_uri.is_none() {
                oauth2.token_uri = Some(ac.token_url.clone());
            }
        } else if let Some(cc) = flows.client_credentials.as_ref() {
            if oauth2.token_uri.is_none() {
                oauth2.token_uri = Some(cc.token_url.clone());
            }
        }
    }
}

/// Client-credentials grant. Posts `grant_type=client_credentials` directly
/// to the token endpoint with HTTP Basic auth.
async fn client_credentials_exchange(
    oauth2: &OAuth2Auth,
) -> Result<crate::auth::handler::ExchangedToken> {
    let token_uri = oauth2
        .token_uri
        .as_deref()
        .ok_or_else(|| Error::config("OAuth2Auth.token_uri is required for client_credentials"))?;
    let secret = oauth2
        .client_secret
        .as_deref()
        .ok_or_else(|| Error::config("client_secret is required for client_credentials"))?;
    let client = reqwest::Client::new();
    let mut form: Vec<(&str, String)> = vec![("grant_type", "client_credentials".into())];
    if !oauth2.scopes.is_empty() {
        form.push(("scope", oauth2.scopes.join(" ")));
    }
    let resp = client
        .post(token_uri)
        .basic_auth(&oauth2.client_id, Some(secret))
        .form(&form)
        .send()
        .await
        .map_err(|e| Error::other(format!("token endpoint: {e}")))?;
    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| Error::other(format!("token response decode: {e}")))?;
    if !status.is_success() {
        return Err(Error::other(format!("token endpoint {status}: {body}")));
    }
    let access_token = body
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::other("token response missing access_token"))?
        .to_string();
    let expires_in = body.get("expires_in").and_then(serde_json::Value::as_i64);
    let refresh_token = body
        .get("refresh_token")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    Ok(crate::auth::handler::ExchangedToken {
        access_token,
        refresh_token,
        expires_at: expires_in.map(|e| Utc::now().timestamp() + e),
    })
}

/// Service-account JWT exchanger. Signs a JWT with the SA's private key and
/// POSTs it to the token endpoint as `grant_type=urn:ietf:params:oauth:grant-type:jwt-bearer`.
/// Mirrors `google.oauth2.service_account.Credentials._make_authorization_grant_assertion`.
#[derive(Debug, Default)]
pub struct ServiceAccountExchanger;

#[async_trait]
impl CredentialExchanger for ServiceAccountExchanger {
    async fn exchange(
        &self,
        _config: &AuthConfig,
        raw: &AuthCredential,
    ) -> Result<Option<AuthCredential>> {
        let Some(sa) = raw.service_account.as_ref() else {
            return Ok(None);
        };
        let now = Utc::now().timestamp();
        // Already-baked access token that's still fresh? Hand through.
        if sa.access_token.is_some() && !raw.is_expired(now) {
            return Ok(Some(raw.clone()));
        }
        // Otherwise re-sign and re-exchange. SA tokens have no refresh-token —
        // a fresh assertion is the refresh mechanism.
        let token = sign_and_post_jwt(sa).await?;
        let mut new = sa.clone();
        new.access_token = Some(token.access_token);
        new.expires_at = token.expires_at;
        Ok(Some(AuthCredential::service_account(new)))
    }
}

/// Sign a JWT-bearer assertion and exchange it for an access token (or ID
/// token, when `target_audience` is set).
async fn sign_and_post_jwt(
    sa: &ServiceAccountAuth,
) -> Result<crate::auth::handler::ExchangedToken> {
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use serde::Serialize;

    if sa.private_key.is_empty() {
        return Err(Error::config(
            "ServiceAccountAuth.private_key is required for JWT signing",
        ));
    }
    if sa.client_email.is_empty() {
        return Err(Error::config("ServiceAccountAuth.client_email is required"));
    }
    if sa.token_uri.is_empty() {
        return Err(Error::config("ServiceAccountAuth.token_uri is required"));
    }

    #[derive(Serialize)]
    struct Claims<'a> {
        iss: &'a str,
        aud: &'a str,
        iat: i64,
        exp: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        target_audience: Option<&'a str>,
    }

    let now = Utc::now().timestamp();
    let exp = now + 3600;
    let claims = Claims {
        iss: &sa.client_email,
        aud: &sa.token_uri,
        iat: now,
        exp,
        scope: if sa.scopes.is_empty() {
            None
        } else {
            Some(sa.scopes.join(" "))
        },
        target_audience: sa.target_audience.as_deref(),
    };
    let mut header = Header::new(Algorithm::RS256);
    if !sa.private_key_id.is_empty() {
        header.kid = Some(sa.private_key_id.clone());
    }
    let key = EncodingKey::from_rsa_pem(sa.private_key.as_bytes())
        .map_err(|e| Error::config(format!("invalid RSA private_key PEM: {e}")))?;
    let assertion =
        encode(&header, &claims, &key).map_err(|e| Error::other(format!("JWT encode: {e}")))?;

    let client = reqwest::Client::new();
    let resp = client
        .post(&sa.token_uri)
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", assertion.as_str()),
        ])
        .send()
        .await
        .map_err(|e| Error::other(format!("SA token endpoint: {e}")))?;
    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| Error::other(format!("SA token response decode: {e}")))?;
    if !status.is_success() {
        return Err(Error::other(format!("SA token endpoint {status}: {body}")));
    }

    // For target_audience requests, the response field is `id_token` instead
    // of `access_token`; we surface it as the access_token field.
    let access_token = body
        .get("access_token")
        .or_else(|| body.get("id_token"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::other("SA response missing access_token / id_token"))?
        .to_string();
    let expires_in = body.get("expires_in").and_then(serde_json::Value::as_i64);
    Ok(crate::auth::handler::ExchangedToken {
        access_token,
        refresh_token: None,
        expires_at: expires_in.map(|e| Utc::now().timestamp() + e),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::credential::AuthCredential;

    #[tokio::test]
    async fn oauth2_passes_through_ready_credential() {
        let raw = AuthCredential::oauth2(OAuth2Auth {
            client_id: "id".into(),
            access_token: Some("baked".into()),
            ..OAuth2Auth::default()
        });
        let cfg = AuthConfig::new(AuthScheme::OAuth2 {
            flows: Default::default(),
            description: None,
        });
        let out = OAuth2Exchanger.exchange(&cfg, &raw).await.unwrap().unwrap();
        assert_eq!(out.oauth2.unwrap().access_token.as_deref(), Some("baked"));
    }

    #[tokio::test]
    async fn oauth2_returns_none_when_no_auth_code_or_secret() {
        let raw = AuthCredential::oauth2(OAuth2Auth {
            client_id: "id".into(),
            ..OAuth2Auth::default()
        });
        let cfg = AuthConfig::new(AuthScheme::OAuth2 {
            flows: Default::default(),
            description: None,
        });
        let out = OAuth2Exchanger.exchange(&cfg, &raw).await.unwrap();
        assert!(out.is_none());
    }

    #[tokio::test]
    async fn service_account_passes_through_ready_credential() {
        let raw = AuthCredential::service_account(ServiceAccountAuth {
            access_token: Some("baked".into()),
            ..ServiceAccountAuth::default()
        });
        let cfg = AuthConfig::new(AuthScheme::Custom {
            tag: "google_sa".into(),
            properties: serde_json::Value::Null,
        });
        let out = ServiceAccountExchanger
            .exchange(&cfg, &raw)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            out.service_account.unwrap().access_token.as_deref(),
            Some("baked")
        );
    }

    /// Verifies bug-fix v0.2.1 #5: an `access_token` that's already expired
    /// MUST NOT be passed through as ready. Without a refresh_token there's
    /// no way to revive it, so the exchanger returns `Ok(None)` and lets
    /// `CredentialManager` fall through to the consent path.
    #[tokio::test]
    async fn oauth2_does_not_serve_expired_token_without_refresh_token() {
        let raw = AuthCredential::oauth2(OAuth2Auth {
            client_id: "id".into(),
            access_token: Some("stale".into()),
            expires_at: Some(0), // expired long ago
            // no refresh_token
            ..OAuth2Auth::default()
        });
        let cfg = AuthConfig::new(AuthScheme::OAuth2 {
            flows: Default::default(),
            description: None,
        });
        let out = OAuth2Exchanger.exchange(&cfg, &raw).await.unwrap();
        assert!(
            out.is_none(),
            "expired access_token without refresh_token should not pass through; got {out:?}"
        );
    }
}

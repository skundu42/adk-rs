//! [`AuthHandler`] — OAuth2 URL generation + token exchange + PKCE.
//!
//! Wraps the [`oauth2`] crate to keep the rest of the auth code free of
//! oauth2 SDK types. All async network operations live here so they're easy
//! to mock for tests.

use std::time::SystemTime;

use oauth2::basic::BasicClient;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointNotSet, EndpointSet,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, TokenResponse, TokenUrl,
};

use crate::auth::credential::OAuth2Auth;
use crate::error::{Error, Result};

type ConfiguredClient =
    BasicClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointSet>;

/// Stateless façade over [`oauth2`]. Construct a fresh one per
/// authorization-code flow.
#[derive(Debug)]
pub struct AuthHandler {
    inner: ConfiguredClient,
    redirect_uri: RedirectUrl,
}

impl AuthHandler {
    /// Build an `AuthHandler` from a partially-filled [`OAuth2Auth`]. The
    /// `auth_uri`, `token_uri`, `client_id`, and `redirect_uri` fields are
    /// required; `client_secret` is optional (omit for public PKCE clients).
    pub fn from_oauth2(oauth2: &OAuth2Auth) -> Result<Self> {
        let auth_uri = oauth2
            .auth_uri
            .as_deref()
            .ok_or_else(|| Error::config("OAuth2Auth.auth_uri is required"))?;
        let token_uri = oauth2
            .token_uri
            .as_deref()
            .ok_or_else(|| Error::config("OAuth2Auth.token_uri is required"))?;
        let redirect_uri = oauth2
            .redirect_uri
            .as_deref()
            .ok_or_else(|| Error::config("OAuth2Auth.redirect_uri is required"))?;

        let mut client = BasicClient::new(ClientId::new(oauth2.client_id.clone()))
            .set_auth_uri(
                AuthUrl::new(auth_uri.to_string())
                    .map_err(|e| Error::config(format!("invalid auth_uri: {e}")))?,
            )
            .set_token_uri(
                TokenUrl::new(token_uri.to_string())
                    .map_err(|e| Error::config(format!("invalid token_uri: {e}")))?,
            );
        if let Some(secret) = oauth2.client_secret.as_deref() {
            client = client.set_client_secret(ClientSecret::new(secret.to_string()));
        }
        let redirect = RedirectUrl::new(redirect_uri.to_string())
            .map_err(|e| Error::config(format!("invalid redirect_uri: {e}")))?;
        Ok(Self {
            inner: client,
            redirect_uri: redirect,
        })
    }

    /// Generate an authorization URL with a fresh PKCE verifier + CSRF state.
    /// Returns `(url, state, verifier)`. The caller must persist the
    /// verifier somewhere so the token exchange can use it.
    pub fn authorize_url(&self, scopes: &[String]) -> (String, String, String) {
        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
        let mut builder = self
            .inner
            .authorize_url(CsrfToken::new_random)
            .set_pkce_challenge(pkce_challenge)
            .set_redirect_uri(std::borrow::Cow::Borrowed(&self.redirect_uri));
        for s in scopes {
            builder = builder.add_scope(Scope::new(s.clone()));
        }
        let (url, state) = builder.url();
        (
            url.to_string(),
            state.secret().clone(),
            pkce_verifier.secret().clone(),
        )
    }

    /// Exchange `auth_code` (returned via the redirect) for an access token.
    /// Reuses the `code_verifier` from the original [`AuthHandler::authorize_url`]
    /// call.
    pub async fn exchange_code(
        &self,
        auth_code: &str,
        code_verifier: &str,
    ) -> Result<ExchangedToken> {
        let http_client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| Error::other(format!("reqwest build: {e}")))?;

        let token = self
            .inner
            .exchange_code(AuthorizationCode::new(auth_code.to_string()))
            .set_redirect_uri(std::borrow::Cow::Borrowed(&self.redirect_uri))
            .set_pkce_verifier(PkceCodeVerifier::new(code_verifier.to_string()))
            .request_async(&http_client)
            .await
            .map_err(|e| Error::other(format!("oauth2 exchange: {e}")))?;

        Ok(ExchangedToken {
            access_token: token.access_token().secret().clone(),
            refresh_token: token.refresh_token().map(|r| r.secret().clone()),
            expires_at: token.expires_in().and_then(|d| {
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .ok()
                    .map(|now| now.as_secs() as i64 + d.as_secs() as i64)
            }),
        })
    }

    /// Exchange a refresh-token for a fresh access token.
    pub async fn refresh(&self, refresh_token: &str) -> Result<ExchangedToken> {
        let http_client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| Error::other(format!("reqwest build: {e}")))?;
        let token = self
            .inner
            .exchange_refresh_token(&oauth2::RefreshToken::new(refresh_token.to_string()))
            .request_async(&http_client)
            .await
            .map_err(|e| Error::other(format!("oauth2 refresh: {e}")))?;
        Ok(ExchangedToken {
            access_token: token.access_token().secret().clone(),
            refresh_token: token.refresh_token().map(|r| r.secret().clone()),
            expires_at: token.expires_in().and_then(|d| {
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .ok()
                    .map(|now| now.as_secs() as i64 + d.as_secs() as i64)
            }),
        })
    }
}

/// Result of an OAuth2 token exchange or refresh.
#[derive(Debug, Clone)]
pub struct ExchangedToken {
    /// New access token.
    pub access_token: String,
    /// New refresh token, if rotated.
    pub refresh_token: Option<String>,
    /// Expiry as Unix epoch seconds, if reported.
    pub expires_at: Option<i64>,
}

impl ExchangedToken {
    /// Merge into an existing [`OAuth2Auth`] in-place.
    pub fn apply_to(&self, oauth2: &mut OAuth2Auth) {
        oauth2.access_token = Some(self.access_token.clone());
        if let Some(rt) = &self.refresh_token {
            oauth2.refresh_token = Some(rt.clone());
        }
        if let Some(exp) = self.expires_at {
            oauth2.expires_at = Some(exp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_oauth2() -> OAuth2Auth {
        OAuth2Auth {
            client_id: "client-abc".into(),
            client_secret: Some("secret".into()),
            auth_uri: Some("https://example/authorize".into()),
            token_uri: Some("https://example/token".into()),
            redirect_uri: Some("https://app/callback".into()),
            ..OAuth2Auth::default()
        }
    }

    #[test]
    fn authorize_url_has_pkce_and_state() {
        let h = AuthHandler::from_oauth2(&fake_oauth2()).unwrap();
        let (url, state, verifier) = h.authorize_url(&["read".into()]);
        assert!(url.contains("code_challenge"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("client_id=client-abc"));
        assert!(url.contains("scope=read"));
        assert!(!state.is_empty());
        assert!(!verifier.is_empty());
    }
}

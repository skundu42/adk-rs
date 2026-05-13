//! Credential refreshers — extend an expired credential's lifetime.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

use crate::auth::config::AuthConfig;
use crate::auth::credential::{AuthCredential, AuthCredentialType};
use crate::error::Result;

/// Refresh an expired credential.
#[async_trait]
pub trait CredentialRefresher: Send + Sync + std::fmt::Debug + 'static {
    /// Try to refresh `cred`. Returns the refreshed credential, or
    /// `Ok(None)` if no refresh is possible (e.g. no refresh token).
    async fn refresh(
        &self,
        config: &AuthConfig,
        cred: &AuthCredential,
    ) -> Result<Option<AuthCredential>>;
}

/// Maps credential types to refreshers.
#[derive(Default, Debug)]
pub struct RefresherRegistry {
    by_type: HashMap<AuthCredentialType, Arc<dyn CredentialRefresher>>,
}

impl RefresherRegistry {
    /// Empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registry with the standard `OAuth2Refresher` for OAuth2 + OIDC.
    #[must_use]
    pub fn with_defaults() -> Self {
        let mut r = Self::new();
        r.register(AuthCredentialType::OAuth2, Arc::new(OAuth2Refresher));
        r.register(AuthCredentialType::OpenIdConnect, Arc::new(OAuth2Refresher));
        r
    }

    /// Replace the refresher for the given type.
    pub fn register(&mut self, ty: AuthCredentialType, refresher: Arc<dyn CredentialRefresher>) {
        self.by_type.insert(ty, refresher);
    }

    /// Look up a refresher.
    #[must_use]
    pub fn get(&self, ty: AuthCredentialType) -> Option<Arc<dyn CredentialRefresher>> {
        self.by_type.get(&ty).cloned()
    }
}

/// Refreshes an OAuth2 credential via its refresh token. Uses the standard
/// authorization-code flow's token endpoint (or the explicitly-set
/// `OAuth2Auth.token_uri`).
#[derive(Debug, Default)]
pub struct OAuth2Refresher;

#[async_trait]
impl CredentialRefresher for OAuth2Refresher {
    async fn refresh(
        &self,
        config: &AuthConfig,
        cred: &AuthCredential,
    ) -> Result<Option<AuthCredential>> {
        let Some(oauth2) = cred.oauth2.as_ref() else {
            return Ok(None);
        };
        let Some(refresh_token) = oauth2.refresh_token.as_deref() else {
            return Ok(None);
        };
        let mut populated = oauth2.clone();
        if let crate::auth::scheme::AuthScheme::OAuth2 { flows, .. } = &config.auth_scheme {
            if let Some(ac) = flows.authorization_code.as_ref() {
                if populated.auth_uri.is_none() {
                    populated.auth_uri.clone_from(&ac.authorization_url);
                }
                if populated.token_uri.is_none() {
                    populated.token_uri = Some(ac.token_url.clone());
                }
                if populated.redirect_uri.is_none() {
                    // PKCE refresh doesn't require a redirect_uri at the
                    // protocol level, but oauth2 crate's BasicClient does, so
                    // fall back to a no-op localhost value.
                    populated.redirect_uri = Some("http://localhost/__adk_refresh__".into());
                }
            }
        }
        let handler = crate::auth::handler::AuthHandler::from_oauth2(&populated)?;
        let tok = handler.refresh(refresh_token).await?;
        let mut new = oauth2.clone();
        tok.apply_to(&mut new);
        Ok(Some(AuthCredential::oauth2(new)))
    }
}

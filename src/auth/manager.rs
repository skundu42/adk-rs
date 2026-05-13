//! [`CredentialManager`] — the orchestrator that resolves a tool's auth needs
//! into a usable credential. Ports Python ADK's 8-step workflow.

use chrono::Utc;
use std::sync::Arc;

use crate::auth::config::AuthConfig;
use crate::auth::credential::{AuthCredential, AuthCredentialType};
use crate::auth::exchanger::ExchangerRegistry;
use crate::auth::provider::AuthProviderRegistry;
use crate::auth::refresher::RefresherRegistry;
use crate::auth::service::CredentialService;
use crate::error::{Error, Result};

/// Outcome of a [`CredentialManager::resolve`] call.
#[derive(Debug, Clone)]
pub enum ResolveOutcome {
    /// A usable credential is ready. Hand to the tool.
    Ready(AuthCredential),
    /// Interactive consent is required. The runner should emit
    /// `adk_request_credential` and pause the tool call.
    NeedsUserConsent(AuthConfig),
    /// Configuration error — the tool can't be invoked.
    Misconfigured(String),
}

/// Resolves [`AuthConfig`] into a ready [`AuthCredential`] per the 8-step
/// workflow:
///
/// 1. validate config
/// 2. return immediately if `is_ready` and not expired
/// 3. try cache: `credential_service.load(app, user, key)`
/// 4. (preprocessor-stored) auth response (handled at runner layer)
/// 5. authorization-code flow with no exchanged credential → `NeedsUserConsent`
/// 6. exchange (service-account / authorization-code → access token)
/// 7. refresh if expired
/// 8. save back to credential service
#[derive(Debug)]
pub struct CredentialManager {
    config: AuthConfig,
    exchangers: Arc<ExchangerRegistry>,
    refreshers: Arc<RefresherRegistry>,
    providers: Arc<AuthProviderRegistry>,
}

impl CredentialManager {
    /// Construct with default exchangers + refreshers.
    #[must_use]
    pub fn new(config: AuthConfig) -> Self {
        Self {
            config,
            exchangers: Arc::new(ExchangerRegistry::with_defaults()),
            refreshers: Arc::new(RefresherRegistry::with_defaults()),
            providers: Arc::new(AuthProviderRegistry::new()),
        }
    }

    /// Construct with explicit registries (override for tests / custom providers).
    #[must_use]
    pub fn with_registries(
        config: AuthConfig,
        exchangers: Arc<ExchangerRegistry>,
        refreshers: Arc<RefresherRegistry>,
        providers: Arc<AuthProviderRegistry>,
    ) -> Self {
        Self {
            config,
            exchangers,
            refreshers,
            providers,
        }
    }

    /// The cache key this manager resolves to.
    #[must_use]
    pub fn credential_key(&self) -> String {
        self.config.resolve_credential_key()
    }

    /// Borrowed view of the wrapped config.
    #[must_use]
    pub fn config(&self) -> &AuthConfig {
        &self.config
    }

    /// Run the resolution workflow.
    pub async fn resolve(
        &self,
        app: &str,
        user: &str,
        credentials: Option<&dyn CredentialService>,
    ) -> Result<ResolveOutcome> {
        let raw = self
            .config
            .raw_auth_credential
            .as_ref()
            .ok_or_else(|| Error::config("AuthConfig.raw_auth_credential is required"))?;

        // Step 2: already-ready and not expired? hand back.
        let now = Utc::now().timestamp();
        if raw.is_ready() && !raw.is_expired(now) {
            return Ok(ResolveOutcome::Ready(raw.clone()));
        }

        let key = self.config.resolve_credential_key();

        // Step 3: try cache.
        if let Some(svc) = credentials {
            if let Some(cached) = svc.load(app, user, &key).await? {
                if cached.is_ready() && !cached.is_expired(now) {
                    return Ok(ResolveOutcome::Ready(cached));
                }
                // Cached but expired — fall through to refresh.
                if let Some(r) = self.refreshers.get(cached.auth_type) {
                    if let Some(refreshed) = r.refresh(&self.config, &cached).await? {
                        svc.save(app, user, &key, &refreshed).await?;
                        return Ok(ResolveOutcome::Ready(refreshed));
                    }
                }
            }
        }

        // Step 5: authorization-code flow with no consent yet → bubble out.
        if matches!(
            raw.auth_type,
            AuthCredentialType::OAuth2 | AuthCredentialType::OpenIdConnect
        ) && raw
            .oauth2
            .as_ref()
            .is_some_and(|o| o.auth_code.is_none() && o.access_token.is_none())
        {
            return Ok(ResolveOutcome::NeedsUserConsent(self.config.clone()));
        }

        // Step 6: exchange.
        if let Some(ex) = self.exchangers.get(raw.auth_type) {
            if let Some(exchanged) = ex.exchange(&self.config, raw).await? {
                if let Some(svc) = credentials {
                    svc.save(app, user, &key, &exchanged).await?;
                }
                return Ok(ResolveOutcome::Ready(exchanged));
            }
        }

        // Step 6b: custom provider escape hatch.
        if let Some(prov) = self.providers.get(self.config.auth_scheme.kind()) {
            if let Some(c) = prov.get_auth_credential(&self.config).await? {
                if let Some(svc) = credentials {
                    svc.save(app, user, &key, &c).await?;
                }
                return Ok(ResolveOutcome::Ready(c));
            }
        }

        Ok(ResolveOutcome::Misconfigured(format!(
            "no exchanger registered for {:?}; credential not ready",
            raw.auth_type
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::credential::AuthCredential;
    use crate::auth::scheme::{ApiKeyLocation, AuthScheme};
    use crate::auth::service::InMemoryCredentialService;

    #[tokio::test]
    async fn api_key_resolves_immediately() {
        let cfg = AuthConfig::new(AuthScheme::ApiKey {
            location: ApiKeyLocation::Header,
            name: "X-API-Key".into(),
            description: None,
        })
        .with_raw(AuthCredential::api_key("secret"));
        let mgr = CredentialManager::new(cfg);
        let svc = InMemoryCredentialService::new();
        match mgr.resolve("a", "u", Some(&svc)).await.unwrap() {
            ResolveOutcome::Ready(c) => assert_eq!(c.api_key.as_deref(), Some("secret")),
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[tokio::test]
    async fn oauth2_without_consent_returns_needs_user() {
        use crate::auth::credential::OAuth2Auth;
        use crate::auth::scheme::{OAuthFlow, OAuthFlows};

        let cfg = AuthConfig::new(AuthScheme::OAuth2 {
            flows: OAuthFlows {
                authorization_code: Some(OAuthFlow {
                    authorization_url: Some("https://p/authorize".into()),
                    token_url: "https://p/token".into(),
                    refresh_url: None,
                    scopes: Default::default(),
                }),
                ..OAuthFlows::default()
            },
            description: None,
        })
        .with_raw(AuthCredential::oauth2(OAuth2Auth {
            client_id: "abc".into(),
            client_secret: Some("xyz".into()),
            ..OAuth2Auth::default()
        }));
        let mgr = CredentialManager::new(cfg);
        let svc = InMemoryCredentialService::new();
        match mgr.resolve("a", "u", Some(&svc)).await.unwrap() {
            ResolveOutcome::NeedsUserConsent(_) => {}
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[tokio::test]
    async fn cached_credential_is_returned_when_raw_not_ready() {
        use crate::auth::credential::OAuth2Auth;
        use crate::auth::scheme::{OAuthFlow, OAuthFlows};

        // Raw credential carries client_id + secret but no access_token →
        // step 2 falls through; cache (step 3) hits and returns the cached
        // ready credential.
        let cfg = AuthConfig::new(AuthScheme::OAuth2 {
            flows: OAuthFlows {
                authorization_code: Some(OAuthFlow {
                    authorization_url: Some("https://p/authorize".into()),
                    token_url: "https://p/token".into(),
                    refresh_url: None,
                    scopes: Default::default(),
                }),
                ..OAuthFlows::default()
            },
            description: None,
        })
        .with_raw(AuthCredential::oauth2(OAuth2Auth {
            client_id: "abc".into(),
            client_secret: Some("xyz".into()),
            ..OAuth2Auth::default()
        }))
        .with_key("fixed");

        let cached = AuthCredential::oauth2(OAuth2Auth {
            client_id: "abc".into(),
            access_token: Some("CACHED_TOKEN".into()),
            ..OAuth2Auth::default()
        });
        let svc = InMemoryCredentialService::new();
        svc.save("a", "u", "fixed", &cached).await.unwrap();

        let mgr = CredentialManager::new(cfg);
        match mgr.resolve("a", "u", Some(&svc)).await.unwrap() {
            ResolveOutcome::Ready(c) => {
                assert_eq!(
                    c.oauth2.as_ref().and_then(|o| o.access_token.as_deref()),
                    Some("CACHED_TOKEN")
                );
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }
}

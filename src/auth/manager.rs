//! [`CredentialManager`] — the orchestrator that resolves a tool's auth needs
//! into a usable credential. Ports Python ADK's 8-step workflow.

use chrono::Utc;
use std::sync::Arc;

use crate::auth::config::AuthConfig;
use crate::auth::credential::{AuthCredential, AuthCredentialType, OAuth2Auth};
use crate::auth::exchanger::ExchangerRegistry;
use crate::auth::handler::AuthHandler;
use crate::auth::provider::AuthProviderRegistry;
use crate::auth::refresher::RefresherRegistry;
use crate::auth::scheme::AuthScheme;
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

/// Output of [`CredentialManager::begin_consent`]. The caller redirects the
/// user to `auth_uri`; when the provider redirects back with `code` + `state`,
/// pass everything to [`CredentialManager::complete_consent`] (which will
/// validate `state` matches and exchange `code`).
#[derive(Debug, Clone)]
pub struct ConsentRequest {
    /// URL the user should be sent to (contains `state`, `code_challenge`,
    /// `client_id`, scopes etc.).
    pub auth_uri: String,
    /// Opaque flow id the caller persists alongside its own UI state. Passed
    /// back to `complete_consent`.
    pub flow_id: String,
}

/// Persisted shape of an in-flight consent. Stored in the
/// [`CredentialService`] under `__pending_consent:<flow_id>` so we can
/// recover the CSRF state and PKCE verifier across the redirect (which may
/// happen in a different process).
const PENDING_CONSENT_PREFIX: &str = "__pending_consent:";

fn pending_consent_key(flow_id: &str) -> String {
    format!("{PENDING_CONSENT_PREFIX}{flow_id}")
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

    /// Start an OAuth 2.0 authorization-code consent flow.
    ///
    /// Generates a fresh CSRF `state` + PKCE verifier via the `oauth2` crate,
    /// persists them in `credentials` keyed by an opaque `flow_id`, and
    /// returns the URL the caller should redirect the user to. After the
    /// provider redirects back, call [`Self::complete_consent`] with the
    /// `flow_id`, the inbound `state`, and the inbound authorization `code` —
    /// it will reject any mismatched state, perform the token exchange, and
    /// save the resolved credential under the regular cache key.
    ///
    /// **Requires** a `credentials` service: the verifier and state must
    /// outlive the HTTP redirect, so transient `None` storage isn't an
    /// option here.
    pub async fn begin_consent(
        &self,
        credentials: &dyn CredentialService,
    ) -> Result<ConsentRequest> {
        let raw = self
            .config
            .raw_auth_credential
            .as_ref()
            .ok_or_else(|| Error::config("AuthConfig.raw_auth_credential is required"))?;
        let oauth2 = raw
            .oauth2
            .as_ref()
            .ok_or_else(|| Error::config("begin_consent requires an OAuth2 credential"))?;
        if !matches!(
            self.config.auth_scheme,
            AuthScheme::OAuth2 { .. } | AuthScheme::OpenIdConnect { .. }
        ) {
            return Err(Error::config(
                "begin_consent requires an OAuth2 / OpenIdConnect scheme",
            ));
        }

        let mut populated = oauth2.clone();
        attach_flow_endpoints(&mut populated, &self.config.auth_scheme);
        let handler = AuthHandler::from_oauth2(&populated)?;
        let (auth_uri, state, verifier) = handler.authorize_url(&populated.scopes);

        // Persist the in-flight verifier + state so `complete_consent` can
        // validate the inbound callback. `flow_id` is what the caller hands
        // back; we use `state` itself as the flow id since it's already
        // a cryptographically-random opaque token from the `oauth2` crate.
        let flow_id = state.clone();
        let pending = AuthCredential::oauth2(OAuth2Auth {
            client_id: populated.client_id.clone(),
            client_secret: populated.client_secret.clone(),
            auth_uri: populated.auth_uri.clone(),
            token_uri: populated.token_uri.clone(),
            redirect_uri: populated.redirect_uri.clone(),
            state: Some(state),
            code_verifier: Some(verifier),
            scopes: populated.scopes.clone(),
            ..OAuth2Auth::default()
        });
        // App / user are not yet known at this point — store under a
        // process-wide bucket. Callers that need multi-tenant isolation can
        // override `begin_consent_for` (see below).
        credentials
            .save(
                "__adk",
                "__pending",
                &pending_consent_key(&flow_id),
                &pending,
            )
            .await?;

        Ok(ConsentRequest { auth_uri, flow_id })
    }

    /// Complete an OAuth 2.0 authorization-code consent flow.
    ///
    /// `callback_state` and `callback_code` are the `state` and `code` query
    /// params received at the provider's redirect_uri. Validates that
    /// `callback_state == flow_id` (the persisted state), exchanges the code
    /// for an access token using the PKCE verifier persisted by
    /// `begin_consent`, and writes the resolved credential under the regular
    /// cache key for `(app, user)`. Returns the exchanged credential.
    pub async fn complete_consent(
        &self,
        app: &str,
        user: &str,
        flow_id: &str,
        callback_state: &str,
        callback_code: &str,
        credentials: &dyn CredentialService,
    ) -> Result<AuthCredential> {
        // Constant-time-ish equality (don't reveal mismatch length via early
        // bail on prefix).
        if !constant_time_eq(callback_state.as_bytes(), flow_id.as_bytes()) {
            return Err(Error::other(
                "OAuth2 callback `state` does not match the flow id (possible CSRF)",
            ));
        }

        let pending_key = pending_consent_key(flow_id);
        let pending = credentials
            .load("__adk", "__pending", &pending_key)
            .await?
            .ok_or_else(|| {
                Error::other(format!(
                    "no pending consent for flow_id {flow_id:?} (expired or already used)"
                ))
            })?;
        let pending_oauth2 = pending
            .oauth2
            .as_ref()
            .ok_or_else(|| Error::other("pending consent payload is not OAuth2"))?;
        let verifier = pending_oauth2
            .code_verifier
            .as_deref()
            .ok_or_else(|| Error::other("pending consent has no PKCE verifier"))?;
        let stored_state = pending_oauth2.state.as_deref().unwrap_or("");
        if !constant_time_eq(stored_state.as_bytes(), flow_id.as_bytes()) {
            return Err(Error::other(
                "pending consent state mismatch (possible replay)",
            ));
        }

        let handler = AuthHandler::from_oauth2(pending_oauth2)?;
        let tok = handler.exchange_code(callback_code, verifier).await?;
        let mut new = pending_oauth2.clone();
        // Clear the one-shot fields so the resolved credential isn't
        // re-exchanged by mistake.
        new.state = None;
        new.code_verifier = None;
        new.auth_code = None;
        tok.apply_to(&mut new);
        let exchanged = AuthCredential::oauth2(new);

        // Cache under the regular key.
        let cache_key = self.config.resolve_credential_key();
        credentials.save(app, user, &cache_key, &exchanged).await?;

        // Single-use: remove the pending entry so a leaked redirect can't be
        // replayed.
        let _ = credentials.delete("__adk", "__pending", &pending_key).await;

        Ok(exchanged)
    }
}

/// Fill in `auth_uri` / `token_uri` from the scheme's authorization-code flow
/// if they aren't already set. Mirrors `exchanger::attach_flow_endpoints`.
fn attach_flow_endpoints(oauth2: &mut OAuth2Auth, scheme: &AuthScheme) {
    if let AuthScheme::OAuth2 { flows, .. } = scheme {
        if let Some(ac) = flows.authorization_code.as_ref() {
            if oauth2.auth_uri.is_none() {
                oauth2.auth_uri.clone_from(&ac.authorization_url);
            }
            if oauth2.token_uri.is_none() {
                oauth2.token_uri = Some(ac.token_url.clone());
            }
        }
    }
}

/// Constant-time `==` for two byte slices. Used to compare CSRF state /
/// flow ids without leaking length via early-return timing.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
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

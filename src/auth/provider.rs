//! [`BaseAuthProvider`] + [`AuthProviderRegistry`] — plug-in registry for
//! vendor-specific authentication schemes. Users can register a provider
//! that knows how to convert a custom [`crate::auth::scheme::AuthScheme`]
//! into a resolved [`crate::auth::credential::AuthCredential`].

use async_trait::async_trait;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

use crate::auth::config::AuthConfig;
use crate::auth::credential::AuthCredential;
use crate::error::Result;

/// Resolves a custom auth scheme to a credential.
#[async_trait]
pub trait BaseAuthProvider: Send + Sync + std::fmt::Debug + 'static {
    /// Get a credential for `config`. Return `None` if not applicable.
    async fn get_auth_credential(&self, config: &AuthConfig) -> Result<Option<AuthCredential>>;
}

/// Thread-safe map of custom `auth_scheme.kind()` tags → provider.
#[derive(Debug, Default)]
pub struct AuthProviderRegistry {
    by_tag: RwLock<HashMap<String, Arc<dyn BaseAuthProvider>>>,
}

impl AuthProviderRegistry {
    /// Empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `provider` under `tag` (the `kind()` of the scheme it handles).
    pub fn register(&self, tag: impl Into<String>, provider: Arc<dyn BaseAuthProvider>) {
        self.by_tag.write().insert(tag.into(), provider);
    }

    /// Look up a provider by tag.
    #[must_use]
    pub fn get(&self, tag: &str) -> Option<Arc<dyn BaseAuthProvider>> {
        self.by_tag.read().get(tag).cloned()
    }
}

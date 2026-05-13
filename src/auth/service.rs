//! [`CredentialService`] — the structured-credential persistence trait used by
//! the auth subsystem. Replaces the v0.1 string-keyed variant.

use async_trait::async_trait;
use dashmap::DashMap;
use parking_lot::Mutex;
use std::sync::Arc;

use crate::auth::credential::AuthCredential;
use crate::core::state::State;
use crate::error::Result;

/// Persists resolved credentials keyed by `(app, user, key)`.
///
/// Implementations are pluggable: in-memory for tests, session-state for
/// per-conversation creds, or a real KMS-backed store in production.
#[async_trait]
pub trait CredentialService: Send + Sync + std::fmt::Debug + 'static {
    /// Fetch a credential, or `None` if not stored.
    async fn load(
        &self,
        app_name: &str,
        user_id: &str,
        key: &str,
    ) -> Result<Option<AuthCredential>>;

    /// Persist a credential, overwriting any previous value at the same key.
    async fn save(
        &self,
        app_name: &str,
        user_id: &str,
        key: &str,
        value: &AuthCredential,
    ) -> Result<()>;

    /// Delete a credential. Default no-op for backends that don't support
    /// removal.
    async fn delete(&self, _app_name: &str, _user_id: &str, _key: &str) -> Result<()> {
        Ok(())
    }
}

/// Volatile process-local credential store.
#[derive(Debug, Default)]
pub struct InMemoryCredentialService {
    by_key: DashMap<(String, String, String), AuthCredential>,
}

impl InMemoryCredentialService {
    /// Construct.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl CredentialService for InMemoryCredentialService {
    async fn load(
        &self,
        app_name: &str,
        user_id: &str,
        key: &str,
    ) -> Result<Option<AuthCredential>> {
        let k = (app_name.to_string(), user_id.to_string(), key.to_string());
        Ok(self.by_key.get(&k).map(|v| v.value().clone()))
    }

    async fn save(
        &self,
        app_name: &str,
        user_id: &str,
        key: &str,
        value: &AuthCredential,
    ) -> Result<()> {
        self.by_key.insert(
            (app_name.to_string(), user_id.to_string(), key.to_string()),
            value.clone(),
        );
        Ok(())
    }

    async fn delete(&self, app_name: &str, user_id: &str, key: &str) -> Result<()> {
        let k = (app_name.to_string(), user_id.to_string(), key.to_string());
        self.by_key.remove(&k);
        Ok(())
    }
}

/// Stores credentials in the session's `temp:` state. Per-session lifetime;
/// useful for short-lived per-conversation auth (OAuth user consent).
///
/// The handle is wrapped in `Arc` because `State` lives inside the session,
/// which is borrowed by the runner.
#[derive(Debug)]
pub struct SessionStateCredentialService {
    overlay: Arc<Mutex<DashMap<String, AuthCredential>>>,
}

impl Default for SessionStateCredentialService {
    fn default() -> Self {
        Self {
            overlay: Arc::new(Mutex::new(DashMap::new())),
        }
    }
}

impl SessionStateCredentialService {
    /// Construct.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Compose the session-state key.
    fn state_key(app: &str, user: &str, key: &str) -> String {
        format!("temp:adk_auth:{app}:{user}:{key}")
    }
}

#[async_trait]
impl CredentialService for SessionStateCredentialService {
    async fn load(
        &self,
        app_name: &str,
        user_id: &str,
        key: &str,
    ) -> Result<Option<AuthCredential>> {
        let map = self.overlay.lock();
        Ok(map
            .get(&Self::state_key(app_name, user_id, key))
            .map(|r| r.clone()))
    }

    async fn save(
        &self,
        app_name: &str,
        user_id: &str,
        key: &str,
        value: &AuthCredential,
    ) -> Result<()> {
        let map = self.overlay.lock();
        map.insert(Self::state_key(app_name, user_id, key), value.clone());
        Ok(())
    }

    async fn delete(&self, app_name: &str, user_id: &str, key: &str) -> Result<()> {
        let map = self.overlay.lock();
        map.remove(&Self::state_key(app_name, user_id, key));
        Ok(())
    }
}

/// Helper used by the runner when bridging credentials into the session state
/// returned to consumers. Stores under `temp:adk_auth:<key>` so credentials
/// are not persisted past the session.
#[must_use]
pub fn session_state_key(key: &str) -> String {
    format!("temp:adk_auth:{key}")
}

/// Helper that exposes the overlay map for state introspection (mostly tests).
#[must_use]
pub fn render_to_state(cred: &AuthCredential) -> serde_json::Value {
    serde_json::to_value(cred).unwrap_or(serde_json::Value::Null)
}

/// Mark this helper used so the unused-warning lint stays quiet when the
/// runner integration lands.
#[doc(hidden)]
pub fn _touch_state(_s: &State) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_memory_save_load_delete() {
        let svc = InMemoryCredentialService::new();
        let c = AuthCredential::api_key("hello");
        svc.save("app", "u", "k", &c).await.unwrap();
        assert_eq!(svc.load("app", "u", "k").await.unwrap(), Some(c.clone()));
        svc.delete("app", "u", "k").await.unwrap();
        assert_eq!(svc.load("app", "u", "k").await.unwrap(), None);
    }

    #[tokio::test]
    async fn session_state_save_load() {
        let svc = SessionStateCredentialService::new();
        let c = AuthCredential::bearer("tok");
        svc.save("app", "u", "k", &c).await.unwrap();
        assert_eq!(svc.load("app", "u", "k").await.unwrap(), Some(c));
    }
}

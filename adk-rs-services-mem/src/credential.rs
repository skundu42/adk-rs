//! In-memory [`CredentialService`](adk_rs_core::CredentialService).

use async_trait::async_trait;
use dashmap::DashMap;

use adk_rs_core::CredentialService;
use adk_rs_error::Result;

/// Volatile credential store.
#[derive(Debug, Default)]
pub struct InMemoryCredentialService {
    by_key: DashMap<(String, String, String), String>,
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
    async fn load(&self, app_name: &str, user_id: &str, key: &str) -> Result<Option<String>> {
        let k = (app_name.to_string(), user_id.to_string(), key.to_string());
        Ok(self.by_key.get(&k).map(|v| v.value().clone()))
    }

    async fn save(&self, app_name: &str, user_id: &str, key: &str, value: &str) -> Result<()> {
        self.by_key.insert(
            (app_name.to_string(), user_id.to_string(), key.to_string()),
            value.to_string(),
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn save_then_load() {
        let svc = InMemoryCredentialService::new();
        svc.save("app", "u", "openai", "sk-...").await.unwrap();
        assert_eq!(
            svc.load("app", "u", "openai").await.unwrap().as_deref(),
            Some("sk-...")
        );
    }
}

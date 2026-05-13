//! In-memory [`ArtifactService`](crate::core::ArtifactService).

use async_trait::async_trait;
use dashmap::DashMap;

use crate::core::{ArtifactKey, ArtifactService};
use crate::error::Result;
use crate::genai_types::Part;

type Inner = DashMap<ArtifactKey, Vec<Part>>;

/// Volatile artifact store.
#[derive(Debug, Default)]
pub struct InMemoryArtifactService {
    versions: Inner,
}

impl InMemoryArtifactService {
    /// Construct.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn snapshot(&self, key: &ArtifactKey) -> Option<Vec<Part>> {
        self.versions.get(key).map(|v| v.value().clone())
    }
}

#[async_trait]
impl ArtifactService for InMemoryArtifactService {
    async fn save_artifact(&self, key: ArtifactKey, part: Part) -> Result<u64> {
        let mut entry = self.versions.entry(key).or_default();
        entry.push(part);
        Ok(entry.len() as u64)
    }

    async fn load_artifact(&self, key: ArtifactKey, version: Option<u64>) -> Result<Option<Part>> {
        let Some(snap) = self.snapshot(&key) else {
            return Ok(None);
        };
        if snap.is_empty() {
            return Ok(None);
        }
        if version == Some(0) {
            return Ok(None);
        }
        let v = version
            .map(|v| v.saturating_sub(1))
            .unwrap_or_else(|| (snap.len() as u64) - 1);
        Ok(snap.get(v as usize).cloned())
    }

    async fn list_artifact_keys(
        &self,
        app_name: &str,
        user_id: &str,
        session_id: &str,
    ) -> Result<Vec<String>> {
        Ok(self
            .versions
            .iter()
            .filter(|kv| {
                kv.key().app_name == app_name
                    && kv.key().user_id == user_id
                    && kv.key().session_id == session_id
            })
            .map(|kv| kv.key().filename.clone())
            .collect())
    }

    async fn delete_artifact(&self, key: ArtifactKey) -> Result<()> {
        self.versions.remove(&key);
        Ok(())
    }

    async fn list_versions(&self, key: ArtifactKey) -> Result<Vec<u64>> {
        Ok(self
            .versions
            .get(&key)
            .map(|v| (1..=v.value().len() as u64).collect())
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genai_types::Part;

    fn key() -> ArtifactKey {
        ArtifactKey::new("app", "u", "s", "file.txt")
    }

    #[tokio::test]
    async fn save_then_load_latest() {
        let svc = InMemoryArtifactService::new();
        assert_eq!(svc.save_artifact(key(), Part::text("v1")).await.unwrap(), 1);
        assert_eq!(svc.save_artifact(key(), Part::text("v2")).await.unwrap(), 2);
        let got = svc.load_artifact(key(), None).await.unwrap().unwrap();
        assert_eq!(got.as_text(), Some("v2"));
    }

    #[tokio::test]
    async fn load_specific_version() {
        let svc = InMemoryArtifactService::new();
        svc.save_artifact(key(), Part::text("v1")).await.unwrap();
        svc.save_artifact(key(), Part::text("v2")).await.unwrap();
        let got = svc.load_artifact(key(), Some(1)).await.unwrap().unwrap();
        assert_eq!(got.as_text(), Some("v1"));
    }

    #[tokio::test]
    async fn version_zero_is_missing() {
        let svc = InMemoryArtifactService::new();
        svc.save_artifact(key(), Part::text("v1")).await.unwrap();
        assert!(svc.load_artifact(key(), Some(0)).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_versions_returns_1_indexed() {
        let svc = InMemoryArtifactService::new();
        svc.save_artifact(key(), Part::text("v1")).await.unwrap();
        svc.save_artifact(key(), Part::text("v2")).await.unwrap();
        assert_eq!(svc.list_versions(key()).await.unwrap(), vec![1, 2]);
    }
}

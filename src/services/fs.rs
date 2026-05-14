//! Filesystem-backed [`ArtifactService`].
//!
//! [`ArtifactService`]: crate::core::ArtifactService
//!
//! Layout under `root`:
//!
//! ```text
//! <root>/<app>/<user>/<session>/<filename>/
//!     v000001.json   <-- one file per version, the Part as JSON
//!     v000002.json
//! ```

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::fs;

use crate::core::{ArtifactKey, ArtifactService};
use crate::error::{Error, Result, ServiceError};
use crate::genai_types::Part;

/// Filesystem artifact service.
#[derive(Debug, Clone)]
pub struct FileArtifactService {
    root: PathBuf,
}

impl FileArtifactService {
    /// Construct rooted at `root`. Will create the directory on save.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn dir(&self, key: &ArtifactKey) -> PathBuf {
        self.root
            .join(sanitize(&key.app_name))
            .join(sanitize(&key.user_id))
            .join(sanitize(&key.session_id))
            .join(sanitize(&key.filename))
    }
}

/// Sanitize one path component. Replaces unsafe characters with `_` and —
/// critically — rewrites dot-only components (`.`, `..`, `...`, ...) to `_`
/// so that an attacker-controlled `app_name` / `user_id` / `session_id` /
/// `filename` can't escape the artifact root via `..` segments.
///
/// Empty input collapses to `_` for the same reason — `Path::join("")` is
/// a no-op and would silently merge two adjacent components.
fn sanitize(s: &str) -> String {
    let mapped: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if mapped.is_empty() || mapped.chars().all(|c| c == '.') {
        return "_".to_string();
    }
    mapped
}

#[async_trait]
impl ArtifactService for FileArtifactService {
    async fn save_artifact(&self, key: ArtifactKey, part: Part) -> Result<u64> {
        let dir = self.dir(&key);
        fs::create_dir_all(&dir).await?;
        let mut max_version = 0_u64;
        let mut rd = fs::read_dir(&dir).await?;
        while let Some(entry) = rd.next_entry().await? {
            if let Some(n) = entry
                .file_name()
                .to_string_lossy()
                .strip_prefix('v')
                .and_then(|s| s.strip_suffix(".json"))
                .and_then(|s| s.parse::<u64>().ok())
            {
                if n > max_version {
                    max_version = n;
                }
            }
        }
        let v = max_version + 1;
        let path = dir.join(format!("v{v:06}.json"));
        fs::write(&path, serde_json::to_vec(&part)?).await?;
        Ok(v)
    }

    async fn load_artifact(&self, key: ArtifactKey, version: Option<u64>) -> Result<Option<Part>> {
        let dir = self.dir(&key);
        if !exists(&dir).await {
            return Ok(None);
        }
        let v = match version {
            Some(v) => v,
            None => latest_version(&dir).await?.unwrap_or(0),
        };
        if v == 0 {
            return Ok(None);
        }
        let path = dir.join(format!("v{v:06}.json"));
        if !exists(&path).await {
            return Ok(None);
        }
        let bytes = fs::read(&path).await?;
        Ok(Some(serde_json::from_slice(&bytes)?))
    }

    async fn list_artifact_keys(
        &self,
        app_name: &str,
        user_id: &str,
        session_id: &str,
    ) -> Result<Vec<String>> {
        let dir = self
            .root
            .join(sanitize(app_name))
            .join(sanitize(user_id))
            .join(sanitize(session_id));
        if !exists(&dir).await {
            return Ok(vec![]);
        }
        let mut out = Vec::new();
        let mut rd = fs::read_dir(&dir).await?;
        while let Some(entry) = rd.next_entry().await? {
            if entry.metadata().await?.is_dir() {
                out.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        Ok(out)
    }

    async fn delete_artifact(&self, key: ArtifactKey) -> Result<()> {
        let dir = self.dir(&key);
        if !exists(&dir).await {
            return Err(Error::Service(ServiceError::ArtifactNotFound(format!(
                "{:?}",
                key
            ))));
        }
        fs::remove_dir_all(dir).await?;
        Ok(())
    }

    async fn list_versions(&self, key: ArtifactKey) -> Result<Vec<u64>> {
        let dir = self.dir(&key);
        if !exists(&dir).await {
            return Ok(vec![]);
        }
        let mut versions = Vec::new();
        let mut rd = fs::read_dir(&dir).await?;
        while let Some(entry) = rd.next_entry().await? {
            if let Some(n) = entry
                .file_name()
                .to_string_lossy()
                .strip_prefix('v')
                .and_then(|s| s.strip_suffix(".json"))
                .and_then(|s| s.parse::<u64>().ok())
            {
                versions.push(n);
            }
        }
        versions.sort_unstable();
        Ok(versions)
    }
}

async fn exists(p: &Path) -> bool {
    fs::metadata(p).await.is_ok()
}

async fn latest_version(dir: &Path) -> Result<Option<u64>> {
    let mut max = None;
    let mut rd = fs::read_dir(dir).await?;
    while let Some(entry) = rd.next_entry().await? {
        if let Some(n) = entry
            .file_name()
            .to_string_lossy()
            .strip_prefix('v')
            .and_then(|s| s.strip_suffix(".json"))
            .and_then(|s| s.parse::<u64>().ok())
        {
            if max.is_none_or(|m| n > m) {
                max = Some(n);
            }
        }
    }
    Ok(max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genai_types::Part;

    #[tokio::test]
    async fn save_and_load_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = FileArtifactService::new(tmp.path());
        let k = ArtifactKey::new("app", "u", "s", "note.txt");
        let v1 = svc
            .save_artifact(k.clone(), Part::text("hello"))
            .await
            .unwrap();
        assert_eq!(v1, 1);
        let v2 = svc
            .save_artifact(k.clone(), Part::text("world"))
            .await
            .unwrap();
        assert_eq!(v2, 2);
        let latest = svc.load_artifact(k.clone(), None).await.unwrap().unwrap();
        assert_eq!(latest.as_text(), Some("world"));
        let v1part = svc
            .load_artifact(k.clone(), Some(1))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(v1part.as_text(), Some("hello"));
        assert_eq!(svc.list_versions(k.clone()).await.unwrap(), vec![1, 2]);
        let names = svc.list_artifact_keys("app", "u", "s").await.unwrap();
        assert_eq!(names, vec!["note.txt"]);
        svc.delete_artifact(k.clone()).await.unwrap();
        assert!(svc.load_artifact(k, None).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn sanitize_funky_names() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = FileArtifactService::new(tmp.path());
        let k = ArtifactKey::new("a/p", "u", "s", "../../oops.txt");
        svc.save_artifact(k.clone(), Part::text("x")).await.unwrap();
        assert!(svc.load_artifact(k, None).await.unwrap().is_some());
    }

    /// Regression: an attacker-controlled component equal to `..` (or `.`,
    /// `...`, etc.) used to survive `sanitize` because `.` was on the
    /// allow-list, and the resulting `Path::join("..")` walked one level out
    /// of the artifact root. `delete_artifact` reaching `remove_dir_all` on
    /// that path could blow away unrelated data.
    #[tokio::test]
    async fn dotdot_component_cannot_escape_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let svc = FileArtifactService::new(&root);

        for evil in ["..", ".", "...", ""] {
            // Each evil string is tried in every position.
            for (a, u, s, f) in [
                (evil, "u", "s", "f"),
                ("a", evil, "s", "f"),
                ("a", "u", evil, "f"),
                ("a", "u", "s", evil),
            ] {
                let k = ArtifactKey::new(a, u, s, f);
                svc.save_artifact(k.clone(), Part::text("payload"))
                    .await
                    .unwrap();
                let dir = svc.dir(&k);
                // Every component must live strictly below `root`.
                let rel = dir.strip_prefix(&root).expect("dir escaped root");
                for comp in rel.components() {
                    match comp {
                        std::path::Component::Normal(s) => {
                            let s = s.to_string_lossy();
                            assert_ne!(&*s, "..", "dotdot component {a:?}/{u:?}/{s:?}/{f:?}");
                            assert_ne!(&*s, ".", "dot component {a:?}/{u:?}/{s:?}/{f:?}");
                            assert!(!s.is_empty(), "empty component {a:?}/{u:?}/{s:?}/{f:?}");
                        }
                        std::path::Component::CurDir | std::path::Component::ParentDir => {
                            panic!("unsafe component {comp:?} in {dir:?}");
                        }
                        _ => {}
                    }
                }
                // delete_artifact may not call remove_dir_all on anything
                // above `root` — verified implicitly: the dir is strictly
                // below root, so remove_dir_all stays scoped.
                svc.delete_artifact(k).await.unwrap();
            }
        }

        // The temporary directory itself must still exist after all that.
        assert!(root.exists(), "artifact root was deleted");
    }
}

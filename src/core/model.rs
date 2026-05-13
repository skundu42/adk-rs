//! The [`Model`] trait abstracts an LLM provider.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::core::llm_request::LlmRequest;
use crate::core::llm_response::LlmResponse;
use crate::core::stream::LlmResponseStream;
use crate::error::Result;

/// A unified LLM-provider abstraction.
///
/// Implementations live in `adk-providers-*` crates.
#[async_trait]
pub trait Model: Send + Sync + std::fmt::Debug + 'static {
    /// The canonical name of this model instance (e.g. `"gemini-2.5-flash"`).
    fn name(&self) -> &str;

    /// Glob-like name patterns this provider can serve. Used by
    /// [`ModelRegistry`] to dispatch requests by model name.
    fn supported_models(&self) -> &'static [&'static str];

    /// Single-shot generation.
    async fn generate_content(&self, req: LlmRequest) -> Result<LlmResponse>;

    /// Streaming generation. The default implementation yields the single
    /// non-streaming response wrapped in a one-element stream.
    async fn stream_generate_content(&self, req: LlmRequest) -> Result<LlmResponseStream> {
        use futures::stream;
        let resp = self.generate_content(req).await?;
        Ok(Box::pin(stream::once(async move { Ok(resp) })))
    }
}

/// A registry mapping model-name patterns → provider instances.
///
/// Lookups walk patterns in insertion order; the first pattern whose
/// glob matches the requested model name wins. There is no global state.
#[derive(Debug, Default, Clone)]
pub struct ModelRegistry {
    by_name: BTreeMap<String, Arc<dyn Model>>,
    patterns: Vec<(globset::GlobMatcher, Arc<dyn Model>)>,
}

impl ModelRegistry {
    /// Construct an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an instance under all of its `supported_models` patterns.
    pub fn register(&mut self, model: Arc<dyn Model>) {
        let patterns = model.supported_models();
        for &p in patterns {
            // Each pattern is a glob (e.g. `"gemini-*"`) or an exact name.
            let glob = match globset::Glob::new(p) {
                Ok(g) => g.compile_matcher(),
                Err(_) => continue,
            };
            self.patterns.push((glob, model.clone()));
        }
        self.by_name.insert(model.name().to_string(), model);
    }

    /// Look up a model by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Model>> {
        if let Some(m) = self.by_name.get(name) {
            return Some(m.clone());
        }
        for (g, m) in &self.patterns {
            if g.is_match(name) {
                return Some(m.clone());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::testing::MockModel;

    #[test]
    fn registry_dispatches_exact_match() {
        let mut r = ModelRegistry::new();
        let m = Arc::new(MockModel::new("test")) as Arc<dyn Model>;
        r.register(m.clone());
        assert!(r.get("test").is_some());
        assert!(r.get("missing").is_none());
    }
}

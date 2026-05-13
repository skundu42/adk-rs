//! Plugin system — hooks invoked by the [`Runner`](crate::runner::runner::Runner) at
//! well-defined points.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::info;

use crate::core::{Event, InvocationContext};
use crate::error::Result;

/// User-overridable plugin contract.
///
/// All hooks have safe defaults so impls can override only what they need.
#[async_trait]
pub trait BasePlugin: Send + Sync + std::fmt::Debug + 'static {
    /// Called once when the plugin is registered.
    async fn on_register(&self) -> Result<()> {
        Ok(())
    }

    /// Called before each invocation begins.
    async fn before_run(&self, _ctx: &InvocationContext) -> Result<()> {
        Ok(())
    }

    /// Called for every event the runner yields.
    async fn on_event(&self, _ctx: &InvocationContext, _event: &Event) -> Result<()> {
        Ok(())
    }

    /// Called when the runner finishes (either gracefully or via error).
    async fn after_run(
        &self,
        _ctx: &InvocationContext,
        _err: Option<&crate::error::Error>,
    ) -> Result<()> {
        Ok(())
    }
}

/// Coordinates [`BasePlugin`] hook invocations.
#[derive(Default)]
pub struct PluginManager {
    plugins: Vec<Arc<dyn BasePlugin>>,
}

impl std::fmt::Debug for PluginManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginManager")
            .field("plugin_count", &self.plugins.len())
            .finish()
    }
}

impl PluginManager {
    /// Construct empty.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a plugin.
    pub async fn register(&mut self, p: Arc<dyn BasePlugin>) -> Result<()> {
        p.on_register().await?;
        self.plugins.push(p);
        Ok(())
    }

    pub(crate) async fn before_run(&self, ctx: &InvocationContext) -> Result<()> {
        for p in &self.plugins {
            p.before_run(ctx).await?;
        }
        Ok(())
    }
    pub(crate) async fn on_event(&self, ctx: &InvocationContext, ev: &Event) -> Result<()> {
        for p in &self.plugins {
            p.on_event(ctx, ev).await?;
        }
        Ok(())
    }
    pub(crate) async fn after_run(
        &self,
        ctx: &InvocationContext,
        err: Option<&crate::error::Error>,
    ) -> Result<()> {
        for p in &self.plugins {
            p.after_run(ctx, err).await?;
        }
        Ok(())
    }
}

/// Logs every event at `INFO` via the `tracing` facade.
#[derive(Debug, Default)]
pub struct LoggingPlugin;

#[async_trait]
impl BasePlugin for LoggingPlugin {
    async fn on_event(&self, _ctx: &InvocationContext, ev: &Event) -> Result<()> {
        let text = ev
            .response
            .content
            .as_ref()
            .map(|c| c.text_concat())
            .unwrap_or_default();
        info!(target: "adk::event", author = %ev.author, invocation = %ev.invocation_id, text = %text);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::tests_support::test_ctx;
    use crate::core::LlmResponse;
    use crate::genai_types::Content;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug, Default)]
    struct CountingPlugin {
        registered: AtomicUsize,
        before: AtomicUsize,
        events: AtomicUsize,
        after: AtomicUsize,
    }

    #[async_trait]
    impl BasePlugin for CountingPlugin {
        async fn on_register(&self) -> Result<()> {
            self.registered.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn before_run(&self, _: &InvocationContext) -> Result<()> {
            self.before.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn on_event(&self, _: &InvocationContext, _: &Event) -> Result<()> {
            self.events.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn after_run(
            &self,
            _: &InvocationContext,
            _: Option<&crate::error::Error>,
        ) -> Result<()> {
            self.after.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn register_calls_on_register_once() {
        let mut m = PluginManager::new();
        let p = Arc::new(CountingPlugin::default());
        m.register(p.clone()).await.unwrap();
        assert_eq!(p.registered.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn hooks_fan_out_to_every_plugin() {
        let mut m = PluginManager::new();
        let a = Arc::new(CountingPlugin::default());
        let b = Arc::new(CountingPlugin::default());
        m.register(a.clone()).await.unwrap();
        m.register(b.clone()).await.unwrap();

        let ctx = test_ctx();
        let ev = Event::new(
            "tester",
            LlmResponse {
                content: Some(Content::model_text("hi")),
                ..LlmResponse::default()
            },
        );
        m.before_run(&ctx).await.unwrap();
        m.on_event(&ctx, &ev).await.unwrap();
        m.after_run(&ctx, None).await.unwrap();

        for p in [&a, &b] {
            assert_eq!(p.before.load(Ordering::SeqCst), 1);
            assert_eq!(p.events.load(Ordering::SeqCst), 1);
            assert_eq!(p.after.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn logging_plugin_default_hooks_are_ok() {
        let mut m = PluginManager::new();
        m.register(Arc::new(LoggingPlugin)).await.unwrap();
        let ctx = test_ctx();
        let ev = Event::new(
            "tester",
            LlmResponse {
                content: Some(Content::model_text("hi")),
                ..LlmResponse::default()
            },
        );
        m.before_run(&ctx).await.unwrap();
        m.on_event(&ctx, &ev).await.unwrap();
        m.after_run(&ctx, None).await.unwrap();
    }
}

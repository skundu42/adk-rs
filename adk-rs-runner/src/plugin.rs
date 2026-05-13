//! Plugin system — hooks invoked by the [`Runner`](crate::runner::Runner) at
//! well-defined points.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::info;

use adk_rs_core::{Event, InvocationContext};
use adk_rs_error::Result;

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
        _err: Option<&adk_rs_error::Error>,
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
        err: Option<&adk_rs_error::Error>,
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

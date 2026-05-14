//! Library scaffolding for the `adk` CLI.
//!
//! Users build their own binary that registers agents and forwards to
//! [`App::run`]; we don't provide a single `adk` binary that loads user
//! agents dynamically (Rust has no equivalent to Python's `importlib`).
//!
//! Quick start:
//!
//! ```ignore
//! use std::sync::Arc;
//! fn main() -> crate::error::Result<()> {
//!     crate::cli::App::new("my_app")
//!         .register("greeter", Arc::new(my_greeter()))
//!         .run()
//! }
//! ```

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use futures::StreamExt;
use tracing::info;

use crate::agents::BaseAgent;
use crate::runner::Runner;
use crate::services::mem::InMemorySessionService;
use crate::telemetry::{LogFormat, TelemetryConfig};

/// Top-level CLI argument set.
#[derive(Debug, Parser)]
#[command(name = "adk", version, about = "Agent Development Kit CLI")]
pub struct Cli {
    /// Logging filter (`RUST_LOG`-style). Default: `info`.
    #[arg(long, env = "ADK_LOG")]
    pub log: Option<String>,
    /// Log output format.
    #[arg(long, default_value = "compact")]
    pub log_format: LogFormatArg,
    /// Subcommand.
    #[command(subcommand)]
    pub command: Command,
}

/// `--log-format`.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum LogFormatArg {
    /// Compact human-friendly output (default).
    Compact,
    /// Pretty multi-line output.
    Pretty,
    /// Newline-delimited JSON.
    Json,
}

impl From<LogFormatArg> for LogFormat {
    fn from(v: LogFormatArg) -> Self {
        match v {
            LogFormatArg::Compact => LogFormat::Compact,
            LogFormatArg::Pretty => LogFormat::Pretty,
            LogFormatArg::Json => LogFormat::Json,
        }
    }
}

/// Subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run a single user turn against a registered agent.
    Run {
        /// Agent name.
        #[arg(long)]
        agent: String,
        /// User id.
        #[arg(long, default_value = "anonymous")]
        user: String,
        /// Optional session id.
        #[arg(long)]
        session: Option<String>,
        /// The user message.
        message: String,
    },
    /// Start the dev HTTP server.
    Web {
        /// Listen address.
        #[arg(long, default_value = "127.0.0.1:8000")]
        bind: SocketAddr,
        /// Bearer token required on `Authorization: Bearer <token>`. When
        /// set, every request must present this token. Recommended whenever
        /// `--bind` is anything other than localhost.
        #[arg(long, env = "ADK_WEB_TOKEN")]
        auth_token: Option<String>,
        /// Bind a non-loopback address without an auth token. Refused by
        /// default to prevent accidentally exposing the agent control plane.
        #[arg(long)]
        dangerously_allow_unauthenticated_remote: bool,
    },
    /// Run an eval set against a registered agent.
    Eval {
        /// Path to the JSON eval set.
        #[arg(long)]
        set: std::path::PathBuf,
        /// Agent name.
        #[arg(long)]
        agent: String,
    },
    /// Print the version.
    Version,
}

/// Registered agents app.
pub struct App {
    name: String,
    agents: HashMap<String, Arc<dyn BaseAgent>>,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("name", &self.name)
            .field("agents", &self.agents.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl App {
    /// Construct empty.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            agents: HashMap::new(),
        }
    }

    /// Register an agent.
    #[must_use]
    pub fn register(mut self, name: impl Into<String>, agent: Arc<dyn BaseAgent>) -> Self {
        self.agents.insert(name.into(), agent);
        self
    }

    /// Parse CLI args and run.
    pub fn run(self) -> crate::error::Result<()> {
        let cli = Cli::parse();
        crate::telemetry::init(TelemetryConfig {
            filter: cli.log,
            format: cli.log_format.into(),
            ..TelemetryConfig::default()
        })?;
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| crate::error::Error::other(format!("tokio: {e}")))?;
        rt.block_on(self.run_async(cli.command))
    }

    /// Async dispatch — useful for tests.
    pub async fn run_async(self, cmd: Command) -> crate::error::Result<()> {
        match cmd {
            Command::Run {
                agent,
                user,
                session,
                message,
            } => {
                let runner = self.build_runner(&agent)?;
                let mut s = runner.run(&user, session.as_deref(), &message).await?;
                while let Some(ev) = s.next().await {
                    let ev = ev?;
                    if let Some(c) = ev.response.content.as_ref() {
                        let text = c.text_concat();
                        if !text.is_empty() {
                            // intentional stdout: this is the CLI's job.
                            #[allow(clippy::print_stdout)]
                            {
                                println!("{}", text)
                            }
                        }
                    }
                }
                Ok(())
            }
            Command::Web {
                bind,
                auth_token,
                dangerously_allow_unauthenticated_remote,
            } => {
                let mut runners = HashMap::new();
                for (name, agent) in &self.agents {
                    runners.insert(name.clone(), Arc::new(self.runner_for(agent.clone())));
                }
                let runners = Arc::new(runners);
                let state = match auth_token {
                    Some(t) => crate::server::AppState::with_bearer_token(runners, t),
                    None => crate::server::AppState::unauthenticated(runners),
                };
                info!("starting dev server on http://{bind}");
                crate::server::serve_with(
                    bind,
                    state,
                    crate::server::ServeOptions {
                        dangerously_allow_unauthenticated_remote,
                    },
                )
                .await
            }
            Command::Eval { set, agent } => {
                let bytes = tokio::fs::read(set).await?;
                let set: crate::eval::EvalSet = serde_json::from_slice(&bytes)?;
                let agent = self.find_agent(&agent)?;
                let runner = crate::eval::EvalRunner::new(
                    agent,
                    self.name.clone(),
                    "eval-user",
                    vec![
                        Arc::new(crate::eval::TrajectoryMatch::new(1.0)),
                        Arc::new(crate::eval::ResponseMatch::new(0.5)),
                    ],
                );
                let report = runner.run_set(&set).await?;
                #[allow(clippy::print_stdout)]
                {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&report).unwrap_or_default()
                    )
                }
                Ok(())
            }
            Command::Version => {
                #[allow(clippy::print_stdout)]
                {
                    println!("adk-rs {}", env!("CARGO_PKG_VERSION"))
                }
                Ok(())
            }
        }
    }

    fn find_agent(&self, name: &str) -> crate::error::Result<Arc<dyn BaseAgent>> {
        self.agents
            .get(name)
            .cloned()
            .ok_or_else(|| crate::error::Error::not_found(format!("agent {name}")))
    }

    fn build_runner(&self, agent_name: &str) -> crate::error::Result<Runner> {
        let agent = self.find_agent(agent_name)?;
        Ok(self.runner_for(agent))
    }

    fn runner_for(&self, agent: Arc<dyn BaseAgent>) -> Runner {
        Runner::builder()
            .app_name(self.name.clone())
            .agent(agent)
            .session_service(Arc::new(InMemorySessionService::new()))
            .auto_create_session(true)
            .build()
            .expect("Runner::build with default services must succeed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::LlmAgent;
    use crate::core::Model;
    use crate::core::testing::MockModel;

    #[tokio::test]
    async fn run_command_prints_text() {
        let m = Arc::new(MockModel::new("m"));
        m.push_text("yo");
        let agent: Arc<dyn BaseAgent> = Arc::new(
            LlmAgent::builder("greet")
                .model(m.clone() as Arc<dyn Model>)
                .instruction("greet")
                .build()
                .unwrap(),
        );
        let app = App::new("hello").register("greet", agent);
        app.run_async(Command::Run {
            agent: "greet".into(),
            user: "u".into(),
            session: None,
            message: "hi".into(),
        })
        .await
        .unwrap();
    }
}

//! [`ParallelAgent`] — fan out to all sub-agents and merge their event streams.

use std::sync::Arc;

use async_stream::try_stream;
use async_trait::async_trait;
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::core::{Event, EventStream, InvocationContext};
use crate::error::{Error, Result};

use crate::agents::base::BaseAgent;

/// Run sub-agents concurrently and merge their event streams.
#[derive(Debug)]
pub struct ParallelAgent {
    name: String,
    description: String,
    sub_agents: Vec<Arc<dyn BaseAgent>>,
}

impl ParallelAgent {
    /// Construct.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        sub_agents: Vec<Arc<dyn BaseAgent>>,
    ) -> Result<Self> {
        if sub_agents.is_empty() {
            return Err(Error::config(
                "ParallelAgent requires at least one sub_agent",
            ));
        }
        Ok(Self {
            name: name.into(),
            description: description.into(),
            sub_agents,
        })
    }
}

#[async_trait]
impl BaseAgent for ParallelAgent {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn sub_agents(&self) -> &[Arc<dyn BaseAgent>] {
        &self.sub_agents
    }
    async fn run(self: Arc<Self>, ctx: Arc<InvocationContext>) -> Result<EventStream<'static>> {
        let (tx, rx) = mpsc::channel::<Result<Event>>(64);
        for (i, sub) in self.sub_agents.iter().enumerate() {
            let sub = sub.clone();
            let ctx = ctx.clone();
            let tx = tx.clone();
            let branch = format!("{}.{}", self.name, i);
            tokio::spawn(async move {
                match sub.run(ctx).await {
                    Ok(mut stream) => {
                        while let Some(ev) = stream.next().await {
                            let mut ev = match ev {
                                Ok(e) => e,
                                Err(e) => {
                                    let _ = tx.send(Err(e)).await;
                                    continue;
                                }
                            };
                            if ev.branch.is_none() {
                                ev.branch = Some(branch.clone());
                            }
                            if tx.send(Ok(ev)).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e)).await;
                    }
                }
            });
        }
        drop(tx); // close after all spawns hold their own clones
        let stream = try_stream! {
            let mut rx = ReceiverStream::new(rx);
            while let Some(ev) = rx.next().await {
                yield ev?;
            }
        };
        Ok(Box::pin(stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::tests_support::{stub_agent, test_ctx};
    use std::collections::HashSet;

    #[tokio::test]
    async fn empty_sub_agents_rejected() {
        let err = ParallelAgent::new("p", "d", vec![]).unwrap_err();
        assert!(err.to_string().contains("at least one sub_agent"));
    }

    #[tokio::test]
    async fn fans_out_to_all_children_and_tags_branch() {
        let a = stub_agent("a", &["from-a"], false);
        let b = stub_agent("b", &["from-b"], false);
        let par = Arc::new(ParallelAgent::new("par", "", vec![a, b]).unwrap());
        let mut stream = par.run(test_ctx()).await.unwrap();
        let mut authors = HashSet::new();
        let mut branches = HashSet::new();
        while let Some(ev) = stream.next().await {
            let ev = ev.unwrap();
            authors.insert(ev.author);
            if let Some(b) = ev.branch {
                branches.insert(b);
            }
        }
        assert_eq!(authors, HashSet::from(["a".into(), "b".into()]));
        assert_eq!(branches.len(), 2, "each branch should be tagged uniquely");
    }
}

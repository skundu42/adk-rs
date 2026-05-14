//! [`AuthPreprocessor`] — absorbs `adk_request_credential` function responses
//! from the user-authored event, stores the resolved credential in the
//! credential service, and exposes the original deferred tool call ids so the
//! runner can replay them on the next turn.

use std::sync::Arc;

use crate::auth::config::AuthConfig;
use crate::auth::service::CredentialService;
use crate::auth::{REQUEST_CREDENTIAL_FUNCTION_NAME, TOOLSET_AUTH_CREDENTIAL_ID_PREFIX};
use crate::core::event::Event;
use crate::error::Result;

/// Outcome of [`AuthPreprocessor::process_event`] — what the runner should
/// do with the inspected event.
#[derive(Debug, Default)]
pub struct PreprocessOutcome {
    /// Function-call ids whose deferred tool calls are now unblocked.
    pub resumed_tool_call_ids: Vec<String>,
    /// Function-call ids the user accepted at a toolset (pre-listing) level.
    pub resumed_toolset_ids: Vec<String>,
}

/// Stateless processor invoked by the runner on each user-authored event.
#[derive(Debug, Default)]
pub struct AuthPreprocessor;

impl AuthPreprocessor {
    /// New.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Walk `event`'s function responses for `adk_request_credential`,
    /// extract the [`AuthConfig`] payload, persist its
    /// `exchanged_auth_credential` via `credentials`, and return the original
    /// function call ids that the runner can now replay.
    pub async fn process_event(
        &self,
        event: &Event,
        app_name: &str,
        user_id: &str,
        credentials: Option<Arc<dyn CredentialService>>,
    ) -> Result<PreprocessOutcome> {
        let mut out = PreprocessOutcome::default();
        if event.author != "user" {
            // Only user-authored events carry the function responses.
            return Ok(out);
        }

        let Some(content) = event.response.content.as_ref() else {
            return Ok(out);
        };

        for part in &content.parts {
            let crate::genai_types::Part::FunctionResponse(fr) = part else {
                continue;
            };
            if fr.name != REQUEST_CREDENTIAL_FUNCTION_NAME {
                continue;
            }

            // Decode the AuthConfig payload from the response value.
            let cfg: AuthConfig = match serde_json::from_value(fr.response.clone()) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("auth preprocessor: malformed AuthConfig response: {e}");
                    continue;
                }
            };

            // The function call id is what the runner originally synthesised.
            // Toolset auth requests use a known prefix so we can tell them
            // apart from per-tool auth. We *require* an id: the agent
            // synthesises one for id-less Gemini calls before persisting the
            // event, so a missing id here means a malformed caller — log and
            // drop rather than match against `""` (which would silently fail
            // to resume).
            let Some(id) = fr.id.clone().filter(|s| !s.is_empty()) else {
                tracing::warn!(
                    "auth preprocessor: dropping adk_request_credential response with no \
                     function_call_id; agent emits a synthesised id for every call, so this \
                     usually means a malformed user-side response"
                );
                continue;
            };
            if id.starts_with(TOOLSET_AUTH_CREDENTIAL_ID_PREFIX) {
                out.resumed_toolset_ids.push(id);
            } else {
                out.resumed_tool_call_ids.push(id);
            }

            // Persist the exchanged credential, if any.
            if let (Some(svc), Some(ex)) =
                (credentials.as_ref(), cfg.exchanged_auth_credential.as_ref())
            {
                let key = cfg.resolve_credential_key();
                svc.save(app_name, user_id, &key, ex).await?;
            }
        }

        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::credential::AuthCredential;
    use crate::auth::scheme::{ApiKeyLocation, AuthScheme};
    use crate::auth::service::InMemoryCredentialService;
    use crate::core::event::Event;
    use crate::core::llm_response::LlmResponse;
    use crate::genai_types::{Content, FunctionResponse, Part, Role};

    fn user_event_with_auth_response(cfg: &AuthConfig, call_id: &str) -> Event {
        let response = serde_json::to_value(cfg).unwrap();
        let part = Part::FunctionResponse(FunctionResponse {
            id: Some(call_id.into()),
            name: REQUEST_CREDENTIAL_FUNCTION_NAME.into(),
            response,
            will_continue: None,
            scheduling: None,
        });
        Event::new(
            "user",
            LlmResponse {
                content: Some(Content {
                    role: Role::User,
                    parts: vec![part],
                }),
                ..LlmResponse::default()
            },
        )
    }

    #[tokio::test]
    async fn absorbs_credential_and_returns_resumed_ids() {
        let cfg = AuthConfig::new(AuthScheme::ApiKey {
            location: ApiKeyLocation::Header,
            name: "X".into(),
            description: None,
        })
        .with_raw(AuthCredential::api_key("raw"))
        .with_key("k1");
        let mut cfg_with_ex = cfg.clone();
        cfg_with_ex.exchanged_auth_credential = Some(AuthCredential::api_key("RESOLVED"));

        let event = user_event_with_auth_response(&cfg_with_ex, "fc-1");
        let svc: Arc<dyn CredentialService> = Arc::new(InMemoryCredentialService::new());
        let out = AuthPreprocessor::new()
            .process_event(&event, "app", "user", Some(svc.clone()))
            .await
            .unwrap();
        assert_eq!(out.resumed_tool_call_ids, vec!["fc-1".to_string()]);
        let stored = svc.load("app", "user", "k1").await.unwrap();
        assert_eq!(stored, Some(AuthCredential::api_key("RESOLVED")));
    }

    #[tokio::test]
    async fn toolset_prefix_routes_to_separate_bucket() {
        let cfg = AuthConfig::new(AuthScheme::ApiKey {
            location: ApiKeyLocation::Header,
            name: "X".into(),
            description: None,
        })
        .with_raw(AuthCredential::api_key("raw"));
        let event =
            user_event_with_auth_response(&cfg, &format!("{TOOLSET_AUTH_CREDENTIAL_ID_PREFIX}abc"));
        let out = AuthPreprocessor::new()
            .process_event(&event, "app", "user", None)
            .await
            .unwrap();
        assert!(out.resumed_tool_call_ids.is_empty());
        assert_eq!(out.resumed_toolset_ids.len(), 1);
    }
}

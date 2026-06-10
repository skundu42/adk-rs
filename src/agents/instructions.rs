//! Instruction templating — injects session state and artifact contents into
//! `{placeholder}` references inside static instructions.
//!
//! Mirrors Python ADK's `instructions` request processor:
//!
//! * `{key}` — replaced with the state value for `key`. Errors if missing.
//! * `{key?}` — optional; replaced with the empty string if missing.
//! * `{app:key}` / `{user:key}` / `{temp:key}` — prefixed state keys.
//! * `{artifact.name}` — replaced with the named artifact's content.
//! * Anything that is not a valid state name (e.g. `{1, 2}`, `{ }`) is left
//!   untouched, so JSON snippets in instructions survive.

use crate::core::ReadonlyContext;
use crate::error::{Error, Result};
use crate::genai_types::Part;

/// Replace `{...}` placeholders in `template` with values from the session
/// state (or artifact service) reachable through `ctx`.
pub async fn inject_session_state(template: &str, ctx: &ReadonlyContext) -> Result<String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;

    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];

        // Match the Python pattern `{+[^{}]*}+`: a run of `{`, a braceless
        // body, then a run of `}`.
        let open_len = after.chars().take_while(|&c| c == '{').count();
        let body_len = after[open_len..]
            .find(['{', '}'])
            .unwrap_or(after.len() - open_len);
        let close_start = open_len + body_len;
        let close_len = after[close_start..].chars().take_while(|&c| c == '}').count();

        if close_len == 0 {
            // Unterminated (next char is `{` or end of string): emit what we
            // consumed verbatim and keep scanning.
            out.push_str(&after[..close_start]);
            rest = &after[close_start..];
            continue;
        }

        let matched = &after[..close_start + close_len];
        let var = after[open_len..close_start].trim();
        match resolve_var(var, ctx).await? {
            Some(v) => out.push_str(&v),
            None => out.push_str(matched),
        }
        rest = &after[close_start + close_len..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Resolve one placeholder body. Returns `Ok(None)` when the body is not a
/// valid state reference and should be left as-is.
async fn resolve_var(var: &str, ctx: &ReadonlyContext) -> Result<Option<String>> {
    let (name, optional) = match var.strip_suffix('?') {
        Some(n) => (n, true),
        None => (var, false),
    };

    if let Some(artifact_name) = name.strip_prefix("artifact.") {
        let svc = ctx.invocation.artifact_service.as_ref().ok_or_else(|| {
            Error::config("instruction references {artifact.*} but no artifact service configured")
        })?;
        let key = crate::core::ArtifactKey::new(
            &ctx.invocation.app_name,
            &ctx.invocation.user_id,
            &ctx.invocation.session.lock().id,
            artifact_name,
        );
        return match svc.load_artifact(key, None).await? {
            Some(part) => Ok(Some(render_part(&part))),
            None if optional => Ok(Some(String::new())),
            None => Err(Error::not_found(format!("artifact {artifact_name}"))),
        };
    }

    if !is_valid_state_name(name) {
        return Ok(None);
    }
    let value = ctx.invocation.session.lock().state.get(name).cloned();
    match value {
        Some(v) => Ok(Some(render_value(&v))),
        None if optional => Ok(Some(String::new())),
        None => Err(Error::invalid_input(format!(
            "context variable not found: `{name}`"
        ))),
    }
}

/// `name` or `prefix:name` where prefix ∈ {app, user, temp} and `name` is an
/// identifier.
fn is_valid_state_name(name: &str) -> bool {
    fn is_identifier(s: &str) -> bool {
        let mut chars = s.chars();
        match chars.next() {
            Some(c) if c.is_alphabetic() || c == '_' => {}
            _ => return false,
        }
        chars.all(|c| c.is_alphanumeric() || c == '_')
    }
    match name.split_once(':') {
        Some((prefix, rest)) => {
            matches!(prefix, "app" | "user" | "temp") && is_identifier(rest)
        }
        None => is_identifier(name),
    }
}

fn render_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn render_part(p: &Part) -> String {
    match p {
        Part::Text(t) => t.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    use parking_lot::Mutex;
    use serde_json::json;

    use crate::core::{
        InvocationContext, InvocationOrigin, ReadonlyContext, RunConfig, Session, State,
    };
    use crate::services::mem::InMemorySessionService;

    fn ctx_with_state(entries: &[(&str, serde_json::Value)]) -> ReadonlyContext {
        let mut session = Session::new("app", "u", "s");
        session.state = State::from_iter(
            entries
                .iter()
                .map(|(k, v)| ((*k).to_string(), v.clone())),
        );
        ReadonlyContext::new(Arc::new(InvocationContext {
            app_name: "app".into(),
            user_id: "u".into(),
            invocation_id: "inv".into(),
            session: Arc::new(Mutex::new(session)),
            session_service: Arc::new(InMemorySessionService::new()),
            artifact_service: None,
            memory_service: None,
            credential_service: None,
            run_config: RunConfig::default(),
            origin: InvocationOrigin::Api,
            user_content: None,
            llm_call_count: Arc::new(Mutex::new(0)),
            cancellation: Default::default(),
            attributes: Arc::new(Mutex::new(HashMap::new())),
        }))
    }

    #[tokio::test]
    async fn replaces_state_keys() {
        let ctx = ctx_with_state(&[("city", json!("Paris")), ("n", json!(3))]);
        let s = inject_session_state("Weather in {city}, retries {n}.", &ctx)
            .await
            .unwrap();
        assert_eq!(s, "Weather in Paris, retries 3.");
    }

    #[tokio::test]
    async fn optional_missing_key_becomes_empty() {
        let ctx = ctx_with_state(&[]);
        let s = inject_session_state("Hello {name?}!", &ctx).await.unwrap();
        assert_eq!(s, "Hello !");
    }

    #[tokio::test]
    async fn required_missing_key_errors() {
        let ctx = ctx_with_state(&[]);
        let err = inject_session_state("Hello {name}!", &ctx).await.unwrap_err();
        assert!(err.to_string().contains("name"));
    }

    #[tokio::test]
    async fn invalid_names_left_untouched() {
        let ctx = ctx_with_state(&[]);
        let template = r#"JSON looks like {"a": 1} and {1,2} and { } stay."#;
        let s = inject_session_state(template, &ctx).await.unwrap();
        assert_eq!(s, template);
    }

    #[tokio::test]
    async fn prefixed_keys_resolve() {
        let ctx = ctx_with_state(&[("user:tier", json!("pro"))]);
        let s = inject_session_state("Tier: {user:tier}", &ctx).await.unwrap();
        assert_eq!(s, "Tier: pro");
    }

    #[tokio::test]
    async fn unterminated_brace_left_untouched() {
        let ctx = ctx_with_state(&[("x", json!("v"))]);
        let s = inject_session_state("open { brace and {x}", &ctx).await.unwrap();
        assert_eq!(s, "open { brace and v");
    }
}

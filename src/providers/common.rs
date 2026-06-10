//! Shared plumbing for provider clients: retry with exponential backoff,
//! jitter, and `Retry-After` support, plus schema conversion for
//! structured-output requests.

use std::future::Future;
use std::time::Duration;

use serde_json::{Value, json};
use tracing::warn;

use crate::core::RetryConfig;
use crate::error::{Error, ProviderError, Result};

/// Parse a `Retry-After` header value as a number of seconds. HTTP-date
/// values are ignored (providers send delta-seconds in practice).
fn retry_after(resp: &reqwest::Response) -> Option<Duration> {
    resp.headers()
        .get("retry-after")?
        .to_str()
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|s| *s >= 0.0)
        .map(Duration::from_secs_f64)
}

/// Send a request via `op`, retrying transient failures per `cfg`.
///
/// Retries when the transport fails before a response arrives (connect /
/// timeout) or the status is one of [`RetryConfig::is_retryable_status`]
/// (408, 409, 429, 5xx). The first non-retryable outcome — success or not —
/// is returned to the caller, which keeps provider-specific error mapping
/// where it lives today.
pub(crate) async fn send_with_retry<F, Fut>(
    cfg: &RetryConfig,
    mut op: F,
) -> Result<reqwest::Response>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = std::result::Result<reqwest::Response, reqwest::Error>>,
{
    let mut attempt: u32 = 0;
    loop {
        let delay = match op().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                if !RetryConfig::is_retryable_status(status) || attempt >= cfg.max_retries {
                    return Ok(resp);
                }
                let d = cfg.delay(attempt, retry_after(&resp));
                warn!(
                    status,
                    attempt,
                    delay_ms = d.as_millis() as u64,
                    "retrying provider request"
                );
                d
            }
            Err(e) => {
                if !(e.is_connect() || e.is_timeout()) || attempt >= cfg.max_retries {
                    return Err(Error::Provider(ProviderError::Transport(e.to_string())));
                }
                let d = cfg.delay(attempt, None);
                warn!(
                    error = %e,
                    attempt,
                    delay_ms = d.as_millis() as u64,
                    "retrying provider request"
                );
                d
            }
        };
        tokio::time::sleep(delay).await;
        attempt += 1;
    }
}

/// Convert an adk [`Schema`](crate::genai_types::Schema) (OpenAPI-3 flavour:
/// SCREAMING_SNAKE types, `nullable`) into the JSON Schema dialect that
/// OpenAI and Anthropic structured-output endpoints accept:
///
/// - types are lowercased;
/// - `nullable: true` becomes a `["type", "null"]` union;
/// - every object gets `additionalProperties: false` (both providers require
///   it);
/// - advisory / unsupported keywords (`format`, `pattern`, `minimum`,
///   `maximum`, `minLength`, `maxLength`, `minItems`, `maxItems`, `example`,
///   `default`, `title`) are stripped — both providers reject most of them;
/// - with `strict_all_required` (OpenAI strict mode), every object property
///   is added to `required`, and properties that were previously optional
///   become nullable so the original semantics survive.
pub(crate) fn to_json_schema(
    schema: &crate::genai_types::Schema,
    strict_all_required: bool,
) -> Value {
    let v = serde_json::to_value(schema).unwrap_or_else(|_| json!({"type": "object"}));
    normalize_json_schema(v, strict_all_required)
}

fn add_null_to_type(obj: &mut serde_json::Map<String, Value>) {
    match obj.get_mut("type") {
        Some(Value::String(t)) => {
            let t = t.clone();
            obj.insert("type".into(), json!([t, "null"]));
        }
        Some(Value::Array(arr)) => {
            if !arr.iter().any(|v| v == "null") {
                arr.push(json!("null"));
            }
        }
        // No plain `type` (anyOf / enum-only nodes): leave as-is.
        _ => {}
    }
}

fn normalize_json_schema(mut v: Value, strict: bool) -> Value {
    let Some(obj) = v.as_object_mut() else {
        return v;
    };
    // Lowercase the type ("OBJECT" → "object").
    if let Some(Value::String(t)) = obj.get_mut("type") {
        *t = t.to_lowercase();
    }
    for k in [
        "format",
        "pattern",
        "minimum",
        "maximum",
        "minLength",
        "maxLength",
        "minItems",
        "maxItems",
        "example",
        "default",
        "title",
    ] {
        obj.remove(k);
    }
    let nullable = matches!(obj.remove("nullable"), Some(Value::Bool(true)));
    if nullable {
        add_null_to_type(obj);
    }
    if let Some(items) = obj.remove("items") {
        obj.insert("items".into(), normalize_json_schema(items, strict));
    }
    if let Some(Value::Array(any_of)) = obj.remove("anyOf") {
        obj.insert(
            "anyOf".into(),
            Value::Array(
                any_of
                    .into_iter()
                    .map(|s| normalize_json_schema(s, strict))
                    .collect(),
            ),
        );
    }

    let is_object =
        obj.get("type").map(|t| t == "object").unwrap_or(false) || obj.contains_key("properties");
    if let Some(Value::Object(props)) = obj.remove("properties") {
        let originally_required: Vec<String> = obj
            .get("required")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let mut new_props = serde_json::Map::new();
        let mut all_keys = Vec::new();
        for (name, prop) in props {
            let mut p = normalize_json_schema(prop, strict);
            if strict && !originally_required.contains(&name) {
                // Strict mode lists every property in `required`; preserve
                // optionality by making the previously-optional ones
                // nullable.
                if let Some(po) = p.as_object_mut() {
                    add_null_to_type(po);
                }
            }
            all_keys.push(name.clone());
            new_props.insert(name, p);
        }
        obj.insert("properties".into(), Value::Object(new_props));
        if strict {
            obj.insert(
                "required".into(),
                Value::Array(all_keys.into_iter().map(Value::String).collect()),
            );
        }
    }
    if is_object {
        obj.insert("additionalProperties".into(), Value::Bool(false));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genai_types::Schema;

    fn sample() -> Schema {
        Schema::object()
            .property("name", Schema::string().with_description("Full name"))
            .property("age", Schema::integer())
            .require("name")
    }

    #[test]
    fn lenient_mode_keeps_original_required() {
        let v = to_json_schema(&sample(), false);
        assert_eq!(v["type"], "object");
        assert_eq!(v["additionalProperties"], false);
        assert_eq!(v["required"], serde_json::json!(["name"]));
        assert_eq!(v["properties"]["name"]["type"], "string");
        assert_eq!(v["properties"]["name"]["description"], "Full name");
    }

    #[test]
    fn strict_mode_requires_all_and_nullifies_optionals() {
        let v = to_json_schema(&sample(), true);
        let required = v["required"].as_array().unwrap();
        assert_eq!(required.len(), 2);
        // `name` was required: type stays plain.
        assert_eq!(v["properties"]["name"]["type"], "string");
        // `age` was optional: becomes nullable.
        assert_eq!(
            v["properties"]["age"]["type"],
            serde_json::json!(["integer", "null"])
        );
    }

    #[test]
    fn strips_unsupported_keywords_recursively() {
        let mut inner = Schema::string();
        inner.pattern = Some("^a".into());
        inner.format = Some("email".into());
        let schema = Schema::object().property("x", inner);
        let v = to_json_schema(&schema, false);
        assert!(v["properties"]["x"].get("pattern").is_none());
        assert!(v["properties"]["x"].get("format").is_none());
    }
}

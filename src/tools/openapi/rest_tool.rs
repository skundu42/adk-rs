//! [`RestApiTool`] — runtime form of one OpenAPI operation. Builds and
//! dispatches an HTTP request, injecting the resolved credential from
//! [`ToolContext::auth_credential`].

use async_trait::async_trait;
use indexmap::IndexMap;
use serde_json::Value;

use crate::auth::config::AuthConfig;
use crate::auth::credential::AuthCredential;
use crate::auth::scheme::{ApiKeyLocation, AuthScheme};
use crate::core::{DynTool, ToolContext};
use crate::error::{Error, Result};
use crate::genai_types::FunctionDeclaration;

use super::operation::{ParamLocation, ParsedOperation};

/// One OpenAPI operation rendered as a callable tool.
pub struct RestApiTool {
    op: ParsedOperation,
    auth_config: Option<AuthConfig>,
    http: reqwest::Client,
}

impl std::fmt::Debug for RestApiTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestApiTool")
            .field("name", &self.op.name)
            .field("method", &self.op.method.as_str())
            .field("path", &self.op.path)
            .finish_non_exhaustive()
    }
}

impl RestApiTool {
    /// Wrap a parsed operation in a tool. `auth_config` is what the runner
    /// will resolve before dispatching; pass `None` for unauthenticated
    /// operations.
    #[must_use]
    pub fn new(op: ParsedOperation, auth_config: Option<AuthConfig>) -> Self {
        Self {
            op,
            auth_config,
            http: reqwest::Client::new(),
        }
    }

    fn parsed(&self) -> &ParsedOperation {
        &self.op
    }
}

#[async_trait]
impl DynTool for RestApiTool {
    fn name(&self) -> &str {
        &self.op.name
    }
    fn description(&self) -> &str {
        &self.op.description
    }
    fn auth_config(&self) -> Option<&AuthConfig> {
        self.auth_config.as_ref()
    }
    fn declaration(&self) -> Option<FunctionDeclaration> {
        Some(
            FunctionDeclaration::new(&self.op.name, &self.op.description)
                .with_parameters(self.op.build_args_schema()),
        )
    }
    async fn run(&self, args: Value, ctx: &mut ToolContext) -> Result<Value> {
        let args = args.as_object().cloned().unwrap_or_default();
        for p in self.parsed().parameters.iter().filter(|p| p.required) {
            if args.get(&p.py_name).is_none_or(Value::is_null) {
                return Err(Error::invalid_input(format!(
                    "missing required parameter `{}`",
                    p.py_name
                )));
            }
        }

        let mut url = format!("{}{}", self.op.base_url.trim_end_matches('/'), self.op.path);

        // Path substitution.
        for p in self
            .parsed()
            .parameters
            .iter()
            .filter(|p| p.location == ParamLocation::Path)
        {
            let v = args.get(&p.py_name).cloned().unwrap_or(Value::Null);
            url = url.replace(
                &format!("{{{}}}", p.name),
                &percent_encode_path_segment(&value_to_path_str(&v)),
            );
        }

        // Query string.
        let mut query: IndexMap<String, String> = IndexMap::new();
        for p in self
            .parsed()
            .parameters
            .iter()
            .filter(|p| p.location == ParamLocation::Query)
        {
            if let Some(v) = args.get(&p.py_name) {
                if !v.is_null() {
                    query.insert(p.name.clone(), value_to_query_str(v));
                }
            }
        }

        // Headers.
        let mut headers = reqwest::header::HeaderMap::new();
        for p in self
            .parsed()
            .parameters
            .iter()
            .filter(|p| p.location == ParamLocation::Header)
        {
            let Some(v) = args.get(&p.py_name) else {
                continue;
            };
            if v.is_null() {
                continue;
            }
            let val = v
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| value_to_query_str(v));
            let name = reqwest::header::HeaderName::try_from(p.name.as_str())
                .map_err(|e| Error::other(format!("invalid header name {:?}: {e}", p.name)))?;
            let hv = reqwest::header::HeaderValue::from_str(&val)
                .map_err(|e| Error::other(format!("invalid header value for {:?}: {e}", p.name)))?;
            headers.insert(name, hv);
        }
        for p in self
            .parsed()
            .parameters
            .iter()
            .filter(|p| p.location == ParamLocation::Cookie)
        {
            if let Some(v) = args.get(&p.py_name) {
                if !v.is_null() {
                    append_cookie(&mut headers, &p.name, &value_to_query_str(v))?;
                }
            }
        }

        // Body.
        let body_value = self
            .parsed()
            .parameters
            .iter()
            .any(|p| p.location == ParamLocation::Body)
            .then(|| args.get("body"))
            .flatten()
            .cloned();

        // Auth injection from ctx.auth_credential.
        if let (Some(cred), Some(cfg)) = (ctx.auth_credential.clone(), &self.auth_config) {
            inject_credential(&cred, &cfg.auth_scheme, &mut headers, &mut query)?;
        }

        // Build the request.
        let method = reqwest::Method::from_bytes(self.op.method.as_str().as_bytes())
            .map_err(|e| Error::other(format!("invalid HTTP method: {e}")))?;
        let mut req = self.http.request(method, &url).headers(headers);
        if !query.is_empty() {
            let q: Vec<(String, String)> = query.into_iter().collect();
            req = req.query(&q);
        }
        if let Some(body) = body_value {
            req = req.json(&body);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| Error::other(format!("HTTP send: {e}")))?;
        let status = resp.status().as_u16();
        let body_text = resp.text().await.unwrap_or_default();
        let body_json: Value = serde_json::from_str(&body_text).unwrap_or(Value::String(body_text));
        Ok(serde_json::json!({
            "status": status,
            "body": body_json,
        }))
    }
}

fn value_to_path_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
        _ => v.to_string(),
    }
}

fn value_to_query_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
        _ => v.to_string(),
    }
}

fn percent_encode_path_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            use std::fmt::Write as _;
            let _ = write!(&mut out, "%{b:02X}");
        }
    }
    out
}

/// Validate that a cookie name / value conforms to RFC 6265 §4.1.1. Rejects
/// control chars and the cookie separators so callers can't inject extra
/// cookies via embedded `;` / CRLF.
fn validate_cookie_octets(label: &str, s: &str) -> Result<()> {
    for b in s.as_bytes() {
        match *b {
            // CTLs (incl. \r, \n, \0) and DEL.
            0..=0x1f | 0x7f => {
                return Err(Error::other(format!(
                    "invalid byte 0x{b:02x} in cookie {label}"
                )));
            }
            // RFC 6265 separators that would terminate / split the cookie pair.
            b';' | b',' | b'"' | b'\\' => {
                return Err(Error::other(format!(
                    "forbidden character {:?} in cookie {label}",
                    char::from(*b)
                )));
            }
            _ => {}
        }
    }
    Ok(())
}

fn append_cookie(headers: &mut reqwest::header::HeaderMap, name: &str, value: &str) -> Result<()> {
    validate_cookie_octets("name", name)?;
    validate_cookie_octets("value", value)?;
    let cookie = match headers.get(reqwest::header::COOKIE) {
        Some(existing) => match existing.to_str() {
            Ok(s) if !s.is_empty() => format!("{s}; {name}={value}"),
            _ => format!("{name}={value}"),
        },
        None => format!("{name}={value}"),
    };
    let hv = reqwest::header::HeaderValue::from_str(&cookie)
        .map_err(|e| Error::other(format!("invalid cookie header: {e}")))?;
    headers.insert(reqwest::header::COOKIE, hv);
    Ok(())
}

fn insert_header(headers: &mut reqwest::header::HeaderMap, name: &str, value: &str) -> Result<()> {
    let hn = reqwest::header::HeaderName::try_from(name)
        .map_err(|e| Error::other(format!("invalid header name {name:?}: {e}")))?;
    let hv = reqwest::header::HeaderValue::from_str(value)
        .map_err(|e| Error::other(format!("invalid header value for {name:?}: {e}")))?;
    headers.insert(hn, hv);
    Ok(())
}

fn inject_credential(
    cred: &AuthCredential,
    scheme: &AuthScheme,
    headers: &mut reqwest::header::HeaderMap,
    query: &mut IndexMap<String, String>,
) -> Result<()> {
    match scheme {
        AuthScheme::ApiKey { location, name, .. } => {
            let Some(k) = cred.api_key.as_deref() else {
                return Ok(());
            };
            match location {
                ApiKeyLocation::Header => insert_header(headers, name, k)?,
                ApiKeyLocation::Query => {
                    query.insert(name.clone(), k.to_string());
                }
                ApiKeyLocation::Cookie => append_cookie(headers, name, k)?,
            }
        }
        AuthScheme::Http { scheme: s, .. } => {
            let Some(http) = cred.http.as_ref() else {
                return Ok(());
            };
            if s.eq_ignore_ascii_case("bearer") {
                if let Some(tok) = http.token.as_deref() {
                    insert_header(headers, "authorization", &format!("Bearer {tok}"))?;
                }
            } else if s.eq_ignore_ascii_case("basic") {
                if let (Some(u), Some(p)) = (http.username.as_deref(), http.password.as_deref()) {
                    use base64::Engine;
                    let encoded =
                        base64::engine::general_purpose::STANDARD.encode(format!("{u}:{p}"));
                    insert_header(headers, "authorization", &format!("Basic {encoded}"))?;
                }
            }
        }
        AuthScheme::OAuth2 { .. } | AuthScheme::OpenIdConnect { .. } => {
            if let Some(token) = cred.oauth2.as_ref().and_then(|o| o.access_token.as_deref()) {
                insert_header(headers, "authorization", &format!("Bearer {token}"))?;
            }
        }
        AuthScheme::Custom { .. } => {} // up to the user
    }
    Ok(())
}

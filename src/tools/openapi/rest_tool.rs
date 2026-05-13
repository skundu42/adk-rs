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
        let mut url = format!("{}{}", self.op.base_url.trim_end_matches('/'), self.op.path);

        // Path substitution.
        for p in self
            .parsed()
            .parameters
            .iter()
            .filter(|p| p.location == ParamLocation::Path)
        {
            let v = args.get(&p.py_name).cloned().unwrap_or(Value::Null);
            url = url.replace(&format!("{{{}}}", p.name), &value_to_path_str(&v));
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
            if let Some(v) = args.get(&p.py_name) {
                if let (Ok(name), Some(val)) = (
                    reqwest::header::HeaderName::try_from(p.name.as_str()),
                    v.as_str(),
                ) {
                    if let Ok(hv) = reqwest::header::HeaderValue::from_str(val) {
                        headers.insert(name, hv);
                    }
                }
            }
        }

        // Body.
        let body_value = args
            .iter()
            .find(|(_, _)| {
                self.parsed()
                    .parameters
                    .iter()
                    .any(|p| p.location == ParamLocation::Body)
            })
            .and_then(|_| args.get("body"))
            .cloned();

        // Auth injection from ctx.auth_credential.
        if let (Some(cred), Some(cfg)) = (ctx.auth_credential.clone(), &self.auth_config) {
            inject_credential(&cred, &cfg.auth_scheme, &mut headers, &mut query);
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

fn inject_credential(
    cred: &AuthCredential,
    scheme: &AuthScheme,
    headers: &mut reqwest::header::HeaderMap,
    query: &mut IndexMap<String, String>,
) {
    match scheme {
        AuthScheme::ApiKey { location, name, .. } => {
            let Some(k) = cred.api_key.as_deref() else {
                return;
            };
            match location {
                ApiKeyLocation::Header => {
                    if let (Ok(hn), Ok(hv)) = (
                        reqwest::header::HeaderName::try_from(name.as_str()),
                        reqwest::header::HeaderValue::from_str(k),
                    ) {
                        headers.insert(hn, hv);
                    }
                }
                ApiKeyLocation::Query => {
                    query.insert(name.clone(), k.to_string());
                }
                ApiKeyLocation::Cookie => {
                    if let Ok(hv) = reqwest::header::HeaderValue::from_str(&format!("{name}={k}")) {
                        headers.insert(reqwest::header::COOKIE, hv);
                    }
                }
            }
        }
        AuthScheme::Http { scheme: s, .. } => {
            let http = match cred.http.as_ref() {
                Some(h) => h,
                None => return,
            };
            if s.eq_ignore_ascii_case("bearer") {
                if let Some(tok) = http.token.as_deref() {
                    if let Ok(hv) = reqwest::header::HeaderValue::from_str(&format!("Bearer {tok}"))
                    {
                        headers.insert(reqwest::header::AUTHORIZATION, hv);
                    }
                }
            } else if s.eq_ignore_ascii_case("basic") {
                if let (Some(u), Some(p)) = (http.username.as_deref(), http.password.as_deref()) {
                    use base64::Engine;
                    let encoded =
                        base64::engine::general_purpose::STANDARD.encode(format!("{u}:{p}"));
                    if let Ok(hv) =
                        reqwest::header::HeaderValue::from_str(&format!("Basic {encoded}"))
                    {
                        headers.insert(reqwest::header::AUTHORIZATION, hv);
                    }
                }
            }
        }
        AuthScheme::OAuth2 { .. } | AuthScheme::OpenIdConnect { .. } => {
            if let Some(token) = cred.oauth2.as_ref().and_then(|o| o.access_token.as_deref()) {
                if let Ok(hv) = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}")) {
                    headers.insert(reqwest::header::AUTHORIZATION, hv);
                }
            }
        }
        AuthScheme::Custom { .. } => {} // up to the user
    }
}

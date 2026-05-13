//! [`OpenAPIToolset`] facade — loads an OpenAPI 3.x spec, parses it, binds
//! credentials, and emits one [`RestApiTool`] per operation.

use std::path::Path;
use std::sync::Arc;

use indexmap::IndexMap;

use crate::auth::config::AuthConfig;
use crate::auth::credential::AuthCredential;
use crate::auth::scheme::AuthScheme;
use crate::core::DynTool;
use crate::error::{Error, Result};

use super::operation::ParsedOperation;
use super::rest_tool::RestApiTool;
use super::spec::{SpecParse, parse_spec};

/// Loads an OpenAPI 3.x spec and generates [`RestApiTool`] instances per
/// operation.
pub struct OpenAPIToolset {
    operations: Vec<ParsedOperation>,
    security_schemes: IndexMap<String, AuthScheme>,
    bound_credentials: IndexMap<String, AuthCredential>,
}

impl std::fmt::Debug for OpenAPIToolset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAPIToolset")
            .field("operation_count", &self.operations.len())
            .field(
                "security_schemes",
                &self.security_schemes.keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl OpenAPIToolset {
    /// Parse a YAML or JSON spec string.
    pub fn from_yaml(spec: &str) -> Result<Self> {
        let SpecParse {
            operations,
            security_schemes,
            base_url: _,
        } = parse_spec(spec)?;
        Ok(Self {
            operations,
            security_schemes,
            bound_credentials: IndexMap::new(),
        })
    }

    /// Parse a JSON spec string.
    pub fn from_json(spec: &str) -> Result<Self> {
        Self::from_yaml(spec)
    }

    /// Read a spec from disk and parse it (YAML or JSON).
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let bytes =
            std::fs::read(path).map_err(|e| Error::other(format!("read OpenAPI spec: {e}")))?;
        let s = String::from_utf8(bytes)
            .map_err(|e| Error::other(format!("OpenAPI spec is not UTF-8: {e}")))?;
        Self::from_yaml(&s)
    }

    /// Bind a credential to one of the spec's security schemes by name.
    /// Subsequent [`Self::into_tools`] calls attach this credential to every
    /// operation that accepts the scheme.
    #[must_use]
    pub fn with_credential(mut self, scheme_name: impl Into<String>, cred: AuthCredential) -> Self {
        self.bound_credentials.insert(scheme_name.into(), cred);
        self
    }

    /// Override the base URL (e.g. point at a local mock for tests).
    #[must_use]
    pub fn with_base_url(mut self, base: impl Into<String>) -> Self {
        let base = base.into();
        for op in &mut self.operations {
            op.base_url = base.clone();
        }
        self
    }

    /// Borrowed view of every operation's name (handy for filters).
    #[must_use]
    pub fn operation_names(&self) -> Vec<&str> {
        self.operations.iter().map(|o| o.name.as_str()).collect()
    }

    /// Generate one [`RestApiTool`] per operation, filtered through
    /// `predicate`. Returns concrete `Arc<dyn DynTool>`s ready to attach to a
    /// `LlmAgent::builder().tool(...)`.
    pub fn build_tools<F>(self, predicate: F) -> Vec<Arc<dyn DynTool>>
    where
        F: Fn(&ParsedOperation) -> bool,
    {
        let mut out: Vec<Arc<dyn DynTool>> = Vec::new();
        for op in self.operations {
            if !predicate(&op) {
                continue;
            }
            let cfg = build_auth_config(&op, &self.security_schemes, &self.bound_credentials);
            out.push(Arc::new(RestApiTool::new(op, cfg)));
        }
        out
    }

    /// Generate all tools (`build_tools(|_| true)`).
    #[must_use]
    pub fn into_tools(self) -> Vec<Arc<dyn DynTool>> {
        self.build_tools(|_| true)
    }
}

fn build_auth_config(
    op: &ParsedOperation,
    schemes: &IndexMap<String, AuthScheme>,
    creds: &IndexMap<String, AuthCredential>,
) -> Option<AuthConfig> {
    // Pick the first security scheme that we both recognise and have a credential for.
    for name in &op.security_schemes {
        let scheme = schemes.get(name).cloned();
        let cred = creds.get(name).cloned();
        if let (Some(scheme), Some(cred)) = (scheme, cred) {
            return Some(AuthConfig::new(scheme).with_raw(cred));
        }
    }
    // If we know the scheme but have no credential, still surface the config —
    // the runner will return `Misconfigured` so the caller can fix it.
    op.security_schemes
        .first()
        .and_then(|n| schemes.get(n).cloned())
        .map(AuthConfig::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TINY_SPEC: &str = r#"
openapi: 3.0.0
info:
  title: Tiny
  version: 1.0.0
servers:
  - url: https://api.example.com
paths:
  /ping:
    get:
      operationId: ping
      responses:
        '200':
          description: ok
  /protected:
    get:
      operationId: protected
      security:
        - bearerAuth: []
      responses:
        '200':
          description: ok
components:
  securitySchemes:
    bearerAuth:
      type: http
      scheme: bearer
"#;

    #[test]
    fn parses_and_generates_tools() {
        let ts = OpenAPIToolset::from_yaml(TINY_SPEC).unwrap();
        assert_eq!(ts.operation_names(), vec!["ping", "protected"]);
        let tools = ts.into_tools();
        assert_eq!(tools.len(), 2);
        let ping = tools
            .iter()
            .find(|t| t.name() == "ping")
            .expect("ping tool");
        assert!(ping.auth_config().is_none());
    }

    #[test]
    fn binds_credential_to_matching_scheme() {
        let ts = OpenAPIToolset::from_yaml(TINY_SPEC)
            .unwrap()
            .with_credential("bearerAuth", AuthCredential::bearer("my-token"));
        let tools = ts.into_tools();
        let prot = tools
            .iter()
            .find(|t| t.name() == "protected")
            .expect("protected tool");
        let cfg = prot.auth_config().unwrap();
        let raw = cfg.raw_auth_credential.as_ref().unwrap();
        assert_eq!(
            raw.http.as_ref().unwrap().token.as_deref(),
            Some("my-token")
        );
    }
}

//! Per-operation metadata derived from one OpenAPI 3.x `(path, method)`
//! entry. The fields here capture everything `RestApiTool` needs to build an
//! HTTP request without consulting the raw spec at runtime.

use indexmap::IndexMap;

use crate::genai_types::Schema;

/// HTTP method, normalised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    /// HTTP GET.
    Get,
    /// HTTP POST.
    Post,
    /// HTTP PUT.
    Put,
    /// HTTP PATCH.
    Patch,
    /// HTTP DELETE.
    Delete,
    /// HTTP HEAD.
    Head,
    /// HTTP OPTIONS.
    Options,
    /// HTTP TRACE.
    Trace,
}

impl HttpMethod {
    /// As an upper-case `&'static str` (e.g. `"GET"`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
            Self::Trace => "TRACE",
        }
    }
}

/// Where a parameter lives in the HTTP request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParamLocation {
    /// `{name}` in the URL path.
    Path,
    /// `?name=value` query string.
    Query,
    /// HTTP header.
    Header,
    /// HTTP cookie.
    Cookie,
    /// Request body (raw JSON).
    Body,
}

impl HttpMethod {
    /// As a lowercase method name (`"get"`, `"post"`, ...).
    #[must_use]
    pub fn as_lower(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Post => "post",
            Self::Put => "put",
            Self::Patch => "patch",
            Self::Delete => "delete",
            Self::Head => "head",
            Self::Options => "options",
            Self::Trace => "trace",
        }
    }
}

/// One parameter on an operation (path/query/header/cookie/body).
#[derive(Debug, Clone)]
pub struct ApiParameter {
    /// Original spec name.
    pub name: String,
    /// snake_case version used in the generated JSON-Schema and tool args.
    pub py_name: String,
    /// Where to inject this parameter.
    pub location: ParamLocation,
    /// JSON-Schema for the value.
    pub schema: Schema,
    /// Whether the spec marks this required.
    pub required: bool,
    /// Optional description.
    pub description: Option<String>,
}

/// One operation extracted from the spec.
#[derive(Debug, Clone)]
pub struct ParsedOperation {
    /// Tool name (snake_case `operationId`, or method+path if missing).
    pub name: String,
    /// Tool description (`operation.summary` or `operation.description`).
    pub description: String,
    /// Server base URL (concatenated with `path`).
    pub base_url: String,
    /// Path template, e.g. `/pets/{petId}`.
    pub path: String,
    /// HTTP method.
    pub method: HttpMethod,
    /// Ordered parameters.
    pub parameters: Vec<ApiParameter>,
    /// Names of `securitySchemes` keys this operation accepts.
    pub security_schemes: Vec<String>,
}

impl ParsedOperation {
    /// Build a single JSON-Schema object covering all parameters.
    #[must_use]
    pub fn build_args_schema(&self) -> Schema {
        let mut props: IndexMap<String, Schema> = IndexMap::new();
        let mut required: Vec<String> = Vec::new();
        for p in &self.parameters {
            let mut s = p.schema.clone();
            if let Some(desc) = &p.description {
                s = s.with_description(desc);
            }
            props.insert(p.py_name.clone(), s);
            if p.required {
                required.push(p.py_name.clone());
            }
        }
        let mut schema = Schema::object();
        for (k, v) in props {
            schema = schema.property(k, v);
        }
        for r in required {
            schema = schema.require(r);
        }
        schema
    }
}

/// Convert an identifier to snake_case (very simple — strips non-alpha chars,
/// lowercases, inserts `_` at camelCase boundaries).
#[must_use]
pub fn to_snake_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_lower = false;
    for c in s.chars() {
        if c.is_alphanumeric() || c == '_' {
            if c.is_ascii_uppercase() {
                if prev_lower {
                    out.push('_');
                }
                out.push(c.to_ascii_lowercase());
                prev_lower = false;
            } else {
                out.push(c);
                prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
            }
        } else {
            out.push('_');
            prev_lower = false;
        }
    }
    // collapse runs of `_`.
    let mut cleaned = String::with_capacity(out.len());
    let mut prev_us = false;
    for c in out.chars() {
        if c == '_' {
            if !prev_us {
                cleaned.push(c);
            }
            prev_us = true;
        } else {
            cleaned.push(c);
            prev_us = false;
        }
    }
    cleaned.trim_matches('_').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_case_smoke() {
        assert_eq!(to_snake_case("getPetById"), "get_pet_by_id");
        assert_eq!(to_snake_case("List-Pets"), "list_pets");
        // The simple casing-aware converter collapses contiguous uppercase
        // runs ("HTTPError" → "httperror"); good enough for v0.2 and matches
        // the typical OpenAPI `operationId` style.
        assert_eq!(to_snake_case("HTTPError"), "httperror");
    }
}

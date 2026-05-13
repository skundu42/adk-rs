//! OpenAPI 3.x spec loader / operation extractor.
//!
//! Wraps `openapiv3` to convert a serialised spec into a list of
//! [`ParsedOperation`] entries, while resolving `$ref`s for parameter
//! schemas and recording each operation's accepted security schemes.

use indexmap::IndexMap;
use openapiv3::{
    OpenAPI, Operation, Parameter, ParameterData, ParameterSchemaOrContent, PathItem, ReferenceOr,
    RequestBody, SchemaKind,
};

use crate::auth::scheme::{ApiKeyLocation, AuthScheme, OAuthFlow, OAuthFlows};
use crate::error::{Error, Result};
use crate::genai_types::Schema;

use super::operation::{ApiParameter, HttpMethod, ParamLocation, ParsedOperation, to_snake_case};

/// Parsed spec ready for tool generation.
pub struct SpecParse {
    /// Operations extracted from the spec.
    pub operations: Vec<ParsedOperation>,
    /// `name → AuthScheme` from `components.securitySchemes`.
    pub security_schemes: IndexMap<String, AuthScheme>,
    /// Server base URL (the first `servers[].url`, or empty).
    pub base_url: String,
}

/// Parse a YAML or JSON spec.
pub fn parse_spec(spec: &str) -> Result<SpecParse> {
    let api: OpenAPI = serde_yaml::from_str(spec)
        .map_err(|e| Error::config(format!("invalid OpenAPI spec: {e}")))?;
    parse(api)
}

/// Parse an `OpenAPI` value directly.
pub fn parse(api: OpenAPI) -> Result<SpecParse> {
    let base_url = api
        .servers
        .first()
        .map(|s| s.url.clone())
        .unwrap_or_default();

    let security_schemes = collect_security_schemes(&api);

    let mut operations = Vec::new();
    for (path, item_ref) in &api.paths.paths {
        let item = match item_ref {
            ReferenceOr::Item(i) => i.clone(),
            ReferenceOr::Reference { .. } => continue, // top-level path refs are rare; skip
        };
        let path_params = item.parameters.clone();
        for (method, op) in iter_operations(&item) {
            let parsed = build_operation(&api, &base_url, path, method, op, &path_params)?;
            operations.push(parsed);
        }
    }
    Ok(SpecParse {
        operations,
        security_schemes,
        base_url,
    })
}

fn iter_operations(item: &PathItem) -> Vec<(HttpMethod, &Operation)> {
    let mut v: Vec<(HttpMethod, &Operation)> = Vec::new();
    if let Some(op) = &item.get {
        v.push((HttpMethod::Get, op));
    }
    if let Some(op) = &item.post {
        v.push((HttpMethod::Post, op));
    }
    if let Some(op) = &item.put {
        v.push((HttpMethod::Put, op));
    }
    if let Some(op) = &item.patch {
        v.push((HttpMethod::Patch, op));
    }
    if let Some(op) = &item.delete {
        v.push((HttpMethod::Delete, op));
    }
    if let Some(op) = &item.head {
        v.push((HttpMethod::Head, op));
    }
    if let Some(op) = &item.options {
        v.push((HttpMethod::Options, op));
    }
    if let Some(op) = &item.trace {
        v.push((HttpMethod::Trace, op));
    }
    v
}

fn build_operation(
    api: &OpenAPI,
    base_url: &str,
    path: &str,
    method: HttpMethod,
    op: &Operation,
    path_params: &[ReferenceOr<Parameter>],
) -> Result<ParsedOperation> {
    let name = op
        .operation_id
        .clone()
        .map(|id| to_snake_case(&id))
        .unwrap_or_else(|| to_snake_case(&format!("{}_{}", method.as_str().to_lowercase(), path)));
    let description = op
        .summary
        .clone()
        .or_else(|| op.description.clone())
        .unwrap_or_default();

    let mut parameters: Vec<ApiParameter> = Vec::new();
    // path-level parameters first, then operation-level (op overrides on name+in collision).
    let combined: Vec<&ReferenceOr<Parameter>> =
        path_params.iter().chain(op.parameters.iter()).collect();
    for p in combined {
        if let Some(ap) = parameter_to_api_param(api, p)? {
            parameters.push(ap);
        }
    }
    if let Some(body) = op.request_body.as_ref() {
        parameters.extend(request_body_to_params(api, body)?);
    }
    // Operation-level params override path-level on (name, location) collision.
    parameters = dedupe_keep_last(parameters);

    let security_schemes = op
        .security
        .as_ref()
        .or(api.security.as_ref())
        .and_then(|s| s.first())
        .map(|req| req.keys().cloned().collect())
        .unwrap_or_default();

    Ok(ParsedOperation {
        name,
        description,
        base_url: base_url.to_string(),
        path: path.to_string(),
        method,
        parameters,
        security_schemes,
    })
}

fn dedupe_keep_last(params: Vec<ApiParameter>) -> Vec<ApiParameter> {
    let mut seen: IndexMap<(String, ParamLocation), ApiParameter> = IndexMap::new();
    for p in params {
        seen.insert((p.name.clone(), p.location), p);
    }
    seen.into_iter().map(|(_, v)| v).collect()
}

fn parameter_to_api_param(
    api: &OpenAPI,
    p: &ReferenceOr<Parameter>,
) -> Result<Option<ApiParameter>> {
    let ReferenceOr::Item(param) = p else {
        return Ok(None);
    };
    let (data, location) = match param {
        Parameter::Query { parameter_data, .. } => (parameter_data, ParamLocation::Query),
        Parameter::Header { parameter_data, .. } => (parameter_data, ParamLocation::Header),
        Parameter::Path { parameter_data, .. } => (parameter_data, ParamLocation::Path),
        Parameter::Cookie { parameter_data, .. } => (parameter_data, ParamLocation::Cookie),
    };
    Ok(Some(parameter_data_to_api_param(api, data, location)?))
}

fn parameter_data_to_api_param(
    api: &OpenAPI,
    data: &ParameterData,
    location: ParamLocation,
) -> Result<ApiParameter> {
    let schema = match &data.format {
        ParameterSchemaOrContent::Schema(s) => schema_or_ref_to_schema(api, s)?,
        ParameterSchemaOrContent::Content(_) => Schema::string(),
    };
    Ok(ApiParameter {
        name: data.name.clone(),
        py_name: to_snake_case(&data.name),
        location,
        schema,
        required: data.required,
        description: data.description.clone(),
    })
}

fn request_body_to_params(
    api: &OpenAPI,
    rb: &ReferenceOr<RequestBody>,
) -> Result<Vec<ApiParameter>> {
    let ReferenceOr::Item(body) = rb else {
        return Ok(vec![]);
    };
    // Prefer application/json; fall back to first content type.
    let media = body
        .content
        .get("application/json")
        .or_else(|| body.content.values().next());
    let Some(media) = media else {
        return Ok(vec![]);
    };
    let Some(schema) = media.schema.as_ref() else {
        return Ok(vec![]);
    };
    let schema = schema_or_ref_to_schema(api, schema)?;
    Ok(vec![ApiParameter {
        name: "body".into(),
        py_name: "body".into(),
        location: ParamLocation::Body,
        schema,
        required: body.required,
        description: body.description.clone(),
    }])
}

fn schema_or_ref_to_schema(api: &OpenAPI, s: &ReferenceOr<openapiv3::Schema>) -> Result<Schema> {
    match s {
        ReferenceOr::Item(s) => openapi_schema_to_adk_schema(api, s),
        ReferenceOr::Reference { reference } => {
            let Some(target) = resolve_schema_ref(api, reference) else {
                return Err(Error::config(format!(
                    "unsupported or unresolved OpenAPI schema ref `{reference}`"
                )));
            };
            schema_or_ref_to_schema(api, target)
        }
    }
}

fn resolve_schema_ref<'a>(
    api: &'a OpenAPI,
    reference: &str,
) -> Option<&'a ReferenceOr<openapiv3::Schema>> {
    let name = reference.strip_prefix("#/components/schemas/")?;
    api.components.as_ref()?.schemas.get(name)
}

fn openapi_schema_to_adk_schema(api: &OpenAPI, s: &openapiv3::Schema) -> Result<Schema> {
    use openapiv3::Type;
    let schema = match &s.schema_kind {
        SchemaKind::Type(t) => match t {
            Type::String(_) => Schema::string(),
            Type::Number(_) => Schema::number(),
            Type::Integer(_) => Schema::integer(),
            Type::Boolean(_) => Schema::boolean(),
            Type::Array(arr) => {
                let item = arr
                    .items
                    .as_ref()
                    .map(|i| schema_or_ref_to_schema(api, &i.clone().unbox()))
                    .transpose()?
                    .unwrap_or_else(Schema::string);
                Schema::array(item)
            }
            Type::Object(obj) => {
                let mut s = Schema::object();
                for (k, v) in &obj.properties {
                    s = s.property(k.clone(), schema_or_ref_to_schema(api, &v.clone().unbox())?);
                }
                for r in &obj.required {
                    s = s.require(r.clone());
                }
                s
            }
        },
        _ => Schema::string(),
    };
    Ok(schema)
}

fn collect_security_schemes(api: &OpenAPI) -> IndexMap<String, AuthScheme> {
    let mut out = IndexMap::new();
    let Some(components) = api.components.as_ref() else {
        return out;
    };
    for (name, ss) in &components.security_schemes {
        let ReferenceOr::Item(scheme) = ss else {
            continue;
        };
        if let Some(adk_scheme) = convert_security_scheme(scheme) {
            out.insert(name.clone(), adk_scheme);
        }
    }
    out
}

fn convert_security_scheme(s: &openapiv3::SecurityScheme) -> Option<AuthScheme> {
    use openapiv3::APIKeyLocation;
    use openapiv3::SecurityScheme as SS;
    Some(match s {
        SS::APIKey {
            location,
            name,
            description,
            ..
        } => AuthScheme::ApiKey {
            location: match location {
                APIKeyLocation::Query => ApiKeyLocation::Query,
                APIKeyLocation::Header => ApiKeyLocation::Header,
                APIKeyLocation::Cookie => ApiKeyLocation::Cookie,
            },
            name: name.clone(),
            description: description.clone(),
        },
        SS::HTTP {
            scheme,
            bearer_format,
            description,
            ..
        } => AuthScheme::Http {
            scheme: scheme.clone(),
            bearer_format: bearer_format.clone(),
            description: description.clone(),
        },
        SS::OAuth2 {
            flows, description, ..
        } => AuthScheme::OAuth2 {
            flows: convert_oauth_flows(flows),
            description: description.clone(),
        },
        SS::OpenIDConnect {
            open_id_connect_url,
            description,
            ..
        } => AuthScheme::OpenIdConnect {
            open_id_connect_url: open_id_connect_url.clone(),
            scopes: vec![],
            description: description.clone(),
        },
    })
}

fn convert_oauth_flows(f: &openapiv3::OAuth2Flows) -> OAuthFlows {
    OAuthFlows {
        authorization_code: f.authorization_code.as_ref().map(|ac| OAuthFlow {
            authorization_url: Some(ac.authorization_url.clone()),
            token_url: ac.token_url.clone(),
            refresh_url: ac.refresh_url.clone(),
            scopes: ac.scopes.clone(),
        }),
        client_credentials: f.client_credentials.as_ref().map(|cc| OAuthFlow {
            authorization_url: None,
            token_url: cc.token_url.clone(),
            refresh_url: cc.refresh_url.clone(),
            scopes: cc.scopes.clone(),
        }),
        implicit: f.implicit.as_ref().map(|im| OAuthFlow {
            authorization_url: Some(im.authorization_url.clone()),
            token_url: String::new(),
            refresh_url: im.refresh_url.clone(),
            scopes: im.scopes.clone(),
        }),
        password: f.password.as_ref().map(|pw| OAuthFlow {
            authorization_url: None,
            token_url: pw.token_url.clone(),
            refresh_url: pw.refresh_url.clone(),
            scopes: pw.scopes.clone(),
        }),
    }
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
  /pets/{id}:
    get:
      operationId: getPetById
      parameters:
        - name: id
          in: path
          required: true
          schema:
            type: integer
      responses:
        '200':
          description: ok
  /pets:
    post:
      operationId: createPet
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                name:
                  type: string
              required: [name]
      responses:
        '201':
          description: created
components:
  securitySchemes:
    bearerAuth:
      type: http
      scheme: bearer
"#;

    #[test]
    fn parses_paths_and_operations() {
        let parsed = parse_spec(TINY_SPEC).unwrap();
        assert_eq!(parsed.base_url, "https://api.example.com");
        let names: Vec<_> = parsed.operations.iter().map(|o| o.name.as_str()).collect();
        assert!(names.contains(&"get_pet_by_id"));
        assert!(names.contains(&"create_pet"));
        // schemes parsed
        assert!(parsed.security_schemes.contains_key("bearerAuth"));
    }

    #[test]
    fn path_param_is_required() {
        let parsed = parse_spec(TINY_SPEC).unwrap();
        let getop = parsed
            .operations
            .iter()
            .find(|o| o.name == "get_pet_by_id")
            .unwrap();
        let p = getop.parameters.iter().find(|p| p.name == "id").unwrap();
        assert_eq!(p.location, ParamLocation::Path);
        assert!(p.required);
    }

    #[test]
    fn request_body_becomes_body_param() {
        let parsed = parse_spec(TINY_SPEC).unwrap();
        let postop = parsed
            .operations
            .iter()
            .find(|o| o.name == "create_pet")
            .unwrap();
        let p = postop
            .parameters
            .iter()
            .find(|p| p.location == ParamLocation::Body)
            .unwrap();
        assert_eq!(p.py_name, "body");
        assert!(p.required);
    }
}

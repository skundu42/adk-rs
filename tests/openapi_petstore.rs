//! End-to-end: load a tiny Petstore-style spec, attach a bearer credential,
//! and exercise the generated tool against a `wiremock` backend.

#![cfg(all(feature = "openapi", feature = "auth"))]

use adk_rs::auth::credential::AuthCredential;
use adk_rs::core::ToolContext;
use adk_rs::genai_types::SchemaType;
use adk_rs::tools::openapi::OpenAPIToolset;
use serde_json::json;
use std::sync::Arc;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ctx_with_auth(cred: Option<AuthCredential>) -> ToolContext {
    let inv = Arc::new(adk_rs::core::testing::test_invocation_context());
    let mut ctx = ToolContext::new(inv);
    ctx.auth_credential = cred;
    ctx
}

const SPEC_TMPL: &str = r#"
openapi: 3.0.0
info:
  title: Petstore
  version: 1.0.0
servers:
  - url: __SERVER__
paths:
  /pets/{id}:
    get:
      operationId: getPetById
      security:
        - bearerAuth: []
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

#[tokio::test]
async fn get_path_param_with_bearer_auth() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/pets/42"))
        .and(header("authorization", "Bearer my-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 42, "name": "Boots"})))
        .expect(1)
        .mount(&server)
        .await;

    let spec = SPEC_TMPL.replace("__SERVER__", &server.uri());
    let tools = OpenAPIToolset::from_yaml(&spec)
        .unwrap()
        .with_credential("bearerAuth", AuthCredential::bearer("my-token"))
        .into_tools();
    let get = tools
        .into_iter()
        .find(|t| t.name() == "get_pet_by_id")
        .expect("get_pet_by_id tool");

    let mut ctx = ctx_with_auth(Some(AuthCredential::bearer("my-token")));
    let out = get.run(json!({"id": 42}), &mut ctx).await.unwrap();
    assert_eq!(out["status"], 200);
    assert_eq!(out["body"]["name"], "Boots");
}

#[tokio::test]
async fn post_body_round_trip() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/pets"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": 1, "name": "Fido"})))
        .expect(1)
        .mount(&server)
        .await;
    let spec = SPEC_TMPL.replace("__SERVER__", &server.uri());
    let tools = OpenAPIToolset::from_yaml(&spec).unwrap().into_tools();
    let create = tools
        .into_iter()
        .find(|t| t.name() == "create_pet")
        .expect("create_pet tool");
    let mut ctx = ctx_with_auth(None);
    let out = create
        .run(json!({"body": {"name": "Fido"}}), &mut ctx)
        .await
        .unwrap();
    assert_eq!(out["status"], 201);
    assert_eq!(out["body"]["name"], "Fido");
}

#[tokio::test]
async fn path_values_are_encoded_and_cookie_params_are_sent() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/pets/a%20b"))
        .and(header("cookie", "session=abc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .expect(1)
        .mount(&server)
        .await;

    let spec = format!(
        r#"
openapi: 3.0.0
info:
  title: Petstore
  version: 1.0.0
servers:
  - url: {}
paths:
  /pets/{{id}}:
    get:
      operationId: getPetCookie
      parameters:
        - name: id
          in: path
          required: true
          schema:
            type: string
        - name: session
          in: cookie
          required: true
          schema:
            type: string
      responses:
        '200':
          description: ok
"#,
        server.uri()
    );
    let tools = OpenAPIToolset::from_yaml(&spec).unwrap().into_tools();
    let tool = tools
        .into_iter()
        .find(|t| t.name() == "get_pet_cookie")
        .expect("get_pet_cookie tool");
    let mut ctx = ctx_with_auth(None);
    let out = tool
        .run(json!({"id": "a b", "session": "abc"}), &mut ctx)
        .await
        .unwrap();
    assert_eq!(out["status"], 200);
}

/// Bug fix v0.2.1 #1 (silent auth bypass): if the bearer token contains a
/// byte the HTTP layer rejects (e.g. CRLF), the request must FAIL — not
/// silently send unauthenticated.
#[tokio::test]
async fn injected_credential_with_crlf_rejects_request() {
    let server = MockServer::start().await;
    // No mounted Mock — the request should never reach the server.
    let spec = SPEC_TMPL.replace("__SERVER__", &server.uri());
    let tools = OpenAPIToolset::from_yaml(&spec)
        .unwrap()
        .with_credential(
            "bearerAuth",
            AuthCredential::bearer("token\r\nX-Injected: 1"),
        )
        .into_tools();
    let get = tools
        .into_iter()
        .find(|t| t.name() == "get_pet_by_id")
        .unwrap();
    let mut ctx = ctx_with_auth(Some(AuthCredential::bearer("token\r\nX-Injected: 1")));
    let err = get.run(json!({"id": 1}), &mut ctx).await.unwrap_err();
    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("authorization") || msg.contains("invalid header"),
        "expected an invalid-header error, got: {msg}"
    );
}

/// Bug fix v0.2.1 #8 (OneOf/AllOf/AnyOf): `allOf` should merge sub-schema
/// properties + required, not degrade to `Schema::string()`.
#[test]
fn all_of_merges_properties_and_required() {
    let spec = r#"
openapi: 3.0.0
info:
  title: AllOf
  version: 1.0.0
paths:
  /pets:
    post:
      operationId: createComposite
      requestBody:
        required: true
        content:
          application/json:
            schema:
              allOf:
                - $ref: '#/components/schemas/Base'
                - type: object
                  properties:
                    extra:
                      type: string
                  required: [extra]
      responses:
        '201':
          description: ok
components:
  schemas:
    Base:
      type: object
      properties:
        id:
          type: integer
      required: [id]
"#;
    let tools = OpenAPIToolset::from_yaml(spec).unwrap().into_tools();
    let create = tools
        .into_iter()
        .find(|t| t.name() == "create_composite")
        .unwrap();
    let decl = create.declaration().unwrap();
    let params = decl.parameters.unwrap();
    let body = params.properties.get("body").unwrap();
    assert_eq!(body.r#type, Some(SchemaType::Object));
    assert!(
        body.properties.contains_key("id"),
        "merged properties should include `id` from Base"
    );
    assert!(
        body.properties.contains_key("extra"),
        "merged properties should include `extra` from inline sub-schema"
    );
    assert!(body.required.contains(&"id".to_string()));
    assert!(body.required.contains(&"extra".to_string()));
}

#[test]
fn request_body_refs_are_resolved_in_tool_schema() {
    let spec = r#"
openapi: 3.0.0
info:
  title: Petstore
  version: 1.0.0
paths:
  /pets:
    post:
      operationId: createPet
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/Pet'
      responses:
        '201':
          description: created
components:
  schemas:
    Pet:
      type: object
      properties:
        name:
          type: string
      required: [name]
"#;
    let tools = OpenAPIToolset::from_yaml(spec).unwrap().into_tools();
    let create = tools
        .into_iter()
        .find(|t| t.name() == "create_pet")
        .expect("create_pet tool");
    let decl = create.declaration().expect("declaration");
    let params = decl.parameters.expect("parameters");
    let body = params.properties.get("body").expect("body property");
    assert_eq!(body.r#type, Some(SchemaType::Object));
    assert!(body.properties.contains_key("name"));
    assert_eq!(body.required, vec!["name".to_string()]);
}

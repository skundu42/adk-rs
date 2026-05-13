//! End-to-end: load a tiny Petstore-style spec, attach a bearer credential,
//! and exercise the generated tool against a `wiremock` backend.

#![cfg(all(feature = "openapi", feature = "auth"))]

use adk_rs::auth::credential::AuthCredential;
use adk_rs::core::testing::MockModel;
use adk_rs::core::{
    GetSessionConfig, InvocationContext, InvocationOrigin, ListSessionsResponse, RunConfig,
    Session, SessionService, State, ToolContext,
};
use adk_rs::tools::openapi::OpenAPIToolset;
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::json;
use std::sync::Arc;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Debug)]
struct NoopSession;
#[async_trait]
impl SessionService for NoopSession {
    async fn create_session(
        &self,
        app: &str,
        user: &str,
        _: Option<State>,
        id: Option<&str>,
    ) -> adk_rs::Result<Session> {
        Ok(Session::new(app, user, id.unwrap_or("s")))
    }
    async fn get_session(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: GetSessionConfig,
    ) -> adk_rs::Result<Option<Session>> {
        Ok(None)
    }
    async fn list_sessions(&self, _: &str, _: &str) -> adk_rs::Result<ListSessionsResponse> {
        Ok(ListSessionsResponse::default())
    }
    async fn delete_session(&self, _: &str, _: &str, _: &str) -> adk_rs::Result<()> {
        Ok(())
    }
}

fn ctx_with_auth(cred: Option<AuthCredential>) -> ToolContext {
    let inv = Arc::new(InvocationContext {
        app_name: "app".into(),
        user_id: "u".into(),
        invocation_id: "inv".into(),
        session: Arc::new(Mutex::new(Session::new("app", "u", "s"))),
        session_service: Arc::new(NoopSession),
        artifact_service: None,
        memory_service: None,
        credential_service: None,
        run_config: RunConfig::default(),
        origin: InvocationOrigin::Api,
        user_content: None,
        llm_call_count: Arc::new(Mutex::new(0)),
        attributes: Arc::new(Mutex::new(Default::default())),
    });
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

// silence unused-import warnings when MockModel isn't used (kept in case a
// future test wires through the runner).
#[allow(dead_code)]
fn _touch(_m: MockModel) {}

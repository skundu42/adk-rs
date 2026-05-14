//! End-to-end OAuth2 exchanger tests against a mock token endpoint.

#![cfg(feature = "auth")]

use adk_rs::auth::CredentialService;
use adk_rs::auth::config::AuthConfig;
use adk_rs::auth::credential::{AuthCredential, OAuth2Auth};
use adk_rs::auth::exchanger::{CredentialExchanger, OAuth2Exchanger};
use adk_rs::auth::handler::AuthHandler;
use adk_rs::auth::manager::CredentialManager;
use adk_rs::auth::refresher::{CredentialRefresher, OAuth2Refresher};
use adk_rs::auth::scheme::{AuthScheme, OAuthFlow, OAuthFlows};
use adk_rs::auth::service::InMemoryCredentialService;
use serde_json::json;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn cc_scheme(server: &MockServer) -> AuthScheme {
    AuthScheme::OAuth2 {
        flows: OAuthFlows {
            client_credentials: Some(OAuthFlow {
                authorization_url: None,
                token_url: format!("{}/token", server.uri()),
                refresh_url: None,
                scopes: Default::default(),
            }),
            ..OAuthFlows::default()
        },
        description: None,
    }
}

fn ac_scheme(server: &MockServer) -> AuthScheme {
    AuthScheme::OAuth2 {
        flows: OAuthFlows {
            authorization_code: Some(OAuthFlow {
                authorization_url: Some(format!("{}/authorize", server.uri())),
                token_url: format!("{}/token", server.uri()),
                refresh_url: None,
                scopes: Default::default(),
            }),
            ..OAuthFlows::default()
        },
        description: None,
    }
}

#[tokio::test]
async fn client_credentials_flow_exchanges_and_carries_token() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=client_credentials"))
        .and(body_string_contains("scope=read"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "AT-abc123",
            "expires_in": 3600,
            "token_type": "Bearer",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let cfg = AuthConfig::new(cc_scheme(&server)).with_raw(AuthCredential::oauth2(OAuth2Auth {
        client_id: "id".into(),
        client_secret: Some("secret".into()),
        scopes: vec!["read".into()],
        ..OAuth2Auth::default()
    }));
    let raw = cfg.raw_auth_credential.as_ref().unwrap().clone();
    let out = OAuth2Exchanger
        .exchange(&cfg, &raw)
        .await
        .unwrap()
        .expect("expected Some(AuthCredential)");
    let o = out.oauth2.unwrap();
    assert_eq!(o.access_token.as_deref(), Some("AT-abc123"));
    assert!(o.expires_at.unwrap_or_default() > 0);
}

#[tokio::test]
async fn authorization_code_flow_exchanges_with_pkce() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .and(body_string_contains("code=user-code-xyz"))
        .and(body_string_contains("code_verifier="))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "AT-pkce-456",
            "refresh_token": "RT-789",
            "expires_in": 1800,
            "token_type": "Bearer",
        })))
        .expect(1)
        .mount(&server)
        .await;

    // Run the authorize step to generate a PKCE verifier we can replay.
    let scheme = ac_scheme(&server);
    let mut raw_oauth2 = OAuth2Auth {
        client_id: "client-1".into(),
        client_secret: Some("client-secret".into()),
        auth_uri: Some(format!("{}/authorize", server.uri())),
        token_uri: Some(format!("{}/token", server.uri())),
        redirect_uri: Some("http://localhost/cb".into()),
        ..OAuth2Auth::default()
    };
    let (_url, _state, verifier) = AuthHandler::from_oauth2(&raw_oauth2)
        .unwrap()
        .authorize_url(&[]);
    raw_oauth2.code_verifier = Some(verifier);
    raw_oauth2.auth_code = Some("user-code-xyz".into());

    let cfg = AuthConfig::new(scheme).with_raw(AuthCredential::oauth2(raw_oauth2.clone()));
    let raw = AuthCredential::oauth2(raw_oauth2);
    let out = OAuth2Exchanger
        .exchange(&cfg, &raw)
        .await
        .unwrap()
        .expect("expected Some(AuthCredential)");
    let o = out.oauth2.unwrap();
    assert_eq!(o.access_token.as_deref(), Some("AT-pkce-456"));
    assert_eq!(o.refresh_token.as_deref(), Some("RT-789"));
}

/// Bug fix v0.2.1 #4: `begin_consent` → user redirect → `complete_consent`
/// validates the CSRF state and exchanges the auth code via the persisted
/// PKCE verifier. Replaying with a mismatched state must fail with a
/// CSRF-flavoured error.
#[tokio::test]
async fn consent_round_trip_validates_state() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .and(body_string_contains("code=cb-code"))
        .and(body_string_contains("code_verifier="))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "AT-after-consent",
            "expires_in": 3600,
            "token_type": "Bearer",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let cfg = AuthConfig::new(ac_scheme(&server))
        .with_raw(AuthCredential::oauth2(OAuth2Auth {
            client_id: "client-1".into(),
            client_secret: Some("secret".into()),
            auth_uri: Some(format!("{}/authorize", server.uri())),
            token_uri: Some(format!("{}/token", server.uri())),
            redirect_uri: Some("http://localhost/cb".into()),
            ..OAuth2Auth::default()
        }))
        .with_key("test-app");
    let mgr = CredentialManager::new(cfg);
    let svc = InMemoryCredentialService::new();

    // 1. Begin consent. Caller would redirect the user to `auth_uri`.
    let consent = mgr.begin_consent(&svc).await.unwrap();
    assert!(consent.auth_uri.contains("code_challenge"));
    let flow_id = consent.flow_id.clone();

    // 2. Replay with a *mismatched* state — must reject (CSRF).
    let err = mgr
        .complete_consent("app", "u", &flow_id, "wrong-state", "cb-code", &svc)
        .await
        .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("csrf")
            || err.to_string().to_lowercase().contains("does not match"),
        "expected CSRF error, got: {err}"
    );

    // 3. Replay with the correct state — exchanges and caches.
    let exchanged = mgr
        .complete_consent("app", "u", &flow_id, &flow_id, "cb-code", &svc)
        .await
        .unwrap();
    assert_eq!(
        exchanged.oauth2.unwrap().access_token.as_deref(),
        Some("AT-after-consent")
    );

    // Cached under the regular key for next time.
    let cached = svc.load("app", "u", "test-app").await.unwrap().unwrap();
    assert_eq!(
        cached.oauth2.unwrap().access_token.as_deref(),
        Some("AT-after-consent")
    );
}

#[tokio::test]
async fn refresher_uses_refresh_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("refresh_token=RT-orig"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "AT-new",
            "expires_in": 3600,
            "token_type": "Bearer",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let cfg = AuthConfig::new(ac_scheme(&server));
    let cred = AuthCredential::oauth2(OAuth2Auth {
        client_id: "client-1".into(),
        client_secret: Some("client-secret".into()),
        auth_uri: Some(format!("{}/authorize", server.uri())),
        token_uri: Some(format!("{}/token", server.uri())),
        redirect_uri: Some("http://localhost/cb".into()),
        access_token: Some("AT-old".into()),
        refresh_token: Some("RT-orig".into()),
        expires_at: Some(0),
        ..OAuth2Auth::default()
    });
    let refreshed = OAuth2Refresher
        .refresh(&cfg, &cred)
        .await
        .unwrap()
        .expect("refresh returned None");
    assert_eq!(
        refreshed.oauth2.unwrap().access_token.as_deref(),
        Some("AT-new")
    );
}

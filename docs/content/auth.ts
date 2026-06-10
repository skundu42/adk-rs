import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'auth',
  title: 'Authenticated tools',
  description:
    'Declare auth requirements on tools and let the runner resolve API keys, OAuth2 tokens, and service accounts before dispatch.',
  srcPath: 'src/auth/mod.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'A tool declares *what the server expects* (`AuthScheme`) plus *what it starts with* (a raw `AuthCredential`) in an `AuthConfig`. Before dispatching the tool, the runner resolves that config into a ready credential — from cache, by exchange, by refresh, or by pausing for interactive OAuth2 consent — and injects it into `ToolContext::auth_credential`. The data types are always available; the OAuth2 machinery lives behind `feature = "auth"`.',
    },
    { kind: 'h2', text: 'The type layer' },
    {
      kind: 'p',
      text: '`AuthScheme` mirrors the OpenAPI 3 `securityScheme` subset adk-rs supports, plus a `Custom` escape hatch wired to the `AuthProviderRegistry`:',
    },
    {
      kind: 'api',
      entries: [
        {
          sig: 'AuthScheme::ApiKey { location: ApiKeyLocation, name: String, description: Option<String> }',
          desc: 'Static API key. `ApiKeyLocation` is `Header`, `Query`, or `Cookie`; `name` is the header/query/cookie key.',
        },
        {
          sig: 'AuthScheme::Http { scheme: String, bearer_format: Option<String>, description: Option<String> }',
          desc: 'HTTP auth: `scheme` is `"basic"` or `"bearer"`.',
        },
        {
          sig: 'AuthScheme::OAuth2 { flows: OAuthFlows, description: Option<String> }',
          desc: '`OAuthFlows` holds optional `authorization_code`, `client_credentials`, `implicit`, and `password` flow descriptors (each an `OAuthFlow` with `authorization_url`, `token_url`, `refresh_url`, `scopes`).',
        },
        {
          sig: 'AuthScheme::OpenIdConnect { open_id_connect_url: String, scopes: Vec<String>, description: Option<String> }',
          desc: 'OIDC via a discovery-document URL.',
        },
        {
          sig: 'AuthScheme::Custom { tag: String, properties: serde_json::Value }',
          desc: 'Vendor-specific scheme resolved by a custom `BaseAuthProvider`.',
        },
      ],
    },
    {
      kind: 'p',
      text: '`AuthCredential` is the value side — a single envelope whose `auth_type` discriminator says which inner payload (`api_key`, `http`, `oauth2`, `service_account`) is populated. Constructors cover the common cases:',
    },
    {
      kind: 'api',
      entries: [
        { sig: 'AuthCredential::api_key(value) / ::bearer(token) / ::basic(username, password)', desc: 'Ready-to-use credentials — no exchange needed.' },
        { sig: 'AuthCredential::oauth2(OAuth2Auth) / ::service_account(ServiceAccountAuth)', desc: 'Raw credentials that need an exchange (or consent) before they carry an `access_token`.' },
        { sig: 'AuthCredential::is_ready(&self) -> bool', desc: 'True when a usable access value is present without further exchange.' },
        { sig: 'AuthCredential::is_expired(&self, now_unix: i64) -> bool', desc: 'True when an expiry is set and has passed, with a 60-second leeway.' },
        { sig: 'AuthConfig::new(scheme).with_raw(cred).with_key(key)', desc: 'Pair scheme + raw credential. `with_key` pins the cache key; otherwise `resolve_credential_key()` derives a stable SHA-256 digest from scheme + raw credential.' },
      ],
    },
    { kind: 'h2', text: 'Credential storage' },
    {
      kind: 'p',
      text: 'Resolved credentials persist through the `CredentialService` trait, keyed by `(app_name, user_id, key)` with `load` / `save` / `delete`. Two implementations ship in the crate: `InMemoryCredentialService` (volatile, process-local) and `SessionStateCredentialService` (stores under `temp:`-prefixed state keys, so credentials live no longer than the session). Register one on the runner with `Runner::builder().credential_service(...)`.',
    },
    { kind: 'h2', text: 'The resolution pipeline' },
    {
      kind: 'p',
      text: 'A tool opts in by returning `Some(&AuthConfig)` from `DynTool::auth_config()`. `FunctionTool` does not expose this hook — implement `DynTool` directly for bespoke authenticated tools, or use the [OpenAPI toolset](/docs/openapi-tools), whose `RestApiTool` carries an `AuthConfig` built from the spec\'s security schemes. During dispatch (after the [confirmation gate](/docs/tool-confirmation), before `run`), the agent builds a `CredentialManager` and resolves:',
    },
    {
      kind: 'list',
      ordered: true,
      items: [
        'Validate the config (a `raw_auth_credential` is required).',
        'If the raw credential `is_ready` and not expired, hand it straight to the tool.',
        'Try the cache: `credential_service.load(app, user, key)`; an expired cached credential is refreshed via the `RefresherRegistry` and saved back.',
        'OAuth2/OIDC authorization-code flow with no token yet → return `NeedsUserConsent` (the interactive pause below).',
        'Exchange: `OAuth2Exchanger` (auth-code + PKCE verifier, or client-credentials) or `ServiceAccountExchanger` (signed RS256 JWT traded for an access/ID token), then save.',
        'Custom-provider escape hatch via the `AuthProviderRegistry`.',
        'Otherwise `Misconfigured(msg)` — surfaced to the model as an `{"error": ...}` tool response.',
      ],
    },
    {
      kind: 'api',
      entries: [
        {
          sig: 'CredentialManager::resolve(&self, app, user, credentials: Option<&dyn CredentialService>) -> Result<ResolveOutcome>',
          desc: 'Runs the workflow. `ResolveOutcome` is `Ready(AuthCredential)`, `NeedsUserConsent(AuthConfig)`, or `Misconfigured(String)`.',
        },
        {
          sig: 'ToolContext::auth_credential: Option<AuthCredential>',
          desc: 'Where a `Ready` credential lands before `run` is called. Read your token/key from here, never from the model-supplied args.',
        },
      ],
    },
    { kind: 'h2', text: 'Interactive consent: pause and resume' },
    {
      kind: 'p',
      text: 'When resolution returns `NeedsUserConsent`, the tool is *not* run. The agent emits a `FunctionResponse` named `adk_request_credential` (`REQUEST_CREDENTIAL_FUNCTION_NAME`) carrying the pending `AuthConfig`, marks the call long-running, and pauses the invocation — the same suspend mechanics as [tool confirmation](/docs/tool-confirmation). Your application walks the user through consent, then resubmits a `FunctionResponse` with the **same call id**, name `adk_request_credential`, and the `AuthConfig` with `exchanged_auth_credential` filled in. On the next invocation, `AuthPreprocessor::process_event` absorbs it: the credential is saved to the credential service and the original call id is returned in `resumed_tool_call_ids`, so the owning agent replays the deferred call with a ready credential. Ids prefixed `_adk_toolset_auth_` (`TOOLSET_AUTH_CREDENTIAL_ID_PREFIX`) authorise an entire toolset before tool discovery.',
    },
    {
      kind: 'p',
      text: 'For driving the browser leg yourself, `CredentialManager` exposes a hardened two-step API: `begin_consent(&credentials)` generates the authorization URL with a fresh PKCE S256 challenge and CSRF state (persisting both, keyed by an opaque `flow_id`), and `complete_consent(app, user, flow_id, callback_state, callback_code, &credentials)` validates the state in constant time, exchanges the code with the stored verifier, caches the result under the regular key, and deletes the single-use pending entry. `AuthHandler` underneath also rejects token endpoints that are neither `https://` nor loopback — validation delegates to the crate-wide [transport-security](/docs/security) policy, and loopback `http://` stays available for local testing.',
    },
    { kind: 'h2', text: 'Worked example: API key' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'A tool that reads its key from ToolContext',
      code: `use adk_rs::auth::{ApiKeyLocation, AuthConfig, AuthCredential, AuthScheme};
use adk_rs::core::ToolContext;
use adk_rs::genai_types::FunctionDeclaration;
use adk_rs::tools::Tool; // re-export of core::DynTool
use serde_json::Value;

#[derive(Debug)]
struct WeatherApi {
    auth: AuthConfig,
}

impl WeatherApi {
    fn new(key: String) -> Self {
        Self {
            auth: AuthConfig::new(AuthScheme::ApiKey {
                location: ApiKeyLocation::Header,
                name: "X-API-Key".into(),
                description: None,
            })
            .with_raw(AuthCredential::api_key(key)),
        }
    }
}

#[async_trait::async_trait]
impl Tool for WeatherApi {
    fn name(&self) -> &str { "get_weather" }
    fn description(&self) -> &str { "Fetch the current weather" }
    fn auth_config(&self) -> Option<&AuthConfig> { Some(&self.auth) }
    fn declaration(&self) -> Option<FunctionDeclaration> {
        Some(FunctionDeclaration::new(self.name(), self.description()))
    }
    async fn run(&self, _args: Value, ctx: &mut ToolContext) -> adk_rs::Result<Value> {
        let key = ctx.auth_credential.as_ref()
            .and_then(|c| c.api_key.as_deref())
            .ok_or_else(|| adk_rs::Error::other("credential not resolved"))?;
        // ... call the API with \`key\` in the X-API-Key header ...
        Ok(serde_json::json!({ "ok": true }))
    }
}`,
    },
    { kind: 'h2', text: 'OAuth2 sketch' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Authorization-code config (consent happens via adk_request_credential)',
      code: `use adk_rs::auth::{
    AuthConfig, AuthCredential, AuthScheme, InMemoryCredentialService,
    OAuth2Auth, OAuthFlow, OAuthFlows,
};

let cfg = AuthConfig::new(AuthScheme::OAuth2 {
    flows: OAuthFlows {
        authorization_code: Some(OAuthFlow {
            authorization_url: Some("https://provider/authorize".into()),
            token_url: "https://provider/token".into(),
            refresh_url: None,
            scopes: Default::default(),
        }),
        ..OAuthFlows::default()
    },
    description: None,
})
.with_raw(AuthCredential::oauth2(OAuth2Auth {
    client_id: "client".into(),
    client_secret: Some("secret".into()),
    ..OAuth2Auth::default()
}))
.with_key("provider-oauth");

let runner = Runner::builder()
    .app_name("app")
    .agent(agent) // a tool returning Some(&cfg) from auth_config()
    .session_service(svc)
    .credential_service(Arc::new(InMemoryCredentialService::new()))
    .build()?;
// First call to the tool pauses with adk_request_credential; resubmit
// the AuthConfig with exchanged_auth_credential set to resume. A
// client_credentials flow instead exchanges server-to-server with no
// pause, and refresh tokens are used automatically on expiry.`,
    },
    {
      kind: 'callout',
      tone: 'note',
      text: 'The integration test `tests/auth_oauth2.rs` exercises all of these flows against a mock token endpoint: client-credentials exchange, authorization-code + PKCE, the `begin_consent`/`complete_consent` round trip with CSRF rejection, and refresh-token rotation.',
    },
    { kind: 'hr' },
    {
      kind: 'list',
      items: [
        '[OpenAPI tools](/docs/openapi-tools) — tools that carry `AuthConfig` automatically from spec security schemes.',
        '[Tool confirmation](/docs/tool-confirmation) — the sibling pause/resume gate.',
        '[Cancellation & resume](/docs/cancellation-and-resume) — resuming paused invocations in place.',
        '[Security](/docs/security) — transport-security rules for credential-bearing clients.',
      ],
    },
  ],
};

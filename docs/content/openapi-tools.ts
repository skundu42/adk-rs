import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'openapi-tools',
  title: 'OpenAPI tools',
  description:
    'Generating one callable tool per operation from an OpenAPI 3.x spec with OpenAPIToolset, including credential binding, parameter mapping, and HTTP execution via RestApiTool.',
  srcPath: 'src/tools/openapi/toolset.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'Point `OpenAPIToolset` at an OpenAPI 3.x spec and get back a `Vec<Arc<dyn Tool>>` — one `RestApiTool` per operation, each with a JSON-Schema declaration derived from the spec’s parameters and a `run` that assembles and sends the real HTTP request. Security schemes in the spec map onto the [auth](/docs/auth) system, so the runner resolves credentials before each call.',
    },
    {
      kind: 'callout',
      tone: 'note',
      text: 'Everything on this page lives behind the `openapi` cargo feature (module `adk_rs::tools::openapi`). Credential binding additionally uses types from the always-compiled `adk_rs::auth` module.',
    },
    { kind: 'h2', text: 'OpenAPIToolset' },
    {
      kind: 'api',
      entries: [
        {
          sig: 'OpenAPIToolset::from_yaml(spec: &str) -> Result<Self>',
          desc: 'Parse a YAML **or** JSON spec string (the YAML parser accepts both).',
        },
        {
          sig: 'OpenAPIToolset::from_json(spec: &str) -> Result<Self>',
          desc: 'Alias for `from_yaml`, for callers who want the intent in the name.',
        },
        {
          sig: 'OpenAPIToolset::from_path(path: impl AsRef<Path>) -> Result<Self>',
          desc: 'Read a spec file from disk and parse it.',
        },
        {
          sig: 'fn with_credential(self, scheme_name: impl Into<String>, cred: AuthCredential) -> Self',
          desc: 'Bind a credential to one of the spec’s `securitySchemes` by name; every operation accepting that scheme gets it attached.',
        },
        {
          sig: 'fn with_base_url(self, base: impl Into<String>) -> Self',
          desc: 'Override the spec’s `servers[0]` URL on every operation — e.g. to point at a staging host or a local mock.',
        },
        {
          sig: 'fn operation_names(&self) -> Vec<&str>',
          desc: 'The generated tool names, handy for building filters.',
        },
        {
          sig: 'fn build_tools<F: Fn(&ParsedOperation) -> bool>(self, predicate: F) -> Vec<Arc<dyn DynTool>>',
          desc: 'Generate one `RestApiTool` per operation passing the predicate.',
        },
        {
          sig: 'fn into_tools(self) -> Vec<Arc<dyn DynTool>>',
          desc: 'Generate all tools — shorthand for `build_tools(|_| true)`.',
        },
      ],
    },
    { kind: 'h2', text: 'How operations become tools' },
    {
      kind: 'list',
      items: [
        '**Name** — the `operationId`, converted to snake_case (`getPetById` → `get_pet_by_id`). Operations without an id fall back to a method+path name.',
        '**Description** — the operation’s `summary`, or `description` when no summary exists.',
        '**Parameters** — each path/query/header/cookie parameter becomes an `ApiParameter` with a snake_case arg name, a `ParamLocation`, and a JSON `Schema`; `build_args_schema()` merges them into a single object schema with the spec’s `required` flags preserved.',
        '**Request body** — exposed as a single `body` argument; `$ref`s are resolved and `allOf` compositions are merged into one object schema.',
      ],
    },
    { kind: 'h2', text: 'RestApiTool execution' },
    {
      kind: 'p',
      text: 'At call time, `RestApiTool::run` validates that every required parameter is present, then assembles the request by `ParamLocation`: path params are substituted into the URL template with percent-encoding (`a b` → `/pets/a%20b`), query params join the query string, header params become headers, cookie params are appended to a `Cookie` header with RFC 6265 validation (so embedded `;` or CRLF cannot smuggle extra cookies), and the `body` arg is sent as JSON. The response always comes back as `{"status": <u16>, "body": <json-or-string>}` — non-2xx statuses are returned, not raised, so the model can react to them.',
    },
    { kind: 'h3', text: 'Security schemes → AuthConfig' },
    {
      kind: 'p',
      text: 'Each operation’s `security` requirements are matched against the spec’s `securitySchemes`. The first scheme with a bound credential yields `AuthConfig::new(scheme).with_raw(credential)`; a recognised scheme with **no** bound credential still surfaces an `AuthConfig`, so the runner reports a misconfiguration instead of silently calling unauthenticated. Before `run`, the runner resolves the config and injects the result into `ToolContext::auth_credential`; the tool then injects it per scheme type — `apiKey` into a header, query param, or cookie; `http: bearer` / `basic` into `Authorization`; OAuth2/OIDC access tokens as `Bearer`.',
    },
    {
      kind: 'callout',
      tone: 'warn',
      title: 'HTTPS guard',
      text: 'Before attaching any credential, `RestApiTool` checks the final URL with `require_secure_url`: it must be `https://` or a loopback host. This catches both a misconfigured `base_url` and a malicious `servers[]` entry in a third-party spec that would otherwise exfiltrate your token over plaintext. Header values are also validated — a token containing CRLF fails the call instead of silently sending the request unauthenticated.',
    },
    { kind: 'h2', text: 'Petstore walkthrough' },
    {
      kind: 'p',
      text: 'This mirrors the repo’s integration test (`tests/openapi_petstore.rs`), which runs the generated tools against a wiremock backend.',
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Spec → tools → agent',
      code: `use adk_rs::auth::credential::AuthCredential;
use adk_rs::tools::openapi::OpenAPIToolset;

const SPEC: &str = r#"
openapi: 3.0.0
info: { title: Petstore, version: 1.0.0 }
servers:
  - url: https://petstore.example.com
paths:
  /pets/{id}:
    get:
      operationId: getPetById
      security: [ { bearerAuth: [] } ]
      parameters:
        - { name: id, in: path, required: true, schema: { type: integer } }
      responses: { '200': { description: ok } }
  /pets:
    post:
      operationId: createPet
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties: { name: { type: string } }
              required: [name]
      responses: { '201': { description: created } }
components:
  securitySchemes:
    bearerAuth: { type: http, scheme: bearer }
"#;

let tools = OpenAPIToolset::from_yaml(SPEC)?
    .with_credential("bearerAuth", AuthCredential::bearer("my-token"))
    .into_tools();
// tools[..] are named get_pet_by_id and create_pet

let agent = adk_rs::agents::LlmAgent::builder("petstore")
    .model(model)
    .instruction("Manage pets via the API tools.")
    .tools(tools)
    .build()?;`,
    },
    {
      kind: 'p',
      text: 'When the model emits `getPetById`-style calls, the agent dispatches `get_pet_by_id` with `{"id": 42}`; the tool issues `GET https://petstore.example.com/pets/42` with `Authorization: Bearer my-token` and hands `{"status": 200, "body": {...}}` back to the model. A `create_pet` call sends its `body` argument as the JSON request body.',
    },
    { kind: 'h2', text: 'Filtering and lower-level access' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Only expose read operations',
      code: `use adk_rs::tools::openapi::{HttpMethod, OpenAPIToolset};

let read_only = OpenAPIToolset::from_path("specs/petstore.yaml")?
    .build_tools(|op| matches!(op.method, HttpMethod::Get | HttpMethod::Head));`,
    },
    {
      kind: 'p',
      text: 'The module also exports the building blocks — `parse_spec` / `SpecParse`, `ParsedOperation`, `ApiParameter`, `ParamLocation`, `HttpMethod`, `to_snake_case`, and `RestApiTool::new(op, auth_config)` — if you want to construct tools from hand-built operations.',
    },
    { kind: 'h2', text: 'Related pages' },
    {
      kind: 'list',
      items: [
        '[Auth](/docs/auth) — `AuthCredential`, `AuthConfig`, and the credential-resolution pipeline.',
        '[Tools overview](/docs/tools-overview) — how generated tools join the dispatch loop.',
        '[Security](/docs/security) — the HTTPS-or-loopback rule across the crate.',
      ],
    },
  ],
};

import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'artifacts',
  title: 'Artifacts',
  description:
    'Versioned named binary or text parts scoped to app, user, and session, the ArtifactService trait, and the in-memory and filesystem backends.',
  srcPath: 'src/core/artifact.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'Artifacts are versioned, named blobs — files a user uploaded, reports an agent generated, images a tool produced. Each one is a genai Part stored under an (app, user, session, filename) key, and every save creates a new monotonically increasing version.',
    },
    { kind: 'h2', text: 'ArtifactKey and Artifact' },
    {
      kind: 'p',
      text: '`ArtifactKey` (in `adk_rs::core::artifact`) is the four-part address: `app_name`, `user_id`, `session_id`, and `filename` (which may contain `/` for a hierarchical layout). Build one with `ArtifactKey::new(app, user, session, filename)`. The stored value is a `Part` — typically `Text`, `InlineData`, or `FileData` — and the companion `Artifact` struct bundles a `part` with its `version` (1-indexed, monotonically increasing per key).',
    },
    { kind: 'h2', text: 'The ArtifactService trait' },
    {
      kind: 'api',
      entries: [
        { sig: 'async fn save_artifact(&self, key: ArtifactKey, part: Part) -> Result<u64>', desc: 'Store a new version; returns the new version number (1 for the first save).' },
        { sig: 'async fn load_artifact(&self, key: ArtifactKey, version: Option<u64>) -> Result<Option<Part>>', desc: 'Load a specific version, or the latest when `version` is `None`. `Ok(None)` for unknown keys.' },
        { sig: 'async fn list_artifact_keys(&self, app_name, user_id, session_id) -> Result<Vec<String>>', desc: 'Filenames stored in one session.' },
        { sig: 'async fn delete_artifact(&self, key: ArtifactKey) -> Result<()>', desc: 'Delete all versions of an artifact.' },
        { sig: 'async fn list_versions(&self, key: ArtifactKey) -> Result<Vec<u64>>', desc: 'All version numbers for a key, ascending.' },
      ],
    },
    { kind: 'h2', text: 'Backends' },
    {
      kind: 'table',
      head: ['Type', 'Module', 'Feature', 'Notes'],
      rows: [
        ['`InMemoryArtifactService`', '`adk_rs::services::mem`', 'always on', 'Volatile `DashMap<ArtifactKey, Vec<Part>>`; version N is the N-th saved part.'],
        ['`FileArtifactService`', '`adk_rs::services::fs`', '`fs`', 'Stores each version as JSON under `<root>/<app>/<user>/<session>/<filename>/v000001.json`. Construct with `FileArtifactService::new(root)`.'],
      ],
    },
    {
      kind: 'callout',
      tone: 'warn',
      title: 'Path-traversal sanitization',
      text: 'Every path component the filesystem backend writes goes through a `sanitize` step: characters outside `[alphanumeric _ - .]` become `_`, and dot-only components (`.`, `..`, `...`) or empty strings collapse to `_`. An attacker-controlled `app_name`, `user_id`, `session_id`, or `filename` therefore cannot escape the artifact root via `..` segments — and `delete_artifact` (which calls `remove_dir_all`) stays scoped below the root.',
    },
    { kind: 'h2', text: 'Artifacts from tools: ToolContext helpers' },
    {
      kind: 'p',
      text: '`ToolContext` (in `adk_rs::core::context`) wraps the configured service with two helpers that fill in the `ArtifactKey` from the current invocation. `save_artifact` also records the new version in `ToolContext.artifact_delta` (filename → version), the accumulator mirrored by `EventActions.artifact_delta` on [events](/docs/events). Both error if no artifact service is configured on the [runner](/docs/runner).',
    },
    {
      kind: 'api',
      entries: [
        { sig: 'async fn save_artifact(&mut self, filename: &str, part: Part) -> Result<u64>', desc: 'Save under the invocation’s `(app, user, session)`; returns the new version and records it in `artifact_delta`.' },
        { sig: 'async fn load_artifact(&self, filename: &str, version: Option<u64>) -> Result<Option<Part>>', desc: 'Load by filename within the invocation scope; latest when `version` is `None`.' },
      ],
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'A tool that saves and re-loads an artifact',
      code: `use adk_rs::core::{DynTool, ToolContext};
use adk_rs::error::Result;
use adk_rs::genai_types::{FunctionDeclaration, Part};
use async_trait::async_trait;
use serde_json::{Value, json};

#[derive(Debug)]
struct WriteReport;

#[async_trait]
impl DynTool for WriteReport {
    fn name(&self) -> &str {
        "write_report"
    }
    fn description(&self) -> &str {
        "Render a report and store it as an artifact"
    }
    fn declaration(&self) -> Option<FunctionDeclaration> {
        Some(FunctionDeclaration::new(self.name(), self.description()))
    }
    async fn run(&self, args: Value, ctx: &mut ToolContext) -> Result<Value> {
        let body = args["body"].as_str().unwrap_or_default().to_string();

        // Save: returns the new 1-indexed version and records it in
        // ctx.artifact_delta.
        let version = ctx.save_artifact("report.md", Part::text(body)).await?;

        // Load it back (latest version).
        let stored = ctx.load_artifact("report.md", None).await?;
        let chars = stored.and_then(|p| p.as_text().map(str::len)).unwrap_or(0);

        Ok(json!({ "saved": "report.md", "version": version, "chars": chars }))
    }
}`,
    },
    { kind: 'h2', text: 'The load_artifacts tool' },
    {
      kind: 'p',
      text: '`adk_rs::tools::load_artifacts_tool()` returns a built-in tool the **model** can call to pull saved artifacts into the conversation. It takes `{ "artifact_names": ["a.txt", ...] }` and returns `{ "loaded": [...] }`, where each entry carries the artifact `name` and its serialized `part` — or `"error": "not found"` for missing names. Register it like any other tool:',
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Registering load_artifacts on an agent',
      code: `use adk_rs::agents::LlmAgent;
use adk_rs::tools::load_artifacts_tool;

let agent = LlmAgent::builder("analyst")
    .model(model)
    .instruction("Use load_artifacts to read files the user uploaded.")
    .tool(load_artifacts_tool())
    .build()?;`,
    },
    { kind: 'h2', text: 'Instruction templating: {artifact.name}' },
    {
      kind: 'p',
      text: 'Static instructions support `{artifact.<filename>}` placeholders (see `src/agents/instructions.rs`): at request-build time the named artifact’s latest version is loaded and its content rendered into the prompt. The optional form `{artifact.<filename>?}` resolves to the empty string when the artifact is missing; the non-optional form is a hard error, as is using the placeholder with no artifact service configured. The same templating handles `{key}`, `{key?}`, and `{app:key}` / `{user:key}` / `{temp:key}` state references.',
    },
    { kind: 'h2', text: 'Related pages' },
    {
      kind: 'list',
      items: [
        '[The Runner](/docs/runner) — wiring `artifact_service` into the builder.',
        '[Function tools](/docs/function-tools) — the `ToolContext` surface in full.',
        '[Server](/docs/server) — HTTP endpoints for listing and loading artifacts.',
        '[Security](/docs/security) — more on the filesystem backend’s hardening.',
      ],
    },
  ],
};

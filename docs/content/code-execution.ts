import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'code-execution',
  title: 'Code execution',
  description:
    'Run model-emitted code through a pluggable CodeExecutor, locally or in a locked-down ephemeral Docker container.',
  srcPath: 'src/code_exec/mod.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'With a `CodeExecutor` attached, an `LlmAgent` turns the model into a programmer: the model emits `Part::ExecutableCode`, the executor runs it, and a `Part::CodeExecutionResult` is fed back on the next turn so the model can interpret — or fix — its own output. Enable the subsystem with `feature = "code-exec"`; the Docker sandbox additionally needs `code-exec-docker`.',
    },
    { kind: 'h2', text: 'The execution loop' },
    {
      kind: 'list',
      ordered: true,
      items: [
        'The model replies with one or more `ExecutableCode` parts (`language` + `code`) instead of a final answer.',
        'The agent extracts them and calls `executor.execute_code(...)` for each, retrying up to `error_retry_attempts()` times on *executor* errors (spawn failures, I/O) — a non-zero exit code from the program itself is a result, not an error.',
        'Each result becomes a `CodeExecutionResult` part with `outcome: OutcomeOk` when `result.is_success()` (exit code 0, or no exit code and empty stderr) and `OutcomeFailed` otherwise; the output is `stdout`, with stderr appended under a `--- stderr ---` divider when both exist.',
        'The code and its results are appended to the next turn\'s contents and the loop continues until the model answers without emitting code.',
      ],
    },
    { kind: 'h2', text: 'The CodeExecutor trait' },
    {
      kind: 'api',
      entries: [
        {
          sig: 'trait CodeExecutor { async fn execute_code(&self, ctx: &InvocationContext, input: CodeExecutionInput) -> Result<CodeExecutionResult>; ... }',
          desc: 'Pluggable executor for `ExecutableCode` parts.',
        },
        {
          sig: 'fn stateful(&self) -> bool — default false',
          desc: 'Whether the executor maintains interpreter state across calls (notebook-style). When `true`, an `execution_id` is threaded into each input.',
        },
        {
          sig: 'fn error_retry_attempts(&self) -> u32 — default 2',
          desc: 'Maximum retries allowed per invocation before the failure surfaces as a failed `CodeExecutionResult`.',
        },
        {
          sig: 'fn timeout(&self) -> Option<Duration> — default Some(30s)',
          desc: 'Per-invocation wall-clock timeout.',
        },
      ],
    },
    { kind: 'h2', text: 'Input and result types' },
    {
      kind: 'api',
      entries: [
        {
          sig: 'struct CodeExecutionInput { code: String, language: String, input_files: Vec<ExecFile>, execution_id: Option<String> }',
          desc: 'One execution: the source, a lowercase language id (`"python"`, `"shell"`, ...), files to stage, and a stable id for stateful executors.',
        },
        {
          sig: 'struct CodeExecutionResult { stdout: String, stderr: String, output_files: Vec<ExecFile>, exit_code: Option<i32> }',
          desc: '`exit_code: None` means the executor could not determine one (e.g. the timeout watchdog killed the process). `combined_output()` merges the streams; `is_success()` is `Some(0)`, or `None` with empty stderr.',
        },
        {
          sig: 'struct ExecFile { name: String, content: Vec<u8>, mime_type: Option<String> }',
          desc: 'A file passed to or returned from an executor; `content` serializes as base64.',
        },
        {
          sig: 'enum Outcome { OutcomeUnspecified, OutcomeOk, OutcomeFailed, OutcomeDeadlineExceeded }',
          desc: 'The `genai_types::part` outcome carried on the `CodeExecutionResult` part fed back to the model.',
        },
      ],
    },
    { kind: 'h2', text: 'LocalCodeExecutor' },
    {
      kind: 'p',
      text: 'Spawns a child interpreter via `tokio::process`, writing the code to stdin. Defaults: `python3` with args `["-"]`, a 30-second timeout, 2 retries. On timeout the child is killed and the result carries `exit_code: None` with a "timed out" stderr message.',
    },
    {
      kind: 'callout',
      tone: 'warn',
      title: 'Not a security boundary',
      text: 'The child process runs with the parent\'s user, network, and filesystem. `LocalCodeExecutor` is subprocess isolation only — use `ContainerCodeExecutor` (or a remote sandbox) for untrusted code. See [Security](/docs/security).',
    },
    {
      kind: 'api',
      entries: [
        { sig: 'LocalCodeExecutor::new() -> Self', desc: 'Defaults: `python3 -`, 30s timeout, 2 retries.' },
        { sig: 'with_interpreter(self, interpreter: impl Into<String>) -> Self', desc: 'Swap the binary (`"node"`, `"bash"`, ...).' },
        { sig: 'with_args(self, args: Vec<String>) -> Self', desc: 'Override interpreter args. Keep `-` (or your interpreter\'s stdin flag) so the child reads source from stdin.' },
        { sig: 'with_timeout(self, t: Duration) -> Self', desc: 'Override the per-call wall-clock timeout.' },
      ],
    },
    { kind: 'h2', text: 'ContainerCodeExecutor (code-exec-docker)' },
    {
      kind: 'p',
      text: 'Runs each call in a fresh ephemeral container via the `docker` CLI (no Docker SDK dependency — `docker` just has to be on `$PATH`). The container is locked down by default; loosening any limit is a deliberate act via the typed builders:',
    },
    {
      kind: 'table',
      head: ['Flag', 'Default', 'Override'],
      rows: [
        ['`--network=none`', 'always on', '—'],
        ['`--read-only` rootfs (+ `--tmpfs=/tmp:rw,exec,size=64m`)', 'always on', '—'],
        ['`--rm` (auto-delete on exit)', 'always on', '—'],
        ['`--memory` / `--memory-swap`', '`256m` (swap pinned to the same value)', '`with_memory("1g")`'],
        ['`--cpus`', '`1.0`', '`with_cpus("0.5")`'],
        ['`--pids-limit`', '`128`', '`with_pids_limit(32)`'],
        ['`--user`', '`65534:65534` (nobody, never root)', '`with_user("1000:1000")`'],
        ['`--cap-drop=ALL` + `--security-opt=no-new-privileges`', 'on (`drop_capabilities: true`)', 'field is public; turning it off is for debugging only'],
      ],
    },
    {
      kind: 'p',
      text: 'Additional builders: `ContainerCodeExecutor::new(image)` (default image `python:3.12-slim`, argv `["python3", "-"]`), `with_timeout`, `with_argv`, and `with_extra_args` to splice raw `docker run` arguments before the image. Every container gets an explicit `--name`, so on timeout the daemon-side container is killed with `docker kill` — `kill_on_drop` on the CLI process alone would not stop it. `build_run_args` is public so you can assert the exact policy in tests.',
    },
    { kind: 'h2', text: 'Attaching an executor' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'LlmAgent with a sandboxed executor',
      code: `use adk_rs::code_exec::docker::ContainerCodeExecutor;
use adk_rs::code_exec::local::LocalCodeExecutor;
use std::sync::Arc;
use std::time::Duration;

// Trusted environments: local subprocess.
let local = Arc::new(
    LocalCodeExecutor::new()
        .with_interpreter("/bin/sh")
        .with_args(vec!["-s".into()]),
);

// Untrusted code: locked-down Docker container per call.
let sandbox = Arc::new(
    ContainerCodeExecutor::new("python:3.12-slim")
        .with_timeout(Duration::from_secs(20))
        .with_memory("512m")
        .with_cpus("0.5"),
);

let agent = LlmAgent::builder("coder")
    .model(model)
    .instruction("Solve problems by writing and running code.")
    .code_executor(sandbox)
    .build()?;`,
    },
    { kind: 'hr' },
    {
      kind: 'list',
      items: [
        '[Code agent example](/docs/examples/code-agent) — a runnable end-to-end demo (`cargo run --example code_agent --features "code-exec,testing"`).',
        '[Security](/docs/security) — the crate\'s secure-by-default posture.',
        '[Builtin tools](/docs/builtin-tools) — including `built_in_code_execution_tool` for Gemini\'s built-in code execution.',
      ],
    },
  ],
};

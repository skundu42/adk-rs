import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'eval',
  title: 'Evaluation',
  description:
    'Replay recorded eval sets against any agent and score tool trajectories and final responses with pluggable metrics.',
  srcPath: 'src/eval/runner.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'The `eval` feature replays recorded conversations — eval sets — against any [`BaseAgent`](/docs/agents-overview) and scores the results. Eval sets are plain, portable JSON files, wire-compatible with Python ADK\'s `adk eval` output: files written by the Python tooling load unmodified, including FunctionCall-shaped `tool_uses` with extra fields. Record a conversation once and replay it against any agent build.',
    },
    { kind: 'h2', text: 'Eval-set format' },
    {
      kind: 'p',
      text: 'An `EvalSet` is a list of `EvalCase`s; each case is a `conversation` of `Invocation`s — one user prompt, the expected final response, and the expected intermediate data (tool calls and intermediate model turns). All shapes derive plain serde, so a file is just JSON. Ids hit the wire under their Python ADK names — `eval_set_id` and `eval_id` — with the older adk-rs `id` key accepted on read as a legacy alias:',
    },
    {
      kind: 'code',
      lang: 'json',
      title: 'hello_world.evalset.json',
      code: `{
  "eval_set_id": "hello-world-set",
  "name": "Hello world",
  "creation_timestamp": 0.0,
  "eval_cases": [
    {
      "eval_id": "case-1",
      "name": "greeting with a tool call",
      "session_input": null,
      "conversation": [
        {
          "invocation_id": "inv-1",
          "user_content": {
            "role": "user",
            "parts": [{ "text": "What's the weather in Paris?" }]
          },
          "final_response": {
            "role": "model",
            "parts": [{ "text": "It is sunny in Paris, 22 degrees." }]
          },
          "intermediate_data": {
            "tool_uses": [
              { "name": "get_weather", "args": { "city": "Paris" } }
            ],
            "intermediate_responses": []
          }
        }
      ]
    }
  ]
}`,
    },
    {
      kind: 'table',
      head: ['Type', 'Fields'],
      rows: [
        ['`EvalSet`', '`id` (serializes as `eval_set_id`), `name` (default empty), `description: Option<String>`, `eval_cases`, `creation_timestamp` (seconds, default 0).'],
        ['`EvalCase`', '`id` (serializes as `eval_id`), `conversation: Vec<Invocation>`, `session_input: Option<SessionInput>`, `name: Option<String>`, `creation_timestamp`.'],
        ['`Invocation`', '`user_content: Content`, `final_response: Option<Content>`, `intermediate_data` (default empty), `invocation_id` (default empty), `creation_timestamp`.'],
        ['`IntermediateData`', '`tool_uses: Vec<ToolUse>`, `intermediate_responses: Vec<(String, Vec<Part>)>` — `(author, parts)` pairs, byte-compatible with Python.'],
        ['`SessionInput`', '`app_name`, `user_id`, `state` (initial session state map).'],
        ['`ToolUse`', '`name`, `args` (JSON value). Python stores full `FunctionCall` objects here; extra fields (e.g. `id`) are ignored on read.'],
      ],
    },
    {
      kind: 'callout',
      tone: 'note',
      text: '`session_input.state` seeds the case’s session: the `EvalRunner` creates each case’s session with that fixture state before replaying the conversation. The `app_name` / `user_id` fields are carried for Python compatibility — the runner uses the names from its own constructor.',
    },
    { kind: 'h2', text: 'Loading' },
    {
      kind: 'api',
      entries: [
        { sig: 'fn load_eval_set_from_str(s: &str) -> Result<EvalSet>', desc: 'Parse from a JSON string.' },
        { sig: 'async fn load_eval_set_from_file(path: impl AsRef<Path>) -> Result<EvalSet>', desc: 'Read and parse a JSON file with `tokio::fs`.' },
      ],
    },
    {
      kind: 'p',
      text: 'Both are thin wrappers over serde — `serde_json::from_slice::<EvalSet>(&bytes)` works just as well.',
    },
    { kind: 'h2', text: 'EvalRunner' },
    {
      kind: 'api',
      entries: [
        {
          sig: 'fn new(agent: Arc<dyn BaseAgent>, app_name: impl Into<String>, user_id: impl Into<String>, evaluators: Vec<Arc<dyn Evaluator>>) -> EvalRunner',
          desc: 'Construct with the agent under test, an app name, a user id, and the metric set.',
        },
        {
          sig: 'async fn run_set(&self, set: &EvalSet) -> Result<EvalReport>',
          desc: 'Run every case in order; `EvalReport { results: Vec<EvalResult> }`.',
        },
        {
          sig: 'async fn run_case(&self, set_id: &str, case: &EvalCase) -> Result<EvalResult>',
          desc: 'Run one case. Each case gets a fresh in-memory session; the conversation’s user messages replay in order against the same session.',
        },
      ],
    },
    {
      kind: 'p',
      text: 'For each invocation the runner drives the agent directly (no `Runner`), collects every `FunctionCall` part into actual `tool_uses`, records non-final content as `(author, parts)` pairs in `intermediate_responses`, and takes the event where `is_final_response()` holds as the actual `final_response`. Each evaluator scores every invocation; the reported per-evaluator `score` is the average across the case’s invocations, with the per-invocation scores riding in `details.per_invocation`. A metric’s `status` is the AND of its per-invocation statuses (`Error` dominating), and any non-`PASSED` metric flips the case’s `overall_status` to `FAILED`.',
    },
    { kind: 'h2', text: 'Metrics' },
    {
      kind: 'p',
      text: 'Metrics implement the `Evaluator` trait — `name(&self) -> &str` (the key in the scores map) and `async fn evaluate(&self, expected: &Invocation, actual: &Invocation) -> Result<EvalScore>`. Two ship in the box:',
    },
    {
      kind: 'table',
      head: ['Metric', 'Key', 'Algorithm'],
      rows: [
        [
          '`TrajectoryMatch::new(threshold)`',
          '`tool_trajectory_avg_score`',
          'In-order exact matching of `(tool name, args)` pairs from `intermediate_data.tool_uses`. Score = matched / max(expected len, actual len, 1). Default threshold 1.0 = exact trajectory.',
        ],
        [
          '`ResponseMatch::new(threshold)`',
          '`final_response_match_v1`',
          'Case-insensitive, whole-token unigram overlap: the fraction of expected tokens that appear as whole tokens in the actual response text — an expected token `cat` does not match inside `concatenate`. A deliberately rough, Rouge-like metric (not a true Rouge-L); an empty expected response scores 1.0. Default threshold 0.8.',
        ],
      ],
    },
    {
      kind: 'p',
      text: 'Scores are `EvalScore { score: f64, status: EvalStatus, details: Value }` with `status` derived from `score >= threshold`. `EvalStatus` serializes uppercase: `PASSED`, `FAILED`, or `ERROR`. An `EvalResult` carries `eval_set_id`, `eval_case_id`, the per-evaluator `scores` map (each score the invocation average described above), and `overall_status` (the logical AND of every metric across every invocation in the case).',
    },
    { kind: 'h2', text: 'Running evals' },
    {
      kind: 'code',
      lang: 'bash',
      title: 'Via the embedded CLI',
      code: `my-app eval --agent greeter --set samples/hello_world.evalset.json
# prints the EvalReport as pretty JSON`,
    },
    {
      kind: 'p',
      text: 'The [CLI](/docs/cli) `eval` subcommand uses `TrajectoryMatch::new(1.0)` and `ResponseMatch::new(0.5)`. Programmatically you pick your own metrics and thresholds:',
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Programmatic eval run',
      code: `use adk_rs::eval::{EvalRunner, EvalSet, ResponseMatch, TrajectoryMatch};
use std::sync::Arc;

let bytes = tokio::fs::read("hello_world.evalset.json").await?;
let set: EvalSet = serde_json::from_slice(&bytes)?;

let runner = EvalRunner::new(
    agent,                 // Arc<dyn BaseAgent>
    "hello_world",
    "eval-user",
    vec![
        Arc::new(TrajectoryMatch::new(1.0)),
        Arc::new(ResponseMatch::new(0.5)),
    ],
);
let report = runner.run_set(&set).await?;
for r in &report.results {
    println!("{} -> {:?}", r.eval_case_id, r.overall_status);
}`,
    },
    {
      kind: 'callout',
      tone: 'tip',
      text: 'Pair evals with [`MockModel`](/docs/testing) in CI to regression-test agent logic without burning provider quota, and run the same sets against real providers before release.',
    },
    { kind: 'hr' },
    {
      kind: 'list',
      items: [
        '[Embedded CLI](/docs/cli) — the `eval` subcommand.',
        '[Testing agents](/docs/testing) — unit-level testing with mocks.',
        '[Events](/docs/events) — `is_final_response` and the event shapes the runner inspects.',
      ],
    },
  ],
};

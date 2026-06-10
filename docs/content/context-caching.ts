import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'context-caching',
  title: 'Context caching',
  description:
    'Cache the stable request prefix server-side with ContextCacheConfig and a static instruction to cut cost and latency.',
  srcPath: 'src/core/cache.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'Agents with large instructions or tool sets resend the same prefix on every LLM call. With a `ContextCacheConfig` attached, cache-capable providers reuse that stable prefix — system instruction plus tool declarations — server-side instead of reprocessing it, cutting both token cost and latency. Gemini creates an explicit `cachedContents` entry; Anthropic gets a `cache_control` breakpoint on the same prefix; OpenAI caches automatically with nothing to configure.',
    },
    { kind: 'h2', text: 'ContextCacheConfig' },
    {
      kind: 'api',
      entries: [
        {
          sig: 'ContextCacheConfig { cache_intervals: u32, ttl_seconds: u64, min_tokens: u64 }',
          desc: 'Configuration for explicit provider-side context caching. Implements `Default`.',
        },
        {
          sig: 'cache_intervals: u32 — default 10',
          desc: 'Maximum number of LLM calls served by one cache entry before it is refreshed; guards against unbounded staleness.',
        },
        {
          sig: 'ttl_seconds: u64 — default 1800',
          desc: 'Cache-entry time-to-live, in seconds.',
        },
        {
          sig: 'min_tokens: u64 — default 0',
          desc: 'Minimum *estimated* token size of the cacheable prefix. Smaller prefixes are sent inline — caching tiny prefixes costs more than it saves, and Gemini also enforces a server-side minimum.',
        },
      ],
    },
    { kind: 'h2', text: 'Wiring it up' },
    {
      kind: 'p',
      text: 'Set the config once on the runner — it is copied into every invocation\'s `RunConfig` — or per invocation via `RunConfig::context_cache_config`, which overrides the app-level value. The agent stamps the config onto each `LlmRequest` it builds (`cache_config`), and the provider takes it from there.',
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Runner-level config',
      code: `use adk_rs::core::ContextCacheConfig;

let runner = Runner::builder()
    .app_name("support")
    .agent(agent)
    .session_service(svc)
    .context_cache_config(ContextCacheConfig {
        cache_intervals: 10,
        ttl_seconds: 1800,
        min_tokens: 2048,
    })
    .build()?;`,
    },
    { kind: 'h2', text: 'Pair with static_instruction' },
    {
      kind: 'p',
      text: 'Caching only pays off if the prefix is **byte-identical across turns**. A regular `.instruction(...)` is templated against session state (`{key}` substitution) and re-resolved every turn, so any change — a new state value, a dynamic provider — produces a different system instruction and a cache miss. `LlmAgent::static_instruction` exists for exactly this: it is sent verbatim, never templated, never re-evaluated, at the very start of the system instruction. When a static instruction is present, the dynamic `instruction` is moved out of the system prompt and appended to the request *contents* (after the user turn), so the cached prefix stays stable.',
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Cache-stable prefix + dynamic remainder',
      code: `let agent = LlmAgent::builder("support")
    .model(Arc::new(Gemini::from_env("gemini-2.5-flash")?))
    // Large, stable: policies, product docs, few-shot examples.
    .static_instruction(POLICY_HANDBOOK)
    // Small, per-turn: templated from session state, rides in contents.
    .instruction("The current user's plan is {plan}.")
    .build()?;`,
    },
    {
      kind: 'api',
      entries: [
        {
          sig: 'LlmAgentBuilder::static_instruction(self, s: impl Into<String>) -> Self',
          desc: 'Cache-stable instruction prefix as text.',
        },
        {
          sig: 'LlmAgentBuilder::static_instruction_content(self, c: Content) -> Self',
          desc: 'Same, but accepts arbitrary `Content` (e.g. multimodal parts).',
        },
        {
          sig: 'RunnerBuilder::context_cache_config(self, cfg: ContextCacheConfig) -> Self',
          desc: 'Enable explicit caching for every invocation; per-invocation `RunConfig` overrides it.',
        },
      ],
    },
    { kind: 'h2', text: 'What the Gemini provider does' },
    {
      kind: 'list',
      items: [
        'Computes a **fingerprint** of the cacheable prefix: model + system instruction + tool declarations. Any change invalidates the entry.',
        'Estimates prefix size (characters / 4) and skips caching below `min_tokens`, or when there is no system instruction and no tools.',
        'Creates an entry via `POST /cachedContents` and tracks it in-process with its expiry and use count; subsequent requests reference the entry with `cachedContent` and **omit** the cached fields from the request body.',
        'Refreshes the entry after `cache_intervals` uses or when the TTL lapses.',
        'On a creation failure, disables caching for that prefix for one TTL and logs a warning — caching is an optimization, never a source of run failures.',
      ],
    },
    { kind: 'h2', text: 'What the Anthropic provider does' },
    {
      kind: 'p',
      text: 'Anthropic\'s prompt cache is server-managed, so there is no entry to create: the same `ContextCacheConfig` instead becomes a `cache_control` breakpoint on the stable prefix. The breakpoint lands on the system block (tools render before system on Anthropic\'s side, so one marker caches both); with no system instruction it lands on the last tool declaration. A `ttl_seconds` of 3600 or more selects the 1-hour cache tier, anything less the default 5-minute tier. `cache_intervals` and `min_tokens` are Gemini-specific and ignored — Anthropic enforces its own server-side minimums. Cache activity is reported per response via `cache_metadata` (`cache_hit`) and `usage_metadata.cached_content_token_count`. Pair with [`static_instruction`](/docs/llm-agent) exactly as for Gemini: the breakpoint only pays off when the prefix is byte-identical across turns.',
    },
    {
      kind: 'callout',
      tone: 'note',
      text: 'Gemini and Anthropic both honour `cache_config` (explicit `cachedContents` entries and `cache_control` breakpoints respectively). The OpenAI provider ignores it — OpenAI\'s prompt caching is automatic server-side, with nothing to configure. The configuration is harmless to leave in place when you swap models.',
    },
    { kind: 'h2', text: 'Observability: CacheMetadata' },
    {
      kind: 'p',
      text: 'Cache-capable providers attach a `CacheMetadata` to each `LlmResponse`, which surfaces on events as `event.response.cache_metadata`:',
    },
    {
      kind: 'api',
      entries: [
        {
          sig: 'struct CacheMetadata { cache_name: String, cache_hit: bool }',
          desc: '`cache_name` is the provider-side resource (e.g. `cachedContents/abc123`); `cache_hit` is `true` when the response was served against an existing entry, `false` when the entry was created for this call.',
        },
      ],
    },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Watching cache behaviour',
      code: `let mut events = runner.run("user", Some(&session_id), "next question").await?;
while let Some(event) = events.next().await {
    let event = event?;
    if let Some(meta) = &event.response.cache_metadata {
        println!("cache {} hit={}", meta.cache_name, meta.cache_hit);
    }
}`,
    },
    { kind: 'hr' },
    {
      kind: 'list',
      items: [
        '[LlmAgent](/docs/llm-agent) — instructions, static instructions, and templating.',
        '[Providers](/docs/providers) — provider capabilities and configuration.',
        '[Event compaction](/docs/event-compaction) — the complementary lever for long *histories* rather than long prefixes.',
      ],
    },
  ],
};

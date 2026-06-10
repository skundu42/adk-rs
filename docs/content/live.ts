import type { DocPage } from '@/lib/types';

export const page: DocPage = {
  slug: 'live',
  title: 'Gemini Live',
  description:
    'Bidirectional WebSocket streaming with the Gemini Live API: realtime text and audio in, text, PCM audio, transcriptions, and tool calls out.',
  srcPath: 'src/providers/gemini/live.rs',
  blocks: [
    {
      kind: 'lede',
      text: 'SSE streaming is one-directional: a request goes up, chunks come down. The Live API is full duplex — `Gemini::connect_live` (feature `live`) opens a `BidiGenerateContent` WebSocket where you push text turns and realtime microphone audio while the model simultaneously streams back text, PCM audio, transcriptions, and tool calls. Server-side voice activity detection means the user can interrupt the model mid-sentence, surfaced to you as `LiveEvent::Interrupted`.',
    },
    { kind: 'h2', text: 'Enabling' },
    {
      kind: 'code',
      lang: 'toml',
      title: 'Cargo.toml',
      code: `[dependencies]
adk-rs = { version = "0.5", features = ["live"] }  # implies "gemini"`,
    },
    {
      kind: 'p',
      text: 'The connection uses the same `GeminiConfig` as the HTTP client — base URL, API version, and key — and the same transport-security policy: `wss://` always, plain `ws://` only to loopback hosts (for local mocks). Use a Live-capable model such as `gemini-2.5-flash-native-audio-preview` for audio, or any flash model for text-mode sessions.',
    },
    { kind: 'h2', text: 'Connecting' },
    {
      kind: 'api',
      entries: [
        {
          sig: 'Gemini::connect_live(&self, cfg: LiveConfig) -> Result<LiveSession>',
          desc: 'Open the WebSocket, send the setup message, and await the server\'s `setupComplete` acknowledgement.',
        },
        {
          sig: 'struct LiveConfig { response_modalities, system_instruction, tools, voice, input_audio_transcription, output_audio_transcription }',
          desc: 'Session setup. `response_modalities` is `["TEXT"]` by default (one modality per session — use `["AUDIO"]` for speech). `voice` picks a prebuilt voice (e.g. "Kore", "Puck"). The transcription flags ask the server to transcribe input and output audio.',
        },
      ],
    },
    { kind: 'h2', text: 'The session surface' },
    {
      kind: 'api',
      entries: [
        {
          sig: 'LiveSession::send_text(&mut self, text: &str, turn_complete: bool) -> Result<()>',
          desc: 'Send a text turn. With `turn_complete: true` the model starts responding immediately.',
        },
        {
          sig: 'LiveSession::send_audio(&mut self, pcm: &[u8], mime_type: &str) -> Result<()>',
          desc: 'Stream a chunk of realtime input audio (typically 16kHz 16-bit PCM, `"audio/pcm;rate=16000"`). Server-side VAD segments turns and may interrupt an in-flight response.',
        },
        {
          sig: 'LiveSession::send_audio_stream_end(&mut self) -> Result<()>',
          desc: 'Signal the input audio stream ended (e.g. microphone muted).',
        },
        {
          sig: 'LiveSession::send_tool_response(&mut self, responses: Vec<FunctionResponse>) -> Result<()>',
          desc: 'Answer a `LiveEvent::ToolCall`.',
        },
        {
          sig: 'LiveSession::recv(&mut self) -> Result<Option<LiveEvent>>',
          desc: 'Next event, or `None` once the server closes the session.',
        },
        {
          sig: 'LiveSession::close(self) -> Result<()>',
          desc: 'Close the WebSocket cleanly.',
        },
      ],
    },
    { kind: 'h2', text: 'LiveEvent' },
    {
      kind: 'table',
      head: ['Event', 'Meaning'],
      rows: [
        ['`Text(String)`', 'Incremental model text.'],
        [
          '`Audio { data, mime_type }`',
          'Model audio chunk, base64-decoded for you (typically 24kHz 16-bit PCM).',
        ],
        ['`InputTranscription(String)`', 'Transcript of the user\'s audio (when requested).'],
        ['`OutputTranscription(String)`', 'Transcript of the model\'s audio (when requested).'],
        ['`ToolCall(Vec<FunctionCall>)`', 'Tool execution requested — answer with `send_tool_response`.'],
        ['`ToolCallCancellation(Vec<String>)`', 'Previously-issued calls cancelled by id (after an interruption).'],
        ['`Interrupted`', 'User barge-in: stop local audio playback immediately.'],
        ['`GenerationComplete`', 'The model finished generating the current response.'],
        ['`TurnComplete`', 'The turn is over; the session is ready for new input.'],
        ['`GoAway { time_left }`', 'The server will close the connection soon — wind down or reconnect.'],
        ['`UsageMetadata(UsageMetadata)`', 'Token usage for the session so far.'],
      ],
    },
    { kind: 'h2', text: 'End-to-end example' },
    {
      kind: 'code',
      lang: 'rust',
      title: 'Voice session with transcription',
      code: `use adk_rs::providers::gemini::{Gemini, LiveConfig, LiveEvent};

#[tokio::main]
async fn main() -> adk_rs::Result<()> {
    let gemini = Gemini::from_env("gemini-2.5-flash-native-audio-preview")?;
    let mut session = gemini
        .connect_live(LiveConfig {
            response_modalities: vec!["AUDIO".into()],
            voice: Some("Kore".into()),
            output_audio_transcription: true,
            ..LiveConfig::default()
        })
        .await?;

    session.send_text("Tell me a joke", true).await?;
    // or stream microphone input as it arrives:
    // session.send_audio(&pcm_chunk, "audio/pcm;rate=16000").await?;

    while let Some(event) = session.recv().await? {
        match event {
            LiveEvent::Audio { data, .. } => { /* queue PCM for playback */ }
            LiveEvent::OutputTranscription(t) => print!("{t}"),
            LiveEvent::Interrupted => { /* flush the playback queue */ }
            LiveEvent::TurnComplete => break,
            _ => {}
        }
    }
    session.close().await?;
    Ok(())
}`,
    },
    {
      kind: 'callout',
      tone: 'note',
      text: 'Live sessions are a model-level surface: they sit alongside `generate_content` / `stream_generate_content`, not inside the `Runner` event loop. Driving a full agent (sessions, tools dispatched by the runner, event log) over a live connection is planned; today you dispatch `LiveEvent::ToolCall` yourself with `send_tool_response`.',
    },
    {
      kind: 'callout',
      tone: 'tip',
      text: 'Sessions are time-limited server-side. Watch for `GoAway` and reconnect proactively — `connect_live` is cheap to call again, and your application owns the conversation state to replay.',
    },
    { kind: 'hr' },
    {
      kind: 'list',
      items: [
        '[Providers](/docs/providers) — the Gemini HTTP client and shared configuration.',
        '[Streaming](/docs/streaming) — one-directional SSE streaming through the runner.',
        '[Function tools](/docs/function-tools) — building the tools you answer `ToolCall` events with.',
      ],
    },
  ],
};

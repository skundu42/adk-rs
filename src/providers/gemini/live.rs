//! Gemini Live API: bidirectional WebSocket streaming.
//!
//! [`Gemini::connect_live`] opens a `BidiGenerateContent` WebSocket session.
//! You push text turns and realtime audio in; the model streams text, audio
//! (PCM), transcriptions, and tool calls back as [`LiveEvent`]s — full
//! duplex, with server-side interruption (barge-in) surfaced as
//! [`LiveEvent::Interrupted`].
//!
//! ```no_run
//! # async fn demo() -> adk_rs::Result<()> {
//! use adk_rs::providers::gemini::{Gemini, LiveConfig, LiveEvent};
//!
//! let gemini = Gemini::from_env("gemini-2.5-flash-native-audio-preview")?;
//! let mut session = gemini
//!     .connect_live(LiveConfig {
//!         response_modalities: vec!["AUDIO".into()],
//!         output_audio_transcription: true,
//!         ..LiveConfig::default()
//!     })
//!     .await?;
//!
//! session.send_text("Tell me a joke", true).await?;
//! while let Some(event) = session.recv().await? {
//!     match event {
//!         LiveEvent::Audio { data, .. } => { /* play PCM */ }
//!         LiveEvent::OutputTranscription(t) => print!("{t}"),
//!         LiveEvent::TurnComplete => break,
//!         _ => {}
//!     }
//! }
//! session.close().await?;
//! # Ok(())
//! # }
//! ```

use std::collections::VecDeque;

use base64::Engine as _;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, warn};

use crate::core::Model as _;
use crate::error::{Error, ProviderError, Result};
use crate::genai_types::{Content, FunctionCall, FunctionResponse, Tool, UsageMetadata};
use crate::providers::gemini::Gemini;

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Configuration for a live session.
#[derive(Debug, Clone)]
pub struct LiveConfig {
    /// Modalities the model may respond with: `"TEXT"` and/or `"AUDIO"`
    /// (default: `["TEXT"]`). The Live API allows one response modality per
    /// session.
    pub response_modalities: Vec<String>,
    /// System instruction, sent once in the setup message.
    pub system_instruction: Option<Content>,
    /// Tool declarations; calls arrive as [`LiveEvent::ToolCall`] and are
    /// answered with [`LiveSession::send_tool_response`].
    pub tools: Vec<Tool>,
    /// Prebuilt voice name for audio output (e.g. `"Kore"`, `"Puck"`).
    pub voice: Option<String>,
    /// Ask the server to transcribe *input* audio
    /// ([`LiveEvent::InputTranscription`]).
    pub input_audio_transcription: bool,
    /// Ask the server to transcribe *output* audio
    /// ([`LiveEvent::OutputTranscription`]).
    pub output_audio_transcription: bool,
}

impl Default for LiveConfig {
    fn default() -> Self {
        Self {
            response_modalities: vec!["TEXT".into()],
            system_instruction: None,
            tools: vec![],
            voice: None,
            input_audio_transcription: false,
            output_audio_transcription: false,
        }
    }
}

/// One server event on a live session.
#[derive(Debug, Clone)]
pub enum LiveEvent {
    /// Incremental model text.
    Text(String),
    /// Model audio chunk (raw bytes, base64-decoded; typically 24kHz 16-bit
    /// PCM — see `mime_type`).
    Audio {
        /// Decoded audio bytes.
        data: Vec<u8>,
        /// MIME type as sent by the server (e.g. `audio/pcm;rate=24000`).
        mime_type: String,
    },
    /// Transcription of the user's input audio.
    InputTranscription(String),
    /// Transcription of the model's output audio.
    OutputTranscription(String),
    /// The model requests tool execution; answer with
    /// [`LiveSession::send_tool_response`].
    ToolCall(Vec<FunctionCall>),
    /// Previously-issued tool calls were cancelled (by id).
    ToolCallCancellation(Vec<String>),
    /// Generation was interrupted by user activity (barge-in).
    Interrupted,
    /// The model finished generating the current response.
    GenerationComplete,
    /// The model's turn is complete; the session is ready for input.
    TurnComplete,
    /// The server will close the connection soon.
    GoAway {
        /// Remaining time before disconnect, as reported by the server
        /// (e.g. `"10s"`).
        time_left: Option<String>,
    },
    /// Token usage for the session so far.
    UsageMetadata(UsageMetadata),
}

/// A live, bidirectional session. Created by [`Gemini::connect_live`].
#[derive(Debug)]
pub struct LiveSession {
    ws: WsStream,
    pending: VecDeque<LiveEvent>,
}

impl Gemini {
    /// Open a Live API session over WebSocket and complete the setup
    /// handshake. The base-URL security policy carries over from
    /// [`Gemini::new`]: `wss://` always, plain `ws://` only to loopback.
    pub async fn connect_live(&self, cfg: LiveConfig) -> Result<LiveSession> {
        let gcfg = self.config();
        if gcfg.api_key.is_empty() {
            return Err(Error::Provider(ProviderError::Auth(
                "Gemini api_key is empty; set $GOOGLE_API_KEY".into(),
            )));
        }
        let base = gcfg.base_url.trim_end_matches('/');
        let ws_base = if let Some(rest) = base.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = base.strip_prefix("http://") {
            // `Gemini::new` only accepts plain http for loopback hosts.
            format!("ws://{rest}")
        } else {
            return Err(Error::config(format!("unsupported base_url: {base}")));
        };
        let url = format!(
            "{ws_base}/ws/google.ai.generativelanguage.{}.GenerativeService.BidiGenerateContent?key={}",
            gcfg.api_version, gcfg.api_key,
        );

        let (ws, _) = tokio_tungstenite::connect_async(&url)
            .await
            .map_err(|e| ProviderError::Transport(format!("live connect: {e}")))?;
        let mut session = LiveSession {
            ws,
            pending: VecDeque::new(),
        };

        let mut generation_config = json!({ "responseModalities": cfg.response_modalities });
        if let Some(voice) = &cfg.voice {
            generation_config["speechConfig"] =
                json!({ "voiceConfig": { "prebuiltVoiceConfig": { "voiceName": voice } } });
        }
        let mut setup = json!({
            "model": format!("models/{}", self.name()),
            "generationConfig": generation_config,
        });
        if let Some(sys) = &cfg.system_instruction {
            setup["systemInstruction"] = serde_json::to_value(sys)?;
        }
        if !cfg.tools.is_empty() {
            setup["tools"] = serde_json::to_value(&cfg.tools)?;
        }
        if cfg.input_audio_transcription {
            setup["inputAudioTranscription"] = json!({});
        }
        if cfg.output_audio_transcription {
            setup["outputAudioTranscription"] = json!({});
        }
        session.send_json(&json!({ "setup": setup })).await?;

        // The first server message must acknowledge the setup.
        match session.next_message().await? {
            Some(v) if v.get("setupComplete").is_some() => Ok(session),
            Some(v) => Err(Error::Provider(ProviderError::Stream(format!(
                "expected setupComplete, got: {v}"
            )))),
            None => Err(Error::Provider(ProviderError::Stream(
                "connection closed before setupComplete".into(),
            ))),
        }
    }
}

impl LiveSession {
    async fn send_json(&mut self, v: &Value) -> Result<()> {
        self.ws
            .send(Message::Text(v.to_string().into()))
            .await
            .map_err(|e| Error::Provider(ProviderError::Transport(format!("live send: {e}"))))
    }

    /// Send a text turn. With `turn_complete`, the model starts responding
    /// immediately; without it, the text is buffered as incremental context.
    pub async fn send_text(&mut self, text: &str, turn_complete: bool) -> Result<()> {
        self.send_json(&json!({
            "clientContent": {
                "turns": [{ "role": "user", "parts": [{ "text": text }] }],
                "turnComplete": turn_complete,
            }
        }))
        .await
    }

    /// Stream a chunk of realtime input audio (e.g. 16kHz 16-bit PCM with
    /// `mime_type` `"audio/pcm;rate=16000"`). The server runs voice activity
    /// detection and may interrupt an in-flight response
    /// ([`LiveEvent::Interrupted`]).
    pub async fn send_audio(&mut self, pcm: &[u8], mime_type: &str) -> Result<()> {
        let data = base64::engine::general_purpose::STANDARD.encode(pcm);
        self.send_json(&json!({
            "realtimeInput": { "audio": { "data": data, "mimeType": mime_type } }
        }))
        .await
    }

    /// Signal that the audio input stream has ended (e.g. microphone off).
    pub async fn send_audio_stream_end(&mut self) -> Result<()> {
        self.send_json(&json!({ "realtimeInput": { "audioStreamEnd": true } }))
            .await
    }

    /// Answer a [`LiveEvent::ToolCall`].
    pub async fn send_tool_response(&mut self, responses: Vec<FunctionResponse>) -> Result<()> {
        self.send_json(&json!({
            "toolResponse": { "functionResponses": serde_json::to_value(&responses)? }
        }))
        .await
    }

    /// Receive the next event, or `None` once the server closes the session.
    pub async fn recv(&mut self) -> Result<Option<LiveEvent>> {
        loop {
            if let Some(ev) = self.pending.pop_front() {
                return Ok(Some(ev));
            }
            match self.next_message().await? {
                Some(v) => self.ingest(&v),
                None => return Ok(None),
            }
        }
    }

    /// Close the session cleanly.
    pub async fn close(mut self) -> Result<()> {
        self.ws
            .close(None)
            .await
            .map_err(|e| Error::Provider(ProviderError::Transport(format!("live close: {e}"))))
    }

    /// Read one JSON message from the socket (text or binary frame), or
    /// `None` on close.
    async fn next_message(&mut self) -> Result<Option<Value>> {
        loop {
            let Some(msg) = self.ws.next().await else {
                return Ok(None);
            };
            let msg =
                msg.map_err(|e| Error::Provider(ProviderError::Transport(format!("live: {e}"))))?;
            let bytes = match msg {
                Message::Text(t) => t.as_bytes().to_vec(),
                Message::Binary(b) => b.to_vec(),
                Message::Close(_) => return Ok(None),
                // Ping/pong handled by the protocol layer.
                _ => continue,
            };
            let v: Value = serde_json::from_slice(&bytes)
                .map_err(|e| ProviderError::Decode(format!("live message: {e}")))?;
            return Ok(Some(v));
        }
    }

    /// Decompose one server message into events.
    fn ingest(&mut self, v: &Value) {
        if let Some(sc) = v.get("serverContent") {
            if sc.get("interrupted").and_then(Value::as_bool) == Some(true) {
                self.pending.push_back(LiveEvent::Interrupted);
            }
            if let Some(t) = sc
                .get("inputTranscription")
                .and_then(|t| t.get("text"))
                .and_then(Value::as_str)
            {
                self.pending
                    .push_back(LiveEvent::InputTranscription(t.to_string()));
            }
            if let Some(t) = sc
                .get("outputTranscription")
                .and_then(|t| t.get("text"))
                .and_then(Value::as_str)
            {
                self.pending
                    .push_back(LiveEvent::OutputTranscription(t.to_string()));
            }
            if let Some(parts) = sc
                .get("modelTurn")
                .and_then(|mt| mt.get("parts"))
                .and_then(Value::as_array)
            {
                for p in parts {
                    if let Some(t) = p.get("text").and_then(Value::as_str) {
                        self.pending.push_back(LiveEvent::Text(t.to_string()));
                    }
                    if let Some(inline) = p.get("inlineData") {
                        let mime_type = inline
                            .get("mimeType")
                            .and_then(Value::as_str)
                            .unwrap_or("audio/pcm")
                            .to_string();
                        let data = inline
                            .get("data")
                            .and_then(Value::as_str)
                            .and_then(|d| base64::engine::general_purpose::STANDARD.decode(d).ok())
                            .unwrap_or_default();
                        self.pending.push_back(LiveEvent::Audio { data, mime_type });
                    }
                }
            }
            if sc.get("generationComplete").and_then(Value::as_bool) == Some(true) {
                self.pending.push_back(LiveEvent::GenerationComplete);
            }
            if sc.get("turnComplete").and_then(Value::as_bool) == Some(true) {
                self.pending.push_back(LiveEvent::TurnComplete);
            }
        } else if let Some(tc) = v.get("toolCall") {
            let calls: Vec<FunctionCall> = tc
                .get("functionCalls")
                .map(|fc| serde_json::from_value(fc.clone()).unwrap_or_default())
                .unwrap_or_default();
            if !calls.is_empty() {
                self.pending.push_back(LiveEvent::ToolCall(calls));
            }
        } else if let Some(tcc) = v.get("toolCallCancellation") {
            let ids: Vec<String> = tcc
                .get("ids")
                .map(|ids| serde_json::from_value(ids.clone()).unwrap_or_default())
                .unwrap_or_default();
            self.pending.push_back(LiveEvent::ToolCallCancellation(ids));
        } else if let Some(ga) = v.get("goAway") {
            self.pending.push_back(LiveEvent::GoAway {
                time_left: ga
                    .get("timeLeft")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            });
        } else if let Some(um) = v.get("usageMetadata") {
            match serde_json::from_value::<UsageMetadata>(um.clone()) {
                Ok(usage) => self.pending.push_back(LiveEvent::UsageMetadata(usage)),
                Err(e) => warn!("live usageMetadata decode failed: {e}"),
            }
        } else {
            debug!("ignoring unknown live message: {v}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::gemini::GeminiConfig;
    use tokio::net::TcpListener;

    /// Minimal scripted Live server: handshake, then one text turn.
    async fn spawn_mock_live_server() -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();

            // 1. setup → setupComplete
            let setup = ws.next().await.unwrap().unwrap();
            let setup: Value = serde_json::from_slice(&setup.into_data()).unwrap();
            assert!(
                setup["setup"]["model"]
                    .as_str()
                    .unwrap()
                    .starts_with("models/")
            );
            assert_eq!(
                setup["setup"]["generationConfig"]["responseModalities"][0],
                "TEXT"
            );
            ws.send(Message::Text(
                json!({"setupComplete": {}}).to_string().into(),
            ))
            .await
            .unwrap();

            // 2. clientContent → two serverContent frames then turnComplete
            let turn = ws.next().await.unwrap().unwrap();
            let turn: Value = serde_json::from_slice(&turn.into_data()).unwrap();
            assert_eq!(
                turn["clientContent"]["turns"][0]["parts"][0]["text"],
                "hello"
            );
            ws.send(Message::Text(
                json!({"serverContent": {"modelTurn": {"parts": [{"text": "hi "}]}}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
            // Binary frame on purpose: the real server mixes frame types.
            ws.send(Message::Binary(
                json!({"serverContent": {
                    "modelTurn": {"parts": [{"text": "there"}]},
                    "turnComplete": true
                }})
                .to_string()
                .into_bytes()
                .into(),
            ))
            .await
            .unwrap();
            let _ = ws.close(None).await;
        });
        addr
    }

    #[tokio::test]
    async fn live_handshake_text_roundtrip() {
        let addr = spawn_mock_live_server().await;
        let g = Gemini::new(
            "gemini-2.5-flash",
            GeminiConfig {
                base_url: format!("http://{addr}"),
                api_key: "k".into(),
                ..GeminiConfig::default()
            },
        )
        .unwrap();

        let mut session = g.connect_live(LiveConfig::default()).await.unwrap();
        session.send_text("hello", true).await.unwrap();

        let mut text = String::new();
        let mut turn_complete = false;
        while let Some(ev) = session.recv().await.unwrap() {
            match ev {
                LiveEvent::Text(t) => text.push_str(&t),
                LiveEvent::TurnComplete => {
                    turn_complete = true;
                    break;
                }
                other => panic!("unexpected event: {other:?}"),
            }
        }
        assert_eq!(text, "hi there");
        assert!(turn_complete);
    }

    #[tokio::test]
    async fn refuses_empty_api_key() {
        let g = Gemini::new(
            "gemini-2.5-flash",
            GeminiConfig {
                base_url: "http://127.0.0.1:1".into(),
                ..GeminiConfig::default()
            },
        )
        .unwrap();
        let err = g.connect_live(LiveConfig::default()).await.unwrap_err();
        assert!(err.to_string().contains("api_key"));
    }
}

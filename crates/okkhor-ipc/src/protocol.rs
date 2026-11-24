#![forbid(unsafe_code)]
//! IPC protocol v1: pipe name, limits, and JSON message shapes (contract §4).
//!
//! Every connection sends exactly one `hello` as its first request. Requests
//! are an externally tagged enum, one per frame:
//! `{"settings.update": {...}}`. Golden fixtures in the tests below pin every
//! request shape.

use serde::{Deserialize, Serialize};

use crate::error::ErrorCode;
use crate::settings::{Settings, SettingsPatch};
use crate::words::{Word, WordLanguage};

/// Protocol version. Mismatches are a typed error, not a silent fallback.
pub const PROTOCOL_VERSION: u16 = 1;
/// Maximum frame payload in bytes (contract §4).
pub const MAX_FRAME_BYTES: usize = 1_048_576;
/// Maximum concurrent clients (contract §4).
pub const MAX_CLIENTS: usize = 4;
/// Named-pipe path (contract §4).
pub const PIPE_NAME: &str = r"\\.\pipe\okkhor-ipc-v1";
/// Maximum time to wait for one inbound frame.
pub const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// Maximum time to produce one response.
pub const REQUEST_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);
/// `text.insert` cap in UTF-16 code units (contract §4).
pub const MAX_INSERT_UTF16: usize = 512;
/// `words.list` page cap (contract §4).
pub const MAX_WORDS_LIST_LIMIT: u64 = 200;
/// `ai.complete` prompt and system cap in Unicode scalar values (ADR 0018 §5).
pub const MAX_AI_PROMPT_CHARS: usize = 20_000;
/// `ai.complete` model id cap in bytes (ADR 0018 §5).
pub const MAX_AI_MODEL_BYTES: usize = 128;
/// `ai.complete` `max_tokens` cap (ADR 0018 §5).
pub const MAX_AI_TOKENS: u32 = 8_192;
/// `ai.complete` `timeout_ms` floor (ADR 0018 §5).
pub const MIN_AI_TIMEOUT_MS: u64 = 1_000;
/// `ai.complete` `timeout_ms` cap (ADR 0018 §5).
pub const MAX_AI_TIMEOUT_MS: u64 = 60_000;

/// First request on every connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u16,
    pub token: String,
}

/// Server greeting after a successful handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerHello {
    pub protocol: u16,
    pub daemon_version: String,
    pub pid: u32,
}

/// Empty `{}` params for argument-free methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NoArgs {}

/// `status` result (contract §4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub daemon_version: String,
    pub paused: bool,
    pub language: String,
    pub layout: String,
    pub input_mode: crate::settings::InputMode,
    pub output_encoding: crate::settings::OutputEncoding,
    pub popup_visible: bool,
    pub words_total: u64,
    pub learning_enabled: bool,
    pub autostart: bool,
}

/// `layouts.list` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutInfo {
    pub code: String,
    pub name: String,
}

/// `words.list` params.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WordsListQuery {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub offset: u64,
    #[serde(default = "default_page_limit")]
    pub limit: u64,
}

fn default_page_limit() -> u64 {
    50
}

/// `words.list` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordsPage {
    pub total: u64,
    pub words: Vec<Word>,
}

/// `words.add` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordAdd {
    pub roman: String,
    pub output: String,
    pub language: WordLanguage,
}

/// `words.remove` params and result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordRemove {
    pub id: u64,
}

/// CSV or JSON payload selector for `words.import` / `words.export`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportFormat {
    Csv,
    Json,
}

/// `words.import` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordsImport {
    pub format: ImportFormat,
    pub data: String,
}

/// `words.import` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordsImported {
    pub imported: usize,
    pub skipped: usize,
}

/// `words.export` params and result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordsExport {
    pub format: ImportFormat,
}

/// `words.export` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordsExported {
    pub format: ImportFormat,
    pub data: String,
}

/// `text.insert` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextInsert {
    pub text: String,
}

/// `capture.set` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureSet {
    pub paused: bool,
}

/// `ai.complete` params (ADR 0018 §5). The daemon validates and enqueues the
/// job, then answers [`AiJobStarted`] immediately; it never waits for the
/// provider under the 5 s request deadline.
///
/// Bounds are checked by [`AiCompleteParams::validate`]: prompt/system ≤
/// [`MAX_AI_PROMPT_CHARS`] Unicode scalar values, model ≤
/// [`MAX_AI_MODEL_BYTES`] bytes, `max_tokens` 1..=[`MAX_AI_TOKENS`],
/// temperature finite in `0.0..=2.0`, and `timeout_ms` in
/// [`MIN_AI_TIMEOUT_MS`]..=[`MAX_AI_TIMEOUT_MS`].
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct AiCompleteParams {
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

impl AiCompleteParams {
    /// Check the ADR 0018 bounds. Failures are static [`ErrorCode`] values;
    /// the prompt and system text are never echoed into an error or a log.
    ///
    /// Every bound violation answers [`ErrorCode::InvalidRequest`] (the
    /// protocol has no separate AI-params code) so a client can report one
    /// stable "request out of bounds" state.
    pub fn validate(&self) -> Result<(), ErrorCode> {
        if self.prompt.chars().count() > MAX_AI_PROMPT_CHARS {
            return Err(ErrorCode::InvalidRequest);
        }
        if let Some(system) = &self.system
            && system.chars().count() > MAX_AI_PROMPT_CHARS
        {
            return Err(ErrorCode::InvalidRequest);
        }
        if let Some(model) = &self.model
            && model.len() > MAX_AI_MODEL_BYTES
        {
            return Err(ErrorCode::InvalidRequest);
        }
        if let Some(max_tokens) = self.max_tokens
            && !(1..=MAX_AI_TOKENS).contains(&max_tokens)
        {
            return Err(ErrorCode::InvalidRequest);
        }
        if let Some(temperature) = self.temperature
            && !(temperature.is_finite() && (0.0..=2.0).contains(&temperature))
        {
            return Err(ErrorCode::InvalidRequest);
        }
        if let Some(timeout_ms) = self.timeout_ms
            && !(MIN_AI_TIMEOUT_MS..=MAX_AI_TIMEOUT_MS).contains(&timeout_ms)
        {
            return Err(ErrorCode::InvalidRequest);
        }
        Ok(())
    }
}

/// Hand-written so prompt and system text never reach a log line, a panic
/// message, or a crash report; only their lengths are shown.
impl std::fmt::Debug for AiCompleteParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiCompleteParams")
            .field("prompt_chars", &self.prompt.chars().count())
            .field(
                "system_chars",
                &self.system.as_ref().map(|system| system.chars().count()),
            )
            .field("model", &self.model)
            .field("max_tokens", &self.max_tokens)
            .field("temperature", &self.temperature)
            .field("timeout_ms", &self.timeout_ms)
            .finish()
    }
}

/// `ai.poll` params (ADR 0018 §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiPollParams {
    pub job_id: u64,
}

/// `ai.cancel` params (ADR 0018 §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiCancelParams {
    pub job_id: u64,
}

/// `ai.complete` result: the enqueued job id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiJobStarted {
    pub job_id: u64,
}

/// `ai.poll` result (ADR 0018 §5). Carries the rewritten text only on `done`;
/// `failed` carries a static code, never the provider's message or the prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum AiJobState {
    Pending,
    Done {
        text: String,
        model: String,
        input_tokens: u32,
        output_tokens: u32,
        latency_ms: u64,
    },
    Failed {
        code: String,
    },
    Cancelled,
}

/// `ai.status` result (ADR 0018 §4): policy and breaker state without text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiStatusInfo {
    pub enabled: bool,
    pub provider: String,
    pub key_present: bool,
    pub breaker_state: String,
    pub active_jobs: u32,
}

/// Typed protocol error body. The message is static text from
/// [`ErrorCode::message`]; it never includes user text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: ErrorCode,
    pub message: String,
}

impl ErrorBody {
    #[must_use]
    pub fn new(code: ErrorCode) -> Self {
        Self {
            code,
            message: code.message().to_owned(),
        }
    }
}

/// Every request method (contract §4 plus the additive 0.7 AI methods from
/// ADR 0018). `ai.complete` validates and enqueues only; the daemon handler
/// answers [`AiJobStarted`] and the result is polled with `ai.poll`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Request {
    #[serde(rename = "hello")]
    Hello(Hello),
    #[serde(rename = "status")]
    Status(NoArgs),
    #[serde(rename = "settings.get")]
    SettingsGet(NoArgs),
    #[serde(rename = "settings.update")]
    SettingsUpdate(SettingsPatch),
    #[serde(rename = "layouts.list")]
    LayoutsList(NoArgs),
    #[serde(rename = "words.list")]
    WordsList(WordsListQuery),
    #[serde(rename = "words.add")]
    WordsAdd(WordAdd),
    #[serde(rename = "words.remove")]
    WordsRemove(WordRemove),
    #[serde(rename = "words.import")]
    WordsImport(WordsImport),
    #[serde(rename = "words.export")]
    WordsExport(WordsExport),
    #[serde(rename = "text.insert")]
    TextInsert(TextInsert),
    #[serde(rename = "capture.set")]
    CaptureSet(CaptureSet),
    #[serde(rename = "shutdown")]
    Shutdown(NoArgs),
    #[serde(rename = "ai.complete")]
    AiComplete(AiCompleteParams),
    #[serde(rename = "ai.poll")]
    AiPoll(AiPollParams),
    #[serde(rename = "ai.cancel")]
    AiCancel(AiCancelParams),
    #[serde(rename = "ai.status")]
    AiStatus(NoArgs),
}

/// Server replies, one per request frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Response {
    #[serde(rename = "hello_ok")]
    HelloOk(ServerHello),
    #[serde(rename = "status")]
    Status(Status),
    #[serde(rename = "settings")]
    Settings(Settings),
    #[serde(rename = "layouts")]
    Layouts(Vec<LayoutInfo>),
    #[serde(rename = "words")]
    Words(WordsPage),
    #[serde(rename = "word")]
    Word(Word),
    #[serde(rename = "removed")]
    Removed(WordRemoveResult),
    #[serde(rename = "import")]
    Import(WordsImported),
    #[serde(rename = "export")]
    Export(WordsExported),
    #[serde(rename = "inserted")]
    Inserted(TextInserted),
    #[serde(rename = "ok")]
    Ok(NoArgs),
    #[serde(rename = "error")]
    Error(ErrorBody),
    #[serde(rename = "ai_job_started")]
    AiJobStarted(AiJobStarted),
    #[serde(rename = "ai_job")]
    AiJob(AiJobState),
    #[serde(rename = "ai_status")]
    AiStatus(AiStatusInfo),
}

/// `words.remove` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordRemoveResult {
    pub removed: bool,
}

/// `text.insert` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextInserted {
    pub inserted: bool,
}

impl Response {
    /// Convenience constructor for typed errors.
    #[must_use]
    pub fn error(code: ErrorCode) -> Self {
        Self::Error(ErrorBody::new(code))
    }

    /// Borrow the error body when this is an error response.
    #[must_use]
    pub fn error_body(&self) -> Option<&ErrorBody> {
        match self {
            Self::Error(body) => Some(body),
            _ => None,
        }
    }
}

/// Validate a `text.insert` payload (contract §4: capped at 512 UTF-16 code
/// units).
///
/// Over-cap text is **rejected** with [`ErrorCode::TextTooLong`], never
/// truncated: silent truncation would corrupt the user's explicit insertion.
/// The daemon handler (Task 3) must call this rather than reimplementing the
/// bound — the unit is UTF-16 code units, not chars or bytes.
pub fn validate_insert_text(text: &str) -> Result<(), ErrorCode> {
    if text.encode_utf16().count() > MAX_INSERT_UTF16 {
        return Err(ErrorCode::TextTooLong);
    }
    Ok(())
}

/// Normalize a `words.list` limit (contract §4: `limit` ≤ 200).
///
/// Over-limit values are **clamped** to [`MAX_WORDS_LIST_LIMIT`]: pagination
/// is tolerant input, and rejecting would break clients that ask for more
/// than the page cap. The daemon handler (Task 3) must call this rather than
/// reimplementing the bound.
#[must_use]
pub fn normalize_words_list_limit(limit: u64) -> u64 {
    limit.min(MAX_WORDS_LIST_LIMIT)
}

impl Request {
    /// Parse one frame body, distinguishing unknown methods from malformed
    /// requests without relying on serde's error strings.
    pub fn from_json(text: &str) -> Result<Self, ErrorCode> {
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|_| ErrorCode::InvalidRequest)?;
        let object = value.as_object().ok_or(ErrorCode::InvalidRequest)?;
        if object.len() != 1 {
            return Err(ErrorCode::InvalidRequest);
        }
        let mut entries = object.iter();
        let (method, _) = entries.next().ok_or(ErrorCode::InvalidRequest)?;
        if !is_known_method(method) {
            return Err(ErrorCode::UnknownMethod);
        }
        serde_json::from_str::<Request>(text).map_err(|_| ErrorCode::InvalidRequest)
    }
}

fn is_known_method(method: &str) -> bool {
    matches!(
        method,
        "hello"
            | "status"
            | "settings.get"
            | "settings.update"
            | "layouts.list"
            | "words.list"
            | "words.add"
            | "words.remove"
            | "words.import"
            | "words.export"
            | "text.insert"
            | "capture.set"
            | "shutdown"
            | "ai.complete"
            | "ai.poll"
            | "ai.cancel"
            | "ai.status"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ai_params() -> AiCompleteParams {
        AiCompleteParams {
            prompt: "amar likhi".to_owned(),
            system: None,
            model: None,
            max_tokens: None,
            temperature: None,
            timeout_ms: None,
        }
    }

    #[test]
    fn ai_complete_validate_prompt_and_system_caps() {
        assert_eq!(
            AiCompleteParams {
                prompt: "x".repeat(MAX_AI_PROMPT_CHARS),
                ..ai_params()
            }
            .validate(),
            Ok(())
        );
        assert_eq!(
            AiCompleteParams {
                prompt: "x".repeat(MAX_AI_PROMPT_CHARS + 1),
                ..ai_params()
            }
            .validate(),
            Err(ErrorCode::InvalidRequest)
        );
        // The cap counts Unicode scalar values, not bytes or UTF-16 units:
        // 20,000 Bengali scalars (60 kB of UTF-8) fit, 20,001 do not.
        assert_eq!(
            AiCompleteParams {
                prompt: "অ".repeat(MAX_AI_PROMPT_CHARS),
                ..ai_params()
            }
            .validate(),
            Ok(())
        );
        assert_eq!(
            AiCompleteParams {
                prompt: "\u{1F600}".repeat(MAX_AI_PROMPT_CHARS),
                ..ai_params()
            }
            .validate(),
            Ok(())
        );
        assert_eq!(
            AiCompleteParams {
                prompt: "\u{1F600}".repeat(MAX_AI_PROMPT_CHARS + 1),
                ..ai_params()
            }
            .validate(),
            Err(ErrorCode::InvalidRequest)
        );
        // `system` shares the same cap.
        assert_eq!(
            AiCompleteParams {
                system: Some("x".repeat(MAX_AI_PROMPT_CHARS)),
                ..ai_params()
            }
            .validate(),
            Ok(())
        );
        assert_eq!(
            AiCompleteParams {
                system: Some("x".repeat(MAX_AI_PROMPT_CHARS + 1)),
                ..ai_params()
            }
            .validate(),
            Err(ErrorCode::InvalidRequest)
        );
    }

    #[test]
    fn ai_complete_validate_model_cap_is_bytes() {
        assert_eq!(
            AiCompleteParams {
                model: Some("m".repeat(MAX_AI_MODEL_BYTES)),
                ..ai_params()
            }
            .validate(),
            Ok(())
        );
        assert_eq!(
            AiCompleteParams {
                model: Some("m".repeat(MAX_AI_MODEL_BYTES + 1)),
                ..ai_params()
            }
            .validate(),
            Err(ErrorCode::InvalidRequest)
        );
        // 64 Bengali scalars are 192 UTF-8 bytes: over the byte cap even
        // though the scalar count is under it.
        assert_eq!(
            AiCompleteParams {
                model: Some("অ".repeat(64)),
                ..ai_params()
            }
            .validate(),
            Err(ErrorCode::InvalidRequest)
        );
    }

    #[test]
    fn ai_complete_validate_numeric_bounds() {
        for max_tokens in [1, MAX_AI_TOKENS] {
            assert_eq!(
                AiCompleteParams {
                    max_tokens: Some(max_tokens),
                    ..ai_params()
                }
                .validate(),
                Ok(())
            );
        }
        for max_tokens in [0, MAX_AI_TOKENS + 1] {
            assert_eq!(
                AiCompleteParams {
                    max_tokens: Some(max_tokens),
                    ..ai_params()
                }
                .validate(),
                Err(ErrorCode::InvalidRequest)
            );
        }
        for temperature in [0.0, 1.5, 2.0] {
            assert_eq!(
                AiCompleteParams {
                    temperature: Some(temperature),
                    ..ai_params()
                }
                .validate(),
                Ok(())
            );
        }
        for temperature in [-0.1, 2.1, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(
                AiCompleteParams {
                    temperature: Some(temperature),
                    ..ai_params()
                }
                .validate(),
                Err(ErrorCode::InvalidRequest)
            );
        }
        for timeout_ms in [MIN_AI_TIMEOUT_MS, MAX_AI_TIMEOUT_MS] {
            assert_eq!(
                AiCompleteParams {
                    timeout_ms: Some(timeout_ms),
                    ..ai_params()
                }
                .validate(),
                Ok(())
            );
        }
        for timeout_ms in [0, MIN_AI_TIMEOUT_MS - 1, MAX_AI_TIMEOUT_MS + 1] {
            assert_eq!(
                AiCompleteParams {
                    timeout_ms: Some(timeout_ms),
                    ..ai_params()
                }
                .validate(),
                Err(ErrorCode::InvalidRequest)
            );
        }
    }

    #[test]
    fn ai_params_never_render_prompt_text() {
        let secret = "amar gupto proshno";
        let params = AiCompleteParams {
            prompt: format!("{secret}{}", "x".repeat(MAX_AI_PROMPT_CHARS)),
            system: Some(secret.to_owned()),
            ..ai_params()
        };
        let error = params.validate().expect_err("prompt over cap");
        assert_eq!(error, ErrorCode::InvalidRequest);
        let debug = format!("{params:?}");
        for rendered in [debug.as_str(), error.message()] {
            assert!(
                !rendered.contains(secret),
                "prompt text leaked into {rendered}"
            );
        }
        // Debug shows the length, enough to diagnose a rejected frame.
        assert!(debug.contains("prompt_chars"), "{debug}");
    }

    #[test]
    fn golden_fixtures_per_request_kind() {
        // One golden JSON shape per request kind (Task 1 acceptance).
        let hello_token = "00".repeat(32);
        let hello_json = format!("{{\"hello\":{{\"protocol\":1,\"token\":\"{hello_token}\"}}}}",);
        let cases: Vec<(Request, &str)> = vec![
            (
                Request::Hello(Hello {
                    protocol: 1,
                    token: hello_token,
                }),
                &hello_json,
            ),
            (Request::Status(NoArgs {}), r#"{"status":{}}"#),
            (Request::SettingsGet(NoArgs {}), r#"{"settings.get":{}}"#),
            (
                Request::SettingsUpdate(SettingsPatch {
                    theme: Some(crate::settings::Theme::Dark),
                    ..SettingsPatch::default()
                }),
                r#"{"settings.update":{"theme":"dark"}}"#,
            ),
            (Request::LayoutsList(NoArgs {}), r#"{"layouts.list":{}}"#),
            (
                Request::WordsList(WordsListQuery {
                    query: None,
                    offset: 0,
                    limit: 50,
                }),
                r#"{"words.list":{"query":null,"offset":0,"limit":50}}"#,
            ),
            (
                Request::WordsAdd(WordAdd {
                    roman: "amar".to_owned(),
                    output: "আমার".to_owned(),
                    language: WordLanguage::Bn,
                }),
                r#"{"words.add":{"roman":"amar","output":"আমার","language":"bn"}}"#,
            ),
            (
                Request::WordsRemove(WordRemove { id: 7 }),
                r#"{"words.remove":{"id":7}}"#,
            ),
            (
                Request::WordsImport(WordsImport {
                    format: ImportFormat::Csv,
                    data: "roman,output,language".to_owned(),
                }),
                r#"{"words.import":{"format":"csv","data":"roman,output,language"}}"#,
            ),
            (
                Request::WordsExport(WordsExport {
                    format: ImportFormat::Json,
                }),
                r#"{"words.export":{"format":"json"}}"#,
            ),
            (
                Request::TextInsert(TextInsert {
                    text: "hi".to_owned(),
                }),
                r#"{"text.insert":{"text":"hi"}}"#,
            ),
            (
                Request::CaptureSet(CaptureSet { paused: true }),
                r#"{"capture.set":{"paused":true}}"#,
            ),
            (Request::Shutdown(NoArgs {}), r#"{"shutdown":{}}"#),
            (
                Request::AiComplete(ai_params()),
                r#"{"ai.complete":{"prompt":"amar likhi"}}"#,
            ),
            (
                Request::AiPoll(AiPollParams { job_id: 7 }),
                r#"{"ai.poll":{"job_id":7}}"#,
            ),
            (
                Request::AiCancel(AiCancelParams { job_id: 7 }),
                r#"{"ai.cancel":{"job_id":7}}"#,
            ),
            (Request::AiStatus(NoArgs {}), r#"{"ai.status":{}}"#),
        ];
        for (request, expected) in &cases {
            let json = serde_json::to_string(request).expect("serializes");
            assert_eq!(&json, expected);
            let back: Request = serde_json::from_str(&json).expect("parses");
            assert_eq!(&back, request);
        }
        // Method count pins the contract table: 17 entries (13 in 0.5 plus the
        // four additive 0.7 AI methods).
        assert_eq!(cases.len(), 17);
    }

    #[test]
    fn ai_complete_full_shape_round_trips() {
        let params = AiCompleteParams {
            prompt: "notun kore likho".to_owned(),
            system: Some("be brief".to_owned()),
            model: Some("gpt-6-luna".to_owned()),
            max_tokens: Some(256),
            temperature: Some(0.7),
            timeout_ms: Some(15_000),
        };
        let json = serde_json::to_string(&Request::AiComplete(params.clone())).expect("serializes");
        assert_eq!(
            json,
            r#"{"ai.complete":{"prompt":"notun kore likho","system":"be brief","model":"gpt-6-luna","max_tokens":256,"temperature":0.7,"timeout_ms":15000}}"#
        );
        let back: Request = serde_json::from_str(&json).expect("parses");
        assert_eq!(back, Request::AiComplete(params));
        // Omitted optional fields parse as unset, so older clients stay valid.
        let minimal: Request =
            serde_json::from_str(r#"{"ai.complete":{"prompt":"x"}}"#).expect("parses");
        assert_eq!(
            minimal,
            Request::AiComplete(AiCompleteParams {
                prompt: "x".to_owned(),
                system: None,
                model: None,
                max_tokens: None,
                temperature: None,
                timeout_ms: None,
            })
        );
    }

    #[test]
    fn from_json_distinguishes_unknown_and_malformed() {
        assert!(matches!(
            Request::from_json(r#"{"status":{}}"#),
            Ok(Request::Status(_))
        ));
        assert!(matches!(
            Request::from_json(r#"{"shutdown":{}}"#),
            Ok(Request::Shutdown(_))
        ));
        assert_eq!(
            Request::from_json(r#"{"nope":{}}"#),
            Err(ErrorCode::UnknownMethod)
        );
        assert_eq!(
            Request::from_json(r#"{"status":{"x":1},"extra":{}}"#),
            Err(ErrorCode::InvalidRequest)
        );
        assert_eq!(
            Request::from_json("not json"),
            Err(ErrorCode::InvalidRequest)
        );
        assert_eq!(
            Request::from_json(r#"{"status":null}"#),
            Err(ErrorCode::InvalidRequest)
        );
        assert_eq!(
            Request::from_json(r#"{"settings.update":{"theme":"nope"}}"#),
            Err(ErrorCode::InvalidRequest)
        );
        // ai.complete parses; the daemon handler decides the answer.
        assert!(matches!(
            Request::from_json(r#"{"ai.complete":{"prompt":"x"}}"#),
            Ok(Request::AiComplete(_))
        ));
        // A missing required prompt is malformed, not a job with empty text.
        assert_eq!(
            Request::from_json(r#"{"ai.complete":{}}"#),
            Err(ErrorCode::InvalidRequest)
        );
        assert!(matches!(
            Request::from_json(r#"{"ai.poll":{"job_id":7}}"#),
            Ok(Request::AiPoll(AiPollParams { job_id: 7 }))
        ));
        assert!(matches!(
            Request::from_json(r#"{"ai.cancel":{"job_id":7}}"#),
            Ok(Request::AiCancel(AiCancelParams { job_id: 7 }))
        ));
        assert!(matches!(
            Request::from_json(r#"{"ai.status":{}}"#),
            Ok(Request::AiStatus(_))
        ));
        assert_eq!(
            Request::from_json(r#"{"ai.nope":{}}"#),
            Err(ErrorCode::UnknownMethod)
        );
    }

    #[test]
    fn error_response_shape() {
        let json =
            serde_json::to_string(&Response::error(ErrorCode::InvalidLayout)).expect("serializes");
        assert_eq!(
            json,
            r#"{"error":{"code":"InvalidLayout","message":"unknown layout"}}"#
        );
    }

    #[test]
    fn status_shape_matches_contract() {
        let status = Status {
            daemon_version: "0.5.0".to_owned(),
            paused: false,
            language: "bn".to_owned(),
            layout: "avro".to_owned(),
            input_mode: crate::settings::InputMode::Bangla,
            output_encoding: crate::settings::OutputEncoding::Unicode,
            popup_visible: false,
            words_total: 42,
            learning_enabled: true,
            autostart: false,
        };
        let value = serde_json::to_value(&status).expect("serializes");
        for key in [
            "daemon_version",
            "paused",
            "language",
            "layout",
            "input_mode",
            "output_encoding",
            "popup_visible",
            "words_total",
            "learning_enabled",
            "autostart",
        ] {
            assert!(value.get(key).is_some(), "status has {key}");
        }
    }

    #[test]
    fn insert_text_cap_is_utf16_reject_not_truncate() {
        // ASCII: 512 ok, 513 rejected.
        assert_eq!(validate_insert_text(&"x".repeat(512)), Ok(()));
        assert_eq!(
            validate_insert_text(&"x".repeat(513)),
            Err(ErrorCode::TextTooLong)
        );
        // Astral characters cost 2 UTF-16 units each: 256 ok, 257 rejected,
        // even though the char count is far below 512.
        assert_eq!(validate_insert_text(&"\u{1F600}".repeat(256)), Ok(()));
        assert_eq!(
            validate_insert_text(&"\u{1F600}".repeat(257)),
            Err(ErrorCode::TextTooLong)
        );
        // BMP text counts one unit per char.
        assert_eq!(validate_insert_text(&"অ".repeat(512)), Ok(()));
        assert_eq!(validate_insert_text(""), Ok(()));
    }

    #[test]
    fn words_list_limit_clamps() {
        assert_eq!(normalize_words_list_limit(0), 0);
        assert_eq!(normalize_words_list_limit(50), 50);
        assert_eq!(normalize_words_list_limit(200), 200);
        assert_eq!(normalize_words_list_limit(201), MAX_WORDS_LIST_LIMIT);
        assert_eq!(normalize_words_list_limit(u64::MAX), MAX_WORDS_LIST_LIMIT);
    }

    #[test]
    fn golden_fixtures_per_response_kind() {
        // One golden JSON shape per response variant (M1): Task 4 codes to
        // these exact keys; the contract pins only `hello_ok` and `error`.
        let status = || Status {
            daemon_version: "0.5.0".to_owned(),
            paused: false,
            language: "bn".to_owned(),
            layout: "avro".to_owned(),
            input_mode: crate::settings::InputMode::Bangla,
            output_encoding: crate::settings::OutputEncoding::Unicode,
            popup_visible: false,
            words_total: 1,
            learning_enabled: true,
            autostart: false,
        };
        let word = || Word {
            id: 7,
            roman: "amar".to_owned(),
            output: "আমার".to_owned(),
            language: WordLanguage::Bn,
            source: crate::words::WordSource::User,
            added_at_ms: 1,
        };
        let cases: Vec<(Response, &str)> = vec![
            (
                Response::HelloOk(ServerHello {
                    protocol: 1,
                    daemon_version: "0.5.0".to_owned(),
                    pid: 1234,
                }),
                r#"{"hello_ok":{"protocol":1,"daemon_version":"0.5.0","pid":1234}}"#,
            ),
            (
                Response::Status(status()),
                r#"{"status":{"daemon_version":"0.5.0","paused":false,"language":"bn","layout":"avro","input_mode":"bangla","output_encoding":"unicode","popup_visible":false,"words_total":1,"learning_enabled":true,"autostart":false}}"#,
            ),
            (
                Response::Layouts(vec![LayoutInfo {
                    code: "avro".to_owned(),
                    name: "Avro".to_owned(),
                }]),
                r#"{"layouts":[{"code":"avro","name":"Avro"}]}"#,
            ),
            (
                Response::Words(WordsPage {
                    total: 1,
                    words: vec![word()],
                }),
                r#"{"words":{"total":1,"words":[{"id":7,"roman":"amar","output":"আমার","language":"bn","source":"user","added_at_ms":1}]}}"#,
            ),
            (
                Response::Word(word()),
                r#"{"word":{"id":7,"roman":"amar","output":"আমার","language":"bn","source":"user","added_at_ms":1}}"#,
            ),
            (
                Response::Removed(WordRemoveResult { removed: true }),
                r#"{"removed":{"removed":true}}"#,
            ),
            (
                Response::Import(WordsImported {
                    imported: 2,
                    skipped: 1,
                }),
                r#"{"import":{"imported":2,"skipped":1}}"#,
            ),
            (
                Response::Export(WordsExported {
                    format: ImportFormat::Csv,
                    data: "roman,output,language".to_owned(),
                }),
                r#"{"export":{"format":"csv","data":"roman,output,language"}}"#,
            ),
            (
                Response::Inserted(TextInserted { inserted: true }),
                r#"{"inserted":{"inserted":true}}"#,
            ),
            (
                Response::Error(ErrorBody::new(ErrorCode::Unsupported)),
                r#"{"error":{"code":"Unsupported","message":"not available in this milestone"}}"#,
            ),
            (
                Response::AiJobStarted(AiJobStarted { job_id: 7 }),
                r#"{"ai_job_started":{"job_id":7}}"#,
            ),
            (Response::Ok(NoArgs {}), r#"{"ok":{}}"#),
            (
                Response::AiJob(AiJobState::Pending),
                r#"{"ai_job":{"state":"pending"}}"#,
            ),
            (
                Response::AiJob(AiJobState::Done {
                    text: "নতুন করে লিখলাম".to_owned(),
                    model: "gpt-6-luna".to_owned(),
                    input_tokens: 12,
                    output_tokens: 9,
                    latency_ms: 640,
                }),
                r#"{"ai_job":{"state":"done","text":"নতুন করে লিখলাম","model":"gpt-6-luna","input_tokens":12,"output_tokens":9,"latency_ms":640}}"#,
            ),
            (
                Response::AiJob(AiJobState::Failed {
                    code: "AiProviderError".to_owned(),
                }),
                r#"{"ai_job":{"state":"failed","code":"AiProviderError"}}"#,
            ),
            (
                Response::AiJob(AiJobState::Cancelled),
                r#"{"ai_job":{"state":"cancelled"}}"#,
            ),
            (
                Response::AiStatus(AiStatusInfo {
                    enabled: true,
                    provider: "mock".to_owned(),
                    key_present: false,
                    breaker_state: "closed".to_owned(),
                    active_jobs: 0,
                }),
                r#"{"ai_status":{"enabled":true,"provider":"mock","key_present":false,"breaker_state":"closed","active_jobs":0}}"#,
            ),
        ];
        // 17 variants: every non-settings response plus the error shape, the
        // shared `ok` answer, and the four additive 0.7 AI answers (all four
        // job states included).
        // `settings` reuses the `Settings::default` shape pinned in
        // settings.rs `defaults_match_contract`.
        assert_eq!(cases.len(), 17);
        for (response, expected) in &cases {
            let json = serde_json::to_string(response).expect("serializes");
            assert_eq!(&json, expected);
            let back: Response = serde_json::from_str(&json).expect("parses");
            assert_eq!(&back, response);
        }
    }
}

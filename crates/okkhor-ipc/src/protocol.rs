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
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

/// All 0.5 methods (contract §4). `ai.complete` parses but always answers
/// [`ErrorCode::Unsupported`] in 0.5.
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
    #[serde(rename = "ai.complete")]
    AiComplete(serde_json::Value),
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
    #[serde(rename = "error")]
    Error(ErrorBody),
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
            | "ai.complete"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
                r#"{"words.list":{"offset":0,"limit":50}}"#,
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
            (
                Request::AiComplete(serde_json::json!({})),
                r#"{"ai.complete":{}}"#,
            ),
        ];
        for (request, expected) in &cases {
            let json = serde_json::to_string(request).expect("serializes");
            assert_eq!(&json, expected);
            let back: Request = serde_json::from_str(&json).expect("parses");
            assert_eq!(&back, request);
        }
        // Method count pins the contract table: 13 entries.
        assert_eq!(cases.len(), 13);
    }

    #[test]
    fn from_json_distinguishes_unknown_and_malformed() {
        assert!(matches!(
            Request::from_json(r#"{"status":{}}"#),
            Ok(Request::Status(_))
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
        // ai.complete parses so the server can answer Unsupported.
        assert!(matches!(
            Request::from_json(r#"{"ai.complete":{"prompt":"x"}}"#),
            Ok(Request::AiComplete(_))
        ));
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
            "popup_visible",
            "words_total",
            "learning_enabled",
            "autostart",
        ] {
            assert!(value.get(key).is_some(), "status has {key}");
        }
    }
}

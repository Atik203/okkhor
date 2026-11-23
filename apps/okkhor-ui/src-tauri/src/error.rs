#![forbid(unsafe_code)]
//! Typed errors surfaced to the WebView.
//!
//! The daemon server is not wired yet: any connection-level failure (missing
//! pipe, missing/unreadable token, framing I/O) maps to `daemon_unavailable`
//! so the UI can render its banner + read-only mode instead of crashing.
//! Daemon `error` responses pass their protocol `ErrorCode` name through
//! verbatim as `code` with the code's static message (never the daemon's
//! message string, which could carry user text).

use serde::Serialize;

/// Error shape returned to JS: `{ code, message }`. No tokens, paths, or
/// user text are ever included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[error("{message}")]
pub struct UiError {
    pub code: String,
    pub message: String,
}

impl UiError {
    #[must_use]
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    /// Connection-level failure: the daemon is absent or unreachable.
    #[must_use]
    pub fn daemon_unavailable() -> Self {
        Self::new(
            "daemon_unavailable",
            "Okkhor daemon is not running. Start Okkhor and retry.",
        )
    }

    /// Bug-shaped local failure (should not happen from UI input).
    #[must_use]
    pub fn internal() -> Self {
        Self::new("internal", "Internal UI error.")
    }
}

/// Map an `okkhor-ipc` transport failure to a typed [`UiError`].
///
/// Daemon `error` responses pass their protocol `ErrorCode` name through
/// verbatim as `code`, but the message is always the code's static text —
/// never the daemon's message string, which could carry user text or paths
/// (issue #72 follow-up).
#[cfg(windows)]
#[must_use]
pub fn map_client_error(error: &okkhor_ipc::transport::ClientError) -> UiError {
    use okkhor_ipc::transport::ClientError;
    match error {
        ClientError::Io(_) | ClientError::Frame(_) => UiError::daemon_unavailable(),
        ClientError::Server(body) => {
            UiError::new(format!("{:?}", body.code), body.code.message().to_owned())
        }
        ClientError::Protocol => UiError::internal(),
    }
}

/// Map a token-file failure to a typed [`UiError`].
///
/// A missing or unreadable token means the daemon has not published a
/// session (it rotates the token on every start), so this is also
/// `daemon_unavailable` — never a raw IO error to the UI.
#[must_use]
pub fn map_token_error(_error: &okkhor_ipc::auth::TokenFileError) -> UiError {
    UiError::daemon_unavailable()
}

/// Read the session token hex for the IPC hello handshake.
/// Returns `daemon_unavailable` when the daemon has not published a token.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn read_session_token_hex() -> Result<String, UiError> {
    let path = okkhor_ipc::auth::default_token_path().map_err(|e| map_token_error(&e))?;
    let token = okkhor_ipc::auth::read_token_file(&path).map_err(|e| map_token_error(&e))?;
    Ok(token.hex())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_unavailable_shape() {
        let error = UiError::daemon_unavailable();
        assert_eq!(error.code, "daemon_unavailable");
        assert!(!error.message.is_empty());
        // Serializes as `{ code, message }` for `invoke` rejection.
        let value = serde_json::to_value(&error).expect("serializes");
        assert_eq!(value["code"], "daemon_unavailable");
        assert!(value["message"].is_string());
    }

    #[test]
    fn token_errors_map_to_daemon_unavailable() {
        // Every token-file failure — even AlreadyExists/Malformed — maps to
        // `daemon_unavailable` and carries no secret or path text.
        let errors = [
            okkhor_ipc::auth::TokenFileError::Entropy,
            okkhor_ipc::auth::TokenFileError::Malformed,
            okkhor_ipc::auth::TokenFileError::AlreadyExists,
        ];
        for error in &errors {
            let mapped = map_token_error(error);
            assert_eq!(mapped.code, "daemon_unavailable", "{error:?}");
            let rendered = serde_json::to_string(&mapped).expect("serializes");
            assert!(!rendered.contains("ipc.token"));
        }
        let io = okkhor_ipc::auth::TokenFileError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "probe",
        ));
        assert_eq!(map_token_error(&io).code, "daemon_unavailable");
    }

    #[cfg(windows)]
    #[test]
    fn client_errors_map_to_typed_codes() {
        use okkhor_ipc::transport::ClientError;
        // Connection-level failures -> daemon_unavailable.
        let io = ClientError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "pipe missing",
        ));
        assert_eq!(map_client_error(&io).code, "daemon_unavailable");
        // Protocol violations -> internal.
        assert_eq!(map_client_error(&ClientError::Protocol).code, "internal");
        // Daemon error bodies pass the code through verbatim by name.
        for code in [
            okkhor_ipc::ErrorCode::InvalidLayout,
            okkhor_ipc::ErrorCode::InvalidHotkey,
            okkhor_ipc::ErrorCode::InvalidSettings,
            okkhor_ipc::ErrorCode::Timeout,
            okkhor_ipc::ErrorCode::Unsupported,
        ] {
            let server = ClientError::Server(okkhor_ipc::protocol::ErrorBody::new(code));
            let mapped = map_client_error(&server);
            assert_eq!(mapped.code, format!("{code:?}"));
            assert!(!mapped.message.is_empty());
        }
    }

    /// Unknown/hostile daemon messages are never echoed: the mapped message is
    /// always the code's static text, even when the daemon's message carries
    /// user text, tokens, or paths (issue #72 follow-up).
    #[cfg(windows)]
    #[test]
    fn server_errors_use_static_messages_never_daemon_text() {
        use okkhor_ipc::transport::ClientError;
        for code in [
            okkhor_ipc::ErrorCode::InvalidWord,
            okkhor_ipc::ErrorCode::DuplicateWord,
            okkhor_ipc::ErrorCode::TooManyWords,
            okkhor_ipc::ErrorCode::TextTooLong,
            okkhor_ipc::ErrorCode::ExportTooLarge,
            okkhor_ipc::ErrorCode::InvalidSettings,
            okkhor_ipc::ErrorCode::Internal,
        ] {
            let hostile = okkhor_ipc::protocol::ErrorBody {
                code,
                message: "user secret আমার C:\\Users\\someone words leak".to_owned(),
            };
            let mapped = map_client_error(&ClientError::Server(hostile));
            assert_eq!(mapped.code, format!("{code:?}"));
            assert_eq!(mapped.message, code.message());
            assert!(!mapped.message.contains("secret"));
            assert!(!mapped.message.contains("আমার"));
        }
    }

    /// A wire `error` with an unrecognized code name cannot deserialize into
    /// `Response`, so it surfaces as `Protocol` -> `internal`, never raw text.
    #[test]
    fn unknown_wire_error_code_maps_to_internal() {
        use okkhor_ipc::transport::ClientError;
        let raw = r#"{"error":{"code":"Nope","message":"user words leak here"}}"#;
        let parsed = serde_json::from_str::<okkhor_ipc::protocol::Response>(raw);
        assert!(parsed.is_err(), "unknown code must not parse");
        #[cfg(windows)]
        {
            let mapped = map_client_error(&ClientError::Protocol);
            assert_eq!(mapped.code, "internal");
            assert!(!mapped.message.contains("leak"));
        }
    }

    #[test]
    fn token_path_resolution_uses_override_or_profile() {
        // With an override set, the token path lives under `<override>\runtime`.
        std::env::set_var("OKKHOR_DATA_HOME", "E:\\TestUiTokenPath");
        let path = okkhor_ipc::auth::default_token_path().expect("resolves");
        std::env::remove_var("OKKHOR_DATA_HOME");
        assert_eq!(
            path,
            std::path::PathBuf::from("E:\\TestUiTokenPath")
                .join("runtime")
                .join("ipc.token")
        );
        // Without override or LOCALAPPDATA there is no token to read —
        // and that still resolves to a typed error, never a panic.
        let local = std::env::var_os("LOCALAPPDATA");
        if local.is_none() {
            assert!(okkhor_ipc::auth::default_token_path().is_err());
        }
    }
}

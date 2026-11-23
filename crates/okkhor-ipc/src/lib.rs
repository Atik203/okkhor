//! IPC protocol, framing, auth, transport, settings, and word types for the
//! Okkhor settings UI (0.5 milestone).
//!
//! Binding contracts: `docs/design/0.5-ui-contracts.md` and
//! `docs/adr/0012-ui-ipc-and-settings.md`.
//!
//! This crate carries only low-rate control traffic (status, settings, word
//! manager, explicit on-screen-keyboard insertion). It is never reachable from
//! the typing hook or the input worker hot path: no per-keystroke IPC.
//!
//! Safety: every module in this crate is `#![forbid(unsafe_code)]` except
//! [`sddl`] (Windows only), which isolates the single unavoidable `unsafe`
//! call (`ConvertStringSecurityDescriptorToSecurityDescriptorW` for the
//! owner-only pipe DACL) with `SAFETY:` comments. The crate root therefore
//! carries no blanket `forbid(unsafe_code)`.
//!
//! Privacy: no secrets, user text, or window titles are logged. Error
//! messages use fixed codes and static text only.

pub mod auth;
pub mod error;
pub mod frame;
pub mod protocol;
#[cfg(windows)]
pub mod sddl;
pub mod settings;
#[cfg(windows)]
pub mod transport;
pub mod words;

pub use auth::{Token, TokenFileError, default_runtime_dir, default_token_path};
pub use error::ErrorCode;
pub use frame::{FrameError, read_frame, write_frame};
pub use protocol::{
    CaptureSet, ErrorBody, Hello, ImportFormat, LayoutInfo, MAX_CLIENTS, MAX_FRAME_BYTES,
    MAX_INSERT_UTF16, MAX_WORDS_LIST_LIMIT, NoArgs, PIPE_NAME, PROTOCOL_VERSION, READ_TIMEOUT,
    REQUEST_DEADLINE, Request, Response, ServerHello, Status, TextInsert, TextInserted, WordAdd,
    WordRemove, WordRemoveResult, WordsExport, WordsExported, WordsImport, WordsImported,
    WordsListQuery, WordsPage, normalize_words_list_limit, validate_insert_text,
};
pub use settings::{Accent, Language, Settings, SettingsError, SettingsPatch, Theme, parse_hotkey};
#[cfg(windows)]
pub use transport::{Client, ClientError, Handler, Server, TransportError};
pub use words::{
    CSV_HEADER, MAX_IMPORT_ROWS, MAX_WORDS_TOTAL, NewWord, OUTPUT_MAX_CHARS, ParsedImport,
    ROMAN_MAX_CHARS, ROMAN_MIN_CHARS, Word, WordLanguage, WordSource, WordsError, WordsFile,
    check_total_cap, is_duplicate, parse_words_csv, parse_words_json, validate_new_word,
    validate_output, validate_roman, validate_words_file, words_to_csv, words_to_json,
};

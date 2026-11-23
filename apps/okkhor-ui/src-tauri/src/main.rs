#![forbid(unsafe_code)]
//! Okkhor settings UI shell (Tauri 2, ADR 0012).
//!
//! Separate process from the daemon: a UI crash must not stop typing.
//! Rust commands below are async, typed, and never `unwrap` on user input.
//! The daemon is the single writer of `settings.json`; this shell only talks
//! to it through the `okkhor-ipc` client. When the daemon is absent every
//! command fails with `daemon_unavailable` and the WebView renders its
//! banner + read-only mode.

mod error;

use error::{map_client_error, read_session_token_hex, UiError};

/// Open an authenticated IPC client, completing the hello handshake.
/// Any connection-level failure becomes `daemon_unavailable`.
#[cfg(windows)]
async fn connect() -> Result<okkhor_ipc::transport::Client, UiError> {
    let token_hex = read_session_token_hex()?;
    okkhor_ipc::transport::Client::connect_to_with_token(
        okkhor_ipc::protocol::PIPE_NAME,
        &token_hex,
    )
    .await
    .map(|(client, _)| client)
    .map_err(|e| map_client_error(&e))
}

#[tauri::command]
async fn ipc_status() -> Result<okkhor_ipc::protocol::Status, UiError> {
    #[cfg(windows)]
    {
        let mut client = connect().await?;
        client.status().await.map_err(|e| map_client_error(&e))
    }
    #[cfg(not(windows))]
    {
        Err(UiError::daemon_unavailable())
    }
}

#[tauri::command]
async fn ipc_get_settings() -> Result<okkhor_ipc::settings::Settings, UiError> {
    #[cfg(windows)]
    {
        let mut client = connect().await?;
        client
            .settings_get()
            .await
            .map_err(|e| map_client_error(&e))
    }
    #[cfg(not(windows))]
    {
        Err(UiError::daemon_unavailable())
    }
}

#[tauri::command]
async fn ipc_update_settings(
    patch: okkhor_ipc::settings::SettingsPatch,
) -> Result<okkhor_ipc::settings::Settings, UiError> {
    // `patch` deserialization rejects malformed shapes before we run; unknown
    // fields are ignored by serde default. No direct file writes here.
    #[cfg(windows)]
    {
        let mut client = connect().await?;
        client
            .settings_update(&patch)
            .await
            .map_err(|e| map_client_error(&e))
    }
    #[cfg(not(windows))]
    {
        let _ = patch;
        Err(UiError::daemon_unavailable())
    }
}

#[tauri::command]
async fn ipc_list_layouts() -> Result<Vec<okkhor_ipc::protocol::LayoutInfo>, UiError> {
    #[cfg(windows)]
    {
        let mut client = connect().await?;
        client
            .layouts_list()
            .await
            .map_err(|e| map_client_error(&e))
    }
    #[cfg(not(windows))]
    {
        Err(UiError::daemon_unavailable())
    }
}

#[tauri::command]
async fn ipc_insert_text(text: String) -> Result<bool, UiError> {
    // The daemon enforces the 512 UTF-16-unit cap; an empty insert is a
    // no-op rejected locally so we never open the pipe for nothing.
    if text.is_empty() {
        return Err(UiError::new("invalid_settings", "Text must not be empty."));
    }
    #[cfg(windows)]
    {
        let mut client = connect().await?;
        client
            .insert_text(&text)
            .await
            .map_err(|e| map_client_error(&e))
    }
    #[cfg(not(windows))]
    {
        Err(UiError::daemon_unavailable())
    }
}

#[tauri::command]
async fn ipc_set_paused(paused: bool) -> Result<okkhor_ipc::protocol::Status, UiError> {
    #[cfg(windows)]
    {
        let mut client = connect().await?;
        client
            .set_capture(paused)
            .await
            .map_err(|e| map_client_error(&e))
    }
    #[cfg(not(windows))]
    {
        let _ = paused;
        Err(UiError::daemon_unavailable())
    }
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            ipc_status,
            ipc_get_settings,
            ipc_update_settings,
            ipc_list_layouts,
            ipc_insert_text,
            ipc_set_paused
        ])
        .run(tauri::generate_context!())
        .expect("error while running Okkhor settings UI");
}

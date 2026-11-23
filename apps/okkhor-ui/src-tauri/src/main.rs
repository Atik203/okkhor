#![deny(unsafe_code)]
//! Okkhor settings UI shell (Tauri 2, ADR 0012).
//!
//! Separate process from the daemon: a UI crash must not stop typing.
//! Rust commands below are async, typed, and never `unwrap` on user input.
//! The daemon is the single writer of `settings.json`; this shell only talks
//! to it through the `okkhor-ipc` client. When the daemon is absent every
//! command fails with `daemon_unavailable` and the WebView renders its
//! banner + read-only mode.
//!
//! The quick control bar (ADR 0014 §5, as amended 2026-11-23) is a second
//! Tauri window (`quickbar`) owed by [`quickbar`]: it never activates on
//! Windows (`WS_EX_NOACTIVATE`), shows with `SW_SHOWNOACTIVATE`, and sits
//! top-right of the primary monitor. `--quickbar` launches the bar only and
//! keeps `main` hidden until `quickbar_open_settings` shows it.

mod error;
mod quickbar;

use error::{map_client_error, read_session_token_hex, UiError};
use quickbar::QuickbarState;
use tauri::{AppHandle, Emitter, Manager};

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

/// Load the full quickbar snapshot: live mode/encoding/pause from `status`
/// plus the bar chrome (`quickbar_collapsed`, `quickbar_opacity`) from
/// settings. `status` alone cannot supply the chrome fields, and settings
/// alone cannot supply pause, so both round trips are needed.
#[cfg(windows)]
async fn load_quickbar_state() -> Result<QuickbarState, UiError> {
    let mut client = connect().await?;
    let status = client.status().await.map_err(|e| map_client_error(&e))?;
    // Chrome fields are best-effort: a settings read failure must not hide
    // live mode state, so fall back to the contract defaults.
    let (collapsed, opacity) = client
        .settings_get()
        .await
        .map(|settings| (settings.quickbar_collapsed, settings.quickbar_opacity))
        .unwrap_or((false, 0.9));
    Ok(QuickbarState {
        input_mode: status.input_mode,
        output_encoding: status.output_encoding,
        paused: status.paused,
        available: true,
        collapsed,
        opacity,
    })
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

#[tauri::command]
async fn ipc_list_words(
    query: Option<String>,
    offset: u64,
    limit: u64,
) -> Result<okkhor_ipc::protocol::WordsPage, UiError> {
    #[cfg(windows)]
    {
        let mut client = connect().await?;
        client
            .words_list(&okkhor_ipc::protocol::WordsListQuery {
                query,
                offset,
                limit,
            })
            .await
            .map_err(|e| map_client_error(&e))
    }
    #[cfg(not(windows))]
    {
        let _ = (query, offset, limit);
        Err(UiError::daemon_unavailable())
    }
}

#[tauri::command]
async fn ipc_add_word(
    roman: String,
    output: String,
    language: okkhor_ipc::words::WordLanguage,
) -> Result<okkhor_ipc::words::Word, UiError> {
    // Empty fields are rejected locally so we never open the pipe for
    // nothing; the daemon re-validates lengths, control characters, and
    // duplicates authoritatively (contract §3).
    if roman.is_empty() || output.is_empty() {
        return Err(UiError::new(
            "invalid_settings",
            "Word fields must not be empty.",
        ));
    }
    #[cfg(windows)]
    {
        let mut client = connect().await?;
        client
            .words_add(&okkhor_ipc::protocol::WordAdd {
                roman,
                output,
                language,
            })
            .await
            .map_err(|e| map_client_error(&e))
    }
    #[cfg(not(windows))]
    {
        let _ = language;
        Err(UiError::daemon_unavailable())
    }
}

#[tauri::command]
async fn ipc_remove_word(id: u64) -> Result<bool, UiError> {
    #[cfg(windows)]
    {
        let mut client = connect().await?;
        client
            .words_remove(id)
            .await
            .map_err(|e| map_client_error(&e))
    }
    #[cfg(not(windows))]
    {
        let _ = id;
        Err(UiError::daemon_unavailable())
    }
}

#[tauri::command]
async fn ipc_import_words(
    format: okkhor_ipc::protocol::ImportFormat,
    data: String,
) -> Result<okkhor_ipc::protocol::WordsImported, UiError> {
    if data.is_empty() {
        return Err(UiError::new(
            "invalid_settings",
            "Import payload must not be empty.",
        ));
    }
    #[cfg(windows)]
    {
        let mut client = connect().await?;
        client
            .words_import(&okkhor_ipc::protocol::WordsImport { format, data })
            .await
            .map_err(|e| map_client_error(&e))
    }
    #[cfg(not(windows))]
    {
        let _ = format;
        Err(UiError::daemon_unavailable())
    }
}

#[tauri::command]
async fn ipc_export_words(
    format: okkhor_ipc::protocol::ImportFormat,
) -> Result<okkhor_ipc::protocol::WordsExported, UiError> {
    #[cfg(windows)]
    {
        let mut client = connect().await?;
        client
            .words_export(format)
            .await
            .map_err(|e| map_client_error(&e))
    }
    #[cfg(not(windows))]
    {
        let _ = format;
        Err(UiError::daemon_unavailable())
    }
}

/// Quick bar snapshot: mode, encoding, pause, availability, and chrome.
/// Reuses the IPC client, so a missing daemon surfaces `daemon_unavailable`
/// exactly like the settings shell.
#[tauri::command]
async fn quickbar_state() -> Result<QuickbarState, UiError> {
    #[cfg(windows)]
    {
        load_quickbar_state().await
    }
    #[cfg(not(windows))]
    {
        Err(UiError::daemon_unavailable())
    }
}

/// Flip Bangla/English through `settings.update` and return the new snapshot.
#[tauri::command]
async fn quickbar_toggle_mode() -> Result<QuickbarState, UiError> {
    #[cfg(windows)]
    {
        let mut client = connect().await?;
        let current = client
            .settings_get()
            .await
            .map_err(|e| map_client_error(&e))?;
        let next = match current.input_mode {
            okkhor_ipc::settings::InputMode::Bangla => okkhor_ipc::settings::InputMode::English,
            okkhor_ipc::settings::InputMode::English => okkhor_ipc::settings::InputMode::Bangla,
        };
        let patch = okkhor_ipc::settings::SettingsPatch {
            input_mode: Some(next),
            ..Default::default()
        };
        client
            .settings_update(&patch)
            .await
            .map_err(|e| map_client_error(&e))?;
        load_quickbar_state().await
    }
    #[cfg(not(windows))]
    {
        Err(UiError::daemon_unavailable())
    }
}

/// Cycle Unicode/Bijoy through `settings.update` and return the new snapshot.
#[tauri::command]
async fn quickbar_cycle_encoding() -> Result<QuickbarState, UiError> {
    #[cfg(windows)]
    {
        let mut client = connect().await?;
        let current = client
            .settings_get()
            .await
            .map_err(|e| map_client_error(&e))?;
        let next = match current.output_encoding {
            okkhor_ipc::settings::OutputEncoding::Unicode => {
                okkhor_ipc::settings::OutputEncoding::Bijoy
            }
            okkhor_ipc::settings::OutputEncoding::Bijoy => {
                okkhor_ipc::settings::OutputEncoding::Unicode
            }
        };
        let patch = okkhor_ipc::settings::SettingsPatch {
            output_encoding: Some(next),
            ..Default::default()
        };
        client
            .settings_update(&patch)
            .await
            .map_err(|e| map_client_error(&e))?;
        load_quickbar_state().await
    }
    #[cfg(not(windows))]
    {
        Err(UiError::daemon_unavailable())
    }
}

/// Toggle capture pause through `capture.set` and return the new snapshot.
#[tauri::command]
async fn quickbar_toggle_pause() -> Result<QuickbarState, UiError> {
    #[cfg(windows)]
    {
        let mut client = connect().await?;
        let status = client.status().await.map_err(|e| map_client_error(&e))?;
        client
            .set_capture(!status.paused)
            .await
            .map_err(|e| map_client_error(&e))?;
        load_quickbar_state().await
    }
    #[cfg(not(windows))]
    {
        Err(UiError::daemon_unavailable())
    }
}

/// Show the settings window and navigate it to `/settings`. The bar itself
/// never activates: only `main` is shown and focused here.
#[tauri::command]
async fn quickbar_open_settings(app: AppHandle) -> Result<(), UiError> {
    let Some(main) = app.get_webview_window("main") else {
        return Err(UiError::internal());
    };
    main.show().map_err(|_| UiError::internal())?;
    // Best-effort unminimize; a failure must not block showing settings.
    let _ = main.unminimize();
    main.set_focus().map_err(|_| UiError::internal())?;
    main.emit("okkhor:navigate", "/settings")
        .map_err(|_| UiError::internal())?;
    Ok(())
}

/// Resize the bar window. The resize always applies locally; persisting
/// `quickbar_collapsed` is best-effort so the orb still works while the
/// daemon is down (shell stays read-only, chrome stays local).
#[tauri::command]
async fn quickbar_collapse(app: AppHandle) -> Result<QuickbarState, UiError> {
    set_quickbar_collapsed(&app, true).await
}

/// Restore the bar window to the full pill. See [`quickbar_collapse`].
#[tauri::command]
async fn quickbar_expand(app: AppHandle) -> Result<QuickbarState, UiError> {
    set_quickbar_collapsed(&app, false).await
}

async fn set_quickbar_collapsed(
    app: &AppHandle,
    collapsed: bool,
) -> Result<QuickbarState, UiError> {
    let Some(bar) = app.get_webview_window("quickbar") else {
        return Err(UiError::internal());
    };
    let (w, h) = if collapsed {
        (quickbar::ORB_LOGICAL, quickbar::ORB_LOGICAL)
    } else {
        (quickbar::QUICKBAR_LOGICAL_W, quickbar::QUICKBAR_LOGICAL_H)
    };
    bar.set_size(tauri::LogicalSize {
        width: f64::from(w),
        height: f64::from(h),
    })
    .map_err(|_| UiError::internal())?;
    #[cfg(windows)]
    {
        // Re-apply the collapsed size's position? No: keep the user's drag
        // position; only persist the chrome flag when the daemon is up.
        let patch = okkhor_ipc::settings::SettingsPatch {
            quickbar_collapsed: Some(collapsed),
            ..Default::default()
        };
        if let Ok(mut client) = connect().await {
            if let Ok(settings) = client
                .settings_update(&patch)
                .await
                .map_err(|e| map_client_error(&e))
            {
                // Window resize already applied; report the persisted truth.
                if let Ok(mut refreshed) = connect().await {
                    if let Ok(status) = refreshed.status().await {
                        return Ok(QuickbarState {
                            input_mode: status.input_mode,
                            output_encoding: status.output_encoding,
                            paused: status.paused,
                            available: true,
                            collapsed: settings.quickbar_collapsed,
                            opacity: settings.quickbar_opacity,
                        });
                    }
                }
            }
        }
        // Daemon down: the resize above still stands; report local truth.
        Ok(QuickbarState::unavailable(collapsed, 0.9))
    }
    #[cfg(not(windows))]
    {
        let _ = collapsed;
        Err(UiError::daemon_unavailable())
    }
}

/// Place the bar top-right of the primary monitor (falling back to its
/// current monitor), scaling the logical size by the monitor's scale factor
/// so 100/150/200% DPI all hug the corner.
fn place_quickbar_top_right(window: &tauri::WebviewWindow) {
    let monitor = window
        .primary_monitor()
        .ok()
        .flatten()
        .or_else(|| window.current_monitor().ok().flatten());
    if let Some(monitor) = monitor {
        let origin = monitor.position();
        let size = monitor.size();
        let (x, y) = quickbar::quickbar_top_right_physical(
            (origin.x, origin.y),
            (size.width, size.height),
            (quickbar::QUICKBAR_LOGICAL_W, quickbar::QUICKBAR_LOGICAL_H),
            monitor.scale_factor(),
            quickbar::EDGE_MARGIN_LOGICAL,
        );
        let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y }));
    }
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let args: Vec<String> = std::env::args().collect();
            let bar_only = quickbar::is_quickbar_launch(&args);
            if bar_only {
                // Bar-only launch: the settings window must never flash.
                // (`main` starts hidden via `tauri.conf.json`.)
                if let Some(bar) = app.get_webview_window("quickbar") {
                    place_quickbar_top_right(&bar);
                    #[cfg(windows)]
                    if let Ok(hwnd) = bar.hwnd() {
                        quickbar::native::apply_noactivate(hwnd);
                        quickbar::native::show_noactivate(hwnd);
                    }
                    #[cfg(not(windows))]
                    {
                        let _ = bar.show();
                    }
                }
            } else {
                // Settings launch: show `main`, keep the bar parked hidden.
                // (The bar is served by `--quickbar` launches from the tray.)
                if let Some(main) = app.get_webview_window("main") {
                    let _ = main.show();
                    let _ = main.set_focus();
                }
                if let Some(bar) = app.get_webview_window("quickbar") {
                    let _ = bar.hide();
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ipc_status,
            ipc_get_settings,
            ipc_update_settings,
            ipc_list_layouts,
            ipc_insert_text,
            ipc_set_paused,
            ipc_list_words,
            ipc_add_word,
            ipc_remove_word,
            ipc_import_words,
            ipc_export_words,
            quickbar_state,
            quickbar_toggle_mode,
            quickbar_cycle_encoding,
            quickbar_toggle_pause,
            quickbar_open_settings,
            quickbar_collapse,
            quickbar_expand
        ])
        .run(tauri::generate_context!())
        .expect("error while running Okkhor settings UI");
}

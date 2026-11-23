#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![deny(unsafe_code)]
//! Okkhor settings UI shell (Tauri 2, ADR 0012).
//!
//! This process is the user-facing entry point: the portable folder ships it
//! as `Okkhor.exe` and keeps the resident engine in `bin\`. On launch it calls
//! [`daemon_ensure`], which starts the engine when it is not already running,
//! so nobody has to know that typing and settings live in separate processes.
//! Separate processes remain deliberate: a UI crash must not stop typing.
//! Rust commands below are async, typed, and never `unwrap` on user input.
//! The daemon is the single writer of `settings.json`; this shell only talks
//! to it through the `okkhor-ipc` client. When the engine cannot start, every
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

/// Locate the resident engine for the single-entry-point launch.
///
/// The portable folder ships one user-facing executable (`Okkhor.exe`, this
/// UI) and keeps the engine in `bin\`. Older 0.5.0 folders and dev builds keep
/// it as a sibling, so those are still accepted. `OKKHOR_ENGINE_PATH`
/// overrides everything (dev/QA only).
#[cfg(windows)]
fn find_engine_exe() -> Option<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("OKKHOR_ENGINE_PATH") {
        let path = std::path::PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    [
        dir.join("bin").join("okkhor-engine.exe"),
        dir.join("okkhor-engine.exe"),
        dir.join("okkhor.exe"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

/// Start the resident engine detached and console-free. The engine's own
/// single-instance guard makes a duplicate spawn a no-op, so a racing launch
/// (UI opened twice) is harmless.
#[cfg(windows)]
fn spawn_engine() -> Result<(), UiError> {
    use std::os::windows::process::CommandExt as _;
    /// Detach from this process's console and never create a new one.
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let exe = find_engine_exe().ok_or_else(UiError::engine_missing)?;
    std::process::Command::new(exe)
        .arg("daemon")
        .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|_| UiError::engine_missing())
}

/// Show the quick bar after a cold engine start when the setting says it
/// should be visible.
///
/// First launch used to resolve `quickbar_visible` in `setup` before the
/// engine had created its pipe, so the default-on bar silently stayed hidden.
/// This runs after `daemon_ensure` and applies the saved position, chrome, and
/// visibility exactly like the tray show path.
#[cfg(windows)]
async fn reveal_quickbar_if_configured(app: &AppHandle) {
    let Ok(state) = load_quickbar_state().await else {
        return;
    };
    if !state.visible {
        return;
    }
    let Some(bar) = app.get_webview_window("quickbar") else {
        return;
    };
    if let Some((x, y)) = state.x.zip(state.y) {
        let _ = bar.set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y }));
    }
    if !bar_is_on_screen(&bar) {
        place_quickbar_top_right(&bar);
    }
    apply_bar_chrome(&bar, state.collapsed);
    show_bar_noactivate(&bar);
}

/// Ensure the engine is running, starting it next to this UI when needed.
///
/// `Okkhor.exe` is the only executable a user launches: it starts the engine
/// itself, then waits (bounded, ~8 s) for the authenticated pipe before the
/// WebView renders read-only. Two webviews (settings + quick bar) can request
/// this at once, so spawns are rate-limited instead of relying on the engine's
/// single-instance guard alone. A later retry can still restart a dead engine.
#[tauri::command]
async fn daemon_ensure(app: AppHandle) -> Result<bool, UiError> {
    #[cfg(windows)]
    {
        use std::time::{Duration, Instant};

        if ipc_status().await.is_ok() {
            reveal_quickbar_if_configured(&app).await;
            return Ok(true);
        }
        // At most one spawn per 5 s across every window of this process.
        static LAST_SPAWN: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);
        let should_spawn = match LAST_SPAWN.lock() {
            Ok(mut last) => match *last {
                Some(at) if at.elapsed() < Duration::from_secs(5) => false,
                _ => {
                    *last = Some(Instant::now());
                    true
                }
            },
            Err(_) => true,
        };
        if should_spawn {
            spawn_engine()?;
        }
        // Engine start includes hook install, tray creation, token write, and
        // the pipe listener. Cold start on a slow disk is the slow case; the
        // loop is bounded and cheap (one local connect per 160 ms).
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(160)).await;
            if ipc_status().await.is_ok() {
                reveal_quickbar_if_configured(&app).await;
                return Ok(true);
            }
        }
        Err(UiError::daemon_unavailable())
    }
    #[cfg(not(windows))]
    {
        Err(UiError::daemon_unavailable())
    }
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
    let (collapsed, opacity, visible, x, y) = client
        .settings_get()
        .await
        .map(|settings| {
            (
                settings.quickbar_collapsed,
                settings.quickbar_opacity,
                settings.quickbar_visible,
                settings.quickbar_x,
                settings.quickbar_y,
            )
        })
        .unwrap_or((false, 0.9, false, None, None));
    Ok(QuickbarState {
        input_mode: status.input_mode,
        output_encoding: status.output_encoding,
        paused: status.paused,
        available: true,
        collapsed,
        opacity,
        visible,
        x,
        y,
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

/// Show the main window and navigate it to `route`. The bar itself
/// never activates: only `main` is shown and focused here.
fn show_main_at(app: &AppHandle, route: &str) -> Result<(), UiError> {
    let Some(main) = app.get_webview_window("main") else {
        return Err(UiError::internal());
    };
    main.show().map_err(|_| UiError::internal())?;
    // Best-effort unminimize; a failure must not block showing the window.
    let _ = main.unminimize();
    main.set_focus().map_err(|_| UiError::internal())?;
    main.emit("okkhor:navigate", route)
        .map_err(|_| UiError::internal())?;
    Ok(())
}

/// Show the settings window and navigate it to `/settings`. The bar itself
/// never activates: only `main` is shown and focused here.
#[tauri::command]
async fn quickbar_open_settings(app: AppHandle) -> Result<(), UiError> {
    show_main_at(&app, "/settings")
}

/// Show the main window at the on-screen keyboard (`/keyboard`), same
/// pattern as [`quickbar_open_settings`].
#[tauri::command]
async fn quickbar_open_keyboard(app: AppHandle) -> Result<(), UiError> {
    show_main_at(&app, "/keyboard")
}

/// Best-effort persistence of the bar visibility flag.
#[cfg(windows)]
async fn persist_quickbar_visible(visible: bool) {
    if let Ok(mut client) = connect().await {
        let patch = okkhor_ipc::settings::SettingsPatch {
            quickbar_visible: Some(visible),
            ..Default::default()
        };
        let _ = client.settings_update(&patch).await;
    }
}

/// Hide the bar to the tray. The process (and its tray) stays alive; the tray
/// menu's "Show quick bar" signals this process back to visibility.
#[tauri::command]
async fn quickbar_hide(app: AppHandle) -> Result<(), UiError> {
    let Some(bar) = app.get_webview_window("quickbar") else {
        return Err(UiError::internal());
    };
    // The bar was shown with raw ShowWindow, so Tauri's visibility state can
    // still read "hidden" and make `bar.hide()` a no-op. Hide natively.
    #[cfg(windows)]
    if let Ok(hwnd) = bar.hwnd() {
        quickbar::native::hide(hwnd);
    }
    #[cfg(not(windows))]
    bar.hide().map_err(|_| UiError::internal())?;
    #[cfg(windows)]
    persist_quickbar_visible(false).await;
    Ok(())
}

/// Show the bar without activating it, restoring it to the primary monitor if
/// it was left off-screen (monitor change, resolution change).
#[tauri::command]
async fn quickbar_show(app: AppHandle) -> Result<(), UiError> {
    let Some(bar) = app.get_webview_window("quickbar") else {
        return Err(UiError::internal());
    };
    if !bar_is_on_screen(&bar) {
        place_quickbar_top_right(&bar);
    } else if let Ok(position) = bar.outer_position() {
        let clamped = clamp_bar_position(&bar, position);
        if clamped != position {
            let _ = bar.set_position(tauri::Position::Physical(clamped));
        }
    }
    #[cfg(windows)]
    show_bar_noactivate(&bar);
    #[cfg(not(windows))]
    bar.show().map_err(|_| UiError::internal())?;
    #[cfg(windows)]
    persist_quickbar_visible(true).await;
    Ok(())
}

/// Clamp a physical position so the whole bar stays on a monitor. The bar can
/// be dragged to an edge; this keeps every control reachable instead of
/// leaving the right buttons past the screen edge.
fn clamp_bar_position(
    bar: &tauri::WebviewWindow,
    position: tauri::PhysicalPosition<i32>,
) -> tauri::PhysicalPosition<i32> {
    let Ok(size) = bar.outer_size() else {
        return position;
    };
    let Ok(monitors) = bar.available_monitors() else {
        return position;
    };
    let right = i64::from(position.x) + i64::from(size.width);
    let bottom = i64::from(position.y) + i64::from(size.height);
    let mut best: Option<(&tauri::Monitor, i64)> = None;
    for monitor in &monitors {
        let origin = monitor.position();
        let monitor_size = monitor.size();
        let overlap_w = (right.min(i64::from(origin.x) + i64::from(monitor_size.width))
            - i64::from(position.x).max(i64::from(origin.x)))
        .max(0);
        let overlap_h = (bottom.min(i64::from(origin.y) + i64::from(monitor_size.height))
            - i64::from(position.y).max(i64::from(origin.y)))
        .max(0);
        let overlap = overlap_w * overlap_h;
        if best.is_none_or(|(_, best_overlap)| overlap > best_overlap) {
            best = Some((monitor, overlap));
        }
    }
    let Some((monitor, _)) = best else {
        return position;
    };
    let origin = monitor.position();
    let monitor_size = monitor.size();
    let max_x = (i64::from(origin.x) + i64::from(monitor_size.width))
        .saturating_sub(i64::from(size.width))
        .max(i64::from(origin.x));
    let max_y = (i64::from(origin.y) + i64::from(monitor_size.height))
        .saturating_sub(i64::from(size.height))
        .max(i64::from(origin.y));
    tauri::PhysicalPosition {
        x: i64::from(position.x).clamp(i64::from(origin.x), max_x) as i32,
        y: i64::from(position.y).clamp(i64::from(origin.y), max_y) as i32,
    }
}

/// True when at least the top-left corner of the bar lands on a monitor.
fn bar_is_on_screen(bar: &tauri::WebviewWindow) -> bool {
    let Ok(position) = bar.outer_position() else {
        return false;
    };
    let Ok(monitors) = bar.available_monitors() else {
        return false;
    };
    monitors.iter().any(|monitor| {
        let origin = monitor.position();
        let size = monitor.size();
        let x = i64::from(position.x);
        let y = i64::from(position.y);
        x >= i64::from(origin.x)
            && x < i64::from(origin.x) + i64::from(size.width)
            && y >= i64::from(origin.y)
            && y < i64::from(origin.y) + i64::from(size.height)
    })
}

/// Begin a native horizontal resize at the current logical width.
#[tauri::command]
async fn quickbar_begin_resize(app: AppHandle, width: u32) -> Result<(), UiError> {
    let Some(bar) = app.get_webview_window("quickbar") else {
        return Err(UiError::internal());
    };
    #[cfg(windows)]
    if let Ok(hwnd) = bar.hwnd() {
        quickbar::native::begin_resize(hwnd, width);
    }
    let _ = (&bar, width);
    Ok(())
}

/// Persist the bar's current physical position after a drag.
#[tauri::command]
async fn quickbar_save_position(app: AppHandle) -> Result<(), UiError> {
    let Some(bar) = app.get_webview_window("quickbar") else {
        return Err(UiError::internal());
    };
    let position = bar.outer_position().map_err(|_| UiError::internal())?;
    let position = clamp_bar_position(&bar, position);
    let _ = bar.set_position(tauri::Position::Physical(position));
    #[cfg(windows)]
    {
        if let Ok(mut client) = connect().await {
            let patch = okkhor_ipc::settings::SettingsPatch {
                quickbar_x: Some(Some(position.x)),
                quickbar_y: Some(Some(position.y)),
                ..Default::default()
            };
            let _ = client.settings_update(&patch).await;
        }
    }
    let _ = position;
    Ok(())
}

/// Quit Okkhor from the bar: ask the daemon to exit cleanly, then terminate
/// this UI process. A watchdog guarantees the bar closes even if the IPC call
/// stalls; works when the daemon is already down.
#[tauri::command]
async fn quickbar_quit() -> Result<(), UiError> {
    // Watchdog: a stuck pipe must never leave an unclosable bar behind.
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(3));
        std::process::exit(0);
    });
    #[cfg(windows)]
    std::thread::spawn(|| {
        tauri::async_runtime::block_on(async {
            if let Ok(mut client) = connect().await {
                let _ = client.shutdown().await;
            }
        });
        std::process::exit(0);
    });
    #[cfg(not(windows))]
    std::process::exit(0);
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
        // Resize preserves the ex-styles, but re-assert them as cheap
        // insurance: the bar must never become activatable.
        if let Ok(hwnd) = bar.hwnd() {
            quickbar::native::apply_noactivate(hwnd);
        }
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
                            visible: settings.quickbar_visible,
                            x: settings.quickbar_x,
                            y: settings.quickbar_y,
                        });
                    }
                }
            }
        }
        // Daemon down: the resize above still stands; report local truth.
        // Opacity falls back to the contract default — the bar keeps the CSS
        // value it already renders (follow-up: return last-known opacity).
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

/// Show the bar without ever activating it. Callers must have applied
/// [`quickbar::native::apply_noactivate`] and positioned the window first;
/// this only flips visibility, so no frame can show before the styles land.
#[cfg(windows)]
fn show_bar_noactivate(bar: &tauri::WebviewWindow) {
    if let Ok(hwnd) = bar.hwnd() {
        quickbar::native::apply_noactivate(hwnd);
        quickbar::native::show_noactivate(hwnd);
    }
}

/// Apply the persisted collapsed size to a still-hidden bar (normal-launch
/// path), keeping the setup-time position.
fn apply_bar_chrome(bar: &tauri::WebviewWindow, collapsed: bool) {
    let (w, h) = if collapsed {
        (quickbar::ORB_LOGICAL, quickbar::ORB_LOGICAL)
    } else {
        (quickbar::QUICKBAR_LOGICAL_W, quickbar::QUICKBAR_LOGICAL_H)
    };
    let _ = bar.set_size(tauri::LogicalSize {
        width: f64::from(w),
        height: f64::from(h),
    });
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            // Both windows start `visible: false` in `tauri.conf.json`, so no
            // frame can show — and no window can activate — before its styles
            // are applied below. Exactly one show path runs per launch.
            let args: Vec<String> = std::env::args().collect();
            let bar_only = quickbar::is_quickbar_launch(&args);
            if bar_only {
                // Bar-only launch: style + position first, then show the bar
                // without activating. `main` is never shown.
                if let Some(bar) = app.get_webview_window("quickbar") {
                    place_quickbar_top_right(&bar);
                    #[cfg(windows)]
                    show_bar_noactivate(&bar);
                    #[cfg(not(windows))]
                    {
                        let _ = bar.show();
                    }
                }
                // The saved drag position (if any) arrives after the settings
                // round trip; apply it without hiding the bar again.
                #[cfg(windows)]
                {
                    let handle = app.handle().clone();
                    tauri::async_runtime::spawn(async move {
                        let state = load_quickbar_state().await.ok();
                        let saved = state.as_ref().and_then(|s| s.x.zip(s.y));
                        if let (Some((x, y)), Some(bar)) =
                            (saved, handle.get_webview_window("quickbar"))
                        {
                            let _ = bar.set_position(tauri::Position::Physical(
                                tauri::PhysicalPosition { x, y },
                            ));
                        }
                    });
                }
            } else {
                // Settings launch: `main` shows and focuses normally …
                if let Some(main) = app.get_webview_window("main") {
                    let _ = main.show();
                    let _ = main.set_focus();
                }
                // … and the bar follows the `quickbar_visible` setting,
                // resolved asynchronously once the settings load.
                // (Windows-only: `load_quickbar_state` needs the IPC client.)
                #[cfg(windows)]
                {
                    let handle = app.handle().clone();
                    tauri::async_runtime::spawn(async move {
                        let state = load_quickbar_state().await.ok();
                        if let Some(bar) = handle.get_webview_window("quickbar") {
                            let collapsed = state.as_ref().is_some_and(|s| s.collapsed);
                            // A dragged position wins when it is still on a
                            // monitor; otherwise hug the top-right corner.
                            // Apply it even while hidden so the tray show path
                            // never reveals the Tauri default cascade position.
                            let saved = state.as_ref().and_then(|s| s.x.zip(s.y));
                            if let Some((x, y)) = saved {
                                let _ = bar.set_position(tauri::Position::Physical(
                                    tauri::PhysicalPosition { x, y },
                                ));
                                // Do not clamp here: the saved width is applied
                                // by the bar page just after mount, and
                                // clamping with the default 460px width would
                                // shift a legitimately wider saved position.
                            }
                            if saved.is_none() || !bar_is_on_screen(&bar) {
                                place_quickbar_top_right(&bar);
                            }
                            apply_bar_chrome(&bar, collapsed);
                            if state.as_ref().is_some_and(|s| s.visible) {
                                show_bar_noactivate(&bar);
                            }
                        }
                    });
                }
            }
            // The quick bar can be hidden to the tray while this process keeps
            // running. The daemon signals a session-local event; forward it to
            // the bar webview, which shows itself no-activate on receipt.
            #[cfg(windows)]
            {
                let handle = app.handle().clone();
                let _ = okkhor_win::spawn_quickbar_show_listener(move || {
                    let _ = handle.emit("okkhor:quickbar-show", ());
                });
            }
            {
                let handle = app.handle().clone();
                let _ = okkhor_win::spawn_state_change_listener(move || {
                    let _ = handle.emit("okkhor:state-changed", ());
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            daemon_ensure,
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
            quickbar_open_keyboard,
            quickbar_collapse,
            quickbar_expand,
            quickbar_hide,
            quickbar_show,
            quickbar_begin_resize,
            quickbar_save_position,
            quickbar_quit
        ])
        .run(tauri::generate_context!())
        .expect("error while running Okkhor settings UI");
}

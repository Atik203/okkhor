//! Daemon IPC server wiring and request handlers (contract §4, ADR 0012 §2).
//!
//! This module owns everything the settings UI needs from the daemon:
//!
//! - [`DaemonState`]: settings + words stores behind a mutex, lock-free live
//!   mirrors (atomics) for the key path, a [`TextSink`] for explicit UI
//!   insertion, and a 20/s insert rate limiter.
//! - [`handle_request`]: one handler implementing contract §4 exactly
//!   (`status`, `settings.get/update`, `layouts.list`, `words.*`,
//!   `text.insert`, `capture.set`; `ai.complete` answers `Unsupported`).
//! - [`IpcServer`]: token create/rotate/remove plus the pipe server on its own
//!   thread and runtime (Windows only).
//! - [`find_ui_exe`] / [`launch_ui`]: tray Settings action (detached spawn).
//!
//! Boundaries (binding):
//!
//! - The hook/worker key path never touches this module's mutexes with a
//!   blocking lock and never performs disk I/O here. It reads atomics only.
//! - Disk I/O (settings/words persistence, token file) happens on the IPC
//!   handler blocking threads, the daemon loop's 10 ms poll (rare control
//!   actions only), or short-lived persist threads — never per keystroke.
//! - The injection [`Mutex`] is shared with the commit path: the commit holds
//!   it across erase+inject (blocking lock, always wins); a UI insert uses
//!   `try_lock` and fails with `Internal` rather than interleaving.
//! - No user text, tokens, or paths are logged or rendered in `Debug` output.
//!   Errors are allowlisted [`ErrorCode`]s with static messages only.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use okkhor_ipc::error::ErrorCode;
use okkhor_ipc::protocol::{
    LayoutInfo, Request, Response, Status, TextInserted, WordAdd, WordRemoveResult,
};

use crate::ai_proxy;
use okkhor_ipc::settings::{InputMode, OutputEncoding, Settings, SettingsPatch};
use okkhor_storage::Protector;
use thiserror::Error;

use crate::settings_store::{SettingsStore, SettingsStoreError};
use crate::words_store::{WordsStore, WordsStoreError};

/// `text.insert` budget: 20 requests per rolling second (explicit UI action).
pub const INSERT_RATE_LIMIT: u32 = 20;
/// Rolling window for [`INSERT_RATE_LIMIT`].
pub const INSERT_RATE_WINDOW: Duration = Duration::from_secs(1);

/// Inserts explicit UI text into the foreground application.
///
/// Production uses the same injector as the commit path; tests define their
/// own recorders against this trait (see the unit tests below and the
/// `ipc_roundtrip` integration test). Failures map to [`ErrorCode::Internal`].
pub trait TextSink: Send + Sync + 'static {
    fn insert_text(&self, text: &str) -> Result<(), ErrorCode>;
}

/// Production sink: the shared Win32 injector (Unicode `SendInput` first,
/// `WM_CHAR` fast path / clipboard fallback per [`okkhor_win`]).
#[cfg(windows)]
#[derive(Debug, Default)]
pub struct ProductionSink;

#[cfg(windows)]
impl TextSink for ProductionSink {
    fn insert_text(&self, text: &str) -> Result<(), ErrorCode> {
        use okkhor_win::{TextInjector as _, WindowsInjector};
        WindowsInjector
            .inject(text)
            .map(|_| ())
            .map_err(|_| ErrorCode::Internal)
    }
}

/// Test sinks live in the test modules: the unit tests below and the
/// `ipc_roundtrip` integration test each define their own recorder against
/// [`TextSink`], so the daemon binary ships no test-only construction paths.
/// Sliding-window insert rate limiter (contract guard: 20/s for `text.insert`).
#[derive(Debug)]
pub struct InsertLimiter {
    max: u32,
    window: Duration,
    hits: Mutex<VecDeque<Instant>>,
}

impl InsertLimiter {
    /// Budget of `max` allows per `window`.
    pub fn new(max: u32, window: Duration) -> Self {
        Self {
            max,
            window,
            hits: Mutex::new(VecDeque::new()),
        }
    }

    /// The production `text.insert` budget (20/s).
    #[must_use]
    pub fn ui_default() -> Self {
        Self::new(INSERT_RATE_LIMIT, INSERT_RATE_WINDOW)
    }

    /// Record one attempt: `true` when inside the budget, `false` when the
    /// window is exhausted (the attempt is not recorded).
    pub fn allow(&self) -> bool {
        let mut hits = self.hits.lock().unwrap_or_else(|error| error.into_inner());
        let now = Instant::now();
        while let Some(front) = hits.front() {
            if now.saturating_duration_since(*front) >= self.window {
                hits.pop_front();
            } else {
                break;
            }
        }
        if hits.len() >= self.max as usize {
            return false;
        }
        hits.push_back(now);
        true
    }
}

/// Live-apply plan for a [`SettingsPatch`]: which set fields take effect by
/// persisting alone versus which need a follow-up.
///
/// - `immediate`: persisted = applied (the daemon mirrors the ones it owns —
///   input mode, output encoding, popup gate, learning flag — into atomics).
/// - `deferred`: hotkeys (need a hotkey-thread re-registration the IPC handler
///   cannot perform) and fonts (validated here, installed out of scope).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LivePlan {
    pub immediate: Vec<&'static str>,
    pub deferred: Vec<&'static str>,
}

/// Classify every set field of `patch` as immediate or deferred.
///
/// Pure: no I/O, no locks, no threads. The field names are the stable settings
/// keys (never user text).
#[must_use]
pub fn live_plan(patch: &SettingsPatch) -> LivePlan {
    let mut plan = LivePlan::default();
    macro_rules! flag {
        ($option:expr, $name:literal, $deferred:expr) => {
            if $option.is_some() {
                if $deferred {
                    plan.deferred.push($name);
                } else {
                    plan.immediate.push($name);
                }
            }
        };
    }
    flag!(patch.language, "language", false);
    flag!(patch.layout, "layout", false);
    flag!(patch.input_mode, "input_mode", false);
    flag!(patch.output_encoding, "output_encoding", false);
    flag!(patch.theme, "theme", false);
    flag!(patch.accent, "accent", false);
    flag!(patch.font_scale, "font_scale", false);
    flag!(patch.font_family, "font_family", false);
    flag!(patch.font_unicode, "font_unicode", true);
    flag!(patch.font_bijoy, "font_bijoy", true);
    flag!(patch.popup_enabled, "popup_enabled", false);
    flag!(patch.popup_max_items, "popup_max_items", false);
    flag!(patch.english_suggestions, "english_suggestions", false);
    flag!(patch.inline_preedit, "inline_preedit", false);
    flag!(patch.toggle_hotkey, "toggle_hotkey", true);
    flag!(patch.pause_hotkey, "pause_hotkey", true);
    flag!(patch.show_toggle_toast, "show_toggle_toast", false);
    flag!(patch.quickbar_visible, "quickbar_visible", false);
    flag!(patch.quickbar_collapsed, "quickbar_collapsed", false);
    flag!(patch.quickbar_x, "quickbar_x", false);
    flag!(patch.quickbar_y, "quickbar_y", false);
    flag!(patch.quickbar_opacity, "quickbar_opacity", false);
    flag!(patch.autostart, "autostart", false);
    flag!(patch.learning_enabled, "learning_enabled", false);
    flag!(
        patch.word_manager_page_size,
        "word_manager_page_size",
        false
    );
    flag!(patch.ai_enabled, "ai_enabled", false);
    flag!(patch.speech_enabled, "speech_enabled", false);
    plan
}

/// Outcome of mapping a validated patch onto live daemon state.
///
/// `Applied` means every changed field took effect. `Deferred` means the patch
/// was persisted and validated, but the listed fields need a follow-up
/// (hotkey re-registration that failed or is pending, font install). The
/// daemon never claims a field is live when it is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveApply {
    Applied,
    Deferred { fields: Vec<String> },
}

impl LiveApply {
    fn from_deferred(deferred: Vec<String>) -> Self {
        if deferred.is_empty() {
            Self::Applied
        } else {
            Self::Deferred { fields: deferred }
        }
    }
}

/// Daemon state shared between the IPC handler and the daemon loop.
///
/// The stores sit behind one mutex (IPC rate only — never the key path). The
/// loop reads the atomics lock-free; disk I/O happens on IPC blocking threads
/// or rare control actions only.
pub struct DaemonState<P> {
    settings: Mutex<SettingsStore>,
    words: Mutex<WordsStore<P>>,
    sink: Arc<dyn TextSink>,
    injection: Arc<Mutex<()>>,
    limiter: InsertLimiter,
    version: String,
    paused: AtomicBool,
    popup_visible: AtomicBool,
    mode: AtomicU8,
    encoding: AtomicU8,
    popup_enabled: AtomicBool,
    popup_max_items: AtomicU8,
    toggle_sound: AtomicBool,
    learning: AtomicBool,
    english_suggestions: AtomicBool,
    inline_preedit: AtomicBool,
    words_revision: AtomicU64,
    shutdown_requested: AtomicBool,
    rebind_requested: AtomicBool,
    apply_seq: AtomicU64,
    last_apply: Mutex<LiveApply>,
    store_failures: AtomicU64,
    /// (roman, output) of the last inline commit, for `words.add_last`.
    last_commit: Mutex<Option<(String, String)>>,
    ai: Arc<ai_proxy::AiProxy>,
}

// Manual `Debug`: stores already redact paths and text; the sink is
// intentionally opaque (a recorder could hold user text in tests).
impl<P> std::fmt::Debug for DaemonState<P> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DaemonState")
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

/// State-open failures. Messages are static; they never include paths.
///
/// Allowed dead in the daemon binary: constructed by [`DaemonState::open`],
/// which the `ipc_roundtrip` integration test drives (the lint cannot see
/// cross-target use).
#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum OpenError {
    /// The settings store failed to open (I/O, corruption, schema, values).
    #[error("settings store failed to open")]
    Settings(#[from] SettingsStoreError),
    /// The word store failed to open (storage, corruption, schema).
    #[error("word store failed to open")]
    Words(#[from] WordsStoreError),
}

const MODE_BANGLA: u8 = 0;
const MODE_ENGLISH: u8 = 1;
const ENCODING_UNICODE: u8 = 0;
const ENCODING_BIJOY: u8 = 1;

fn mode_to_u8(mode: InputMode) -> u8 {
    match mode {
        InputMode::Bangla => MODE_BANGLA,
        InputMode::English => MODE_ENGLISH,
    }
}

fn mode_from_u8(value: u8) -> InputMode {
    match value {
        MODE_ENGLISH => InputMode::English,
        _ => InputMode::Bangla,
    }
}

fn encoding_to_u8(encoding: OutputEncoding) -> u8 {
    match encoding {
        OutputEncoding::Unicode => ENCODING_UNICODE,
        OutputEncoding::Bijoy => ENCODING_BIJOY,
    }
}

fn encoding_from_u8(value: u8) -> OutputEncoding {
    match value {
        ENCODING_BIJOY => OutputEncoding::Bijoy,
        _ => OutputEncoding::Unicode,
    }
}

impl<P: Protector> DaemonState<P> {
    /// Open the stores at explicit paths. The daemon passes its
    /// [`crate::paths`] locations (via the default loaders and
    /// [`DaemonState::from_stores`]); tests pass temp dirs.
    ///
    /// Allowed dead in the daemon binary: the `ipc_roundtrip` integration
    /// test drives this constructor (the lint cannot see cross-target use).
    #[allow(dead_code)]
    pub fn open(
        settings_path: &Path,
        words_path: &Path,
        protector: P,
        sink: Arc<dyn TextSink>,
        injection: Arc<Mutex<()>>,
    ) -> Result<Self, OpenError> {
        let settings = SettingsStore::load(settings_path)?;
        let words = WordsStore::open(words_path, protector)?;
        Ok(Self::from_stores(settings, words, sink, injection))
    }

    /// Build shared state from already-open stores (the daemon's default
    /// locations, or temp dirs in tests via [`DaemonState::open`]).
    pub fn from_stores(
        settings: SettingsStore,
        words: WordsStore<P>,
        sink: Arc<dyn TextSink>,
        injection: Arc<Mutex<()>>,
    ) -> Self {
        let current = settings.settings().clone();
        let ai = ai_proxy::AiProxy::new(current.ai_enabled);
        Self {
            mode: AtomicU8::new(mode_to_u8(current.input_mode)),
            encoding: AtomicU8::new(encoding_to_u8(current.output_encoding)),
            popup_enabled: AtomicBool::new(current.popup_enabled),
            popup_max_items: AtomicU8::new(current.popup_max_items),
            toggle_sound: AtomicBool::new(current.toggle_sound),
            learning: AtomicBool::new(current.learning_enabled),
            english_suggestions: AtomicBool::new(current.english_suggestions),
            inline_preedit: AtomicBool::new(current.inline_preedit),
            words_revision: AtomicU64::new(0),
            shutdown_requested: AtomicBool::new(false),
            settings: Mutex::new(settings),
            words: Mutex::new(words),
            sink,
            injection,
            limiter: InsertLimiter::ui_default(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            paused: AtomicBool::new(false),
            popup_visible: AtomicBool::new(false),
            rebind_requested: AtomicBool::new(false),
            apply_seq: AtomicU64::new(0),
            last_apply: Mutex::new(LiveApply::Applied),
            store_failures: AtomicU64::new(0),
            last_commit: Mutex::new(None),
            ai,
        }
    }

    /// Remember the (roman, output) pair of the last inline commit so the quick
    /// bar can add it as a custom mapping with `words.add_last`.
    pub fn remember_commit(&self, roman: &str, output: &str) {
        if let Ok(mut guard) = self.last_commit.lock() {
            *guard = Some((roman.to_owned(), output.to_owned()));
        }
    }

    /// Take the remembered pair; a second call without a new commit is empty.
    pub fn take_last_commit(&self) -> Option<(String, String)> {
        self.last_commit
            .lock()
            .ok()
            .and_then(|mut guard| guard.take())
    }

    fn lock_settings(&self) -> MutexGuard<'_, SettingsStore> {
        self.settings
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn lock_words(&self) -> MutexGuard<'_, WordsStore<P>> {
        self.words.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Current validated settings (clone under a brief IPC-rate lock).
    pub fn current_settings(&self) -> Settings {
        self.lock_settings().settings().clone()
    }

    /// Daemon-owned AI job proxy. Never called from the key path.
    pub fn ai(&self) -> &Arc<ai_proxy::AiProxy> {
        &self.ai
    }

    /// Live input mode mirror (lock-free; the key path reads this).
    pub fn mode(&self) -> InputMode {
        mode_from_u8(self.mode.load(Ordering::SeqCst))
    }

    /// Live output encoding mirror (lock-free).
    pub fn encoding(&self) -> OutputEncoding {
        encoding_from_u8(self.encoding.load(Ordering::SeqCst))
    }

    /// Live capture-pause flag. The loop syncs the hook from this.
    pub fn paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// Set the pause flag (IPC `capture.set`, tray, hotkey).
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
    }

    /// Live input-mode mirror for local (tray/hotkey) toggles.
    pub fn set_mode_live(&self, mode: InputMode) {
        self.mode.store(mode_to_u8(mode), Ordering::SeqCst);
    }

    /// Whether the suggestion popup is currently shown.
    pub fn popup_visible(&self) -> bool {
        self.popup_visible.load(Ordering::SeqCst)
    }

    /// Record popup visibility (daemon loop only).
    pub fn set_popup_visible(&self, visible: bool) {
        self.popup_visible.store(visible, Ordering::SeqCst);
    }

    /// Whether the popup may show (live mirror of the setting).
    pub fn popup_enabled(&self) -> bool {
        self.popup_enabled.load(Ordering::SeqCst)
    }

    /// Whether a mode change plays the feedback click (live mirror).
    pub fn toggle_sound(&self) -> bool {
        self.toggle_sound.load(Ordering::SeqCst)
    }

    /// Rows the popup may show (live mirror of `popup_max_items`, 1-10).
    pub fn popup_max_items(&self) -> usize {
        usize::from(self.popup_max_items.load(Ordering::SeqCst)).clamp(1, 10)
    }

    /// Whether an authenticated client asked the daemon to exit.
    pub fn shutdown_requested(&self) -> bool {
        self.shutdown_requested.load(Ordering::SeqCst)
    }

    /// Whether learning commits are queued (live mirror of the setting).
    pub fn learning(&self) -> bool {
        self.learning.load(Ordering::SeqCst)
    }

    /// Whether English prefix completions are enabled.
    pub fn english_suggestions(&self) -> bool {
        self.english_suggestions.load(Ordering::SeqCst)
    }

    /// Whether Bangla is rendered in the document per keystroke.
    pub fn inline_preedit(&self) -> bool {
        self.inline_preedit.load(Ordering::SeqCst)
    }

    /// Monotonic counter bumped by every words mutation.
    pub fn words_revision(&self) -> u64 {
        self.words_revision.load(Ordering::Relaxed)
    }

    /// Bangla user words for the key-path suggestion snapshot.
    pub fn user_words(&self) -> Vec<okkhor_engine::UserWord> {
        self.lock_words()
            .snapshot()
            .into_iter()
            .filter(|word| word.language == okkhor_ipc::words::WordLanguage::Bn)
            .map(|word| okkhor_engine::UserWord {
                roman: word.roman,
                bangla: word.output,
            })
            .collect()
    }

    /// Desired hotkey chords, or `None` when the settings lock is contended
    /// (the caller re-arms and retries on the next poll — never blocks).
    pub fn hotkey_chords(&self) -> Option<(String, String)> {
        let settings = self.settings.try_lock().ok()?;
        let current = settings.settings();
        Some((current.toggle_hotkey.clone(), current.pause_hotkey.clone()))
    }

    /// Whether a hotkey re-registration is pending; consumes the flag.
    pub fn take_rebind_request(&self) -> bool {
        self.rebind_requested.swap(false, Ordering::SeqCst)
    }

    /// Re-arm a deferred re-registration attempt.
    pub fn request_rebind(&self) {
        self.rebind_requested.store(true, Ordering::SeqCst);
    }

    /// Monotonic count of successful settings applications.
    pub fn apply_seq(&self) -> u64 {
        self.apply_seq.load(Ordering::SeqCst)
    }

    /// Outcome of the most recent settings application.
    pub fn last_apply(&self) -> LiveApply {
        self.lock_apply().clone()
    }

    fn lock_apply(&self) -> MutexGuard<'_, LiveApply> {
        self.last_apply
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    /// Count of store persist-side failures (I/O, data home, storage). The
    /// daemon loop watches this and logs one allowlisted code per increase.
    pub fn store_failures(&self) -> u64 {
        self.store_failures.load(Ordering::SeqCst)
    }

    /// Mark hotkey fields live after the loop re-registers them.
    pub fn note_rebound(&self, toggle_ok: bool, pause_ok: bool) {
        let mut apply = self.lock_apply();
        if let LiveApply::Deferred { fields } = &mut *apply {
            fields.retain(|field| {
                !((field == "toggle_hotkey" && toggle_ok) || (field == "pause_hotkey" && pause_ok))
            });
            if fields.is_empty() {
                *apply = LiveApply::Applied;
            }
        }
    }

    /// Merge `patch`, persist atomically, mirror the live atomics, and attempt
    /// the handler-safe live applies (autostart). Hotkey chords cannot be
    /// re-registered from the IPC thread, so they stay deferred with
    /// [`Self::take_rebind_request`] armed for the daemon loop.
    ///
    /// Runs on IPC blocking threads or short-lived persist threads — never on
    /// the key path.
    pub fn update_settings(
        &self,
        patch: &SettingsPatch,
    ) -> Result<(Settings, LiveApply), SettingsStoreError> {
        let previous_mode = self.mode();
        let merged = match self.lock_settings().update(patch) {
            Ok(merged) => merged,
            Err(error) => {
                if matches!(
                    error,
                    SettingsStoreError::Io(_) | SettingsStoreError::NoDataHome
                ) {
                    self.store_failures.fetch_add(1, Ordering::SeqCst);
                }
                return Err(error);
            }
        };
        self.mode
            .store(mode_to_u8(merged.input_mode), Ordering::SeqCst);
        self.encoding
            .store(encoding_to_u8(merged.output_encoding), Ordering::SeqCst);
        self.popup_enabled
            .store(merged.popup_enabled, Ordering::SeqCst);
        self.popup_max_items
            .store(merged.popup_max_items, Ordering::SeqCst);
        self.toggle_sound
            .store(merged.toggle_sound, Ordering::SeqCst);
        self.learning
            .store(merged.learning_enabled, Ordering::SeqCst);
        self.english_suggestions
            .store(merged.english_suggestions, Ordering::SeqCst);
        self.inline_preedit
            .store(merged.inline_preedit, Ordering::SeqCst);
        self.ai.set_enabled(merged.ai_enabled);
        // The bar toggles mode through `settings.update`; the hotkey and tray
        // use the daemon-local path. Play the same click for both so feedback
        // is consistent, and only when the mode actually changed.
        if merged.input_mode != previous_mode && merged.toggle_sound {
            okkhor_win::play_toggle_click();
        }

        let plan = live_plan(patch);
        let mut deferred: Vec<String> = plan
            .deferred
            .iter()
            .map(|field| (*field).to_owned())
            .collect();
        if patch.autostart.is_some() && !self.apply_autostart(merged.autostart) {
            deferred.push("autostart".to_owned());
        }
        if patch.toggle_hotkey.is_some() || patch.pause_hotkey.is_some() {
            self.request_rebind();
        }
        let outcome = LiveApply::from_deferred(deferred);
        *self.lock_apply() = outcome.clone();
        self.apply_seq.fetch_add(1, Ordering::SeqCst);
        Ok((merged, outcome))
    }

    /// Apply the persisted autostart flag to the OS. `true` also when there is
    /// nothing to do; `false` when the registry write failed (stays deferred).
    #[cfg(windows)]
    fn apply_autostart(&self, autostart: bool) -> bool {
        if autostart {
            let executable = match std::env::current_exe() {
                Ok(executable) => executable,
                Err(_) => return false,
            };
            okkhor_win::enable_autostart(&executable).is_ok()
        } else {
            okkhor_win::disable_autostart().is_ok()
        }
    }

    /// Non-Windows daemons cannot touch the registry: autostart always stays
    /// deferred there (persisted, applied on Windows).
    #[cfg(not(windows))]
    fn apply_autostart(&self, _autostart: bool) -> bool {
        false
    }

    /// Contract §4 `status`.
    pub fn status(&self) -> Status {
        let settings = self.current_settings();
        let words_total = self.lock_words().len() as u64;
        let (language, layout) = match settings.language {
            okkhor_ipc::settings::Language::Bn => ("bn".to_owned(), settings.layout.clone()),
            okkhor_ipc::settings::Language::En => ("en".to_owned(), settings.layout.clone()),
        };
        Status {
            daemon_version: self.version.clone(),
            paused: self.paused(),
            language,
            layout,
            input_mode: settings.input_mode,
            output_encoding: settings.output_encoding,
            popup_visible: self.popup_visible(),
            words_total,
            learning_enabled: settings.learning_enabled,
            autostart: settings.autostart,
        }
    }

    /// One contract §4 request. Synchronous and bounded: the transport runs it
    /// on a blocking thread under the 5 s per-request deadline.
    pub fn handle_request(&self, request: Request) -> Response {
        match request {
            Request::Hello(_) => Response::error(ErrorCode::InvalidRequest),
            Request::Status(_) => Response::Status(self.status()),
            Request::SettingsGet(_) => Response::Settings(self.current_settings()),
            Request::SettingsUpdate(patch) => match self.update_settings(&patch) {
                Ok((settings, _)) => Response::Settings(settings),
                Err(error) => Response::error(error.code()),
            },
            Request::LayoutsList(_) => Response::Layouts(layout_list()),
            Request::WordsList(query) => {
                let page =
                    self.lock_words()
                        .list(query.query.as_deref(), query.offset, query.limit);
                Response::Words(page)
            }
            Request::WordsAdd(add) => match self.lock_words().add(&add) {
                Ok(word) => {
                    self.words_revision.fetch_add(1, Ordering::Relaxed);
                    Response::Word(word)
                }
                Err(error) => {
                    self.note_store_error(&error);
                    Response::error(error.code())
                }
            },
            Request::WordsAddLast(_) => match self.take_last_commit() {
                Some((roman, output)) => {
                    let add = WordAdd {
                        roman,
                        output,
                        language: okkhor_ipc::words::WordLanguage::Bn,
                    };
                    match self.lock_words().add(&add) {
                        Ok(word) => {
                            self.words_revision.fetch_add(1, Ordering::Relaxed);
                            Response::Word(word)
                        }
                        Err(error) => {
                            self.note_store_error(&error);
                            Response::error(error.code())
                        }
                    }
                }
                None => Response::error(ErrorCode::InvalidRequest),
            },
            Request::WordsRemove(remove) => match self.lock_words().remove(remove.id) {
                Ok(removed) => {
                    if removed {
                        self.words_revision.fetch_add(1, Ordering::Relaxed);
                    }
                    Response::Removed(WordRemoveResult { removed })
                }
                Err(error) => {
                    self.note_store_error(&error);
                    Response::error(error.code())
                }
            },
            Request::WordsImport(import) => {
                match self.lock_words().import(import.format, &import.data) {
                    Ok(imported) => {
                        if imported.imported > 0 {
                            self.words_revision.fetch_add(1, Ordering::Relaxed);
                        }
                        Response::Import(imported)
                    }
                    Err(error) => {
                        self.note_store_error(&error);
                        Response::error(error.code())
                    }
                }
            }
            Request::WordsExport(export) => match self.lock_words().export(export.format) {
                Ok(exported) => Response::Export(exported),
                Err(error) => {
                    self.note_store_error(&error);
                    Response::error(error.code())
                }
            },
            Request::TextInsert(insert) => self.insert_text(&insert.text),
            Request::CaptureSet(capture) => {
                self.set_paused(capture.paused);
                Response::Status(self.status())
            }
            Request::Shutdown(_) => {
                self.shutdown_requested.store(true, Ordering::SeqCst);
                Response::Ok(okkhor_ipc::protocol::NoArgs {})
            }
            Request::AiComplete(params) => match self.ai.complete(params) {
                Ok(started) => Response::AiJobStarted(started),
                Err(code) => Response::error(code),
            },
            Request::AiPoll(params) => Response::AiJob(self.ai.poll(params.job_id)),
            Request::AiCancel(params) => {
                self.ai.cancel(params.job_id);
                Response::Ok(okkhor_ipc::protocol::NoArgs {})
            }
            Request::AiStatus(_) => Response::AiStatus(self.ai.status()),
        }
    }

    fn note_store_error(&self, error: &WordsStoreError) {
        if matches!(
            error,
            WordsStoreError::Storage(_) | WordsStoreError::NoDataHome | WordsStoreError::Internal
        ) {
            self.store_failures.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Explicit UI insertion: validate, rate-limit, then inject through the
    /// sink under the shared injection lock (commit wins on contention).
    fn insert_text(&self, text: &str) -> Response {
        if let Err(code) = okkhor_ipc::protocol::validate_insert_text(text) {
            return Response::error(code);
        }
        if !self.limiter.allow() {
            return Response::error(ErrorCode::Timeout);
        }
        let _guard = match self.injection.try_lock() {
            Ok(guard) => guard,
            // A commit is mid erase+inject: never interleave, fail loudly.
            Err(_) => return Response::error(ErrorCode::Internal),
        };
        match self.sink.insert_text(text) {
            Ok(()) => Response::Inserted(TextInserted { inserted: true }),
            Err(code) => Response::error(code),
        }
    }
}

/// Contract §4 `layouts.list`: only the default phonetic style and the three
/// fixed physical-key layouts are advertised. `phonetic` and `khipro` are the
/// same phonetic engine with different rule tables; they remain accepted
/// stored values and keep working, but new users get one clear roman choice
/// instead of three labels that look like different features.
#[must_use]
pub fn layout_list() -> Vec<LayoutInfo> {
    [
        ("avro", "Avro (Phonetic)"),
        ("national", "Bangla (National/Jatiya)"),
        ("probhat", "Probhat"),
        ("bijoy", "Bijoy"),
    ]
    .into_iter()
    .map(|(code, name)| LayoutInfo {
        code: code.to_owned(),
        name: name.to_owned(),
    })
    .collect()
}

#[cfg(windows)]
impl<P: Protector + 'static> okkhor_ipc::transport::Handler for DaemonState<P> {
    fn handle(&self, request: Request) -> Response {
        self.handle_request(request)
    }
}

/// Create (or rotate) the session token file at `path` (ADR 0012 §2).
///
/// A fresh token is generated per call: created when absent, atomically
/// replaced when present. Failures carry no secrets or paths.
pub fn install_token(
    path: &Path,
) -> Result<okkhor_ipc::auth::Token, okkhor_ipc::auth::TokenFileError> {
    use okkhor_ipc::auth::{Token, TokenFileError, create_token_file, replace_token_file};
    let token = Token::generate()?;
    match create_token_file(path, &token) {
        Ok(()) => Ok(token),
        Err(TokenFileError::AlreadyExists) => {
            replace_token_file(path, &token)?;
            Ok(token)
        }
        Err(other) => Err(other),
    }
}

/// Remove the session token file. Missing files are fine (idempotent
/// shutdown); other failures surface to the caller for a loud log code.
pub fn remove_token_file(path: &Path) -> Result<(), okkhor_ipc::auth::TokenFileError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Environment override for the settings UI executable (tests/dev only).
pub const UI_ENV_OVERRIDE: &str = "OKKHOR_UI_PATH";
/// Settings UI file name next to the daemon executable.
pub const UI_EXE_NAME: &str = "okkhor-ui.exe";

/// Locate the settings UI.
///
/// Portable beta layout: the user-facing `Okkhor.exe` sits at the folder root
/// and this engine in `bin\`, so the parent folder is checked too. Dev builds
/// and 0.5.0 folders keep a sibling `okkhor-ui.exe`. `OKKHOR_UI_PATH`
/// overrides every probe. Returns `None` (never a path string in an error)
/// when nothing is a file.
#[must_use]
pub fn find_ui_exe() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(UI_ENV_OVERRIDE) {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    let dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))?;
    [
        dir.join(UI_EXE_NAME),
        dir.join("..").join("Okkhor.exe"),
        dir.join("..").join(UI_EXE_NAME),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

/// Settings UI launch failures. Static messages only — never paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum UiLaunchError {
    /// No UI executable was found next to the daemon.
    #[error("settings UI is not installed")]
    NotFound,
    /// The UI executable failed to spawn.
    #[error("settings UI failed to start")]
    SpawnFailed,
}

/// Launch the settings UI detached (it outlives the launcher and never takes
/// focus from the daemon). Tray Settings and `daemon --ui` share this path.
#[cfg(windows)]
pub fn launch_ui() -> Result<(), UiLaunchError> {
    launch_ui_with_args(&[])
}

/// Launch the UI in bar-only mode (tray "Show quick bar" fallback when no
/// listener is running).
#[cfg(windows)]
pub fn launch_quickbar_ui() -> Result<(), UiLaunchError> {
    launch_ui_with_args(&["--quickbar"])
}

#[cfg(windows)]
fn launch_ui_with_args(args: &[&str]) -> Result<(), UiLaunchError> {
    use std::os::windows::process::CommandExt as _;
    /// The UI must not hold a console handle on the daemon.
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    let exe = find_ui_exe().ok_or(UiLaunchError::NotFound)?;
    let mut command = std::process::Command::new(&exe);
    command.args(args).creation_flags(DETACHED_PROCESS);
    command
        .spawn()
        .map(|_| ())
        .map_err(|_| UiLaunchError::SpawnFailed)
}

/// The settings UI is Windows-only.
#[cfg(not(windows))]
pub fn launch_ui() -> Result<(), UiLaunchError> {
    Err(UiLaunchError::NotFound)
}

/// Pipe server on its own thread and multi-thread runtime (Windows only).
///
/// Created after the token file, stopped before the token is removed. Max 4
/// clients, 10 s read timeout, 5 s per-request deadline, owner-only pipe DACL
/// — all enforced by [`okkhor_ipc::transport`].
#[cfg(windows)]
pub struct IpcServer {
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<Result<(), okkhor_ipc::transport::TransportError>>>,
}

#[cfg(windows)]
impl IpcServer {
    /// Start serving `state` with `token` on the production pipe.
    pub fn start<P: Protector + 'static>(
        state: Arc<DaemonState<P>>,
        token: okkhor_ipc::auth::Token,
    ) -> Result<Self, std::io::Error> {
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let server = Arc::new(okkhor_ipc::transport::Server::new(
            state,
            token,
            env!("CARGO_PKG_VERSION"),
        ));
        let thread = std::thread::Builder::new()
            .name("okkhor-ipc".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()?;
                let result = runtime.block_on(server.run(shutdown_rx));
                // Do not let a lingering client connection delay process exit:
                // in-flight handlers had their chance, and the token is removed
                // by the caller immediately after this join.
                runtime.shutdown_timeout(std::time::Duration::from_millis(200));
                result
            })?;
        Ok(Self {
            shutdown: Some(shutdown_tx),
            thread: Some(thread),
        })
    }

    /// Stop accepting and join the server thread. In-flight requests run to
    /// completion; the token file is removed by the caller afterwards.
    pub fn stop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use okkhor_ipc::protocol::ImportFormat;
    use okkhor_ipc::protocol::{
        CaptureSet, NoArgs, TextInsert, WordAdd, WordsExport, WordsImport, WordsListQuery,
    };
    use okkhor_ipc::settings::Theme;
    use okkhor_storage::FakeProtector;

    /// Test-only recorder. Never touches Win32.
    #[derive(Debug, Default)]
    struct RecorderSink {
        texts: Mutex<Vec<String>>,
    }

    impl RecorderSink {
        fn new() -> Self {
            Self::default()
        }

        fn texts(&self) -> Vec<String> {
            self.texts
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone()
        }

        fn len(&self) -> usize {
            self.texts
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .len()
        }
    }

    impl TextSink for RecorderSink {
        fn insert_text(&self, text: &str) -> Result<(), ErrorCode> {
            self.texts
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(text.to_owned());
            Ok(())
        }
    }

    /// Test-only sink that always refuses, exercising the sink-error mapping.
    #[derive(Debug, Default)]
    struct RefusingSink;

    impl TextSink for RefusingSink {
        fn insert_text(&self, _text: &str) -> Result<(), ErrorCode> {
            Err(ErrorCode::Internal)
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "okkhor-ipc-state-{}-{}-{}-{}",
            name,
            std::process::id(),
            unique,
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn open_state(
        dir: &Path,
        name: &str,
    ) -> (
        DaemonState<FakeProtector>,
        Arc<RecorderSink>,
        Arc<Mutex<()>>,
    ) {
        let sink = Arc::new(RecorderSink::new());
        let injection = Arc::new(Mutex::new(()));
        let state = DaemonState::open(
            &dir.join(format!("{name}-settings.json")),
            &dir.join(format!("{name}-words.bin")),
            FakeProtector,
            sink.clone(),
            Arc::clone(&injection),
        )
        .expect("opens");
        (state, sink, injection)
    }

    #[test]
    fn live_plan_maps_fields() {
        let empty = SettingsPatch::default();
        let plan = live_plan(&empty);
        assert!(plan.immediate.is_empty());
        assert!(plan.deferred.is_empty());

        let patch = SettingsPatch {
            input_mode: Some(InputMode::English),
            output_encoding: Some(OutputEncoding::Bijoy),
            learning_enabled: Some(false),
            autostart: Some(true),
            ..SettingsPatch::default()
        };
        let plan = live_plan(&patch);
        assert_eq!(
            plan.immediate,
            vec![
                "input_mode",
                "output_encoding",
                "autostart",
                "learning_enabled"
            ]
        );
        assert!(plan.deferred.is_empty());

        let patch = SettingsPatch {
            toggle_hotkey: Some("Ctrl+Space".to_owned()),
            pause_hotkey: Some("Ctrl+Alt+Shift+P".to_owned()),
            font_unicode: Some(okkhor_ipc::settings::FontUnicode::Kalpurush),
            theme: Some(Theme::Dark),
            ..SettingsPatch::default()
        };
        let plan = live_plan(&patch);
        assert_eq!(plan.immediate, vec!["theme"]);
        assert_eq!(
            plan.deferred,
            vec!["font_unicode", "toggle_hotkey", "pause_hotkey"]
        );
    }

    #[test]
    fn add_last_needs_a_commit_and_adds_the_pair() {
        let dir = temp_dir("add-last");
        let (state, _sink, _injection) = open_state(&dir, "s");
        assert!(matches!(
            state.handle_request(Request::WordsAddLast(NoArgs {})),
            Response::Error(_)
        ));
        state.remember_commit("kortechi", "করতেছি");
        assert!(matches!(
            state.handle_request(Request::WordsAddLast(NoArgs {})),
            Response::Word(_)
        ));
        assert!(
            matches!(
                state.handle_request(Request::WordsAddLast(NoArgs {})),
                Response::Error(_)
            ),
            "the pair is consumed once"
        );
    }

    #[test]
    fn settings_update_applies_live_mirrors_and_reports_deferred() {
        let dir = temp_dir("apply");
        let (state, _sink, _injection) = open_state(&dir, "s");
        assert_eq!(state.mode(), InputMode::Bangla);

        let (merged, outcome) = state
            .update_settings(&SettingsPatch {
                input_mode: Some(InputMode::English),
                output_encoding: Some(OutputEncoding::Bijoy),
                learning_enabled: Some(false),
                ..SettingsPatch::default()
            })
            .expect("applies");
        assert_eq!(merged.input_mode, InputMode::English);
        assert_eq!(state.mode(), InputMode::English);
        assert_eq!(state.encoding(), OutputEncoding::Bijoy);
        assert!(!state.learning());
        assert_eq!(outcome, LiveApply::Applied);
        assert_eq!(state.last_apply(), LiveApply::Applied);

        let (_, outcome) = state
            .update_settings(&SettingsPatch {
                toggle_hotkey: Some("Ctrl+Space".to_owned()),
                ..SettingsPatch::default()
            })
            .expect("persists hotkey");
        assert_eq!(
            outcome,
            LiveApply::Deferred {
                fields: vec!["toggle_hotkey".to_owned()]
            }
        );
        assert!(state.take_rebind_request());
        assert!(!state.take_rebind_request());
        // A successful rebind clears the field back to Applied.
        state.note_rebound(true, false);
        assert_eq!(state.last_apply(), LiveApply::Applied);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_patch_rejects_without_side_effects() {
        let dir = temp_dir("invalid");
        let (state, _sink, _injection) = open_state(&dir, "s");
        let before = state.current_settings();
        let failures_before = state.store_failures();
        let error = state
            .update_settings(&SettingsPatch {
                layout: Some("nope".to_owned()),
                ..SettingsPatch::default()
            })
            .expect_err("bad layout");
        assert_eq!(error.code(), ErrorCode::InvalidLayout);
        assert_eq!(state.current_settings(), before);
        // Validation errors are user errors, not persist failures.
        assert_eq!(state.store_failures(), failures_before);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rate_limiter_bursts_then_denies_then_recovers() {
        let limiter = InsertLimiter::new(2, Duration::from_millis(60));
        assert!(limiter.allow());
        assert!(limiter.allow());
        assert!(!limiter.allow());
        std::thread::sleep(Duration::from_millis(80));
        assert!(limiter.allow());
    }

    #[test]
    fn production_budget_allows_twenty_per_second() {
        let limiter = InsertLimiter::ui_default();
        for _ in 0..INSERT_RATE_LIMIT {
            assert!(limiter.allow());
        }
        assert!(!limiter.allow());
    }

    #[test]
    fn insert_validates_rate_limits_and_needs_the_lock() {
        let dir = temp_dir("insert");
        let (state, sink, injection) = open_state(&dir, "s");

        // Valid insert reaches the sink.
        let response = state.handle_request(Request::TextInsert(TextInsert {
            text: "abc".to_owned(),
        }));
        assert!(matches!(
            response,
            Response::Inserted(TextInserted { inserted: true })
        ));
        assert_eq!(sink.texts(), vec!["abc".to_owned()]);

        // Over the 512 UTF-16 cap: rejected, budget untouched.
        let response = state.handle_request(Request::TextInsert(TextInsert {
            text: "x".repeat(513),
        }));
        assert_eq!(
            response.error_body().map(|body| body.code),
            Some(ErrorCode::TextTooLong)
        );

        // Commit wins: a held injection lock fails the UI insert loudly.
        let _held = injection.lock().expect("holds");
        let response = state.handle_request(Request::TextInsert(TextInsert {
            text: "blocked".to_owned(),
        }));
        assert_eq!(
            response.error_body().map(|body| body.code),
            Some(ErrorCode::Internal)
        );
        drop(_held);
        let response = state.handle_request(Request::TextInsert(TextInsert {
            text: "after".to_owned(),
        }));
        assert!(matches!(
            response,
            Response::Inserted(TextInserted { inserted: true })
        ));
        assert_eq!(sink.texts(), vec!["abc".to_owned(), "after".to_owned()]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn insert_rate_limit_maps_to_timeout() {
        let dir = temp_dir("ratelimit");
        let (state, sink, _injection) = open_state(&dir, "s");
        for _ in 0..INSERT_RATE_LIMIT {
            let response = state.handle_request(Request::TextInsert(TextInsert {
                text: "x".to_owned(),
            }));
            assert!(matches!(response, Response::Inserted(_)));
        }
        let response = state.handle_request(Request::TextInsert(TextInsert {
            text: "one-too-many".to_owned(),
        }));
        assert_eq!(
            response.error_body().map(|body| body.code),
            Some(ErrorCode::Timeout)
        );
        assert_eq!(sink.len() as u32, INSERT_RATE_LIMIT);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sink_failure_maps_to_internal() {
        let dir = temp_dir("sinkfail");
        let injection = Arc::new(Mutex::new(()));
        let state = DaemonState::open(
            &dir.join("settings.json"),
            &dir.join("words.bin"),
            FakeProtector,
            Arc::new(RefusingSink),
            injection,
        )
        .expect("opens");
        let response = state.handle_request(Request::TextInsert(TextInsert {
            text: "x".to_owned(),
        }));
        assert_eq!(
            response.error_body().map(|body| body.code),
            Some(ErrorCode::Internal)
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn status_words_capture_and_layouts() {
        let dir = temp_dir("status");
        let (state, _sink, _injection) = open_state(&dir, "s");

        let status = state.status();
        assert_eq!(status.language, "bn");
        assert_eq!(status.layout, "avro");
        assert_eq!(status.input_mode, InputMode::Bangla);
        assert_eq!(status.output_encoding, OutputEncoding::Unicode);
        assert!(!status.paused);
        assert!(!status.popup_visible);
        assert_eq!(status.words_total, 0);
        assert!(status.learning_enabled);
        assert!(!status.autostart);

        // layouts.list advertises only the layout wired to the engine.
        let layouts = layout_list();
        assert_eq!(
            layouts
                .iter()
                .map(|entry| entry.code.as_str())
                .collect::<Vec<_>>(),
            vec!["avro", "national", "probhat", "bijoy"]
        );
        assert!(layouts.iter().all(|entry| !entry.name.is_empty()));

        // words.add → status count follows.
        let word = match state.handle_request(Request::WordsAdd(WordAdd {
            roman: "amar".to_owned(),
            output: "আমার".to_owned(),
            language: okkhor_ipc::words::WordLanguage::Bn,
        })) {
            Response::Word(word) => word,
            other => panic!("add failed: {other:?}"),
        };
        assert_eq!(state.status().words_total, 1);

        // Duplicate triple is rejected.
        let response = state.handle_request(Request::WordsAdd(WordAdd {
            roman: "amar".to_owned(),
            output: "আমার".to_owned(),
            language: okkhor_ipc::words::WordLanguage::Bn,
        }));
        assert_eq!(
            response.error_body().map(|body| body.code),
            Some(ErrorCode::DuplicateWord)
        );

        // Search + pagination through the handler.
        let page = match state.handle_request(Request::WordsList(WordsListQuery {
            query: Some("ama".to_owned()),
            offset: 0,
            limit: 50,
        })) {
            Response::Words(page) => page,
            other => panic!("list failed: {other:?}"),
        };
        assert_eq!(page.total, 1);

        // capture.set flips paused and reports it back.
        let status = match state.handle_request(Request::CaptureSet(CaptureSet { paused: true })) {
            Response::Status(status) => status,
            other => panic!("capture.set failed: {other:?}"),
        };
        assert!(status.paused);
        assert!(state.paused());

        // words.remove then capture off.
        let removed =
            match state.handle_request(Request::WordsRemove(okkhor_ipc::protocol::WordRemove {
                id: word.id,
            })) {
                Response::Removed(result) => result.removed,
                other => panic!("remove failed: {other:?}"),
            };
        assert!(removed);
        state.set_paused(false);
        let status = match state.handle_request(Request::Status(NoArgs {})) {
            Response::Status(status) => status,
            other => panic!("status failed: {other:?}"),
        };
        assert!(!status.paused);
        assert_eq!(status.words_total, 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn words_import_export_round_trip() {
        let dir = temp_dir("import");
        let (state, _sink, _injection) = open_state(&dir, "s");
        let imported = match state.handle_request(Request::WordsImport(WordsImport {
            format: ImportFormat::Csv,
            data: "roman,output,language\r\nvalo,ভালো,bn\r\n".to_owned(),
        })) {
            Response::Import(imported) => imported,
            other => panic!("import failed: {other:?}"),
        };
        assert_eq!((imported.imported, imported.skipped), (1, 0));

        let exported = match state.handle_request(Request::WordsExport(WordsExport {
            format: ImportFormat::Json,
        })) {
            Response::Export(exported) => exported,
            other => panic!("export failed: {other:?}"),
        };
        assert_eq!(exported.format, ImportFormat::Json);
        let file: okkhor_ipc::words::WordsFile =
            serde_json::from_str(&exported.data).expect("valid envelope");
        assert_eq!(file.schema, 1);
        assert_eq!(file.words.len(), 1);

        // Re-importing the export skips the existing row instead of failing.
        let again = match state.handle_request(Request::WordsImport(WordsImport {
            format: ImportFormat::Json,
            data: exported.data,
        })) {
            Response::Import(imported) => imported,
            other => panic!("re-import failed: {other:?}"),
        };
        assert_eq!((again.imported, again.skipped), (0, 1));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hello_and_ai_complete_have_typed_answers() {
        let dir = temp_dir("hello");
        let (state, _sink, _injection) = open_state(&dir, "s");
        let response = state.handle_request(Request::Hello(okkhor_ipc::protocol::Hello {
            protocol: 1,
            token: "00".repeat(32),
        }));
        assert_eq!(
            response.error_body().map(|body| body.code),
            Some(ErrorCode::InvalidRequest)
        );
        let response = state.handle_request(Request::AiComplete(okkhor_ipc::AiCompleteParams {
            prompt: "x".to_owned(),
            system: None,
            model: None,
            max_tokens: None,
            temperature: None,
            timeout_ms: None,
        }));
        assert_eq!(
            response.error_body().map(|body| body.code),
            Some(ErrorCode::AiDisabled)
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn debug_output_carries_no_paths_or_text() {
        let dir = temp_dir("debug");
        let (state, _sink, _injection) = open_state(&dir, "s");
        let _ = state.handle_request(Request::WordsAdd(WordAdd {
            roman: "gopon".to_owned(),
            output: "গোপন".to_owned(),
            language: okkhor_ipc::words::WordLanguage::Bn,
        }));
        let rendered = format!("{state:?}");
        assert!(!rendered.contains("gopon"));
        assert!(!rendered.contains("গোপন"));
        let dir_text: String = dir.to_string_lossy().into_owned();
        assert!(!rendered.contains(dir_text.as_str()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn token_file_install_rotates_and_removes() {
        use okkhor_ipc::auth::{Token, read_token_file};
        let dir = temp_dir("token");
        let path = dir.join("runtime").join("ipc.token");

        // Fresh install creates the file.
        let first = install_token(&path).expect("installs");
        assert!(path.is_file());
        let back = read_token_file(&path).expect("reads");
        assert!(first.hex() == back.hex());

        // A second install rotates (replaces) the token.
        let second = install_token(&path).expect("rotates");
        assert_ne!(first.hex(), second.hex());
        let back = read_token_file(&path).expect("reads rotated");
        assert!(Token::parse_hex(&second.hex()).expect("parses").hex() == back.hex());

        // Removal is idempotent (clean shutdown may run twice).
        remove_token_file(&path).expect("removes");
        assert!(!path.exists());
        remove_token_file(&path).expect("second remove is fine");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ui_lookup_honors_the_override() {
        // SAFETY: only this test touches `OKKHOR_UI_PATH`, and the value is
        // read back synchronously on this thread.
        unsafe {
            std::env::set_var(UI_ENV_OVERRIDE, "definitely-not-a-ui.exe");
        }
        assert_eq!(find_ui_exe(), None);
        let probe = temp_dir("ui").join("okkhor-ui.exe");
        std::fs::write(&probe, b"probe").expect("plants probe");
        // SAFETY: same contract as above.
        unsafe {
            std::env::set_var(UI_ENV_OVERRIDE, &probe);
        }
        assert_eq!(find_ui_exe(), Some(probe));
        // SAFETY: same contract as above.
        unsafe {
            std::env::remove_var(UI_ENV_OVERRIDE);
        }
    }
}

//! Okkhor daemon.
//!
//! The binary hosts the resident input daemon, the learning-engine self-check,
//! and manual smoke tests for the Win32 boundary. The daemon keeps tray and
//! logging off the keydown path.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use okkhor_core::{Candidate, CandidateSource, Language, Ranker, SuggestionRequest};
use okkhor_learn::{CommitSource, LearningEvent, Smriti, SmritiConfig};
use okkhor_storage::JsonFileRepository;

#[cfg(windows)]
mod logging;

mod input_session;
mod ipc;
mod paths;
mod settings_store;
mod words_store;

fn main() {
    #[cfg(windows)]
    let _ = okkhor_win::enable_per_monitor_dpi_awareness();

    let command = std::env::args().nth(1);

    match command.as_deref() {
        Some("--version") => println!("okkhor {}", env!("CARGO_PKG_VERSION")),
        Some("self-check") => {
            if let Err(error) = self_check() {
                eprintln!("self-check failed: {error}");
                std::process::exit(1);
            }
            println!("self-check: ok");
        }
        Some("hook-demo") => {
            if let Err(error) = hook_demo() {
                eprintln!("hook-demo failed: {error}");
                std::process::exit(1);
            }
        }
        Some("popup-demo") => {
            if let Err(error) = popup_demo() {
                eprintln!("popup-demo failed: {error}");
                std::process::exit(1);
            }
        }
        Some("tray-demo") => {
            if let Err(error) = tray_demo() {
                eprintln!("tray-demo failed: {error}");
                std::process::exit(1);
            }
        }
        Some("daemon") => {
            let rest: Vec<String> = std::env::args().skip(2).collect();
            match parse_daemon_args(&rest) {
                Ok(options) => {
                    if let Err(error) = run_daemon(options) {
                        eprintln!("daemon failed: {error}");
                        std::process::exit(1);
                    }
                }
                Err(usage) => {
                    eprintln!("{usage}");
                    std::process::exit(2);
                }
            }
        }
        Some("fonts") => {
            let rest: Vec<String> = std::env::args().skip(2).collect();
            match parse_fonts_args(&rest) {
                Ok(FontsAction::Status) => {
                    if let Err(error) = fonts_status() {
                        eprintln!("fonts status failed: {error}");
                        std::process::exit(1);
                    }
                }
                Ok(FontsAction::Install) => {
                    if let Err(error) = fonts_install() {
                        eprintln!("fonts install failed: {error}");
                        std::process::exit(1);
                    }
                }
                Ok(FontsAction::Uninstall(target)) => {
                    if let Err(error) = fonts_uninstall(&target) {
                        eprintln!("fonts uninstall failed: {error}");
                        std::process::exit(1);
                    }
                }
                Err(usage) => {
                    eprintln!("{usage}");
                    std::process::exit(2);
                }
            }
        }
        Some("inject") => {
            let arguments: Vec<String> = std::env::args().skip(2).collect();
            if let Err(error) = inject_cli(&arguments) {
                eprintln!("inject failed: {error}");
                std::process::exit(1);
            }
        }
        Some("autostart") => match parse_autostart_action(std::env::args().nth(2).as_deref()) {
            Some(action) => {
                if let Err(error) = autostart_cli(action) {
                    eprintln!("autostart failed: {error}");
                    std::process::exit(1);
                }
            }
            None => {
                eprintln!("usage: okkhor autostart <enable|disable|status>");
                std::process::exit(2);
            }
        },
        Some("--help") | Some("-h") | None => print_help(),
        Some(other) => {
            eprintln!("unknown argument: {other}");
            print_help();
            std::process::exit(2);
        }
    }
}

fn print_help() {
    println!(
        "Okkhor {}\n\nUSAGE:\n    okkhor [--help | --version | self-check | hook-demo | popup-demo | tray-demo | daemon [--encoding unicode|bijoy] [--mode bangla|english] [--no-ipc] [--ui] | inject <text> [--sendinput] [--delay-ms N] | autostart <enable|disable|status> | fonts <status|install|uninstall <name>>]\n\n\
         `self-check` exercises the learning engine, ranking, and persistence.\n\
         `hook-demo` installs a global hook for 5 seconds and prints key events.\n\
         `popup-demo` shows a non-activating popup near the caret for 3 seconds.\n\
         `tray-demo` shows the tray menu for 10 seconds.\n\
         `daemon [--encoding unicode|bijoy] [--mode bangla|english]` runs the resident daemon with hook, mode + pause hotkeys, tray, and the settings-UI pipe (defaults: unicode, bangla; `--no-ipc` disables the pipe; `--ui` launches the settings UI once at startup).\n\
         `inject` is a QA-only command for synthetic text; focus the target during the delay.\n\
         `autostart enable|disable|status` manages the per-user login entry.\n\
         `fonts status` lists staged font files, roles, and per-user install state (read-only, no admin).\n\
         `fonts install` installs all staged fonts per user (idempotent, no admin).\n\
         `fonts uninstall <name>` removes one per-user font (file + registry + session entry).\n\
         The demos are manual smoke tests; run them only in a disposable session.",
        env!("CARGO_PKG_VERSION")
    );
}

/// Output encoding selected at commit time (ADR 0013/0014).
///
/// Unicode is the default for modern apps; Bijoy converts the composed text
/// to legacy ANSI bytes (rendered with a Bijoy-compatible font) once, at
/// commit, off the hot path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputEncoding {
    Unicode,
    Bijoy,
}

impl OutputEncoding {
    /// Stable machine-readable name (also the CLI spelling).
    const fn as_str(self) -> &'static str {
        match self {
            Self::Unicode => "unicode",
            Self::Bijoy => "bijoy",
        }
    }

    /// Parse a CLI value; `None` for anything outside the allowlisted pair.
    fn parse(value: &str) -> Option<Self> {
        match value {
            "unicode" => Some(Self::Unicode),
            "bijoy" => Some(Self::Bijoy),
            _ => None,
        }
    }
}

/// Usage text for the `daemon` subcommand (also the exit-2 message).
const DAEMON_USAGE: &str =
    "usage: okkhor daemon [--encoding unicode|bijoy] [--mode bangla|english] [--no-ipc] [--ui]";

/// Input mode: Bangla phonetic composition or English pass-through
/// (contract §2, ADR 0014 §1). Default is Bangla. Pause stays separate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum InputMode {
    #[default]
    Bangla,
    English,
}

impl InputMode {
    /// Stable machine-readable name (also the CLI spelling).
    const fn as_str(self) -> &'static str {
        match self {
            Self::Bangla => "bangla",
            Self::English => "english",
        }
    }

    /// Parse a CLI value; `None` for anything outside the allowlisted pair.
    fn parse(value: &str) -> Option<Self> {
        match value {
            "bangla" => Some(Self::Bangla),
            "english" => Some(Self::English),
            _ => None,
        }
    }

    const fn toggle(self) -> Self {
        match self {
            Self::Bangla => Self::English,
            Self::English => Self::Bangla,
        }
    }

    /// Whether keystrokes feed the composition session. English passes
    /// through untouched (pause remains a separate gate).
    const fn allows_composition(self) -> bool {
        matches!(self, Self::Bangla)
    }
}

/// Parsed `okkhor daemon` options. Pure data; no I/O, no process state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DaemonOptions {
    encoding: OutputEncoding,
    mode: InputMode,
    /// Skip the settings-UI pipe (typing, tray, and hotkeys keep working).
    no_ipc: bool,
    /// Launch the settings UI once at startup (tray Settings always can).
    ui: bool,
}

impl DaemonOptions {
    const fn new(encoding: OutputEncoding, mode: InputMode) -> Self {
        Self {
            encoding,
            mode,
            no_ipc: false,
            ui: false,
        }
    }
}

/// Parse `okkhor daemon` arguments. Pure: no I/O, no process state.
/// Accepts `--encoding <v>` / `--encoding=<v>`, `--mode <v>` /
/// `--mode=<v>`, and the bare flags `--no-ipc` / `--ui`, in any order;
/// anything else is a usage error.
fn parse_daemon_args(args: &[String]) -> Result<DaemonOptions, &'static str> {
    let mut encoding = OutputEncoding::Unicode;
    let mut mode = InputMode::Bangla;
    let mut no_ipc = false;
    let mut ui = false;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if let Some(value) = arg.strip_prefix("--encoding=") {
            encoding = OutputEncoding::parse(value).ok_or(DAEMON_USAGE)?;
            index += 1;
        } else if let Some(value) = arg.strip_prefix("--mode=") {
            mode = InputMode::parse(value).ok_or(DAEMON_USAGE)?;
            index += 1;
        } else if arg == "--encoding" {
            let value = args.get(index + 1).ok_or(DAEMON_USAGE)?;
            encoding = OutputEncoding::parse(value).ok_or(DAEMON_USAGE)?;
            index += 2;
        } else if arg == "--mode" {
            let value = args.get(index + 1).ok_or(DAEMON_USAGE)?;
            mode = InputMode::parse(value).ok_or(DAEMON_USAGE)?;
            index += 2;
        } else if arg == "--no-ipc" {
            no_ipc = true;
            index += 1;
        } else if arg == "--ui" {
            ui = true;
            index += 1;
        } else {
            return Err(DAEMON_USAGE);
        }
    }
    let mut options = DaemonOptions::new(encoding, mode);
    options.no_ipc = no_ipc;
    options.ui = ui;
    Ok(options)
}

/// Shared daemon/IPC state type: DPAPI-backed words on Windows.
#[cfg(windows)]
type ProdState = crate::ipc::DaemonState<crate::words_store::DpapiProtector>;

/// Convert the daemon's CLI/mode enums to the IPC settings enums (and back).
/// The daemon owns the key-path copies; the IPC state owns the persisted ones;
/// these keep the two in sync without sharing types across the boundary.
#[cfg(windows)]
const fn ipc_input_mode(mode: crate::InputMode) -> okkhor_ipc::settings::InputMode {
    match mode {
        crate::InputMode::Bangla => okkhor_ipc::settings::InputMode::Bangla,
        crate::InputMode::English => okkhor_ipc::settings::InputMode::English,
    }
}

#[cfg(windows)]
const fn daemon_input_mode(mode: okkhor_ipc::settings::InputMode) -> crate::InputMode {
    match mode {
        okkhor_ipc::settings::InputMode::Bangla => crate::InputMode::Bangla,
        okkhor_ipc::settings::InputMode::English => crate::InputMode::English,
    }
}

#[cfg(windows)]
const fn ipc_output_encoding(
    encoding: crate::OutputEncoding,
) -> okkhor_ipc::settings::OutputEncoding {
    match encoding {
        crate::OutputEncoding::Unicode => okkhor_ipc::settings::OutputEncoding::Unicode,
        crate::OutputEncoding::Bijoy => okkhor_ipc::settings::OutputEncoding::Bijoy,
    }
}

#[cfg(windows)]
const fn daemon_output_encoding(
    encoding: okkhor_ipc::settings::OutputEncoding,
) -> crate::OutputEncoding {
    match encoding {
        okkhor_ipc::settings::OutputEncoding::Unicode => crate::OutputEncoding::Unicode,
        okkhor_ipc::settings::OutputEncoding::Bijoy => crate::OutputEncoding::Bijoy,
    }
}

/// Usage text for the `fonts` subcommand (also the exit-2 message).
const FONTS_USAGE: &str = "usage: okkhor fonts <status|install|uninstall <name>>";

/// Action selected by `okkhor fonts ...`. The uninstall target is a bundled
/// file name (resolved by [`resolve_uninstall_target`]); anything else is
/// rejected at parse time with the usage text.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FontsAction {
    Status,
    Install,
    Uninstall(String),
}

/// Parse `okkhor fonts` arguments. Pure: no I/O, no registry access.
fn parse_fonts_args(args: &[String]) -> Result<FontsAction, &'static str> {
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["status"] => Ok(FontsAction::Status),
        ["install"] => Ok(FontsAction::Install),
        ["uninstall", target] => {
            let resolved = resolve_uninstall_target(target).ok_or(FONTS_USAGE)?;
            Ok(FontsAction::Uninstall(resolved.to_owned()))
        }
        _ => Err(FONTS_USAGE),
    }
}

/// Map a `fonts uninstall` argument to a bundled file name.
///
/// Accepts an exact bundled file name (case-insensitive), a family shorthand
/// (`sutonnymj`, `kalpurush`, `kalpurush-ansi`, `noto`), or any other plain
/// font file name (passed through for [`okkhor_win::uninstall_font_per_user`]
/// to validate). Returns `None` for empty input, paths, or non-font
/// extensions.
fn resolve_uninstall_target(arg: &str) -> Option<&str> {
    for (name, _) in okkhor_win::BUNDLED_FONTS {
        if name.eq_ignore_ascii_case(arg) {
            return Some(name);
        }
    }
    let normalized: String = arg
        .to_ascii_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect();
    let bundled: Option<&'static str> = match normalized.as_str() {
        "sutonnymj" | "sutonnymjregular" => Some("SutonnyMJ Regular.ttf"),
        "kalpurush" => Some("kalpurush.ttf"),
        "kalpurushansi" => Some("kalpurush ANSI.ttf"),
        "noto" | "notosansbengali" | "notosansbengalifallback" => Some("NotoSansBengali.ttf"),
        _ => None,
    };
    if let Some(name) = bundled {
        return Some(name);
    }
    // Pass-through: a plain file name the uninstall API validates itself.
    // Rejects separators and non-font extensions up front so paths and
    // executables never reach the registry/File APIs.
    if arg.is_empty() || arg.len() > 128 {
        return None;
    }
    if arg.contains('/') || arg.contains('\\') || arg.contains(':') {
        return None;
    }
    let (stem, extension) = arg.rsplit_once('.')?;
    if stem.is_empty() || stem.contains('.') {
        return None;
    }
    if !matches!(
        extension.to_ascii_lowercase().as_str(),
        "ttf" | "otf" | "ttc"
    ) {
        return None;
    }
    Some(arg)
}

/// Convert composed Unicode text to the commit-time output encoding.
///
/// Unicode passes through unchanged. Bijoy applies
/// [`okkhor_lang::bijoy::to_bijoy`]; the error is projected to its stable
/// logging kind (never text). Pure: no I/O, no logging.
fn apply_encoding(text: &str, encoding: OutputEncoding) -> Result<String, &'static str> {
    match encoding {
        OutputEncoding::Unicode => Ok(text.to_owned()),
        OutputEncoding::Bijoy => okkhor_lang::bijoy::to_bijoy(text).map_err(|error| error.kind()),
    }
}

/// Prefer `<exe-dir>\fonts` when it exists, else `dev_fonts` (the
/// `<repo>/assets/fonts` checkout used under `cargo run`). Falls back to the
/// exe-adjacent directory when neither exists so `fonts status` can report
/// every file as missing instead of failing. Pure over its inputs.
fn select_fonts_dir(exe_fonts: &Path, dev_fonts: &Path) -> PathBuf {
    if exe_fonts.is_dir() {
        return exe_fonts.to_path_buf();
    }
    if dev_fonts.is_dir() {
        return dev_fonts.to_path_buf();
    }
    exe_fonts.to_path_buf()
}

/// Locate the staged-font directory: `<exe-dir>\fonts` first, else the
/// development checkout's `assets/fonts` (found by walking up from the
/// executable, then from the working directory). Never contains hardcoded
/// absolute paths or user names.
fn locate_fonts_dir() -> PathBuf {
    let exe_fonts = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.join("fonts")))
        .unwrap_or_else(|| PathBuf::from("fonts"));
    if exe_fonts.is_dir() {
        return exe_fonts;
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut cursor = exe.parent().map(Path::to_path_buf);
        for _ in 0..8 {
            match cursor {
                Some(dir) => {
                    let candidate = dir.join("assets").join("fonts");
                    if candidate.is_dir() {
                        return candidate;
                    }
                    cursor = dir.parent().map(Path::to_path_buf);
                }
                None => break,
            }
        }
    }
    let cwd_candidate = std::env::current_dir()
        .map(|dir| dir.join("assets").join("fonts"))
        .unwrap_or_else(|_| PathBuf::from("assets").join("fonts"));
    select_fonts_dir(&exe_fonts, &cwd_candidate)
}

/// Family name for display: the font's own name table when the staged file
/// is present, else the file stem. Never a filesystem path (paths can embed
/// the user's name).
fn display_family(dir: &Path, file_name: &str) -> String {
    let staged = dir.join(file_name);
    if let Ok(bytes) = std::fs::read(&staged) {
        if let Some(family) = okkhor_win::font_family_name(&bytes) {
            return family;
        }
    }
    Path::new(file_name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(file_name)
        .to_owned()
}

/// `okkhor fonts status`: list staged files, roles, and per-user install
/// state plus Bijoy resolution. Read-only: no registry writes, no copies,
/// no admin. Prints names/roles/yes-no only — never paths.
fn fonts_status() -> Result<(), Box<dyn Error>> {
    #[cfg(windows)]
    let logger = logging::init();
    #[cfg(windows)]
    logger.command(logging::Command::FontsStatus);

    let dir = locate_fonts_dir();
    for bundled in okkhor_win::list_bundled_fonts(&dir) {
        let family = display_family(&dir, bundled.file_name);
        let installed = okkhor_win::is_font_installed(&family);
        println!(
            "{} | role={} | staged={} | installed={}",
            bundled.file_name,
            bundled.role.as_str(),
            if bundled.present { "yes" } else { "no" },
            if installed { "yes" } else { "no" },
        );
    }
    let choice = okkhor_win::resolve_bijoy_font_with_probe(&dir, &okkhor_win::SystemFontProbe);
    println!("bijoy: {}", choice.as_str());
    Ok(())
}

/// `okkhor fonts install`: install every staged `.ttf` per user via the
/// existing API. Idempotent; missing files are skipped with a warning.
/// Prints family + role only — never paths (they can embed user names).
fn fonts_install() -> Result<(), Box<dyn Error>> {
    #[cfg(windows)]
    let logger = logging::init();
    #[cfg(windows)]
    logger.command(logging::Command::FontsInstall);

    let dir = locate_fonts_dir();
    let mut staged_any = false;
    for (file_name, role) in okkhor_win::BUNDLED_FONTS {
        let source = dir.join(file_name);
        if !source.is_file() {
            eprintln!("fonts: missing {file_name}; skipping");
            continue;
        }
        staged_any = true;
        match okkhor_win::install_font_per_user(&source) {
            Ok(done) => {
                let family = display_family(&dir, file_name);
                println!(
                    "fonts: {family} [{}] {}",
                    role.as_str(),
                    if done.already_installed {
                        "already installed"
                    } else {
                        "installed"
                    },
                );
            }
            Err(error) => {
                #[cfg(windows)]
                logger.error(logging::Scope::Registry, error.kind(), None);
                eprintln!("fonts: failed {file_name}: {}", error.kind());
            }
        }
    }
    if !staged_any {
        eprintln!("fonts: no staged fonts found; skipping");
    }
    Ok(())
}

/// `okkhor fonts uninstall <name>`: remove one per-user font (session
/// resource + file + registry). `target` is a bundled file name resolved by
/// [`parse_fonts_args`]. Prints outcome flags only — never paths.
fn fonts_uninstall(target: &str) -> Result<(), Box<dyn Error>> {
    #[cfg(windows)]
    let logger = logging::init();
    #[cfg(windows)]
    logger.command(logging::Command::FontsUninstall);

    match okkhor_win::uninstall_font_per_user(target) {
        Ok(removed) => {
            println!(
                "fonts: {target} removed file={} registration={}",
                if removed.removed_file { "yes" } else { "no" },
                if removed.removed_registration {
                    "yes"
                } else {
                    "no"
                },
            );
            Ok(())
        }
        Err(error) => {
            #[cfg(windows)]
            logger.error(logging::Scope::Registry, error.kind(), None);
            Err(error.into())
        }
    }
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutostartAction {
    Enable,
    Disable,
    Status,
}

#[cfg(windows)]
impl AutostartAction {
    const fn command(self) -> logging::Command {
        match self {
            Self::Enable => logging::Command::AutostartEnable,
            Self::Disable => logging::Command::AutostartDisable,
            Self::Status => logging::Command::AutostartStatus,
        }
    }
}

#[cfg(windows)]
fn parse_autostart_action(argument: Option<&str>) -> Option<AutostartAction> {
    match argument {
        Some("enable") => Some(AutostartAction::Enable),
        Some("disable") => Some(AutostartAction::Disable),
        Some("status") => Some(AutostartAction::Status),
        _ => None,
    }
}

#[cfg(not(windows))]
fn parse_autostart_action(_argument: Option<&str>) -> Option<()> {
    None
}

#[cfg(windows)]
fn run_daemon(options: DaemonOptions) -> Result<(), Box<dyn Error>> {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    use okkhor_core::Language;
    use okkhor_learn::{LearningEvent, Smriti};
    use okkhor_win::{
        InjectError, KeyPhase, KeyboardHook, ModeHotkey, PauseHotkey, SingleInstance,
        SingleInstanceError, SuggestionPopup, TrayEvent, TrayIcon, TrayMode, WindowsKeyboardHook,
        caret_anchor, ctrl_or_alt_pressed, foreground_window_id, inject_commit, place_popup,
    };

    use crate::input_session::{
        BackspaceOutcome, Commit, DefaultEngine, InputSession, VK_BACK, VK_DOWN, VK_ESCAPE,
        VK_LEFT, VK_RETURN, VK_RIGHT, VK_SPACE, VK_TAB, VK_UP, default_engine, vk_to_digit_index,
        vk_to_letter,
    };

    /// Allowlisted log kind for a mode toggle (no text, no codes).
    const MODE_TOGGLE_KIND: &str = "mode_toggle";
    /// How long the mode toast stays visible (ADR 0014 §3).
    const MODE_TOAST_MS: u64 = 800;

    /// Toast text per mode. Bangla label is a fixed literal, never user text.
    fn mode_toast_text(mode: crate::InputMode) -> &'static str {
        match mode {
            crate::InputMode::Bangla => "বাংলা",
            crate::InputMode::English => "English",
        }
    }

    /// Everything one key event needs. Kept in one struct so the hot loop
    /// stays a thin dispatch with no globals and no cross-thread sharing.
    /// `state` is the IPC mirror (lock-free atomics only — never a store
    /// lock, never disk I/O); `injection` serializes commit erase+inject
    /// against explicit UI inserts (commit wins, never interleaves).
    struct CompositionCtx<'a> {
        session: &'a mut InputSession<DefaultEngine>,
        popup: &'a mut Option<SuggestionPopup>,
        logger: &'a logging::Logger,
        learn_tx: &'a mpsc::SyncSender<LearningEvent>,
        learn_dropped: &'a AtomicU64,
        compose_window: &'a mut isize,
        encoding: crate::OutputEncoding,
        learning_enabled: bool,
        state: Option<&'a ProdState>,
        injection: &'a std::sync::Mutex<()>,
    }

    fn hide_popup(popup: &mut Option<SuggestionPopup>, state: Option<&ProdState>) {
        if let Some(window) = popup {
            window.hide();
        }
        if let Some(state) = state {
            state.set_popup_visible(false);
        }
    }

    fn refresh_popup(ctx: &mut CompositionCtx<'_>) {
        let enabled = ctx.popup_enabled();
        let state = ctx.state;
        if !enabled {
            hide_popup(ctx.popup, state);
            return;
        }
        let Some(window) = ctx.popup.as_mut() else {
            return;
        };
        if !ctx.session.is_composing() || ctx.session.candidates().is_empty() {
            window.hide();
            if let Some(state) = state {
                state.set_popup_visible(false);
            }
            return;
        }
        let mut text = String::new();
        for (index, candidate) in ctx.session.candidates().iter().enumerate() {
            use std::fmt::Write as _;
            if index > 0 {
                text.push('\n');
            }
            let marker = if index == ctx.session.selected() {
                '>'
            } else {
                ' '
            };
            let _ = write!(text, "{marker} {}. {candidate}", index + 1);
        }
        match caret_anchor() {
            Some(anchor) => {
                let height = (36 + 24 * ctx.session.candidates().len() as i32).clamp(36, 200);
                let placement = place_popup(&anchor, 280, height);
                if let Err(error) = window.show(&text, placement) {
                    ctx.logger
                        .error(logging::Scope::Daemon, "popup_show", error.code());
                    if let Some(state) = ctx.state {
                        state.set_popup_visible(false);
                    }
                } else if let Some(state) = ctx.state {
                    state.set_popup_visible(true);
                }
            }
            None => {
                window.hide();
                if let Some(state) = ctx.state {
                    state.set_popup_visible(false);
                }
            }
        }
    }

    impl CompositionCtx<'_> {
        /// Whether the popup may show. Without IPC state the popup behaves as
        /// before (enabled); with state it follows the persisted setting live.
        fn popup_enabled(&self) -> bool {
            self.state
                .map(|state| state.popup_enabled())
                .unwrap_or(true)
        }
    }

    fn inject_code(error: &InjectError) -> Option<i32> {
        match error {
            InjectError::Win32 { code, .. } => Some(*code as i32),
            _ => None,
        }
    }

    fn now_ms() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0)
    }

    /// Commit path: verify focus, then erase + inject in one batch.
    ///
    /// Erase, Bangla text, and the optional trailing space go out in a single
    /// `SendInput` batch, so a zero-delivery failure erases nothing and
    /// injects nothing. A nonzero partial send can still erase a roman prefix
    /// without replacement (inherent to Win32); retry means retyping the word.
    /// With `--encoding bijoy` the composed Unicode text is converted with
    /// [`okkhor_lang::bijoy::to_bijoy`] before injection; the trailing space
    /// stays ASCII either way. A conversion failure injects the Unicode text
    /// unchanged and logs the allowlisted `encoding_fallback` code.
    /// Never logs the buffer or the committed text; failures log an
    /// allowlisted kind plus an optional numeric code only.
    fn do_commit(ctx: &mut CompositionCtx<'_>, commit: Commit, roman: String) {
        if foreground_window_id() != *ctx.compose_window {
            ctx.session.reset();
            hide_popup(ctx.popup, ctx.state);
            ctx.logger
                .error(logging::Scope::Daemon, "focus_changed", None);
            return;
        }
        // Unicode original for learning: Smriti maps roman input to Unicode
        // output, so Bijoy bytes must never enter the learning path.
        let unicode_text = commit.text.clone();
        let encoding = ctx.encoding;
        let inject_text = match encoding {
            crate::OutputEncoding::Unicode => commit.text,
            crate::OutputEncoding::Bijoy => match crate::apply_encoding(&commit.text, encoding) {
                Ok(converted) => converted,
                Err(_) => {
                    ctx.logger
                        .error(logging::Scope::Daemon, "encoding_fallback", None);
                    commit.text
                }
            },
        };
        // The injection lock serializes this erase+inject batch against
        // explicit UI inserts: the commit always wins (blocking wait of at
        // most one UI `SendInput`), so the two can never interleave.
        let _injection = ctx
            .injection
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Err(error) = inject_commit(commit.erase_chars, &inject_text, commit.trailing_space) {
            ctx.logger
                .error(logging::Scope::Daemon, "inject_commit", inject_code(&error));
            ctx.session.reset();
            hide_popup(ctx.popup, ctx.state);
            return;
        }
        hide_popup(ctx.popup, ctx.state);
        if !ctx.learning_enabled {
            return;
        }
        // Queue learning for the background thread. The channel is bounded and
        // `try_send` never blocks, so a stalled learn thread can never stall
        // the key path; the drop is counted (a count carries no text) and
        // reported once at shutdown. A disconnected receiver drops the same way.
        // `Smriti::observe` performs no I/O.
        let event = LearningEvent::TransliterationCommitted {
            roman_input: roman,
            output: unicode_text,
            language: Language::Bangla,
            previous_word: None,
            timestamp_ms: now_ms(),
        };
        if ctx.learn_tx.try_send(event).is_err() {
            ctx.learn_dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn cancel_composition(ctx: &mut CompositionCtx<'_>) {
        // `cancel` returns the raw roman buffer, which the caller deliberately
        // drops: already-typed text stays untouched in the target.
        let _ = ctx.session.cancel();
        hide_popup(ctx.popup, ctx.state);
    }

    /// Record or re-validate the composition window around one keystroke.
    ///
    /// When focus moved mid-composition the stale buffer is dropped (the roman
    /// text stays where it was typed) and the window id is refreshed.
    fn track_focus(ctx: &mut CompositionCtx<'_>) {
        let foreground = foreground_window_id();
        if ctx.session.is_composing() && foreground != *ctx.compose_window {
            ctx.session.reset();
            hide_popup(ctx.popup, ctx.state);
        }
        if !ctx.session.is_composing() {
            *ctx.compose_window = foreground;
        }
    }

    #[allow(clippy::too_many_lines)]
    fn handle_key(vk: u16, ctx: &mut CompositionCtx<'_>) {
        if let Some(letter) = vk_to_letter(vk) {
            // Shortcuts (Ctrl/Alt/Win chords) produce no text in the target;
            // growing the buffer for them would erase the user's real text on
            // commit, so the composition ends before the chord passes through.
            if ctrl_or_alt_pressed() {
                cancel_composition(ctx);
                return;
            }
            track_focus(ctx);
            if ctx.session.push_letter(letter) {
                refresh_popup(ctx);
            }
            return;
        }
        if let Some(index) = vk_to_digit_index(vk) {
            if !ctx.session.is_composing() {
                return;
            }
            let roman = ctx.session.buffer().to_owned();
            if let Some(commit) = ctx.session.select_digit(index) {
                do_commit(ctx, commit, roman);
            }
            return;
        }
        match vk {
            VK_BACK => {
                // Focus is re-validated before the edit, mirroring the letter
                // arm: a focus switch resets the stale buffer (pass-through
                // below) instead of paying for one wasted edit + refresh.
                // Erase-count safety never depends on this ordering — commit
                // always uses the live buffer length, and `do_commit`
                // re-checks focus before touching `SendInput`.
                track_focus(ctx);
                if !ctx.session.is_composing() {
                    return;
                }
                if ctx.session.backspace() != BackspaceOutcome::Empty {
                    refresh_popup(ctx);
                }
            }
            VK_SPACE | VK_RETURN => {
                if !ctx.session.is_composing() {
                    return;
                }
                let roman = ctx.session.buffer().to_owned();
                match ctx.session.commit(vk == VK_SPACE) {
                    Some(commit) => do_commit(ctx, commit, roman),
                    // No candidates: the roman text stays as typed; the Space
                    // or Enter key itself passes through untouched.
                    None => {
                        ctx.session.reset();
                        hide_popup(ctx.popup, ctx.state);
                    }
                }
            }
            VK_LEFT | VK_UP => {
                if ctx.session.is_composing() {
                    ctx.session.move_selection(-1);
                    refresh_popup(ctx);
                }
            }
            VK_RIGHT | VK_DOWN => {
                if ctx.session.is_composing() {
                    ctx.session.move_selection(1);
                    refresh_popup(ctx);
                }
            }
            VK_ESCAPE | VK_TAB if ctx.session.is_composing() => {
                cancel_composition(ctx);
            }
            _ => {}
        }
    }

    /// Open the IPC stores and seed a fresh data home from the CLI flags.
    /// The CLI selects the initial mode only (ADR 0014 §1): afterwards the
    /// persisted last mode wins. Any failure returns an allowlisted log kind;
    /// the caller degrades to typing-without-IPC, never a dead daemon.
    fn setup_ipc_state(
        options: &DaemonOptions,
        injection: &Arc<std::sync::Mutex<()>>,
    ) -> Result<ProdState, &'static str> {
        use okkhor_ipc::settings::SettingsPatch;
        let fresh = !crate::paths::settings_path()
            .map_err(|_| "ipc_state")?
            .exists();
        let settings =
            crate::settings_store::SettingsStore::load_default().map_err(|_| "ipc_state")?;
        let words = crate::words_store::WordsStore::open_default(
            crate::words_store::DpapiProtector::words(),
        )
        .map_err(|_| "ipc_state")?;
        let state = crate::ipc::DaemonState::from_stores(
            settings,
            words,
            Arc::new(crate::ipc::ProductionSink),
            Arc::clone(injection),
        );
        if fresh {
            let mut patch = SettingsPatch::default();
            if state.mode() != ipc_input_mode(options.mode) {
                patch.input_mode = Some(ipc_input_mode(options.mode));
            }
            if state.encoding() != ipc_output_encoding(options.encoding) {
                patch.output_encoding = Some(ipc_output_encoding(options.encoding));
            }
            if patch != SettingsPatch::default() {
                state.update_settings(&patch).map_err(|_| "ipc_state")?;
            }
        }
        Ok(state)
    }

    /// Persist a local (tray/F9) mode toggle without touching the key path.
    /// The live mirror is already flipped by the caller; the file write runs
    /// on a throwaway thread and surfaces through the store-failure counter
    /// the loop watches (one allowlisted code, never text).
    fn persist_mode(state: &Arc<ProdState>, mode: crate::InputMode) {
        use okkhor_ipc::settings::SettingsPatch;
        let state = Arc::clone(state);
        let desired = ipc_input_mode(mode);
        let _ = std::thread::Builder::new()
            .name("okkhor-mode-persist".to_owned())
            .spawn(move || {
                let patch = SettingsPatch {
                    input_mode: Some(desired),
                    ..SettingsPatch::default()
                };
                let _ = state.update_settings(&patch);
            });
    }

    /// Re-register hotkeys after a settings change (daemon loop only — the
    /// IPC thread cannot touch the hotkey threads). Feasible chords apply
    /// live; anything else stays deferred and is logged with one allowlisted
    /// code (never the chord text).
    #[allow(clippy::too_many_lines)]
    fn rebind_hotkeys(
        state: &ProdState,
        mode_hotkey: &mut ModeHotkey,
        pause_hotkey: &mut PauseHotkey,
        mode_ready: &mut bool,
        pause_ready: &mut bool,
        logger: &logging::Logger,
    ) {
        let Some((toggle, pause)) = state.hotkey_chords() else {
            // Settings lock contended (an IPC write is in flight): retry next poll.
            state.request_rebind();
            return;
        };
        let mut toggle_ok = false;
        match okkhor_win::ModeBinding::parse(&toggle) {
            Some(binding) if *mode_ready && mode_hotkey.binding() == binding => {
                toggle_ok = true;
            }
            Some(binding) => match mode_hotkey.start_with(binding) {
                Ok(actual) => {
                    *mode_ready = true;
                    toggle_ok = actual == binding;
                }
                Err(_) => {
                    *mode_ready = false;
                }
            },
            None => {}
        }
        if !toggle_ok {
            logger.error(logging::Scope::Hotkey, "hotkey_rebind", None);
        }
        let mut pause_ok = false;
        if pause_is_default(&pause) {
            if pause_hotkey.is_running() {
                pause_ok = true;
            } else {
                pause_hotkey.stop();
                match pause_hotkey.start() {
                    Ok(()) => {
                        *pause_ready = true;
                        pause_ok = true;
                    }
                    Err(_) => {
                        *pause_ready = false;
                    }
                }
            }
        }
        if !pause_ok {
            logger.error(logging::Scope::Hotkey, "hotkey_rebind", None);
        }
        state.note_rebound(toggle_ok, pause_ok);
    }

    /// Whether `pause` names the daemon's baked chord (the only one a
    /// `PauseHotkey` can register). Compared parsed, not verbatim, so
    /// `ctrl+alt+shift+p` counts as the default too.
    fn pause_is_default(pause: &str) -> bool {
        use okkhor_ipc::settings::parse_hotkey;
        match (parse_hotkey(pause), parse_hotkey("Ctrl+Alt+Shift+P")) {
            (Ok(got), Ok(want)) => got == want,
            _ => false,
        }
    }

    let logger = logging::init();
    logger.startup();
    logger.command(logging::Command::Daemon);

    let _single_instance = match SingleInstance::acquire("Local\\Okkhor.Daemon") {
        Ok(guard) => guard,
        Err(SingleInstanceError::AlreadyRunning) => {
            println!("daemon: another Okkhor instance is already running");
            return Ok(());
        }
        Err(error) => {
            logger.error(logging::Scope::SingleInstance, "single_instance", None);
            return Err(error.into());
        }
    };

    let mut hook = WindowsKeyboardHook::new();
    if let Err(error) = hook.start() {
        logger.error(logging::Scope::Hook, "hook_start", None);
        return Err(error.into());
    }

    let mut hotkey = PauseHotkey::new();
    let mut hotkey_ready = match hotkey.start() {
        Ok(()) => true,
        Err(error) => {
            logger.error(logging::Scope::Hotkey, "hotkey_start", None);
            eprintln!("daemon: pause hotkey unavailable: {error}");
            false
        }
    };

    let mut tray = match TrayIcon::new() {
        Ok(tray) => tray,
        Err(error) => {
            logger.error(logging::Scope::Tray, "tray_create", None);
            return Err(error.into());
        }
    };

    // IPC mirror + injection lock. The injection mutex is shared with the
    // commit path (commit wins, never interleaves); the stores sit behind the
    // state mutex at IPC rate only — never on the hook thread.
    let injection_lock = Arc::new(std::sync::Mutex::new(()));
    let ipc_state: Option<Arc<ProdState>> = match setup_ipc_state(&options, &injection_lock) {
        Ok(state) => Some(Arc::new(state)),
        Err(kind) => {
            logger.error(logging::Scope::Ipc, kind, None);
            None
        }
    };

    // Learning runs on a background thread: `Smriti::observe` performs no I/O,
    // the channel is bounded (1024), and `try_send` never blocks, so a
    // stalled learn thread drops (and counts) events instead of stalling the
    // key path. If the thread cannot start, typing continues without learning.
    const LEARN_QUEUE_CAP: usize = 1024;
    let (learn_tx, learn_rx) = mpsc::sync_channel::<LearningEvent>(LEARN_QUEUE_CAP);
    let learn_dropped = Arc::new(AtomicU64::new(0));
    if std::thread::Builder::new()
        .name("okkhor-learn".to_owned())
        .spawn(move || {
            let smriti = Smriti::new();
            for event in learn_rx {
                smriti.observe(event);
            }
        })
        .is_err()
    {
        logger.error(logging::Scope::Daemon, "learn_thread", None);
    }

    let mut popup = match SuggestionPopup::new() {
        Ok(window) => Some(window),
        Err(error) => {
            logger.error(logging::Scope::Daemon, "popup_create", error.code());
            None
        }
    };

    let engine = default_engine();
    if engine.is_fallback() {
        // Embedded-table load failure: allowlisted startup record only
        // (scope + kind, never text). Typing continues with no candidates.
        let kind = engine
            .fallback_kind()
            .unwrap_or("transliterate_embedded_table");
        logger.error(logging::Scope::Transliterate, kind, None);
    }
    let mut session: InputSession<DefaultEngine> = InputSession::new(engine);
    let mut compose_window = foreground_window_id();
    let mut encoding = options.encoding;
    let mut mode = options.mode;
    let mut learning_enabled = true;
    if let Some(state) = ipc_state.as_deref() {
        // Persisted last mode wins over the CLI flags (the flags seed a fresh
        // home only — see `setup_ipc_state`).
        mode = daemon_input_mode(state.mode());
        encoding = daemon_output_encoding(state.encoding());
        learning_enabled = state.learning();
    }
    let mut toast_until_ms: u64 = 0;
    let mut last_apply_seq: u64 = 0;
    let mut last_store_failures: u64 = 0;

    fn tray_mode_of(mode: crate::InputMode) -> TrayMode {
        match mode {
            crate::InputMode::Bangla => TrayMode::Bangla,
            crate::InputMode::English => TrayMode::English,
        }
    }

    fn show_mode_toast(
        popup: &mut Option<SuggestionPopup>,
        logger: &logging::Logger,
        mode: crate::InputMode,
        state: Option<&ProdState>,
    ) -> bool {
        let Some(window) = popup.as_mut() else {
            return false;
        };
        let Some(anchor) = caret_anchor() else {
            window.hide();
            if let Some(state) = state {
                state.set_popup_visible(false);
            }
            return false;
        };
        // Small fixed toast near the caret; never activates, never steals
        // focus. Text is a fixed literal per mode, never user content.
        let placement = place_popup(&anchor, 160, 48);
        if let Err(error) = window.show(mode_toast_text(mode), placement) {
            logger.error(logging::Scope::Daemon, "popup_show", error.code());
            if let Some(state) = state {
                state.set_popup_visible(false);
            }
            return false;
        }
        if let Some(state) = state {
            state.set_popup_visible(true);
        }
        true
    }

    // Mode toggle: flip Bangla/English, drop any partial composition, update
    // the tray label, and show a short non-activating toast. Never blocks the
    // key path: no sleep, no I/O, no logging of text — one allowlisted code.
    // The last mode also persists through the IPC state (mirror first, file
    // write on a throwaway thread).
    fn toggle_mode(
        session: &mut InputSession<DefaultEngine>,
        popup: &mut Option<SuggestionPopup>,
        tray: &TrayIcon,
        logger: &logging::Logger,
        mode: &mut crate::InputMode,
        toast_until_ms: &mut u64,
        state: Option<&Arc<ProdState>>,
    ) {
        *mode = mode.toggle();
        session.reset();
        let live = state.map(|state| &**state);
        hide_popup(popup, live);
        tray.set_mode(tray_mode_of(*mode));
        if let Some(state) = state {
            state.set_mode_live(ipc_input_mode(*mode));
            persist_mode(state, *mode);
        }
        if show_mode_toast(popup, logger, *mode, live) {
            *toast_until_ms = now_ms().saturating_add(MODE_TOAST_MS);
        } else {
            *toast_until_ms = 0;
        }
        logger.error(logging::Scope::Daemon, MODE_TOGGLE_KIND, None);
        println!("daemon: mode {}", mode.as_str());
    }

    tray.set_mode(tray_mode_of(mode));

    println!(
        "daemon: running (encoding {}, mode {})",
        encoding.as_str(),
        mode.as_str()
    );
    if hotkey_ready {
        println!("daemon: Ctrl+Alt+Shift+P toggles capture");
    }
    // Mode hotkey: F9 by default, Ctrl+Space if F9 is already claimed.
    let mut mode_hotkey = ModeHotkey::new();
    let mut mode_hotkey_ready = match mode_hotkey.start() {
        Ok(binding) => {
            println!("daemon: {} toggles Bangla/English", binding.name());
            if binding != okkhor_win::ModeBinding::F9 {
                // One-time, text-free notice: the setting stays editable and
                // no admin was involved.
                logger.error(logging::Scope::Hotkey, "hotkey_fallback", None);
                eprintln!("daemon: F9 unavailable; using Ctrl+Space for mode toggle");
            }
            true
        }
        Err(error) => {
            logger.error(logging::Scope::Hotkey, "hotkey_start", None);
            eprintln!("daemon: mode hotkey unavailable: {error}");
            false
        }
    };
    if mode == crate::InputMode::English {
        println!("daemon: English mode: typing passes through (F9 toggles Bangla)");
    } else {
        println!("daemon: type Banglish and press Space to commit Bangla");
    }
    println!("daemon: use the tray menu or pause hotkey to pause");

    // IPC pipe (unless --no-ipc): token create/rotate, then serve on a
    // dedicated thread. Any failure degrades to typing-without-IPC (logged
    // with one allowlisted code); the pipe is owner-only per ADR 0012 §2.
    let mut ipc_server: Option<(crate::ipc::IpcServer, std::path::PathBuf)> = None;
    if !options.no_ipc {
        if let Some(state) = ipc_state.clone() {
            match crate::paths::token_path() {
                Ok(token_path) => match crate::ipc::install_token(&token_path) {
                    Ok(token) => match crate::ipc::IpcServer::start(state, token) {
                        Ok(server) => {
                            logger.error(logging::Scope::Ipc, "ipc_start", None);
                            println!("daemon: IPC ready for the settings UI");
                            ipc_server = Some((server, token_path));
                        }
                        Err(_) => logger.error(logging::Scope::Ipc, "ipc_server", None),
                    },
                    Err(_) => logger.error(logging::Scope::Ipc, "ipc_token", None),
                },
                Err(_) => logger.error(logging::Scope::Ipc, "ipc_token", None),
            }
        }
    }

    if options.ui {
        open_settings_ui(&tray, logger);
    }

    'run: loop {
        while let Some(event) = tray.try_next() {
            match event {
                TrayEvent::TogglePause => toggle_capture(&hook, &tray, ipc_state.as_ref()),
                TrayEvent::ToggleMode => toggle_mode(
                    &mut session,
                    &mut popup,
                    &tray,
                    logger,
                    &mut mode,
                    &mut toast_until_ms,
                    ipc_state.as_ref(),
                ),
                TrayEvent::Settings => open_settings_ui(&tray, logger),
                TrayEvent::Exit => break 'run,
            }
        }

        if hotkey_ready && hotkey.try_next() {
            toggle_capture(&hook, &tray, ipc_state.as_ref());
        }

        if mode_hotkey_ready && mode_hotkey.try_next() {
            toggle_mode(
                &mut session,
                &mut popup,
                &tray,
                logger,
                &mut mode,
                &mut toast_until_ms,
                ipc_state.as_ref(),
            );
        }

        // IPC live sync: the UI flips `capture.set` and settings, and the loop
        // applies them here so the hook thread and the blocking IPC threads
        // never meet. All reads are lock-free atomics except the hotkey chords
        // (try_lock — a contended settings write retries next poll).
        if let Some(state) = ipc_state.as_deref() {
            let want_enabled = !state.paused();
            if hook.capture_enabled() != want_enabled {
                hook.set_capture_enabled(want_enabled);
            }
            if tray.paused() != state.paused() {
                tray.set_paused(state.paused());
            }
            let live_mode = daemon_input_mode(state.mode());
            if live_mode != mode {
                // IPC-driven mode change: the visible effects of a local
                // toggle, minus the toast (no caret access from this path).
                mode = live_mode;
                session.reset();
                hide_popup(&mut popup, Some(state));
                tray.set_mode(tray_mode_of(mode));
                println!("daemon: mode {}", mode.as_str());
            }
            let live_encoding = daemon_output_encoding(state.encoding());
            if live_encoding != encoding {
                encoding = live_encoding;
                println!("daemon: encoding {}", encoding.as_str());
            }
            learning_enabled = state.learning();
            if state.take_rebind_request() {
                rebind_hotkeys(
                    state,
                    &mut mode_hotkey,
                    &mut hotkey,
                    &mut mode_hotkey_ready,
                    &mut hotkey_ready,
                    logger,
                );
            }
            let seq = state.apply_seq();
            if seq != last_apply_seq {
                last_apply_seq = seq;
                if matches!(state.last_apply(), crate::ipc::LiveApply::Deferred { .. }) {
                    logger.error(logging::Scope::Ipc, "settings_deferred", None);
                }
            }
            let failures = state.store_failures();
            if failures != last_store_failures {
                last_store_failures = failures;
                logger.error(logging::Scope::Ipc, "store_persist", None);
            }
        }

        // Toast expiry: hide the mode toast after ~800 ms unless composition
        // has already replaced it. Never blocks; checked once per poll.
        if toast_until_ms != 0 && now_ms() >= toast_until_ms {
            toast_until_ms = 0;
            if !session.is_composing() {
                hide_popup(&mut popup, ipc_state.as_deref());
            }
        }

        if !hook.capture_enabled() {
            // Pause cancels silently: already-typed roman text stays, the
            // popup hides, and no key is swallowed (the hook passes through).
            if session.is_composing() {
                session.reset();
                hide_popup(&mut popup, ipc_state.as_deref());
            }
            toast_until_ms = 0;
            // Drain injected and real events alike while paused.
            while hook.try_next().is_some() {}
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }

        if !mode.allows_composition() {
            // English mode: no composition, no candidates, no popup. The hook
            // keeps passing everything through; the buffer is already clear
            // from the toggle, and any stray state is dropped here before the
            // drain so a mode switch can never leak a partial buffer.
            if session.is_composing() {
                session.reset();
            }
            if toast_until_ms == 0 {
                hide_popup(&mut popup, ipc_state.as_deref());
            }
            while hook.try_next().is_some() {}
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }

        // Injected events (our own SendInput backspaces/text) never reach this
        // loop: the hook skips LLKHF_INJECTED before enqueueing. Only key-down
        // transitions drive the session; releases pass through untouched.
        while let Some(event) = hook.try_next() {
            if event.phase != KeyPhase::Down {
                continue;
            }
            let mut ctx = CompositionCtx {
                session: &mut session,
                popup: &mut popup,
                logger,
                learn_tx: &learn_tx,
                learn_dropped: &learn_dropped,
                compose_window: &mut compose_window,
                encoding,
                learning_enabled,
                state: ipc_state.as_deref(),
                injection: &injection_lock,
            };
            handle_key(event.virtual_key, &mut ctx);
        }

        std::thread::sleep(Duration::from_millis(10));
    }

    hook.stop();
    hotkey.stop();
    mode_hotkey.stop();
    if let Some((mut server, token_path)) = ipc_server {
        server.stop();
        logger.error(logging::Scope::Ipc, "ipc_stop", None);
        if crate::ipc::remove_token_file(&token_path).is_err() {
            logger.error(logging::Scope::Ipc, "ipc_token", None);
        }
    }
    tray.remove();
    // A dropped-learn count carries no text, so it is safe to print.
    let dropped = learn_dropped.load(Ordering::Relaxed);
    if dropped > 0 {
        println!("daemon: dropped {dropped} learn events (queue full)");
    }
    Ok(())
}

#[cfg(windows)]
fn toggle_capture(
    hook: &okkhor_win::WindowsKeyboardHook,
    tray: &okkhor_win::TrayIcon,
    state: Option<&std::sync::Arc<ProdState>>,
) {
    let enabled = hook.toggle_capture();
    tray.set_paused(!enabled);
    if let Some(state) = state {
        state.set_paused(!enabled);
    }
    println!(
        "daemon: capture {}",
        if enabled { "enabled" } else { "paused" }
    );
}

/// Launch the settings UI next to the daemon (tray Settings, `daemon --ui`).
/// A missing UI shows a tray balloon and logs one allowlisted code — never a
/// user path.
#[cfg(windows)]
fn open_settings_ui(tray: &okkhor_win::TrayIcon, logger: &logging::Logger) {
    match crate::ipc::launch_ui() {
        Ok(()) => logger.error(logging::Scope::Ipc, "ui_launch", None),
        Err(crate::ipc::UiLaunchError::NotFound) => {
            logger.error(logging::Scope::Ipc, "ui_missing", None);
            tray.show_balloon("Okkhor", "Settings UI was not found next to the daemon");
        }
        Err(crate::ipc::UiLaunchError::SpawnFailed) => {
            logger.error(logging::Scope::Ipc, "ui_launch_failed", None);
            tray.show_balloon("Okkhor", "Settings UI failed to start");
        }
    }
}

#[cfg(not(windows))]
fn run_daemon(_options: DaemonOptions) -> Result<(), Box<dyn Error>> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "daemon is only available on Windows",
    )
    .into())
}

#[cfg(windows)]
fn autostart_cli(action: AutostartAction) -> Result<(), Box<dyn Error>> {
    use okkhor_win::{disable_autostart, enable_autostart, is_autostart_enabled};

    let logger = logging::init();
    logger.startup();
    logger.command(action.command());

    match action {
        AutostartAction::Enable => {
            let executable = match std::env::current_exe() {
                Ok(path) => path,
                Err(error) => {
                    logger.error(logging::Scope::Daemon, "current_exe", error.raw_os_error());
                    return Err(error.into());
                }
            };
            match enable_autostart(&executable) {
                Ok(()) => {
                    println!("autostart: enabled for {}", executable.display());
                    Ok(())
                }
                Err(error) => {
                    logger.error(
                        logging::Scope::Registry,
                        "autostart_enable",
                        autostart_code(&error),
                    );
                    Err(error.into())
                }
            }
        }
        AutostartAction::Disable => match disable_autostart() {
            Ok(true) => {
                println!("autostart: disabled");
                Ok(())
            }
            Ok(false) => {
                println!("autostart: already disabled");
                Ok(())
            }
            Err(error) => {
                logger.error(
                    logging::Scope::Registry,
                    "autostart_disable",
                    autostart_code(&error),
                );
                Err(error.into())
            }
        },
        AutostartAction::Status => match is_autostart_enabled() {
            Ok(true) => {
                println!("autostart: enabled");
                Ok(())
            }
            Ok(false) => {
                println!("autostart: disabled");
                Ok(())
            }
            Err(error) => {
                logger.error(
                    logging::Scope::Registry,
                    "autostart_status",
                    autostart_code(&error),
                );
                Err(error.into())
            }
        },
    }
}

#[cfg(windows)]
fn autostart_code(error: &okkhor_win::AutostartError) -> Option<i32> {
    match error {
        okkhor_win::AutostartError::Win32 { code, .. } => Some(*code),
        _ => None,
    }
}

#[cfg(not(windows))]
fn autostart_cli(_action: ()) -> Result<(), Box<dyn Error>> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "autostart is only available on Windows",
    )
    .into())
}

#[cfg(windows)]
fn hook_demo() -> Result<(), Box<dyn Error>> {
    use okkhor_win::{
        KeyboardHook, PauseHotkey, SingleInstance, SingleInstanceError, WindowsKeyboardHook,
    };
    use std::time::{Duration, Instant};

    let _single_instance = match SingleInstance::acquire("Local\\Okkhor.HookDemo") {
        Ok(guard) => guard,
        Err(SingleInstanceError::AlreadyRunning) => {
            println!("hook-demo: another demo instance is already running");
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };

    let mut hook = WindowsKeyboardHook::new();
    hook.start()?;

    let mut pause_hotkey = PauseHotkey::new();
    let hotkey_ready = match pause_hotkey.start() {
        Ok(()) => true,
        Err(error) => {
            eprintln!("hook-demo: pause hotkey unavailable: {error}");
            false
        }
    };

    println!("hook-demo: installed for 5 seconds; press Esc to stop early.");
    if hotkey_ready {
        println!("hook-demo: Ctrl+Alt+Shift+P toggles capture.");
    }

    let started = Instant::now();
    let mut stop = false;
    while started.elapsed() < Duration::from_secs(5) && !stop {
        if hotkey_ready && pause_hotkey.try_next() {
            let enabled = hook.toggle_capture();
            println!(
                "hook-demo: capture {}",
                if enabled { "enabled" } else { "paused" }
            );
        }

        while let Some(event) = hook.try_next() {
            println!(
                "{:?} vk=0x{:02X} scan=0x{:02X}",
                event.phase, event.virtual_key, event.scan_code
            );
            if event.virtual_key == 0x1B {
                stop = true;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    hook.stop();
    pause_hotkey.stop();
    println!("hook-demo: stopped");
    Ok(())
}

#[cfg(not(windows))]
fn hook_demo() -> Result<(), Box<dyn Error>> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "hook-demo is only available on Windows",
    )
    .into())
}

#[cfg(windows)]
fn popup_demo() -> Result<(), Box<dyn Error>> {
    use okkhor_win::{SuggestionPopup, caret_anchor, place_popup};
    use std::time::Duration;

    let Some(anchor) = caret_anchor() else {
        println!("popup-demo: no caret anchor available");
        return Ok(());
    };
    let placement = place_popup(&anchor, 360, 140);
    let mut popup = SuggestionPopup::new()?;
    popup.show("Okkhor suggestions\nভালো   ভাল   ভালো", placement)?;
    println!(
        "popup-demo: shown at ({}, {}) for 3 seconds",
        placement.x, placement.y
    );
    std::thread::sleep(Duration::from_secs(3));
    popup.hide();
    Ok(())
}

#[cfg(windows)]
fn inject_cli(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    use okkhor_win::{TextInjector, WindowsInjector};
    use std::time::Duration;

    let mut delay = Duration::from_secs(3);
    let mut force_sendinput = false;
    let mut words = Vec::new();
    let mut index = 0;

    while index < arguments.len() {
        match arguments[index].as_str() {
            "--sendinput" => {
                force_sendinput = true;
                index += 1;
            }
            "--delay-ms" => {
                let value = arguments
                    .get(index + 1)
                    .ok_or("missing value for --delay-ms")?;
                delay = Duration::from_millis(value.parse()?);
                index += 2;
            }
            argument if argument.starts_with("--delay-ms=") => {
                let value = argument.trim_start_matches("--delay-ms=");
                delay = Duration::from_millis(value.parse()?);
                index += 1;
            }
            argument => {
                words.push(argument.to_owned());
                index += 1;
            }
        }
    }

    let text = words.join(" ");
    if text.is_empty() {
        return Err("usage: okkhor inject <text> [--sendinput] [--delay-ms N]".into());
    }

    println!(
        "inject: focus the target window within {} ms; use synthetic text only",
        delay.as_millis()
    );
    std::thread::sleep(delay);

    let injector = WindowsInjector;
    let method = if force_sendinput {
        injector.inject_unicode_only(&text)?
    } else {
        injector.inject(&text)?
    };
    println!("inject: completed with method {method:?}");
    Ok(())
}

#[cfg(not(windows))]
fn inject_cli(_arguments: &[String]) -> Result<(), Box<dyn Error>> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "inject is only available on Windows",
    )
    .into())
}

#[cfg(not(windows))]
fn popup_demo() -> Result<(), Box<dyn Error>> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "popup-demo is only available on Windows",
    )
    .into())
}

#[cfg(windows)]
fn tray_demo() -> Result<(), Box<dyn Error>> {
    use okkhor_win::{TrayEvent, TrayIcon};
    use std::time::{Duration, Instant};

    let logger = logging::init();
    logger.startup();
    logger.command(logging::Command::TrayDemo);

    let mut tray = match TrayIcon::new() {
        Ok(tray) => tray,
        Err(error) => {
            logger.error(logging::Scope::Tray, "tray_demo_create", None);
            return Err(error.into());
        }
    };

    println!(
        "tray-demo: left-click toggles mode; right-click the icon for the menu; stops after 10 seconds."
    );
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        while let Some(event) = tray.try_next() {
            match event {
                TrayEvent::TogglePause => {
                    let paused = !tray.paused();
                    tray.set_paused(paused);
                    println!("tray-demo: {}", if paused { "paused" } else { "active" });
                }
                TrayEvent::ToggleMode => {
                    let next = tray.mode().toggle();
                    tray.set_mode(next);
                    println!("tray-demo: mode {}", next.label());
                }
                TrayEvent::Settings => {
                    println!("tray-demo: settings (the daemon would launch okkhor-ui.exe)");
                }
                TrayEvent::Exit => {
                    println!("tray-demo: exit");
                    tray.remove();
                    return Ok(());
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    tray.remove();
    println!("tray-demo: stopped");
    Ok(())
}

#[cfg(not(windows))]
fn tray_demo() -> Result<(), Box<dyn Error>> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "tray-demo is only available on Windows",
    )
    .into())
}

fn self_check() -> Result<(), Box<dyn Error>> {
    let smriti = Smriti::new();
    let now = 2_000_u64;

    // English frequency: a repeated word should beat a shorter prefix.
    for _ in 0..10 {
        smriti.observe(LearningEvent::WordCommitted {
            word: "keyboard".to_owned(),
            language: Language::English,
            previous_word: None,
            timestamp_ms: 1_000,
            source: CommitSource::Typed,
        });
    }
    let english = smriti.rank(
        &SuggestionRequest::new("key", Language::English, 5).at(now),
        vec![
            Candidate::new("key", Language::English, CandidateSource::Dictionary),
            Candidate::new("keyboard", Language::English, CandidateSource::Dictionary),
        ],
    );
    assert_eq!(
        english.first().map(|item| item.text.as_str()),
        Some("keyboard")
    );

    // Bangla context: "আমি" should make "ভালো" the expected next word.
    smriti.observe(LearningEvent::WordCommitted {
        word: "আমি".to_owned(),
        language: Language::Bangla,
        previous_word: None,
        timestamp_ms: 1_000,
        source: CommitSource::Typed,
    });
    for _ in 0..10 {
        smriti.observe(LearningEvent::WordCommitted {
            word: "ভালো".to_owned(),
            language: Language::Bangla,
            previous_word: Some("আমি".to_owned()),
            timestamp_ms: 1_500,
            source: CommitSource::Typed,
        });
    }
    let bangla = smriti.rank(
        &SuggestionRequest::new("ভা", Language::Bangla, 5)
            .with_context(["আমি"])
            .at(now),
        vec![
            Candidate::new("খারাপ", Language::Bangla, CandidateSource::Dictionary),
            Candidate::new("ভালো", Language::Bangla, CandidateSource::Dictionary),
        ],
    );
    assert_eq!(bangla.first().map(|item| item.text.as_str()), Some("ভালো"));

    // Transliteration: remember that "bhalo" means "ভালো" for this user.
    for _ in 0..5 {
        smriti.observe(LearningEvent::TransliterationCommitted {
            roman_input: "bhalo".to_owned(),
            output: "ভালো".to_owned(),
            language: Language::Bangla,
            previous_word: None,
            timestamp_ms: 1_700,
        });
    }
    let pair_ranked = smriti.rank(
        &SuggestionRequest::new("ভা", Language::Bangla, 5)
            .with_roman_input("bhalo")
            .at(now),
        vec![
            Candidate::new("ভাল", Language::Bangla, CandidateSource::Transliteration),
            Candidate::new("ভালো", Language::Bangla, CandidateSource::Transliteration),
        ],
    );
    assert_eq!(
        pair_ranked.first().map(|item| item.text.as_str()),
        Some("ভালো")
    );

    // Persistence round-trip.
    let root = fresh_temp_dir();
    let repository = JsonFileRepository::new(root.join("learning.json"));
    repository.save(&smriti.snapshot())?;
    let loaded = repository.load()?.ok_or("snapshot was not written")?;
    let restored = Smriti::from_snapshot(loaded, SmritiConfig::default())?;
    let restored_ranked = restored.rank(
        &SuggestionRequest::new("key", Language::English, 5).at(now),
        vec![
            Candidate::new("key", Language::English, CandidateSource::Dictionary),
            Candidate::new("keyboard", Language::English, CandidateSource::Dictionary),
        ],
    );
    assert_eq!(
        restored_ranked.first().map(|item| item.text.as_str()),
        Some("keyboard")
    );

    let _ = fs::remove_dir_all(root);
    Ok(())
}

fn fresh_temp_dir() -> std::path::PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("okkhor-self-check-{}-{unique}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::{
        DaemonOptions, FontsAction, InputMode, OutputEncoding, apply_encoding, parse_daemon_args,
        parse_fonts_args, resolve_uninstall_target, select_fonts_dir,
    };

    #[test]
    fn self_check_passes() {
        super::self_check().expect("self-check");
    }

    /// Manual smoke: the zero-backspace path touches no Win32 API and installs
    /// no hook, so it is safe to run anywhere — but it stays ignored so CI
    /// never depends on Windows-only helpers.
    #[cfg(windows)]
    #[test]
    #[ignore = "manual smoke only; needs no hook, asserts the no-op erase path"]
    fn backspace_zero_is_noop_without_hook() {
        okkhor_win::send_backspaces(0).expect("zero backspaces");
    }

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| word.to_string()).collect()
    }

    #[test]
    fn daemon_args_default_to_unicode_bangla() {
        assert_eq!(
            parse_daemon_args(&args(&[])),
            Ok(DaemonOptions::new(
                OutputEncoding::Unicode,
                InputMode::Bangla
            ))
        );
        assert_eq!(OutputEncoding::Unicode.as_str(), "unicode");
        assert_eq!(OutputEncoding::Bijoy.as_str(), "bijoy");
        assert_eq!(InputMode::Bangla.as_str(), "bangla");
        assert_eq!(InputMode::English.as_str(), "english");
        assert_eq!(InputMode::default(), InputMode::Bangla);
    }

    #[test]
    fn daemon_args_accept_space_and_equals_forms() {
        assert_eq!(
            parse_daemon_args(&args(&["--encoding", "unicode"])),
            Ok(DaemonOptions::new(
                OutputEncoding::Unicode,
                InputMode::Bangla
            ))
        );
        assert_eq!(
            parse_daemon_args(&args(&["--encoding", "bijoy"])),
            Ok(DaemonOptions::new(OutputEncoding::Bijoy, InputMode::Bangla))
        );
        assert_eq!(
            parse_daemon_args(&args(&["--encoding=unicode"])),
            Ok(DaemonOptions::new(
                OutputEncoding::Unicode,
                InputMode::Bangla
            ))
        );
        assert_eq!(
            parse_daemon_args(&args(&["--encoding=bijoy"])),
            Ok(DaemonOptions::new(OutputEncoding::Bijoy, InputMode::Bangla))
        );
        assert_eq!(
            parse_daemon_args(&args(&["--mode", "bangla"])),
            Ok(DaemonOptions::new(
                OutputEncoding::Unicode,
                InputMode::Bangla
            ))
        );
        assert_eq!(
            parse_daemon_args(&args(&["--mode", "english"])),
            Ok(DaemonOptions::new(
                OutputEncoding::Unicode,
                InputMode::English
            ))
        );
        assert_eq!(
            parse_daemon_args(&args(&["--mode=english"])),
            Ok(DaemonOptions::new(
                OutputEncoding::Unicode,
                InputMode::English
            ))
        );
        // Combined flags in either order.
        assert_eq!(
            parse_daemon_args(&args(&["--encoding", "bijoy", "--mode", "english"])),
            Ok(DaemonOptions::new(
                OutputEncoding::Bijoy,
                InputMode::English
            ))
        );
        assert_eq!(
            parse_daemon_args(&args(&["--mode=english", "--encoding=bijoy"])),
            Ok(DaemonOptions::new(
                OutputEncoding::Bijoy,
                InputMode::English
            ))
        );
    }

    #[test]
    fn daemon_args_reject_unknown_or_missing_values() {
        // Unknown value, missing value, unknown flag, and extras all print
        // usage and exit 2 at the call site; parsing reports the usage text.
        assert!(parse_daemon_args(&args(&["--encoding"])).is_err());
        assert!(parse_daemon_args(&args(&["--encoding", "ansi"])).is_err());
        assert!(parse_daemon_args(&args(&["--encoding="])).is_err());
        assert!(parse_daemon_args(&args(&["--encoding=BIJOY"])).is_err());
        assert!(parse_daemon_args(&args(&["--mode"])).is_err());
        assert!(parse_daemon_args(&args(&["--mode", "bengali"])).is_err());
        assert!(parse_daemon_args(&args(&["--mode="])).is_err());
        assert!(parse_daemon_args(&args(&["--mode=BANGLA"])).is_err());
        assert!(parse_daemon_args(&args(&["--other"])).is_err());
        assert!(parse_daemon_args(&args(&["--encoding", "bijoy", "extra"])).is_err());
        assert!(parse_daemon_args(&args(&["--no-ipc=yes"])).is_err());
        assert!(parse_daemon_args(&args(&["--ui=no"])).is_err());
        assert_eq!(
            OutputEncoding::parse("unicode"),
            Some(OutputEncoding::Unicode)
        );
        assert_eq!(OutputEncoding::parse("bijoy"), Some(OutputEncoding::Bijoy));
        assert_eq!(OutputEncoding::parse("ansi"), None);
        assert_eq!(InputMode::parse("bangla"), Some(InputMode::Bangla));
        assert_eq!(InputMode::parse("english"), Some(InputMode::English));
        assert_eq!(InputMode::parse("bengali"), None);
        assert_eq!(InputMode::parse(""), None);
    }

    #[test]
    fn daemon_args_ipc_and_ui_flags() {
        // Bare flags in any order; everything else from the earlier tests
        // still parses with both flags off.
        assert_eq!(
            parse_daemon_args(&args(&[])),
            Ok(DaemonOptions {
                encoding: OutputEncoding::Unicode,
                mode: InputMode::Bangla,
                no_ipc: false,
                ui: false,
            })
        );
        assert_eq!(
            parse_daemon_args(&args(&["--no-ipc"])),
            Ok(DaemonOptions {
                encoding: OutputEncoding::Unicode,
                mode: InputMode::Bangla,
                no_ipc: true,
                ui: false,
            })
        );
        assert_eq!(
            parse_daemon_args(&args(&["--ui", "--mode=english", "--no-ipc"])),
            Ok(DaemonOptions {
                encoding: OutputEncoding::Unicode,
                mode: InputMode::English,
                no_ipc: true,
                ui: true,
            })
        );
        assert_eq!(
            parse_daemon_args(&args(&["--encoding", "bijoy", "--mode", "english", "--ui"])),
            Ok(DaemonOptions {
                encoding: OutputEncoding::Bijoy,
                mode: InputMode::English,
                no_ipc: false,
                ui: true,
            })
        );
    }

    #[test]
    fn input_mode_toggle_and_composition_gate() {
        // Bangla composes; English is pass-through; pause stays separate
        // (the daemon checks `capture_enabled` before this gate).
        assert_eq!(InputMode::Bangla.toggle(), InputMode::English);
        assert_eq!(InputMode::English.toggle(), InputMode::Bangla);
        assert!(InputMode::Bangla.allows_composition());
        assert!(!InputMode::English.allows_composition());
    }

    #[test]
    fn mode_transitions_compose_in_bangla_pass_through_in_english() {
        use crate::input_session::{InputSession, default_engine};

        let mut session: InputSession<_> = InputSession::new(default_engine());
        let mut mode = InputMode::Bangla;
        // Bangla: letters build the buffer and yield the table candidate.
        for byte in *b"bhal" {
            if mode.allows_composition() {
                assert!(session.push_letter(byte));
            }
        }
        assert!(session.is_composing());
        // Switching to English clears the buffer (nothing to hide here; the
        // daemon also hides the popup and drains the key queue).
        mode = mode.toggle();
        assert_eq!(mode, InputMode::English);
        session.reset();
        assert!(!session.is_composing());
        assert_eq!(session.buffer(), "");
        assert!(session.candidates().is_empty());
        // English: keys are not fed to the session at all.
        for byte in *b"bhal" {
            if mode.allows_composition() {
                assert!(session.push_letter(byte));
            }
        }
        assert!(!session.is_composing());
        assert_eq!(session.buffer(), "");
        // Back to Bangla: composition works again from a clean slate.
        mode = mode.toggle();
        assert_eq!(mode, InputMode::Bangla);
        for byte in *b"ami" {
            if mode.allows_composition() {
                assert!(session.push_letter(byte));
            }
        }
        assert!(session.is_composing());
        assert_eq!(session.buffer(), "ami");
    }

    /// Manual smoke: start the daemon, press F9 / left-click the tray, and
    /// confirm the toast + tooltip change without typing real text into an
    /// app. Ignored in CI so no hook is ever installed there.
    #[test]
    #[ignore = "manual smoke only: run `okkhor daemon --mode english`, toggle with F9/tray"]
    fn manual_mode_toggle_smoke_note() {
        // Steps: `okkhor daemon`, type `amar`+Space (commits আমার), press F9,
        // confirm `English` toast + `Okkhor - English` tooltip, type (passes
        // through), press F9 again, confirm `বাংলা` toast, type `amar`+Space.
    }

    #[test]
    fn encoding_selection_converts_amar_to_exact_bijoy_bytes() {
        // Golden from `okkhor-lang::bijoy` (ANSI 1 / SutonnyMJ): আমার -> Avgvi.
        let direct = okkhor_lang::bijoy::to_bijoy("আমার").expect("golden converts");
        assert_eq!(direct, "Avgvi");
        assert_eq!(
            apply_encoding("আমার", OutputEncoding::Bijoy).expect("bijoy encodes"),
            direct
        );
        // Unicode is a pass-through, whatever the input.
        assert_eq!(
            apply_encoding("আমার", OutputEncoding::Unicode).expect("unicode passes"),
            "আমার"
        );
        assert_eq!(
            apply_encoding("", OutputEncoding::Bijoy).expect("empty converts"),
            ""
        );
    }

    #[test]
    fn encoding_projection_never_carries_text() {
        // Over-long input fails with a static kind only (logging-safe).
        let long: String = "অ".repeat(okkhor_lang::bijoy::MAX_INPUT_CHARS + 1);
        let kind = apply_encoding(&long, OutputEncoding::Bijoy).unwrap_err();
        assert_eq!(kind, "bijoy_input_too_long");
        assert!(!kind.contains("অ"), "kind must not carry text: {kind}");
    }

    #[test]
    fn fonts_args_parse_status_install_uninstall() {
        assert_eq!(
            parse_fonts_args(&args(&["status"])),
            Ok(FontsAction::Status)
        );
        assert_eq!(
            parse_fonts_args(&args(&["install"])),
            Ok(FontsAction::Install)
        );
        assert_eq!(
            parse_fonts_args(&args(&["uninstall", "kalpurush.ttf"])),
            Ok(FontsAction::Uninstall("kalpurush.ttf".to_owned()))
        );
        // Missing target, unknown verb, and empty input are usage errors.
        assert!(parse_fonts_args(&args(&["uninstall"])).is_err());
        assert!(parse_fonts_args(&args(&["remove"])).is_err());
        assert!(parse_fonts_args(&[]).is_err());
    }

    #[test]
    fn uninstall_target_resolves_bundled_and_family_names() {
        assert_eq!(
            resolve_uninstall_target("kalpurush.ttf"),
            Some("kalpurush.ttf")
        );
        assert_eq!(
            resolve_uninstall_target("KALPURUSH.TTF"),
            Some("kalpurush.ttf")
        );
        assert_eq!(
            resolve_uninstall_target("SutonnyMJ"),
            Some("SutonnyMJ Regular.ttf")
        );
        assert_eq!(
            resolve_uninstall_target("kalpurush-ansi"),
            Some("kalpurush ANSI.ttf")
        );
        assert_eq!(
            resolve_uninstall_target("Noto"),
            Some("NotoSansBengali.ttf")
        );
        // Plain pass-through names survive; paths and non-fonts do not.
        assert_eq!(resolve_uninstall_target("other.ttf"), Some("other.ttf"));
        assert_eq!(resolve_uninstall_target(""), None);
        assert_eq!(resolve_uninstall_target("../evil.ttf"), None);
        assert_eq!(resolve_uninstall_target("font.exe"), None);
        assert_eq!(resolve_uninstall_target("font"), None);
    }

    #[test]
    fn font_dir_prefers_exe_dir_then_dev_checkout() {
        let base = super::fresh_temp_dir();
        let exe_fonts = base.join("exe").join("fonts");
        let dev_fonts = base.join("repo").join("assets").join("fonts");
        std::fs::create_dir_all(&exe_fonts).expect("exe fonts dir");
        std::fs::create_dir_all(&dev_fonts).expect("dev fonts dir");
        // Both present: the portable exe-adjacent directory wins.
        assert_eq!(select_fonts_dir(&exe_fonts, &dev_fonts), exe_fonts);
        // Exe-adjacent absent: the dev checkout is used (`cargo run` path).
        let missing = base.join("nowhere").join("fonts");
        assert_eq!(select_fonts_dir(&missing, &dev_fonts), dev_fonts);
        // Neither present: report the exe-adjacent path so status can list
        // every file as missing instead of failing.
        assert_eq!(
            select_fonts_dir(&missing, &base.join("also-missing")),
            missing
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn fonts_status_path_lists_roles_without_installing() {
        // Non-destructive: pure filesystem listing + Bijoy resolution over a
        // temp dir; installs nothing, starts no hook, touches no registry.
        let dir = super::fresh_temp_dir();
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join("kalpurush.ttf"), b"fake-font").expect("stage");
        let listed = okkhor_win::list_bundled_fonts(&dir);
        assert_eq!(listed.len(), okkhor_win::BUNDLED_FONTS.len());
        let first = listed
            .iter()
            .find(|item| item.file_name == "kalpurush.ttf")
            .expect("entry");
        assert!(first.present);
        assert_eq!(first.role.as_str(), "unicode-ui");
        let missing = listed
            .iter()
            .find(|item| item.file_name == "SutonnyMJ Regular.ttf")
            .expect("entry");
        assert!(!missing.present);
        // No Bijoy font staged: typed, non-fatal missing (Unicode keeps working).
        assert_eq!(
            okkhor_win::resolve_bijoy_font(&dir),
            okkhor_win::BijoyFontChoice::NoneAvailable
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

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
use okkhor_model::{HybridRanker, load_bundled};
use okkhor_storage::JsonFileRepository;

#[cfg(windows)]
mod logging;

mod ai_proxy;
mod english;
mod input_session;
mod ipc;
mod layout_engine;
mod nextword_model;
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
        Some("shutdown") => {
            if let Err(error) = shutdown_daemon() {
                eprintln!("shutdown failed: {error}");
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
        "Okkhor {}\n\nUSAGE:\n    okkhor [--help | --version | self-check | hook-demo | popup-demo | tray-demo | shutdown | daemon [--encoding unicode|bijoy] [--mode bangla|english] [--no-ipc] [--ui] | inject <text> [--sendinput] [--delay-ms N] | autostart <enable|disable|status> | fonts <status|install|uninstall <name>>]\n\n\
         `self-check` exercises the learning engine, ranking, and persistence.\n\
         `hook-demo` installs a global hook for 5 seconds and prints key events.\n\
         `popup-demo` shows a non-activating popup near the caret for 3 seconds.\n\
         `tray-demo` shows the tray menu for 10 seconds.\n\
         `shutdown` asks the running daemon to exit cleanly over the owner-only pipe.\n\
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
const DAEMON_USAGE: &str = "usage: okkhor daemon [--encoding unicode|bijoy] [--mode bangla|english] [--ranker baseline|hybrid] [--no-ipc] [--ui]";

/// Which ranker the daemon injects into the live suggestion engine.
///
/// `Baseline` is the deterministic `Smriti` order and the default. `Hybrid`
/// adds the bundled Tier 1 model; a load failure logs an allowlisted code and
/// falls back to `Baseline`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum RankerChoice {
    #[default]
    Baseline,
    Hybrid,
}

impl RankerChoice {
    /// Parse a CLI value; `None` for anything outside the allowlisted pair.
    fn parse(value: &str) -> Option<Self> {
        match value {
            "baseline" => Some(Self::Baseline),
            "hybrid" => Some(Self::Hybrid),
            _ => None,
        }
    }
}

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
    /// Final ranking stage: deterministic baseline or the hybrid reranker.
    ranker: RankerChoice,
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
            ranker: RankerChoice::Baseline,
            no_ipc: false,
            ui: false,
        }
    }
}

/// Parse `okkhor daemon` arguments. Pure: no I/O, no process state.
/// Accepts `--encoding <v>` / `--encoding=<v>`, `--mode <v>` /
/// `--mode=<v>`, `--ranker <baseline|hybrid>` / `--ranker=<v>`, and the bare
/// flags `--no-ipc` / `--ui`, in any order; anything else is a usage error.
fn parse_daemon_args(args: &[String]) -> Result<DaemonOptions, &'static str> {
    let mut encoding = OutputEncoding::Unicode;
    let mut mode = InputMode::Bangla;
    let mut ranker = RankerChoice::Baseline;
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
        } else if let Some(value) = arg.strip_prefix("--ranker=") {
            ranker = RankerChoice::parse(value).ok_or(DAEMON_USAGE)?;
            index += 1;
        } else if arg == "--encoding" {
            let value = args.get(index + 1).ok_or(DAEMON_USAGE)?;
            encoding = OutputEncoding::parse(value).ok_or(DAEMON_USAGE)?;
            index += 2;
        } else if arg == "--mode" {
            let value = args.get(index + 1).ok_or(DAEMON_USAGE)?;
            mode = InputMode::parse(value).ok_or(DAEMON_USAGE)?;
            index += 2;
        } else if arg == "--ranker" {
            let value = args.get(index + 1).ok_or(DAEMON_USAGE)?;
            ranker = RankerChoice::parse(value).ok_or(DAEMON_USAGE)?;
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
    options.ranker = ranker;
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

/// Locate the staged-font directory: `<exe-dir>\fonts` first, then the
/// portable folder root (`bin\..\fonts`), else the development checkout's
/// `assets/fonts` (found by walking up from the executable, then from the
/// working directory). Never contains hardcoded absolute paths or user names.
fn locate_fonts_dir() -> PathBuf {
    let exe_fonts = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.join("fonts")))
        .unwrap_or_else(|| PathBuf::from("fonts"));
    if exe_fonts.is_dir() {
        return exe_fonts;
    }
    // Portable beta layout: the engine lives in `bin\` and the fonts are a
    // sibling of `bin`, next to the user-facing `Okkhor.exe`.
    let parent_fonts = std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.parent()
                .and_then(Path::parent)
                .map(|root| root.join("fonts"))
        })
        .unwrap_or_else(|| PathBuf::from("fonts"));
    if parent_fonts.is_dir() {
        return parent_fonts;
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

/// A commit whose replacement is empty or all whitespace must never reach
/// `inject_commit`: that would erase the roman word and leave a blank. The
/// daemon abandons the composition and logs an allowlisted code instead.
#[cfg(windows)]
/// Raw text to re-emit when a fixed-layout key is unmapped or injection fails.
#[cfg(windows)]
fn raw_key_text(vk: u16, shift: bool) -> String {
    let plain = match vk {
        0x30..=0x39 => (vk as u8) as char,
        0x41..=0x5A => {
            let lower = (vk as u8) + 32;
            if shift {
                (lower - 32) as char
            } else {
                lower as char
            }
        }
        0x60..=0x69 => (0x30 + (vk - 0x60) as u8) as char,
        0x6A => '*',
        0x6B => '+',
        0x6D => '-',
        0x6E => '.',
        0x6F => '/',
        0xBA => ';',
        0xBB => '=',
        0xBC => ',',
        0xBD => '-',
        0xBE => '.',
        0xBF => '/',
        0xC0 => '`',
        0xDB => '[',
        0xDC => '\\',
        0xDD => ']',
        0xDE => '\'',
        _ => return String::new(),
    };
    plain.to_string()
}

/// Convert a fixed-layout character for the active output encoding. Pure so
/// the timeout-flush path can be tested without Win32.
#[cfg(windows)]
fn fixed_text_for_encoding(text: &str, encoding: crate::OutputEncoding) -> String {
    match encoding {
        crate::OutputEncoding::Unicode => text.to_owned(),
        crate::OutputEncoding::Bijoy => {
            crate::apply_encoding(text, encoding).unwrap_or_else(|_| text.to_owned())
        }
    }
}

/// Inject a fixed-layout character with the current output encoding. On a
/// failed injection the raw key is re-emitted so no keystroke is lost.
#[cfg(windows)]
fn inject_fixed_text(
    text: &str,
    raw: &str,
    encoding: crate::OutputEncoding,
    logger: &logging::Logger,
) {
    let converted = fixed_text_for_encoding(text, encoding);
    if let Err(error) = okkhor_win::inject_commit(0, &converted, false) {
        let code = match &error {
            okkhor_win::InjectError::Win32 { code, .. } => Some(*code as i32),
            _ => None,
        };
        logger.error(logging::Scope::Daemon, "inject_commit", code);
        if !raw.is_empty() {
            let _ = okkhor_win::inject_commit(0, raw, false);
        }
    }
}

fn injectable_text(text: &str) -> bool {
    // `str::trim` does not remove zero-width joiners/non-joiners. A string of
    // only those characters paints as nothing but would still erase the roman
    // word, so treat them as invisible too.
    text.chars().any(|ch| {
        !ch.is_whitespace()
            && !matches!(
                ch,
                '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' | '\u{FEFF}'
            )
    })
}

/// Whether the foreground window still belongs to the same typing target.
///
/// Browsers replace the HWND of the focused control with a child window when
/// the address-bar autocomplete opens. Comparing only HWNDs reset the roman
/// buffer after the first letter, so a commit erased `mar` and left the first
/// `a` in the document. A PID match means the same application, so the handle
/// change is benign. If either PID is unavailable, fall back to exact HWND
/// comparison.
/// What the English completion card should do for one keystroke.
///
/// A card is shown only with a fresh, known non-password sample. It is hidden
/// on a password sample or a field change. Flicker is avoided not by showing a
/// stale card, but by only *requesting* a new caret sample when a token starts
/// or the foreground changes — continuous typing keeps the last sample fresh.
#[cfg(windows)]
fn english_popup_action(
    latest_password: Option<bool>,
    fresh: bool,
    field_changed: bool,
) -> EnglishPopupAction {
    if field_changed || !fresh || latest_password != Some(false) {
        EnglishPopupAction::Hide
    } else {
        EnglishPopupAction::Show
    }
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnglishPopupAction {
    Show,
    Hide,
}

#[cfg(windows)]
fn same_target(pid: u32, hwnd: isize, current_pid: u32, current_hwnd: isize) -> bool {
    if pid == 0 || current_pid == 0 {
        hwnd != 0 && hwnd == current_hwnd
    } else {
        pid == current_pid && hwnd != 0 && current_hwnd != 0
    }
}

/// Ghost candidates offered after a commit. The hint shows one at a time and
/// Up/Down cycles the list, so this is the reachable set.
const GHOST_CANDIDATES: usize = 5;

/// One grey next-word hint after a commit.
#[derive(Debug, Default)]
struct GhostState {
    words: Vec<String>,
    selected: usize,
    visible: bool,
}

impl GhostState {
    fn hide(&mut self) {
        self.words.clear();
        self.selected = 0;
        self.visible = false;
    }
}

#[cfg(windows)]
fn run_daemon(options: DaemonOptions) -> Result<(), Box<dyn Error>> {
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, OnceLock, mpsc};
    use std::time::{Duration, Instant};

    use crate::english::{EnglishCommit, EnglishSession};
    use crate::layout_engine::{FixedComposition, FixedKeyOutcome, LayoutEngine};
    use okkhor_core::Language;
    use okkhor_lang::fixed_layout::{FixedLayout, ModifierSet, vk_to_key_name};
    use okkhor_learn::{LearningEvent, LearningSnapshot, Smriti, SmritiConfig};
    use okkhor_storage::{EncryptedJsonRepository, StorageError};
    use okkhor_win::{
        CaretAnchor, CaretTracker, InjectError, KeyPhase, KeyboardHook, ModeHotkey, PauseHotkey,
        SingleInstance, SingleInstanceError, SuggestionPopup, TrayEvent, TrayIcon, TrayMode,
        WindowsKeyboardHook, caret_anchor, ctrl_or_alt_pressed, foreground_window_id,
        foreground_window_pid, inject_commit, place_popup, send_tab,
    };

    use crate::input_session::{
        BackspaceOutcome, Commit, DefaultEngine, InputSession, VK_BACK, VK_DOWN, VK_ESCAPE,
        VK_LEFT, VK_OEM_PERIOD, VK_RETURN, VK_RIGHT, VK_SPACE, VK_TAB, VK_UP, default_engine,
        digit_char, is_modifier_key, vk_to_digit_index, vk_to_letter,
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
        compose_pid: &'a mut u32,
        /// Bangla text last written by inline preedit (empty when inactive).
        preedit_rendered: &'a mut String,
        /// Candidate words already counted as shown in this composition.
        shown_words: &'a mut HashSet<String>,
        /// Last committed Bangla word and its timestamp, for correction detection.
        last_commit: &'a mut Option<(String, u64)>,
        /// Word that a recent Backspace removed, awaiting the replacement.
        pending_correction: &'a mut Option<String>,
        encoding: crate::OutputEncoding,
        learning_enabled: bool,
        state: Option<&'a ProdState>,
        injection: &'a std::sync::Mutex<()>,
        caret: Option<&'a CaretTracker>,
        caret_generation: &'a mut u64,
    }

    fn hide_popup(popup: &mut Option<SuggestionPopup>, state: Option<&ProdState>) {
        if let Some(window) = popup {
            window.hide();
        }
        if let Some(state) = state {
            state.set_popup_visible(false);
        }
    }

    /// Use the tracker's newest sample, or fall back to the synchronous probe
    /// only when the tracker has not published a sample yet.
    fn caret_for_popup(caret: Option<&CaretTracker>, generation: &mut u64) -> Option<CaretAnchor> {
        if let Some(tracker) = caret {
            if let Some(sample) = tracker.latest() {
                *generation = (*generation).max(sample.generation);
                return Some(sample.anchor);
            }
        }
        caret_anchor()
    }

    /// Move an already-visible popup to a newer caret without re-showing it.
    fn apply_caret_anchor(
        popup: &mut Option<SuggestionPopup>,
        caret: Option<&CaretTracker>,
        logger: &logging::Logger,
        generation: &mut u64,
    ) {
        let Some(tracker) = caret else {
            return;
        };
        let Some(window) = popup.as_mut() else {
            return;
        };
        if !window.is_visible() {
            return;
        }
        let Some(sample) = tracker.latest() else {
            return;
        };
        if sample.generation <= *generation {
            return;
        }
        *generation = sample.generation;
        if let Err(error) = window.update_anchor(&sample.anchor) {
            logger.error(logging::Scope::Daemon, "popup_anchor", error.code());
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
        // Snapshot the session view before touching the window: the popup only
        // needs the candidate words and the highlight index.
        let candidates = ctx.session.candidates();
        let selected = ctx.session.selected();
        let composing = ctx.session.is_composing();
        if !composing || candidates.is_empty() {
            window.hide();
            if let Some(state) = state {
                state.set_popup_visible(false);
            }
            return;
        }
        // A key changed the composition or selection: ask the dedicated UIA
        // thread for a fresh caret without blocking this worker.
        if let Some(tracker) = ctx.caret {
            let _ = tracker.request();
        }
        let anchor = caret_for_popup(ctx.caret, ctx.caret_generation).or_else(|| window.anchor());
        match anchor {
            Some(anchor) => {
                if let Err(error) = window.show_candidates(candidates, selected, &anchor) {
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
                if let Some(state) = state {
                    state.set_popup_visible(false);
                }
            }
        }
        emit_suggestion_shown(ctx);
    }

    /// Render the current best Bangla candidate into the document as an
    /// inline preedit. Only the suffix after the common prefix is rewritten,
    /// and the whole edit is one atomic `SendInput` batch.
    fn refresh_inline_preedit(ctx: &mut CompositionCtx<'_>) {
        if ctx.encoding != crate::OutputEncoding::Unicode {
            return;
        }
        let next = if ctx.session.is_composing() {
            ctx.session
                .candidates()
                .get(ctx.session.selected())
                .cloned()
                .unwrap_or_default()
        } else {
            String::new()
        };
        let plan = okkhor_core::plan_edit(ctx.preedit_rendered, &next);
        if plan.is_noop() {
            *ctx.preedit_rendered = next;
            return;
        }
        if let Err(error) = okkhor_win::inject_commit(plan.erase_utf16, &plan.insert, false) {
            ctx.logger.error(
                logging::Scope::Daemon,
                "preedit_inject",
                inject_code(&error),
            );
            return;
        }
        *ctx.preedit_rendered = next;
    }

    /// Finalize an inline-preedit word. The document already contains the
    /// rendered Bangla, so only the delimiter is injected.
    fn finalize_inline(
        ctx: &mut CompositionCtx<'_>,
        roman: String,
        previous_word: Option<String>,
        trailing_space: bool,
        newline: bool,
    ) {
        let rendered = ctx.preedit_rendered.clone();
        let committed = ctx.session.commit(trailing_space);
        ctx.preedit_rendered.clear();
        hide_popup(ctx.popup, ctx.state);
        if newline {
            let _ = okkhor_win::send_enter();
        } else if trailing_space {
            let _ = okkhor_win::inject_commit(0, "", true);
        } else if committed.is_none() {
            // Nothing to accept: preserve the Tab the hook swallowed.
            let _ = okkhor_win::send_tab();
        }
        let Some(commit) = committed else {
            return;
        };
        let unicode_text = if rendered.is_empty() {
            commit.text
        } else {
            rendered
        };
        if !ctx.learning_enabled || !has_bengali(&unicode_text) {
            ctx.shown_words.clear();
            return;
        }
        emit_suggestion_accepted(ctx, &unicode_text, previous_word.clone());
        emit_correction(ctx, &unicode_text);
        *ctx.last_commit = Some((unicode_text.clone(), now_ms()));
        ctx.shown_words.clear();
        let event = LearningEvent::TransliterationCommitted {
            roman_input: roman,
            output: unicode_text,
            language: Language::Bangla,
            previous_word,
            timestamp_ms: now_ms(),
        };
        if ctx.learn_tx.try_send(event).is_err() {
            ctx.learn_dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Show or hide the English completion popup at the caret.
    ///
    /// A password sample or a new foreground window hides it at once. A
    /// pending caret request no longer blanks an existing card: it stays where
    /// it is until the sample lands, which removes the per-keystroke flicker.
    fn refresh_english_popup(
        popup: &mut Option<SuggestionPopup>,
        english: &EnglishSession,
        state: Option<&ProdState>,
        caret: Option<&CaretTracker>,
        caret_generation: &mut u64,
        last_caret_request: u64,
        field_changed: bool,
    ) {
        let Some(window) = popup.as_mut() else {
            return;
        };
        if !english.is_visible() {
            window.hide();
            if let Some(state) = state {
                state.set_popup_visible(false);
            }
            return;
        }
        let action = if let Some(tracker) = caret {
            let fresh = last_caret_request == 0
                || tracker
                    .latest()
                    .is_some_and(|sample| sample.generation >= last_caret_request);
            english_popup_action(
                tracker.latest().map(|sample| sample.password),
                fresh,
                field_changed,
            )
        } else {
            EnglishPopupAction::Show
        };
        if action == EnglishPopupAction::Hide {
            window.hide();
            if let Some(state) = state {
                state.set_popup_visible(false);
            }
            return;
        }
        match caret_for_popup(caret, caret_generation).or_else(|| window.anchor()) {
            Some(anchor) => {
                if window
                    .show_candidates(english.suggestions(), english.selected(), &anchor)
                    .is_err()
                {
                    window.hide();
                    if let Some(state) = state {
                        state.set_popup_visible(false);
                    }
                } else if let Some(state) = state {
                    state.set_popup_visible(true);
                }
            }
            None => {
                window.hide();
                if let Some(state) = state {
                    state.set_popup_visible(false);
                }
            }
        }
    }

    /// Erase the English token and inject the completion under the shared
    /// injection lock. Returns false when the caller should re-emit Tab.
    fn apply_english_commit(
        commit: EnglishCommit,
        logger: &logging::Logger,
        injection: &std::sync::Mutex<()>,
    ) -> bool {
        let _injection = injection.lock().unwrap_or_else(|error| error.into_inner());
        if let Err(error) = inject_commit(commit.erase_chars, &commit.text, false) {
            logger.error(logging::Scope::Daemon, "inject_commit", inject_code(&error));
            return false;
        }
        true
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

    fn has_bengali(text: &str) -> bool {
        text.chars()
            .any(|ch| ('\u{0980}'..='\u{09FF}').contains(&ch))
    }

    /// Count each candidate once per composition as "shown". The card repaints
    /// many times per keystroke, so a raw emit would inflate the denominator.
    fn emit_suggestion_shown(ctx: &mut CompositionCtx<'_>) {
        if !ctx.learning_enabled || !ctx.session.is_composing() {
            return;
        }
        let candidates: Vec<String> = ctx.session.candidates().to_vec();
        let now = now_ms();
        for word in candidates {
            if !has_bengali(&word) || !ctx.shown_words.insert(word.clone()) {
                continue;
            }
            let event = LearningEvent::SuggestionShown {
                word,
                language: Language::Bangla,
                timestamp_ms: now,
            };
            if ctx.learn_tx.try_send(event).is_err() {
                ctx.learn_dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Count an accepted word only when it was actually shown for this
    /// composition, so acceptance never exceeds the denominator.
    fn emit_suggestion_accepted(
        ctx: &mut CompositionCtx<'_>,
        word: &str,
        previous_word: Option<String>,
    ) {
        if !ctx.learning_enabled || !has_bengali(word) || !ctx.shown_words.remove(word) {
            return;
        }
        let event = LearningEvent::SuggestionAccepted {
            word: word.to_owned(),
            language: Language::Bangla,
            previous_word,
            timestamp_ms: now_ms(),
        };
        if ctx.learn_tx.try_send(event).is_err() {
            ctx.learn_dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// A Backspace soon after a commit records the removed word; the next
    /// commit supplies the replacement. This is a weak signal and never
    /// deletes anything from the learned store.
    fn emit_correction(ctx: &mut CompositionCtx<'_>, to: &str) {
        let Some(from) = ctx.pending_correction.take() else {
            return;
        };
        if from == to || !has_bengali(&from) || !has_bengali(to) || !ctx.learning_enabled {
            return;
        }
        let event = LearningEvent::Correction {
            from,
            to: to.to_owned(),
            language: Language::Bangla,
            timestamp_ms: now_ms(),
        };
        if ctx.learn_tx.try_send(event).is_err() {
            ctx.learn_dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn show_ghost(ctx: &mut CompositionCtx<'_>, ghost: &mut GhostState) {
        if ghost.words.is_empty() {
            return;
        }
        let Some(window) = ctx.popup.as_mut() else {
            return;
        };
        let anchor = caret_for_popup(ctx.caret, ctx.caret_generation).or_else(|| window.anchor());
        let Some(anchor) = anchor else {
            return;
        };
        let word = ghost.words[ghost.selected].clone();
        if window.show_ghost(&word, &anchor).is_ok() {
            ghost.visible = true;
        }
    }

    fn hide_ghost(ctx: &mut CompositionCtx<'_>, ghost: &mut GhostState) {
        ghost.hide();
        if let Some(window) = ctx.popup.as_mut() {
            window.hide();
        }
        if let Some(state) = ctx.state {
            state.set_popup_visible(false);
        }
    }

    fn accept_ghost(ctx: &mut CompositionCtx<'_>, ghost: &mut GhostState) {
        let Some(word) = ghost.words.get(ghost.selected).cloned() else {
            hide_ghost(ctx, ghost);
            return;
        };
        let text = format!("{word} ");
        if let Err(error) = okkhor_win::inject_commit(0, &text, false) {
            ctx.logger
                .error(logging::Scope::Daemon, "ghost_inject", inject_code(&error));
        }
        ghost.hide();
        hide_popup(ctx.popup, ctx.state);
    }

    fn emit_english_commit(
        learn_tx: &mpsc::SyncSender<LearningEvent>,
        learn_dropped: &AtomicU64,
        learning_enabled: bool,
        word: &str,
        previous_word: Option<String>,
    ) {
        if !learning_enabled || word.is_empty() {
            return;
        }
        let event = LearningEvent::WordCommitted {
            word: word.to_owned(),
            language: Language::English,
            previous_word,
            timestamp_ms: now_ms(),
            source: okkhor_learn::CommitSource::AcceptedSuggestion,
        };
        if learn_tx.try_send(event).is_err() {
            learn_dropped.fetch_add(1, Ordering::Relaxed);
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
    fn do_commit(
        ctx: &mut CompositionCtx<'_>,
        commit: Commit,
        roman: String,
        newline: bool,
        previous_word: Option<String>,
    ) -> bool {
        let foreground = foreground_window_id();
        let pid = foreground_window_pid();
        if !same_target(*ctx.compose_pid, *ctx.compose_window, pid, foreground) {
            ctx.session.reset();
            hide_popup(ctx.popup, ctx.state);
            ctx.logger
                .error(logging::Scope::Daemon, "focus_changed", None);
            return false;
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
        // Defense in depth for the owner's "spaces instead of Bangla" report:
        // never erase a word when the replacement carries no visible text.
        // The normal source (engine candidate text) is non-empty and the
        // whitespace-only case would otherwise look like a blank insertion.
        if !injectable_text(&inject_text) {
            ctx.logger
                .error(logging::Scope::Daemon, "empty_commit", None);
            ctx.session.reset();
            hide_popup(ctx.popup, ctx.state);
            return false;
        }
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
            return false;
        }
        // Enter is swallowed by the hook while composing, so the newline the
        // target never received is re-emitted here, after the replacement.
        if newline {
            if let Err(error) = okkhor_win::send_enter() {
                ctx.logger
                    .error(logging::Scope::Daemon, "inject_commit", inject_code(&error));
            }
        }
        hide_popup(ctx.popup, ctx.state);
        if !ctx.learning_enabled {
            ctx.shown_words.clear();
            return true;
        }
        // A raw Latin fallback (unknown phonetic word) is not a roman -> Bangla
        // pair. Recording it would poison the personal transliteration map.
        if !has_bengali(&unicode_text) {
            ctx.shown_words.clear();
            return true;
        }
        emit_suggestion_accepted(ctx, &unicode_text, previous_word.clone());
        emit_correction(ctx, &unicode_text);
        *ctx.last_commit = Some((unicode_text.clone(), now_ms()));
        ctx.shown_words.clear();
        // Queue learning for the background thread. The channel is bounded and
        // `try_send` never blocks, so a stalled learn thread can never stall
        // the key path; the drop is counted (a count carries no text) and
        // reported once at shutdown. A disconnected receiver drops the same way.
        // `Smriti::observe` performs no I/O.
        let event = LearningEvent::TransliterationCommitted {
            roman_input: roman,
            output: unicode_text,
            language: Language::Bangla,
            previous_word,
            timestamp_ms: now_ms(),
        };
        if ctx.learn_tx.try_send(event).is_err() {
            ctx.learn_dropped.fetch_add(1, Ordering::Relaxed);
        }
        true
    }

    fn cancel_composition(ctx: &mut CompositionCtx<'_>) {
        // `cancel` returns the raw roman buffer, which the caller deliberately
        // drops: already-typed text stays untouched in the target.
        let _ = ctx.session.cancel();
        // Inline preedit leaves the already-rendered Bangla in the document
        // (Kontho behaviour); only the tracking state is dropped.
        ctx.preedit_rendered.clear();
        ctx.shown_words.clear();
        *ctx.pending_correction = None;
        hide_popup(ctx.popup, ctx.state);
    }

    /// Record or re-validate the composition window around one keystroke.
    ///
    /// When focus moved mid-composition the stale buffer is dropped (the roman
    /// text stays where it was typed) and the window id is refreshed.
    fn track_focus(ctx: &mut CompositionCtx<'_>) {
        let foreground = foreground_window_id();
        let pid = foreground_window_pid();
        let same = same_target(*ctx.compose_pid, *ctx.compose_window, pid, foreground);
        if ctx.session.is_composing() && !same {
            ctx.session.reset();
            ctx.preedit_rendered.clear();
            ctx.shown_words.clear();
            *ctx.pending_correction = None;
            hide_popup(ctx.popup, ctx.state);
        }
        // A child-window swap inside the same application is not a focus
        // change: re-anchor so commit's own focus check agrees. This is what
        // fixes the stray first `a` in browser address bars.
        if !ctx.session.is_composing() || same {
            *ctx.compose_window = foreground;
            *ctx.compose_pid = pid;
        }
    }

    #[allow(clippy::too_many_lines)]
    fn handle_key(
        vk: u16,
        ctx: &mut CompositionCtx<'_>,
        fixed: Option<&FixedLayout>,
        fixed_composition: &mut FixedComposition,
        fixed_enabled: bool,
        inline_enabled: bool,
        ghost: &mut GhostState,
    ) {
        if ghost.visible {
            match vk {
                VK_TAB => {
                    accept_ghost(ctx, ghost);
                    return;
                }
                VK_UP => {
                    let len = ghost.words.len() as i32;
                    if len > 0 {
                        ghost.selected = (ghost.selected as i32 - 1).rem_euclid(len) as usize;
                        show_ghost(ctx, ghost);
                    }
                    return;
                }
                VK_DOWN => {
                    let len = ghost.words.len() as i32;
                    if len > 0 {
                        ghost.selected = (ghost.selected as i32 + 1).rem_euclid(len) as usize;
                        show_ghost(ctx, ghost);
                    }
                    return;
                }
                VK_ESCAPE => {
                    hide_ghost(ctx, ghost);
                    return;
                }
                _ => hide_ghost(ctx, ghost),
            }
        }
        if fixed_enabled {
            if let Some(layout) = fixed {
                // AltGr is a layout modifier here, not a shortcut chord.
                if ctrl_or_alt_pressed() && !okkhor_win::altgr_pressed() {
                    return;
                }
                track_focus(ctx);
                if let Some(key) = vk_to_key_name(vk) {
                    let is_digit = key.starts_with("Num")
                        || key.starts_with("Key_")
                            && key.as_bytes().get(4).is_some_and(u8::is_ascii_digit);
                    if is_digit && ctx.encoding == crate::OutputEncoding::Bijoy {
                        return;
                    }
                    let flags = okkhor_win::modifier_state();
                    let modifiers = ModifierSet {
                        shift: flags.shift,
                        altgr: flags.altgr,
                    };
                    let raw = raw_key_text(vk, flags.shift);
                    let fallback = raw.chars().next().unwrap_or('?');
                    match fixed_composition.push(layout, key, modifiers, Instant::now(), fallback) {
                        FixedKeyOutcome::Hold => {}
                        FixedKeyOutcome::Text(text) => {
                            inject_fixed_text(&text, &raw, ctx.encoding, ctx.logger);
                        }
                        FixedKeyOutcome::PassThrough(text) => {
                            // Unmapped for this modifier pair: the hook swallowed
                            // it, so re-emit the raw key instead of dropping it.
                            let _ = okkhor_win::inject_commit(0, &text, false);
                        }
                    }
                    return;
                }
            }
        }
        if inline_enabled {
            if let Some(letter) = vk_to_letter(vk) {
                if ctrl_or_alt_pressed() {
                    let _ = ctx.session.cancel();
                    ctx.preedit_rendered.clear();
                    return;
                }
                track_focus(ctx);
                if ctx.session.push_letter(letter) {
                    refresh_popup(ctx);
                    refresh_inline_preedit(ctx);
                } else {
                    let _ = ctx.session.cancel();
                    ctx.preedit_rendered.clear();
                }
                return;
            }
        }
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
            } else {
                // Buffer full (or unusable letter): the target received this
                // character but the session cannot track it, so the erase
                // count would drift. End the composition; the roman text stays
                // as typed and the next letter starts a new buffer.
                cancel_composition(ctx);
            }
            return;
        }
        if let Some(index) = vk_to_digit_index(vk) {
            if !ctx.session.is_composing() {
                // Idle digit in Bangla Unicode: the hook swallowed it so the
                // app gets a Bengali numeral (১২৩) instead of ASCII.
                if ctx.encoding == crate::OutputEncoding::Unicode {
                    let text = digit_char(index, true).to_string();
                    if let Err(error) = okkhor_win::inject_commit(0, &text, false) {
                        ctx.logger.error(
                            logging::Scope::Daemon,
                            "inject_commit",
                            inject_code(&error),
                        );
                    }
                }
                return;
            }
            let roman = ctx.session.buffer().to_owned();
            let previous_word = ctx.session.previous_word().map(str::to_owned);
            if let Some(commit) = ctx.session.select_digit(index) {
                do_commit(ctx, commit, roman, false, previous_word);
            } else {
                // Out-of-range selection: the hook swallowed the digit, so
                // re-emit it as text after ending the composition.
                ctx.session.reset();
                hide_popup(ctx.popup, ctx.state);
                let text =
                    digit_char(index, ctx.encoding == crate::OutputEncoding::Unicode).to_string();
                if let Err(error) = okkhor_win::inject_commit(0, &text, false) {
                    ctx.logger
                        .error(logging::Scope::Daemon, "inject_commit", inject_code(&error));
                }
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
                    // A Backspace within 3 s of a commit is treated as the
                    // start of a correction; the next commit supplies the
                    // replacement. The key still passes through untouched.
                    if let Some((word, at)) = ctx.last_commit.take()
                        && now_ms().saturating_sub(at) <= 3_000
                    {
                        *ctx.pending_correction = Some(word);
                    }
                    return;
                }
                if ctx.session.backspace() != BackspaceOutcome::Empty {
                    refresh_popup(ctx);
                }
                if inline_enabled {
                    refresh_inline_preedit(ctx);
                }
            }
            VK_SPACE | VK_RETURN => {
                if !ctx.session.is_composing() {
                    return;
                }
                if inline_enabled {
                    let roman = ctx.session.buffer().to_owned();
                    let previous_word = ctx.session.previous_word().map(str::to_owned);
                    finalize_inline(ctx, roman, previous_word, vk == VK_SPACE, vk == VK_RETURN);
                    return;
                }
                let roman = ctx.session.buffer().to_owned();
                let newline = vk == VK_RETURN;
                let previous_word = ctx.session.previous_word().map(str::to_owned);
                match ctx.session.commit(vk == VK_SPACE) {
                    Some(commit) => {
                        do_commit(ctx, commit, roman, newline, previous_word);
                    }
                    // No candidates: the roman text stays as typed. The key
                    // was swallowed by the hook, so re-emit it explicitly.
                    None => {
                        ctx.session.reset();
                        hide_popup(ctx.popup, ctx.state);
                        let injected = if newline {
                            okkhor_win::send_enter()
                        } else {
                            okkhor_win::inject_commit(0, "", true)
                        };
                        if let Err(error) = injected {
                            ctx.logger.error(
                                logging::Scope::Daemon,
                                "inject_commit",
                                inject_code(&error),
                            );
                        }
                    }
                }
            }
            VK_LEFT | VK_UP => {
                if ctx.session.is_composing() {
                    ctx.session.move_selection(-1);
                    refresh_popup(ctx);
                    if inline_enabled {
                        refresh_inline_preedit(ctx);
                    }
                }
            }
            VK_RIGHT | VK_DOWN => {
                if ctx.session.is_composing() {
                    ctx.session.move_selection(1);
                    refresh_popup(ctx);
                    if inline_enabled {
                        refresh_inline_preedit(ctx);
                    }
                }
            }
            VK_TAB if ctx.session.is_composing() => {
                // Tab accepts the highlighted candidate, like the English
                // completion list. It no longer cancels the composition, which
                // is why the selected Bangla word was never typed.
                let roman = ctx.session.buffer().to_owned();
                let previous_word = ctx.session.previous_word().map(str::to_owned);
                if inline_enabled {
                    finalize_inline(ctx, roman, previous_word, false, false);
                    return;
                }
                match ctx.session.commit(false) {
                    Some(commit) => {
                        do_commit(ctx, commit, roman, false, previous_word);
                    }
                    None => {
                        ctx.session.reset();
                        hide_popup(ctx.popup, ctx.state);
                        let _ = okkhor_win::send_tab();
                    }
                }
            }
            VK_ESCAPE if ctx.session.is_composing() => {
                cancel_composition(ctx);
            }
            VK_OEM_PERIOD if ctx.encoding == crate::OutputEncoding::Unicode => {
                // Bangla Unicode: the hook swallowed this `.` so the sentence
                // ends with the Bengali danda (।). Commit any live composition
                // first, then emit the danda on its own.
                let roman = ctx.session.buffer().to_owned();
                let previous_word = ctx.session.previous_word().map(str::to_owned);
                let ready = if ctx.session.is_composing() {
                    match ctx.session.commit(false) {
                        Some(commit) => do_commit(ctx, commit, roman, false, previous_word),
                        None => {
                            ctx.session.reset();
                            hide_popup(ctx.popup, ctx.state);
                            true
                        }
                    }
                } else {
                    true
                };
                if ready {
                    if let Err(error) = okkhor_win::inject_commit(0, "।", false) {
                        ctx.logger.error(
                            logging::Scope::Daemon,
                            "inject_commit",
                            inject_code(&error),
                        );
                    }
                }
            }
            _ => {
                // Any other key (punctuation, Home/End, Delete, function keys)
                // would shift the target text relative to the buffer. End the
                // composition and let the key act normally, so a later commit
                // can never erase the wrong characters.
                if !is_modifier_key(vk) {
                    cancel_composition(ctx);
                }
            }
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

    // IPC pipe starts as soon as settings and the word store are ready, before
    // the slower learning/engine/caret setup. The UI can connect immediately
    // instead of polling for up to eight seconds while those load.
    let mut ipc_server: Option<(crate::ipc::IpcServer, std::path::PathBuf)> = None;
    if !options.no_ipc {
        if let Some(state) = ipc_state.clone() {
            match crate::paths::token_path() {
                Ok(token_path) => match crate::ipc::install_token(&token_path) {
                    Ok(token) => match crate::ipc::IpcServer::start(state, token) {
                        Ok(server) => {
                            logger.info(logging::Scope::Ipc, "ipc_start");
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

    // Learning: load the encrypted snapshot once, share it with the engine
    // (read locks for ranking) and the background thread (writes + debounced
    // saves). A corrupt/missing file starts empty; typing never depends on it.
    let learning_path = crate::paths::learning_path().ok();
    let smriti = match &learning_path {
        Some(path) => {
            let repository = EncryptedJsonRepository::<
                LearningSnapshot,
                crate::words_store::DpapiProtector,
            >::new(
                path.clone(), crate::words_store::DpapiProtector::learning()
            );
            match repository.load() {
                Ok(snapshot) => match Smriti::from_snapshot(snapshot, SmritiConfig::default()) {
                    Ok(smriti) => Arc::new(smriti),
                    Err(_) => {
                        logger.error(logging::Scope::Daemon, "learning_snapshot", None);
                        Arc::new(Smriti::new())
                    }
                },
                Err(StorageError::NotFound) => Arc::new(Smriti::new()),
                Err(_) => {
                    logger.error(logging::Scope::Daemon, "learning_load", None);
                    Arc::new(Smriti::new())
                }
            }
        }
        None => Arc::new(Smriti::new()),
    };

    // Parse the embedded next-word priors off the typing path. Validation is a
    // one-time ~80 ms walk over both blobs; doing it now keeps the first ghost
    // hint after login instant.
    {
        let spawned = std::thread::Builder::new()
            .name("okkhor-prior".to_owned())
            .spawn(|| {
                let _ = okkhor_engine::nextword::global(Language::Bangla);
                let _ = okkhor_engine::nextword::global(Language::English);
            });
        if spawned.is_err() {
            logger.error(logging::Scope::Daemon, "prior_prewarm", None);
        }
    }

    // Optional global next-word models. Loading reads a few hundred megabytes,
    // so it happens on a worker thread; the loop only ever reads the ready
    // `OnceLock`. A missing or corrupt file keeps the prior + Smriti path.
    let models: Arc<OnceLock<nextword_model::Models>> = Arc::new(OnceLock::new());
    {
        let models = Arc::clone(&models);
        let directory = crate::paths::models_dir().ok();
        let spawned = std::thread::Builder::new()
            .name("okkhor-model".to_owned())
            .spawn(move || {
                let (loaded, failures) = match &directory {
                    Some(path) => nextword_model::Models::load(path),
                    None => (nextword_model::Models::default(), Vec::new()),
                };
                for (code, _error) in failures {
                    eprintln!("next-word model: {code}");
                }
                let _ = models.set(loaded);
            });
        if spawned.is_err() {
            logger.error(logging::Scope::Daemon, "model_spawn", None);
        }
    }
    let mut recent = nextword_model::Recent::new(8);
    const LEARN_QUEUE_CAP: usize = 1024;
    let (learn_tx, learn_rx) = mpsc::sync_channel::<LearningEvent>(LEARN_QUEUE_CAP);
    let learn_dropped = Arc::new(AtomicU64::new(0));
    let learning_save_failures = Arc::new(AtomicU64::new(0));
    {
        let smriti = Arc::clone(&smriti);
        let learning_save_failures = Arc::clone(&learning_save_failures);
        let repository = learning_path.clone().map(|path| {
            EncryptedJsonRepository::<LearningSnapshot, crate::words_store::DpapiProtector>::new(
                path,
                crate::words_store::DpapiProtector::learning(),
            )
        });
        let spawned = std::thread::Builder::new()
            .name("okkhor-learn".to_owned())
            .spawn(move || {
                let mut dirty = 0u32;
                let mut last_save = Instant::now();
                for event in learn_rx {
                    smriti.observe(event);
                    dirty += 1;
                    if dirty >= 64 || last_save.elapsed() >= Duration::from_secs(30) {
                        if let Some(repository) = &repository {
                            if repository.save(&smriti.snapshot()).is_err() {
                                learning_save_failures.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        dirty = 0;
                        last_save = Instant::now();
                    }
                }
                if let Some(repository) = &repository {
                    if repository.save(&smriti.snapshot()).is_err() {
                        learning_save_failures.fetch_add(1, Ordering::Relaxed);
                    }
                }
            });
        if spawned.is_err() {
            logger.error(logging::Scope::Daemon, "learn_thread", None);
        }
    }

    let mut popup = match SuggestionPopup::new() {
        Ok(window) => Some(window),
        Err(error) => {
            logger.error(logging::Scope::Daemon, "popup_create", error.code());
            None
        }
    };

    // One dedicated thread owns COM/UIA for caret and password probes. The
    // daemon only reads the newest published sample, so typing never waits on
    // UI Automation. A spawn failure degrades to the old synchronous fallback.
    let caret_tracker = match CaretTracker::spawn() {
        Ok(tracker) => Some(tracker),
        Err(_) => {
            logger.error(logging::Scope::Daemon, "caret_thread", None);
            None
        }
    };
    let mut last_anchor_generation: u64 = 0;
    let mut last_caret_request: u64 = 0;
    let mut last_english_token = String::new();
    let mut last_english_visible = false;
    let mut last_english_foreground = foreground_window_id();
    let mut inline_was_enabled = false;

    let baseline: Arc<dyn Ranker + Send + Sync> = smriti.clone();
    let ranker: Arc<dyn Ranker + Send + Sync> = match options.ranker {
        RankerChoice::Hybrid => match load_bundled() {
            Ok(model) => Arc::new(HybridRanker::new(Arc::clone(&smriti), model)),
            Err(_) => {
                // Allowlisted code only; typing continues on the baseline.
                logger.error(logging::Scope::Engine, "model_load", None);
                Arc::clone(&baseline)
            }
        },
        RankerChoice::Baseline => baseline,
    };
    let mut layout_code = ipc_state
        .as_deref()
        .map(|state| state.status().layout)
        .unwrap_or_else(|| "avro".to_owned());
    let (layout_engine, layout_health) =
        LayoutEngine::from_settings(&layout_code, Arc::clone(&ranker));
    if let Some(kind) = layout_health {
        // Allowlisted startup record only: never the layout text or a path.
        logger.error(logging::Scope::Engine, kind, None);
    }
    let base_engine = default_engine(Arc::clone(&ranker));
    if let Some(kind) = base_engine.fallback_kind() {
        // Load failure or a disabled dictionary: allowlisted startup record
        // only (scope + kind, never text). Typing continues on rule output.
        logger.error(logging::Scope::Engine, kind, None);
    }
    let mut session: InputSession<DefaultEngine> = InputSession::new(base_engine);
    let mut fixed_layout = match layout_engine {
        LayoutEngine::Fixed(layout) => Some(layout),
        LayoutEngine::Phonetic(engine) => {
            session = InputSession::new(DefaultEngine::Suggest {
                engine,
                ranker: Arc::clone(&ranker),
            });
            None
        }
    };
    let mut fixed_composition = FixedComposition::default();
    let mut english = EnglishSession::new();
    let mut english_previous: Option<String> = None;
    let mut compose_window = foreground_window_id();
    let mut compose_pid = foreground_window_pid();
    let mut preedit_rendered = String::new();
    let mut shown_words: HashSet<String> = HashSet::new();
    let mut last_commit: Option<(String, u64)> = None;
    let mut pending_correction: Option<String> = None;
    let mut ghost = GhostState::default();
    let mut ghost_prev: Option<String> = None;
    let mut encoding = options.encoding;
    let mut mode = options.mode;
    let mut learning_enabled = true;
    let mut last_words_revision = 0;
    if let Some(state) = ipc_state.as_deref() {
        // Persisted last mode wins over the CLI flags (the flags seed a fresh
        // home only - see `setup_ipc_state`).
        mode = daemon_input_mode(state.mode());
        encoding = daemon_output_encoding(state.encoding());
        learning_enabled = state.learning();
        session.set_user_words(state.user_words());
        last_words_revision = state.words_revision();
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
        caret: Option<&CaretTracker>,
        caret_generation: &mut u64,
    ) -> bool {
        let Some(window) = popup.as_mut() else {
            return false;
        };
        let Some(anchor) = caret_for_popup(caret, caret_generation) else {
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
    #[allow(clippy::too_many_arguments)]
    fn toggle_mode(
        session: &mut InputSession<DefaultEngine>,
        popup: &mut Option<SuggestionPopup>,
        tray: &TrayIcon,
        logger: &logging::Logger,
        mode: &mut crate::InputMode,
        toast_until_ms: &mut u64,
        state: Option<&Arc<ProdState>>,
        caret: Option<&CaretTracker>,
        caret_generation: &mut u64,
    ) {
        *mode = mode.toggle();
        let _ = okkhor_win::signal_state_changed();
        session.reset();
        let live = state.map(|state| &**state);
        hide_popup(popup, live);
        tray.set_mode(tray_mode_of(*mode));
        if state.is_none_or(|state| state.toggle_sound()) {
            okkhor_win::play_toggle_click();
        }
        if let Some(state) = state {
            state.set_mode_live(ipc_input_mode(*mode));
            persist_mode(state, *mode);
        }
        if show_mode_toast(popup, logger, *mode, live, caret, caret_generation) {
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
        println!("daemon: type phonetic English and press Space to commit Bangla");
    }
    println!("daemon: use the tray menu or pause hotkey to pause");

    if options.ui {
        open_settings_ui(&tray, logger);
    }

    'run: loop {
        if ipc_state
            .as_ref()
            .is_some_and(|state| state.shutdown_requested())
        {
            break 'run;
        }
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
                    caret_tracker.as_ref(),
                    &mut last_anchor_generation,
                ),
                TrayEvent::Settings => open_settings_ui(&tray, logger),
                TrayEvent::ShowQuickBar => show_quick_bar(&tray, logger),
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
                caret_tracker.as_ref(),
                &mut last_anchor_generation,
            );
        }

        // Flush a paint that was coalesced while the user was typing fast.
        // This is a no-op unless a visible list is waiting for its slot.
        if let Some(window) = popup.as_mut() {
            if let Err(error) = window.tick() {
                logger.error(logging::Scope::Daemon, "popup_paint", error.code());
            }
        }

        // IPC live sync: the UI flips `capture.set` and settings, and the loop
        // applies them here so the hook thread and the blocking IPC threads
        // never meet. All reads are lock-free atomics except the hotkey chords
        // (try_lock — a contended settings write retries next poll).
        if let Some(state) = ipc_state.as_deref() {
            session.set_max_candidates(state.popup_max_items());
            let words_revision = state.words_revision();
            if words_revision != last_words_revision {
                session.set_user_words(state.user_words());
                last_words_revision = words_revision;
            }
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
                let _ = okkhor_win::signal_state_changed();
                session.reset();
                preedit_rendered.clear();
                hide_popup(&mut popup, Some(state));
                tray.set_mode(tray_mode_of(mode));
                println!("daemon: mode {}", mode.as_str());
            }
            let live_encoding = daemon_output_encoding(state.encoding());
            if live_encoding != encoding {
                encoding = live_encoding;
                fixed_composition.clear();
                session.reset();
                preedit_rendered.clear();
                println!("daemon: encoding {}", encoding.as_str());
            }
            // Layout changes apply live: no daemon restart required. Rebuild the
            // engine once on the IPC thread, never on the typing path.
            let live_layout = state.status().layout;
            if live_layout != layout_code {
                layout_code = live_layout;
                let (next, health) = LayoutEngine::from_settings(&layout_code, Arc::clone(&ranker));
                if let Some(kind) = health {
                    logger.error(logging::Scope::Engine, kind, None);
                }
                fixed_composition.clear();
                session.reset();
                preedit_rendered.clear();
                hide_popup(&mut popup, Some(state));
                match next {
                    LayoutEngine::Fixed(layout) => {
                        fixed_layout = Some(layout);
                        session = InputSession::new(default_engine(Arc::clone(&ranker)));
                    }
                    LayoutEngine::Phonetic(engine) => {
                        fixed_layout = None;
                        session = InputSession::new(DefaultEngine::Suggest {
                            engine,
                            ranker: Arc::clone(&ranker),
                        });
                    }
                }
                println!("daemon: layout {}", layout_code);
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
            preedit_rendered.clear();
            // Nothing may be swallowed while capture is off.
            hook.set_english_popup(false);
            okkhor_win::set_swallow_inline(false);
            hook.set_composition_state(false, false, false);
            // Drain injected and real events alike while paused.
            while hook.try_next().is_some() {}
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }

        if !mode.allows_composition() {
            // English mode: no Bangla composition. Track the Latin token and
            // offer prefix completions; Space/Enter/punctuation pass through
            // untouched. Only Tab (while visible) or a popup click accepts.
            if session.is_composing() {
                session.reset();
            }
            preedit_rendered.clear();
            okkhor_win::set_swallow_inline(false);
            let enabled = ipc_state
                .as_deref()
                .is_none_or(|state| state.english_suggestions());
            let limit = ipc_state
                .as_deref()
                .map_or(8usize, |state| state.popup_max_items());
            if !enabled {
                english.reset();
            }

            let mut clicked: Vec<usize> = Vec::new();
            if let Some(window) = popup.as_mut() {
                while let Some(index) = window.try_next_click() {
                    clicked.push(index);
                    if clicked.len() >= 8 {
                        break;
                    }
                }
            }
            for index in clicked {
                if english.is_visible() {
                    if let Some(commit) = english.accept_index(index) {
                        let text = commit.text.clone();
                        if !apply_english_commit(commit, logger, &injection_lock)
                            && okkhor_win::english_popup_visible()
                        {
                            let _ = send_tab();
                        }
                        emit_english_commit(
                            &learn_tx,
                            &learn_dropped,
                            learning_enabled,
                            &text,
                            english_previous.take(),
                        );
                        recent.push(&text);
                        ghost.words = nextword_model::candidates(
                            models
                                .get()
                                .and_then(|loaded| loaded.get(Language::English)),
                            &smriti,
                            Language::English,
                            &recent,
                            GHOST_CANDIDATES,
                        );
                        ghost.selected = 0;
                        english_previous = Some(text);
                    }
                }
            }

            while let Some(event) = hook.try_next() {
                if event.phase != KeyPhase::Down {
                    continue;
                }
                if !enabled {
                    continue;
                }
                if ghost.visible {
                    match event.virtual_key {
                        VK_TAB => {
                            if let Some(word) = ghost.words.get(ghost.selected).cloned() {
                                let _ = okkhor_win::inject_commit(0, &format!("{word} "), false);
                            }
                            ghost.hide();
                            hide_popup(&mut popup, ipc_state.as_deref());
                            continue;
                        }
                        VK_UP | VK_DOWN => {
                            let len = ghost.words.len() as i32;
                            if len > 0 {
                                let delta = if event.virtual_key == VK_UP { -1 } else { 1 };
                                ghost.selected =
                                    (ghost.selected as i32 + delta).rem_euclid(len) as usize;
                                let anchor = caret_tracker
                                    .as_ref()
                                    .and_then(CaretTracker::latest)
                                    .map(|sample| sample.anchor)
                                    .or_else(|| popup.as_ref().and_then(SuggestionPopup::anchor));
                                if let (Some(anchor), Some(window)) = (anchor, popup.as_mut()) {
                                    let _ =
                                        window.show_ghost(&ghost.words[ghost.selected], &anchor);
                                }
                            }
                            continue;
                        }
                        VK_ESCAPE => {
                            ghost.hide();
                            hide_popup(&mut popup, ipc_state.as_deref());
                            continue;
                        }
                        _ => {
                            ghost.hide();
                            hide_popup(&mut popup, ipc_state.as_deref());
                        }
                    }
                }
                match event.virtual_key {
                    VK_TAB if english.is_visible() => {
                        // Never erase a token that was typed in another field.
                        if english.window() != foreground_window_id() {
                            english.reset();
                            if okkhor_win::english_popup_visible() {
                                let _ = send_tab();
                            }
                        } else if let Some(commit) = english.accept() {
                            let text = commit.text.clone();
                            if !apply_english_commit(commit, logger, &injection_lock)
                                && okkhor_win::english_popup_visible()
                            {
                                let _ = send_tab();
                            }
                            emit_english_commit(
                                &learn_tx,
                                &learn_dropped,
                                learning_enabled,
                                &text,
                                english_previous.take(),
                            );
                            recent.push(&text);
                            ghost.words = nextword_model::candidates(
                                models
                                    .get()
                                    .and_then(|loaded| loaded.get(Language::English)),
                                &smriti,
                                Language::English,
                                &recent,
                                GHOST_CANDIDATES,
                            );
                            ghost.selected = 0;
                            english_previous = Some(text);
                        } else if okkhor_win::english_popup_visible() {
                            let _ = send_tab();
                        }
                    }
                    VK_UP | VK_DOWN if english.is_visible() => {
                        // Preview the highlighted row in the document right
                        // away; Tab still accepts, but is no longer required.
                        let delta = if event.virtual_key == VK_UP { -1 } else { 1 };
                        if let Some(commit) = english.preview_selection(delta) {
                            let _ = apply_english_commit(commit, logger, &injection_lock);
                        }
                    }
                    VK_ESCAPE => english.dismiss(),
                    VK_BACK => english.backspace(limit, |prefix, limit| {
                        session.english_suggestions(prefix, limit)
                    }),
                    other => {
                        if let Some(letter) = vk_to_letter(other) {
                            let foreground = foreground_window_id();
                            if !english.token().is_empty() && english.window() != foreground {
                                english.reset();
                            }
                            english.push_letter(letter, limit, foreground, |prefix, limit| {
                                session.english_suggestions(prefix, limit)
                            });
                        } else {
                            english.reset();
                        }
                    }
                }
            }

            // Requests are event-driven: one per changed English token while a
            // popup is visible, when a popup first becomes visible, or when the
            // foreground window changes (same token, different field), never one
            // per 5 ms poll.
            let english_visible = english.is_visible();
            let token_changed = last_english_token != english.token();
            if token_changed {
                last_english_token.clear();
                last_english_token.push_str(english.token());
            }
            let became_visible = english_visible && !last_english_visible;
            last_english_visible = english_visible;
            let foreground = foreground_window_id();
            let foreground_changed = foreground != last_english_foreground;
            last_english_foreground = foreground;
            if english_visible && (became_visible || foreground_changed) {
                if let Some(tracker) = caret_tracker.as_ref() {
                    last_caret_request = tracker.request();
                }
            }
            let sample = caret_tracker.as_ref().and_then(CaretTracker::latest);
            // The password flag is only trusted when the sample answers the
            // latest request: an older sample may belong to the previous field,
            // and `refresh_english_popup` keeps the popup hidden until a fresh
            // one lands.
            let password = last_caret_request != 0
                && sample.is_some_and(|sample| {
                    sample.generation >= last_caret_request && sample.password
                });
            if english_visible && password {
                english.dismiss();
            }
            refresh_english_popup(
                &mut popup,
                &english,
                ipc_state.as_deref(),
                caret_tracker.as_ref(),
                &mut last_anchor_generation,
                last_caret_request,
                foreground_changed,
            );
            if english.is_visible() {
                apply_caret_anchor(
                    &mut popup,
                    caret_tracker.as_ref(),
                    logger,
                    &mut last_anchor_generation,
                );
            }
            if english.is_visible() {
                if ghost.visible || !ghost.words.is_empty() {
                    ghost.hide();
                    hide_popup(&mut popup, ipc_state.as_deref());
                }
            } else if !ghost.words.is_empty() && !ghost.visible {
                let anchor = caret_tracker
                    .as_ref()
                    .and_then(CaretTracker::latest)
                    .map(|sample| sample.anchor)
                    .or_else(|| popup.as_ref().and_then(SuggestionPopup::anchor));
                if let (Some(anchor), Some(window)) = (anchor, popup.as_mut()) {
                    if window
                        .show_ghost(&ghost.words[ghost.selected], &anchor)
                        .is_ok()
                    {
                        ghost.visible = true;
                    }
                }
            }
            okkhor_win::set_ghost_popup(ghost.visible);
            hook.set_english_popup(english.is_visible());
            okkhor_win::set_swallow_letters(false);
            fixed_composition.clear();
            hook.set_composition_state(false, false, false);
            if !english.is_visible() && toast_until_ms == 0 {
                hide_popup(&mut popup, ipc_state.as_deref());
            }
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }

        // Publish the state the hook uses to swallow commit keys. Done before
        // the drain so a key arriving during this poll sees the last buffer,
        // and again after it so the next poll sees the new one.
        hook.set_english_popup(false);
        let fixed_enabled =
            fixed_layout.is_some() && mode.allows_composition() && hook.capture_enabled();
        okkhor_win::set_swallow_letters(fixed_enabled);
        let inline_enabled = !fixed_enabled
            && mode.allows_composition()
            && hook.capture_enabled()
            && encoding == crate::OutputEncoding::Unicode
            && ipc_state
                .as_deref()
                .is_none_or(|state| state.inline_preedit());
        okkhor_win::set_swallow_inline(inline_enabled);
        if inline_enabled != inline_was_enabled {
            // Switching the setting mid-word cannot reconcile text that was
            // already delivered under the other mode; end the composition and
            // leave the document as-is.
            session.reset();
            preedit_rendered.clear();
            hide_popup(&mut popup, ipc_state.as_deref());
            inline_was_enabled = inline_enabled;
        }
        if !inline_enabled {
            preedit_rendered.clear();
        }
        if !session.is_composing() {
            shown_words.clear();
        }
        // Next-word ghost: only after a commit, only while idle, and only for
        // Bangla until the English learning events are wired.
        if session.is_composing() || fixed_enabled || !mode.allows_composition() {
            if ghost.visible || !ghost.words.is_empty() {
                ghost.hide();
                if let Some(window) = popup.as_mut() {
                    window.hide();
                }
            }
            ghost_prev = None;
        } else {
            let previous = session.previous_word().map(str::to_owned);
            if previous != ghost_prev {
                if let Some(word) = previous.as_deref() {
                    recent.push(word);
                }
                ghost_prev = previous.clone();
                ghost.words = nextword_model::candidates(
                    models.get().and_then(|loaded| loaded.get(Language::Bangla)),
                    &smriti,
                    Language::Bangla,
                    &recent,
                    GHOST_CANDIDATES,
                );
                ghost.selected = 0;
            }
            if !ghost.words.is_empty() && !ghost.visible {
                let anchor = caret_tracker
                    .as_ref()
                    .and_then(CaretTracker::latest)
                    .map(|sample| sample.anchor)
                    .or_else(|| popup.as_ref().and_then(SuggestionPopup::anchor));
                if let (Some(anchor), Some(window)) = (anchor, popup.as_mut()) {
                    if window
                        .show_ghost(&ghost.words[ghost.selected], &anchor)
                        .is_ok()
                    {
                        ghost.visible = true;
                    }
                }
            }
        }
        okkhor_win::set_ghost_popup(ghost.visible);
        if fixed_enabled
            && !same_target(
                compose_pid,
                compose_window,
                foreground_window_pid(),
                foreground_window_id(),
            )
        {
            fixed_composition.clear();
        }
        if !hook.capture_enabled() || !mode.allows_composition() {
            fixed_composition.clear();
        }
        if let Some((text, raw)) = fixed_composition.flush(Instant::now()) {
            inject_fixed_text(&text, &raw, encoding, logger);
        }
        let bangla_unicode =
            mode.allows_composition() && encoding == crate::OutputEncoding::Unicode;
        hook.set_composition_state(
            mode.allows_composition(),
            session.is_composing(),
            bangla_unicode,
        );

        // Mouse selection: a popup click (queued without activation) commits
        // that row through the same select_digit path as the digit keys. The
        // queue is drained before the key drain so a click and a keystroke in
        // the same poll commit in arrival order. Out-of-range rows are ignored
        // by `select_digit`; clicks with no composition do nothing.
        let mut clicked: Vec<usize> = Vec::new();
        if let Some(window) = popup.as_mut() {
            while let Some(index) = window.try_next_click() {
                clicked.push(index);
                if clicked.len() >= 8 {
                    break;
                }
            }
        }
        for index in clicked {
            if session.is_composing() {
                let roman = session.buffer().to_owned();
                let previous_word = session.previous_word().map(str::to_owned);
                let mut ctx = CompositionCtx {
                    session: &mut session,
                    popup: &mut popup,
                    logger,
                    learn_tx: &learn_tx,
                    learn_dropped: &learn_dropped,
                    compose_window: &mut compose_window,
                    compose_pid: &mut compose_pid,
                    preedit_rendered: &mut preedit_rendered,
                    shown_words: &mut shown_words,
                    last_commit: &mut last_commit,
                    pending_correction: &mut pending_correction,
                    encoding,
                    learning_enabled,
                    state: ipc_state.as_deref(),
                    injection: &injection_lock,
                    caret: caret_tracker.as_ref(),
                    caret_generation: &mut last_anchor_generation,
                };
                if let Some(commit) = ctx.session.select_digit(index) {
                    do_commit(&mut ctx, commit, roman, false, previous_word);
                }
            }
        }

        // Injected events (our own SendInput backspaces/text) never reach this
        // loop: the hook skips LLKHF_INJECTED before enqueueing. Only key-down
        // transitions drive the session; releases pass through untouched.
        while let Some(event) = hook.try_next() {
            if event.phase != KeyPhase::Down {
                continue;
            }
            session.set_now_ms(now_ms());
            let mut ctx = CompositionCtx {
                session: &mut session,
                popup: &mut popup,
                logger,
                learn_tx: &learn_tx,
                learn_dropped: &learn_dropped,
                compose_window: &mut compose_window,
                compose_pid: &mut compose_pid,
                preedit_rendered: &mut preedit_rendered,
                shown_words: &mut shown_words,
                last_commit: &mut last_commit,
                pending_correction: &mut pending_correction,
                encoding,
                learning_enabled,
                state: ipc_state.as_deref(),
                injection: &injection_lock,
                caret: caret_tracker.as_ref(),
                caret_generation: &mut last_anchor_generation,
            };
            handle_key(
                event.virtual_key,
                &mut ctx,
                fixed_layout.as_deref(),
                &mut fixed_composition,
                fixed_layout.is_some() && mode.allows_composition() && hook.capture_enabled(),
                inline_enabled,
                &mut ghost,
            );
        }

        if session.is_composing() {
            // The caret resolver can finish after the key that requested it.
            // Re-show/move from that late sample instead of waiting for the
            // next keystroke; this is what keeps a popup from staying hidden
            // for the rest of a word after a transient probe failure.
            if let Some(sample) = caret_tracker.as_ref().and_then(CaretTracker::latest) {
                if sample.generation > last_anchor_generation {
                    last_anchor_generation = sample.generation;
                    if let Some(window) = popup.as_mut() {
                        match window.show_candidates(
                            session.candidates(),
                            session.selected(),
                            &sample.anchor,
                        ) {
                            Ok(_) => {
                                if let Some(state) = ipc_state.as_deref() {
                                    state.set_popup_visible(window.is_visible());
                                }
                            }
                            Err(error) => {
                                logger.error(logging::Scope::Daemon, "popup_show", error.code());
                            }
                        }
                    }
                } else {
                    apply_caret_anchor(
                        &mut popup,
                        caret_tracker.as_ref(),
                        logger,
                        &mut last_anchor_generation,
                    );
                }
            } else {
                apply_caret_anchor(
                    &mut popup,
                    caret_tracker.as_ref(),
                    logger,
                    &mut last_anchor_generation,
                );
            }
        }

        hook.set_composition_state(
            mode.allows_composition(),
            session.is_composing(),
            bangla_unicode,
        );
        if let Some(state) = ipc_state.as_deref() {
            state.ai().reap();
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    if let Some(state) = ipc_state.as_deref() {
        state.ai().shutdown();
    }
    hook.stop();
    hotkey.stop();
    mode_hotkey.stop();
    if let Some(mut tracker) = caret_tracker {
        tracker.stop();
    }
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
    let _ = okkhor_win::signal_state_changed();
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
fn show_quick_bar(tray: &okkhor_win::TrayIcon, logger: &logging::Logger) {
    if okkhor_win::signal_quickbar_show() {
        return;
    }
    match crate::ipc::launch_quickbar_ui() {
        Ok(()) => logger.info(logging::Scope::Ipc, "quickbar_launch"),
        Err(crate::ipc::UiLaunchError::NotFound) => {
            logger.error(logging::Scope::Ipc, "ui_missing", None);
            tray.show_balloon("Okkhor", "Quick bar UI was not found next to the daemon");
        }
        Err(crate::ipc::UiLaunchError::SpawnFailed) => {
            logger.error(logging::Scope::Ipc, "ui_launch_failed", None);
            tray.show_balloon("Okkhor", "Quick bar failed to start");
        }
    }
}

#[cfg(windows)]
fn open_settings_ui(tray: &okkhor_win::TrayIcon, logger: &logging::Logger) {
    // Tray Settings must reuse the running UI. Spawning Okkhor.exe again
    // opened a second window every time.
    if okkhor_win::signal_settings_show() {
        return;
    }
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
fn shutdown_daemon() -> Result<(), Box<dyn Error>> {
    use okkhor_ipc::protocol::PIPE_NAME;

    let token_path = crate::paths::token_path()?;
    let token_hex = std::fs::read_to_string(token_path)?.trim().to_owned();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let (mut client, _) =
            okkhor_ipc::transport::Client::connect_to_with_token(PIPE_NAME, &token_hex).await?;
        client.shutdown().await?;
        Ok::<(), okkhor_ipc::ClientError>(())
    })?;
    println!("shutdown: requested");
    Ok(())
}

#[cfg(not(windows))]
fn shutdown_daemon() -> Result<(), Box<dyn Error>> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "shutdown is only available on Windows",
    )
    .into())
}

#[cfg(windows)]
fn popup_demo() -> Result<(), Box<dyn Error>> {
    use okkhor_win::{SuggestionPopup, caret_anchor};
    use std::time::{Duration, Instant};

    let Some(anchor) = caret_anchor() else {
        println!("popup-demo: no caret anchor available");
        return Ok(());
    };
    let items: Vec<String> = [
        "আমার",
        "বাংলাদেশ",
        "তুমি",
        "কেমন",
        "ধন্যবাদ",
        "ভালোবাসা",
        "বাংলা",
        "শিক্ষা",
        "স্বাধীনতা",
        "কম্পিউটার",
    ]
    .iter()
    .map(|word| word.to_string())
    .collect();
    let mut popup = SuggestionPopup::new()?;
    let metrics = popup.show_candidates(&items, 1, &anchor)?;
    let placement = metrics.placement;
    println!(
        "popup-demo: {} rows at ({}, {}) size {}x{} for 5 seconds",
        metrics.rows, placement.x, placement.y, metrics.width, metrics.height
    );
    // Pump the queue while visible so the card paints and stays responsive;
    // a click reports its row without activating. No text is logged.
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(5) {
        if let Some(index) = popup.try_next_click() {
            println!("popup-demo: clicked row {index}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
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
                TrayEvent::ShowQuickBar => {
                    println!("tray-demo: show quick bar (daemon would signal the UI)");
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
        DaemonOptions, FontsAction, InputMode, OutputEncoding, RankerChoice, apply_encoding,
        injectable_text, parse_daemon_args, parse_fonts_args, resolve_uninstall_target,
        same_target, select_fonts_dir,
    };

    #[cfg(windows)]
    #[test]
    fn english_card_only_shows_with_a_fresh_non_password_sample() {
        use super::EnglishPopupAction;
        // No stale card is ever kept in an unverified field.
        assert_eq!(
            super::english_popup_action(Some(false), false, false),
            EnglishPopupAction::Hide
        );
        assert_eq!(
            super::english_popup_action(Some(false), true, false),
            EnglishPopupAction::Show
        );
        assert_eq!(
            super::english_popup_action(None, true, false),
            EnglishPopupAction::Hide
        );
        assert_eq!(
            super::english_popup_action(Some(true), true, false),
            EnglishPopupAction::Hide
        );
        assert_eq!(
            super::english_popup_action(Some(false), true, true),
            EnglishPopupAction::Hide
        );
    }

    #[cfg(windows)]
    #[test]
    fn timeout_flush_text_is_bijoy_encoded() {
        let virama = "্";
        assert_eq!(
            super::fixed_text_for_encoding(virama, super::OutputEncoding::Unicode),
            virama
        );
        let bijoy = super::fixed_text_for_encoding(virama, super::OutputEncoding::Bijoy);
        assert_ne!(bijoy, virama, "Bijoy output must differ from Unicode");
        assert!(!bijoy.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn raw_key_re_emit_uses_the_held_shift_state() {
        assert_eq!(super::raw_key_text(0x41, false), "a");
        assert_eq!(super::raw_key_text(0x41, true), "A");
        assert_eq!(super::raw_key_text(0xBC, false), ",");
        assert_eq!(super::raw_key_text(0x61, false), "1");
        assert_eq!(super::raw_key_text(0x6A, false), "*");
        assert_eq!(super::raw_key_text(0x6B, false), "+");
        assert_eq!(super::raw_key_text(0x6D, false), "-");
        assert_eq!(super::raw_key_text(0x6E, false), ".");
        assert_eq!(super::raw_key_text(0x6F, false), "/");
        assert_eq!(super::raw_key_text(0x20, false), "");
    }

    #[cfg(windows)]
    #[test]
    fn browser_child_window_does_not_look_like_a_focus_change() {
        assert!(same_target(100, 10, 100, 10));
        // Same process, new HWND: the omnibox/autocomplete child window.
        assert!(same_target(100, 10, 100, 11));
        // A genuinely different application must still reset the buffer.
        assert!(!same_target(100, 10, 200, 11));
        // Unknown PIDs fall back to exact HWND matching.
        assert!(same_target(0, 10, 0, 10));
        assert!(!same_target(0, 10, 0, 11));
    }

    #[cfg(windows)]
    #[test]
    fn blank_replacements_are_never_injected() {
        assert!(injectable_text("আমার"));
        assert!(injectable_text(" also"));
        assert!(!injectable_text(""));
        assert!(!injectable_text("   "));
        assert!(!injectable_text("\u{200b}\u{200c}"));
    }

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
                ranker: RankerChoice::Baseline,
                no_ipc: false,
                ui: false,
            })
        );
        assert_eq!(
            parse_daemon_args(&args(&["--no-ipc"])),
            Ok(DaemonOptions {
                encoding: OutputEncoding::Unicode,
                mode: InputMode::Bangla,
                ranker: RankerChoice::Baseline,
                no_ipc: true,
                ui: false,
            })
        );
        assert_eq!(
            parse_daemon_args(&args(&["--ui", "--mode=english", "--no-ipc"])),
            Ok(DaemonOptions {
                encoding: OutputEncoding::Unicode,
                mode: InputMode::English,
                ranker: RankerChoice::Baseline,
                no_ipc: true,
                ui: true,
            })
        );
        assert_eq!(
            parse_daemon_args(&args(&["--encoding", "bijoy", "--mode", "english", "--ui"])),
            Ok(DaemonOptions {
                encoding: OutputEncoding::Bijoy,
                mode: InputMode::English,
                ranker: RankerChoice::Baseline,
                no_ipc: false,
                ui: true,
            })
        );
    }

    #[test]
    fn daemon_args_ranker_defaults_to_baseline_and_accepts_hybrid() {
        assert_eq!(
            parse_daemon_args(&args(&[])).expect("default").ranker,
            RankerChoice::Baseline
        );
        assert_eq!(
            parse_daemon_args(&args(&["--ranker", "hybrid"]))
                .expect("hybrid")
                .ranker,
            RankerChoice::Hybrid
        );
        assert_eq!(
            parse_daemon_args(&args(&["--ranker=hybrid"]))
                .expect("hybrid")
                .ranker,
            RankerChoice::Hybrid
        );
        assert_eq!(
            parse_daemon_args(&args(&["--ranker=baseline"]))
                .expect("baseline")
                .ranker,
            RankerChoice::Baseline
        );
        assert!(parse_daemon_args(&args(&["--ranker"])).is_err());
        assert!(parse_daemon_args(&args(&["--ranker=model"])).is_err());
        assert!(parse_daemon_args(&args(&["--ranker=HYBRID"])).is_err());
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

        let mut session: InputSession<_> = InputSession::new(default_engine(std::sync::Arc::new(
            okkhor_learn::Smriti::new(),
        )));
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

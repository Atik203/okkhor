//! Okkhor daemon.
//!
//! The binary hosts the resident input daemon, the learning-engine self-check,
//! and manual smoke tests for the Win32 boundary. The daemon keeps tray and
//! logging off the keydown path.

use std::error::Error;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use okkhor_core::{Candidate, CandidateSource, Language, Ranker, SuggestionRequest};
use okkhor_learn::{CommitSource, LearningEvent, Smriti, SmritiConfig};
use okkhor_storage::JsonFileRepository;

#[cfg(windows)]
mod logging;

mod input_session;

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
            if let Err(error) = run_daemon() {
                eprintln!("daemon failed: {error}");
                std::process::exit(1);
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
        "Okkhor {}\n\nUSAGE:\n    okkhor [--help | --version | self-check | hook-demo | popup-demo | tray-demo | daemon | inject <text> [--sendinput] [--delay-ms N] | autostart <enable|disable|status>]\n\n\
         `self-check` exercises the learning engine, ranking, and persistence.\n\
         `hook-demo` installs a global hook for 5 seconds and prints key events.\n\
         `popup-demo` shows a non-activating popup near the caret for 3 seconds.\n\
         `tray-demo` shows the tray menu for 10 seconds.\n\
         `daemon` runs the resident daemon with hook, pause hotkey, and tray.\n\
         `inject` is a QA-only command for synthetic text; focus the target during the delay.\n\
         `autostart enable|disable|status` manages the per-user login entry.\n\
         The demos are manual smoke tests; run them only in a disposable session.",
        env!("CARGO_PKG_VERSION")
    );
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
fn run_daemon() -> Result<(), Box<dyn Error>> {
    use std::sync::mpsc;
    use std::time::Duration;

    use okkhor_core::Language;
    use okkhor_learn::{LearningEvent, Smriti};
    use okkhor_transliterate::RuleTransliterator;
    use okkhor_win::{
        InjectError, KeyPhase, KeyboardHook, PauseHotkey, SingleInstance, SingleInstanceError,
        SuggestionPopup, TextInjector, TrayEvent, TrayIcon, WindowsInjector, WindowsKeyboardHook,
        caret_anchor, ctrl_or_alt_pressed, foreground_window_id, place_popup, send_backspaces,
    };

    use crate::input_session::{
        BackspaceOutcome, Commit, InputSession, VK_BACK, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN,
        VK_RIGHT, VK_SPACE, VK_TAB, VK_UP, default_engine, vk_to_digit_index, vk_to_letter,
    };

    /// Everything one key event needs. Kept in one struct so the hot loop
    /// stays a thin dispatch with no globals and no cross-thread sharing.
    struct CompositionCtx<'a> {
        session: &'a mut InputSession<RuleTransliterator>,
        popup: &'a mut Option<SuggestionPopup>,
        injector: &'a WindowsInjector,
        logger: &'a logging::Logger,
        learn_tx: &'a mpsc::Sender<LearningEvent>,
        compose_window: &'a mut isize,
    }

    fn hide_popup(popup: &mut Option<SuggestionPopup>) {
        if let Some(window) = popup {
            window.hide();
        }
    }

    fn refresh_popup(ctx: &mut CompositionCtx<'_>) {
        let Some(window) = ctx.popup.as_mut() else {
            return;
        };
        if !ctx.session.is_composing() || ctx.session.candidates().is_empty() {
            window.hide();
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
                }
            }
            None => window.hide(),
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

    /// Commit path: verify focus, erase the roman buffer, inject Bangla.
    ///
    /// Never logs the buffer or the committed text; failures log an
    /// allowlisted kind plus an optional numeric code only.
    fn do_commit(ctx: &mut CompositionCtx<'_>, commit: Commit, roman: String) {
        if foreground_window_id() != *ctx.compose_window {
            ctx.session.reset();
            hide_popup(ctx.popup);
            ctx.logger
                .error(logging::Scope::Daemon, "focus_changed", None);
            return;
        }
        if let Err(error) = send_backspaces(commit.erase_chars) {
            ctx.logger.error(
                logging::Scope::Daemon,
                "inject_backspace",
                inject_code(&error),
            );
            ctx.session.reset();
            hide_popup(ctx.popup);
            return;
        }
        if let Err(error) = ctx.injector.inject(&commit.text) {
            ctx.logger
                .error(logging::Scope::Daemon, "inject_text", inject_code(&error));
            hide_popup(ctx.popup);
            return;
        }
        if commit.trailing_space {
            if let Err(error) = ctx.injector.inject(" ") {
                ctx.logger
                    .error(logging::Scope::Daemon, "inject_space", inject_code(&error));
            }
        }
        hide_popup(ctx.popup);
        // Queue learning for the background thread. The channel is unbounded
        // and `Smriti::observe` performs no I/O, so nothing here can stall the
        // key path. A disconnected receiver just drops the event.
        let _ = ctx.learn_tx.send(LearningEvent::TransliterationCommitted {
            roman_input: roman,
            output: commit.text,
            language: Language::Bangla,
            previous_word: None,
            timestamp_ms: now_ms(),
        });
    }

    fn cancel_composition(ctx: &mut CompositionCtx<'_>) {
        // `cancel` returns the raw roman buffer, which the caller deliberately
        // drops: already-typed text stays untouched in the target.
        let _ = ctx.session.cancel();
        hide_popup(ctx.popup);
    }

    /// Record or re-validate the composition window around one keystroke.
    ///
    /// When focus moved mid-composition the stale buffer is dropped (the roman
    /// text stays where it was typed) and the window id is refreshed.
    fn track_focus(ctx: &mut CompositionCtx<'_>) {
        let foreground = foreground_window_id();
        if ctx.session.is_composing() && foreground != *ctx.compose_window {
            ctx.session.reset();
            hide_popup(ctx.popup);
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
                if !ctx.session.is_composing() {
                    return;
                }
                match ctx.session.backspace() {
                    BackspaceOutcome::Empty => {}
                    BackspaceOutcome::StillComposing | BackspaceOutcome::BecameEmpty => {
                        track_focus(ctx);
                        refresh_popup(ctx);
                    }
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
                        hide_popup(ctx.popup);
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
    let hotkey_ready = match hotkey.start() {
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

    // Learning runs on a background thread: `Smriti::observe` performs no I/O
    // and the unbounded channel never blocks the key path. If the thread
    // cannot start, typing continues without learning.
    let (learn_tx, learn_rx) = mpsc::channel::<LearningEvent>();
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

    let injector = WindowsInjector;
    let mut session: InputSession<RuleTransliterator> = InputSession::new(default_engine());
    let mut compose_window = foreground_window_id();

    println!("daemon: running");
    if hotkey_ready {
        println!("daemon: Ctrl+Alt+Shift+P toggles capture");
    }
    println!("daemon: type Banglish and press Space to commit Bangla");
    println!("daemon: use the tray menu or pause hotkey to pause");

    'run: loop {
        while let Some(event) = tray.try_next() {
            match event {
                TrayEvent::TogglePause => toggle_capture(&hook, &tray),
                TrayEvent::Exit => break 'run,
            }
        }

        if hotkey_ready && hotkey.try_next() {
            toggle_capture(&hook, &tray);
        }

        if !hook.capture_enabled() {
            // Pause cancels silently: already-typed roman text stays, the
            // popup hides, and no key is swallowed (the hook passes through).
            if session.is_composing() {
                session.reset();
                hide_popup(&mut popup);
            }
            // Drain injected and real events alike while paused.
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
                injector: &injector,
                logger,
                learn_tx: &learn_tx,
                compose_window: &mut compose_window,
            };
            handle_key(event.virtual_key, &mut ctx);
        }

        std::thread::sleep(Duration::from_millis(10));
    }

    hook.stop();
    hotkey.stop();
    tray.remove();
    Ok(())
}

#[cfg(windows)]
fn toggle_capture(hook: &okkhor_win::WindowsKeyboardHook, tray: &okkhor_win::TrayIcon) {
    let enabled = hook.toggle_capture();
    tray.set_paused(!enabled);
    println!(
        "daemon: capture {}",
        if enabled { "enabled" } else { "paused" }
    );
}

#[cfg(not(windows))]
fn run_daemon() -> Result<(), Box<dyn Error>> {
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

    println!("tray-demo: right-click the icon for the menu; stops after 10 seconds.");
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        while let Some(event) = tray.try_next() {
            match event {
                TrayEvent::TogglePause => {
                    let paused = !tray.paused();
                    tray.set_paused(paused);
                    println!("tray-demo: {}", if paused { "paused" } else { "active" });
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
}

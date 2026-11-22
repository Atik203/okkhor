//! Injection benchmark.
//!
//! The end-to-end path creates a private Win32 `EDIT` window, focuses it, sends
//! synthetic English and Bangla text through `WindowsInjector`, and reads the
//! text back. It installs no global hook and does not touch the user clipboard:
//! it calls `WindowsInjector::inject_unicode_only`, which cannot select the
//! clipboard fallback. If the window is not available (headless session, focus
//! denied), the report falls back to an explicit UTF-16 event-construction
//! benchmark and records the reason.

use std::hint::black_box;
use std::time::Instant;

use crate::output::{InjectionLanguageReport, InjectionReport, REPORT_SCHEMA_VERSION};
use crate::stats::LatencySummary;

pub const TEXTS: &[(&str, &str)] = &[("english", "keyboard"), ("bangla", "কীবোর্ড")];

#[derive(Debug, Clone, Copy)]
pub struct InjectionOptions {
    pub iterations: usize,
}

/// Run the injection benchmark and always return a report.
pub fn run(options: InjectionOptions) -> InjectionReport {
    #[cfg(windows)]
    {
        windows_impl::run(options)
    }

    #[cfg(not(windows))]
    {
        construction_report(
            options,
            "End-to-end injection is deferred: Win32 EDIT-window injection is only available on Windows."
                .to_owned(),
        )
    }
}

fn construction_report(options: InjectionOptions, reason: String) -> InjectionReport {
    let mut languages = Vec::with_capacity(TEXTS.len());
    for (language, text) in TEXTS {
        let units = utf16_units(text).len();
        let mut samples = Vec::with_capacity(options.iterations.max(1));
        let _ = black_box(build_input_plan(text));

        for _ in 0..options.iterations {
            let started = Instant::now();
            let plan = build_input_plan(text);
            let elapsed = started.elapsed();
            black_box(&plan);
            samples.push(elapsed);
        }

        languages.push(InjectionLanguageReport {
            language,
            text_utf16_units: units,
            method: "utf16_event_construction",
            correct: 0,
            failed: 0,
            inject_latency: LatencySummary::from_samples(samples),
            readback_latency: None,
        });
    }

    InjectionReport {
        schema_version: REPORT_SCHEMA_VERSION,
        mode: "injection",
        path: "event_construction_only",
        deferred: true,
        reason: Some(reason),
        iterations: options.iterations,
        all_correct: false,
        languages,
    }
}

/// Build the same down/up event plan that the Unicode injector derives from
/// UTF-16 code units. This is a pure construction benchmark, not a Win32 send.
fn build_input_plan(text: &str) -> Vec<(u16, bool)> {
    let units = utf16_units(text);
    let mut plan = Vec::with_capacity(units.len().saturating_mul(2));
    for unit in units {
        plan.push((unit, false));
        plan.push((unit, true));
    }
    plan
}

fn utf16_units(text: &str) -> Vec<u16> {
    #[cfg(windows)]
    {
        okkhor_win::utf16_units(text)
    }

    #[cfg(not(windows))]
    {
        text.encode_utf16().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{InjectionOptions, build_input_plan, construction_report};

    #[test]
    fn construction_plan_has_a_down_and_up_event_per_unit() {
        assert_eq!(build_input_plan("ab").len(), 4);
        assert_eq!(build_input_plan("ভালো").len(), 8);
    }

    #[test]
    fn deferred_report_is_explicit_and_desktop_free() {
        let report =
            construction_report(InjectionOptions { iterations: 3 }, "test reason".to_owned());
        assert!(report.deferred);
        assert!(!report.all_correct);
        assert_eq!(report.path, "event_construction_only");
        assert_eq!(report.reason.as_deref(), Some("test reason"));
        assert_eq!(report.languages.len(), 2);
        for language in &report.languages {
            assert_eq!(language.correct, 0);
            assert_eq!(language.failed, 0);
            assert_eq!(language.inject_latency.count, 3);
            assert!(language.readback_latency.is_none());
        }
    }
}

#[cfg(windows)]
mod windows_impl {
    use std::ffi::c_void;
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    use okkhor_win::{InjectionMethod, WindowsInjector};
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, DispatchMessageW, ES_AUTOVSCROLL, ES_MULTILINE,
        GUITHREADINFO, GetForegroundWindow, GetGUIThreadInfo, GetMessageW, GetWindowTextLengthW,
        GetWindowTextW, GetWindowThreadProcessId, MSG, PostMessageW, PostThreadMessageW, SW_SHOW,
        SetForegroundWindow, SetWindowTextW, ShowWindow, TranslateMessage, WINDOW_EX_STYLE,
        WINDOW_STYLE, WM_CLOSE, WM_QUIT, WS_CHILD, WS_EX_CLIENTEDGE, WS_OVERLAPPEDWINDOW,
        WS_VISIBLE, WS_VSCROLL,
    };
    use windows::core::{PCWSTR, w};

    use super::{InjectionOptions, TEXTS};
    use crate::output::{InjectionLanguageReport, InjectionReport, REPORT_SCHEMA_VERSION};
    use crate::stats::LatencySummary;

    const READBACK_TIMEOUT: Duration = Duration::from_millis(1_000);

    pub(super) fn run(options: InjectionOptions) -> InjectionReport {
        let window = match TestWindow::spawn() {
            Ok(window) => window,
            Err(reason) => {
                return super::construction_report(
                    options,
                    format!("End-to-end injection is deferred: {reason}"),
                );
            }
        };

        let injector = WindowsInjector;
        let mut languages = Vec::with_capacity(TEXTS.len());
        let mut reason = None;

        for (language, text) in TEXTS {
            let units = super::utf16_units(text).len();
            let mut inject_samples = Vec::with_capacity(options.iterations.max(1));
            let mut readback_samples = Vec::with_capacity(options.iterations.max(1));
            let mut correct = 0;
            let mut failed = 0;

            if let Err(error) = inject_once(&window, &injector, text) {
                failed = options.iterations;
                reason = Some(format!("{language} warmup failed: {error}"));
            }

            if failed == 0 {
                for _ in 0..options.iterations {
                    match inject_once(&window, &injector, text) {
                        Ok(sample) => {
                            inject_samples.push(sample.inject);
                            readback_samples.push(sample.readback);
                            correct += 1;
                        }
                        Err(error) => {
                            failed += 1;
                            if reason.is_none() {
                                reason = Some(format!("{language}: {error}"));
                            }
                            break;
                        }
                    }
                }
            }

            languages.push(InjectionLanguageReport {
                language,
                text_utf16_units: units,
                method: "unicode_send_input",
                correct,
                failed,
                inject_latency: LatencySummary::from_samples(inject_samples),
                readback_latency: (correct > 0)
                    .then(|| LatencySummary::from_samples(readback_samples)),
            });
        }

        let all_correct = languages
            .iter()
            .all(|language| language.failed == 0 && language.correct == options.iterations);

        InjectionReport {
            schema_version: REPORT_SCHEMA_VERSION,
            mode: "injection",
            path: "focused_edit_window",
            deferred: false,
            reason,
            iterations: options.iterations,
            all_correct,
            languages,
        }
    }

    struct InjectionSample {
        inject: Duration,
        readback: Duration,
    }

    fn inject_once(
        window: &TestWindow,
        injector: &WindowsInjector,
        text: &str,
    ) -> Result<InjectionSample, String> {
        window.clear()?;
        if !window.is_foreground() {
            return Err("test window lost foreground before injection".to_owned());
        }
        if !window.is_edit_focused() {
            return Err("EDIT control lost keyboard focus before injection".to_owned());
        }

        let started = Instant::now();
        let method = injector
            .inject_unicode_only(text)
            .map_err(|error| format!("WindowsInjector failed: {error}"))?;
        let inject = started.elapsed();
        if method != InjectionMethod::UnicodeSendInput {
            return Err(format!(
                "WindowsInjector used {method:?}; the clipboard fallback is not allowed in this benchmark"
            ));
        }

        loop {
            let current = window.text();
            if current == text {
                return Ok(InjectionSample {
                    inject,
                    readback: started.elapsed(),
                });
            }
            if started.elapsed() >= READBACK_TIMEOUT {
                return Err(format!(
                    "readback did not match within {} ms (expected {} UTF-16 units, got {})",
                    READBACK_TIMEOUT.as_millis(),
                    text.encode_utf16().count(),
                    current.encode_utf16().count(),
                ));
            }
            std::hint::spin_loop();
        }
    }

    struct TestWindow {
        frame: isize,
        edit: isize,
        thread: Option<JoinHandle<()>>,
    }

    impl TestWindow {
        fn spawn() -> Result<Self, String> {
            let (ready_tx, ready_rx) = mpsc::channel::<Result<(isize, isize), String>>();
            let thread = thread::Builder::new()
                .name("okkhor-bench-window".to_owned())
                .spawn(move || match create_and_focus() {
                    Ok((frame, edit)) => {
                        if ready_tx
                            .send(Ok((frame.0 as isize, edit.0 as isize)))
                            .is_err()
                        {
                            // SAFETY: `frame` was created on this thread and is
                            // no longer observable by a receiver.
                            let _ = unsafe { DestroyWindow(frame) };
                            return;
                        }
                        message_loop();
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                    }
                })
                .map_err(|error| format!("could not spawn the test-window thread: {error}"))?;

            match ready_rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Ok((frame, edit))) => Ok(Self {
                    frame,
                    edit,
                    thread: Some(thread),
                }),
                Ok(Err(error)) => {
                    let _ = thread.join();
                    Err(error)
                }
                Err(RecvTimeoutError::Timeout) => {
                    Err("timed out waiting for the test-window thread to become ready".to_owned())
                }
                Err(RecvTimeoutError::Disconnected) => {
                    let _ = thread.join();
                    Err("test-window thread exited before reporting readiness".to_owned())
                }
            }
        }

        fn frame(&self) -> HWND {
            HWND(self.frame as *mut c_void)
        }

        fn edit(&self) -> HWND {
            HWND(self.edit as *mut c_void)
        }

        fn is_foreground(&self) -> bool {
            // SAFETY: GetForegroundWindow has no pointer preconditions; the
            // result is only compared with our live frame handle.
            unsafe { GetForegroundWindow() == self.frame() }
        }

        fn is_edit_focused(&self) -> bool {
            // SAFETY: `frame` is a live window created by this process; the
            // thread id and GUITHREADINFO are plain output values.
            let thread_id = unsafe { GetWindowThreadProcessId(self.frame(), None) };
            if thread_id == 0 {
                return false;
            }
            let mut info = GUITHREADINFO {
                cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
                ..Default::default()
            };
            // SAFETY: `info` is a valid, initialized output structure and the
            // thread id belongs to our live test-window thread.
            let result = unsafe { GetGUIThreadInfo(thread_id, &mut info) };
            result.is_ok() && info.hwndFocus == self.edit()
        }

        fn clear(&self) -> Result<(), String> {
            let empty = [0u16];
            // SAFETY: `empty` is a valid NUL-terminated UTF-16 buffer that
            // outlives the call; the edit handle is live.
            unsafe { SetWindowTextW(self.edit(), PCWSTR::from_raw(empty.as_ptr())) }
                .map_err(|error| format!("could not clear the EDIT window: {error}"))?;

            let current = self.text();
            if current.is_empty() {
                Ok(())
            } else {
                Err(format!(
                    "EDIT window did not clear ({} UTF-16 units remain)",
                    current.encode_utf16().count()
                ))
            }
        }

        fn text(&self) -> String {
            // SAFETY: the edit handle is live; length is read from the control.
            let length = unsafe { GetWindowTextLengthW(self.edit()) };
            if length <= 0 {
                return String::new();
            }

            let mut buffer = vec![0u16; length as usize + 1];
            // SAFETY: `buffer` has room for `length + 1` UTF-16 units and the
            // API writes at most that many including the terminator.
            let copied = unsafe { GetWindowTextW(self.edit(), &mut buffer) };
            buffer.truncate(usize::try_from(copied.max(0)).unwrap_or(0));
            String::from_utf16_lossy(&buffer)
        }
    }

    impl Drop for TestWindow {
        fn drop(&mut self) {
            if let Some(thread) = self.thread.take() {
                let frame = self.frame();
                // SAFETY: `frame` is our live test window. The thread id is a
                // plain output value used only to end the owned message loop.
                let thread_id = unsafe { GetWindowThreadProcessId(frame, None) };
                // SAFETY: WM_CLOSE is queued to our own window and WM_QUIT is
                // queued to its owning thread; neither call passes user data.
                unsafe {
                    let _ = PostMessageW(Some(frame), WM_CLOSE, WPARAM(0), LPARAM(0));
                    if thread_id != 0 {
                        let _ = PostThreadMessageW(thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
                    }
                }
                let _ = thread.join();
            }
        }
    }

    fn create_and_focus() -> Result<(HWND, HWND), String> {
        // SAFETY: `STATIC` is a registered system class; the two strings are
        // static NUL-terminated UTF-16 literals; null parent/hinstance are
        // valid for a top-level system-class window.
        let frame = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("Okkhor injection benchmark"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                200,
                200,
                720,
                180,
                None,
                None,
                None,
                None,
            )
        }
        .map_err(|error| format!("CreateWindowExW(STATIC) failed: {error}"))?;

        // SAFETY: the parent handle is the live frame above; `EDIT` is a
        // registered system class; styles and dimensions are plain values.
        let edit = unsafe {
            CreateWindowExW(
                WS_EX_CLIENTEDGE,
                w!("EDIT"),
                w!(""),
                WS_CHILD
                    | WS_VISIBLE
                    | WS_VSCROLL
                    | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL) as u32),
                8,
                8,
                688,
                132,
                Some(frame),
                None,
                None,
                None,
            )
        };
        let edit = match edit {
            Ok(edit) => edit,
            Err(error) => {
                // SAFETY: `frame` is a live window created above and is no
                // longer needed after child creation failed.
                let _ = unsafe { DestroyWindow(frame) };
                return Err(format!("CreateWindowExW(EDIT) failed: {error}"));
            }
        };

        // SAFETY: `frame` is a live top-level window created above.
        let _ = unsafe { ShowWindow(frame, SW_SHOW) };

        let mut last_state = String::new();
        for attempt in 1..=20 {
            // SAFETY: both handles are live windows owned by this thread; the
            // calls only request activation and keyboard focus.
            unsafe {
                let _ = SetForegroundWindow(frame);
                let _ = SetFocus(Some(edit));
            }
            thread::sleep(Duration::from_millis(10));

            // SAFETY: both calls have no pointer preconditions.
            let foreground = unsafe { GetForegroundWindow() };
            // SAFETY: GetFocus returns the focus window for this thread's queue.
            let focus = unsafe { GetFocus() };
            if foreground == frame && focus == edit {
                return Ok((frame, edit));
            }
            last_state = format!(
                "attempt {attempt}: foreground_match={}, focus_match={}",
                foreground == frame,
                focus == edit
            );
        }

        // SAFETY: `frame` is a live window created above; cleanup is required
        // before returning the focus failure.
        let _ = unsafe { DestroyWindow(frame) };
        Err(format!(
            "could not focus the test EDIT window ({last_state}); run this mode in an interactive Windows session"
        ))
    }

    fn message_loop() {
        let mut message = MSG::default();
        loop {
            // SAFETY: `message` is a valid local MSG and the loop owns the
            // windows for this thread.
            let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
            if result.0 <= 0 {
                break;
            }

            // SAFETY: `message` was populated by GetMessageW and is valid for
            // translation and dispatch on this thread.
            unsafe {
                let _ = TranslateMessage(&message);
                let _ = DispatchMessageW(&message);
            }
        }
    }
}

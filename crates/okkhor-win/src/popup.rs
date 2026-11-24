//! Native custom-painted suggestion popup.
//!
//! The production suggestion list lives here: a top-level, non-activating
//! Win32 window that paints up to [`MAX_ITEMS`] candidates near the caret and
//! follows it like Kontho/Avro. The window contract is what matters: no
//! activation, no focus theft, physical-pixel placement, per-monitor DPI.
//!
//! The legacy suggestion-window themes (light card `rgb(220,228,245)` with
//! black text, dark card `rgb(1,22,39)` with light text, Noto Sans Bengali
//! 15/16pt, corner radius 4-5, 3px padding) inspire the palette, but nothing
//! is pixel-copied: the rounded card, the teal selection fill, and the index
//! badge column are new, DPI-scaled, and drawn with GDI.
//!
//! Privacy: this module never logs. Candidate words live in memory only for
//! painting and are dropped on hide.

use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    COLORREF, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, RECT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateCompatibleBitmap,
    CreateCompatibleDC, CreateFontIndirectW, CreatePen, CreateSolidBrush, DEFAULT_CHARSET,
    DEFAULT_PITCH, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER,
    DeleteDC, DeleteObject, DrawTextW, EndPaint, FF_DONTCARE, FW_BOLD, FW_NORMAL, GetDC,
    GetTextExtentPoint32W, HDC, HFONT, InvalidateRect, LOGFONTW, OUT_DEFAULT_PRECIS, PAINTSTRUCT,
    PS_SOLID, ReleaseDC, RoundRect, SRCCOPY, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
    UpdateWindow,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, RegCloseKey, RegOpenKeyExW, RegQueryValueExW,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CS_DROPSHADOW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA,
    GetWindowLongPtrW, GetWindowRect, HWND_TOPMOST, IDC_ARROW, LoadCursorW, MA_NOACTIVATE, MSG,
    PM_REMOVE, PeekMessageW, RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SetWindowLongPtrW, SetWindowPos,
    ShowWindow, TranslateMessage, WM_DPICHANGED, WM_ERASEBKGND, WM_LBUTTONDOWN, WM_MOUSEACTIVATE,
    WM_PAINT, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use crate::caret::{CaretAnchor, PopupPlacement, place_popup};

/// Maximum suggestion rows painted (and clickable) in one popup.
///
/// The engine yields up to ten candidates (eight by default); the layout
/// keeps headroom and never paints more than ten rows.
pub const MAX_ITEMS: usize = 10;

/// Row height at 96 DPI.
const ROW_HEIGHT_96: i32 = 30;
/// Outer padding at 96 DPI (legacy themes use 3px; the card needs more air).
const PADDING_96: i32 = 8;
/// Index badge column width at 96 DPI.
const BADGE_WIDTH_96: i32 = 22;
/// Gap between the badge column and the word text at 96 DPI.
const BADGE_GAP_96: i32 = 4;
/// Minimum card width at 96 DPI.
const MIN_WIDTH_96: i32 = 180;
/// Maximum card width at 96 DPI.
const MAX_WIDTH_96: i32 = 380;
/// Card corner radius at 96 DPI (legacy themes use 4-5).
const CARD_RADIUS_96: i32 = 6;
/// Selected-row corner radius at 96 DPI.
const SELECT_RADIUS_96: i32 = 4;
/// Word font size in points (legacy themes use 15/16).
const FONT_POINT_SIZE: i32 = 15;
/// Click queue bound: bursts collapse to the latest few taps.
const MAX_QUEUED_CLICKS: usize = 8;

/// Minimum time between painted list updates. The daemon still recomputes the
/// current candidates on every key (so Space/digits always commit the current
/// row); only the visible card is coalesced, which keeps fast typing calm.
const PAINT_COALESCE: Duration = Duration::from_millis(30);

const POPUP_CLASS: PCWSTR = w!("OkkhorSuggestPopup.v1");
const PERSONALIZE_SUBKEY: PCWSTR =
    w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
const LIGHT_VALUE_NAME: PCWSTR = w!("AppsUseLightTheme");

#[derive(Debug, thiserror::Error)]
pub enum PopupError {
    #[error("Win32 call {operation} failed with code {code}")]
    Win32 { operation: &'static str, code: i32 },
}

impl PopupError {
    /// Numeric code for redacted logging. The operation name is static and
    /// safe to log as a `kind`; no user text is ever attached.
    pub fn code(&self) -> Option<i32> {
        match self {
            Self::Win32 { code, .. } => Some(*code),
        }
    }
}

/// Light/dark card theme, following the Windows apps preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PopupTheme {
    #[default]
    Light,
    Dark,
}

/// GDI palette for one theme. Stored as packed `0x00BBGGRR` values so the
/// pure [`theme_colors`] lookup stays testable without Win32 types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemeColors {
    /// Card background.
    pub background: u32,
    /// 1px card border.
    pub border: u32,
    /// Selected-row fill (teal accent).
    pub selected_fill: u32,
    /// Word text.
    pub text: u32,
    /// Word text on the selected row.
    pub text_selected: u32,
    /// Index badge (`1`-`9`/`0`).
    pub badge: u32,
    /// Index badge on the selected row.
    pub badge_selected: u32,
}

const fn rgb(red: u32, green: u32, blue: u32) -> u32 {
    red | (green << 8) | (blue << 16)
}

/// Palette for a theme, inspired by the legacy suggestion-window themes
/// (light `rgb(220,228,245)`/black, dark `rgb(1,22,39)`/light). Pure: no
/// registry access, so tests inject the theme instead.
pub const fn theme_colors(theme: PopupTheme) -> ThemeColors {
    match theme {
        PopupTheme::Light => ThemeColors {
            background: rgb(220, 228, 245),
            border: rgb(186, 186, 186),
            selected_fill: rgb(13, 148, 136),
            text: rgb(0, 0, 0),
            text_selected: rgb(255, 255, 255),
            badge: rgb(0, 121, 107),
            badge_selected: rgb(255, 255, 255),
        },
        PopupTheme::Dark => ThemeColors {
            background: rgb(1, 22, 39),
            border: rgb(72, 105, 145),
            selected_fill: rgb(4, 36, 68),
            text: rgb(240, 240, 240),
            text_selected: rgb(255, 255, 255),
            badge: rgb(135, 175, 244),
            badge_selected: rgb(255, 255, 255),
        },
    }
}

/// Map the `AppsUseLightTheme` registry value to a theme. `None` (missing
/// key/value, unreadable data) falls back to light; `0` means dark and any
/// other value means light. Pure: tests inject the value.
pub const fn theme_from_apps_use_light_theme(value: Option<u32>) -> PopupTheme {
    match value {
        Some(0) => PopupTheme::Dark,
        _ => PopupTheme::Light,
    }
}

/// The badge for a zero-based row: `1`-`9`, then `0`. Pure.
pub const fn badge_label(index: usize) -> &'static str {
    match index {
        0 => "1",
        1 => "2",
        2 => "3",
        3 => "4",
        4 => "5",
        5 => "6",
        6 => "7",
        7 => "8",
        8 => "9",
        9 => "0",
        _ => "",
    }
}

/// Scale a 96-DPI pixel value to the current DPI, rounding to nearest.
pub fn scale_for_dpi(px_at_96: i32, dpi: u32) -> i32 {
    let dpi = i64::from(dpi.max(1));
    ((i64::from(px_at_96) * dpi + 48) / 96) as i32
}

/// DPI-scaled card geometry. Pure: `max_text_width_px` is the widest row in
/// physical pixels (measured with the live font by the caller), so layout
/// stays unit-testable without a device context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PopupLayout {
    pub width: i32,
    pub height: i32,
    pub row_height: i32,
    pub padding: i32,
    pub badge_width: i32,
    pub badge_gap: i32,
    pub radius: i32,
}

/// Card geometry for `item_count` rows. The count clamps to [`MAX_ITEMS`];
/// the width clamps to the 180-380px (96-DPI) band. Pure.
pub fn layout_for_items(item_count: usize, dpi: u32, max_text_width_px: i32) -> PopupLayout {
    let rows = item_count.min(MAX_ITEMS);
    let row_height = scale_for_dpi(ROW_HEIGHT_96, dpi);
    let padding = scale_for_dpi(PADDING_96, dpi);
    let badge_width = scale_for_dpi(BADGE_WIDTH_96, dpi);
    let badge_gap = scale_for_dpi(BADGE_GAP_96, dpi);
    let radius = scale_for_dpi(CARD_RADIUS_96, dpi);
    let content = badge_width + badge_gap + max_text_width_px.max(0);
    let width = (padding * 2 + content).clamp(
        scale_for_dpi(MIN_WIDTH_96, dpi),
        scale_for_dpi(MAX_WIDTH_96, dpi),
    );
    let height = padding * 2 + rows as i32 * row_height;
    PopupLayout {
        width,
        height,
        row_height,
        padding,
        badge_width,
        badge_gap,
        radius,
    }
}

/// Which list row (if any) owns a client-area y coordinate. Pure.
pub fn row_at_point(y: i32, padding: i32, row_height: i32, rows: usize) -> Option<usize> {
    if row_height <= 0 || y < padding {
        return None;
    }
    let index = (y - padding) / row_height;
    if index < 0 || index as usize >= rows {
        None
    } else {
        Some(index as usize)
    }
}

/// What a `show_candidates` call produced. Carries placement and geometry
/// only — never candidate text, so it is safe to log field by field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PopupMetrics {
    pub placement: PopupPlacement,
    pub rows: usize,
    pub width: i32,
    pub height: i32,
    pub row_height: i32,
    pub theme: PopupTheme,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PopupContent {
    Hidden,
    Toast(String),
    List { items: Vec<String>, selected: usize },
}

impl PopupContent {
    /// Compare painted rows with a caller's slice without allocating. Used by
    /// the no-op fast path in `show_candidates`: when the rows and highlight
    /// are unchanged there is nothing to repaint.
    fn same_list(&self, items: &[String], selected: usize) -> bool {
        match self {
            Self::List {
                items: painted,
                selected: painted_selected,
            } => {
                *painted_selected == selected
                    && painted.len() == items.len()
                    && painted.iter().zip(items.iter()).all(|(a, b)| a == b)
            }
            _ => false,
        }
    }
}

/// Heap state behind the popup window.
///
/// The window procedure stores a pointer to this in `GWLP_USERDATA`. It must
/// live at a stable address, so [`SuggestionPopup`] owns it behind a `Box`:
/// moving the outer popup never relocates these contents.
struct PopupInner {
    window: HWND,
    content: PopupContent,
    font: HFONT,
    font_bold: HFONT,
    font_dpi: u32,
    theme: PopupTheme,
    layout: PopupLayout,
    client_w: i32,
    client_h: i32,
    rows: usize,
    anchor: Option<CaretAnchor>,
    pending_clicks: VecDeque<usize>,
    /// Whether the Win32 window is actually shown. This is separate from
    /// `content`: a list can be current while the window is hidden (for
    /// example while waiting for a fresh caret sample).
    shown: bool,
    /// Last time a visible update was painted. Used to coalesce fast typing.
    last_paint: Instant,
    /// Content/placement changed but the coalescing window has not elapsed.
    pending_paint: bool,
    pending_placement: Option<PopupPlacement>,
    /// Width does not shrink while the card stays visible. A list that grows
    /// and shrinks by a few pixels on every key is a visible flicker.
    sticky_width: i32,
    last_placement: Option<PopupPlacement>,
    /// Measured row width by text, valid for the current font/DPI. Cleared on
    /// font recreation. Avoids a GetDC/GetTextExtentPoint32W pair per row on
    /// every keystroke when the same word is painted again.
    width_cache: HashMap<String, i32>,
}

impl PopupInner {
    /// Paint the current content. `WM_ERASEBKGND` already suppresses the
    /// background erase; `UpdateWindow` makes the paint happen now instead of
    /// waiting for an unrelated message.
    fn invalidate(&self) {
        // SAFETY: window is live and owned by this popup.
        unsafe {
            let _ = InvalidateRect(Some(self.window), None, false);
            let _ = UpdateWindow(self.window);
        }
    }

    /// Move and optionally show the window, but only when the target rectangle
    /// actually differs. This mirrors the legacy apps' "move only if changed"
    /// rule and avoids the visible jitter of a redundant `SetWindowPos`.
    fn apply_placement(&mut self, placement: PopupPlacement, show: bool) -> Result<(), PopupError> {
        let mut rect = RECT::default();
        // SAFETY: window is live and owned by this popup.
        let have_rect = unsafe { GetWindowRect(self.window, &mut rect) }.is_ok();
        let same_pos = have_rect && rect.left == placement.x && rect.top == placement.y;
        let same_size = have_rect
            && rect.right - rect.left == placement.width
            && rect.bottom - rect.top == placement.height;
        let mut flags = SWP_NOACTIVATE;
        if same_pos {
            flags |= SWP_NOMOVE;
        }
        if same_size {
            flags |= SWP_NOSIZE;
        }
        if show {
            // Match the reference HUDs: insert into the topmost band on the
            // first show. A move-only update leaves the z-order alone.
            flags |= SWP_SHOWWINDOW;
        } else {
            flags |= SWP_NOZORDER;
        }
        if show || !(same_pos && same_size) {
            let insert_after = if show { Some(HWND_TOPMOST) } else { None };
            // SAFETY: window is live and owned by this popup; the placement is
            // in physical pixels and no activation is requested.
            unsafe {
                SetWindowPos(
                    self.window,
                    insert_after,
                    placement.x,
                    placement.y,
                    placement.width,
                    placement.height,
                    flags,
                )
            }
            .map_err(|error| PopupError::Win32 {
                operation: "SetWindowPos",
                code: error.code().0,
            })?;
        }
        if show {
            // SAFETY: window is live; SW_SHOWNOACTIVATE never steals focus.
            unsafe {
                let _ = ShowWindow(self.window, SW_SHOWNOACTIVATE);
            }
        }
        self.last_placement = Some(placement);
        Ok(())
    }
}

/// A top-level, non-activating suggestion window near the caret.
pub struct SuggestionPopup {
    inner: Box<PopupInner>,
}

impl SuggestionPopup {
    pub fn new() -> Result<Self, PopupError> {
        ensure_class_registered()?;
        // SAFETY: the class is registered above; the parent, menu, module,
        // and creation data are all null, and no activation is requested.
        let window = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                POPUP_CLASS,
                w!(""),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                None,
                None,
            )
        }
        .map_err(|error| PopupError::Win32 {
            operation: "CreateWindowExW",
            code: error.code().0,
        })?;

        // SAFETY: GetDpiForWindow has no pointer preconditions; 0 means the
        // lookup failed and 96 keeps every scaled metric sane.
        let dpi = unsafe { GetDpiForWindow(window) };
        let dpi = if dpi == 0 { 96 } else { dpi };
        let face = pick_font_face();
        let layout = layout_for_items(0, dpi, 0);
        let mut popup = Self {
            inner: Box::new(PopupInner {
                window,
                content: PopupContent::Hidden,
                font: create_font(face, dpi, false),
                font_bold: create_font(face, dpi, true),
                font_dpi: dpi,
                theme: current_theme(),
                layout,
                client_w: 1,
                client_h: 1,
                rows: 0,
                anchor: None,
                pending_clicks: VecDeque::new(),
                shown: false,
                last_paint: Instant::now(),
                pending_paint: false,
                pending_placement: None,
                sticky_width: 0,
                last_placement: None,
                width_cache: HashMap::new(),
            }),
        };
        // SAFETY: the Box contents have a stable heap address for the
        // lifetime of this popup, and the window only exists while this
        // popup owns it, so the procedure's pointer stays valid.
        unsafe {
            SetWindowLongPtrW(
                window,
                GWLP_USERDATA,
                (&mut *popup.inner as *mut PopupInner) as isize,
            );
        }
        Ok(popup)
    }

    /// Paint `items` (up to [`MAX_ITEMS`]) with `selected` highlighted,
    /// anchored at the caret. Computes the card size from the live font and
    /// DPI, places it below the caret (flipping above when it would leave the
    /// monitor work area), and shows it without activating. An empty list
    /// hides the window. Returns geometry only — never text.
    ///
    /// A visible card is updated in place: it is never re-shown and it only
    /// moves when the computed placement actually changes. Fast updates are
    /// coalesced to [`PAINT_COALESCE`] and flushed by [`Self::tick`]; the
    /// daemon's composition state (and therefore Space/digit commits) is
    /// always current even while the paint is deferred.
    pub fn show_candidates(
        &mut self,
        items: &[String],
        selected: usize,
        anchor: &CaretAnchor,
    ) -> Result<PopupMetrics, PopupError> {
        let rows = items.len().min(MAX_ITEMS);
        if rows == 0 {
            let theme = self.inner.theme;
            self.hide();
            return Ok(PopupMetrics {
                placement: place_popup(anchor, 1, 1),
                rows: 0,
                width: 0,
                height: 0,
                row_height: 0,
                theme,
            });
        }
        let inner = &mut *self.inner;
        let dpi = if anchor.dpi == 0 { 96 } else { anchor.dpi };
        let theme = current_theme();
        if theme != inner.theme || dpi != inner.font_dpi {
            inner.recreate_fonts(dpi);
            inner.theme = theme;
        }
        let selected = selected.min(rows - 1);
        // No-op fast path: a visible card whose rows, highlight and anchor are
        // unchanged needs no clone, no GDI text measurement, no SetWindowPos
        // and no repaint. The candidate list itself is recomputed by the
        // engine on every key; this only removes redundant *popup* work.
        if inner.shown
            && !inner.pending_paint
            && inner.anchor == Some(*anchor)
            && inner.content.same_list(&items[..rows], selected)
        {
            let placement = place_popup(anchor, inner.layout.width, inner.layout.height);
            return Ok(PopupMetrics {
                placement,
                rows,
                width: inner.layout.width,
                height: inner.layout.height,
                row_height: inner.layout.row_height,
                theme,
            });
        }
        let visible: Vec<String> = items.iter().take(rows).cloned().collect();
        let max_text = inner.max_text_width(&visible);
        let mut layout = layout_for_items(rows, dpi, max_text);
        // Keep the card width stable for the lifetime of one visible list.
        // Shrinking and growing by a few pixels on every key is visible.
        if inner.shown {
            inner.sticky_width = inner.sticky_width.max(layout.width);
            layout.width = layout.width.max(inner.sticky_width);
        } else {
            inner.sticky_width = layout.width;
        }
        let placement = place_popup(anchor, layout.width, layout.height);
        let new_content = PopupContent::List {
            items: visible,
            selected,
        };
        let content_changed = inner.content != new_content;
        inner.content = new_content;
        inner.layout = layout;
        inner.client_w = layout.width;
        inner.client_h = layout.height;
        inner.rows = rows;
        inner.anchor = Some(*anchor);

        let first_show = !inner.shown;
        let now = Instant::now();
        if first_show || now.duration_since(inner.last_paint) >= PAINT_COALESCE {
            inner.pending_paint = false;
            inner.pending_placement = None;
            inner.apply_placement(placement, first_show)?;
            if content_changed {
                inner.invalidate();
            }
            inner.shown = true;
            inner.last_paint = now;
        } else {
            inner.pending_paint = true;
            inner.pending_placement = Some(placement);
        }
        Ok(PopupMetrics {
            placement,
            rows,
            width: layout.width,
            height: layout.height,
            row_height: layout.row_height,
            theme,
        })
    }

    /// Show a one-line toast (mode toggles) at a caller-chosen placement.
    /// The text is a fixed daemon literal, never user content.
    pub fn show(&mut self, text: &str, placement: PopupPlacement) -> Result<(), PopupError> {
        let inner = &mut *self.inner;
        // SAFETY: GetDpiForWindow has no pointer preconditions.
        let dpi = unsafe { GetDpiForWindow(inner.window) };
        let dpi = if dpi == 0 { 96 } else { dpi };
        let theme = current_theme();
        if theme != inner.theme || dpi != inner.font_dpi {
            inner.recreate_fonts(dpi);
            inner.theme = theme;
        }
        inner.content = PopupContent::Toast(text.to_owned());
        inner.client_w = placement.width;
        inner.client_h = placement.height;
        inner.rows = 0;
        inner.anchor = None;
        inner.pending_clicks.clear();
        inner.sticky_width = 0;
        let first_show = !inner.shown;
        inner.apply_placement(placement, first_show)?;
        inner.invalidate();
        inner.shown = true;
        inner.pending_paint = false;
        inner.pending_placement = None;
        inner.last_paint = Instant::now();
        Ok(())
    }

    /// Flush a paint that was coalesced by [`Self::show_candidates`].
    ///
    /// Called from the daemon loop (one poll every few milliseconds); a
    /// no-op when nothing is pending. Returns a typed Win32 error only when
    /// the window actually needs to move or repaint.
    pub fn tick(&mut self) -> Result<(), PopupError> {
        let inner = &mut *self.inner;
        if !inner.shown || !inner.pending_paint {
            return Ok(());
        }
        if inner.last_paint.elapsed() < PAINT_COALESCE {
            return Ok(());
        }
        if let Some(placement) = inner.pending_placement.take() {
            inner.apply_placement(placement, false)?;
        }
        inner.invalidate();
        inner.pending_paint = false;
        inner.last_paint = Instant::now();
        Ok(())
    }

    /// Hide the window and drop queued clicks and painted words.
    pub fn hide(&mut self) {
        let inner = &mut *self.inner;
        let was_shown = inner.shown;
        inner.content = PopupContent::Hidden;
        inner.rows = 0;
        inner.pending_clicks.clear();
        inner.shown = false;
        inner.pending_paint = false;
        inner.pending_placement = None;
        inner.sticky_width = 0;
        inner.last_placement = None;
        if was_shown {
            // SAFETY: window is live until Drop.
            unsafe {
                let _ = ShowWindow(inner.window, SW_HIDE);
            }
        }
    }

    /// Whether the popup window is actually shown.
    pub fn is_visible(&self) -> bool {
        self.inner.shown
    }

    /// Last caret anchor used or stored, including while hidden. The daemon
    /// uses it as a fallback when a transient UI Automation probe fails, so a
    /// single failed probe cannot hide the card for the rest of a word.
    pub fn anchor(&self) -> Option<CaretAnchor> {
        self.inner.anchor
    }

    /// Follow a newer caret without repainting content.
    ///
    /// The anchor is always stored. A visible popup is moved with the same
    /// clamping used by [`show_candidates`], but without a show/repaint: only
    /// `SetWindowPos` runs when the placement actually changed, preserving
    /// focus and the painted rows. A hidden popup only records the anchor for
    /// the next show or DPI change.
    pub fn update_anchor(&mut self, anchor: &CaretAnchor) -> Result<(), PopupError> {
        let inner = &mut *self.inner;
        inner.anchor = Some(*anchor);
        if !inner.shown {
            return Ok(());
        }
        let placement = place_popup(anchor, inner.client_w, inner.client_h);
        if inner.last_placement == Some(placement) {
            return Ok(());
        }
        inner.apply_placement(placement, false)?;
        if inner.pending_paint {
            inner.pending_placement = Some(placement);
        }
        Ok(())
    }

    /// Drain one queued row click, if any. Pumps this window's messages first
    /// so taps land within one daemon poll; never blocks, never activates.
    pub fn try_next_click(&mut self) -> Option<usize> {
        pump_window_messages(self.inner.window);
        self.inner.pending_clicks.pop_front()
    }
}

impl Drop for SuggestionPopup {
    fn drop(&mut self) {
        let inner = &mut *self.inner;
        // SAFETY: both fonts are owned by this popup; deleting a null handle
        // is guarded, and the window outlives them.
        unsafe {
            if !inner.font.is_invalid() {
                let _ = DeleteObject(inner.font.into());
            }
            if !inner.font_bold.is_invalid() {
                let _ = DeleteObject(inner.font_bold.into());
            }
        }
        // SAFETY: window is owned by this popup and destroyed exactly once.
        // The Box is freed after this, so no procedure call can observe the
        // dangling GWLP_USERDATA pointer: the window is gone first.
        unsafe {
            let _ = DestroyWindow(inner.window);
        }
    }
}

impl PopupInner {
    fn recreate_fonts(&mut self, dpi: u32) {
        let face = pick_font_face();
        let regular = create_font(face, dpi, false);
        let bold = create_font(face, dpi, true);
        // SAFETY: both handles are owned by this popup; null handles are
        // guarded so a failed creation never deletes anything.
        unsafe {
            if !self.font.is_invalid() {
                let _ = DeleteObject(self.font.into());
            }
            if !self.font_bold.is_invalid() {
                let _ = DeleteObject(self.font_bold.into());
            }
        }
        self.font = regular;
        self.font_bold = bold;
        self.font_dpi = dpi;
        // Every cached extent was measured with the old font and DPI.
        self.width_cache.clear();
    }

    /// Widest row in physical pixels with the live regular font. Returns 0
    /// when no device context is available, which collapses to the minimum
    /// card width instead of failing the whole show.
    fn max_text_width(&mut self, items: &[String]) -> i32 {
        // SAFETY: GetDC/ReleaseDC are paired on every path; the font is a
        // live HFONT owned by this popup and the previous font is restored.
        unsafe {
            let hdc = GetDC(Some(self.window));
            if hdc.is_invalid() {
                return 0;
            }
            let previous = SelectObject(hdc, self.font.into());
            let mut widest = 0i32;
            for item in items {
                let width = if let Some(cached) = self.width_cache.get(item) {
                    *cached
                } else {
                    let units: Vec<u16> = item.encode_utf16().collect();
                    let mut size = SIZE::default();
                    let measured = if units.is_empty() {
                        0
                    } else if GetTextExtentPoint32W(hdc, &units, &mut size).as_bool() {
                        size.cx.max(0)
                    } else {
                        0
                    };
                    self.width_cache.insert(item.clone(), measured);
                    measured
                };
                widest = widest.max(width);
            }
            let _ = SelectObject(hdc, previous);
            let _ = ReleaseDC(Some(self.window), hdc);
            widest.max(0)
        }
    }

    fn handle_click(&mut self, x: i32, y: i32) {
        let _ = x;
        if !matches!(self.content, PopupContent::List { .. }) {
            return;
        }
        if let Some(index) = row_at_point(y, self.layout.padding, self.layout.row_height, self.rows)
        {
            if self.pending_clicks.len() < MAX_QUEUED_CLICKS {
                self.pending_clicks.push_back(index);
            }
        }
    }

    fn on_dpi_changed(&mut self, new_dpi: u32) {
        if new_dpi == 0 || new_dpi == self.font_dpi {
            return;
        }
        self.recreate_fonts(new_dpi);
        let Some(anchor) = self.anchor else {
            // SAFETY: window is live.
            unsafe {
                let _ = InvalidateRect(Some(self.window), None, true);
            }
            return;
        };
        let items: Vec<String> = match &self.content {
            PopupContent::List { items, .. } => items.clone(),
            _ => {
                // SAFETY: window is live.
                unsafe {
                    let _ = InvalidateRect(Some(self.window), None, true);
                }
                return;
            }
        };
        // The list content (items + selection) is untouched; only the card
        // geometry follows the new DPI.
        let max_text = self.max_text_width(&items);
        let layout = layout_for_items(items.len(), new_dpi, max_text);
        let mut moved = anchor;
        moved.dpi = new_dpi;
        let placement = place_popup(&moved, layout.width, layout.height);
        self.layout = layout;
        self.client_w = layout.width;
        self.client_h = layout.height;
        self.anchor = Some(moved);
        // SAFETY: window is live and owned by this popup; no activation.
        unsafe {
            let _ = SetWindowPos(
                self.window,
                None,
                placement.x,
                placement.y,
                placement.width,
                placement.height,
                SWP_NOACTIVATE | SWP_NOZORDER | SWP_SHOWWINDOW,
            );
            let _ = InvalidateRect(Some(self.window), None, true);
        }
    }

    fn paint(&self) {
        // SAFETY: called from our own WM_PAINT with a live window; `ps` is
        // valid local storage for the paint pair. Drawing into a memory DC
        // and blitting once removes the row-by-row flicker of direct GDI.
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(self.window, &mut ps);
            if hdc.is_invalid() {
                return;
            }
            let width = self.client_w.max(1);
            let height = self.client_h.max(1);
            let memory = CreateCompatibleDC(Some(hdc));
            if memory.is_invalid() {
                self.paint_dc(hdc);
            } else {
                let bitmap = CreateCompatibleBitmap(hdc, width, height);
                if bitmap.is_invalid() {
                    self.paint_dc(hdc);
                } else {
                    let previous = SelectObject(memory, bitmap.into());
                    self.paint_dc(memory);
                    let _ = BitBlt(hdc, 0, 0, width, height, Some(memory), 0, 0, SRCCOPY);
                    SelectObject(memory, previous);
                    let _ = DeleteObject(bitmap.into());
                }
                let _ = DeleteDC(memory);
            }
            let _ = EndPaint(self.window, &ps);
        }
    }

    fn paint_dc(&self, hdc: HDC) {
        let colors = theme_colors(self.theme);
        paint_card(
            hdc,
            self.client_w,
            self.client_h,
            self.layout.radius,
            colors,
        );
        match &self.content {
            PopupContent::Hidden => {}
            PopupContent::Toast(text) => {
                paint_centered_text(hdc, self, text, colors.text, self.font);
            }
            PopupContent::List { items, selected } => {
                for (index, item) in items.iter().enumerate() {
                    let active = index == *selected;
                    if active {
                        paint_selected_row(hdc, self, index, colors);
                    }
                    paint_row(
                        hdc,
                        self,
                        index,
                        item,
                        active,
                        colors,
                        if active { self.font_bold } else { self.font },
                    );
                }
            }
        }
    }
}

/// Rounded card: 1px border over the theme background.
fn paint_card(hdc: HDC, w: i32, h: i32, radius: i32, colors: ThemeColors) {
    // SAFETY: brush/pen are valid GDI objects until deleted below; the
    // previous objects are restored before deletion.
    unsafe {
        let brush = CreateSolidBrush(COLORREF(colors.background));
        let pen = CreatePen(PS_SOLID, 1, COLORREF(colors.border));
        if brush.is_invalid() || pen.is_invalid() {
            if !brush.is_invalid() {
                let _ = DeleteObject(brush.into());
            }
            if !pen.is_invalid() {
                let _ = DeleteObject(pen.into());
            }
            return;
        }
        let previous_brush = SelectObject(hdc, brush.into());
        let previous_pen = SelectObject(hdc, pen.into());
        let _ = RoundRect(hdc, 0, 0, w.max(1), h.max(1), radius, radius);
        let _ = SelectObject(hdc, previous_brush);
        let _ = SelectObject(hdc, previous_pen);
        let _ = DeleteObject(brush.into());
        let _ = DeleteObject(pen.into());
    }
}

/// Soft selected-row fill behind one row.
fn paint_selected_row(hdc: HDC, inner: &PopupInner, index: usize, colors: ThemeColors) {
    let pad = inner.layout.padding;
    let row_h = inner.layout.row_height;
    let top = pad + index as i32 * row_h;
    let radius = scale_for_dpi(SELECT_RADIUS_96, inner.font_dpi);
    let rect = RECT {
        left: pad + 1,
        top,
        right: (inner.client_w - pad - 1).max(pad + 2),
        bottom: top + row_h,
    };
    // SAFETY: same GDI lifetime contract as `paint_card`.
    unsafe {
        let brush = CreateSolidBrush(COLORREF(colors.selected_fill));
        let pen = CreatePen(PS_SOLID, 1, COLORREF(colors.selected_fill));
        if brush.is_invalid() || pen.is_invalid() {
            if !brush.is_invalid() {
                let _ = DeleteObject(brush.into());
            }
            if !pen.is_invalid() {
                let _ = DeleteObject(pen.into());
            }
            return;
        }
        let previous_brush = SelectObject(hdc, brush.into());
        let previous_pen = SelectObject(hdc, pen.into());
        let _ = RoundRect(
            hdc,
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            radius,
            radius,
        );
        let _ = SelectObject(hdc, previous_brush);
        let _ = SelectObject(hdc, previous_pen);
        let _ = DeleteObject(brush.into());
        let _ = DeleteObject(pen.into());
    }
}

/// One row: index badge (`DT_CENTER`) plus the word with end-ellipsis.
fn paint_row(
    hdc: HDC,
    inner: &PopupInner,
    index: usize,
    item: &str,
    active: bool,
    colors: ThemeColors,
    font: HFONT,
) {
    let pad = inner.layout.padding;
    let row_h = inner.layout.row_height;
    let top = pad + index as i32 * row_h;
    let badge_color = if active {
        colors.badge_selected
    } else {
        colors.badge
    };
    let text_color = if active {
        colors.text_selected
    } else {
        colors.text
    };
    // SAFETY: the font is a live HFONT owned by the popup; TRANSPARENT keeps
    // the card and selection fills visible behind the glyphs.
    unsafe {
        let previous_font = SelectObject(hdc, font.into());
        let _ = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, COLORREF(badge_color));
        let mut badge_rect = RECT {
            left: pad,
            top,
            right: pad + inner.layout.badge_width,
            bottom: top + row_h,
        };
        let mut badge: Vec<u16> = badge_label(index).encode_utf16().collect();
        if !badge.is_empty() {
            let _ = DrawTextW(
                hdc,
                &mut badge,
                &mut badge_rect,
                DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX,
            );
        }
        let _ = SetTextColor(hdc, COLORREF(text_color));
        let mut text_rect = RECT {
            left: pad + inner.layout.badge_width + inner.layout.badge_gap,
            top,
            right: inner.client_w - pad,
            bottom: top + row_h,
        };
        let mut units: Vec<u16> = item.encode_utf16().collect();
        if !units.is_empty() && text_rect.right > text_rect.left {
            let _ = DrawTextW(
                hdc,
                &mut units,
                &mut text_rect,
                DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_NOPREFIX | DT_END_ELLIPSIS,
            );
        }
        let _ = SelectObject(hdc, previous_font);
    }
}

fn paint_centered_text(hdc: HDC, inner: &PopupInner, text: &str, color: u32, font: HFONT) {
    // SAFETY: same font/background contract as `paint_row`.
    unsafe {
        let previous_font = SelectObject(hdc, font.into());
        let _ = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, COLORREF(color));
        let mut rect = RECT {
            left: inner.layout.padding,
            top: 0,
            right: inner.client_w - inner.layout.padding,
            bottom: inner.client_h,
        };
        let mut units: Vec<u16> = text.encode_utf16().collect();
        if !units.is_empty() && rect.right > rect.left && rect.bottom > rect.top {
            let _ = DrawTextW(
                hdc,
                &mut units,
                &mut rect,
                DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX | DT_END_ELLIPSIS,
            );
        }
        let _ = SelectObject(hdc, previous_font);
    }
}

fn ensure_class_registered() -> Result<(), PopupError> {
    static REGISTERED: OnceLock<()> = OnceLock::new();
    if REGISTERED.get().is_some() {
        return Ok(());
    }
    // SAFETY: GetModuleHandleW with a null name has no pointer preconditions.
    let module = unsafe { GetModuleHandleW(None) }.map_err(|error| PopupError::Win32 {
        operation: "GetModuleHandleW",
        code: error.code().0,
    })?;
    // SAFETY: IDC_ARROW is a system cursor resource; a null instance asks the
    // system for the shared cursor.
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.map_err(|error| PopupError::Win32 {
        operation: "LoadCursorW",
        code: error.code().0,
    })?;
    let class = WNDCLASSW {
        style: CS_DROPSHADOW,
        lpfnWndProc: Some(popup_wnd_proc),
        hInstance: HINSTANCE(module.0),
        hCursor: cursor,
        lpszClassName: POPUP_CLASS,
        ..WNDCLASSW::default()
    };
    // SAFETY: the class structure is fully initialized; the procedure and the
    // class-name literal live for the process lifetime.
    let atom = unsafe { RegisterClassW(&class) };
    if atom == 0 {
        let code = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        if code != ERROR_CLASS_ALREADY_EXISTS.0 as i32 {
            return Err(PopupError::Win32 {
                operation: "RegisterClassW",
                code,
            });
        }
    }
    let _ = REGISTERED.set(());
    Ok(())
}

/// Preferred Bengali-capable font, first installed match wins. GDI font
/// mapping still substitutes per-character, so an unexpected miss degrades to
/// a readable fallback rather than tofu.
fn pick_font_face() -> &'static str {
    for face in ["Kalpurush", "Noto Sans Bengali", "Nirmala UI", "Segoe UI"] {
        if crate::fonts::is_font_installed(face) {
            return face;
        }
    }
    "Segoe UI"
}

fn create_font(face: &str, dpi: u32, bold: bool) -> HFONT {
    let mut name = [0u16; 32];
    for (slot, unit) in name.iter_mut().zip(face.encode_utf16().take(31)) {
        *slot = unit;
    }
    let info = LOGFONTW {
        lfHeight: -((i64::from(FONT_POINT_SIZE) * i64::from(dpi.max(1)) + 36) / 72) as i32,
        lfWeight: if bold {
            FW_BOLD.0 as i32
        } else {
            FW_NORMAL.0 as i32
        },
        lfCharSet: DEFAULT_CHARSET,
        lfOutPrecision: OUT_DEFAULT_PRECIS,
        lfClipPrecision: CLIP_DEFAULT_PRECIS,
        lfQuality: CLEARTYPE_QUALITY,
        lfPitchAndFamily: DEFAULT_PITCH.0 | FF_DONTCARE.0,
        lfFaceName: name,
        ..LOGFONTW::default()
    };
    // SAFETY: `info` is a fully initialized LOGFONTW with a NUL-padded face
    // name; a null return (invalid handle) is handled by callers.
    unsafe { CreateFontIndirectW(&info) }
}

fn current_theme() -> PopupTheme {
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    static CACHE: OnceLock<Mutex<(Instant, PopupTheme)>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| {
        Mutex::new((
            Instant::now()
                .checked_sub(Duration::from_secs(60))
                .unwrap_or_else(Instant::now),
            PopupTheme::Light,
        ))
    });
    let mut guard = cache.lock().unwrap_or_else(|error| error.into_inner());
    if guard.0.elapsed() < Duration::from_secs(1) {
        return guard.1;
    }
    let theme = theme_from_apps_use_light_theme(read_apps_use_light_theme());
    *guard = (Instant::now(), theme);
    theme
}

/// Read `HKCU\...\Personalize\AppsUseLightTheme` (`0` = dark). Read-only;
/// `None` on any failure so the caller falls back to light.
fn read_apps_use_light_theme() -> Option<u32> {
    use windows::Win32::System::Registry::KEY_QUERY_VALUE;
    // SAFETY: the root and subkey are valid, `key` points to valid local
    // storage, and the handle is closed on every path. Read-only access.
    unsafe {
        let mut key = windows::Win32::System::Registry::HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PERSONALIZE_SUBKEY,
            None,
            KEY_QUERY_VALUE,
            &mut key,
        )
        .is_err()
        {
            return None;
        }
        let mut value: u32 = 0;
        let mut len = std::mem::size_of::<u32>() as u32;
        let status = RegQueryValueExW(
            key,
            LIGHT_VALUE_NAME,
            None,
            None,
            Some(&mut value as *mut u32 as *mut u8),
            Some(&mut len),
        );
        let _ = RegCloseKey(key);
        if status.is_err() || len as usize != std::mem::size_of::<u32>() {
            return None;
        }
        Some(value)
    }
}

/// Pump this window's queued messages so clicks paint promptly. Only our own
/// messages are dispatched; never blocks.
fn pump_window_messages(window: HWND) {
    // SAFETY: `message` is valid local storage and the filter selects this
    // window only. Dispatched messages run our own procedure above.
    unsafe {
        let mut message = MSG::default();
        while PeekMessageW(&mut message, Some(window), 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

fn instance_for(window: HWND) -> Option<&'static mut PopupInner> {
    // SAFETY: windows of our class carry a `*mut PopupInner` set at creation
    // (stable Box address, valid until Drop destroys the window first). A
    // null pointer means the window is not attached yet; anything else comes
    // from our own `SetWindowLongPtrW`.
    unsafe {
        let pointer = GetWindowLongPtrW(window, GWLP_USERDATA) as *mut PopupInner;
        if pointer.is_null() {
            None
        } else {
            Some(&mut *pointer)
        }
    }
}

unsafe extern "system" fn popup_wnd_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            if let Some(inner) = instance_for(window) {
                inner.paint();
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x = ((lparam.0 as u32) & 0xFFFF) as u16 as i16 as i32;
            let y = (((lparam.0 as u32) >> 16) & 0xFFFF) as u16 as i16 as i32;
            if let Some(inner) = instance_for(window) {
                inner.handle_click(x, y);
            }
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let dpi = ((wparam.0 >> 16) & 0xFFFF) as u32;
            if let Some(inner) = instance_for(window) {
                inner.on_dpi_changed(dpi);
            }
            LRESULT(0)
        }
        // SAFETY: all other messages follow the default window procedure.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{
        MAX_ITEMS, PopupContent, PopupTheme, badge_label, layout_for_items, row_at_point,
        scale_for_dpi, theme_colors, theme_from_apps_use_light_theme,
    };

    #[test]
    fn identical_rows_are_a_no_op() {
        let painted = PopupContent::List {
            items: vec!["আমার".to_owned(), "আমি".to_owned()],
            selected: 0,
        };
        assert!(painted.same_list(&["আমার".to_owned(), "আমি".to_owned()], 0));
        // A highlight change is still a paint: the teal row must move.
        assert!(!painted.same_list(&["আমার".to_owned(), "আমি".to_owned()], 1));
        // A new word is a real change; freezing the old rows would be stale.
        assert!(!painted.same_list(&["আমার".to_owned(), "তুমি".to_owned()], 0));
        assert!(!PopupContent::Hidden.same_list(&[], 0));
    }

    #[test]
    fn rows_clamp_to_ten() {
        assert_eq!(layout_for_items(15, 96, 100).height - 2 * 8, 10 * 30);
        assert_eq!(layout_for_items(10, 96, 100).height - 2 * 8, 10 * 30);
    }

    #[test]
    fn empty_list_has_no_rows() {
        let layout = layout_for_items(0, 96, 0);
        assert_eq!(layout.height, 2 * 8);
        assert_eq!(layout.width, 180);
    }

    #[test]
    fn single_row_shrinks() {
        let one = layout_for_items(1, 96, 50);
        let five = layout_for_items(5, 96, 50);
        assert_eq!(one.height, 2 * 8 + 30);
        assert_eq!(five.height, 2 * 8 + 5 * 30);
        assert!(one.height < five.height);
    }

    #[test]
    fn width_clamps_to_band() {
        assert_eq!(layout_for_items(5, 96, 0).width, 180);
        assert_eq!(layout_for_items(5, 96, 10_000).width, 380);
        let mid = layout_for_items(5, 96, 200);
        assert_eq!(mid.width, 2 * 8 + 22 + 4 + 200);
    }

    #[test]
    fn dpi_scales_row_padding_and_width() {
        let single = layout_for_items(1, 96, 0);
        let double = layout_for_items(1, 192, 0);
        assert_eq!(single.row_height, 30);
        assert_eq!(double.row_height, 60);
        assert_eq!(single.padding, 8);
        assert_eq!(double.padding, 16);
        assert_eq!(single.width, 180);
        assert_eq!(double.width, 360);
        assert_eq!(scale_for_dpi(30, 144), 45);
        assert_eq!(scale_for_dpi(8, 120), 10);
    }

    #[test]
    fn hit_testing_respects_padding_and_bounds() {
        // padding 8, row height 30, 3 rows: spans 8..98.
        assert_eq!(row_at_point(7, 8, 30, 3), None);
        assert_eq!(row_at_point(8, 8, 30, 3), Some(0));
        assert_eq!(row_at_point(37, 8, 30, 3), Some(0));
        assert_eq!(row_at_point(38, 8, 30, 3), Some(1));
        assert_eq!(row_at_point(97, 8, 30, 3), Some(2));
        assert_eq!(row_at_point(98, 8, 30, 3), None);
        assert_eq!(row_at_point(200, 8, 30, 3), None);
        assert_eq!(row_at_point(20, 8, 30, 0), None);
        assert_eq!(row_at_point(20, 8, 0, 3), None);
    }

    #[test]
    fn badge_labels_match_digit_keys() {
        let expected = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"];
        for (index, label) in expected.iter().enumerate() {
            assert_eq!(badge_label(index), *label);
        }
        assert_eq!(badge_label(10), "");
        assert_eq!(MAX_ITEMS, 10);
    }

    #[test]
    fn theme_selection_falls_back_to_light() {
        assert_eq!(theme_from_apps_use_light_theme(None), PopupTheme::Light);
        assert_eq!(theme_from_apps_use_light_theme(Some(0)), PopupTheme::Dark);
        assert_eq!(theme_from_apps_use_light_theme(Some(1)), PopupTheme::Light);
        assert_eq!(theme_from_apps_use_light_theme(Some(7)), PopupTheme::Light);
    }

    #[test]
    fn theme_colors_follow_legacy_values() {
        let light = theme_colors(PopupTheme::Light);
        assert_eq!(light.background, 220 | (228 << 8) | (245 << 16));
        assert_eq!(light.text, 0);
        let dark = theme_colors(PopupTheme::Dark);
        assert_eq!(dark.background, 1 | (22 << 8) | (39 << 16));
        assert_ne!(dark.text, 0);
        // Selected rows stay legible on the teal fill in both themes.
        assert_eq!(light.text_selected, 255 | (255 << 8) | (255 << 16));
        assert_eq!(dark.text_selected, 255 | (255 << 8) | (255 << 16));
    }

    #[test]
    fn hidden_popup_stores_a_new_anchor_without_touching_the_window() {
        use super::{PopupContent, PopupInner, SuggestionPopup};
        use crate::caret::CaretAnchor;
        use std::collections::VecDeque;
        use windows::Win32::Foundation::{HWND, RECT};

        // A headless popup: `update_anchor` on hidden content must not call
        // `SetWindowPos`, so a null HWND is never touched.
        let mut popup = SuggestionPopup {
            inner: Box::new(PopupInner {
                window: HWND::default(),
                content: PopupContent::Hidden,
                font: Default::default(),
                font_bold: Default::default(),
                font_dpi: 96,
                theme: PopupTheme::Light,
                layout: layout_for_items(0, 96, 0),
                client_w: 1,
                client_h: 1,
                rows: 0,
                anchor: None,
                pending_clicks: VecDeque::new(),
                shown: false,
                last_paint: std::time::Instant::now(),
                pending_paint: false,
                pending_placement: None,
                sticky_width: 0,
                last_placement: None,
                width_cache: HashMap::new(),
            }),
        };
        let anchor = CaretAnchor {
            rect: RECT {
                left: 40,
                top: 60,
                right: 41,
                bottom: 80,
            },
            dpi: 96,
            work_area: RECT {
                left: 0,
                top: 0,
                right: 800,
                bottom: 600,
            },
        };
        popup
            .update_anchor(&anchor)
            .expect("a hidden popup only stores the anchor");
        assert_eq!(popup.inner.anchor, Some(anchor));
        assert!(!popup.is_visible());
    }

    /// Manual smoke test: paints five Bengali rows at a fixed screen anchor
    /// for eight seconds. This does not depend on a live caret, so it is the
    /// reliable visual check for the card, selection, and double buffering.
    #[test]
    #[ignore = "manual: opens a real popup window"]
    fn manual_smoke_paints_five_rows_at_a_fixed_anchor() {
        use super::SuggestionPopup;
        use crate::caret::CaretAnchor;
        use std::time::{Duration, Instant};
        use windows::Win32::Foundation::RECT;

        let anchor = CaretAnchor {
            rect: RECT {
                left: 400,
                top: 200,
                right: 401,
                bottom: 220,
            },
            dpi: 96,
            work_area: RECT {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
        };
        let items: Vec<String> = ["আমার", "বাংলা", "তুমি", "কেমন", "ধন্যবাদ"]
            .iter()
            .map(|word| word.to_string())
            .collect();
        let mut popup = SuggestionPopup::new().expect("popup creates");
        let metrics = popup
            .show_candidates(&items, 1, &anchor)
            .expect("popup shows");
        assert_eq!(metrics.rows, 5);
        let mut rect = RECT::default();
        let _ = unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetWindowRect(popup.inner.window, &mut rect)
        };
        println!(
            "popup hwnd={:?} visible={} rect=({}, {})-({}, {})",
            popup.inner.window,
            popup.is_visible(),
            rect.left,
            rect.top,
            rect.right,
            rect.bottom
        );
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(8) {
            popup.tick().expect("popup ticks");
            let _ = popup.try_next_click();
            std::thread::sleep(Duration::from_millis(10));
        }
        popup.hide();
    }

    /// Manual smoke test: paints five Bengali rows near the caret for five
    /// seconds. Ignored in CI; run in a disposable Windows session with
    /// `cargo test -p okkhor-win -- --ignored`.
    #[test]
    #[ignore = "manual: opens a real popup window"]
    fn manual_smoke_paints_five_rows() {
        use crate::caret::caret_anchor;
        use std::time::{Duration, Instant};

        let Some(anchor) = caret_anchor() else {
            return;
        };
        let items: Vec<String> = ["আমার", "বাংলা", "তুমি", "কেমন", "ধন্যবাদ"]
            .iter()
            .map(|word| word.to_string())
            .collect();
        let mut popup = super::SuggestionPopup::new().expect("popup creates");
        let metrics = popup
            .show_candidates(&items, 1, &anchor)
            .expect("popup shows");
        assert_eq!(metrics.rows, 5);
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(5) {
            let _ = popup.try_next_click();
            std::thread::sleep(Duration::from_millis(10));
        }
        popup.hide();
    }
}

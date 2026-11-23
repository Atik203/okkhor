#![forbid(unsafe_code)]
//! Settings schema v1 (contract §2).
//!
//! The daemon is the single writer of `settings.json`. The UI reads and
//! writes through IPC (`settings.get`, `settings.update`). Unknown fields are
//! ignored on read and dropped on write.
//!
//! [`SettingsPatch`] carries only set fields; [`Settings::apply`] merges a
//! patch onto the current settings, clamps ranged fields, validates the
//! merged value, and returns the result. Atomic persistence lives in the
//! daemon store (Task 2), not here.

use serde::{Deserialize, Serialize};

use crate::error::ErrorCode;

/// Schema version accepted by [`Settings`]. Any other value maps to
/// [`ErrorCode::UnsupportedSchema`].
pub const SETTINGS_SCHEMA: u32 = 1;

/// Bounds for `font_scale` (contract §2).
pub const FONT_SCALE_MIN: f32 = 0.8;
/// Bounds for `font_scale` (contract §2).
pub const FONT_SCALE_MAX: f32 = 1.6;
/// Bounds for `popup_max_items` (contract §2).
pub const POPUP_MAX_ITEMS_MIN: u8 = 1;
/// Bounds for `popup_max_items` (contract §2).
pub const POPUP_MAX_ITEMS_MAX: u8 = 10;
/// Bounds for `word_manager_page_size` (contract §2).
pub const PAGE_SIZE_MIN: u16 = 10;
/// Bounds for `word_manager_page_size` (contract §2).
pub const PAGE_SIZE_MAX: u16 = 200;
/// Maximum length of `font_family` in Unicode scalar values (contract §2).
pub const FONT_FAMILY_MAX_CHARS: usize = 64;
/// Bounds for `quickbar_opacity` (contract §2, ADR 0014 §5).
pub const QUICKBAR_OPACITY_MIN: f32 = 0.5;
/// Bounds for `quickbar_opacity` (contract §2, ADR 0014 §5).
pub const QUICKBAR_OPACITY_MAX: f32 = 1.0;
/// Bounds for `quickbar_x`/`quickbar_y` (pixels; `null` = auto position).
pub const QUICKBAR_COORD_MIN: i32 = -100_000;
/// Bounds for `quickbar_x`/`quickbar_y` (pixels; `null` = auto position).
pub const QUICKBAR_COORD_MAX: i32 = 100_000;
/// Length bounds for `toggle_hotkey` in bytes (contract §2).
pub const TOGGLE_HOTKEY_MAX_LEN: usize = 32;

/// Known layout codes (mirrors `okkhor_lang::LayoutId::is_known`).
///
/// This crate does not depend on `okkhor-lang` (dependency budget, Task 1),
/// so the known set is spelled out here. Any other value maps to
/// [`ErrorCode::InvalidLayout`].
pub const KNOWN_LAYOUTS: [&str; 6] = ["avro", "national", "probhat", "bijoy", "banglish", "khipro"];

/// UI language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Bn,
    En,
}

/// Color theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    System,
    Light,
    Dark,
}

/// Accent color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Accent {
    Teal,
    Indigo,
    Amber,
    Rose,
}

/// Input mode: phonetic Bangla composition or English pass-through
/// (contract §2, ADR 0014 §1). Persists across restarts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum InputMode {
    #[default]
    Bangla,
    English,
}

/// Output encoding (contract §2, ADR 0013). Independent of [`InputMode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OutputEncoding {
    #[default]
    Unicode,
    Bijoy,
}

/// Unicode font for Okkhor's own rendering (contract §2, ADR 0014 §4).
///
/// Unknown values fall back to the default (`Kalpurush`) instead of failing
/// the whole document, so a settings file from a newer version still loads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FontUnicode {
    Kalpurush,
    SiyamRupali,
    NotoSansBengali,
}

impl<'de> Deserialize<'de> for FontUnicode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.as_str() {
            "kalpurush" => Self::Kalpurush,
            "siyam-rupali" => Self::SiyamRupali,
            "noto-sans-bengali" => Self::NotoSansBengali,
            _ => Self::Kalpurush,
        })
    }
}

/// Bijoy (ANSI) font (contract §2, ADR 0014 §4). The default is `SutonnyMj`,
/// the widely used official-work font; the daemon falls back to Kalpurush
/// ANSI at runtime when SutonnyMJ is not installed, so this layer performs
/// no font detection.
///
/// Note: the wire value is `"sutonnymj"` (no hyphen), so the variants carry
/// explicit renames instead of `kebab-case`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FontBijoy {
    #[serde(rename = "kalpurush-ansi")]
    KalpurushAnsi,
    #[serde(rename = "sutonnymj")]
    SutonnyMj,
}

/// Full settings document (contract §2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub schema: u32,
    pub language: Language,
    pub layout: String,
    #[serde(default)]
    pub input_mode: InputMode,
    #[serde(default)]
    pub output_encoding: OutputEncoding,
    pub theme: Theme,
    pub accent: Accent,
    pub font_scale: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<String>,
    #[serde(default = "default_font_unicode")]
    pub font_unicode: FontUnicode,
    #[serde(default = "default_font_bijoy")]
    pub font_bijoy: FontBijoy,
    pub popup_enabled: bool,
    pub popup_max_items: u8,
    #[serde(default = "default_toggle_hotkey")]
    pub toggle_hotkey: String,
    #[serde(default = "default_pause_hotkey")]
    pub pause_hotkey: String,
    #[serde(default = "default_true")]
    pub show_toggle_toast: bool,
    /// Play a short click when the input mode changes (hotkey, tray, bar).
    #[serde(default = "default_true")]
    pub toggle_sound: bool,
    #[serde(default = "default_true")]
    pub quickbar_visible: bool,
    #[serde(default)]
    pub quickbar_collapsed: bool,
    #[serde(default)]
    pub quickbar_x: Option<i32>,
    #[serde(default)]
    pub quickbar_y: Option<i32>,
    #[serde(default = "default_quickbar_opacity")]
    pub quickbar_opacity: f32,
    pub autostart: bool,
    pub learning_enabled: bool,
    pub word_manager_page_size: u16,
    pub ai_enabled: bool,
    pub speech_enabled: bool,
}

fn default_font_unicode() -> FontUnicode {
    FontUnicode::Kalpurush
}

fn default_font_bijoy() -> FontBijoy {
    FontBijoy::SutonnyMj
}

fn default_toggle_hotkey() -> String {
    "F9".to_owned()
}

fn default_pause_hotkey() -> String {
    "Ctrl+Alt+Shift+P".to_owned()
}

fn default_true() -> bool {
    true
}

fn default_quickbar_opacity() -> f32 {
    0.9
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema: SETTINGS_SCHEMA,
            language: Language::Bn,
            layout: "avro".to_owned(),
            input_mode: InputMode::Bangla,
            output_encoding: OutputEncoding::Unicode,
            theme: Theme::System,
            accent: Accent::Teal,
            font_scale: 1.0,
            font_family: None,
            font_unicode: FontUnicode::Kalpurush,
            font_bijoy: FontBijoy::SutonnyMj,
            popup_enabled: true,
            popup_max_items: 5,
            toggle_hotkey: "F9".to_owned(),
            pause_hotkey: "Ctrl+Alt+Shift+P".to_owned(),
            show_toggle_toast: true,
            toggle_sound: true,
            quickbar_visible: true,
            quickbar_collapsed: false,
            quickbar_x: None,
            quickbar_y: None,
            quickbar_opacity: 0.9,
            autostart: false,
            learning_enabled: true,
            word_manager_page_size: 50,
            ai_enabled: false,
            speech_enabled: false,
        }
    }
}

/// Validation failures for settings. Each maps to a protocol [`ErrorCode`]
/// with a static message (no user text).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SettingsError {
    #[error("unsupported schema version")]
    UnsupportedSchema,
    #[error("unknown layout")]
    InvalidLayout,
    #[error("invalid hotkey")]
    InvalidHotkey,
    #[error("invalid font scale")]
    InvalidFontScale,
    #[error("invalid font family")]
    InvalidFontFamily,
    #[error("invalid settings value")]
    InvalidValue,
}

impl SettingsError {
    #[must_use]
    pub const fn code(self) -> ErrorCode {
        match self {
            Self::UnsupportedSchema => ErrorCode::UnsupportedSchema,
            Self::InvalidLayout => ErrorCode::InvalidLayout,
            Self::InvalidHotkey => ErrorCode::InvalidHotkey,
            Self::InvalidFontScale | Self::InvalidFontFamily | Self::InvalidValue => {
                ErrorCode::InvalidSettings
            }
        }
    }
}

/// A parsed pause hotkey: one or more modifiers plus a single key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    /// Canonical key token (`"P"`, `"F5"`, `"Space"`, ...).
    pub key: String,
}

/// Parse a hotkey string such as `"Ctrl+Alt+P"`.
///
/// Grammar: one or more modifiers (`Ctrl`/`Control`, `Alt`, `Shift`,
/// `Win`/`Meta`/`Super`/`Cmd`, case-insensitive) joined with `+`, followed by
/// a single key: one ASCII alphanumeric character, `F1`–`F24`, or a named key
/// (`Space`, `Tab`, `Enter`, `Escape`/`Esc`, `Backspace`, `Delete`/`Del`,
/// `Insert`/`Ins`, `Home`, `End`, `PageUp`, `PageDown`, `Up`, `Down`, `Left`,
/// `Right`). Empty parts, duplicate modifiers, a missing key, or a bare key
/// with no modifier are rejected.
pub fn parse_hotkey(text: &str) -> Result<Hotkey, SettingsError> {
    let parts: Vec<&str> = text.split('+').collect();
    if parts.len() < 2 {
        return Err(SettingsError::InvalidHotkey);
    }
    let mut hotkey = Hotkey {
        ctrl: false,
        alt: false,
        shift: false,
        win: false,
        key: String::new(),
    };
    for part in &parts[..parts.len() - 1] {
        let token = part.trim();
        if token.is_empty() {
            return Err(SettingsError::InvalidHotkey);
        }
        let lower = token.to_ascii_lowercase();
        let slot = match lower.as_str() {
            "ctrl" | "control" => &mut hotkey.ctrl,
            "alt" => &mut hotkey.alt,
            "shift" => &mut hotkey.shift,
            "win" | "meta" | "super" | "cmd" | "windows" => &mut hotkey.win,
            _ => return Err(SettingsError::InvalidHotkey),
        };
        if *slot {
            // Duplicate modifier such as "Ctrl+Ctrl+P".
            return Err(SettingsError::InvalidHotkey);
        }
        *slot = true;
    }
    let key = parts[parts.len() - 1].trim();
    hotkey.key = canonical_key(key).ok_or(SettingsError::InvalidHotkey)?;
    Ok(hotkey)
}

/// Canonicalize a bare function key (`F1`–`F24`, any case, no leading
/// zeros), or `None` for anything else. Shared by [`canonical_key`] and
/// [`parse_toggle_hotkey`].
fn canonical_function_key(key: &str) -> Option<String> {
    let upper = key.to_ascii_uppercase();
    if let Some(number) = upper.strip_prefix('F')
        && let Ok(n) = number.parse::<u32>()
        && (1..=24).contains(&n)
        && number.len() == n.to_string().len()
    {
        return Some(upper);
    }
    None
}

/// Canonicalize a key token, or `None` when it is not an allowed key.
fn canonical_key(key: &str) -> Option<String> {
    if key.is_empty() {
        return None;
    }
    // A modifier cannot double as the key.
    if matches!(
        key.to_ascii_lowercase().as_str(),
        "ctrl" | "control" | "alt" | "shift" | "win" | "meta" | "super" | "cmd" | "windows"
    ) {
        return None;
    }
    if key.len() == 1 {
        let byte = key.as_bytes()[0];
        if byte.is_ascii_alphanumeric() {
            return Some(key.to_ascii_uppercase());
        }
        return None;
    }
    if let Some(function) = canonical_function_key(key) {
        return Some(function);
    }
    let canonical = match key.to_ascii_lowercase().as_str() {
        "space" => "Space",
        "tab" => "Tab",
        "enter" => "Enter",
        "escape" | "esc" => "Escape",
        "backspace" => "Backspace",
        "delete" | "del" => "Delete",
        "insert" | "ins" => "Insert",
        "home" => "Home",
        "end" => "End",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        "up" => "Up",
        "down" => "Down",
        "left" => "Left",
        "right" => "Right",
        _ => return None,
    };
    Some(canonical.to_owned())
}

/// Parse a toggle hotkey such as `"F9"` (contract §2, ADR 0014 §2).
///
/// Accepted forms: a single function key (`F1`–`F24`) or a `+` chord with
/// the [`parse_hotkey`] grammar (e.g. `"Ctrl+Space"`, the daemon's fallback
/// when `F9` is already registered). The whole string must be 1–32
/// printable ASCII bytes with no whitespace; anything else (bare letters,
/// empty, too long, unknown keys) is rejected. Full OS registration is the
/// daemon's job — this only validates the shape.
pub fn parse_toggle_hotkey(text: &str) -> Result<Hotkey, SettingsError> {
    if text.is_empty()
        || text.len() > TOGGLE_HOTKEY_MAX_LEN
        || !text.bytes().all(|byte| (0x21..=0x7E).contains(&byte))
    {
        return Err(SettingsError::InvalidHotkey);
    }
    if let Ok(chord) = parse_hotkey(text) {
        return Ok(chord);
    }
    if let Some(key) = canonical_function_key(text) {
        return Ok(Hotkey {
            ctrl: false,
            alt: false,
            shift: false,
            win: false,
            key,
        });
    }
    Err(SettingsError::InvalidHotkey)
}

/// Whether `layout` is one of the known layout codes.
#[must_use]
pub fn is_known_layout(layout: &str) -> bool {
    KNOWN_LAYOUTS.contains(&layout)
}

/// Validate and normalize `font_family`: trim, reject empty-to-`None`,
/// enforce ASCII-printable and the 64-character cap.
fn normalize_font_family(raw: &str) -> Result<Option<String>, SettingsError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().count() > FONT_FAMILY_MAX_CHARS
        || !trimmed.bytes().all(|b| (0x20..=0x7E).contains(&b))
    {
        return Err(SettingsError::InvalidFontFamily);
    }
    Ok(Some(trimmed.to_owned()))
}

impl Settings {
    /// Validate the merged value. Ranged fields are clamped by
    /// [`Settings::clamped`], not here; non-finite font scales and unknown
    /// enumerated values are rejected. (Both `NaN` and infinities fail:
    /// `clamped` leaves `NaN` untouched so `validate` still rejects it, and
    /// clamps infinities so `validate(clamped)` accepts them.)
    pub fn validate(&self) -> Result<(), SettingsError> {
        if self.schema != SETTINGS_SCHEMA {
            return Err(SettingsError::UnsupportedSchema);
        }
        if !is_known_layout(&self.layout) {
            return Err(SettingsError::InvalidLayout);
        }
        if !self.font_scale.is_finite() {
            return Err(SettingsError::InvalidFontScale);
        }
        if !self.quickbar_opacity.is_finite() {
            return Err(SettingsError::InvalidValue);
        }
        if let Some(family) = &self.font_family {
            let normalized = normalize_font_family(family)?;
            if normalized.as_deref() != Some(family.as_str()) {
                // Stored value must already be normalized (trimmed).
                return Err(SettingsError::InvalidFontFamily);
            }
        }
        parse_hotkey(&self.pause_hotkey)?;
        parse_toggle_hotkey(&self.toggle_hotkey)?;
        for coord in [self.quickbar_x, self.quickbar_y] {
            if let Some(value) = coord
                && !(QUICKBAR_COORD_MIN..=QUICKBAR_COORD_MAX).contains(&value)
            {
                return Err(SettingsError::InvalidValue);
            }
        }
        Ok(())
    }

    /// Return a copy with ranged fields clamped to contract §2 bounds.
    /// `NaN` scales are left untouched (`f32::clamp` would panic on them) so
    /// [`Settings::validate`] still rejects them; infinities clamp normally.
    #[must_use]
    pub fn clamped(&self) -> Self {
        let mut out = self.clone();
        if !out.font_scale.is_nan() {
            out.font_scale = out.font_scale.clamp(FONT_SCALE_MIN, FONT_SCALE_MAX);
        }
        if !out.quickbar_opacity.is_nan() {
            out.quickbar_opacity = out
                .quickbar_opacity
                .clamp(QUICKBAR_OPACITY_MIN, QUICKBAR_OPACITY_MAX);
        }
        out.popup_max_items = out
            .popup_max_items
            .clamp(POPUP_MAX_ITEMS_MIN, POPUP_MAX_ITEMS_MAX);
        out.word_manager_page_size = out
            .word_manager_page_size
            .clamp(PAGE_SIZE_MIN, PAGE_SIZE_MAX);
        out
    }

    /// Merge a patch onto `self`, clamp ranged fields, validate the merged
    /// value, and return it. The caller persists the result atomically.
    pub fn apply(&self, patch: &SettingsPatch) -> Result<Settings, SettingsError> {
        let mut merged = self.clone();
        if let Some(language) = patch.language {
            merged.language = language;
        }
        if let Some(layout) = &patch.layout {
            merged.layout = layout.clone();
        }
        if let Some(theme) = patch.theme {
            merged.theme = theme;
        }
        if let Some(accent) = patch.accent {
            merged.accent = accent;
        }
        if let Some(font_scale) = patch.font_scale {
            merged.font_scale = font_scale;
        }
        if let Some(font_family) = &patch.font_family {
            merged.font_family = match font_family {
                Some(raw) => normalize_font_family(raw)?,
                None => None,
            };
        }
        if let Some(popup_enabled) = patch.popup_enabled {
            merged.popup_enabled = popup_enabled;
        }
        if let Some(popup_max_items) = patch.popup_max_items {
            merged.popup_max_items = popup_max_items;
        }
        if let Some(input_mode) = patch.input_mode {
            merged.input_mode = input_mode;
        }
        if let Some(output_encoding) = patch.output_encoding {
            merged.output_encoding = output_encoding;
        }
        if let Some(font_unicode) = patch.font_unicode {
            merged.font_unicode = font_unicode;
        }
        if let Some(font_bijoy) = patch.font_bijoy {
            merged.font_bijoy = font_bijoy;
        }
        if let Some(toggle_hotkey) = &patch.toggle_hotkey {
            merged.toggle_hotkey = toggle_hotkey.clone();
        }
        if let Some(show_toggle_toast) = patch.show_toggle_toast {
            merged.show_toggle_toast = show_toggle_toast;
        }
        if let Some(toggle_sound) = patch.toggle_sound {
            merged.toggle_sound = toggle_sound;
        }
        if let Some(quickbar_visible) = patch.quickbar_visible {
            merged.quickbar_visible = quickbar_visible;
        }
        if let Some(quickbar_collapsed) = patch.quickbar_collapsed {
            merged.quickbar_collapsed = quickbar_collapsed;
        }
        if let Some(quickbar_x) = patch.quickbar_x {
            merged.quickbar_x = quickbar_x;
        }
        if let Some(quickbar_y) = patch.quickbar_y {
            merged.quickbar_y = quickbar_y;
        }
        if let Some(quickbar_opacity) = patch.quickbar_opacity {
            merged.quickbar_opacity = quickbar_opacity;
        }
        if let Some(pause_hotkey) = &patch.pause_hotkey {
            merged.pause_hotkey = pause_hotkey.clone();
        }
        if let Some(autostart) = patch.autostart {
            merged.autostart = autostart;
        }
        if let Some(learning_enabled) = patch.learning_enabled {
            merged.learning_enabled = learning_enabled;
        }
        if let Some(page_size) = patch.word_manager_page_size {
            merged.word_manager_page_size = page_size;
        }
        if let Some(ai_enabled) = patch.ai_enabled {
            merged.ai_enabled = ai_enabled;
        }
        if let Some(speech_enabled) = patch.speech_enabled {
            merged.speech_enabled = speech_enabled;
        }
        let merged = merged.clamped();
        merged.validate()?;
        Ok(merged)
    }
}

/// Partial settings update: only set fields are merged (contract §2).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SettingsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<Language>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_mode: Option<InputMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_encoding: Option<OutputEncoding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<Theme>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<Accent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_scale: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_unicode: Option<FontUnicode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_bijoy: Option<FontBijoy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub popup_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub popup_max_items: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toggle_hotkey: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause_hotkey: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_toggle_toast: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toggle_sound: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quickbar_visible: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quickbar_collapsed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quickbar_x: Option<Option<i32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quickbar_y: Option<Option<i32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quickbar_opacity: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autostart: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learning_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word_manager_page_size: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speech_enabled: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_contract() {
        let mut json = serde_json::to_value(Settings::default()).expect("serializes");
        // f32 0.9 has no exact f64 spelling: pin it separately (approximate
        // in the Value model, exact on the wire) and keep the golden exact
        // for everything else.
        let opacity = json
            .as_object_mut()
            .expect("object")
            .remove("quickbar_opacity")
            .expect("opacity present");
        assert!(
            (opacity.as_f64().expect("opacity number") - 0.9).abs() < f64::from(f32::EPSILON),
            "opacity defaults to 0.9, got {opacity}"
        );
        let expected = serde_json::json!({
            "schema": 1,
            "language": "bn",
            "layout": "avro",
            "input_mode": "bangla",
            "output_encoding": "unicode",
            "theme": "system",
            "accent": "teal",
            "font_scale": 1.0,
            "font_unicode": "kalpurush",
            "font_bijoy": "sutonnymj",
            "popup_enabled": true,
            "popup_max_items": 5,
            "toggle_hotkey": "F9",
            "pause_hotkey": "Ctrl+Alt+Shift+P",
            "show_toggle_toast": true,
            "toggle_sound": true,
            "quickbar_visible": true,
            "quickbar_collapsed": false,
            "quickbar_x": null,
            "quickbar_y": null,
            "autostart": false,
            "learning_enabled": true,
            "word_manager_page_size": 50,
            "ai_enabled": false,
            "speech_enabled": false,
        });
        assert_eq!(json, expected);
        // Wire shape pins the shortest round-trip spelling.
        let raw = serde_json::to_string(&Settings::default()).expect("serializes");
        assert!(raw.contains("\"quickbar_opacity\":0.9"), "wire shape");
        Settings::default().validate().expect("defaults valid");
    }

    #[test]
    fn settings_json_round_trip() {
        let json = serde_json::to_string(&Settings::default()).expect("serializes");
        let back: Settings = serde_json::from_str(&json).expect("parses");
        assert_eq!(back, Settings::default());
    }

    #[test]
    fn strict_enums_reject_unknown() {
        for (field, value) in [
            ("input_mode", "phonetic"),
            ("output_encoding", "ascii"),
            ("font_bijoy", "vrinda"),
        ] {
            let mut doc = serde_json::to_value(Settings::default()).expect("serializes");
            doc[field] = serde_json::Value::String(value.to_owned());
            assert!(
                serde_json::from_value::<Settings>(doc).is_err(),
                "{field} rejects {value:?}"
            );
        }
    }

    #[test]
    fn font_unicode_unknown_falls_back_to_default() {
        // Contract §2: unknown → default, so newer files still load.
        let mut doc = serde_json::to_value(Settings::default()).expect("serializes");
        doc["font_unicode"] = serde_json::Value::String("vrinda".to_owned());
        let settings: Settings = serde_json::from_value(doc).expect("loads with default");
        assert_eq!(settings.font_unicode, FontUnicode::Kalpurush);
        settings.validate().expect("fallback valid");
        for known in ["kalpurush", "siyam-rupali", "noto-sans-bengali"] {
            let mut doc = serde_json::to_value(Settings::default()).expect("serializes");
            doc["font_unicode"] = serde_json::Value::String(known.to_owned());
            let settings: Settings = serde_json::from_value(doc).expect("known loads");
            settings.validate().expect("known valid");
        }
    }

    #[test]
    fn unknown_fields_ignored_on_read() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "schema": 1,
            "language": "bn",
            "layout": "avro",
            "theme": "system",
            "accent": "teal",
            "font_scale": 1.0,
            "popup_enabled": true,
            "popup_max_items": 5,
            "pause_hotkey": "Ctrl+Alt+P",
            "autostart": false,
            "learning_enabled": false,
            "word_manager_page_size": 50,
            "ai_enabled": false,
            "speech_enabled": false,
            "future_field": "dropped",
        }))
        .expect("unknown fields ignored");
        assert!(!settings.learning_enabled);
    }

    #[test]
    fn rejects_non_one_schema() {
        for schema in [0, 2, 99, u32::MAX] {
            let settings = Settings {
                schema,
                ..Settings::default()
            };
            assert_eq!(
                settings.validate(),
                Err(SettingsError::UnsupportedSchema),
                "schema {schema}"
            );
        }
    }

    #[test]
    fn rejects_unknown_layout() {
        for layout in ["", "Avro", "qwerty", "custom-layout_2", "a b", "avro "] {
            let settings = Settings {
                layout: layout.to_owned(),
                ..Settings::default()
            };
            assert_eq!(
                settings.validate(),
                Err(SettingsError::InvalidLayout),
                "layout {layout:?}"
            );
        }
        for layout in KNOWN_LAYOUTS {
            let settings = Settings {
                layout: layout.to_owned(),
                ..Settings::default()
            };
            settings.validate().expect("known layout valid");
        }
    }

    #[test]
    fn rejects_invalid_language_theme_accent() {
        for field in ["language", "theme", "accent"] {
            let mut value = serde_json::to_value(Settings::default()).expect("serializes");
            value[field] = serde_json::Value::String("xx".to_owned());
            assert!(
                serde_json::from_value::<Settings>(value).is_err(),
                "{field} rejects unknown value"
            );
        }
    }

    #[test]
    fn rejects_non_finite_font_scale() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let settings = Settings {
                font_scale: bad,
                ..Settings::default()
            };
            assert_eq!(settings.validate(), Err(SettingsError::InvalidFontScale));
            // ...but clamping first (the `apply` path) rescues infinities
            // while NaN still fails.
            let clamped = settings.clamped();
            if bad.is_nan() {
                assert_eq!(clamped.validate(), Err(SettingsError::InvalidFontScale));
            } else {
                clamped.validate().expect("clamped infinity valid");
            }
        }
    }

    #[test]
    fn clamps_font_scale() {
        for (input, expected) in [
            (0.5, FONT_SCALE_MIN),
            (0.8, 0.8),
            (1.2, 1.2),
            (1.6, 1.6),
            (2.0, FONT_SCALE_MAX),
            (f32::INFINITY, FONT_SCALE_MAX),
            (f32::NEG_INFINITY, FONT_SCALE_MIN),
        ] {
            let settings = Settings {
                font_scale: input,
                ..Settings::default()
            };
            assert_eq!(settings.clamped().font_scale, expected, "input {input}");
        }
    }

    #[test]
    fn clamps_popup_max_items() {
        for (input, expected) in [
            (0, 1),
            (1, 1),
            (5, 5),
            (9, 9),
            (10, 10),
            (99, 10),
            (u8::MAX, 10),
        ] {
            let settings = Settings {
                popup_max_items: input,
                ..Settings::default()
            };
            assert_eq!(settings.clamped().popup_max_items, expected);
        }
    }

    #[test]
    fn clamps_page_size() {
        for (input, expected) in [(0, 10), (5, 10), (10, 10), (50, 50), (200, 200), (500, 200)] {
            let settings = Settings {
                word_manager_page_size: input,
                ..Settings::default()
            };
            assert_eq!(settings.clamped().word_manager_page_size, expected);
        }
    }

    #[test]
    fn toggle_hotkey_accepts_function_key_or_chord() {
        for text in ["F9", "f9", "F1", "F24", "Ctrl+Space", "Ctrl+Alt+F9"] {
            let hotkey = parse_toggle_hotkey(text).unwrap_or_else(|_| panic!("{text:?} valid"));
            assert!(hotkey.key.starts_with('F') || text.contains('+'));
        }
    }

    #[test]
    fn toggle_hotkey_rejects_bad_shapes() {
        for text in [
            "",
            "P",
            "Space",
            "Ctrl+Alt+P ",
            " F9",
            "F9 ",
            "Ctrl F9",
            "F0",
            "F25",
            "F01",
            "not-a-key-at-all-really-long-string-here",
        ] {
            assert_eq!(
                parse_toggle_hotkey(text),
                Err(SettingsError::InvalidHotkey),
                "{text:?}"
            );
        }
        // Over the 32-byte cap.
        assert_eq!(
            parse_toggle_hotkey(&"x".repeat(33)),
            Err(SettingsError::InvalidHotkey)
        );
        // Exactly 32 printable ASCII bytes is the cap.
        let max = format!("Ctrl+{}", "x".repeat(27));
        assert_eq!(max.len(), 32);
        assert_eq!(parse_toggle_hotkey(&max), Err(SettingsError::InvalidHotkey));
        let settings = Settings {
            toggle_hotkey: "bogus".to_owned(),
            ..Settings::default()
        };
        assert_eq!(settings.validate(), Err(SettingsError::InvalidHotkey));
    }

    #[test]
    fn rejects_non_finite_opacity_and_clamps() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let settings = Settings {
                quickbar_opacity: bad,
                ..Settings::default()
            };
            assert_eq!(settings.validate(), Err(SettingsError::InvalidValue));
            let clamped = settings.clamped();
            if bad.is_nan() {
                assert_eq!(clamped.validate(), Err(SettingsError::InvalidValue));
            } else {
                clamped.validate().expect("clamped infinity valid");
            }
        }
        for (input, expected) in [
            (0.1, QUICKBAR_OPACITY_MIN),
            (0.5, 0.5),
            (0.9, 0.9),
            (1.0, 1.0),
            (2.0, QUICKBAR_OPACITY_MAX),
        ] {
            let settings = Settings {
                quickbar_opacity: input,
                ..Settings::default()
            };
            assert_eq!(
                settings.clamped().quickbar_opacity,
                expected,
                "input {input}"
            );
        }
    }

    #[test]
    fn quickbar_coords_bounds() {
        for good in [-100_000, -1, 0, 1920, 100_000] {
            let settings = Settings {
                quickbar_x: Some(good),
                quickbar_y: Some(good),
                ..Settings::default()
            };
            settings.validate().expect("coord in range valid");
        }
        for bad in [-100_001, 100_001, i32::MIN, i32::MAX] {
            let settings = Settings {
                quickbar_x: Some(bad),
                ..Settings::default()
            };
            assert_eq!(settings.validate(), Err(SettingsError::InvalidValue));
            let settings = Settings {
                quickbar_y: Some(bad),
                ..Settings::default()
            };
            assert_eq!(settings.validate(), Err(SettingsError::InvalidValue));
        }
        // Null stays null through a merge.
        let cleared = Settings::default()
            .apply(&SettingsPatch {
                quickbar_x: Some(Some(100)),
                ..SettingsPatch::default()
            })
            .expect("sets")
            .apply(&SettingsPatch {
                quickbar_x: Some(None),
                ..SettingsPatch::default()
            })
            .expect("clears to null");
        assert_eq!(cleared.quickbar_x, None);
    }

    #[test]
    fn old_files_without_new_fields_load_with_defaults() {
        // The pre-extension v1 shape: new fields default instead of failing,
        // so existing installs migrate without loss.
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "schema": 1,
            "language": "bn",
            "layout": "avro",
            "theme": "system",
            "accent": "teal",
            "font_scale": 1.0,
            "popup_enabled": true,
            "popup_max_items": 5,
            "pause_hotkey": "Ctrl+Alt+Shift+P",
            "autostart": false,
            "learning_enabled": true,
            "word_manager_page_size": 50,
            "ai_enabled": false,
            "speech_enabled": false,
        }))
        .expect("old shape loads");
        let defaults = Settings::default();
        assert_eq!(settings.input_mode, defaults.input_mode);
        assert_eq!(settings.output_encoding, defaults.output_encoding);
        assert_eq!(settings.font_unicode, defaults.font_unicode);
        assert_eq!(settings.font_bijoy, defaults.font_bijoy);
        assert_eq!(settings.toggle_hotkey, "F9");
        assert!(settings.show_toggle_toast);
        assert!(settings.toggle_sound);
        assert!(settings.quickbar_visible);
        assert!(!settings.quickbar_collapsed);
        assert_eq!(settings.quickbar_x, None);
        assert_eq!(settings.quickbar_y, None);
        assert_eq!(settings.quickbar_opacity, 0.9);
        settings.validate().expect("migrated valid");
    }

    #[test]
    fn patch_merges_new_fields() {
        let base = Settings::default();
        let merged = base
            .apply(&SettingsPatch {
                input_mode: Some(InputMode::English),
                output_encoding: Some(OutputEncoding::Bijoy),
                font_unicode: Some(FontUnicode::NotoSansBengali),
                font_bijoy: Some(FontBijoy::KalpurushAnsi),
                toggle_hotkey: Some("Ctrl+Space".to_owned()),
                show_toggle_toast: Some(false),
                toggle_sound: Some(false),
                quickbar_visible: Some(false),
                quickbar_collapsed: Some(true),
                quickbar_x: Some(Some(120)),
                quickbar_y: Some(Some(40)),
                quickbar_opacity: Some(2.0),
                ..SettingsPatch::default()
            })
            .expect("merges");
        assert_eq!(merged.input_mode, InputMode::English);
        assert_eq!(merged.output_encoding, OutputEncoding::Bijoy);
        assert_eq!(merged.font_unicode, FontUnicode::NotoSansBengali);
        assert_eq!(merged.font_bijoy, FontBijoy::KalpurushAnsi);
        assert_eq!(merged.toggle_hotkey, "Ctrl+Space");
        assert!(!merged.show_toggle_toast);
        assert!(!merged.toggle_sound);
        assert!(!merged.quickbar_visible);
        assert!(merged.quickbar_collapsed);
        assert_eq!(merged.quickbar_x, Some(120));
        assert_eq!(merged.quickbar_y, Some(40));
        assert_eq!(merged.quickbar_opacity, QUICKBAR_OPACITY_MAX);
        // Untouched legacy fields survive.
        assert_eq!(merged.pause_hotkey, "Ctrl+Alt+Shift+P");

        assert_eq!(
            base.apply(&SettingsPatch {
                toggle_hotkey: Some("bogus!!".to_owned()),
                ..SettingsPatch::default()
            }),
            Err(SettingsError::InvalidHotkey)
        );
        assert_eq!(
            base.apply(&SettingsPatch {
                quickbar_x: Some(Some(100_001)),
                ..SettingsPatch::default()
            }),
            Err(SettingsError::InvalidValue)
        );
        assert_eq!(
            base.apply(&SettingsPatch {
                quickbar_opacity: Some(f32::NAN),
                ..SettingsPatch::default()
            }),
            Err(SettingsError::InvalidValue)
        );
    }

    #[test]
    fn font_family_rules() {
        // Trims, caps at 64 chars, ASCII printable only.
        let base = Settings::default();
        let out = base
            .apply(&SettingsPatch {
                font_family: Some(Some("  Noto Sans  ".to_owned())),
                ..SettingsPatch::default()
            })
            .expect("trims");
        assert_eq!(out.font_family.as_deref(), Some("Noto Sans"));

        let cleared = base
            .apply(&SettingsPatch {
                font_family: Some(None),
                ..SettingsPatch::default()
            })
            .expect("clears");
        assert_eq!(cleared.font_family, None);

        let blank = base
            .apply(&SettingsPatch {
                font_family: Some(Some("   ".to_owned())),
                ..SettingsPatch::default()
            })
            .expect("blank clears");
        assert_eq!(blank.font_family, None);

        for bad in ["x".repeat(65), "Noto\nSans".to_owned(), "অভ্র".to_owned()] {
            let err = base.apply(&SettingsPatch {
                font_family: Some(Some(bad.clone())),
                ..SettingsPatch::default()
            });
            assert_eq!(err, Err(SettingsError::InvalidFontFamily), "family {bad:?}");
        }
        let ok = "x".repeat(64);
        let kept = base
            .apply(&SettingsPatch {
                font_family: Some(Some(ok.clone())),
                ..SettingsPatch::default()
            })
            .expect("64 chars ok");
        assert_eq!(kept.font_family.as_deref(), Some(ok.as_str()));
    }

    #[test]
    fn valid_hotkeys_accepted() {
        for text in [
            "Ctrl+Alt+P",
            "ctrl+alt+p",
            "Ctrl+Shift+F5",
            "Alt+Space",
            "Ctrl+Alt+Shift+Delete",
            "Win+Enter",
            "Ctrl+1",
        ] {
            parse_hotkey(text).unwrap_or_else(|_| panic!("{text:?} valid"));
        }
    }

    #[test]
    fn invalid_hotkeys_rejected() {
        for text in [
            "",
            "P",
            "Ctrl",
            "Ctrl+",
            "+P",
            "Ctrl++P",
            "Ctrl+Ctrl+P",
            "Ctrl+Alt+",
            "Foo+P",
            "Ctrl+Foo",
            "Ctrl+Alt+Shift",
            "Ctrl+?",
            "Ctrl+Shift",
            "Ctrl+F25",
            "Ctrl+F0",
        ] {
            assert_eq!(
                parse_hotkey(text),
                Err(SettingsError::InvalidHotkey),
                "{text:?}"
            );
        }
        let settings = Settings {
            pause_hotkey: "bogus".to_owned(),
            ..Settings::default()
        };
        assert_eq!(settings.validate(), Err(SettingsError::InvalidHotkey));
    }

    #[test]
    fn patch_merges_clamps_and_validates() {
        let base = Settings::default();
        let merged = base
            .apply(&SettingsPatch {
                theme: Some(Theme::Dark),
                font_scale: Some(9.0),
                popup_max_items: Some(0),
                ..SettingsPatch::default()
            })
            .expect("merges");
        assert_eq!(merged.theme, Theme::Dark);
        assert_eq!(merged.font_scale, FONT_SCALE_MAX);
        assert_eq!(merged.popup_max_items, POPUP_MAX_ITEMS_MIN);
        // Untouched fields survive.
        assert_eq!(merged.layout, "avro");
        assert_eq!(merged.language, Language::Bn);

        assert_eq!(
            base.apply(&SettingsPatch {
                layout: Some("nope".to_owned()),
                ..SettingsPatch::default()
            }),
            Err(SettingsError::InvalidLayout)
        );
        assert_eq!(
            base.apply(&SettingsPatch {
                font_scale: Some(f32::NAN),
                ..SettingsPatch::default()
            }),
            Err(SettingsError::InvalidFontScale)
        );
    }

    #[test]
    fn error_codes_map_to_protocol() {
        assert_eq!(
            SettingsError::UnsupportedSchema.code(),
            ErrorCode::UnsupportedSchema
        );
        assert_eq!(
            SettingsError::InvalidLayout.code(),
            ErrorCode::InvalidLayout
        );
        assert_eq!(
            SettingsError::InvalidHotkey.code(),
            ErrorCode::InvalidHotkey
        );
        assert_eq!(
            SettingsError::InvalidFontScale.code(),
            ErrorCode::InvalidSettings
        );
        assert_eq!(
            SettingsError::InvalidFontFamily.code(),
            ErrorCode::InvalidSettings
        );
        assert_eq!(
            SettingsError::InvalidValue.code(),
            ErrorCode::InvalidSettings
        );
    }
}

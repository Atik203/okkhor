//! Per-user font management for Okkhor.
//!
//! Okkhor renders Bijoy byte text with a Bijoy-compatible font (Kalpurush
//! ANSI by default, SutonnyMJ where the user already has it) and Unicode
//! text with Kalpurush / Noto Sans Bengali. Fonts ship as data, never as
//! binaries in this repository: `scripts/fetch-fonts.ps1` downloads them
//! into `assets/fonts/` at build time.
//!
//! This module installs those files per user only: it copies into
//! `%LOCALAPPDATA%\Microsoft\Windows\Fonts`, registers under
//! `HKCU\...\Fonts`, and broadcasts `WM_FONTCHANGE`. No elevation is ever
//! needed and nothing is written outside the user's own profile.
//!
//! The registry value name comes from the font's own name table
//! ([`font_family_name`], e.g. `Kalpurush (TrueType)`), falling back to the
//! file stem. [`list_bundled_fonts`] reports which expected files are
//! present; it performs no logging, so user paths can never leak.
//!
//! Tests never touch the real registry or fonts directory: pass
//! [`InstallOptions`] with `dry_run = true` and a temporary `fonts_dir`,
//! which exercises the same copy/idempotence logic without Win32 calls.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Maximum accepted font file size (64 MiB); bounds every read.
pub const MAX_FONT_BYTES: u64 = 64 * 1024 * 1024;

/// Role of a bundled font file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontRole {
    /// Unicode UI/editor font (Kalpurush).
    UnicodeUi,
    /// Bijoy-ANSI rendering font (Kalpurush ANSI).
    BijoyAnsi,
    /// Unicode fallback font (Noto Sans Bengali).
    Fallback,
}

impl FontRole {
    /// Stable machine-readable role code.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnicodeUi => "unicode-ui",
            Self::BijoyAnsi => "bijoy-ansi",
            Self::Fallback => "fallback",
        }
    }
}

/// Expected bundled font files in install order: `(file name, role)`.
pub const BUNDLED_FONTS: &[(&str, FontRole)] = &[
    ("kalpurush.ttf", FontRole::UnicodeUi),
    ("kalpurush ANSI.ttf", FontRole::BijoyAnsi),
    ("NotoSansBengali.ttf", FontRole::Fallback),
];

/// Presence of one expected bundled font file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundledFont {
    /// Expected file name, e.g. `"kalpurush.ttf"`.
    pub file_name: &'static str,
    /// Role of this font.
    pub role: FontRole,
    /// Whether `dir.join(file_name)` is a file.
    pub present: bool,
}

/// Result of [`install_font_per_user`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledFont {
    /// Final per-user copy of the font.
    pub path: PathBuf,
    /// True when an identical file was already installed (copy skipped).
    pub already_installed: bool,
}

/// Result of [`uninstall_font_per_user`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemovedFont {
    /// True when the per-user font file was deleted.
    pub removed_file: bool,
    /// True when a registry registration was deleted.
    pub removed_registration: bool,
}

/// Options for [`install_font_per_user_with_options`] and
/// [`uninstall_font_per_user_with_options`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallOptions {
    /// Override the `%LOCALAPPDATA%\Microsoft\Windows\Fonts` directory.
    /// Used by tests with a temporary directory.
    pub fonts_dir: Option<PathBuf>,
    /// When true, copy/delete files but skip every Win32 call (registry,
    /// `AddFontResourceW`, broadcast). Used by tests so the real registry
    /// is never touched.
    pub dry_run: bool,
}

/// Typed font errors. `Display` strings are static: they never contain user
/// text or file paths. Use [`FontError::kind`] for logging.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FontError {
    /// Path is empty, has no file name, or (for uninstall) is not a plain
    /// file name; also used when `%LOCALAPPDATA%` is unavailable.
    #[error("font path is invalid")]
    InvalidPath,
    /// Source font file does not exist.
    #[error("font file not found")]
    NotFound,
    /// Extension is not `.ttf`, `.otf`, or `.ttc`.
    #[error("unsupported font extension")]
    UnsupportedExtension,
    /// Font file exceeds [`MAX_FONT_BYTES`].
    #[error("font file is too large")]
    TooLarge,
    /// Filesystem I/O failed.
    #[error("font file operation failed")]
    Io(#[from] std::io::Error),
    /// Registry read or write failed.
    #[error("font registry update failed")]
    Registry,
    /// A Win32 font call failed; carries the raw error code.
    #[error("windows font call failed")]
    Win32(u32),
    /// System registration is unavailable on this platform.
    #[error("not supported on this platform")]
    UnsupportedPlatform,
}

impl FontError {
    /// Stable snake_case identifier for logging.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::InvalidPath => "font_invalid_path",
            Self::NotFound => "font_not_found",
            Self::UnsupportedExtension => "font_unsupported_extension",
            Self::TooLarge => "font_too_large",
            Self::Io(_) => "font_io",
            Self::Registry => "font_registry",
            Self::Win32(_) => "font_win32",
            Self::UnsupportedPlatform => "font_unsupported_platform",
        }
    }
}

impl PartialEq for FontError {
    /// Variant-level equality for tests; `Io` payloads compare by kind only
    /// so assertions stay independent of OS messages.
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::InvalidPath, Self::InvalidPath)
            | (Self::NotFound, Self::NotFound)
            | (Self::UnsupportedExtension, Self::UnsupportedExtension)
            | (Self::TooLarge, Self::TooLarge)
            | (Self::Registry, Self::Registry)
            | (Self::UnsupportedPlatform, Self::UnsupportedPlatform) => true,
            (Self::Win32(a), Self::Win32(b)) => a == b,
            (Self::Io(a), Self::Io(b)) => a.kind() == b.kind(),
            _ => false,
        }
    }
}

impl Eq for FontError {}

/// Validate a font source path: non-empty, points at an existing file with
/// a font extension, within the size bound.
pub fn validate_font_path(path: &Path) -> Result<(), FontError> {
    if path.as_os_str().is_empty() {
        return Err(FontError::InvalidPath);
    }
    let extension = path
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if !matches!(extension.as_str(), "ttf" | "otf" | "ttc") {
        return Err(FontError::UnsupportedExtension);
    }
    let metadata = std::fs::metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            FontError::NotFound
        } else {
            FontError::Io(error)
        }
    })?;
    if !metadata.is_file() {
        return Err(FontError::InvalidPath);
    }
    if metadata.len() > MAX_FONT_BYTES {
        return Err(FontError::TooLarge);
    }
    Ok(())
}

/// Read a font's family name from its sfnt `name` table.
///
/// Prefers the Windows Unicode entry (platform 3) for name id 1, then the
/// Macintosh entry (platform 1). Returns `None` for truncated or
/// unrecognized data. Pure parsing: no I/O, no Win32.
pub fn font_family_name(data: &[u8]) -> Option<String> {
    let table_count = u16::from_be_bytes(data.get(4..6)?.try_into().ok()?) as usize;
    let mut name_table: Option<&[u8]> = None;
    for i in 0..table_count {
        let base = 12usize.checked_add(i.checked_mul(16)?)?;
        let entry = data.get(base..base.checked_add(16)?)?;
        if entry[0..4] != *b"name" {
            continue;
        }
        let offset = u32::from_be_bytes(entry[8..12].try_into().ok()?) as usize;
        let length = u32::from_be_bytes(entry[12..16].try_into().ok()?) as usize;
        name_table = Some(data.get(offset..offset.checked_add(length)?)?);
        break;
    }
    let table = name_table?;
    let count = u16::from_be_bytes(table.get(2..4)?.try_into().ok()?) as usize;
    let storage_offset = u16::from_be_bytes(table.get(4..6)?.try_into().ok()?) as usize;
    let mut mac_fallback: Option<&[u8]> = None;
    for i in 0..count {
        let base = 6usize.checked_add(i.checked_mul(12)?)?;
        let record = table.get(base..base.checked_add(12)?)?;
        let platform = u16::from_be_bytes(record[0..2].try_into().ok()?);
        let name_id = u16::from_be_bytes(record[6..8].try_into().ok()?);
        if name_id != 1 {
            continue;
        }
        let length = u16::from_be_bytes(record[8..10].try_into().ok()?) as usize;
        let offset = u16::from_be_bytes(record[10..12].try_into().ok()?) as usize;
        let bytes = table
            .get(storage_offset.checked_add(offset)?..)?
            .get(..length)?;
        if platform == 3 {
            return decode_family_name(bytes, true);
        } else if platform == 1 && mac_fallback.is_none() {
            mac_fallback = Some(bytes);
        }
    }
    mac_fallback.and_then(|bytes| decode_family_name(bytes, false))
}

/// Decode one name-table string; `wide` selects UTF-16BE (Windows) versus
/// single-byte Macintosh Roman (ASCII subset only, else `None`).
fn decode_family_name(bytes: &[u8], wide: bool) -> Option<String> {
    let text = if wide {
        if bytes.len() % 2 != 0 {
            return None;
        }
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16(&units).ok()?
    } else {
        if !bytes.iter().all(|byte| byte.is_ascii()) {
            return None;
        }
        String::from_utf8(bytes.to_vec()).ok()?
    };
    let text = text.trim().to_owned();
    if text.is_empty() || text.len() > 128 {
        return None;
    }
    Some(text)
}

/// Registry value name for an installed font, e.g.
/// `Kalpurush (TrueType)`. Takes the family name (or file stem fallback)
/// plus the file extension.
pub fn font_registry_value_name(family_or_stem: &str, extension: &str) -> String {
    let kind = match extension.to_ascii_lowercase().as_str() {
        "otf" => "OpenType",
        "ttc" => "TrueType Collection",
        _ => "TrueType",
    };
    format!("{family_or_stem} ({kind})")
}

/// Resolve the per-user fonts directory
/// (`%LOCALAPPDATA%\Microsoft\Windows\Fonts`).
pub fn user_fonts_dir() -> Result<PathBuf, FontError> {
    let local = std::env::var_os("LOCALAPPDATA").ok_or(FontError::InvalidPath)?;
    let dir = PathBuf::from(local)
        .join("Microsoft")
        .join("Windows")
        .join("Fonts");
    if dir.as_os_str().is_empty() {
        return Err(FontError::InvalidPath);
    }
    Ok(dir)
}

/// Report which expected bundled font files are present in `dir`.
/// Performs no logging, so directory contents never leak into logs.
pub fn list_bundled_fonts(dir: &Path) -> Vec<BundledFont> {
    BUNDLED_FONTS
        .iter()
        .map(|(file_name, role)| BundledFont {
            file_name,
            role: *role,
            present: dir.join(file_name).is_file(),
        })
        .collect()
}

/// Load a font file as a private, non-enumerated process font.
#[cfg(windows)]
pub fn load_private_font(path: &Path) -> Result<(), FontError> {
    use windows::Win32::Graphics::Gdi::{
        AddFontResourceExW, FONT_RESOURCE_CHARACTERISTICS, FR_NOT_ENUM, FR_PRIVATE,
    };
    use windows::core::PCWSTR;

    validate_font_path(path)?;
    let wide = to_wide_null(path.as_os_str());
    let flags = FONT_RESOURCE_CHARACTERISTICS(FR_PRIVATE.0 | FR_NOT_ENUM.0);
    // SAFETY: `wide` is a null-terminated UTF-16 buffer that outlives the
    // call; the flags only request private loading, which has no
    // process-wide effect. A zero return means no font was added.
    let added = unsafe { AddFontResourceExW(PCWSTR(wide.as_ptr()), flags, None) };
    if added == 0 {
        return Err(FontError::Win32(last_error_code()));
    }
    Ok(())
}

/// Unload a font previously loaded with [`load_private_font`].
#[cfg(windows)]
pub fn unload_private_font(path: &Path) -> Result<(), FontError> {
    use windows::Win32::Graphics::Gdi::{FR_NOT_ENUM, FR_PRIVATE, RemoveFontResourceExW};
    use windows::core::PCWSTR;

    validate_font_path(path)?;
    let wide = to_wide_null(path.as_os_str());
    // SAFETY: `wide` is a null-terminated UTF-16 buffer that outlives the
    // call; the flag word matches the private load, so only this process's
    // private registration is removed. False means it was not registered.
    let removed =
        unsafe { RemoveFontResourceExW(PCWSTR(wide.as_ptr()), FR_PRIVATE.0 | FR_NOT_ENUM.0, None) };
    if !removed.as_bool() {
        return Err(FontError::Win32(last_error_code()));
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn load_private_font(path: &Path) -> Result<(), FontError> {
    validate_font_path(path)?;
    Err(FontError::UnsupportedPlatform)
}

#[cfg(not(windows))]
pub fn unload_private_font(path: &Path) -> Result<(), FontError> {
    validate_font_path(path)?;
    Err(FontError::UnsupportedPlatform)
}

/// Install a font file per user: copy into the user's fonts directory,
/// register under `HKCU\...\Fonts`, add the resource, broadcast
/// `WM_FONTCHANGE`. Idempotent when the same file is already installed.
pub fn install_font_per_user(path: &Path) -> Result<InstalledFont, FontError> {
    install_font_per_user_with_options(path, &InstallOptions::default())
}

/// [`install_font_per_user`] with an explicit [`InstallOptions`]: tests pass
/// a temporary `fonts_dir` with `dry_run = true` so the real registry and
/// font table are never touched.
pub fn install_font_per_user_with_options(
    path: &Path,
    options: &InstallOptions,
) -> Result<InstalledFont, FontError> {
    validate_font_path(path)?;
    let bytes = std::fs::read(path)?;
    if bytes.len() as u64 > MAX_FONT_BYTES {
        return Err(FontError::TooLarge);
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(FontError::InvalidPath)?;
    let extension = path.extension().and_then(OsStr::to_str).unwrap_or("ttf");
    let family = font_family_name(&bytes).unwrap_or_else(|| file_stem_or_name(file_name));
    let value_name = font_registry_value_name(&family, extension);

    let fonts_dir = match &options.fonts_dir {
        Some(dir) => dir.clone(),
        None => user_fonts_dir()?,
    };
    let dest = fonts_dir.join(file_name);

    let already_installed = match std::fs::read(&dest) {
        Ok(existing) if existing == bytes => true,
        Ok(_) => {
            std::fs::write(&dest, &bytes)?;
            false
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&dest, &bytes)?;
            false
        }
        Err(error) => return Err(FontError::Io(error)),
    };

    if !options.dry_run {
        register_font(&dest, &value_name, file_name)?;
    }
    Ok(InstalledFont {
        path: dest,
        already_installed,
    })
}

/// Remove a per-user font installed by [`install_font_per_user`].
/// `name` is a plain file name such as `"kalpurush.ttf"`; paths are
/// rejected. Missing files/registrations are tolerated and reported.
pub fn uninstall_font_per_user(name: &str) -> Result<RemovedFont, FontError> {
    uninstall_font_per_user_with_options(name, &InstallOptions::default())
}

/// [`uninstall_font_per_user`] with an explicit [`InstallOptions`]; see
/// [`install_font_per_user_with_options`] for the test story.
pub fn uninstall_font_per_user_with_options(
    name: &str,
    options: &InstallOptions,
) -> Result<RemovedFont, FontError> {
    // A plain file name only: no separators, no parent traversal, one
    // extension, and a font suffix.
    if name.is_empty() || name.len() > 128 {
        return Err(FontError::InvalidPath);
    }
    if name.contains('/') || name.contains('\\') || name.contains(':') {
        return Err(FontError::InvalidPath);
    }
    let (stem, extension) = name.rsplit_once('.').ok_or(FontError::InvalidPath)?;
    if stem.is_empty() || stem.contains('.') || name == "." || name == ".." {
        return Err(FontError::InvalidPath);
    }
    if !matches!(
        extension.to_ascii_lowercase().as_str(),
        "ttf" | "otf" | "ttc"
    ) {
        return Err(FontError::UnsupportedExtension);
    }

    let fonts_dir = match &options.fonts_dir {
        Some(dir) => dir.clone(),
        None => user_fonts_dir()?,
    };
    let dest = fonts_dir.join(name);

    let mut removed = RemovedFont::default();
    match std::fs::remove_file(&dest) {
        Ok(()) => removed.removed_file = true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(FontError::Io(error)),
    }
    if !options.dry_run {
        removed.removed_registration = unregister_font(name)?;
    }
    Ok(removed)
}

/// File stem (or full name when non-UTF8) used when the name table has no
/// family name.
fn file_stem_or_name(file_name: &str) -> String {
    Path::new(file_name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(file_name)
        .to_owned()
}

#[cfg(windows)]
fn to_wide_null(value: &OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt as _;
    value.encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn last_error_code() -> u32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32
}

#[cfg(windows)]
const FONTS_KEY_W: windows::core::PCWSTR =
    windows::core::w!(r"Software\Microsoft\Windows NT\CurrentVersion\Fonts");

/// Owns an open registry key and closes it on every exit path.
#[cfg(windows)]
struct RegistryKey(windows::Win32::System::Registry::HKEY);

#[cfg(windows)]
impl Drop for RegistryKey {
    fn drop(&mut self) {
        // SAFETY: the handle came from RegOpenKeyExW/RegCreateKeyExW and is
        // closed exactly once here.
        unsafe {
            let _ = windows::Win32::System::Registry::RegCloseKey(self.0);
        }
    }
}

#[cfg(windows)]
fn register_font(dest: &Path, value_name: &str, file_name: &str) -> Result<(), FontError> {
    use windows::Win32::Graphics::Gdi::AddFontResourceW;
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCreateKeyExW,
        RegSetValueExW,
    };
    use windows::core::PCWSTR;

    let wide_dest = to_wide_null(dest.as_os_str());
    let wide_value = to_wide_null(OsStr::new(value_name));
    let file_wide = to_wide_null(OsStr::new(file_name));
    // REG_SZ payload is UTF-16LE including the terminator; the registry API
    // copies the bytes before returning, so this owned buffer is retained
    // only for the call.
    let payload: Vec<u8> = file_wide
        .iter()
        .flat_map(|unit| unit.to_le_bytes())
        .collect();

    // SAFETY: the subkey literal is null-terminated, `key` points to valid
    // local storage, and the handle is closed by `RegistryKey`. HKCU only:
    // no elevation, no machine-wide state.
    let mut key = HKEY::default();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            FONTS_KEY_W,
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
    };
    if status.is_err() {
        return Err(FontError::Registry);
    }
    let key = RegistryKey(key);

    // SAFETY: `key` is open and owned, the value name is null-terminated,
    // and `payload` outlives the call.
    let status = unsafe {
        RegSetValueExW(
            key.0,
            PCWSTR(wide_value.as_ptr()),
            None,
            REG_SZ,
            Some(&payload),
        )
    };
    if status.is_err() {
        return Err(FontError::Registry);
    }

    // SAFETY: `wide_dest` outlives this call; adding a session font is
    // additive and scoped by the registry entry for persistence.
    let added = unsafe { AddFontResourceW(PCWSTR(wide_dest.as_ptr())) };
    if added == 0 {
        return Err(FontError::Win32(last_error_code()));
    }
    broadcast_font_change();
    Ok(())
}

#[cfg(windows)]
fn unregister_font(file_name: &str) -> Result<bool, FontError> {
    use windows::Win32::Foundation::ERROR_NO_MORE_ITEMS;
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SAM_FLAGS, RegDeleteValueW,
        RegEnumValueW, RegOpenKeyExW,
    };
    use windows::core::{PCWSTR, PWSTR};

    let access = REG_SAM_FLAGS(KEY_QUERY_VALUE.0 | KEY_SET_VALUE.0);
    // SAFETY: the root and subkey are valid, `key` points to valid local
    // storage, and the handle is closed by `RegistryKey`. HKCU only.
    let mut key = HKEY::default();
    let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, FONTS_KEY_W, None, access, &mut key) };
    if status.is_err() {
        return Err(FontError::Registry);
    }
    let key = RegistryKey(key);

    // Find every value whose data names this font file (registrations may
    // use the family value name or an older duplicate).
    let mut removed = false;
    let mut index = 0u32;
    loop {
        let mut name_buf = vec![0u16; 256];
        let mut name_len = 256u32;
        let mut value_type = 0u32;
        let mut data_buf = vec![0u8; 1024];
        let mut data_len = 1024u32;
        // SAFETY: all out-parameters point to valid local buffers with the
        // advertised lengths; `key` is open. Short buffers report an error
        // instead of overflowing.
        let status = unsafe {
            RegEnumValueW(
                key.0,
                index,
                Some(PWSTR(name_buf.as_mut_ptr())),
                &mut name_len,
                None,
                Some(&mut value_type),
                Some(data_buf.as_mut_ptr()),
                Some(&mut data_len),
            )
        };
        if status == ERROR_NO_MORE_ITEMS {
            break;
        }
        index += 1;
        if status.is_err() {
            continue;
        }
        let unit_count = data_len as usize / 2;
        let stored: String = String::from_utf16_lossy(
            &data_buf[..unit_count * 2]
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<u16>>(),
        );
        if stored
            .trim_end_matches('\0')
            .eq_ignore_ascii_case(file_name)
        {
            name_buf.truncate(name_len as usize);
            // SAFETY: `name_buf` holds the null-terminated value name just
            // reported by the enumeration on the same open key.
            let status = unsafe { RegDeleteValueW(key.0, PCWSTR(name_buf.as_ptr())) };
            if !status.is_err() {
                removed = true;
            }
        }
    }
    if removed {
        broadcast_font_change();
    }
    Ok(removed)
}

#[cfg(windows)]
fn broadcast_font_change() {
    use windows::Win32::UI::WindowsAndMessaging::{
        HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_FONTCHANGE,
    };

    // SAFETY: broadcast with null w/lparams is the documented WM_FONTCHANGE
    // pattern; abort-if-hung with a 5 s cap keeps this off any hot path
    // (install/uninstall only, never typing).
    unsafe {
        let mut _result = 0usize;
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_FONTCHANGE,
            Default::default(),
            Default::default(),
            SMTO_ABORTIFHUNG,
            5000,
            Some(&mut _result),
        );
    }
}

#[cfg(not(windows))]
fn register_font(_dest: &Path, _value_name: &str, _file_name: &str) -> Result<(), FontError> {
    Err(FontError::UnsupportedPlatform)
}

#[cfg(not(windows))]
fn unregister_font(_file_name: &str) -> Result<bool, FontError> {
    Err(FontError::UnsupportedPlatform)
}

#[cfg(test)]
mod tests {
    use super::{
        BUNDLED_FONTS, FontError, FontRole, InstallOptions, font_family_name,
        font_registry_value_name, install_font_per_user_with_options, list_bundled_fonts,
        uninstall_font_per_user_with_options,
    };
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("okkhor-font-tests").join(tag);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn options_for(dir: &std::path::Path) -> InstallOptions {
        InstallOptions {
            fonts_dir: Some(dir.to_path_buf()),
            dry_run: true,
        }
    }

    /// Minimal sfnt with one `name` record (platform 3, name id 1).
    fn sfnt_with_family(family: &str) -> Vec<u8> {
        let units: Vec<u16> = family.encode_utf16().collect();
        let mut string_data = Vec::with_capacity(units.len() * 2);
        for unit in &units {
            string_data.extend_from_slice(&unit.to_be_bytes());
        }
        let mut name_table = Vec::new();
        name_table.extend_from_slice(&0u16.to_be_bytes()); // format
        name_table.extend_from_slice(&1u16.to_be_bytes()); // count
        name_table.extend_from_slice(&18u16.to_be_bytes()); // string offset
        name_table.extend_from_slice(&3u16.to_be_bytes()); // platform 3
        name_table.extend_from_slice(&1u16.to_be_bytes()); // encoding 1
        name_table.extend_from_slice(&0x0409u16.to_be_bytes()); // lang
        name_table.extend_from_slice(&1u16.to_be_bytes()); // name id 1
        name_table.extend_from_slice(&(string_data.len() as u16).to_be_bytes());
        name_table.extend_from_slice(&0u16.to_be_bytes()); // offset
        name_table.extend_from_slice(&string_data);

        let mut sfnt = Vec::new();
        sfnt.extend_from_slice(&0x00010000u32.to_be_bytes()); // version
        sfnt.extend_from_slice(&1u16.to_be_bytes()); // one table
        sfnt.extend_from_slice(&0u16.to_be_bytes()); // search range etc.
        sfnt.extend_from_slice(&0u16.to_be_bytes());
        sfnt.extend_from_slice(&0u16.to_be_bytes());
        sfnt.extend_from_slice(b"name");
        sfnt.extend_from_slice(&0u32.to_be_bytes()); // checksum
        let table_offset = (12 + 16) as u32;
        sfnt.extend_from_slice(&table_offset.to_be_bytes());
        sfnt.extend_from_slice(&(name_table.len() as u32).to_be_bytes());
        sfnt.extend_from_slice(&name_table);
        sfnt
    }

    fn write_font(dir: &std::path::Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, bytes).expect("write font");
        path
    }

    #[test]
    fn family_name_parses_windows_entry() {
        let data = sfnt_with_family("Kalpurush");
        assert_eq!(font_family_name(&data).as_deref(), Some("Kalpurush"));
    }

    #[test]
    fn family_name_rejects_truncated_data() {
        assert_eq!(font_family_name(&[]), None);
        assert_eq!(font_family_name(&[0u8; 20]), None);
        let mut data = sfnt_with_family("Kalpurush ANSI");
        data.truncate(data.len() - 4);
        assert_eq!(font_family_name(&data), None);
    }

    #[test]
    fn registry_value_names_follow_windows_convention() {
        assert_eq!(
            font_registry_value_name("Kalpurush", "ttf"),
            "Kalpurush (TrueType)"
        );
        assert_eq!(
            font_registry_value_name("Kalpurush ANSI", "TTF"),
            "Kalpurush ANSI (TrueType)"
        );
        assert_eq!(
            font_registry_value_name("Noto Sans Bengali", "otf"),
            "Noto Sans Bengali (OpenType)"
        );
    }

    #[test]
    fn validation_rejects_bad_paths() {
        let dir = temp_dir("validate");
        assert_eq!(
            super::validate_font_path(std::path::Path::new("")),
            Err(FontError::InvalidPath)
        );
        assert!(matches!(
            super::validate_font_path(&dir.join("missing.ttf")),
            Err(FontError::NotFound)
        ));
        let txt = write_font(&dir, "notes.txt", b"not a font");
        assert_eq!(
            super::validate_font_path(&txt),
            Err(FontError::UnsupportedExtension)
        );
        let dir_as_font = dir.join("subdir.ttf");
        std::fs::create_dir_all(&dir_as_font).expect("mkdir");
        assert_eq!(
            super::validate_font_path(&dir_as_font),
            Err(FontError::InvalidPath)
        );
    }

    #[test]
    fn dry_run_install_is_idempotent() {
        let src = temp_dir("install-src");
        let dst = temp_dir("install-dst");
        let options = options_for(&dst);
        let bytes = sfnt_with_family("Kalpurush");
        let path = write_font(&src, "kalpurush.ttf", &bytes);

        let first = install_font_per_user_with_options(&path, &options).expect("install");
        assert!(!first.already_installed);
        assert_eq!(first.path, dst.join("kalpurush.ttf"));
        assert_eq!(std::fs::read(&first.path).expect("read"), bytes);

        let second = install_font_per_user_with_options(&path, &options).expect("reinstall");
        assert!(second.already_installed);

        // Different bytes overwrite and report a fresh install.
        let changed = sfnt_with_family("Kalpurush Changed");
        std::fs::write(&path, &changed).expect("rewrite");
        let third = install_font_per_user_with_options(&path, &options).expect("overwrite");
        assert!(!third.already_installed);
        assert_eq!(std::fs::read(&third.path).expect("read"), changed);
    }

    #[test]
    fn dry_run_uninstall_removes_file_only() {
        let src = temp_dir("uninstall-src");
        let dst = temp_dir("uninstall-dst");
        let options = options_for(&dst);
        let path = write_font(&src, "kalpurush.ttf", &sfnt_with_family("Kalpurush"));
        install_font_per_user_with_options(&path, &options).expect("install");

        let removed =
            uninstall_font_per_user_with_options("kalpurush.ttf", &options).expect("uninstall");
        assert!(removed.removed_file);
        assert!(!removed.removed_registration);
        assert!(!dst.join("kalpurush.ttf").exists());

        let again = uninstall_font_per_user_with_options("kalpurush.ttf", &options)
            .expect("uninstall again");
        assert!(!again.removed_file);
    }

    #[test]
    fn uninstall_rejects_paths_and_bad_extensions() {
        let dst = temp_dir("uninstall-bad");
        let options = options_for(&dst);
        for bad in [
            "",
            "../evil.ttf",
            "sub/dir.ttf",
            "C:\\fonts\\a.ttf",
            "font.exe",
            "font",
            ".",
            "..",
        ] {
            assert!(
                uninstall_font_per_user_with_options(bad, &options).is_err(),
                "must reject {bad:?}"
            );
        }
    }

    #[test]
    fn bundled_list_reports_roles_and_presence() {
        let dir = temp_dir("bundled");
        write_font(&dir, "kalpurush.ttf", &sfnt_with_family("Kalpurush"));
        let listed = list_bundled_fonts(&dir);
        assert_eq!(listed.len(), BUNDLED_FONTS.len());
        assert_eq!(listed[0].file_name, "kalpurush.ttf");
        assert_eq!(listed[0].role, FontRole::UnicodeUi);
        assert!(listed[0].present);
        assert_eq!(listed[1].role, FontRole::BijoyAnsi);
        assert!(!listed[1].present);
        assert_eq!(listed[2].role, FontRole::Fallback);
        assert!(!listed[2].present);
        assert_eq!(FontRole::UnicodeUi.as_str(), "unicode-ui");
        assert_eq!(FontRole::BijoyAnsi.as_str(), "bijoy-ansi");
        assert_eq!(FontRole::Fallback.as_str(), "fallback");
    }

    #[test]
    fn error_kinds_are_static() {
        assert_eq!(FontError::InvalidPath.kind(), "font_invalid_path");
        assert_eq!(FontError::NotFound.kind(), "font_not_found");
        assert_eq!(
            FontError::UnsupportedExtension.kind(),
            "font_unsupported_extension"
        );
        assert_eq!(FontError::Registry.kind(), "font_registry");
        assert_eq!(FontError::Win32(5).kind(), "font_win32");
        for error in [
            FontError::InvalidPath,
            FontError::NotFound,
            FontError::Registry,
            FontError::Win32(0),
            FontError::UnsupportedPlatform,
        ] {
            let text = format!("{error}");
            assert!(!text.contains("C:\\"), "leaks path: {text}");
        }
    }
}

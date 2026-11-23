//! Per-user font management for Okkhor.
//!
//! Okkhor renders Bijoy byte text with a Bijoy-compatible font (SutonnyMJ
//! where installed, Kalpurush ANSI as fallback) and Unicode text with
//! Kalpurush / Noto Sans Bengali. Fonts ship as data, never as binaries in
//! this repository: `scripts/fetch-fonts.ps1` downloads the OFL fonts and
//! stages SutonnyMJ from the owner's legacy installation into
//! `assets/fonts/` at build time.
//!
//! This module installs those files per user only: it copies into
//! `%LOCALAPPDATA%\Microsoft\Windows\Fonts`, registers under
//! `HKCU\...\Fonts`, and broadcasts `WM_FONTCHANGE`. No elevation is ever
//! needed and nothing is written outside the user's own profile.
//!
//! The registry value name comes from the font's own name table
//! ([`font_family_name`], e.g. `SutonnyMJ (TrueType)`), falling back to the
//! file stem. [`list_bundled_fonts`] reports which expected files are
//! present; it performs no logging, so user paths can never leak.
//! [`resolve_bijoy_font`] picks the Bijoy rendering font: SutonnyMJ when
//! staged, Kalpurush ANSI otherwise, or [`BijoyFontChoice::NoneAvailable`]
//! (a typed, non-fatal "missing" — the app must keep working on Unicode
//! and Kalpurush ANSI paths). [`resolve_bijoy_font_with_probe`] extends
//! this to installed fonts via [`is_font_installed`]/[`SystemFontProbe`]
//! (read-only HKCU/HKLM registry scan plus `%WINDIR%\Fonts`).
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
/// SutonnyMJ is the primary Bijoy font; Kalpurush ANSI is its fallback.
/// Both share the `bijoy-ansi` role; [`resolve_bijoy_font`] encodes the
/// preference.
pub const BUNDLED_FONTS: &[(&str, FontRole)] = &[
    ("kalpurush.ttf", FontRole::UnicodeUi),
    ("SutonnyMJ Regular.ttf", FontRole::BijoyAnsi),
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
    /// True when this session's font-table entry was removed before the
    /// file was deleted. Always false on dry runs.
    pub removed_resource: bool,
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
    /// A staged font failed length/hash verification.
    #[error("staged font failed verification")]
    HashMismatch,
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
            Self::HashMismatch => "font_hash_mismatch",
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
            | (Self::HashMismatch, Self::HashMismatch)
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

/// Expected staged bytes for bundled fonts:
/// `(file name, length in bytes, lowercase-or-uppercase SHA256 hex)`.
/// SutonnyMJ is staged from the owner's legacy installation
/// (`OKKHOR_LEGACY_HOME/Res/Fonts`); the rest are pinned upstream
/// downloads (see `scripts/fetch-fonts.ps1`).
pub const EXPECTED_FONT_HASHES: &[(&str, u64, &str)] = &[
    (
        "kalpurush.ttf",
        314592,
        "83613A03E53AAD8337F2BE9FB974409A23C1D99B3B4B494567BA9D156C492B01",
    ),
    (
        "kalpurush ANSI.ttf",
        113300,
        "BFB06B2360A558424C61405A29C24B09C70487757204814C6DD04621A2A9EEBC",
    ),
    (
        "NotoSansBengali.ttf",
        463668,
        "dcd42978094e584a849c84a51450eeac40c8826057d566ea6d4b9627a403a05a",
    ),
    (
        "SutonnyMJ Regular.ttf",
        73656,
        "D58D9A45311447301426649C641FE148010163594AE57A4094AC086B21D52BDC",
    ),
];

/// Verify raw font bytes against an expected SHA256 hex digest.
/// Used for staged build inputs; rejects mismatches with
/// [`FontError::HashMismatch`].
pub fn verify_font_hash(data: &[u8], expected_sha256_hex: &str) -> Result<(), FontError> {
    use sha2::{Digest, Sha256};

    let expected = parse_sha256_hex(expected_sha256_hex).ok_or(FontError::HashMismatch)?;
    let mut hasher = Sha256::new();
    hasher.update(data);
    if hasher.finalize().as_slice() == expected {
        Ok(())
    } else {
        Err(FontError::HashMismatch)
    }
}

/// Parse 64 hex characters into 32 bytes; `None` on any malformed input.
fn parse_sha256_hex(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(chunk).ok()?;
        out[i] = u8::from_str_radix(text, 16).ok()?;
    }
    Some(out)
}

/// Verify a staged font file by its file name against
/// [`EXPECTED_FONT_HASHES`] (byte length plus SHA256). Unknown names are
/// [`FontError::InvalidPath`]; any content mismatch is
/// [`FontError::HashMismatch`].
pub fn verify_staged_font(path: &Path) -> Result<(), FontError> {
    let file_name = path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or(FontError::InvalidPath)?;
    let (_, expected_len, expected_hex) = EXPECTED_FONT_HASHES
        .iter()
        .find(|(name, _, _)| *name == file_name)
        .ok_or(FontError::InvalidPath)?;
    let bytes = std::fs::read(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            FontError::NotFound
        } else {
            FontError::Io(error)
        }
    })?;
    if bytes.len() as u64 != *expected_len {
        return Err(FontError::HashMismatch);
    }
    verify_font_hash(&bytes, expected_hex)
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

/// Which Bijoy rendering font to use. The app must keep working when the
/// answer is [`BijoyFontChoice::NoneAvailable`] (Unicode paths and
/// Kalpurush ANSI staging do not depend on SutonnyMJ).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BijoyFontChoice {
    /// Staged `SutonnyMJ Regular.ttf`: the default Bijoy font.
    SutonnyMJ(PathBuf),
    /// Staged `kalpurush ANSI.ttf`: used when SutonnyMJ is absent.
    KalpurushAnsi(PathBuf),
    /// Neither Bijoy font is staged. Non-fatal: keep Unicode working.
    NoneAvailable,
}

impl BijoyFontChoice {
    /// The staged file to render Bijoy bytes with, if any.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::SutonnyMJ(path) | Self::KalpurushAnsi(path) => Some(path),
            Self::NoneAvailable => None,
        }
    }

    /// Stable machine-readable choice code.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SutonnyMJ(_) => "sutonnymj",
            Self::KalpurushAnsi(_) => "kalpurush-ansi",
            Self::NoneAvailable => "none-available",
        }
    }
}

/// Pick the Bijoy rendering font staged in `dir`: SutonnyMJ when present,
/// Kalpurush ANSI as fallback, [`BijoyFontChoice::NoneAvailable`] when
/// neither is staged. Pure filesystem presence check; deterministic and
/// test-safe. Prefer [`resolve_bijoy_font_with_probe`] for the full
/// contract (staged or installed).
pub fn resolve_bijoy_font(dir: &Path) -> BijoyFontChoice {
    let sutonny = dir.join("SutonnyMJ Regular.ttf");
    if sutonny.is_file() {
        return BijoyFontChoice::SutonnyMJ(sutonny);
    }
    let ansi = dir.join("kalpurush ANSI.ttf");
    if ansi.is_file() {
        return BijoyFontChoice::KalpurushAnsi(ansi);
    }
    BijoyFontChoice::NoneAvailable
}

/// Pick the Bijoy rendering font with the full contract: SutonnyMJ when
/// staged in `dir` or reported installed by `probe`, Kalpurush ANSI as
/// staged in `dir` or reported installed by `probe`, Kalpurush ANSI as
/// fallback (staged or installed), [`BijoyFontChoice::NoneAvailable`]
/// otherwise. The probe seam keeps this deterministic under test; pass
/// [`SystemFontProbe`] in production.
pub fn resolve_bijoy_font_with_probe(
    dir: &Path,
    probe: &dyn FontInstallationProbe,
) -> BijoyFontChoice {
    let sutonny = dir.join("SutonnyMJ Regular.ttf");
    if sutonny.is_file() {
        return BijoyFontChoice::SutonnyMJ(sutonny);
    }
    if let Some(path) = probe.installed_font_file("SutonnyMJ") {
        return BijoyFontChoice::SutonnyMJ(path);
    }
    let ansi = dir.join("kalpurush ANSI.ttf");
    if ansi.is_file() {
        return BijoyFontChoice::KalpurushAnsi(ansi);
    }
    if let Some(path) = probe.installed_font_file("Kalpurush ANSI") {
        return BijoyFontChoice::KalpurushAnsi(path);
    }
    BijoyFontChoice::NoneAvailable
}

/// Whether a font family is registered on this machine (HKCU then HKLM
/// `...\Fonts`, plus `%WINDIR%\Fonts` resolution). Read-only; never
/// elevates. Always false off Windows.
pub fn is_font_installed(family: &str) -> bool {
    SystemFontProbe.installed_font_file(family).is_some()
}

/// Testable source of installed-font locations. Production code uses
/// [`SystemFontProbe`]; tests inject a mock.
pub trait FontInstallationProbe {
    /// Absolute path of an installed font file for `family`, if any.
    fn installed_font_file(&self, family: &str) -> Option<PathBuf>;
}

/// Registry-backed [`FontInstallationProbe`]: scans HKCU then HKLM
/// `Software\Microsoft\Windows NT\CurrentVersion\Fonts` for a value named
/// `<family> (...)` and resolves its file against `%WINDIR%\Fonts`
/// (or an absolute path). Off Windows it reports nothing installed.
pub struct SystemFontProbe;

impl FontInstallationProbe for SystemFontProbe {
    fn installed_font_file(&self, family: &str) -> Option<PathBuf> {
        #[cfg(windows)]
        {
            scan_fonts_key(windows::Win32::System::Registry::HKEY_CURRENT_USER, family).or_else(
                || scan_fonts_key(windows::Win32::System::Registry::HKEY_LOCAL_MACHINE, family),
            )
        }
        #[cfg(not(windows))]
        {
            let _ = family;
            None
        }
    }
}

/// Whether a `Fonts`-key value name belongs to `family`
/// (`<family> (...)`, case-insensitive). Pure; shared by the install probe
/// so family matching stays testable without touching the registry.
fn font_value_belongs_to_family(value_name: &str, family: &str) -> bool {
    let mut expected = family.to_ascii_lowercase();
    expected.push_str(" (");
    value_name.to_ascii_lowercase().starts_with(&expected)
}

/// Scan one `Fonts` registry root for a value named `<family> (...)`
/// and resolve its file. Read-only; `None` on any failure.
#[cfg(windows)]
fn scan_fonts_key(root: windows::Win32::System::Registry::HKEY, family: &str) -> Option<PathBuf> {
    use windows::Win32::System::Registry::{KEY_QUERY_VALUE, RegOpenKeyExW};

    // SAFETY: the root and subkey are valid, `key` points to valid local
    // storage, and the handle is closed by `RegistryKey`. Read-only access
    // on either root: no elevation.
    let mut key = windows::Win32::System::Registry::HKEY::default();
    let status = unsafe { RegOpenKeyExW(root, FONTS_KEY_W, None, KEY_QUERY_VALUE, &mut key) };
    if status.is_err() {
        return None;
    }
    let key = RegistryKey(key);

    for entry in snapshot_font_values(key.0) {
        use windows::Win32::System::Registry::REG_SZ;
        if entry.value_type != REG_SZ.0 {
            continue;
        }
        if !font_value_belongs_to_family(&entry.name, family) {
            continue;
        }
        let file = entry.data.trim_end_matches('\0').trim().to_owned();
        if file.is_empty() {
            continue;
        }
        if let Some(path) = resolve_font_file(&file) {
            return Some(path);
        }
    }
    None
}

/// Resolve a registry font-file reference: absolute paths must exist;
/// bare file names resolve against `%WINDIR%\Fonts` and must exist.
#[cfg(windows)]
fn resolve_font_file(file: &str) -> Option<PathBuf> {
    let direct = PathBuf::from(file);
    if direct.is_absolute() {
        return direct.is_file().then_some(direct);
    }
    let windir = std::env::var_os("WINDIR")?;
    let candidate = PathBuf::from(windir).join("Fonts").join(file);
    candidate.is_file().then_some(candidate)
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

/// Remove one session font-table entry previously added with
/// [`AddFontResourceW`](windows::Win32::Graphics::Gdi::AddFontResourceW)
/// (the per-user install path). Returns whether an entry was removed;
/// false (not loaded here) is normal for uninstall, which proceeds to the
/// file and registry regardless. Dry-run seam: callers skip this under
/// [`InstallOptions::dry_run`].
#[cfg(windows)]
fn remove_session_font(path: &Path) -> bool {
    use windows::Win32::Graphics::Gdi::RemoveFontResourceW;
    use windows::core::PCWSTR;

    let wide = to_wide_null(path.as_os_str());
    // SAFETY: `wide` is a null-terminated UTF-16 buffer that outlives the
    // call; removal only affects this session's table entry for the path.
    unsafe { RemoveFontResourceW(PCWSTR(wide.as_ptr())) }.as_bool()
}

#[cfg(not(windows))]
fn remove_session_font(_path: &Path) -> bool {
    false
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
    // Remove the live session resource first (I2): deleting a file that is
    // still loaded leaves a usable-until-logoff ghost. A missing session
    // entry is not an error — uninstall must still delete file + key.
    // Dry runs skip every Win32 call.
    if !options.dry_run {
        removed.removed_resource = remove_session_font(&dest);
    }
    match std::fs::remove_file(&dest) {
        Ok(()) => removed.removed_file = true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(FontError::Io(error)),
    }
    if !options.dry_run {
        removed.removed_registration = unregister_font(name)?;
        #[cfg(windows)]
        if removed.removed_resource && !removed.removed_registration {
            // The registry entry was already gone, but a live resource was
            // removed above: still notify running apps.
            broadcast_font_change();
        }
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
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SAM_FLAGS, RegDeleteValueW,
        RegOpenKeyExW,
    };
    use windows::core::PCWSTR;

    let access = REG_SAM_FLAGS(KEY_QUERY_VALUE.0 | KEY_SET_VALUE.0);
    // SAFETY: the root and subkey are valid, `key` points to valid local
    // storage, and the handle is closed by `RegistryKey`. HKCU only.
    let mut key = HKEY::default();
    let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, FONTS_KEY_W, None, access, &mut key) };
    if status.is_err() {
        return Err(FontError::Registry);
    }
    let key = RegistryKey(key);

    // Snapshot value names first, then delete (I3): deleting while
    // enumerating forward shifts later values down and skips one entry per
    // delete, so duplicate registrations for the same file could survive.
    let entries = snapshot_font_values(key.0);
    let mut removed = false;
    for name in registration_names_for_file(&entries, file_name) {
        let wide = to_wide_null(OsStr::new(&name));
        // SAFETY: `wide` is the null-terminated snapshot of a value name
        // read from the same open key moments ago.
        let status = unsafe { RegDeleteValueW(key.0, PCWSTR(wide.as_ptr())) };
        if !status.is_err() {
            removed = true;
        }
    }
    if removed {
        broadcast_font_change();
    }
    Ok(removed)
}

/// One decoded `Fonts`-key value: (value name, value type, string data).
/// Snapshots let callers match first and mutate after (never delete while
/// enumerating).
#[cfg(windows)]
struct SnapshotFontValue {
    name: String,
    value_type: u32,
    data: String,
}

/// Enumerate every value of an open `Fonts` key into a snapshot. Short
/// buffers report an error instead of overflowing; unreadable entries are
/// skipped without aborting the scan.
#[cfg(windows)]
fn snapshot_font_values(key: windows::Win32::System::Registry::HKEY) -> Vec<SnapshotFontValue> {
    use windows::Win32::Foundation::ERROR_NO_MORE_ITEMS;
    use windows::Win32::System::Registry::RegEnumValueW;
    use windows::core::PWSTR;

    let mut entries = Vec::new();
    let mut index = 0u32;
    loop {
        let mut name_buf = vec![0u16; 256];
        let mut name_len = 256u32;
        let mut value_type = 0u32;
        let mut data_buf = vec![0u8; 1024];
        let mut data_len = 1024u32;
        // SAFETY: all out-parameters point to valid local buffers with the
        // advertised lengths; `key` is open. Errors never overflow.
        let status = unsafe {
            RegEnumValueW(
                key,
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
        name_buf.truncate(name_len as usize);
        data_buf.truncate(data_len as usize);
        entries.push(SnapshotFontValue {
            name: String::from_utf16_lossy(&name_buf),
            value_type,
            data: decode_reg_sz(&data_buf),
        });
    }
    entries
}

/// Decode REG_SZ bytes (UTF-16LE, NUL-terminated) lossily. Pure; no Win32.
fn decode_reg_sz(data: &[u8]) -> String {
    let units: Vec<u16> = data
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

/// Pure selection of registry value names whose REG_SZ data names
/// `file_name` (case-insensitive, NUL-trimmed). Two values pointing at the
/// same file both match, so duplicates never survive uninstall.
#[cfg(windows)]
fn registration_names_for_file(entries: &[SnapshotFontValue], file_name: &str) -> Vec<String> {
    use windows::Win32::System::Registry::REG_SZ;

    entries
        .iter()
        .filter(|entry| {
            entry.value_type == REG_SZ.0
                && entry
                    .data
                    .trim_end_matches('\0')
                    .eq_ignore_ascii_case(file_name)
        })
        .map(|entry| entry.name.clone())
        .collect()
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
        BUNDLED_FONTS, BijoyFontChoice, EXPECTED_FONT_HASHES, FontError, FontInstallationProbe,
        FontRole, InstallOptions, font_family_name, font_registry_value_name,
        install_font_per_user_with_options, is_font_installed, list_bundled_fonts,
        resolve_bijoy_font, resolve_bijoy_font_with_probe, uninstall_font_per_user_with_options,
        verify_font_hash, verify_staged_font,
    };
    use std::collections::HashMap;
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
            font_registry_value_name("SutonnyMJ", "ttf"),
            "SutonnyMJ (TrueType)"
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
        // Dry runs skip every Win32 call, including session removal.
        assert!(!removed.removed_resource);
        assert!(!dst.join("kalpurush.ttf").exists());

        let again = uninstall_font_per_user_with_options("kalpurush.ttf", &options)
            .expect("uninstall again");
        assert!(!again.removed_file);
        assert!(!again.removed_resource);
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
        assert_eq!(listed.len(), 4);
        assert_eq!(listed[0].file_name, "kalpurush.ttf");
        assert_eq!(listed[0].role, FontRole::UnicodeUi);
        assert!(listed[0].present);
        assert_eq!(listed[1].file_name, "SutonnyMJ Regular.ttf");
        assert_eq!(listed[1].role, FontRole::BijoyAnsi);
        assert!(!listed[1].present);
        assert_eq!(listed[2].file_name, "kalpurush ANSI.ttf");
        assert_eq!(listed[2].role, FontRole::BijoyAnsi);
        assert!(!listed[2].present);
        assert_eq!(listed[3].role, FontRole::Fallback);
        assert!(!listed[3].present);
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
        assert_eq!(FontError::HashMismatch.kind(), "font_hash_mismatch");
        assert_eq!(FontError::Registry.kind(), "font_registry");
        assert_eq!(FontError::Win32(5).kind(), "font_win32");
        for error in [
            FontError::InvalidPath,
            FontError::NotFound,
            FontError::Registry,
            FontError::Win32(0),
            FontError::HashMismatch,
            FontError::UnsupportedPlatform,
        ] {
            let text = format!("{error}");
            assert!(!text.contains("C:\\"), "leaks path: {text}");
        }
    }

    #[test]
    fn bijoy_resolution_prefers_sutonny_then_ansi() {
        let dir = temp_dir("resolve");
        // Neither staged: typed, non-fatal missing.
        let missing = resolve_bijoy_font(&dir);
        assert_eq!(missing, BijoyFontChoice::NoneAvailable);
        assert_eq!(missing.path(), None);
        assert_eq!(missing.as_str(), "none-available");

        // Only the fallback staged.
        let ansi = write_font(
            &dir,
            "kalpurush ANSI.ttf",
            &sfnt_with_family("Kalpurush ANSI"),
        );
        let fallback = resolve_bijoy_font(&dir);
        assert_eq!(fallback, BijoyFontChoice::KalpurushAnsi(ansi.clone()));
        assert_eq!(fallback.path(), Some(ansi.as_path()));
        assert_eq!(fallback.as_str(), "kalpurush-ansi");

        // SutonnyMJ staged: it wins even with the fallback present.
        let sutonny = write_font(
            &dir,
            "SutonnyMJ Regular.ttf",
            &sfnt_with_family("SutonnyMJ"),
        );
        let primary = resolve_bijoy_font(&dir);
        assert_eq!(primary, BijoyFontChoice::SutonnyMJ(sutonny.clone()));
        assert_eq!(primary.path(), Some(sutonny.as_path()));
        assert_eq!(primary.as_str(), "sutonnymj");
    }

    #[test]
    fn hash_verification_accepts_a_known_vector() {
        // SHA256("") — standard test vector.
        assert!(
            verify_font_hash(
                b"",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
            )
            .is_ok()
        );
        assert_eq!(EXPECTED_FONT_HASHES.len(), 4);
    }

    #[test]
    fn hash_verification_rejects_mismatches() {
        let zeros = "0".repeat(64);
        assert_eq!(
            verify_font_hash(b"font bytes", &zeros),
            Err(FontError::HashMismatch)
        );
        // Malformed digests (wrong length, non-hex) also reject.
        assert_eq!(
            verify_font_hash(b"", "e3b0c44"),
            Err(FontError::HashMismatch)
        );
        assert_eq!(
            verify_font_hash(b"", &"z".repeat(64)),
            Err(FontError::HashMismatch)
        );
    }

    #[test]
    fn staged_verification_rejects_wrong_content() {
        let dir = temp_dir("staged");
        // Right name, wrong bytes (length mismatch).
        let wrong = write_font(&dir, "kalpurush.ttf", b"too short");
        assert_eq!(verify_staged_font(&wrong), Err(FontError::HashMismatch));
        // Unknown names and missing files are typed, not panics.
        assert_eq!(
            verify_staged_font(&dir.join("other.ttf")),
            Err(FontError::InvalidPath)
        );
        assert_eq!(
            verify_staged_font(&dir.join("kalpurush ANSI.ttf")),
            Err(FontError::NotFound)
        );
    }

    #[test]
    fn dry_run_install_supports_display_names_with_spaces() {
        let src = temp_dir("sutonny-src");
        let dst = temp_dir("sutonny-dst");
        let options = options_for(&dst);
        let path = write_font(
            &src,
            "SutonnyMJ Regular.ttf",
            &sfnt_with_family("SutonnyMJ"),
        );
        let installed = install_font_per_user_with_options(&path, &options).expect("install");
        assert!(!installed.already_installed);
        assert_eq!(installed.path, dst.join("SutonnyMJ Regular.ttf"));

        let removed = uninstall_font_per_user_with_options("SutonnyMJ Regular.ttf", &options)
            .expect("uninstall");
        assert!(removed.removed_file);
    }

    /// In-memory [`FontInstallationProbe`] for deterministic resolution tests.
    #[derive(Default)]
    struct MockProbe {
        files: HashMap<String, PathBuf>,
    }

    impl MockProbe {
        fn with(files: &[(&str, &str)]) -> Self {
            let mut probe = Self::default();
            for (family, path) in files {
                probe
                    .files
                    .insert(family.to_ascii_lowercase(), PathBuf::from(path));
            }
            probe
        }
    }

    impl FontInstallationProbe for MockProbe {
        fn installed_font_file(&self, family: &str) -> Option<PathBuf> {
            self.files.get(&family.to_ascii_lowercase()).cloned()
        }
    }

    #[test]
    fn probe_resolution_prefers_installed_or_staged_sutonny() {
        let empty = MockProbe::default();

        // Neither staged nor installed: typed, non-fatal missing.
        let dir = temp_dir("probe-neither");
        assert_eq!(
            resolve_bijoy_font_with_probe(&dir, &empty),
            BijoyFontChoice::NoneAvailable
        );

        // Installed only (no staged file): installed SutonnyMJ wins.
        let dir = temp_dir("probe-installed");
        let installed_only = MockProbe::with(&[("SutonnyMJ", "C:\\Windows\\Fonts\\sutonnymj.ttf")]);
        assert_eq!(
            resolve_bijoy_font_with_probe(&dir, &installed_only),
            BijoyFontChoice::SutonnyMJ(PathBuf::from("C:\\Windows\\Fonts\\sutonnymj.ttf"))
        );

        // Staged only (nothing installed): staged SutonnyMJ wins.
        let dir = temp_dir("probe-staged");
        let staged = write_font(
            &dir,
            "SutonnyMJ Regular.ttf",
            &sfnt_with_family("SutonnyMJ"),
        );
        assert_eq!(
            resolve_bijoy_font_with_probe(&dir, &empty),
            BijoyFontChoice::SutonnyMJ(staged)
        );

        // Installed and staged: the staged file wins (same family).
        let installed_too = MockProbe::with(&[("SutonnyMJ", "C:\\Windows\\Fonts\\other.ttf")]);
        let staged_path = dir.join("SutonnyMJ Regular.ttf");
        assert_eq!(
            resolve_bijoy_font_with_probe(&dir, &installed_too),
            BijoyFontChoice::SutonnyMJ(staged_path)
        );

        // Kalpurush ANSI installed as fallback when SutonnyMJ is absent.
        let dir = temp_dir("probe-ansi");
        let ansi_only = MockProbe::with(&[("Kalpurush ANSI", "C:\\Windows\\Fonts\\kpan.ttf")]);
        assert_eq!(
            resolve_bijoy_font_with_probe(&dir, &ansi_only),
            BijoyFontChoice::KalpurushAnsi(PathBuf::from("C:\\Windows\\Fonts\\kpan.ttf"))
        );
    }

    #[test]
    fn installed_probe_is_case_insensitive() {
        let probe = MockProbe::with(&[("sutonnymj", "C:\\Windows\\Fonts\\s.ttf")]);
        assert_eq!(
            probe.installed_font_file("SutonnyMJ"),
            Some(PathBuf::from("C:\\Windows\\Fonts\\s.ttf"))
        );
        assert_eq!(probe.installed_font_file("Kalpurush"), None);
    }

    #[test]
    fn is_font_installed_rejects_unknown_families() {
        // Deterministic negative: no such family is ever registered.
        assert!(!is_font_installed("Okkhor No Such Font Family 12345"));
    }

    #[test]
    fn family_matching_uses_name_prefix() {
        use super::font_value_belongs_to_family;
        assert!(font_value_belongs_to_family(
            "SutonnyMJ (TrueType)",
            "SutonnyMJ"
        ));
        assert!(font_value_belongs_to_family(
            "sutonnymj (truetype)",
            "SutonnyMJ"
        ));
        assert!(font_value_belongs_to_family(
            "Kalpurush ANSI (TrueType)",
            "Kalpurush ANSI"
        ));
        // "Kalpurush (" must not swallow the ANSI sibling.
        assert!(!font_value_belongs_to_family(
            "Kalpurush ANSI (TrueType)",
            "Kalpurush"
        ));
        assert!(!font_value_belongs_to_family(
            "Kalpurush (TrueType)",
            "SutonnyMJ"
        ));
    }

    #[test]
    fn reg_sz_decodes_utf16le() {
        let bytes: Vec<u8> = "sutonnymj.ttf\0"
            .encode_utf16()
            .flat_map(|unit| unit.to_le_bytes())
            .collect();
        assert_eq!(super::decode_reg_sz(&bytes), "sutonnymj.ttf\0");
    }

    #[cfg(windows)]
    #[test]
    fn duplicate_registrations_for_one_file_all_match() {
        use super::{SnapshotFontValue, registration_names_for_file};
        use windows::Win32::System::Registry::REG_SZ;

        let entries = vec![
            SnapshotFontValue {
                name: "SutonnyMJ (TrueType)".to_owned(),
                value_type: REG_SZ.0,
                data: "SutonnyMJ Regular.ttf\0".to_owned(),
            },
            SnapshotFontValue {
                name: "SutonnyMJ (Legacy Duplicate)".to_owned(),
                value_type: REG_SZ.0,
                data: "sutonnymj regular.ttf\0".to_owned(),
            },
            SnapshotFontValue {
                name: "Kalpurush (TrueType)".to_owned(),
                value_type: REG_SZ.0,
                data: "kalpurush.ttf\0".to_owned(),
            },
            // Wrong type: must never match even with identical data.
            SnapshotFontValue {
                name: "SutonnyMJ (Binary)".to_owned(),
                value_type: 3,
                data: "SutonnyMJ Regular.ttf\0".to_owned(),
            },
        ];
        // Both values pointing at the same file match (I3 regression: the
        // old in-loop delete skipped one per delete).
        assert_eq!(
            registration_names_for_file(&entries, "SutonnyMJ Regular.ttf"),
            vec![
                "SutonnyMJ (TrueType)".to_owned(),
                "SutonnyMJ (Legacy Duplicate)".to_owned()
            ]
        );
        assert_eq!(
            registration_names_for_file(&entries, "kalpurush.ttf"),
            vec!["Kalpurush (TrueType)".to_owned()]
        );
        assert!(registration_names_for_file(&entries, "missing.ttf").is_empty());
    }
}

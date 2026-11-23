#![forbid(unsafe_code)]
//! Session-token auth (ADR 0012 §2): a 32-byte OS CSPRNG token, hex-encoded,
//! compared in constant time, stored at
//! `%LOCALAPPDATA%\Okkhor\runtime\ipc.token`.
//!
//! The daemon rotates the token on every start and removes it on clean
//! shutdown. Tests use an explicit temp directory instead of the real path
//! (pass a path, or set `OKKHOR_DATA_HOME`, which is honored as a test-only
//! override by [`default_runtime_dir`]).

use std::path::{Path, PathBuf};

/// Token length in bytes (contract: 32 random bytes).
pub const TOKEN_BYTES: usize = 32;
/// Hex-encoded token length in chars.
pub const TOKEN_HEX_CHARS: usize = TOKEN_BYTES * 2;
/// Test-only override for the data home (see [`default_runtime_dir`]).
pub const DATA_HOME_OVERRIDE_ENV: &str = "OKKHOR_DATA_HOME";

/// A 32-byte session token. Never logged or serialized with debug output.
#[derive(Clone, PartialEq, Eq)]
pub struct Token([u8; TOKEN_BYTES]);

impl Token {
    /// Generate a fresh token from the OS CSPRNG.
    pub fn generate() -> Result<Self, TokenFileError> {
        let mut bytes = [0u8; TOKEN_BYTES];
        getrandom::fill(&mut bytes).map_err(|_| TokenFileError::Entropy)?;
        Ok(Self(bytes))
    }

    /// Hex-encode for the `hello` request (64 lowercase chars).
    #[must_use]
    pub fn hex(&self) -> String {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(TOKEN_HEX_CHARS);
        for byte in self.0 {
            out.push(DIGITS[(byte >> 4) as usize] as char);
            out.push(DIGITS[(byte & 0x0F) as usize] as char);
        }
        out
    }

    /// Decode 64 hex chars back into a token.
    pub fn parse_hex(text: &str) -> Result<Self, TokenFileError> {
        let bytes = text.as_bytes();
        if bytes.len() != TOKEN_HEX_CHARS {
            return Err(TokenFileError::Malformed);
        }
        let mut out = [0u8; TOKEN_BYTES];
        for (index, chunk) in bytes.chunks_exact(2).enumerate() {
            let hi = hex_value(chunk[0]).ok_or(TokenFileError::Malformed)?;
            let lo = hex_value(chunk[1]).ok_or(TokenFileError::Malformed)?;
            out[index] = (hi << 4) | lo;
        }
        Ok(Self(out))
    }

    /// Constant-time equality. Never early-returns on content.
    #[must_use]
    pub fn constant_time_eq(&self, other: &Self) -> bool {
        let mut diff = 0u8;
        for (a, b) in self.0.iter().zip(other.0.iter()) {
            diff |= a ^ b;
        }
        diff == 0
    }

    /// Raw bytes for storage only. Prefer [`Token::hex`] on the wire.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; TOKEN_BYTES] {
        &self.0
    }
}

// Custom debug: a token must never appear in logs.
impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Token and token-file failures. No variants carry secrets or paths.
#[derive(Debug, thiserror::Error)]
pub enum TokenFileError {
    #[error("could not read OS randomness")]
    Entropy,
    #[error("token file is malformed")]
    Malformed,
    #[error("token file already exists")]
    AlreadyExists,
    #[error("token file I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Resolve `%LOCALAPPDATA%\Okkhor\runtime`.
///
/// When `OKKHOR_DATA_HOME` is set (tests only), `<override>\runtime` is used
/// instead so tests never touch the real profile.
pub fn default_runtime_dir() -> Result<PathBuf, TokenFileError> {
    let home_override = std::env::var(DATA_HOME_OVERRIDE_ENV)
        .ok()
        .filter(|value| !value.is_empty());
    let local_app_data = std::env::var("LOCALAPPDATA").ok();
    runtime_dir_with_override(home_override.as_deref(), local_app_data.as_deref())
}

/// Pure path resolution behind [`default_runtime_dir`]; directly unit-testable
/// without touching process environment.
fn runtime_dir_with_override(
    home_override: Option<&str>,
    local_app_data: Option<&str>,
) -> Result<PathBuf, TokenFileError> {
    if let Some(home) = home_override {
        return Ok(PathBuf::from(home).join("runtime"));
    }
    let local = local_app_data.ok_or_else(|| {
        TokenFileError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "LOCALAPPDATA not set",
        ))
    })?;
    Ok(PathBuf::from(local).join("Okkhor").join("runtime"))
}

/// Resolve the token file path (`ipc.token` inside the runtime dir).
pub fn default_token_path() -> Result<PathBuf, TokenFileError> {
    Ok(default_runtime_dir()?.join("ipc.token"))
}

/// Read and parse a token file.
pub fn read_token_file(path: &Path) -> Result<Token, TokenFileError> {
    let text = std::fs::read_to_string(path)?;
    Token::parse_hex(text.trim())
}

/// Write a token file with create-new semantics: fails with
/// [`TokenFileError::AlreadyExists`] when the file exists instead of
/// overwriting it. Parents are created; on Windows the file then receives an
/// owner-only DACL (0600-equivalent intent).
pub fn create_token_file(path: &Path, token: &Token) -> Result<(), TokenFileError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                TokenFileError::AlreadyExists
            } else {
                TokenFileError::Io(error)
            }
        })?;
        use std::io::Write as _;
        file.write_all(token.hex().as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
    }
    #[cfg(windows)]
    crate::sddl::apply_owner_only_dacl_to_path(path)?;
    Ok(())
}

/// Atomically replace the token file (temp file in the same directory +
/// rename), creating parents as needed. On Windows the replaced file receives
/// an owner-only DACL.
pub fn replace_token_file(path: &Path, token: &Token) -> Result<(), TokenFileError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut temp = path.as_os_str().to_owned();
    temp.push(format!(
        ".tmp-{}-{}",
        std::process::id(),
        next_temp_counter()
    ));
    let temp_path = PathBuf::from(temp);
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&temp_path).map_err(TokenFileError::Io)?;
        use std::io::Write as _;
        file.write_all(token.hex().as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
    }
    #[cfg(windows)]
    crate::sddl::apply_owner_only_dacl_to_path(&temp_path)?;
    std::fs::rename(&temp_path, path)?;
    Ok(())
}

fn next_temp_counter() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_hex_round_trip() {
        let token = Token::generate().expect("entropy");
        let hex = token.hex();
        assert_eq!(hex.len(), TOKEN_HEX_CHARS);
        assert!(
            hex.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
        let back = Token::parse_hex(&hex).expect("parses");
        assert!(token.constant_time_eq(&back));
    }

    #[test]
    fn parse_hex_rejects_bad_input() {
        assert!(Token::parse_hex("").is_err());
        assert!(Token::parse_hex(&"0".repeat(TOKEN_HEX_CHARS - 1)).is_err());
        assert!(Token::parse_hex(&"0".repeat(TOKEN_HEX_CHARS + 1)).is_err());
        assert!(Token::parse_hex(&"zz".repeat(TOKEN_BYTES)).is_err());
        // Uppercase hex is accepted (decodes identically).
        let lower = "ab".repeat(TOKEN_BYTES);
        let upper = "AB".repeat(TOKEN_BYTES);
        assert_eq!(
            Token::parse_hex(&lower).expect("lower").hex(),
            Token::parse_hex(&upper).expect("upper").hex()
        );
    }

    #[test]
    fn tokens_differ_and_compare_constant_time() {
        let a = Token::generate().expect("entropy");
        let b = Token::generate().expect("entropy");
        assert!(!a.constant_time_eq(&b));
        assert!(a.constant_time_eq(&a.clone()));
        // No two generated tokens collide in practice.
        assert_ne!(a.as_bytes(), b.as_bytes());
    }

    #[test]
    fn debug_redacts() {
        let token = Token::generate().expect("entropy");
        let rendered = format!("{token:?}");
        assert!(!rendered.contains(&token.hex()));
        assert!(rendered.contains("redacted"));
    }

    /// Unique temp dir without extra dev-dependencies.
    fn temp_dir(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "okkhor-ipc-{}-{}-{}",
            name,
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn token_file_round_trip() {
        let dir = temp_dir("token");
        let path = dir.join("runtime").join("ipc.token");
        let token = Token::generate().expect("entropy");

        create_token_file(&path, &token).expect("creates");
        let back = read_token_file(&path).expect("reads");
        assert!(token.constant_time_eq(&back));

        // Create-new refuses to overwrite.
        assert!(matches!(
            create_token_file(&path, &token),
            Err(TokenFileError::AlreadyExists)
        ));

        // Atomic replace overwrites.
        let next = Token::generate().expect("entropy");
        replace_token_file(&path, &next).expect("replaces");
        let back = read_token_file(&path).expect("reads");
        assert!(next.constant_time_eq(&back));

        // Trailing newline tolerated; garbage rejected.
        std::fs::write(&path, "not-hex\n").expect("writes garbage");
        assert!(matches!(
            read_token_file(&path),
            Err(TokenFileError::Malformed)
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn data_home_override() {
        // Override wins and appends `runtime`.
        let resolved =
            runtime_dir_with_override(Some("E:\\Test"), Some("C:\\Users\\x\\AppData\\Local"));
        assert_eq!(
            resolved.expect("override"),
            PathBuf::from("E:\\Test").join("runtime")
        );
        // Without the override, the real profile path is used.
        let resolved = runtime_dir_with_override(None, Some("C:\\Users\\x\\AppData\\Local"));
        assert_eq!(
            resolved.expect("profile"),
            PathBuf::from("C:\\Users\\x\\AppData\\Local")
                .join("Okkhor")
                .join("runtime")
        );
        // Neither set is an error.
        assert!(runtime_dir_with_override(None, None).is_err());
    }
}

use unicode_normalization::UnicodeNormalization;

/// The first code point of the Bengali block.
const BENGALI_START: char = '\u{0980}';
/// The last code point of the Bengali block.
const BENGALI_END: char = '\u{09FF}';

/// Normalize text to NFC without changing whitespace or case.
///
/// Use this for committed text and for comparisons where spacing matters.
pub fn normalize_nfc(input: &str) -> String {
    input.nfc().collect()
}

/// Normalize a lookup key: NFC, trimmed, with internal whitespace collapsed.
///
/// This is for dictionary and ranking lookups, not for text that will be
/// injected into the target application.
pub fn normalize_lookup(input: &str) -> String {
    let normalized: String = input.nfc().collect();
    let mut output = String::with_capacity(normalized.len());
    let mut pending_space = false;

    for ch in normalized.chars() {
        if ch.is_whitespace() {
            pending_space = !output.is_empty();
        } else {
            if pending_space {
                output.push(' ');
                pending_space = false;
            }
            output.push(ch);
        }
    }

    output
}

/// Whether a character is in the Bengali Unicode block.
pub const fn is_bengali_char(ch: char) -> bool {
    ch >= BENGALI_START && ch <= BENGALI_END
}

/// Whether a string contains at least one Bengali-script character.
pub fn contains_bengali(input: &str) -> bool {
    input.chars().any(is_bengali_char)
}

#[cfg(test)]
mod tests {
    use super::{contains_bengali, is_bengali_char, normalize_lookup, normalize_nfc};

    #[test]
    fn detects_bengali_letters() {
        assert!(is_bengali_char('ক'));
        assert!(contains_bengali("hello বিশ্ব"));
        assert!(!contains_bengali("hello world"));
    }

    #[test]
    fn lookup_collapses_and_trims_whitespace() {
        assert_eq!(normalize_lookup("  hello \t world \n"), "hello world");
        assert_eq!(normalize_lookup("   "), "");
    }

    #[test]
    fn nfc_is_stable_for_bengali() {
        let input = "ভালো";
        assert_eq!(normalize_nfc(input), input);
    }
}

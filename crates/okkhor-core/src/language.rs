use serde::{Deserialize, Serialize};

/// A language supported by the typing engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    English,
    Bangla,
}

impl Language {
    /// Stable code used in storage, IPC, and metrics.
    pub const fn code(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Bangla => "bn",
        }
    }

    /// Parse a stored language code.
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "en" | "eng" | "english" => Some(Self::English),
            "bn" | "bang" | "bangla" | "bengali" => Some(Self::Bangla),
            _ => None,
        }
    }

    /// Whether the language is written in Bengali script.
    pub const fn is_bengali_script(self) -> bool {
        matches!(self, Self::Bangla)
    }
}

#[cfg(test)]
mod tests {
    use super::Language;

    #[test]
    fn codes_round_trip() {
        for language in [Language::English, Language::Bangla] {
            assert_eq!(Language::from_code(language.code()), Some(language));
        }
    }

    #[test]
    fn accepts_common_aliases() {
        assert_eq!(Language::from_code("bengali"), Some(Language::Bangla));
        assert_eq!(Language::from_code("english"), Some(Language::English));
        assert_eq!(Language::from_code("fr"), None);
    }
}

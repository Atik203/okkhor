use akkhor_core::Language;
use serde::{Deserialize, Serialize};

/// How a word entered the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitSource {
    Typed,
    AcceptedSuggestion,
    Correction,
    Paste,
    Transliteration,
}

/// A privacy-preserving learning observation.
///
/// Events contain committed words and their immediate context, never raw
/// keystrokes. `previous_word` is optional so the engine can learn simple
/// bigrams without keeping a global session buffer inside Smriti.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LearningEvent {
    WordCommitted {
        word: String,
        language: Language,
        previous_word: Option<String>,
        timestamp_ms: u64,
        source: CommitSource,
    },
    SuggestionShown {
        word: String,
        language: Language,
        timestamp_ms: u64,
    },
    SuggestionAccepted {
        word: String,
        language: Language,
        previous_word: Option<String>,
        timestamp_ms: u64,
    },
    Correction {
        from: String,
        to: String,
        language: Language,
        timestamp_ms: u64,
    },
    TransliterationCommitted {
        roman_input: String,
        output: String,
        language: Language,
        previous_word: Option<String>,
        timestamp_ms: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::{CommitSource, LearningEvent};
    use akkhor_core::Language;

    #[test]
    fn events_round_trip_through_json() {
        let event = LearningEvent::WordCommitted {
            word: "ভালো".to_owned(),
            language: Language::Bangla,
            previous_word: Some("আমি".to_owned()),
            timestamp_ms: 42,
            source: CommitSource::Typed,
        };

        let json = serde_json::to_string(&event).expect("serialize event");
        let restored: LearningEvent = serde_json::from_str(&json).expect("deserialize event");
        assert_eq!(restored, event);
    }
}

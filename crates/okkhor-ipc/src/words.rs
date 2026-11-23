#![forbid(unsafe_code)]
//! User word types, validation, and CSV/JSON import/export (contract §3).
//!
//! Words are sensitive text: they are encrypted at rest by the daemon store
//! (Task 2, DPAPI). This module never logs word text. Validation rejects
//! control characters so hostile payloads cannot smuggle formatting or escape
//! sequences through the word manager.

use serde::{Deserialize, Serialize};

use crate::error::ErrorCode;

/// Total words cap (contract §3).
pub const MAX_WORDS_TOTAL: usize = 20_000;
/// Import cap per request (contract §3).
pub const MAX_IMPORT_ROWS: usize = 5_000;
/// `roman` length bounds in Unicode scalar values (contract §3).
pub const ROMAN_MIN_CHARS: usize = 1;
/// `roman` length bounds in Unicode scalar values (contract §3).
pub const ROMAN_MAX_CHARS: usize = 64;
/// `output` length bound in Unicode scalar values (contract §3).
pub const OUTPUT_MAX_CHARS: usize = 128;
/// CSV header (contract §3).
pub const CSV_HEADER: &str = "roman,output,language";

/// Word language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WordLanguage {
    Bn,
    En,
}

/// Where a word came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WordSource {
    User,
    #[default]
    Import,
    Learned,
}

/// A stored user word (contract §3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Word {
    pub id: u64,
    pub roman: String,
    pub output: String,
    pub language: WordLanguage,
    pub source: WordSource,
    pub added_at_ms: u64,
}

/// A word without daemon-assigned metadata, for add/import rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewWord {
    pub roman: String,
    pub output: String,
    pub language: WordLanguage,
    #[serde(default)]
    pub source: WordSource,
}

/// Words file envelope (contract §3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordsFile {
    pub schema: u32,
    pub words: Vec<Word>,
}

/// Word validation/import/export failures. Messages are static; they never
/// include word text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WordsError {
    #[error("unsupported schema version")]
    UnsupportedSchema,
    #[error("invalid word")]
    InvalidWord,
    #[error("duplicate word")]
    DuplicateWord,
    #[error("word cap exceeded")]
    TooManyWords,
    #[error("malformed import payload")]
    Malformed,
}

impl WordsError {
    #[must_use]
    pub const fn code(self) -> ErrorCode {
        match self {
            Self::UnsupportedSchema => ErrorCode::UnsupportedSchema,
            Self::InvalidWord => ErrorCode::InvalidWord,
            Self::DuplicateWord => ErrorCode::DuplicateWord,
            Self::TooManyWords => ErrorCode::TooManyWords,
            Self::Malformed => ErrorCode::InvalidRequest,
        }
    }
}

/// Validate `roman`: 1–64 Unicode scalar values, no control characters.
pub fn validate_roman(roman: &str) -> Result<(), WordsError> {
    let len = roman.chars().count();
    if !(ROMAN_MIN_CHARS..=ROMAN_MAX_CHARS).contains(&len) || roman.chars().any(char::is_control) {
        return Err(WordsError::InvalidWord);
    }
    Ok(())
}

/// Validate `output`: 1–128 Unicode scalar values, no control characters.
///
/// ZWJ (U+200D) and ZWNJ (U+200C) are format characters (`Cf`), not control
/// characters (`Cc`), so they pass this check as the contract requires.
pub fn validate_output(output: &str) -> Result<(), WordsError> {
    let len = output.chars().count();
    if len == 0 || len > OUTPUT_MAX_CHARS || output.chars().any(char::is_control) {
        return Err(WordsError::InvalidWord);
    }
    Ok(())
}

/// Validate one candidate word's fields.
pub fn validate_new_word(word: &NewWord) -> Result<(), WordsError> {
    validate_roman(&word.roman)?;
    validate_output(&word.output)?;
    Ok(())
}

/// Validate a stored word, including its envelope-level invariants.
pub fn validate_word(word: &Word) -> Result<(), WordsError> {
    validate_new_word(&NewWord {
        roman: word.roman.clone(),
        output: word.output.clone(),
        language: word.language,
        source: word.source,
    })
}

/// Whether `candidate` duplicates an entry in `words` on the
/// `(language, roman, output)` triple (contract §3).
pub fn is_duplicate(words: &[Word], candidate: &NewWord) -> bool {
    words.iter().any(|word| {
        word.language == candidate.language
            && word.roman == candidate.roman
            && word.output == candidate.output
    })
}

/// Check the total-words cap for adding `additional` words to `existing`.
pub fn check_total_cap(existing: usize, additional: usize) -> Result<(), WordsError> {
    if existing.saturating_add(additional) > MAX_WORDS_TOTAL {
        return Err(WordsError::TooManyWords);
    }
    Ok(())
}

/// Validate a whole file: schema, caps, field validity, intra-file duplicates.
pub fn validate_words_file(file: &WordsFile) -> Result<(), WordsError> {
    if file.schema != 1 {
        return Err(WordsError::UnsupportedSchema);
    }
    if file.words.len() > MAX_WORDS_TOTAL {
        return Err(WordsError::TooManyWords);
    }
    let mut seen = std::collections::HashSet::new();
    for word in &file.words {
        validate_word(word)?;
        let key = (word.language, word.roman.clone(), word.output.clone());
        if !seen.insert(key) {
            return Err(WordsError::DuplicateWord);
        }
    }
    Ok(())
}

/// One parsed CSV record.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CsvRecord {
    roman: String,
    output: String,
    language: String,
}

/// Parse CSV per RFC 4180: quoted fields, embedded commas/quotes/CRLF, and a
/// UTF-8 BOM. Returns the raw records (header excluded).
fn parse_csv_records(data: &str) -> Result<Vec<CsvRecord>, WordsError> {
    let text = data.strip_prefix('\u{FEFF}').unwrap_or(data);
    let chars: Vec<char> = text.chars().collect();
    let mut records: Vec<Vec<String>> = Vec::new();
    let mut field = String::new();
    let mut record: Vec<String> = Vec::new();
    let mut in_quotes = false;
    let mut index = 0;
    let mut has_data = false;
    // Tracks whether the current record has any content, so blank lines can
    // be skipped without counting them as rows.
    let mut record_has_content = false;

    macro_rules! end_field {
        () => {
            record.push(std::mem::take(&mut field));
        };
    }
    macro_rules! end_record {
        () => {
            end_field!();
            if record_has_content {
                records.push(std::mem::take(&mut record));
                record_has_content = false;
            } else {
                record.clear();
            }
        };
    }

    while index < chars.len() {
        let char = chars[index];
        has_data = true;
        if in_quotes {
            if char == '"' {
                if chars.get(index + 1) == Some(&'"') {
                    field.push('"');
                    index += 1;
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(char);
            }
        } else if char == '"' {
            if field.is_empty() {
                in_quotes = true;
            } else {
                // A quote mid-field is malformed.
                return Err(WordsError::Malformed);
            }
        } else if char == ',' {
            end_field!();
        } else if char == '\r' {
            if chars.get(index + 1) == Some(&'\n') {
                index += 1;
            }
            end_record!();
        } else if char == '\n' {
            end_record!();
        } else {
            field.push(char);
        }
        if char != '\r' && char != '\n' {
            record_has_content = true;
        }
        index += 1;
    }
    if in_quotes {
        return Err(WordsError::Malformed);
    }
    if has_data {
        end_field!();
        if record_has_content {
            records.push(std::mem::take(&mut record));
        }
    }
    // Drop a single trailing empty record from a final newline: handled by
    // `record_has_content`, which stays false after the last line break.
    let mut out = Vec::with_capacity(records.len());
    for fields in records {
        if fields.len() != 3 {
            return Err(WordsError::Malformed);
        }
        let mut iter = fields.into_iter();
        out.push(CsvRecord {
            roman: iter.next().unwrap_or_default(),
            output: iter.next().unwrap_or_default(),
            language: iter.next().unwrap_or_default(),
        });
    }
    Ok(out)
}

fn parse_language(text: &str) -> Result<WordLanguage, WordsError> {
    match text {
        "bn" => Ok(WordLanguage::Bn),
        "en" => Ok(WordLanguage::En),
        _ => Err(WordsError::InvalidWord),
    }
}

/// Result of parsing an import payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedImport {
    /// Rows that passed validation (deduped within the payload).
    pub words: Vec<NewWord>,
    /// Rows skipped: blank-excluded malformed rows, invalid fields, or
    /// intra-payload duplicates.
    pub skipped: usize,
}

/// Parse a CSV import: header `roman,output,language`, then rows.
/// Enforces the per-request import cap ([`MAX_IMPORT_ROWS`]).
pub fn parse_words_csv(data: &str) -> Result<ParsedImport, WordsError> {
    let mut records = parse_csv_records(data)?;
    if records.is_empty() {
        return Err(WordsError::Malformed);
    }
    let header = records.remove(0);
    if header.roman != "roman" || header.output != "output" || header.language != "language" {
        return Err(WordsError::Malformed);
    }
    if records.len() > MAX_IMPORT_ROWS {
        return Err(WordsError::TooManyWords);
    }
    let mut words = Vec::new();
    let mut skipped = 0;
    let mut seen = std::collections::HashSet::new();
    for record in records {
        let language = match parse_language(record.language.trim()) {
            Ok(language) => language,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        let candidate = NewWord {
            roman: record.roman,
            output: record.output,
            language,
            source: WordSource::Import,
        };
        if validate_new_word(&candidate).is_err() {
            skipped += 1;
            continue;
        }
        let key = (
            candidate.language,
            candidate.roman.clone(),
            candidate.output.clone(),
        );
        if !seen.insert(key) {
            skipped += 1;
            continue;
        }
        words.push(candidate);
    }
    Ok(ParsedImport { words, skipped })
}

/// Import-row JSON: the envelope shape without `id`/`added_at_ms`.
#[derive(Debug, Clone, Deserialize)]
struct ImportEntry {
    roman: String,
    output: String,
    language: WordLanguage,
    #[serde(default)]
    source: WordSource,
}

/// Parse a JSON import: either the envelope `{"schema":1,"words":[...]}` or a
/// bare array of rows, both without `id`/`added_at_ms`.
pub fn parse_words_json(data: &str) -> Result<ParsedImport, WordsError> {
    #[derive(Debug, Deserialize)]
    struct Envelope {
        schema: u32,
        words: Vec<ImportEntry>,
    }
    let trimmed = data.strip_prefix('\u{FEFF}').unwrap_or(data);
    let entries: Vec<ImportEntry> = if let Ok(envelope) = serde_json::from_str::<Envelope>(trimmed)
    {
        if envelope.schema != 1 {
            return Err(WordsError::UnsupportedSchema);
        }
        envelope.words
    } else if let Ok(rows) = serde_json::from_str::<Vec<ImportEntry>>(trimmed) {
        rows
    } else {
        return Err(WordsError::Malformed);
    };
    if entries.len() > MAX_IMPORT_ROWS {
        return Err(WordsError::TooManyWords);
    }
    let mut words = Vec::new();
    let mut skipped = 0;
    let mut seen = std::collections::HashSet::new();
    for entry in entries {
        let candidate = NewWord {
            roman: entry.roman,
            output: entry.output,
            language: entry.language,
            source: entry.source,
        };
        if validate_new_word(&candidate).is_err() {
            skipped += 1;
            continue;
        }
        let key = (
            candidate.language,
            candidate.roman.clone(),
            candidate.output.clone(),
        );
        if !seen.insert(key) {
            skipped += 1;
            continue;
        }
        words.push(candidate);
    }
    Ok(ParsedImport { words, skipped })
}

fn csv_escape(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_owned()
    }
}

/// Serialize words to CSV with the contract header.
#[must_use]
pub fn words_to_csv(words: &[Word]) -> String {
    let mut out = String::from(CSV_HEADER);
    out.push_str("\r\n");
    for word in words {
        let language = match word.language {
            WordLanguage::Bn => "bn",
            WordLanguage::En => "en",
        };
        out.push_str(&csv_escape(&word.roman));
        out.push(',');
        out.push_str(&csv_escape(&word.output));
        out.push(',');
        out.push_str(language);
        out.push_str("\r\n");
    }
    out
}

/// Serialize words to the JSON envelope (contract §3).
pub fn words_to_json(words: &[Word]) -> Result<String, WordsError> {
    let file = WordsFile {
        schema: 1,
        words: words.to_vec(),
    };
    serde_json::to_string(&file).map_err(|_| WordsError::InvalidWord)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(roman: &str, output: &str) -> NewWord {
        NewWord {
            roman: roman.to_owned(),
            output: output.to_owned(),
            language: WordLanguage::Bn,
            source: WordSource::User,
        }
    }

    #[test]
    fn field_bounds() {
        assert_eq!(validate_roman(""), Err(WordsError::InvalidWord));
        assert_eq!(validate_roman(&"a".repeat(64)), Ok(()));
        assert_eq!(
            validate_roman(&"a".repeat(65)),
            Err(WordsError::InvalidWord)
        );
        assert_eq!(validate_roman("a\tb"), Err(WordsError::InvalidWord));
        assert_eq!(validate_roman("a\nb"), Err(WordsError::InvalidWord));
        // Bengali output with ZWJ/ZWNJ is allowed.
        assert_eq!(validate_output("ক্\u{200D}ষ"), Ok(()));
        assert_eq!(validate_output("ক্\u{200C}ষ"), Ok(()));
        assert_eq!(validate_output(""), Err(WordsError::InvalidWord));
        assert_eq!(validate_output(&"অ".repeat(128)), Ok(()));
        assert_eq!(
            validate_output(&"অ".repeat(129)),
            Err(WordsError::InvalidWord)
        );
        assert_eq!(validate_output("a\u{0}b"), Err(WordsError::InvalidWord));
        assert_eq!(validate_output("a\u{7F}b"), Err(WordsError::InvalidWord));
    }

    #[test]
    fn duplicates_and_caps() {
        let stored = vec![Word {
            id: 1,
            roman: "amar".to_owned(),
            output: "আমার".to_owned(),
            language: WordLanguage::Bn,
            source: WordSource::User,
            added_at_ms: 1,
        }];
        assert!(is_duplicate(&stored, &word("amar", "আমার")));
        assert!(!is_duplicate(&stored, &word("amar", "আমারx")));
        assert!(!is_duplicate(
            &stored,
            &NewWord {
                language: WordLanguage::En,
                ..word("amar", "আমার")
            }
        ));
        assert_eq!(check_total_cap(19_999, 1), Ok(()));
        assert_eq!(check_total_cap(20_000, 1), Err(WordsError::TooManyWords));
    }

    #[test]
    fn csv_round_trip_quoted_crlf_bom() {
        let words = vec![
            Word {
                id: 1,
                roman: "a,b".to_owned(),
                output: "x\"y,z".to_owned(),
                language: WordLanguage::Bn,
                source: WordSource::User,
                added_at_ms: 1,
            },
            Word {
                id: 2,
                roman: "line".to_owned(),
                output: "ঠিক".to_owned(),
                language: WordLanguage::En,
                source: WordSource::Import,
                added_at_ms: 2,
            },
        ];
        let csv = words_to_csv(&words);
        assert!(csv.starts_with("roman,output,language\r\n"));
        let mut with_bom = String::from("\u{FEFF}");
        with_bom.push_str(&csv);
        let parsed = parse_words_csv(&with_bom).expect("parses with BOM");
        assert_eq!(parsed.skipped, 0);
        assert_eq!(parsed.words.len(), 2);
        assert_eq!(parsed.words[0].roman, "a,b");
        assert_eq!(parsed.words[0].output, "x\"y,z");
        assert_eq!(parsed.words[1].output, "ঠিক");
        assert_eq!(parsed.words[1].language, WordLanguage::En);
    }

    #[test]
    fn csv_quoted_newline_does_not_desync_rows() {
        // A quoted field containing CRLF parses as one record; its control
        // character is rejected by validation (counted as skipped), and the
        // rows around it still land correctly.
        let parsed = parse_words_csv(
            "roman,output,language\r\nok,ঠিক,bn\r\n\"a\r\nb\",x,bn\r\nafter,পরে,bn\r\n",
        )
        .expect("parses");
        assert_eq!(parsed.words.len(), 2);
        assert_eq!(parsed.words[0].roman, "ok");
        assert_eq!(parsed.words[1].roman, "after");
        assert_eq!(parsed.skipped, 1);
    }

    #[test]
    fn csv_rejects_bad_header_and_control_chars() {
        assert_eq!(
            parse_words_csv("roman,output\r\na,b,bn\r\n"),
            Err(WordsError::Malformed)
        );
        assert_eq!(parse_words_csv(""), Err(WordsError::Malformed));
        // Control characters in fields are rejected (counted as skipped).
        let parsed = parse_words_csv("roman,output,language\r\na,b\u{1}c,bn\r\nok,ঠিক,bn\r\n")
            .expect("parses");
        assert_eq!(parsed.words.len(), 1);
        assert_eq!(parsed.skipped, 1);
    }

    #[test]
    fn csv_counts_duplicates_and_bad_rows_as_skipped() {
        let parsed = parse_words_csv(
            "roman,output,language\r\namar,আমার,bn\r\namar,আমার,bn\r\nbad,,xx\r\n\r\nvalo,ভালো,bn\r\n",
        )
        .expect("parses");
        assert_eq!(parsed.words.len(), 2);
        assert_eq!(parsed.skipped, 2);
    }

    #[test]
    fn import_cap_enforced() {
        let mut big = String::from("roman,output,language\r\n");
        for index in 0..=MAX_IMPORT_ROWS {
            big.push_str(&format!("r{index},o{index},bn\r\n"));
        }
        assert_eq!(parse_words_csv(&big), Err(WordsError::TooManyWords));

        let rows: Vec<String> = (0..=MAX_IMPORT_ROWS)
            .map(|index| {
                format!("{{\"roman\":\"r{index}\",\"output\":\"o{index}\",\"language\":\"bn\"}}")
            })
            .collect();
        let payload = format!("[{}]", rows.join(","));
        assert_eq!(parse_words_json(&payload), Err(WordsError::TooManyWords));
    }

    #[test]
    fn json_round_trip_envelope_and_array() {
        let words = vec![Word {
            id: 7,
            roman: "amar".to_owned(),
            output: "আমার".to_owned(),
            language: WordLanguage::Bn,
            source: WordSource::User,
            added_at_ms: 1_763_900_000_000,
        }];
        let json = words_to_json(&words).expect("serializes");
        let back: WordsFile = serde_json::from_str(&json).expect("parses");
        assert_eq!(back.schema, 1);
        assert_eq!(back.words, words);

        // Envelope import without id/added_at_ms.
        let parsed = parse_words_json(
            r#"{"schema":1,"words":[{"roman":"amar","output":"আমার","language":"bn"}]}"#,
        )
        .expect("envelope parses");
        assert_eq!(parsed.words.len(), 1);
        assert_eq!(parsed.words[0].source, WordSource::Import);

        // Bare array import.
        let parsed = parse_words_json(r#"[{"roman":"ami","output":"আমি","language":"bn"}]"#)
            .expect("array parses");
        assert_eq!(parsed.words.len(), 1);

        assert_eq!(
            parse_words_json(r#"{"schema":2,"words":[]}"#),
            Err(WordsError::UnsupportedSchema)
        );
        assert_eq!(parse_words_json("nope"), Err(WordsError::Malformed));
    }

    #[test]
    fn file_validation_rejects_schema_cap_and_dupes() {
        let make = |roman: &str| Word {
            id: 1,
            roman: roman.to_owned(),
            output: "আমার".to_owned(),
            language: WordLanguage::Bn,
            source: WordSource::User,
            added_at_ms: 1,
        };
        assert_eq!(
            validate_words_file(&WordsFile {
                schema: 2,
                words: vec![]
            }),
            Err(WordsError::UnsupportedSchema)
        );
        assert_eq!(
            validate_words_file(&WordsFile {
                schema: 1,
                words: vec![make("a"), make("a")]
            }),
            Err(WordsError::DuplicateWord)
        );
        assert_eq!(
            validate_words_file(&WordsFile {
                schema: 1,
                words: vec![make("")]
            }),
            Err(WordsError::InvalidWord)
        );
    }

    #[test]
    fn words_error_codes() {
        assert_eq!(
            WordsError::UnsupportedSchema.code(),
            ErrorCode::UnsupportedSchema
        );
        assert_eq!(WordsError::InvalidWord.code(), ErrorCode::InvalidWord);
        assert_eq!(WordsError::DuplicateWord.code(), ErrorCode::DuplicateWord);
        assert_eq!(WordsError::TooManyWords.code(), ErrorCode::TooManyWords);
    }
}

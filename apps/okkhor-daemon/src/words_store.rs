//! Encrypted user-word store (contract §3).
//!
//! Words live only in the encrypted repository at `data/words.bin` (DPAPI,
//! current-user scope). The store assigns monotonically increasing ids,
//! enforces the 20,000-word cap and per-request import cap, rejects
//! duplicates on `(language, roman, output)`, and serves search/pagination.
//! Import accepts CSV (header `roman,output,language`) or the JSON envelope;
//! export produces either format, bounded to one frame.
//!
//! Errors and debug output are static and carry no word text: [`WordsStore`]
//! has a manual `Debug` impl that reports counts only.

use std::collections::HashSet;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use okkhor_ipc::MAX_FRAME_BYTES;
use okkhor_ipc::error::ErrorCode;
use okkhor_ipc::protocol::{
    ImportFormat, WordAdd, WordsExported, WordsImported, WordsPage, normalize_words_list_limit,
};
use okkhor_ipc::words::{
    MAX_IMPORT_ROWS, MAX_WORDS_TOTAL, NewWord, Word, WordLanguage, WordSource, WordsError,
    WordsFile, check_total_cap, is_duplicate, parse_words_csv, parse_words_json, validate_new_word,
    validate_words_file, words_to_csv, words_to_json,
};
use okkhor_storage::{EncryptedJsonRepository, Protector, StorageError};
use thiserror::Error;

/// Fixed, non-secret scope label for the words blob. It is additional
/// authenticated data for DPAPI, not a key: it binds the blob to this store
/// so a blob from another Okkhor file cannot be swapped in.
pub const WORDS_ENTROPY: &[u8] = b"Okkhor/words.bin/v1";

/// Word store failures. Messages are static; they never include word text,
/// file content, or paths.
#[derive(Debug, Error)]
pub enum WordsStoreError {
    /// Repository failure (I/O, corruption, platform, size). The display is
    /// the redacted [`StorageError`] message.
    #[error("word store storage failed")]
    Storage(#[from] StorageError),
    /// Words file `schema` is not 1.
    #[error("unsupported schema version")]
    UnsupportedSchema,
    /// A word field failed validation.
    #[error("invalid word")]
    InvalidWord,
    /// A `(language, roman, output)` triple already exists.
    #[error("duplicate word")]
    DuplicateWord,
    /// The 20,000-word cap or the 5,000-row import cap would be exceeded.
    #[error("word cap exceeded")]
    TooManyWords,
    /// The import payload is not valid CSV/JSON.
    #[error("malformed import payload")]
    Malformed,
    /// The export would exceed the 1 MiB frame limit.
    #[error("export exceeds the size limit")]
    ExportTooLarge,
    /// The data home could not be resolved.
    #[error("data home is not available")]
    NoDataHome,
    /// Unreachable-in-practice serialization failure.
    #[error("word store failed")]
    Internal,
}

impl From<WordsError> for WordsStoreError {
    fn from(error: WordsError) -> Self {
        match error {
            WordsError::UnsupportedSchema => Self::UnsupportedSchema,
            WordsError::InvalidWord => Self::InvalidWord,
            WordsError::DuplicateWord => Self::DuplicateWord,
            WordsError::TooManyWords => Self::TooManyWords,
            WordsError::Malformed => Self::Malformed,
        }
    }
}

impl WordsStoreError {
    /// Protocol code for the IPC layer (Task 3).
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::Storage(StorageError::Unsupported) => ErrorCode::Unsupported,
            Self::Storage(_) => ErrorCode::Internal,
            Self::UnsupportedSchema => ErrorCode::UnsupportedSchema,
            Self::InvalidWord => ErrorCode::InvalidWord,
            Self::DuplicateWord => ErrorCode::DuplicateWord,
            Self::TooManyWords => ErrorCode::TooManyWords,
            Self::Malformed => ErrorCode::InvalidRequest,
            Self::ExportTooLarge => ErrorCode::ExportTooLarge,
            Self::NoDataHome | Self::Internal => ErrorCode::Internal,
        }
    }
}

/// Production protector: DPAPI current-user scope bound to [`WORDS_ENTROPY`].
#[derive(Debug, Clone, Copy)]
pub struct DpapiProtector {
    entropy: &'static [u8],
}

impl DpapiProtector {
    /// Protector for the user-word store.
    #[must_use]
    pub const fn words() -> Self {
        Self {
            entropy: WORDS_ENTROPY,
        }
    }
}

impl Protector for DpapiProtector {
    fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, StorageError> {
        okkhor_win::dpapi::protect(plaintext, self.entropy).map_err(|error| {
            use okkhor_win::dpapi::DpapiError;
            match error {
                DpapiError::TooLarge => StorageError::TooLarge,
                DpapiError::Unsupported => StorageError::Unsupported,
                DpapiError::ProtectFailed(source) => StorageError::Protect(source),
                DpapiError::UnprotectFailed => StorageError::Corrupt,
            }
        })
    }

    fn unprotect(&self, ciphertext: &[u8]) -> Result<Vec<u8>, StorageError> {
        okkhor_win::dpapi::unprotect(ciphertext, self.entropy).map_err(|error| {
            use okkhor_win::dpapi::DpapiError;
            match error {
                DpapiError::TooLarge => StorageError::TooLarge,
                DpapiError::Unsupported => StorageError::Unsupported,
                // Wrong entropy and tampered blobs are indistinguishable by
                // design: both fail closed as corruption.
                DpapiError::ProtectFailed(_) | DpapiError::UnprotectFailed => StorageError::Corrupt,
            }
        })
    }
}

/// In-memory words plus the encrypted repository they persist to.
///
/// Invariants (checked on open, upheld by every mutator): file schema is 1,
/// every word validates, no duplicate triple, `words.len() <= 20,000`, and
/// `next_id` exceeds every stored id. Mutators persist atomically and roll
/// back the in-memory state when persistence fails, so memory and disk never
/// diverge silently.
pub struct WordsStore<P> {
    repository: EncryptedJsonRepository<WordsFile, P>,
    file: WordsFile,
    next_id: u64,
}

// Manual `Debug`: word text must never appear in debug output or logs.
impl<P> std::fmt::Debug for WordsStore<P> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WordsStore")
            .field("path", &self.repository.path())
            .field("words", &self.file.words.len())
            .field("next_id", &self.next_id)
            .finish()
    }
}

impl<P: Protector> WordsStore<P> {
    /// Open `path`, creating an empty encrypted store when the file is
    /// missing. A present file is fully validated; anything invalid fails
    /// with a typed error and is left untouched.
    pub fn open(path: &Path, protector: P) -> Result<Self, WordsStoreError> {
        let repository = EncryptedJsonRepository::new(path, protector);
        let missing = match repository.load() {
            Ok(file) => {
                validate_words_file(&file)?;
                return Self::checked(repository, file);
            }
            Err(StorageError::NotFound) => true,
            Err(error) => return Err(error.into()),
        };
        debug_assert!(missing);
        let file = WordsFile {
            schema: 1,
            words: Vec::new(),
        };
        let store = Self::checked(repository, file)?;
        store.persist()?;
        Ok(store)
    }

    /// Open from the default location (`<home>/data/words.bin`).
    pub fn open_default(protector: P) -> Result<Self, WordsStoreError> {
        let path = crate::paths::words_path().map_err(|_| WordsStoreError::NoDataHome)?;
        Self::open(&path, protector)
    }

    /// File this store persists to.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.repository.path()
    }

    /// Number of stored words.
    #[must_use]
    pub fn len(&self) -> usize {
        self.file.words.len()
    }

    /// Whether the store holds no words.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.file.words.is_empty()
    }

    /// Add one user word, assigning the next id. Rejects invalid fields,
    /// duplicates, and over-cap adds without persisting anything.
    pub fn add(&mut self, request: &WordAdd) -> Result<Word, WordsStoreError> {
        let candidate = NewWord {
            roman: request.roman.clone(),
            output: request.output.clone(),
            language: request.language,
            source: WordSource::User,
        };
        validate_new_word(&candidate)?;
        if is_duplicate(&self.file.words, &candidate) {
            return Err(WordsStoreError::DuplicateWord);
        }
        check_total_cap(self.file.words.len(), 1)?;
        let word = Word {
            id: self.next_id,
            roman: candidate.roman,
            output: candidate.output,
            language: candidate.language,
            source: candidate.source,
            added_at_ms: now_ms(),
        };
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(WordsStoreError::TooManyWords)?;
        self.file.words.push(word.clone());
        if let Err(error) = self.persist() {
            self.file.words.pop();
            self.next_id -= 1;
            return Err(error);
        }
        Ok(word)
    }

    /// Remove the word with `id`. Returns `false` (without writing) when no
    /// such word exists.
    pub fn remove(&mut self, id: u64) -> Result<bool, WordsStoreError> {
        let Some(index) = self.file.words.iter().position(|word| word.id == id) else {
            return Ok(false);
        };
        let removed = self.file.words.remove(index);
        if let Err(error) = self.persist() {
            self.file.words.insert(index, removed);
            return Err(error);
        }
        Ok(true)
    }

    /// List words with substring search and pagination (contract §4).
    ///
    /// `query` matches `roman` or `output` case-insensitively; `None`/empty
    /// matches everything. `limit` is clamped to 200; `offset` past the end
    /// yields an empty page with the true total.
    #[must_use]
    pub fn list(&self, query: Option<&str>, offset: u64, limit: u64) -> WordsPage {
        let limit = normalize_words_list_limit(limit) as usize;
        let offset = usize::try_from(offset).unwrap_or(usize::MAX);
        let needle = query
            .map(str::to_lowercase)
            .filter(|needle| !needle.is_empty());
        let mut selected: Vec<&Word> = Vec::new();
        for word in &self.file.words {
            let matches = match &needle {
                None => true,
                Some(needle) => {
                    word.roman.to_lowercase().contains(needle.as_str())
                        || word.output.to_lowercase().contains(needle.as_str())
                }
            };
            if matches {
                selected.push(word);
            }
        }
        let total = selected.len() as u64;
        let words = selected
            .into_iter()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect();
        WordsPage { total, words }
    }

    /// Import words from a CSV or JSON payload (contract §3).
    ///
    /// The parser enforces the 5,000-row import cap; rows that are invalid or
    /// duplicate (within the payload or against the store) are counted as
    /// skipped. The total-words cap is enforced before any mutation, so an
    /// over-cap import adds nothing.
    pub fn import(
        &mut self,
        format: ImportFormat,
        data: &str,
    ) -> Result<WordsImported, WordsStoreError> {
        if data.len() > MAX_FRAME_BYTES {
            return Err(WordsStoreError::Malformed);
        }
        let parsed = match format {
            ImportFormat::Csv => parse_words_csv(data)?,
            ImportFormat::Json => parse_words_json(data)?,
        };
        let mut skipped = parsed.skipped;
        let mut seen: HashSet<(WordLanguage, String, String)> = HashSet::new();
        let mut accepted: Vec<NewWord> = Vec::new();
        for candidate in parsed.words {
            if is_duplicate(&self.file.words, &candidate) || validate_new_word(&candidate).is_err()
            {
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
            accepted.push(candidate);
        }
        check_total_cap(self.file.words.len(), accepted.len())?;
        debug_assert!(self.file.words.len() + accepted.len() <= MAX_WORDS_TOTAL);
        if accepted.len() > MAX_IMPORT_ROWS {
            // Unreachable: the parser already caps rows per request, and
            // filtering only shrinks the set. Fail closed if that changes.
            return Err(WordsStoreError::TooManyWords);
        }

        let base_len = self.file.words.len();
        let base_next_id = self.next_id;
        // Reserve the id block up front: the increments below cannot
        // overflow, and a failed persist rolls everything back.
        self.next_id
            .checked_add(accepted.len() as u64)
            .ok_or(WordsStoreError::TooManyWords)?;
        for candidate in accepted {
            let id = self.next_id;
            self.next_id += 1;
            self.file.words.push(Word {
                id,
                roman: candidate.roman,
                output: candidate.output,
                language: candidate.language,
                source: candidate.source,
                added_at_ms: now_ms(),
            });
        }
        if let Err(error) = self.persist() {
            self.file.words.truncate(base_len);
            self.next_id = base_next_id;
            return Err(error);
        }
        Ok(WordsImported {
            imported: self.file.words.len() - base_len,
            skipped,
        })
    }

    /// Export all words in `format`, bounded to one 1 MiB frame.
    pub fn export(&self, format: ImportFormat) -> Result<WordsExported, WordsStoreError> {
        let data = match format {
            ImportFormat::Csv => words_to_csv(&self.file.words),
            ImportFormat::Json => {
                words_to_json(&self.file.words).map_err(|_| WordsStoreError::Internal)?
            }
        };
        if data.len() > MAX_FRAME_BYTES {
            return Err(WordsStoreError::ExportTooLarge);
        }
        Ok(WordsExported { format, data })
    }

    fn checked(
        repository: EncryptedJsonRepository<WordsFile, P>,
        file: WordsFile,
    ) -> Result<Self, WordsStoreError> {
        // `next_id` always exceeds every stored id. An empty store starts
        // at 0; `u64::MAX` as a stored id exhausts the space and refuses to
        // open rather than aliasing ids.
        let next_id = match file.words.iter().map(|word| word.id).max() {
            None => 0,
            Some(max) => max.checked_add(1).ok_or(WordsStoreError::TooManyWords)?,
        };
        Ok(Self {
            repository,
            file,
            next_id,
        })
    }

    fn persist(&self) -> Result<(), WordsStoreError> {
        self.repository.save(&self.file)?;
        Ok(())
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use okkhor_ipc::words::{WordLanguage, WordSource};
    use okkhor_storage::FakeProtector;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "okkhor-words-{}-{}-{}-{}",
            name,
            std::process::id(),
            unique,
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn open_store(dir: &std::path::Path, name: &str) -> WordsStore<FakeProtector> {
        WordsStore::open(&dir.join(name), FakeProtector).expect("opens")
    }

    fn add(store: &mut WordsStore<FakeProtector>, roman: &str, output: &str) -> Word {
        store
            .add(&WordAdd {
                roman: roman.to_owned(),
                output: output.to_owned(),
                language: WordLanguage::Bn,
            })
            .expect("adds")
    }

    #[test]
    fn missing_file_opens_empty_and_persists() {
        let dir = temp_dir("empty");
        let path = dir.join("words.bin");
        let store = open_store(&dir, "words.bin");
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
        assert!(path.exists(), "empty store is persisted");

        // Reload sees the same empty store with ids starting at 0... first
        // assigned id is 0.
        let mut again = open_store(&dir, "words.bin");
        let word = add(&mut again, "amar", "আমার");
        assert_eq!(word.id, 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn add_assigns_ids_and_reload_continues() {
        let dir = temp_dir("ids");
        let mut store = open_store(&dir, "words.bin");
        let first = add(&mut store, "amar", "আমার");
        let second = add(&mut store, "ami", "আমি");
        assert_eq!((first.id, second.id), (0, 1));

        let mut again = open_store(&dir, "words.bin");
        let third = add(&mut again, "tumi", "তুমি");
        assert_eq!(third.id, 2, "ids survive reload");
        assert_eq!(again.len(), 3);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn duplicate_is_rejected_without_side_effects() {
        let dir = temp_dir("dupe");
        let mut store = open_store(&dir, "words.bin");
        add(&mut store, "amar", "আমার");
        let before = std::fs::read(store.path()).expect("reads file");

        let error = store
            .add(&WordAdd {
                roman: "amar".to_owned(),
                output: "আমার".to_owned(),
                language: WordLanguage::Bn,
            })
            .expect_err("duplicate");
        assert!(matches!(error, WordsStoreError::DuplicateWord));
        assert_eq!(error.code(), ErrorCode::DuplicateWord);
        assert_eq!(store.len(), 1);
        assert_eq!(std::fs::read(store.path()).expect("untouched"), before);

        // Same roman/output in the other language is a different word.
        let other = store
            .add(&WordAdd {
                roman: "amar".to_owned(),
                output: "আমার".to_owned(),
                language: WordLanguage::En,
            })
            .expect("other language");
        assert_eq!(other.language, WordLanguage::En);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_words_are_rejected() {
        let dir = temp_dir("invalid");
        let mut store = open_store(&dir, "words.bin");
        for (roman, output) in [("", "আমার"), ("amar", ""), ("a\tb", "আমার")] {
            let error = store
                .add(&WordAdd {
                    roman: roman.to_owned(),
                    output: output.to_owned(),
                    language: WordLanguage::Bn,
                })
                .expect_err("invalid");
            assert!(matches!(error, WordsStoreError::InvalidWord), "{roman:?}");
        }
        assert!(store.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn remove_deletes_and_reports_missing() {
        let dir = temp_dir("remove");
        let mut store = open_store(&dir, "words.bin");
        let word = add(&mut store, "amar", "আমার");
        assert!(store.remove(word.id).expect("removes"));
        assert!(store.is_empty());
        assert!(!store.remove(word.id).expect("missing reports false"));
        assert!(!store.remove(u64::MAX).expect("missing reports false"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn search_and_pagination() {
        let dir = temp_dir("list");
        let mut store = open_store(&dir, "words.bin");
        add(&mut store, "amar", "আমার");
        add(&mut store, "ami", "আমি");
        add(&mut store, "tumi", "তুমি");
        store
            .add(&WordAdd {
                roman: "hello".to_owned(),
                output: "hello".to_owned(),
                language: WordLanguage::En,
            })
            .expect("adds english");

        // Empty/None query returns everything.
        assert_eq!(store.list(None, 0, 50).total, 4);
        assert_eq!(store.list(Some(""), 0, 50).total, 4);

        // Substring, case-insensitive on roman.
        let page = store.list(Some("AM"), 0, 50);
        assert_eq!(page.total, 2);

        // Output search (Bengali has no case; English matches either way).
        assert_eq!(store.list(Some("তুমি"), 0, 50).total, 1);
        assert_eq!(store.list(Some("HELLO"), 0, 50).total, 1);

        // Pagination slices but keeps the total.
        let page = store.list(None, 1, 2);
        assert_eq!(page.total, 4);
        assert_eq!(page.words.len(), 2);
        assert_eq!(page.words[0].roman, "ami");

        // Offset past the end is an empty page, not an error.
        let page = store.list(None, 99, 50);
        assert_eq!((page.total, page.words.len()), (4, 0));

        // Limit clamps to 200 (contract §4 `words.list` bound).
        let page = store.list(None, 0, 10_000);
        assert_eq!(page.words.len(), 4);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn import_csv_counts_and_dedupes() {
        let dir = temp_dir("import-csv");
        let mut store = open_store(&dir, "words.bin");
        add(&mut store, "amar", "আমার");

        let result = store
            .import(
                ImportFormat::Csv,
                "roman,output,language\r\namar,আমার,bn\r\nvalo,ভালো,bn\r\nbad,,xx\r\nvalo,ভালো,bn\r\n",
            )
            .expect("imports");
        // One new word; the existing duplicate, the bad row, and the
        // intra-payload duplicate are all skipped.
        assert_eq!((result.imported, result.skipped), (1, 3));
        assert_eq!(store.len(), 2);

        // Sources are `import`, ids keep increasing.
        let page = store.list(Some("valo"), 0, 50);
        assert_eq!(page.words[0].source, WordSource::Import);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn import_json_envelope_and_reimport_round_trip() {
        let dir = temp_dir("import-json");
        let mut store = open_store(&dir, "words.bin");
        let result = store
            .import(
                ImportFormat::Json,
                r#"{"schema":1,"words":[{"roman":"amar","output":"আমার","language":"bn"}]}"#,
            )
            .expect("imports envelope");
        assert_eq!((result.imported, result.skipped), (1, 0));

        // Export then re-import: ids are reassigned server-side, so rows that
        // already exist count as skipped instead of failing.
        let exported = store.export(ImportFormat::Json).expect("exports");
        let again = store
            .import(ImportFormat::Json, &exported.data)
            .expect("re-imports");
        assert_eq!((again.imported, again.skipped), (0, 1));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn import_rejects_malformed_and_over_cap() {
        let dir = temp_dir("import-bad");
        let mut store = open_store(&dir, "words.bin");
        assert!(matches!(
            store.import(ImportFormat::Csv, "no-header-here"),
            Err(WordsStoreError::Malformed)
        ));
        assert!(matches!(
            store.import(ImportFormat::Json, "nope"),
            Err(WordsStoreError::Malformed)
        ));
        assert!(matches!(
            store.import(ImportFormat::Json, r#"{"schema":2,"words":[]}"#),
            Err(WordsStoreError::UnsupportedSchema)
        ));

        // Over the 5,000-row per-request cap.
        let mut big = String::from("roman,output,language\r\n");
        for index in 0..=MAX_IMPORT_ROWS {
            big.push_str(&format!("r{index},o{index},bn\r\n"));
        }
        assert!(matches!(
            store.import(ImportFormat::Csv, &big),
            Err(WordsStoreError::TooManyWords)
        ));
        assert!(store.is_empty(), "rejected import adds nothing");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn total_cap_is_all_or_nothing() {
        let dir = temp_dir("cap");
        let path = dir.join("words.bin");
        // Plant a store one word short of the cap directly through the
        // repository (20,000 single adds would be pointlessly slow).
        let words: Vec<Word> = (0..MAX_WORDS_TOTAL as u64 - 1)
            .map(|id| Word {
                id,
                roman: format!("r{id}"),
                output: format!("o{id}"),
                language: WordLanguage::Bn,
                source: WordSource::Import,
                added_at_ms: 1,
            })
            .collect();
        EncryptedJsonRepository::new(&path, FakeProtector)
            .save(&WordsFile { schema: 1, words })
            .expect("plants full store");

        let mut store = open_store(&dir, "words.bin");
        assert_eq!(store.len(), MAX_WORDS_TOTAL - 1);
        add(&mut store, "last", "শেষ");
        assert_eq!(store.len(), MAX_WORDS_TOTAL);

        let error = store
            .add(&WordAdd {
                roman: "over".to_owned(),
                output: "বেশি".to_owned(),
                language: WordLanguage::Bn,
            })
            .expect_err("cap");
        assert!(matches!(error, WordsStoreError::TooManyWords));
        assert_eq!(store.len(), MAX_WORDS_TOTAL);

        // An import that would overflow adds nothing.
        let result = store.import(
            ImportFormat::Csv,
            "roman,output,language\r\nfresh,নতুন,bn\r\n",
        );
        assert!(matches!(result, Err(WordsStoreError::TooManyWords)));
        assert_eq!(store.len(), MAX_WORDS_TOTAL);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn export_round_trips_both_formats() {
        let dir = temp_dir("export");
        let mut store = open_store(&dir, "words.bin");
        add(&mut store, "amar", "আমার");
        add(&mut store, "a,b", "x\"y");

        let csv = store.export(ImportFormat::Csv).expect("exports csv");
        assert_eq!(csv.format, ImportFormat::Csv);
        assert!(csv.data.starts_with("roman,output,language\r\n"));
        assert!(csv.data.contains("\"a,b\""));

        let json = store.export(ImportFormat::Json).expect("exports json");
        let file: WordsFile = serde_json::from_str(&json.data).expect("valid envelope");
        assert_eq!(file.schema, 1);
        assert_eq!(file.words.len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn oversized_export_is_rejected() {
        let dir = temp_dir("export-big");
        let path = dir.join("words.bin");
        // Long outputs push the export past 1 MiB while staying under the
        // 20,000-word cap.
        let words: Vec<Word> = (0..6_000u64)
            .map(|id| Word {
                id,
                roman: format!("roman-{id:05}"),
                output: "অ".repeat(128),
                language: WordLanguage::Bn,
                source: WordSource::Import,
                added_at_ms: 1,
            })
            .collect();
        EncryptedJsonRepository::new(&path, FakeProtector)
            .save(&WordsFile { schema: 1, words })
            .expect("plants big store");

        let store = open_store(&dir, "words.bin");
        let error = store.export(ImportFormat::Csv).expect_err("too large");
        assert!(matches!(error, WordsStoreError::ExportTooLarge));
        assert_eq!(error.code(), ErrorCode::ExportTooLarge);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_and_schema_files_fail_closed() {
        let dir = temp_dir("corrupt");
        let path = dir.join("words.bin");

        std::fs::write(&path, b"definitely-not-an-envelope").expect("plants garbage");
        let error = WordsStore::open(&path, FakeProtector).expect_err("garbage");
        assert!(matches!(
            error,
            WordsStoreError::Storage(StorageError::Corrupt)
        ));

        // Valid envelope, wrong JSON schema: the file is left alone.
        let words: Vec<Word> = Vec::new();
        EncryptedJsonRepository::new(&path, FakeProtector)
            .save(&WordsFile { schema: 2, words })
            .expect("plants schema 2");
        let before = std::fs::read(&path).expect("reads planted");
        let error = WordsStore::open(&path, FakeProtector).expect_err("schema 2");
        assert!(matches!(error, WordsStoreError::UnsupportedSchema));
        assert_eq!(std::fs::read(&path).expect("untouched"), before);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn debug_output_carries_no_word_text() {
        let dir = temp_dir("debug");
        let mut store = open_store(&dir, "words.bin");
        add(&mut store, "amar", "আমার");
        let rendered = format!("{store:?}");
        assert!(!rendered.contains("amar"));
        assert!(!rendered.contains("আমার"));
        assert!(rendered.contains("words"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Production path: DPAPI-backed repository round-trip. Windows-only; CI
    /// on other platforms runs every other test with the fake protector.
    #[cfg(windows)]
    #[test]
    fn dpapi_backed_repository_round_trip() {
        let dir = temp_dir("dpapi-repo");
        let path = dir.join("words.bin");
        let mut store = WordsStore::open(&path, DpapiProtector::words()).expect("opens");
        let word = store
            .add(&WordAdd {
                roman: "amar".to_owned(),
                output: "আমার".to_owned(),
                language: WordLanguage::Bn,
            })
            .expect("adds through DPAPI");
        assert_eq!(word.id, 0);

        // Ciphertext on disk, plaintext in memory only after open.
        let raw = std::fs::read(&path).expect("reads raw");
        assert!(!raw.windows(6).any(|window| window == "আমার".as_bytes()));

        let again = WordsStore::open(&path, DpapiProtector::words()).expect("reopens");
        assert_eq!(again.len(), 1);

        // A blob sealed with other entropy does not open here.
        struct ForeignEntropy;
        impl Protector for ForeignEntropy {
            fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, StorageError> {
                okkhor_win::dpapi::protect(plaintext, b"foreign").map_err(|_| StorageError::Corrupt)
            }
            fn unprotect(&self, ciphertext: &[u8]) -> Result<Vec<u8>, StorageError> {
                okkhor_win::dpapi::unprotect(ciphertext, b"foreign")
                    .map_err(|_| StorageError::Corrupt)
            }
        }
        assert!(matches!(
            WordsStore::open(&path, ForeignEntropy),
            Err(WordsStoreError::Storage(StorageError::Corrupt))
        ));
        std::fs::remove_dir_all(&dir).ok();
    }
}

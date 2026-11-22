//! Akkhor daemon scaffold.
//!
//! Milestone 0.1 deliberately contains no keyboard hook or tray UI. It proves
//! that the workspace, learning engine, ranking, and persistence boundary build
//! and work together. The native input layer is milestone 0.2.

use std::error::Error;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use akkhor_core::{Candidate, CandidateSource, Language, Ranker, SuggestionRequest};
use akkhor_learn::{CommitSource, LearningEvent, Smriti, SmritiConfig};
use akkhor_storage::JsonFileRepository;

fn main() {
    let command = std::env::args().nth(1);

    match command.as_deref() {
        Some("--version") => println!("akkhor {}", env!("CARGO_PKG_VERSION")),
        Some("self-check") => {
            if let Err(error) = self_check() {
                eprintln!("self-check failed: {error}");
                std::process::exit(1);
            }
            println!("self-check: ok");
        }
        Some("--help") | Some("-h") | None => print_help(),
        Some(other) => {
            eprintln!("unknown argument: {other}");
            print_help();
            std::process::exit(2);
        }
    }
}

fn print_help() {
    println!(
        "Akkhor {}\n\nUSAGE:\n    akkhor [--help | --version | self-check]\n\n\
         The 0.1 scaffold has no keyboard hook yet. `self-check` exercises the\n\
         learning engine, ranking, and snapshot persistence.",
        env!("CARGO_PKG_VERSION")
    );
}

fn self_check() -> Result<(), Box<dyn Error>> {
    let smriti = Smriti::new();
    let now = 2_000_u64;

    // English frequency: a repeated word should beat a shorter prefix.
    for _ in 0..10 {
        smriti.observe(LearningEvent::WordCommitted {
            word: "keyboard".to_owned(),
            language: Language::English,
            previous_word: None,
            timestamp_ms: 1_000,
            source: CommitSource::Typed,
        });
    }
    let english = smriti.rank(
        &SuggestionRequest::new("key", Language::English, 5).at(now),
        vec![
            Candidate::new("key", Language::English, CandidateSource::Dictionary),
            Candidate::new("keyboard", Language::English, CandidateSource::Dictionary),
        ],
    );
    assert_eq!(
        english.first().map(|item| item.text.as_str()),
        Some("keyboard")
    );

    // Bangla context: "আমি" should make "ভালো" the expected next word.
    smriti.observe(LearningEvent::WordCommitted {
        word: "আমি".to_owned(),
        language: Language::Bangla,
        previous_word: None,
        timestamp_ms: 1_000,
        source: CommitSource::Typed,
    });
    for _ in 0..10 {
        smriti.observe(LearningEvent::WordCommitted {
            word: "ভালো".to_owned(),
            language: Language::Bangla,
            previous_word: Some("আমি".to_owned()),
            timestamp_ms: 1_500,
            source: CommitSource::Typed,
        });
    }
    let bangla = smriti.rank(
        &SuggestionRequest::new("ভা", Language::Bangla, 5)
            .with_context(["আমি"])
            .at(now),
        vec![
            Candidate::new("খারাপ", Language::Bangla, CandidateSource::Dictionary),
            Candidate::new("ভালো", Language::Bangla, CandidateSource::Dictionary),
        ],
    );
    assert_eq!(bangla.first().map(|item| item.text.as_str()), Some("ভালো"));

    // Transliteration: remember that "bhalo" means "ভালো" for this user.
    for _ in 0..5 {
        smriti.observe(LearningEvent::TransliterationCommitted {
            roman_input: "bhalo".to_owned(),
            output: "ভালো".to_owned(),
            language: Language::Bangla,
            previous_word: None,
            timestamp_ms: 1_700,
        });
    }
    let pair_ranked = smriti.rank(
        &SuggestionRequest::new("ভা", Language::Bangla, 5)
            .with_roman_input("bhalo")
            .at(now),
        vec![
            Candidate::new("ভাল", Language::Bangla, CandidateSource::Transliteration),
            Candidate::new("ভালো", Language::Bangla, CandidateSource::Transliteration),
        ],
    );
    assert_eq!(
        pair_ranked.first().map(|item| item.text.as_str()),
        Some("ভালো")
    );

    // Persistence round-trip.
    let root = fresh_temp_dir();
    let repository = JsonFileRepository::new(root.join("learning.json"));
    repository.save(&smriti.snapshot())?;
    let loaded = repository.load()?.ok_or("snapshot was not written")?;
    let restored = Smriti::from_snapshot(loaded, SmritiConfig::default())?;
    let restored_ranked = restored.rank(
        &SuggestionRequest::new("key", Language::English, 5).at(now),
        vec![
            Candidate::new("key", Language::English, CandidateSource::Dictionary),
            Candidate::new("keyboard", Language::English, CandidateSource::Dictionary),
        ],
    );
    assert_eq!(
        restored_ranked.first().map(|item| item.text.as_str()),
        Some("keyboard")
    );

    let _ = fs::remove_dir_all(root);
    Ok(())
}

fn fresh_temp_dir() -> std::path::PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("akkhor-self-check-{}-{unique}", std::process::id()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn self_check_passes() {
        super::self_check().expect("self-check");
    }
}

//! Windows IPC round-trip: start the server, handshake, and exercise
//! status/settings/words over the real named-pipe transport with an
//! in-memory handler (daemon stores land in Task 2; the handler here proves
//! the IPC layer carries them faithfully).
//!
//! Uses a unique pipe name per run so parallel test binaries never clash.

#![cfg(windows)]

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use okkhor_ipc::{
    Client, ErrorCode, Handler, ImportFormat, Request, Response, Server, Settings, SettingsPatch,
    Status, Token, Word, WordsListQuery,
};

struct MemoryHandler {
    settings: Mutex<Settings>,
    words: Mutex<Vec<Word>>,
    next_id: AtomicU64,
    paused: Mutex<bool>,
}

impl MemoryHandler {
    fn new() -> Self {
        Self {
            settings: Mutex::new(Settings::default()),
            words: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
            paused: Mutex::new(false),
        }
    }

    fn status(&self) -> Status {
        let settings = self.settings.lock().expect("settings");
        let words = self.words.lock().expect("words");
        Status {
            daemon_version: "test-0.5.0".to_owned(),
            paused: *self.paused.lock().expect("paused"),
            language: match settings.language {
                okkhor_ipc::Language::Bn => "bn".to_owned(),
                okkhor_ipc::Language::En => "en".to_owned(),
            },
            layout: settings.layout.clone(),
            input_mode: settings.input_mode,
            output_encoding: settings.output_encoding,
            popup_visible: false,
            words_total: words.len() as u64,
            learning_enabled: settings.learning_enabled,
            autostart: settings.autostart,
        }
    }
}

impl Handler for MemoryHandler {
    fn handle(&self, request: Request) -> Response {
        match request {
            Request::Status(_) => Response::Status(self.status()),
            Request::SettingsGet(_) => {
                Response::Settings(self.settings.lock().expect("settings").clone())
            }
            Request::SettingsUpdate(patch) => {
                let current = self.settings.lock().expect("settings").clone();
                match current.apply(&patch) {
                    Ok(next) => {
                        *self.settings.lock().expect("settings") = next.clone();
                        Response::Settings(next)
                    }
                    Err(error) => Response::error(error.code()),
                }
            }
            Request::LayoutsList(_) => Response::Layouts(vec![
                okkhor_ipc::LayoutInfo {
                    code: "avro".to_owned(),
                    name: "Avro".to_owned(),
                },
                okkhor_ipc::LayoutInfo {
                    code: "probhat".to_owned(),
                    name: "Probhat".to_owned(),
                },
            ]),
            Request::WordsList(query) => {
                let words = self.words.lock().expect("words");
                // Production clamp from `okkhor_ipc` (contract §4: limit ≤ 200).
                let limit = okkhor_ipc::normalize_words_list_limit(query.limit);
                let total = words.len() as u64;
                let start = (query.offset as usize).min(words.len());
                let end = start.saturating_add(limit as usize).min(words.len());
                Response::Words(okkhor_ipc::WordsPage {
                    total,
                    words: words[start..end].to_vec(),
                })
            }
            Request::WordsAdd(add) => {
                let candidate = okkhor_ipc::NewWord {
                    roman: add.roman,
                    output: add.output,
                    language: add.language,
                    source: okkhor_ipc::WordSource::User,
                };
                if okkhor_ipc::validate_new_word(&candidate).is_err() {
                    return Response::error(ErrorCode::InvalidWord);
                }
                let mut words = self.words.lock().expect("words");
                if words.len() >= okkhor_ipc::MAX_WORDS_TOTAL {
                    return Response::error(ErrorCode::TooManyWords);
                }
                if okkhor_ipc::is_duplicate(&words, &candidate) {
                    return Response::error(ErrorCode::DuplicateWord);
                }
                let word = Word {
                    id: self.next_id.fetch_add(1, Ordering::Relaxed),
                    roman: candidate.roman,
                    output: candidate.output,
                    language: candidate.language,
                    source: candidate.source,
                    added_at_ms: 1_763_900_000_000,
                };
                words.push(word.clone());
                Response::Word(word)
            }
            Request::WordsRemove(remove) => {
                let mut words = self.words.lock().expect("words");
                let before = words.len();
                words.retain(|word| word.id != remove.id);
                Response::Removed(okkhor_ipc::WordRemoveResult {
                    removed: words.len() != before,
                })
            }
            Request::WordsImport(import) => {
                let parsed = match import.format {
                    ImportFormat::Csv => okkhor_ipc::parse_words_csv(&import.data),
                    ImportFormat::Json => okkhor_ipc::parse_words_json(&import.data),
                };
                let parsed = match parsed {
                    Ok(parsed) => parsed,
                    Err(error) => return Response::error(error.code()),
                };
                let mut words = self.words.lock().expect("words");
                let mut imported = 0;
                let mut skipped = parsed.skipped;
                for candidate in parsed.words {
                    if words.len() >= okkhor_ipc::MAX_WORDS_TOTAL {
                        return Response::error(ErrorCode::TooManyWords);
                    }
                    if okkhor_ipc::is_duplicate(&words, &candidate) {
                        skipped += 1;
                        continue;
                    }
                    words.push(Word {
                        id: self.next_id.fetch_add(1, Ordering::Relaxed),
                        roman: candidate.roman,
                        output: candidate.output,
                        language: candidate.language,
                        source: candidate.source,
                        added_at_ms: 1_763_900_000_000,
                    });
                    imported += 1;
                }
                Response::Import(okkhor_ipc::WordsImported { imported, skipped })
            }
            Request::WordsExport(export) => {
                let words = self.words.lock().expect("words");
                let data = match export.format {
                    ImportFormat::Csv => okkhor_ipc::words_to_csv(&words),
                    ImportFormat::Json => match okkhor_ipc::words_to_json(&words) {
                        Ok(data) => data,
                        Err(error) => return Response::error(error.code()),
                    },
                };
                if data.len() > okkhor_ipc::MAX_FRAME_BYTES {
                    return Response::error(ErrorCode::ExportTooLarge);
                }
                Response::Export(okkhor_ipc::WordsExported {
                    format: export.format,
                    data,
                })
            }
            Request::TextInsert(insert) => {
                // Production cap from `okkhor_ipc` (contract §4: 512 UTF-16
                // units, reject — never truncate an explicit insertion).
                if let Err(code) = okkhor_ipc::validate_insert_text(&insert.text) {
                    return Response::error(code);
                }
                Response::Inserted(okkhor_ipc::TextInserted { inserted: true })
            }
            Request::CaptureSet(capture) => {
                *self.paused.lock().expect("paused") = capture.paused;
                Response::Status(self.status())
            }
            Request::Hello(_) => Response::error(ErrorCode::InvalidRequest),
            Request::Shutdown(_) => Response::Ok(okkhor_ipc::protocol::NoArgs {}),
            Request::AiComplete(_) => Response::error(ErrorCode::Unsupported),
            // 0.7 AI methods are unknown to this 0.5-era handler stand-in;
            // the typed variants must still map to a typed error.
            Request::AiPoll(_) | Request::AiCancel(_) | Request::AiStatus(_) => {
                Response::error(ErrorCode::Unsupported)
            }
        }
    }
}

#[tokio::test]
async fn ipc_round_trip_status_settings_words() {
    let pipe_name = format!(r"\\.\pipe\okkhor-ipc-it-{}", std::process::id());
    let token = Token::generate().expect("entropy");
    let server = Arc::new(
        Server::new(Arc::new(MemoryHandler::new()), token.clone(), "test-0.5.0")
            .with_pipe_name(pipe_name.clone()),
    );
    let failures = server.auth_failures();
    assert_eq!(failures, 0);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn({
        let server = server.clone();
        async move { server.run(shutdown_rx).await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Handshake.
    let (mut client, greeting) = Client::connect_to_with_token(&pipe_name, &token.hex())
        .await
        .expect("handshake");
    assert_eq!(greeting.protocol, okkhor_ipc::PROTOCOL_VERSION);
    assert_eq!(greeting.daemon_version, "test-0.5.0");

    // Status round-trip.
    let status = client.status().await.expect("status");
    assert_eq!(status.words_total, 0);
    assert!(!status.paused);

    // capture.set toggles pause and returns fresh status.
    let status = client.set_capture(true).await.expect("capture.set");
    assert!(status.paused);

    // settings.get returns defaults; settings.update merges and clamps.
    let settings = client.settings_get().await.expect("settings.get");
    assert_eq!(settings, Settings::default());
    assert_eq!(settings.input_mode, okkhor_ipc::InputMode::Bangla);
    let updated = client
        .settings_update(&SettingsPatch {
            theme: Some(okkhor_ipc::Theme::Dark),
            input_mode: Some(okkhor_ipc::InputMode::English),
            output_encoding: Some(okkhor_ipc::OutputEncoding::Bijoy),
            toggle_hotkey: Some("Ctrl+Space".to_owned()),
            quickbar_x: Some(Some(120)),
            quickbar_opacity: Some(9.0),
            popup_max_items: Some(99),
            ..SettingsPatch::default()
        })
        .await
        .expect("settings.update");
    assert_eq!(updated.theme, okkhor_ipc::Theme::Dark);
    assert_eq!(updated.input_mode, okkhor_ipc::InputMode::English);
    assert_eq!(updated.output_encoding, okkhor_ipc::OutputEncoding::Bijoy);
    assert_eq!(updated.toggle_hotkey, "Ctrl+Space");
    assert_eq!(updated.quickbar_x, Some(120));
    assert_eq!(updated.quickbar_opacity, 1.0);
    assert_eq!(updated.popup_max_items, 10);
    assert_eq!(
        client.settings_get().await.expect("get").theme,
        okkhor_ipc::Theme::Dark
    );
    // Status reflects the updated mode and encoding for the quick bar.
    let status = client.status().await.expect("status");
    assert_eq!(status.input_mode, okkhor_ipc::InputMode::English);
    assert_eq!(status.output_encoding, okkhor_ipc::OutputEncoding::Bijoy);

    // layouts.list.
    let layouts = client.layouts_list().await.expect("layouts.list");
    assert!(layouts.iter().any(|layout| layout.code == "avro"));

    // words.add / words.list / words.remove round-trip.
    let word = client
        .words_add(&okkhor_ipc::WordAdd {
            roman: "amar".to_owned(),
            output: "আমার".to_owned(),
            language: okkhor_ipc::WordLanguage::Bn,
        })
        .await
        .expect("words.add");
    assert_eq!(word.id, 1);
    let duplicate = client
        .words_add(&okkhor_ipc::WordAdd {
            roman: "amar".to_owned(),
            output: "আমার".to_owned(),
            language: okkhor_ipc::WordLanguage::Bn,
        })
        .await
        .expect_err("duplicate rejected");
    assert!(matches!(
        duplicate,
        okkhor_ipc::ClientError::Server(body) if body.code == ErrorCode::DuplicateWord
    ));
    let page = client
        .words_list(&WordsListQuery {
            query: None,
            offset: 0,
            limit: 50,
        })
        .await
        .expect("words.list");
    assert_eq!(page.total, 1);
    assert_eq!(page.words.len(), 1);

    // words.import (CSV) then words.export (JSON) round-trip.
    let imported = client
        .words_import(&okkhor_ipc::WordsImport {
            format: ImportFormat::Csv,
            data: "roman,output,language\r\nami,আমি,bn\r\n".to_owned(),
        })
        .await
        .expect("words.import");
    assert_eq!(imported.imported, 1);
    let exported = client
        .words_export(ImportFormat::Json)
        .await
        .expect("words.export");
    let file: serde_json::Value = serde_json::from_str(&exported.data).expect("export is JSON");
    assert_eq!(file["words"].as_array().map(Vec::len), Some(2));

    // words.remove.
    assert!(client.words_remove(word.id).await.expect("remove"));
    let page = client
        .words_list(&WordsListQuery {
            query: None,
            offset: 0,
            limit: 50,
        })
        .await
        .expect("words.list");
    assert_eq!(page.total, 1);

    // text.insert within the cap.
    assert!(client.insert_text("অ").await.expect("insert"));
    let too_long = "x".repeat(513);
    let rejected = client
        .insert_text(&too_long)
        .await
        .expect_err("over cap rejected");
    assert!(matches!(
        rejected,
        okkhor_ipc::ClientError::Server(body) if body.code == ErrorCode::TextTooLong
    ));

    // ai.complete still answers typed Unsupported until the daemon handler
    // lands (WS3 task 4); the transport no longer short-circuits the variant.
    let rejected = client
        .ai_complete(&okkhor_ipc::AiCompleteParams {
            prompt: "x".to_owned(),
            system: None,
            model: None,
            max_tokens: None,
            temperature: None,
            timeout_ms: None,
        })
        .await
        .expect_err("ai.complete answers Unsupported");
    assert!(matches!(
        rejected,
        okkhor_ipc::ClientError::Server(body) if body.code == ErrorCode::Unsupported
    ));

    // Clean shutdown: the run loop exits on shutdown.
    drop(client);
    let _ = shutdown_tx.send(());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
}

//! Daemon IPC round-trip tests (contract §4, Task 3).
//!
//! The daemon's IPC modules are included directly (same pattern as unit
//! tests) so this file drives the real [`ipc::DaemonState`] handler — not a
//! stub. On Windows the handler additionally serves behind a real
//! [`okkhor_ipc::Server`] on a unique test pipe: handshake, status, settings
//! round-trip, words add/list/remove/import/export, insert validation and the
//! 20/s rate limit with a recorder sink, `capture.set`, bad token, oversized
//! frame, unknown method, and client-drop health. On other platforms the
//! transport is unavailable, so the same handler is driven directly.
//!
//! No hook is installed, no real `SendInput` runs (the recorder stands in for
//! the injector), no user text is logged, and temp dirs are removed.
//!
//! `dead_code` is allowed on the included copies only: the daemon binary and
//! this target use complementary subsets (loop-only wiring vs. test drivers),
//! and neither target can see the other's uses. Real dead code in *this* file
//! still warns.
#[allow(dead_code)]
#[path = "../src/ipc.rs"]
mod ipc;
#[allow(dead_code)]
#[path = "../src/paths.rs"]
mod paths;
#[allow(dead_code)]
#[path = "../src/settings_store.rs"]
mod settings_store;
#[allow(dead_code)]
#[path = "../src/words_store.rs"]
mod words_store;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use ipc::{DaemonState, TextSink};
use okkhor_storage::FakeProtector;

fn temp_dir(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "okkhor-ipc-roundtrip-{}-{}-{}-{}",
        name,
        std::process::id(),
        unique,
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// Layout mimicking the production data home: `<home>/settings.json`,
/// `<home>/data/words.bin`, `<home>/runtime/ipc.token`.
struct TempHome {
    dir: PathBuf,
    settings: PathBuf,
    words: PathBuf,
    token: PathBuf,
}

impl TempHome {
    fn new(name: &str) -> Self {
        let dir = temp_dir(name);
        std::fs::create_dir_all(dir.join("data")).expect("data dir");
        std::fs::create_dir_all(dir.join("runtime")).expect("runtime dir");
        Self {
            settings: dir.join("settings.json"),
            words: dir.join("data").join("words.bin"),
            token: dir.join("runtime").join("ipc.token"),
            dir,
        }
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

/// Recorder sink: explicit UI inserts land here instead of `SendInput`.
#[derive(Debug, Default)]
struct Recorder {
    texts: Mutex<Vec<String>>,
}

impl Recorder {
    fn texts(&self) -> Vec<String> {
        self.lock().clone()
    }

    fn lock(&self) -> MutexGuard<'_, Vec<String>> {
        self.texts.lock().unwrap_or_else(|error| error.into_inner())
    }
}

impl TextSink for Recorder {
    fn insert_text(&self, text: &str) -> Result<(), okkhor_ipc::ErrorCode> {
        self.lock().push(text.to_owned());
        Ok(())
    }
}

fn open_state(home: &TempHome) -> (DaemonState<FakeProtector>, Arc<Recorder>) {
    let sink = Arc::new(Recorder::default());
    let injection = Arc::new(Mutex::new(()));
    let state = DaemonState::open(
        &home.settings,
        &home.words,
        FakeProtector,
        sink.clone(),
        injection,
    )
    .expect("opens state");
    (state, sink)
}

fn unique_pipe(name: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        r"\\.\pipe\okkhor-ipc-test-{}-{}-{}",
        name,
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// Full stack over a named pipe: handshake, status, settings, words, insert,
/// capture, and every failure shape in contract §4.
#[cfg(windows)]
#[tokio::test]
async fn ipc_roundtrip_over_pipe() {
    use okkhor_ipc::protocol::{
        Hello, ImportFormat, Request, Response, WordAdd, WordsImport, WordsListQuery,
    };
    use okkhor_ipc::settings::{InputMode, OutputEncoding, SettingsPatch};
    use okkhor_ipc::{
        Client, ErrorCode, MAX_FRAME_BYTES, READ_TIMEOUT, Server,
        frame::{read_frame, write_frame},
    };
    use tokio::io::AsyncWriteExt as _;

    let home = TempHome::new("pipe");
    let (state, sink) = open_state(&home);
    let token = ipc::install_token(&home.token).expect("installs token");
    assert!(home.token.is_file(), "token file is created");

    let pipe = unique_pipe("roundtrip");
    let server =
        Arc::new(Server::new(Arc::new(state), token.clone(), "test").with_pipe_name(pipe.clone()));
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn({
        let server = server.clone();
        async move { server.run(shutdown_rx).await }
    });
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Handshake: hello_ok carries protocol, version, and pid.
    let (mut client, greeting) = Client::connect_to_with_token(&pipe, &token.hex())
        .await
        .expect("handshake");
    assert_eq!(greeting.protocol, okkhor_ipc::PROTOCOL_VERSION);
    assert!(!greeting.daemon_version.is_empty());
    assert!(greeting.pid > 0);

    // Status defaults (contract §4 + input_mode/output_encoding mirrors).
    let status = client.status().await.expect("status");
    assert!(!status.paused);
    assert_eq!(status.language, "bn");
    assert_eq!(status.layout, "avro");
    assert_eq!(status.input_mode, InputMode::Bangla);
    assert_eq!(status.output_encoding, OutputEncoding::Unicode);
    assert!(!status.popup_visible);
    assert_eq!(status.words_total, 0);
    assert!(status.learning_enabled);
    assert!(!status.autostart);

    // layouts.list carries the four 0.5 layouts with display names.
    let layouts = client.layouts_list().await.expect("layouts");
    assert_eq!(
        layouts
            .iter()
            .map(|entry| entry.code.as_str())
            .collect::<Vec<_>>(),
        vec!["avro", "khipro", "banglish", "bijoy"]
    );
    assert!(layouts.iter().all(|entry| !entry.name.is_empty()));

    // settings round-trip: mode + encoding apply live and persist.
    let settings = client
        .settings_update(&SettingsPatch {
            input_mode: Some(InputMode::English),
            output_encoding: Some(OutputEncoding::Bijoy),
            ..SettingsPatch::default()
        })
        .await
        .expect("settings.update");
    assert_eq!(settings.input_mode, InputMode::English);
    assert_eq!(settings.output_encoding, OutputEncoding::Bijoy);
    let fetched = client.settings_get().await.expect("settings.get");
    assert_eq!(fetched, settings);
    let status = client.status().await.expect("status");
    assert_eq!(status.input_mode, InputMode::English);
    assert_eq!(status.output_encoding, OutputEncoding::Bijoy);
    // Revert so the persisted file stays at defaults.
    client
        .settings_update(&SettingsPatch {
            input_mode: Some(InputMode::Bangla),
            output_encoding: Some(OutputEncoding::Unicode),
            ..SettingsPatch::default()
        })
        .await
        .expect("reverts");

    // Invalid settings are typed errors, never text.
    let error = client
        .settings_update(&SettingsPatch {
            layout: Some("nope".to_owned()),
            ..SettingsPatch::default()
        })
        .await
        .expect_err("bad layout");
    assert!(matches!(
        error,
        okkhor_ipc::ClientError::Server(body) if body.code == ErrorCode::InvalidLayout
    ));

    // words: add, duplicate, list/search/page, remove.
    let word = client
        .words_add(&WordAdd {
            roman: "amar".to_owned(),
            output: "আমার".to_owned(),
            language: okkhor_ipc::WordLanguage::Bn,
        })
        .await
        .expect("words.add");
    assert_eq!(word.id, 0);
    let error = client
        .words_add(&WordAdd {
            roman: "amar".to_owned(),
            output: "আমার".to_owned(),
            language: okkhor_ipc::WordLanguage::Bn,
        })
        .await
        .expect_err("duplicate");
    assert!(matches!(
        error,
        okkhor_ipc::ClientError::Server(body) if body.code == ErrorCode::DuplicateWord
    ));
    let page = client
        .words_list(&WordsListQuery {
            query: Some("ama".to_owned()),
            offset: 0,
            limit: 50,
        })
        .await
        .expect("words.list");
    assert_eq!((page.total, page.words.len()), (1, 1));
    let empty = client
        .words_list(&WordsListQuery {
            query: Some("zzz-no-match".to_owned()),
            offset: 0,
            limit: 50,
        })
        .await
        .expect("words.list empty");
    assert_eq!((empty.total, empty.words.len()), (0, 0));
    assert!(client.words_remove(word.id).await.expect("removes"));
    assert!(!client.words_remove(word.id).await.expect("missing"));

    // import CSV then export JSON; the export re-imports with skips.
    let imported = client
        .words_import(&WordsImport {
            format: ImportFormat::Csv,
            data: "roman,output,language\r\nvalo,ভালো,bn\r\n".to_owned(),
        })
        .await
        .expect("words.import");
    assert_eq!((imported.imported, imported.skipped), (1, 0));
    let exported = client
        .words_export(ImportFormat::Json)
        .await
        .expect("words.export");
    assert_eq!(exported.format, ImportFormat::Json);
    let file: okkhor_ipc::words::WordsFile =
        serde_json::from_str(&exported.data).expect("valid envelope");
    assert_eq!((file.schema, file.words.len()), (1, 1));
    let again = client
        .words_import(&WordsImport {
            format: ImportFormat::Json,
            data: exported.data,
        })
        .await
        .expect("re-imports");
    assert_eq!((again.imported, again.skipped), (0, 1));

    // text.insert: valid text reaches the recorder; over-cap is rejected.
    assert!(client.insert_text("abc").await.expect("inserts"));
    assert_eq!(sink.texts(), vec!["abc".to_owned()]);
    let error = client
        .insert_text(&"x".repeat(513))
        .await
        .expect_err("too long");
    assert!(matches!(
        error,
        okkhor_ipc::ClientError::Server(body) if body.code == ErrorCode::TextTooLong
    ));

    // capture.set flips pause and reports it back.
    let status = client.set_capture(true).await.expect("capture.set");
    assert!(status.paused);
    let status = client.set_capture(false).await.expect("capture.set");
    assert!(!status.paused);

    // ai.complete is typed Unsupported in 0.5.
    let response = client
        .ai_complete(serde_json::json!({}))
        .await
        .expect("frame");
    assert_eq!(
        response.error_body().map(|body| body.code),
        Some(ErrorCode::Unsupported)
    );

    // Bad token: typed Unauthorized.
    let wrong = okkhor_ipc::Token::generate().expect("entropy");
    let mut intruder = Client::connect_to(&pipe).await.expect("connects");
    let error = intruder.hello(&wrong.hex()).await.expect_err("rejected");
    assert!(matches!(
        error,
        okkhor_ipc::ClientError::Server(body) if body.code == ErrorCode::Unauthorized
    ));
    assert_eq!(server.auth_failures(), 1);

    // Raw connection: oversized frame then unknown method.
    let mut raw = tokio::net::windows::named_pipe::ClientOptions::new()
        .open(&pipe)
        .expect("raw connects");
    let hello = Request::Hello(Hello {
        protocol: okkhor_ipc::PROTOCOL_VERSION,
        token: token.hex(),
    });
    write_frame(&mut raw, &serde_json::to_vec(&hello).expect("hello"))
        .await
        .expect("writes");
    let frame = read_frame(&mut raw, READ_TIMEOUT).await.expect("reads");
    let greeting: Response = serde_json::from_slice(&frame).expect("greeting");
    assert!(matches!(greeting, Response::HelloOk(_)));
    let oversized = (MAX_FRAME_BYTES + 1) as u32;
    raw.write_u32_le(oversized).await.expect("writes len");
    raw.write_all(&vec![b'x'; 1024]).await.expect("writes part");
    let frame = read_frame(&mut raw, READ_TIMEOUT).await.expect("reads");
    let response: Response = serde_json::from_slice(&frame).expect("error frame");
    assert_eq!(
        response.error_body().map(|body| body.code),
        Some(ErrorCode::FrameTooLarge)
    );

    let mut raw2 = tokio::net::windows::named_pipe::ClientOptions::new()
        .open(&pipe)
        .expect("raw connects");
    write_frame(&mut raw2, &serde_json::to_vec(&hello).expect("hello"))
        .await
        .expect("writes");
    let frame = read_frame(&mut raw2, READ_TIMEOUT).await.expect("reads");
    let _: Response = serde_json::from_slice(&frame).expect("greeting");
    write_frame(&mut raw2, br#"{"nope":{}}"#)
        .await
        .expect("writes");
    let frame = read_frame(&mut raw2, READ_TIMEOUT).await.expect("reads");
    let response: Response = serde_json::from_slice(&frame).expect("error frame");
    assert_eq!(
        response.error_body().map(|body| body.code),
        Some(ErrorCode::UnknownMethod)
    );

    // Client drop health: dropping mid-session never wedges the server.
    drop(client);
    drop(raw2);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (mut fresh, _) = Client::connect_to_with_token(&pipe, &token.hex())
        .await
        .expect("reconnects after drops");
    let status = fresh.status().await.expect("status after drops");
    assert_eq!(status.words_total, 1);

    // Rate limit: 20 inserts pass on a fresh budget, the 21st is denied.
    let home2 = TempHome::new("ratelimit");
    let (state2, _) = open_state(&home2);
    let pipe2 = unique_pipe("ratelimit");
    let server2 = Arc::new(
        Server::new(Arc::new(state2), token.clone(), "test").with_pipe_name(pipe2.clone()),
    );
    let (shutdown2, rx2) = tokio::sync::oneshot::channel();
    let task2 = tokio::spawn({
        let server2 = server2.clone();
        async move { server2.run(rx2).await }
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    let (mut burst, _) = Client::connect_to_with_token(&pipe2, &token.hex())
        .await
        .expect("burst connects");
    for _ in 0..okkhor_ipc::protocol::MAX_INSERT_UTF16.min(20) {
        assert!(burst.insert_text("x").await.expect("burst insert"));
    }
    let error = burst
        .insert_text("one-too-many")
        .await
        .expect_err("limited");
    assert!(matches!(
        error,
        okkhor_ipc::ClientError::Server(body) if body.code == ErrorCode::Timeout
    ));
    let _ = shutdown2.send(());
    let _ = tokio::time::timeout(Duration::from_secs(5), task2).await;

    // Clean shutdown: the server stops and the token file is removed.
    let _ = shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
    ipc::remove_token_file(&home.token).expect("removes token");
    assert!(!home.token.exists(), "token removed on shutdown");
}

/// Handler-direct coverage for platforms without the named-pipe transport.
/// Mirrors the contract §4 sweep above (minus framing and auth, which live in
/// the transport crate's own tests).
#[test]
fn handler_direct_status_settings_words_insert_capture() {
    use okkhor_ipc::ErrorCode;
    use okkhor_ipc::protocol::{
        CaptureSet, NoArgs, Request, Response, TextInsert, WordAdd, WordsListQuery,
    };
    use okkhor_ipc::settings::{InputMode, OutputEncoding, SettingsPatch};

    let home = TempHome::new("direct");
    let (state, sink) = open_state(&home);

    // status defaults + layouts.
    let status = state.status();
    assert_eq!(
        (status.language.as_str(), status.layout.as_str()),
        ("bn", "avro")
    );
    assert_eq!(status.input_mode, InputMode::Bangla);
    assert_eq!(ipc::layout_list().len(), 4);

    // settings round-trip through requests.
    let response = state.handle_request(Request::SettingsUpdate(SettingsPatch {
        input_mode: Some(InputMode::English),
        output_encoding: Some(OutputEncoding::Bijoy),
        ..SettingsPatch::default()
    }));
    assert!(matches!(response, Response::Settings(_)));
    assert_eq!(state.mode(), InputMode::English);
    let response = state.handle_request(Request::SettingsGet(NoArgs {}));
    assert!(matches!(response, Response::Settings(_)));
    let response = state.handle_request(Request::SettingsUpdate(SettingsPatch {
        layout: Some("nope".to_owned()),
        ..SettingsPatch::default()
    }));
    assert_eq!(
        response.error_body().map(|body| body.code),
        Some(ErrorCode::InvalidLayout)
    );

    // words add/list/remove through requests.
    let response = state.handle_request(Request::WordsAdd(WordAdd {
        roman: "amar".to_owned(),
        output: "আমার".to_owned(),
        language: okkhor_ipc::WordLanguage::Bn,
    }));
    assert!(matches!(response, Response::Word(_)));
    let response = state.handle_request(Request::WordsList(WordsListQuery {
        query: None,
        offset: 0,
        limit: 50,
    }));
    match response {
        Response::Words(page) => assert_eq!(page.total, 1),
        other => panic!("words.list failed: {other:?}"),
    }

    // insert validation + sink delivery.
    let response = state.handle_request(Request::TextInsert(TextInsert {
        text: "abc".to_owned(),
    }));
    assert!(matches!(
        response,
        Response::Inserted(inserted) if inserted.inserted
    ));
    assert_eq!(sink.texts(), vec!["abc".to_owned()]);
    let response = state.handle_request(Request::TextInsert(TextInsert {
        text: "x".repeat(513),
    }));
    assert_eq!(
        response.error_body().map(|body| body.code),
        Some(ErrorCode::TextTooLong)
    );

    // capture.set + ai.complete.
    let response = state.handle_request(Request::CaptureSet(CaptureSet { paused: true }));
    match response {
        Response::Status(status) => assert!(status.paused),
        other => panic!("capture.set failed: {other:?}"),
    }
    let response = state.handle_request(Request::AiComplete(serde_json::json!({})));
    assert_eq!(
        response.error_body().map(|body| body.code),
        Some(ErrorCode::Unsupported)
    );
}

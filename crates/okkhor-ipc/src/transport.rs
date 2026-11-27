//! Tokio named-pipe transport (Windows only, ADR 0012 §2).
//!
//! Pipe: `\\.\pipe\okkhor-ipc-v1`, one instance per client, max 4 concurrent
//! clients. Every connection starts with a `hello` handshake (protocol check
//! plus constant-time token compare); failures send a typed error, close the
//! connection, and bump a saturating counter. Requests run under a 5 s
//! deadline; inbound reads time out after 10 s.
//!
//! Each pipe instance is created with the owner-only DACL already attached
//! ([`crate::sddl::create_server_instance`]), so the descriptor is present
//! from the first byte — at no point is an instance reachable under the
//! default DACL.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};
use tokio::sync::Semaphore;

use crate::auth::Token;
use crate::error::ErrorCode;
use crate::frame::{FrameError, read_frame, write_frame};
use crate::protocol::{
    MAX_CLIENTS, PIPE_NAME, PROTOCOL_VERSION, READ_TIMEOUT, REQUEST_DEADLINE, Request, Response,
    ServerHello,
};

/// Transport failures. None carry user text, tokens, or paths.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("pipe I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("framing failed: {0}")]
    Frame(#[from] FrameError),
}

/// Client-side failures: transport problems, typed daemon errors, or local
/// protocol violations (fixed strings only).
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("pipe I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("framing failed: {0}")]
    Frame(#[from] FrameError),
    #[error("daemon error: {0:?}")]
    Server(crate::protocol::ErrorBody),
    #[error("protocol violation")]
    Protocol,
}

/// Request handler implemented by the daemon (Task 3). Kept synchronous and
/// fast: it runs on a blocking thread under the 5 s request deadline so a
/// slow handler cannot stall the connection task.
pub trait Handler: Send + Sync + 'static {
    fn handle(&self, request: Request) -> Response;
}

/// IPC server. Create with [`Server::new`], drive with [`Server::run`].
///
/// The pipe name defaults to [`PIPE_NAME`]; tests use [`Server::with_pipe_name`]
/// so parallel test binaries never share the real pipe.
pub struct Server {
    handler: Arc<dyn Handler>,
    token: Token,
    daemon_version: String,
    pipe_name: String,
    request_deadline: std::time::Duration,
    slots: Arc<Semaphore>,
    auth_failures: Arc<AtomicU64>,
}

impl Server {
    #[must_use]
    pub fn new(handler: Arc<dyn Handler>, token: Token, daemon_version: impl Into<String>) -> Self {
        Self {
            handler,
            token,
            daemon_version: daemon_version.into(),
            pipe_name: PIPE_NAME.to_owned(),
            request_deadline: REQUEST_DEADLINE,
            slots: Arc::new(Semaphore::new(MAX_CLIENTS)),
            auth_failures: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Use a custom pipe name (tests only). Production always uses [`PIPE_NAME`].
    #[must_use]
    pub fn with_pipe_name(mut self, pipe_name: impl Into<String>) -> Self {
        self.pipe_name = pipe_name.into();
        self
    }

    /// Override the per-request deadline (tests only; production is 5 s per
    /// contract §4). Lets the timeout path be exercised without slow tests.
    #[must_use]
    pub fn with_request_deadline(mut self, deadline: std::time::Duration) -> Self {
        self.request_deadline = deadline;
        self
    }

    /// Saturating count of failed handshakes (bad token, wrong protocol,
    /// malformed hello). Bounded: saturates at `u64::MAX`.
    #[must_use]
    pub fn auth_failures(&self) -> u64 {
        self.auth_failures.load(Ordering::Relaxed)
    }

    /// Accept clients until `shutdown` fires. Stops accepting; in-flight
    /// connections run to completion.
    pub async fn run(
        &self,
        mut shutdown: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<(), TransportError> {
        let mut first = true;
        loop {
            let pipe_name: &str = &self.pipe_name;
            let mut builder = ServerOptions::new();
            builder
                .first_pipe_instance(first)
                .reject_remote_clients(true);
            // The owner-only DACL is attached by creation (sddl.rs), so the
            // instance is never reachable under the default DACL.
            let instance = crate::sddl::create_server_instance(&builder, pipe_name)?;
            first = false;
            tokio::select! {
                biased;
                _ = &mut shutdown => {
                    let _ = instance.disconnect();
                    return Ok(());
                }
                result = instance.connect() => {
                    result?;
                    self.spawn_connection(instance);
                }
            }
        }
    }

    fn spawn_connection(&self, stream: NamedPipeServer) {
        // At capacity: answer with a typed error instead of queueing
        // (bounded), so the client sees `Internal` rather than an EOF.
        // The error frame fits the pipe buffer, so this cannot block.
        let Ok(permit) = self.slots.clone().try_acquire_owned() else {
            tokio::spawn(async move {
                let mut stream = stream;
                send_error(&mut stream, ErrorCode::Internal).await;
            });
            return;
        };
        let handler = self.handler.clone();
        let token = self.token.clone();
        let version = self.daemon_version.clone();
        let deadline = self.request_deadline;
        let failures = self.auth_failures.clone();
        tokio::spawn(async move {
            let _permit = permit;
            serve_connection(stream, handler, &token, &version, deadline, &failures).await;
        });
    }
}

/// Saturating increment with a compare-exchange loop: concurrent handshake
/// failures are each counted exactly once, and the counter pins at
/// `u64::MAX` instead of wrapping.
fn fail(counter: &AtomicU64) {
    let mut current = counter.load(Ordering::Relaxed);
    loop {
        let next = current.saturating_add(1);
        match counter.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(actual) => current = actual,
        }
    }
}

/// Read one frame and parse it as a [`Request`] with a precise error code.
async fn read_request(stream: &mut NamedPipeServer) -> Result<Request, (Option<ErrorCode>, bool)> {
    let frame = read_frame(stream, READ_TIMEOUT).await.map_err(|error| {
        let code = match error {
            FrameError::Empty | FrameError::Truncated | FrameError::Timeout => None,
            // The length prefix arrived intact, so the peer is still reading:
            // report the violation, then close.
            FrameError::TooLarge(_) => Some(ErrorCode::FrameTooLarge),
            FrameError::Io(_) => None,
        };
        (code, code.is_some())
    })?;
    let text = String::from_utf8(frame).map_err(|_| (Some(ErrorCode::InvalidRequest), true))?;
    Request::from_json(&text).map_err(|code| (Some(code), true))
}

async fn send_error(stream: &mut NamedPipeServer, code: ErrorCode) {
    let payload = serde_json::to_vec(&Response::error(code)).unwrap_or_else(|_| b"{}".to_vec());
    let _ = write_frame(stream, &payload).await;
}

async fn serve_connection(
    mut stream: NamedPipeServer,
    handler: Arc<dyn Handler>,
    token: &Token,
    daemon_version: &str,
    request_deadline: std::time::Duration,
    failures: &AtomicU64,
) {
    // The first request must be a valid hello.
    let request = match read_request(&mut stream).await {
        Ok(request) => request,
        Err((code, _)) => {
            if let Some(code) = code {
                send_error(&mut stream, code).await;
            }
            fail(failures);
            return;
        }
    };
    let Request::Hello(hello) = request else {
        send_error(&mut stream, ErrorCode::InvalidRequest).await;
        fail(failures);
        return;
    };
    if hello.protocol != PROTOCOL_VERSION {
        send_error(&mut stream, ErrorCode::ProtocolMismatch).await;
        fail(failures);
        return;
    }
    let presented = Token::parse_hex(hello.token.trim());
    let Ok(presented) = presented else {
        send_error(&mut stream, ErrorCode::Unauthorized).await;
        fail(failures);
        return;
    };
    if !presented.constant_time_eq(token) {
        send_error(&mut stream, ErrorCode::Unauthorized).await;
        fail(failures);
        return;
    }
    let greeting = Response::HelloOk(ServerHello {
        protocol: PROTOCOL_VERSION,
        daemon_version: daemon_version.to_owned(),
        pid: std::process::id(),
    });
    let Ok(payload) = serde_json::to_vec(&greeting) else {
        return;
    };
    if write_frame(&mut stream, &payload).await.is_err() {
        return;
    }

    loop {
        let request = match read_request(&mut stream).await {
            Ok(request) => request,
            Err((code, reply)) => {
                if reply && let Some(code) = code {
                    send_error(&mut stream, code).await;
                }
                return;
            }
        };
        if matches!(request, Request::Hello(_)) {
            // Exactly one hello per connection, as the first request.
            send_error(&mut stream, ErrorCode::InvalidRequest).await;
            return;
        }
        let response = dispatch(handler.clone(), request, request_deadline).await;
        let Ok(payload) = serde_json::to_vec(&response) else {
            send_error(&mut stream, ErrorCode::Internal).await;
            return;
        };
        if write_frame(&mut stream, &payload).await.is_err() {
            return;
        }
    }
}

/// Run the handler under the per-request deadline on a blocking thread.
/// `ai.complete` is a normal request: it must validate and enqueue, never wait
/// for the provider (ADR 0018 §5).
///
/// On deadline expiry the client gets [`ErrorCode::Timeout`]; the orphaned
/// blocking thread runs to completion on its own and never touches the
/// connection again.
async fn dispatch(
    handler: Arc<dyn Handler>,
    request: Request,
    deadline: std::time::Duration,
) -> Response {
    let worker = tokio::task::spawn_blocking(move || handler.handle(request));
    match tokio::time::timeout(deadline, worker).await {
        Ok(Ok(response)) => response,
        Ok(Err(_)) => Response::error(ErrorCode::Internal),
        Err(_) => Response::error(ErrorCode::Timeout),
    }
}

/// IPC client used by the daemon tests and the Tauri Rust side (Task 4).
pub struct Client {
    stream: tokio::net::windows::named_pipe::NamedPipeClient,
}

/// How long a client keeps retrying while every daemon pipe instance is busy
/// or the daemon is still starting. The server caps concurrent clients at
/// [`MAX_CLIENTS`]; a burst of UI queries must wait for a free slot instead of
/// failing with `daemon_unavailable`.
const CONNECT_RETRY_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);
/// First backoff between connect attempts (doubles up to 200 ms).
const CONNECT_RETRY_START: std::time::Duration = std::time::Duration::from_millis(25);
const ERROR_PIPE_BUSY_CODE: i32 = 231;
const ERROR_FILE_NOT_FOUND_CODE: i32 = 2;

/// Open a client pipe, retrying transient failures (all instances busy, or the
/// daemon has not created its first instance yet) for a bounded window.
async fn open_with_retry(
    pipe_name: &str,
) -> Result<tokio::net::windows::named_pipe::NamedPipeClient, ClientError> {
    use tokio::time::Instant;
    let started = Instant::now();
    let mut delay = CONNECT_RETRY_START;
    loop {
        match ClientOptions::new().open(pipe_name) {
            Ok(stream) => return Ok(stream),
            Err(error) => {
                let retryable = matches!(
                    error.raw_os_error(),
                    Some(ERROR_PIPE_BUSY_CODE | ERROR_FILE_NOT_FOUND_CODE)
                );
                if !retryable || started.elapsed() >= CONNECT_RETRY_WINDOW {
                    return Err(error.into());
                }
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(std::time::Duration::from_millis(200));
            }
        }
    }
}

impl Client {
    /// Connect to the daemon pipe ([`PIPE_NAME`]).
    pub async fn connect() -> Result<Self, ClientError> {
        Self::connect_to(PIPE_NAME).await
    }

    /// Connect to a custom pipe name (tests only).
    pub async fn connect_to(pipe_name: &str) -> Result<Self, ClientError> {
        let stream = open_with_retry(pipe_name).await?;
        Ok(Self { stream })
    }

    /// Connect and complete the hello handshake.
    pub async fn connect_with_token(token_hex: &str) -> Result<(Self, ServerHello), ClientError> {
        Self::connect_to_with_token(PIPE_NAME, token_hex).await
    }

    /// Connect to a custom pipe name and complete the hello handshake.
    pub async fn connect_to_with_token(
        pipe_name: &str,
        token_hex: &str,
    ) -> Result<(Self, ServerHello), ClientError> {
        let mut client = Self::connect_to(pipe_name).await?;
        let greeting = client.hello(token_hex).await?;
        Ok((client, greeting))
    }

    /// Send the hello handshake on an open connection.
    pub async fn hello(&mut self, token_hex: &str) -> Result<ServerHello, ClientError> {
        let request = Request::Hello(crate::protocol::Hello {
            protocol: PROTOCOL_VERSION,
            token: token_hex.to_owned(),
        });
        match self.call(&request).await? {
            Response::HelloOk(greeting) => Ok(greeting),
            Response::Error(body) => Err(ClientError::Server(body)),
            _ => Err(ClientError::Protocol),
        }
    }

    /// One request round-trip with a 10 s client deadline.
    pub async fn call(&mut self, request: &Request) -> Result<Response, ClientError> {
        let payload = serde_json::to_vec(request).map_err(|_| ClientError::Protocol)?;
        tokio::time::timeout(READ_TIMEOUT, async {
            write_frame(&mut self.stream, &payload).await?;
            let frame = read_frame(&mut self.stream, READ_TIMEOUT).await?;
            let text = String::from_utf8(frame).map_err(|_| ClientError::Protocol)?;
            serde_json::from_str::<Response>(&text).map_err(|_| ClientError::Protocol)
        })
        .await
        .map_err(|_| ClientError::Protocol)?
    }

    /// Map a daemon `error` response into [`ClientError::Server`].
    fn lift<T>(
        response: Response,
        extract: impl FnOnce(Response) -> Option<T>,
    ) -> Result<T, ClientError> {
        match response {
            Response::Error(body) => Err(ClientError::Server(body)),
            other => extract(other).ok_or(ClientError::Protocol),
        }
    }

    pub async fn status(&mut self) -> Result<crate::protocol::Status, ClientError> {
        let response = self
            .call(&Request::Status(crate::protocol::NoArgs {}))
            .await?;
        Self::lift(response, |response| match response {
            Response::Status(status) => Some(status),
            _ => None,
        })
    }

    pub async fn settings_get(&mut self) -> Result<crate::settings::Settings, ClientError> {
        let response = self
            .call(&Request::SettingsGet(crate::protocol::NoArgs {}))
            .await?;
        Self::lift(response, |response| match response {
            Response::Settings(settings) => Some(settings),
            _ => None,
        })
    }

    pub async fn settings_update(
        &mut self,
        patch: &crate::settings::SettingsPatch,
    ) -> Result<crate::settings::Settings, ClientError> {
        let response = self.call(&Request::SettingsUpdate(patch.clone())).await?;
        Self::lift(response, |response| match response {
            Response::Settings(settings) => Some(settings),
            _ => None,
        })
    }

    pub async fn layouts_list(&mut self) -> Result<Vec<crate::protocol::LayoutInfo>, ClientError> {
        let response = self
            .call(&Request::LayoutsList(crate::protocol::NoArgs {}))
            .await?;
        Self::lift(response, |response| match response {
            Response::Layouts(layouts) => Some(layouts),
            _ => None,
        })
    }

    pub async fn words_list(
        &mut self,
        query: &crate::protocol::WordsListQuery,
    ) -> Result<crate::protocol::WordsPage, ClientError> {
        let response = self.call(&Request::WordsList(query.clone())).await?;
        Self::lift(response, |response| match response {
            Response::Words(page) => Some(page),
            _ => None,
        })
    }

    pub async fn words_add(
        &mut self,
        add: &crate::protocol::WordAdd,
    ) -> Result<crate::words::Word, ClientError> {
        let response = self.call(&Request::WordsAdd(add.clone())).await?;
        Self::lift(response, |response| match response {
            Response::Word(word) => Some(word),
            _ => None,
        })
    }

    /// Add the last committed (roman, output) pair as a custom mapping.
    /// Render `roman` as Bangla candidates for the add-word panel. The list is
    /// advisory: the user can edit before saving.
    pub async fn words_preview(&mut self, roman: &str) -> Result<Vec<String>, ClientError> {
        let response = self
            .call(&Request::WordsPreview(crate::protocol::WordsPreview {
                roman: roman.to_owned(),
            }))
            .await?;
        Self::lift(response, |response| match response {
            Response::Preview(candidates) => Some(candidates.words),
            _ => None,
        })
    }
    pub async fn words_add_last(&mut self) -> Result<crate::words::Word, ClientError> {
        let response = self
            .call(&Request::WordsAddLast(crate::protocol::NoArgs {}))
            .await?;
        Self::lift(response, |response| match response {
            Response::Word(word) => Some(word),
            _ => None,
        })
    }

    pub async fn words_remove(&mut self, id: u64) -> Result<bool, ClientError> {
        let response = self
            .call(&Request::WordsRemove(crate::protocol::WordRemove { id }))
            .await?;
        Self::lift(response, |response| match response {
            Response::Removed(result) => Some(result.removed),
            _ => None,
        })
    }

    pub async fn words_import(
        &mut self,
        import: &crate::protocol::WordsImport,
    ) -> Result<crate::protocol::WordsImported, ClientError> {
        let response = self.call(&Request::WordsImport(import.clone())).await?;
        Self::lift(response, |response| match response {
            Response::Import(result) => Some(result),
            _ => None,
        })
    }

    pub async fn words_export(
        &mut self,
        format: crate::protocol::ImportFormat,
    ) -> Result<crate::protocol::WordsExported, ClientError> {
        let response = self
            .call(&Request::WordsExport(crate::protocol::WordsExport {
                format,
            }))
            .await?;
        Self::lift(response, |response| match response {
            Response::Export(result) => Some(result),
            _ => None,
        })
    }

    pub async fn insert_text(&mut self, text: &str) -> Result<bool, ClientError> {
        let response = self
            .call(&Request::TextInsert(crate::protocol::TextInsert {
                text: text.to_owned(),
            }))
            .await?;
        Self::lift(response, |response| match response {
            Response::Inserted(result) => Some(result.inserted),
            _ => None,
        })
    }

    pub async fn set_capture(
        &mut self,
        paused: bool,
    ) -> Result<crate::protocol::Status, ClientError> {
        let response = self
            .call(&Request::CaptureSet(crate::protocol::CaptureSet { paused }))
            .await?;
        Self::lift(response, |response| match response {
            Response::Status(status) => Some(status),
            _ => None,
        })
    }

    /// Ask the daemon to shut down cleanly (tray Exit equivalent). The daemon
    /// replies before its loop exits.
    pub async fn shutdown(&mut self) -> Result<(), ClientError> {
        let response = self
            .call(&Request::Shutdown(crate::protocol::NoArgs {}))
            .await?;
        Self::lift(response, |response| match response {
            Response::Ok(_) => Some(()),
            _ => None,
        })
    }

    /// `ai.complete`: validate and enqueue, then return the new job id. The
    /// daemon answers immediately; the provider result is polled with
    /// [`Client::ai_poll`] (ADR 0018 §5).
    pub async fn ai_complete(
        &mut self,
        params: &crate::protocol::AiCompleteParams,
    ) -> Result<crate::protocol::AiJobStarted, ClientError> {
        let response = self.call(&Request::AiComplete(params.clone())).await?;
        Self::lift(response, |response| match response {
            Response::AiJobStarted(started) => Some(started),
            _ => None,
        })
    }

    /// `ai.poll`: the current state of one AI job.
    pub async fn ai_poll(
        &mut self,
        job_id: u64,
    ) -> Result<crate::protocol::AiJobState, ClientError> {
        let response = self
            .call(&Request::AiPoll(crate::protocol::AiPollParams { job_id }))
            .await?;
        Self::lift(response, |response| match response {
            Response::AiJob(state) => Some(state),
            _ => None,
        })
    }

    /// `ai.cancel`: request cancellation. `Ok` when the job is cancelled (or
    /// was already finished); failures arrive as [`ClientError::Server`].
    pub async fn ai_cancel(&mut self, job_id: u64) -> Result<(), ClientError> {
        let response = self
            .call(&Request::AiCancel(crate::protocol::AiCancelParams {
                job_id,
            }))
            .await?;
        Self::lift(response, |response| match response {
            Response::Ok(_) => Some(()),
            _ => None,
        })
    }

    /// `ai.status`: policy and circuit-breaker state, never prompt or key
    /// material (ADR 0018 §4).
    pub async fn ai_status(&mut self) -> Result<crate::protocol::AiStatusInfo, ClientError> {
        let response = self
            .call(&Request::AiStatus(crate::protocol::NoArgs {}))
            .await?;
        Self::lift(response, |response| match response {
            Response::AiStatus(status) => Some(status),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::MAX_FRAME_BYTES;
    use crate::protocol::NoArgs;
    use tokio::io::AsyncWriteExt as _;

    /// Fixed handler for transport tests: answers status only.
    struct StubHandler;

    impl Handler for StubHandler {
        fn handle(&self, request: Request) -> Response {
            match request {
                Request::Status(_) => Response::Status(crate::protocol::Status {
                    daemon_version: "test".to_owned(),
                    paused: false,
                    language: "bn".to_owned(),
                    layout: "avro".to_owned(),
                    input_mode: crate::settings::InputMode::Bangla,
                    output_encoding: crate::settings::OutputEncoding::Unicode,
                    popup_visible: false,
                    words_total: 0,
                    learning_enabled: true,
                    autostart: false,
                }),
                Request::AiComplete(_) => Response::error(ErrorCode::Unsupported),
                _ => Response::error(ErrorCode::UnknownMethod),
            }
        }
    }

    /// Start a server on a unique test pipe, run `test`, then shut down.
    /// Each test gets its own pipe name so test binaries and threads never
    /// share the production pipe.
    async fn with_server<F>(test: F)
    where
        F: AsyncFnOnce(Token, String, Arc<Server>),
    {
        with_custom_server(StubHandler, REQUEST_DEADLINE, test).await;
    }

    async fn with_custom_server<H, F>(handler: H, deadline: std::time::Duration, test: F)
    where
        H: Handler,
        F: AsyncFnOnce(Token, String, Arc<Server>),
    {
        use std::sync::atomic::{AtomicU64 as Counter, Ordering as Memory};
        static COUNTER: Counter = Counter::new(0);
        let pipe_name = format!(
            r"\\.\pipe\okkhor-ipc-ut-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Memory::Relaxed)
        );
        let token = Token::generate().expect("entropy");
        let server = Arc::new(
            Server::new(Arc::new(handler), token.clone(), "test")
                .with_pipe_name(pipe_name.clone())
                .with_request_deadline(deadline),
        );
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn({
            let server = server.clone();
            async move { server.run(shutdown_rx).await }
        });
        // Give the listener a moment to create the first instance.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        test(token, pipe_name, server).await;
        let _ = shutdown_tx.send(());
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), task).await;
    }

    /// Handler that sleeps before answering, to exercise the deadline path.
    struct SlowHandler {
        delay: std::time::Duration,
        slow_once: std::sync::atomic::AtomicBool,
    }

    impl Handler for SlowHandler {
        fn handle(&self, request: Request) -> Response {
            if self
                .slow_once
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                std::thread::sleep(self.delay);
            }
            StubHandler.handle(request)
        }
    }

    #[tokio::test]
    async fn hello_handshake_and_status() {
        with_server(async |token: Token, pipe: String, _server: Arc<Server>| {
            let (mut client, greeting) = Client::connect_to_with_token(&pipe, &token.hex())
                .await
                .expect("handshake");
            assert_eq!(greeting.protocol, PROTOCOL_VERSION);
            let status = client.status().await.expect("status");
            assert_eq!(status.language, "bn");
        })
        .await;
    }

    /// AI handler for the transport round-trip: proves `dispatch` no longer
    /// short-circuits `ai.complete` before the handler (ADR 0018 §5).
    struct AiHandler;

    impl Handler for AiHandler {
        fn handle(&self, request: Request) -> Response {
            match request {
                Request::AiComplete(params) if params.prompt == "x" => {
                    Response::AiJobStarted(crate::protocol::AiJobStarted { job_id: 9 })
                }
                Request::AiComplete(_) => Response::error(ErrorCode::InvalidRequest),
                Request::AiPoll(poll) if poll.job_id == 9 => {
                    Response::AiJob(crate::protocol::AiJobState::Pending)
                }
                Request::AiPoll(_) => Response::error(ErrorCode::InvalidRequest),
                Request::AiCancel(cancel) if cancel.job_id == 9 => Response::Ok(NoArgs {}),
                Request::AiCancel(_) => Response::error(ErrorCode::InvalidRequest),
                Request::AiStatus(_) => Response::AiStatus(crate::protocol::AiStatusInfo {
                    enabled: true,
                    provider: "mock".to_owned(),
                    key_present: false,
                    breaker_state: "closed".to_owned(),
                    active_jobs: 1,
                }),
                _ => Response::error(ErrorCode::UnknownMethod),
            }
        }
    }

    #[tokio::test]
    async fn ai_complete_still_answers_stub_unsupported() {
        // The 0.5 answer is now produced by the handler, not by `dispatch`.
        with_server(async |token: Token, pipe: String, _server: Arc<Server>| {
            let (mut client, _) = Client::connect_to_with_token(&pipe, &token.hex())
                .await
                .expect("handshake");
            let error = client
                .ai_complete(&crate::protocol::AiCompleteParams {
                    prompt: "x".to_owned(),
                    system: None,
                    model: None,
                    max_tokens: None,
                    temperature: None,
                    timeout_ms: None,
                })
                .await
                .expect_err("stub answers Unsupported");
            assert!(matches!(
                error,
                ClientError::Server(body) if body.code == ErrorCode::Unsupported
            ));
        })
        .await;
    }

    #[tokio::test]
    async fn ai_job_methods_reach_the_handler() {
        with_custom_server(
            AiHandler,
            REQUEST_DEADLINE,
            async |token: Token, pipe: String, _server: Arc<Server>| {
                let (mut client, _) = Client::connect_to_with_token(&pipe, &token.hex())
                    .await
                    .expect("handshake");
                let started = client
                    .ai_complete(&crate::protocol::AiCompleteParams {
                        prompt: "x".to_owned(),
                        system: None,
                        model: None,
                        max_tokens: None,
                        temperature: None,
                        timeout_ms: None,
                    })
                    .await
                    .expect("ai.complete");
                assert_eq!(started.job_id, 9);
                assert_eq!(
                    client.ai_poll(9).await.expect("ai.poll"),
                    crate::protocol::AiJobState::Pending
                );
                client.ai_cancel(9).await.expect("ai.cancel");
                let status = client.ai_status().await.expect("ai.status");
                assert!(status.enabled);
                assert_eq!(status.active_jobs, 1);
                // A mismatched job id is a typed error, not a connection leak.
                let error = client.ai_poll(10).await.expect_err("unknown job");
                assert!(matches!(
                    error,
                    ClientError::Server(body) if body.code == ErrorCode::InvalidRequest
                ));
            },
        )
        .await;
    }

    #[tokio::test]
    async fn bad_token_rejected() {
        with_server(async |_token: Token, pipe: String, server: Arc<Server>| {
            let wrong = Token::generate().expect("entropy");
            let mut client = Client::connect_to(&pipe).await.expect("connects");
            let error = client.hello(&wrong.hex()).await.expect_err("rejected");
            assert!(matches!(
                error,
                ClientError::Server(body) if body.code == ErrorCode::Unauthorized
            ));
            assert_eq!(server.auth_failures(), 1);
        })
        .await;
    }

    #[tokio::test]
    async fn wrong_protocol_rejected() {
        with_server(async |token: Token, pipe: String, server: Arc<Server>| {
            let mut client = Client::connect_to(&pipe).await.expect("connects");
            let bad = Request::Hello(crate::protocol::Hello {
                protocol: PROTOCOL_VERSION + 1,
                token: token.hex(),
            });
            let response = client.call(&bad).await.expect("error frame");
            assert_eq!(
                response.error_body().map(|body| body.code),
                Some(ErrorCode::ProtocolMismatch)
            );
            assert_eq!(server.auth_failures(), 1);
        })
        .await;
    }

    #[tokio::test]
    async fn unknown_method_rejected() {
        with_server(async |token: Token, pipe: String, _server: Arc<Server>| {
            let (mut client, _) = Client::connect_to_with_token(&pipe, &token.hex())
                .await
                .expect("handshake");
            // Bypass the typed `Request` to send a genuinely unknown method.
            let payload = br#"{"nope":{}}"#;
            write_frame(&mut client.stream, payload)
                .await
                .expect("writes");
            let frame = read_frame(&mut client.stream, READ_TIMEOUT)
                .await
                .expect("reads");
            let response: Response = serde_json::from_slice(&frame).expect("error response");
            assert_eq!(
                response.error_body().map(|body| body.code),
                Some(ErrorCode::UnknownMethod)
            );
        })
        .await;
    }

    #[tokio::test]
    async fn non_hello_first_rejected() {
        with_server(async |_token: Token, pipe: String, server: Arc<Server>| {
            let mut client = Client::connect_to(&pipe).await.expect("connects");
            let response = client
                .call(&Request::Status(NoArgs {}))
                .await
                .expect("error frame");
            assert_eq!(
                response.error_body().map(|body| body.code),
                Some(ErrorCode::InvalidRequest)
            );
            assert_eq!(server.auth_failures(), 1);
        })
        .await;
    }

    #[tokio::test]
    async fn oversized_frame_rejected() {
        with_server(async |token: Token, pipe: String, _server: Arc<Server>| {
            let (mut client, _) = Client::connect_to_with_token(&pipe, &token.hex())
                .await
                .expect("handshake");
            let big = vec![b'x'; MAX_FRAME_BYTES + 1];
            // Length prefix only; `write_frame` refuses to send the body.
            client
                .stream
                .write_u32_le(big.len() as u32)
                .await
                .expect("writes len");
            client
                .stream
                .write_all(&big[..1024])
                .await
                .expect("writes part");
            let frame = read_frame(&mut client.stream, READ_TIMEOUT)
                .await
                .expect("reads");
            let response: Response = serde_json::from_slice(&frame).expect("error response");
            assert_eq!(
                response.error_body().map(|body| body.code),
                Some(ErrorCode::FrameTooLarge)
            );
        })
        .await;
    }

    #[tokio::test]
    async fn slow_handler_times_out_and_connection_survives() {
        with_custom_server(
            SlowHandler {
                delay: std::time::Duration::from_millis(500),
                slow_once: std::sync::atomic::AtomicBool::new(true),
            },
            std::time::Duration::from_millis(50),
            async |token: Token, pipe: String, _server: Arc<Server>| {
                let (mut client, _) = Client::connect_to_with_token(&pipe, &token.hex())
                    .await
                    .expect("handshake");
                // First request exceeds the 50 ms test deadline.
                let error = client.status().await.expect_err("times out");
                assert!(
                    matches!(
                        &error,
                        ClientError::Server(body) if body.code == ErrorCode::Timeout
                    ),
                    "expected typed Timeout, got {error:?}"
                );
                // The connection stays usable: the next request runs on a
                // fresh blocking thread and answers immediately.
                let status = client.status().await.expect("recovers");
                assert_eq!(status.language, "bn");
            },
        )
        .await;
    }

    #[test]
    fn auth_failure_counter_exact_under_concurrency() {
        let counter = Arc::new(AtomicU64::new(0));
        std::thread::scope(|scope| {
            for _ in 0..16 {
                scope.spawn(|| {
                    for _ in 0..1000 {
                        fail(&counter);
                    }
                });
            }
        });
        assert_eq!(counter.load(Ordering::SeqCst), 16_000);
        // Saturation pins at u64::MAX instead of wrapping.
        let saturated = AtomicU64::new(u64::MAX - 1);
        fail(&saturated);
        fail(&saturated);
        fail(&saturated);
        assert_eq!(saturated.load(Ordering::SeqCst), u64::MAX);
    }

    #[tokio::test]
    async fn over_capacity_answers_internal() {
        with_server(async |token: Token, pipe: String, _server: Arc<Server>| {
            let mut holders = Vec::new();
            for _ in 0..MAX_CLIENTS {
                let (client, _) = Client::connect_to_with_token(&pipe, &token.hex())
                    .await
                    .expect("holder connects");
                holders.push(client);
            }
            // One client past the cap: accepted, answered Internal, closed.
            let mut extra = Client::connect_to(&pipe).await.expect("connects");
            let error = extra.hello(&token.hex()).await.expect_err("busy");
            assert!(
                matches!(
                    &error,
                    ClientError::Server(body) if body.code == ErrorCode::Internal
                ),
                "expected typed Internal, got {error:?}"
            );
            drop(holders);
        })
        .await;
    }
}

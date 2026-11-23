//! Tokio named-pipe transport (Windows only, ADR 0012 §2).
//!
//! Pipe: `\\.\pipe\okkhor-ipc-v1`, one instance per client, max 4 concurrent
//! clients. Every connection starts with a `hello` handshake (protocol check
//! plus constant-time token compare); failures send a typed error, close the
//! connection, and bump a saturating counter. Requests run under a 5 s
//! deadline; inbound reads time out after 10 s.
//!
//! Tokio's `ServerOptions` cannot carry a security descriptor, so each pipe
//! instance is hardened with the owner-only DACL from [`crate::sddl`]
//! immediately after creation, before it is connected.

use std::os::windows::io::AsRawHandle as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};
use tokio::sync::Semaphore;
use windows::Win32::Foundation::HANDLE;

use crate::auth::Token;
use crate::error::ErrorCode;
use crate::frame::{FrameError, read_frame, write_frame};
use crate::protocol::{
    MAX_CLIENTS, PIPE_NAME, PROTOCOL_VERSION, READ_TIMEOUT, REQUEST_DEADLINE, Request, Response,
    ServerHello,
};
use crate::sddl::apply_owner_only_dacl;

/// Transport failures. None carry user text, tokens, or paths.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("pipe I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("framing failed: {0}")]
    Frame(#[from] FrameError),
    #[error("security hardening failed")]
    Hardening,
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
            let instance = ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                // Request WRITE_DAC so the owner-only DACL can be applied to
                // the fresh handle immediately after creation.
                .write_dac(true)
                .create(pipe_name)?;
            first = false;
            // Harden before any client can use the instance.
            let hardened = apply_owner_only_dacl(HANDLE(instance.as_raw_handle().cast()))
                .map_err(|_| TransportError::Hardening);
            if let Err(error) = hardened {
                let _ = instance.disconnect();
                return Err(error);
            }
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
        // At capacity: close immediately instead of queueing (bounded).
        let Ok(permit) = self.slots.clone().try_acquire_owned() else {
            return;
        };
        let handler = self.handler.clone();
        let token = self.token.clone();
        let version = self.daemon_version.clone();
        let failures = self.auth_failures.clone();
        tokio::spawn(async move {
            let _permit = permit;
            serve_connection(stream, handler, &token, &version, &failures).await;
        });
    }
}

fn fail(counter: &AtomicU64) {
    counter.fetch_max(
        counter.load(Ordering::Relaxed).saturating_add(1),
        Ordering::Relaxed,
    );
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
        let response = dispatch(handler.clone(), request).await;
        let Ok(payload) = serde_json::to_vec(&response) else {
            send_error(&mut stream, ErrorCode::Internal).await;
            return;
        };
        if write_frame(&mut stream, &payload).await.is_err() {
            return;
        }
    }
}

/// Run the handler under the 5 s deadline on a blocking thread.
/// `ai.complete` always answers [`ErrorCode::Unsupported`] in 0.5.
async fn dispatch(handler: Arc<dyn Handler>, request: Request) -> Response {
    if matches!(request, Request::AiComplete(_)) {
        return Response::error(ErrorCode::Unsupported);
    }
    let worker = tokio::task::spawn_blocking(move || handler.handle(request));
    match tokio::time::timeout(REQUEST_DEADLINE, worker).await {
        Ok(Ok(response)) => response,
        Ok(Err(_)) => Response::error(ErrorCode::Internal),
        Err(_) => Response::error(ErrorCode::Timeout),
    }
}

/// IPC client used by the daemon tests and the Tauri Rust side (Task 4).
pub struct Client {
    stream: tokio::net::windows::named_pipe::NamedPipeClient,
}

impl Client {
    /// Connect to the daemon pipe ([`PIPE_NAME`]).
    pub async fn connect() -> Result<Self, ClientError> {
        Self::connect_to(PIPE_NAME).await
    }

    /// Connect to a custom pipe name (tests only).
    pub async fn connect_to(pipe_name: &str) -> Result<Self, ClientError> {
        let stream = ClientOptions::new().open(pipe_name)?;
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

    pub async fn ai_complete(
        &mut self,
        params: serde_json::Value,
    ) -> Result<Response, ClientError> {
        self.call(&Request::AiComplete(params)).await
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
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let pipe_name = format!(
            r"\\.\pipe\okkhor-ipc-ut-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let token = Token::generate().expect("entropy");
        let server = Arc::new(
            Server::new(Arc::new(StubHandler), token.clone(), "test")
                .with_pipe_name(pipe_name.clone()),
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
}

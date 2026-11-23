//! Live OpenAI chat-completions provider.
//!
//! Built only for the non-default `http` feature so the default build stays
//! offline (ADR 0018 §3). The provider holds the API key in memory, sends it
//! only in the `Authorization` header, and never logs or embeds the key, prompt,
//! or response text in an error.

use std::fmt;
use std::time::{Duration, Instant};

use reqwest::Url;
use reqwest::blocking::Client;
use reqwest::header::AUTHORIZATION;
use serde_json::{Value, json};

use crate::cancel::CancellationToken;
use crate::error::AiError;
use crate::provider::AiProvider;
use crate::request::{AiRequest, AiResponse, AiUsage};

/// Fallback deadline when a request does not carry its own timeout.
const FALLBACK_TIMEOUT: Duration = Duration::from_secs(15);

/// Connection settings for [`OpenAiProvider`].
///
/// [`Debug`] never prints `api_key`; it shows a fixed `<redacted>` marker
/// instead. The key is supplied from the process environment by the caller and
/// is not persisted by this crate.
pub struct OpenAiConfig {
    /// Bearer token for the provider. Never logged or persisted here.
    pub api_key: String,
    /// Base URL without the `/chat/completions` suffix, for example
    /// `https://api.openai.com/v1`.
    pub base_url: String,
    /// Default model id, for example `gpt-6-luna`. A request may name another
    /// model; the policy allowlist decides whether that is accepted.
    pub model: String,
}

impl OpenAiConfig {
    /// Build a configuration from owned or borrowed strings.
    pub fn new(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: base_url.into(),
            model: model.into(),
        }
    }
}

impl fmt::Debug for OpenAiConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiConfig")
            .field("api_key", &"<redacted>")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .finish()
    }
}

/// OpenAI provider backed by the blocking `reqwest` client.
///
/// One request maps to `POST {base_url}/chat/completions`. The response is read
/// as `choices[0].message.content` plus `usage.prompt_tokens` and
/// `usage.completion_tokens`. The content is trimmed, and missing or empty
/// content is treated as invalid. Every failure is a typed [`AiError`] without
/// key, prompt, or response text.
pub struct OpenAiProvider {
    config: OpenAiConfig,
    endpoint: Url,
    client: Client,
}

impl OpenAiProvider {
    /// Validate the configuration and build the HTTP client.
    ///
    /// Rejects a blank key ([`AiError::KeyMissing`]), a blank model or invalid
    /// URL ([`AiError::InvalidRequest`]), and a non-HTTPS URL unless it points
    /// at loopback (`127.0.0.1` or `localhost`) for local tests.
    pub fn new(config: OpenAiConfig) -> Result<Self, AiError> {
        if config.api_key.trim().is_empty() {
            return Err(AiError::KeyMissing);
        }
        if config.model.trim().is_empty() {
            return Err(AiError::InvalidRequest);
        }

        let endpoint = chat_completions_url(&config.base_url)?;
        let client = Client::builder()
            .timeout(FALLBACK_TIMEOUT)
            .build()
            .map_err(|_| AiError::Internal)?;

        Ok(Self {
            config,
            endpoint,
            client,
        })
    }

    /// Configuration, with the key redacted by [`OpenAiConfig`]'s `Debug`.
    pub fn config(&self) -> &OpenAiConfig {
        &self.config
    }

    fn authorization_header(&self) -> String {
        format!("Bearer {}", self.config.api_key.trim())
    }
}

impl fmt::Debug for OpenAiProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiProvider")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl AiProvider for OpenAiProvider {
    fn name(&self) -> &'static str {
        "openai"
    }

    fn complete(
        &self,
        request: &AiRequest,
        cancel: &CancellationToken,
    ) -> Result<AiResponse, AiError> {
        cancel.check()?;

        let started = Instant::now();
        let response = self
            .client
            .post(self.endpoint.clone())
            .header(AUTHORIZATION, self.authorization_header())
            .timeout(request.timeout)
            .json(&chat_request_body(request))
            .send();

        let response = match response {
            Ok(response) => response,
            Err(error) if error.is_timeout() => return Err(AiError::Timeout),
            Err(_) => return Err(AiError::Internal),
        };

        let status = response.status();
        if matches!(status.as_u16(), 401 | 403) {
            return Err(AiError::KeyMissing);
        }
        if !status.is_success() {
            return Err(AiError::Provider {
                status: status.as_u16(),
            });
        }

        let body = response.text().map_err(|_| AiError::Internal)?;
        cancel.check()?;

        let parsed: Value = serde_json::from_str(&body).map_err(|_| AiError::InvalidResponse)?;

        let text = parsed
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .ok_or(AiError::InvalidResponse)?;

        let model = parsed
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or(&request.model);

        Ok(AiResponse {
            text: text.to_owned(),
            model: model.to_owned(),
            usage: AiUsage {
                input_tokens: token_count(&parsed, "prompt_tokens"),
                output_tokens: token_count(&parsed, "completion_tokens"),
            },
            latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        })
    }
}

/// Parse and validate a base URL, then append the chat-completions path.
///
/// Plain HTTP is accepted only for loopback so a test can use a local socket; a
/// remote base URL must be HTTPS because the key and prompt travel in the
/// request.
fn chat_completions_url(raw: &str) -> Result<Url, AiError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AiError::InvalidRequest);
    }

    let mut url = Url::parse(trimmed).map_err(|_| AiError::InvalidRequest)?;
    match url.scheme() {
        "https" => {}
        "http" => {
            let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost"));
            if !loopback {
                return Err(AiError::InvalidRequest);
            }
        }
        _ => return Err(AiError::InvalidRequest),
    }
    if url.cannot_be_a_base() || url.host().is_none() {
        return Err(AiError::InvalidRequest);
    }

    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| AiError::InvalidRequest)?;
        segments.pop_if_empty();
        segments.push("chat").push("completions");
    }
    Ok(url)
}

/// Chat-completions request body. Contains the prompt, so it is never logged.
///
/// The crate-internal [`AiRequest::max_tokens`] field maps to the wire field
/// `max_completion_tokens`; OpenAI deprecated `max_tokens` for chat completions
/// and reasoning models reject it.
fn chat_request_body(request: &AiRequest) -> Value {
    let mut messages = Vec::with_capacity(2);
    if let Some(system) = request.system.as_deref() {
        messages.push(json!({ "role": "system", "content": system }));
    }
    messages.push(json!({ "role": "user", "content": request.prompt }));

    json!({
        "model": request.model,
        "messages": messages,
        "max_completion_tokens": request.max_tokens,
        "temperature": request.temperature,
    })
}

fn token_count(parsed: &Value, field: &str) -> u32 {
    parsed
        .get("usage")
        .and_then(|usage| usage.get(field))
        .and_then(Value::as_u64)
        .map_or(0, |count| u32::try_from(count).unwrap_or(u32::MAX))
}

#[cfg(all(test, feature = "http"))]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::thread;

    use super::*;

    /// One HTTP request as received by the fake provider.
    #[derive(Debug)]
    struct Captured {
        method: String,
        path: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    impl Captured {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.as_str())
        }

        fn json(&self) -> Value {
            serde_json::from_slice(&self.body).expect("request body is JSON")
        }
    }

    /// Single-connection HTTP server on loopback. No real network is used.
    struct TestServer {
        base_url: String,
        captured: mpsc::Receiver<Captured>,
        thread: thread::JoinHandle<()>,
    }

    impl TestServer {
        fn start(delay: Duration, status_line: &str, body: &str) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake provider");
            let address = listener.local_addr().expect("fake provider address");
            let (sender, captured) = mpsc::channel();
            let status_line = String::from(status_line);
            let body = String::from(body);

            let thread = thread::spawn(move || {
                let (stream, _peer) = listener.accept().expect("accept one request");
                let mut reader =
                    BufReader::new(stream.try_clone().expect("clone fake provider stream"));
                let request = read_request(&mut reader);
                let _ = sender.send(request);

                if !delay.is_zero() {
                    thread::sleep(delay);
                }

                let response = format!(
                    "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let mut stream = stream;
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            });

            Self {
                base_url: format!("http://{address}"),
                captured,
                thread,
            }
        }

        fn wait(self) -> Captured {
            let captured = self.captured.recv().expect("captured request");
            self.thread.join().expect("server thread");
            captured
        }
    }

    fn read_request(reader: &mut BufReader<TcpStream>) -> Captured {
        let mut request_line = String::new();
        reader.read_line(&mut request_line).expect("request line");
        let mut parts = request_line.split_whitespace();
        let method = String::from(parts.next().unwrap_or_default());
        let path = String::from(parts.next().unwrap_or_default());

        let mut headers = Vec::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("header line");
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if trimmed.is_empty() {
                break;
            }
            if let Some((name, value)) = trimmed.split_once(':') {
                headers.push((String::from(name.trim()), String::from(value.trim())));
            }
        }

        let length = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.parse::<usize>().ok())
            .unwrap_or(0);
        let mut body = vec![0u8; length];
        if length > 0 {
            reader.read_exact(&mut body).expect("request body");
        }

        Captured {
            method,
            path,
            headers,
            body,
        }
    }

    fn provider(base_url: &str) -> OpenAiProvider {
        OpenAiProvider::new(OpenAiConfig::new("sk-test-key", base_url, "gpt-6-luna"))
            .expect("valid provider")
    }

    fn request(prompt: &str) -> AiRequest {
        AiRequest::new(prompt, "gpt-6-luna")
    }

    #[test]
    fn success_maps_text_usage_and_request_body() {
        let server = TestServer::start(
            Duration::ZERO,
            "200 OK",
            r#"{"model":"gpt-6-luna","choices":[{"message":{"role":"assistant","content":"Better text."}}],"usage":{"prompt_tokens":11,"completion_tokens":4,"total_tokens":15}}"#,
        );
        let provider = provider(&format!("{}/v1", server.base_url));
        let request = request("original text").with_system("rewrite it");

        let response = provider
            .complete(&request, &CancellationToken::new())
            .expect("successful completion");

        assert_eq!(response.text, "Better text.");
        assert_eq!(response.model, "gpt-6-luna");
        assert_eq!(
            response.usage,
            AiUsage {
                input_tokens: 11,
                output_tokens: 4,
            }
        );

        let captured = server.wait();
        assert_eq!(captured.method, "POST");
        assert_eq!(captured.path, "/v1/chat/completions");
        assert!(
            captured
                .header("authorization")
                .is_some_and(|value| value.starts_with("Bearer "))
        );

        let body = captured.json();
        assert_eq!(body["model"], "gpt-6-luna");
        assert_eq!(body["max_completion_tokens"], 512);
        assert!(
            body.get("max_tokens").is_none(),
            "the deprecated max_tokens field must not be sent"
        );
        let temperature = body["temperature"].as_f64().expect("temperature");
        assert!((temperature - f64::from(0.2f32)).abs() < 1e-6);

        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "rewrite it");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "original text");
    }

    #[test]
    fn request_without_system_sends_only_the_user_message() {
        let server = TestServer::start(
            Duration::ZERO,
            "200 OK",
            r#"{"choices":[{"message":{"content":"ok"}}]}"#,
        );
        let provider = provider(&server.base_url);

        let response = provider
            .complete(&request("hello"), &CancellationToken::new())
            .expect("successful completion");
        assert_eq!(response.text, "ok");
        assert_eq!(response.model, "gpt-6-luna");
        assert_eq!(response.usage, AiUsage::default());

        let body = server.wait().json();
        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
    }

    #[test]
    fn unauthorized_and_forbidden_map_to_key_missing() {
        for status_line in ["401 Unauthorized", "403 Forbidden"] {
            let server = TestServer::start(Duration::ZERO, status_line, r#"{"error":"nope"}"#);
            let provider = provider(&server.base_url);

            assert_eq!(
                provider.complete(&request("hello"), &CancellationToken::new()),
                Err(AiError::KeyMissing),
                "status {status_line} should map to KeyMissing"
            );
            let _ = server.wait();
        }
    }

    #[test]
    fn other_non_success_maps_to_provider_status() {
        let server = TestServer::start(Duration::ZERO, "500 Internal Server Error", "boom");
        let provider = provider(&server.base_url);

        assert_eq!(
            provider.complete(&request("hello"), &CancellationToken::new()),
            Err(AiError::Provider { status: 500 })
        );
        let _ = server.wait();
    }

    #[test]
    fn invalid_json_maps_to_invalid_response() {
        let server = TestServer::start(Duration::ZERO, "200 OK", "not json");
        let provider = provider(&server.base_url);

        assert_eq!(
            provider.complete(&request("hello"), &CancellationToken::new()),
            Err(AiError::InvalidResponse)
        );
        let _ = server.wait();
    }

    #[test]
    fn missing_or_empty_content_maps_to_invalid_response() {
        for body in [
            r#"{"choices":[]}"#,
            r#"{"choices":[{"message":{}}]}"#,
            r#"{"choices":[{"message":{"content":"   "}}]}"#,
            "{}",
        ] {
            let server = TestServer::start(Duration::ZERO, "200 OK", body);
            let provider = provider(&server.base_url);

            assert_eq!(
                provider.complete(&request("hello"), &CancellationToken::new()),
                Err(AiError::InvalidResponse),
                "body {body} should be invalid"
            );
            let _ = server.wait();
        }
    }

    #[test]
    fn slow_provider_maps_to_timeout() {
        let server = TestServer::start(Duration::from_secs(1), "200 OK", "{}");
        let provider = provider(&server.base_url);

        let mut request = request("hello");
        request.timeout = Duration::from_millis(200);

        assert_eq!(
            provider.complete(&request, &CancellationToken::new()),
            Err(AiError::Timeout)
        );
    }

    #[test]
    fn refused_connection_maps_to_internal() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("reserve a port");
        let address = listener.local_addr().expect("reserved address");
        drop(listener);

        let provider = provider(&format!("http://{address}"));

        assert_eq!(
            provider.complete(&request("hello"), &CancellationToken::new()),
            Err(AiError::Internal)
        );
    }

    #[test]
    fn cancellation_before_the_call_skips_the_request() {
        // Nothing listens on port 1, so an attempted request would fail instead
        // of returning `Cancelled`.
        let provider = provider("http://127.0.0.1:1");
        let cancel = CancellationToken::new();
        cancel.cancel();

        assert_eq!(
            provider.complete(&request("hello"), &cancel),
            Err(AiError::Cancelled)
        );
    }

    #[test]
    fn config_and_provider_debug_redact_the_key() {
        let config =
            OpenAiConfig::new("sk-super-secret", "https://api.openai.com/v1", "gpt-6-luna");

        let config_debug = format!("{config:?}");
        assert!(!config_debug.contains("sk-super-secret"));
        assert!(config_debug.contains("<redacted>"));

        let provider = OpenAiProvider::new(config).expect("valid provider");
        assert_eq!(provider.name(), "openai");
        let provider_debug = format!("{provider:?}");
        assert!(!provider_debug.contains("sk-super-secret"));
        assert!(provider_debug.contains("<redacted>"));
    }

    #[test]
    fn new_rejects_blank_key_and_model() {
        assert_eq!(
            OpenAiProvider::new(OpenAiConfig::new(
                "  ",
                "https://api.openai.com/v1",
                "gpt-6-luna"
            ))
            .expect_err("blank key"),
            AiError::KeyMissing
        );
        assert_eq!(
            OpenAiProvider::new(OpenAiConfig::new(
                "sk-test",
                "https://api.openai.com/v1",
                " "
            ))
            .expect_err("blank model"),
            AiError::InvalidRequest
        );
    }

    #[test]
    fn new_rejects_remote_http_and_non_http_schemes() {
        for base_url in [
            "http://api.openai.com/v1",
            "http://example.test",
            "ftp://127.0.0.1/v1",
            "file:///tmp/v1",
            "",
            "not a url",
        ] {
            assert_eq!(
                OpenAiProvider::new(OpenAiConfig::new("sk-test", base_url, "gpt-6-luna"))
                    .expect_err("rejected base url"),
                AiError::InvalidRequest,
                "base_url {base_url:?} should be rejected"
            );
        }
    }

    #[test]
    fn new_accepts_https_and_loopback_http() {
        for base_url in [
            "https://api.openai.com/v1",
            "http://127.0.0.1:8080",
            "http://localhost:4321/v1/",
        ] {
            OpenAiProvider::new(OpenAiConfig::new("sk-test", base_url, "gpt-6-luna"))
                .expect("accepted base url");
        }
    }
}

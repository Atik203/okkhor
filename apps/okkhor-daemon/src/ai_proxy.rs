#![forbid(unsafe_code)]
//! Daemon-side AI job proxy.
//!
//! The daemon never calls a provider. It owns a bounded job map, lazily spawns
//! the isolated `okkhor-worker.exe`, forwards one request over the worker's
//! private pipe, and lets the UI poll the result. A worker failure or timeout
//! is a typed job failure; typing is untouched.

use std::collections::HashMap;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use okkhor_ipc::ErrorCode;
use okkhor_ipc::protocol::{
    AiCompleteParams, AiJobStarted, AiJobState, AiStatusInfo, Request, Response,
};

const DEFAULT_PIPE: &str = r"\\.\pipe\okkhor-ai-worker-v1";
const DEFAULT_MODEL: &str = "gpt-6-luna";
const WORKER_EXE: &str = "okkhor-worker.exe";
const SPAWN_BACKOFF: Duration = Duration::from_secs(1);
const MAX_JOBS: usize = 64;

/// One daemon-owned AI job proxy. Cheap to clone through [`Arc`].
pub struct AiProxy {
    enabled: AtomicBool,
    key: Option<String>,
    model: String,
    pipe: String,
    token: Mutex<Option<String>>,
    worker: Mutex<Option<Child>>,
    jobs: Mutex<HashMap<u64, AiJobState>>,
    next_job: AtomicU64,
    worker_path: Option<std::path::PathBuf>,
    last_spawn: Mutex<Option<Instant>>,
}

impl std::fmt::Debug for AiProxy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AiProxy")
            .field("enabled", &self.enabled.load(Ordering::SeqCst))
            .field("key_present", &self.key.is_some())
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl AiProxy {
    /// Build a proxy from the current settings and environment. The worker is
    /// spawned only on the first `complete` call.
    #[must_use]
    pub fn new(enabled: bool) -> Arc<Self> {
        let env_file = load_env_file();
        let key = setting(&env_file, "OKKHOR_OPENAI_API_KEY");
        let model =
            setting(&env_file, "OKKHOR_AI_MODEL").unwrap_or_else(|| DEFAULT_MODEL.to_owned());
        Arc::new(Self {
            enabled: AtomicBool::new(enabled),
            key,
            model,
            pipe: DEFAULT_PIPE.to_owned(),
            token: Mutex::new(None),
            worker: Mutex::new(None),
            jobs: Mutex::new(HashMap::new()),
            next_job: AtomicU64::new(0),
            worker_path: discover_worker(),
            last_spawn: Mutex::new(None),
        })
    }

    /// Whether AI is enabled in settings.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
    }

    /// Queue one request and return its job id immediately.
    pub fn complete(self: &Arc<Self>, params: AiCompleteParams) -> Result<AiJobStarted, ErrorCode> {
        if !self.enabled.load(Ordering::SeqCst) {
            return Err(ErrorCode::AiDisabled);
        }
        if self.key.is_none() {
            return Err(ErrorCode::AiKeyMissing);
        }
        params.validate()?;
        self.ensure_worker()?;

        let job_id = self.next_job.fetch_add(1, Ordering::SeqCst) + 1;
        {
            let mut jobs = lock(&self.jobs);
            jobs.insert(job_id, AiJobState::Pending);
            if jobs.len() > MAX_JOBS {
                jobs.retain(|_, state| matches!(state, AiJobState::Pending));
            }
        }

        let proxy = Arc::clone(self);
        std::thread::spawn(move || proxy.run_job(job_id, params));
        Ok(AiJobStarted { job_id })
    }

    /// Current state of one job.
    pub fn poll(&self, job_id: u64) -> AiJobState {
        lock(&self.jobs)
            .get(&job_id)
            .cloned()
            .unwrap_or(AiJobState::Failed {
                code: "unknown_job".to_owned(),
            })
    }

    /// Cancel a pending job. A completed result is left untouched.
    pub fn cancel(&self, job_id: u64) {
        let mut jobs = lock(&self.jobs);
        if matches!(jobs.get(&job_id), Some(AiJobState::Pending)) {
            jobs.insert(job_id, AiJobState::Cancelled);
        }
    }

    /// Text-free status for `ai.status`.
    pub fn status(&self) -> AiStatusInfo {
        let active_jobs = lock(&self.jobs)
            .values()
            .filter(|state| matches!(state, AiJobState::Pending))
            .count();
        AiStatusInfo {
            enabled: self.enabled.load(Ordering::SeqCst),
            provider: if self.key.is_some() {
                "openai".to_owned()
            } else {
                "disabled".to_owned()
            },
            key_present: self.key.is_some(),
            breaker_state: "closed".to_owned(),
            active_jobs: u32::try_from(active_jobs).unwrap_or(u32::MAX),
        }
    }

    /// Reap a worker that exited. Pending jobs fail; typing continues.
    pub fn reap(&self) {
        let mut worker = lock(&self.worker);
        let Some(child) = worker.as_mut() else {
            return;
        };
        if child.try_wait().ok().flatten().is_some() {
            *worker = None;
            *lock(&self.token) = None;
            let mut jobs = lock(&self.jobs);
            for state in jobs.values_mut() {
                if matches!(state, AiJobState::Pending) {
                    *state = AiJobState::Failed {
                        code: "WorkerDown".to_owned(),
                    };
                }
            }
        }
    }

    /// Kill the worker on daemon shutdown.
    pub fn shutdown(&self) {
        if let Some(mut child) = lock(&self.worker).take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        *lock(&self.token) = None;
    }

    fn ensure_worker(&self) -> Result<(), ErrorCode> {
        let mut worker = lock(&self.worker);
        if let Some(child) = worker.as_mut() {
            if child.try_wait().ok().flatten().is_none() {
                return Ok(());
            }
            *worker = None;
            *lock(&self.token) = None;
        }

        let mut last = lock(&self.last_spawn);
        if last.is_some_and(|at| at.elapsed() < SPAWN_BACKOFF) {
            return Err(ErrorCode::WorkerDown);
        }
        *last = Some(Instant::now());

        let Some(path) = self.worker_path.as_ref() else {
            return Err(ErrorCode::WorkerDown);
        };
        let token = okkhor_ipc::Token::generate().map_err(|_| ErrorCode::Internal)?;
        let token_hex = token.hex();
        let mut command = Command::new(path);
        command
            .env("OKKHOR_AI_TOKEN", &token_hex)
            .env("OKKHOR_AI_PIPE", &self.pipe)
            .env("OKKHOR_AI_MODEL", &self.model)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(key) = self.key.as_ref() {
            command.env("OKKHOR_OPENAI_API_KEY", key);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0000_0008 | 0x0800_0000); // DETACHED_PROCESS | CREATE_NO_WINDOW
        }

        let child = command.spawn().map_err(|_| ErrorCode::WorkerDown)?;
        *worker = Some(child);
        *lock(&self.token) = Some(token_hex);
        Ok(())
    }

    fn run_job(&self, job_id: u64, params: AiCompleteParams) {
        let result = self.request_worker(params);
        let mut jobs = lock(&self.jobs);
        if matches!(jobs.get(&job_id), Some(AiJobState::Pending)) {
            jobs.insert(job_id, result);
        }
    }

    #[cfg(windows)]
    fn request_worker(&self, params: AiCompleteParams) -> AiJobState {
        let Some(token) = lock(&self.token).clone() else {
            return AiJobState::Failed {
                code: "WorkerDown".to_owned(),
            };
        };
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(_) => {
                return AiJobState::Failed {
                    code: "Internal".to_owned(),
                };
            }
        };
        runtime.block_on(async {
            let mut client = match okkhor_ipc::transport::Client::connect_to_with_token(
                &self.pipe, &token,
            )
            .await
            {
                Ok((client, _)) => client,
                Err(_) => {
                    return AiJobState::Failed {
                        code: "WorkerDown".to_owned(),
                    };
                }
            };
            match client.call(&Request::AiComplete(params)).await {
                Ok(Response::AiJob(state)) => state,
                Ok(Response::Error(body)) => AiJobState::Failed {
                    code: format!("{:?}", body.code),
                },
                Ok(_) => AiJobState::Failed {
                    code: "InvalidResponse".to_owned(),
                },
                Err(_) => AiJobState::Failed {
                    code: "WorkerDown".to_owned(),
                },
            }
        })
    }

    #[cfg(not(windows))]
    fn request_worker(&self, _params: AiCompleteParams) -> AiJobState {
        AiJobState::Failed {
            code: "WorkerDown".to_owned(),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

fn discover_worker() -> Option<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("OKKHOR_WORKER_PATH") {
        return Some(std::path::PathBuf::from(path));
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let beside = dir.join(WORKER_EXE);
    if beside.is_file() {
        return Some(beside);
    }
    let bin = dir.join("bin").join(WORKER_EXE);
    bin.is_file().then_some(bin)
}

fn setting(env_file: &HashMap<String, String>, key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .or_else(|| env_file.get(key).cloned())
        .filter(|value| !value.trim().is_empty())
}

fn load_env_file() -> HashMap<String, String> {
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("OKKHOR_ENV_FILE") {
        candidates.push(std::path::PathBuf::from(path));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(".env"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(".env"));
            candidates.push(dir.join("..").join("..").join(".env"));
        }
    }
    for path in candidates {
        if let Ok(text) = std::fs::read_to_string(&path) {
            return parse_env(&text);
        }
    }
    HashMap::new()
}

fn parse_env(text: &str) -> HashMap<String, String> {
    let mut values = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        values.insert(
            key.to_owned(),
            value.trim().trim_matches('"').trim_matches('\'').to_owned(),
        );
    }
    values
}

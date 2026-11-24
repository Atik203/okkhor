# AI text actions (0.7)

Status: **implemented for 0.7**. The provider, isolated worker, daemon proxy,
IPC job protocol, and Settings panel are all present.
Binding rules: `AGENTS.md` §2, ADR 0002 (AI off the hot path), ADR 0018
(opt-in AI worker), `docs/security-privacy.md`.

## Scope

0.7 ships **one** explicit action: **Try a rewrite**.

- The user pastes or types text into the panel, reviews it, and clicks the
  button.
- The result is displayed for review. It is **never inserted automatically**.
- There is no passive capture of keystrokes, selections, clipboard contents, or
  window titles.
- Proofread and translate are deliberately out of scope until the worker and
  provider path are proven.

## Data flow

```text
Settings / panel (React)
        -> Tauri invoke
        -> daemon IPC ai.complete
        -> policy (opt-in, enabled, key present, queue space)
        -> { job_id } returned immediately
        -> apps\okkhor-worker (own pipe/token)
        -> OpenAI Responses/Chat API
UI polls ai.poll(job_id) -> pending | done(text) | error
UI may ai.cancel(job_id)
```

The keydown-to-character path never calls this code. A worker crash, timeout,
or provider outage cannot delay a keystroke.

## Provider and model

The provider is a trait. OpenAI is the first live implementation behind the
`http` Cargo feature; Gemini remains a future provider.

### Implementation status

| Piece | State |
| --- | --- |
| `crates/okkhor-ai` core (policy, cache, breaker, single-flight, cancel) | implemented |
| `MockProvider`, `DisabledProvider` | implemented, used by tests |
| `OpenAiProvider` (`http` feature) | implemented, offline-tested against a local fake server |
| `apps/okkhor-worker`, daemon IPC `ai.*` | implemented; worker smoke-tested live |
| Settings **Try a rewrite** panel | implemented, 6 UI tests |

- The `http` feature is **not** a default feature. The default build and CI
  stay offline; provider tests run with `cargo test -p okkhor-ai --features http`.
- Endpoint: `POST {base_url}/chat/completions`, with the base URL defaulting to
  `https://api.openai.com/v1`. The JSON body carries `model`, `messages`
  (optional `system`, then `user`), `max_completion_tokens`, and `temperature`.
  The crate-internal `AiRequest.max_tokens` limit maps to the wire field
  `max_completion_tokens`; the deprecated `max_tokens` field is never sent.
- The provider reads `choices[0].message.content` and
  `usage.prompt_tokens` / `usage.completion_tokens`.
- Plain HTTP is rejected for remote hosts; only `127.0.0.1` and `localhost` are
  allowed for the local fake server in tests.
- The transport is `reqwest` 0.12 (blocking client, rustls TLS, no default
  features; MIT or Apache-2.0), compiled only for the `http` feature. The client
  is constructed before the Tokio runtime starts, and each provider call runs on
  a plain OS thread so `reqwest::blocking` is never inside a Tokio context.
- `gpt-6-luna` is a reasoning model and accepts only the default temperature
  `1.0`; Okkhor sends that. `gpt-6.1-terra` can use the same value.
- Failures are typed and carry no key, prompt, or response text: 401/403 →
  `KeyMissing`, other non-success → `Provider { status }`, unparsable body or
  missing content → `InvalidResponse`, deadline → `Timeout`, other transport
  errors → `Internal`.

| Setting | Value |
| --- | --- |
| Key | `OKKHOR_OPENAI_API_KEY` (process environment only) |
| Model | `OKKHOR_AI_MODEL` |
| Default model | `gpt-6-luna` |
| Quality option | `gpt-6.1-terra` |
| Timeout | 15 s |
| In-flight | 1 |
| Cache | in-memory, 64 entries / 256 KiB |

Prices per 1M tokens, checked at
<https://developers.openai.com/api/docs/pricing> on 2026-11-24:

| Model | Input | Output | ~500 in + 300 out |
| --- | ---: | ---: | ---: |
| `gpt-6-luna` | $0.10 | $0.50 | ~$0.0002 |
| `gpt-6.1-terra` | $1.00 | $5.00 | ~$0.002 |
| `gpt-6.2-sol` | $2.00 | $10.00 | ~$0.004 |
| `gpt-6.2-astra` | $8.00 | $40.00 | ~$0.016 |

`gpt-6-luna` is the default because a short rewrite costs a fraction of a cent
and the action is user-initiated. `gpt-6.1-terra` is the quality option for
natural prose. A 20-sample Bangla/English bake-off decides whether Terra's
quality difference justifies 10x the price.

Prices change. Re-check the official page before quoting a cost, and never
hard-code a price in the UI.

## Key handling

- The key is read from the process environment; it is never written to
  `settings.json`, the learning store, logs, crash reports, or telemetry.
- `.env` is for local development only and is git-ignored. `.env.example` is
  the tracked template.
- The UI can report only "set" or "unset".
- A packaged build should use the user's environment; do not ship a key or a
  `.env` file.

## Privacy

- AI prompts and responses are **memory-only** in 0.7; nothing is written to
  disk.
- The cache key is a SHA-256 of system + prompt + model + prompt version +
  temperature + max_tokens; the prompt itself is not logged.
- No prompt, response, key, or window title appears in logs or errors.
- The provider is opt-in and disabled by default.
- Pause and the local dictionary remain the escape hatch.

## Failure modes

| Failure | Behavior |
| --- | --- |
| AI disabled | no worker spawn; panel explains opt-in |
| Key unset | explicit "key missing"; no request |
| Worker missing/crashed | typed `WorkerDown`; typing continues |
| Provider 429/500 | typed error; breaker opens after repeated failures |
| Timeout | typed timeout at 15 s; cancel is safe |
| Cancel | job ends; a late response is discarded |
| Invalid model | typed configuration error; fall back to the documented default only if the user has not chosen one |
| Network unavailable | typed error; local suggestions unaffected |

## IPC

- `ai.complete` validates and enqueues, returning `{ job_id }` immediately so
  the 5 s daemon request deadline is never exceeded.
- `ai.poll` returns pending/done/error; `ai.cancel` cancels; `ai.status` reports
  enabled, provider, key set/unset, circuit state, and active job without text.
- The UI never talks to the worker directly.

## Testing

- `MockProvider` and `DisabledProvider` for unit tests.
- A local fake HTTP server for provider success/error mapping.
- No real network in CI.
- A worker-kill test proves typing continues and the next call returns
  `WorkerDown`.
- A log inspection proves no prompt text.

## Open questions

- Does `gpt-6-luna` handle Bangla prose well enough, or is `gpt-6.1-terra`
  worth 10x for the rewrite action?
- Should the model id be exposed in Settings, or stay an environment variable
  for 0.7?
- Do we need a user-visible per-request cost estimate once usage grows?

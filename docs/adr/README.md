# Architecture decision records

| ADR | Title | Status |
| --- | --- | --- |
| [0001](0001-rust-workspace-and-process-boundaries.md) | Rust workspace and process boundaries | Accepted |
| [0002](0002-local-first-ai-off-hot-path.md) | Local-first typing and AI off the hot path | Accepted |
| [0003](0003-smriti-learning-and-privacy.md) | Smriti learning and privacy | Accepted |
| [0004](0004-ui-shell-and-native-popup.md) | UI shell and native suggestion popup | Accepted (amended by 0012) |
| [0005](0005-legacy-reuse-via-compatibility-layer.md) | Legacy reuse via a compatibility layer | Accepted |
| [0006](0006-windows-toolchain.md) | Windows toolchain and build targets | Accepted |
| [0007](0007-hybrid-global-local-learning-to-rank.md) | Hybrid global/local learning-to-rank | Accepted |
| [0008](0008-non-hook-benchmark-harness.md) | Non-hook benchmark harness | Accepted |
| [0009](0009-native-control-wm-char-fast-path.md) | Native-control WM_CHAR fast path | Accepted |
| [0010](0010-core-contracts.md) | Core contracts for the 0.3 multi-agent sprint | Accepted |
| [0011](0011-candidate-engine.md) | Candidate engine and 0.4 contracts | Accepted |
| [0012](0012-ui-ipc-and-settings.md) | UI process, IPC, settings ownership, encrypted store | Accepted |
| [0013](0013-encodings-and-fonts.md) | Bangla output encodings and bundled fonts | Accepted |
| [0014](0014-input-modes-hotkeys-fonts.md) | Input modes, one-key toggle, zero-config fonts | Accepted |
| [0015](0015-offline-replay-and-training.md) | Offline replay harness and feature schema v1 (`okkhor-train`) | Accepted |
| [0016](0016-suggestion-index-and-english-completion.md) | Suggestion index, English completion, and instant state | Accepted |
| [0017](0017-feature-runtime-and-hybrid-blend.md) | Feature runtime crate and hybrid reranker blend | Proposed |
| [0018](0018-opt-in-ai-worker.md) | Opt-in AI worker process boundary | Proposed |
| [0019](0019-ranker-injection-and-bounded-work.md) | Ranker injection and bounded hybrid work | Proposed |

An ADR is required for a new crate, process, protocol, storage engine, provider,
or a change to a hard performance/privacy rule.

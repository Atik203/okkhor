# Documentation index

| Document | Purpose |
| --- | --- |
| [Architecture](architecture.md) | system boundaries, hot path, crates, data flow |
| [Roadmap](roadmap.md) | phased plan and release gates |
| [Learning](learning.md) | Smriti personalization, ranking, privacy, evaluation |
| [Model roadmap](model-roadmap.md) | hybrid global/local learning-to-rank, training and rollout |
| [AI actions](ai.md) | opt-in rewrite worker, provider, pricing, privacy |
| [Performance](performance.md) | latency budgets and measurement protocol |
| [Security and privacy](security-privacy.md) | threat model and data controls |
| [Testing](testing.md) | test strategy, golden corpus, Windows matrix |
| [QA process](qa/README.md) | manual QA setup, evidence rules, privacy constraints |
| [0.2 Windows matrix](qa/0.2-matrix.md) | pass/fail matrix, release gate, defect template |
| [0.7 hardening matrix](qa/0.7-matrix.md) | IME/RDP/DPI/stress/accessibility/AI rows |
| [0.8 checklist](qa/0.8-checklist.md) | popup stability, raw fallback, import and signing gates |
| [0.8 parity matrix](parity/0.8-matrix.md) | legacy capability coverage and explicit gaps |
| [Compatibility](compatibility.md) | legacy assets, FFI reuse, migration |
| [Release](release.md) | versioning, signing, update and rollback |
| [0.8.0 release notes](releases/0.8.0.md) | what shipped, verification, limitations |
| [0.8.1 release notes](releases/0.8.1.md) | forward/backward suggestions, popup fast path, blank-commit guard |
| [0.9.0 release notes](releases/0.9.0.md) | all layouts working, fixed-key engine, popup and first-letter fixes |
| [0.9.1 release notes](releases/0.9.1.md) | English arrows/Tab, Bangla Tab, partial-commit recovery, tray fixes |
| [0.9.2 release notes](releases/0.9.2.md) | Bangla Tab swallow, stuck-key fix, English live preview |
| [0.9 QA checklist](qa/0.9-checklist.md) | per-layout real-key rows and automated gates |
| [Layout engine design](superpowers/specs/2026-11-24-fixed-layout-engine-design.md) | approved architecture for the fixed and alternate phonetic paths |
| [Layout engine plan](superpowers/plans/2026-11-24-fixed-layout-engine.md) | implementation tasks and verification |
| [0.9 learning plan](superpowers/plans/0.9-learning-and-next-word.md) | local user learning and next-word suggestions (0.9.x) |
| [Legacy layout evidence](superpowers/plans/0.8.1-layout-engine.md) | static findings from Avro, Kontho and Bongo |
| [0.2.0 release notes](releases/0.2.0.md) | what shipped, measured results, limitations |
| [Glossary](glossary.md) | project and Bengali terms |
| [ADRs](adr/) | accepted and proposed architecture decisions |

The root [AGENTS.md](../AGENTS.md) is the binding engineering contract. Read it
before changing code.

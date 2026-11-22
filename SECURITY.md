# Security policy

Akkhor is a keyboard utility and therefore handles highly sensitive input. We
take reports seriously.

## Reporting

Do not open a public issue for a vulnerability. Use the repository's private
security advisory feature, or contact the maintainer through the private channel
provided with the project. Include:

- affected version or commit;
- Windows version and build;
- reproduction steps;
- impact and any known mitigation;
- whether user text, credentials, or local data may have been exposed.

Do not include real passwords, personal messages, or other sensitive text in a
report. Use synthetic examples.

## Scope

In scope:

- keyboard hook and text injection behavior;
- local IPC and process boundaries;
- learning-data storage and encryption;
- update signature and installer integrity;
- credential or token handling;
- privilege escalation and UIPI bypass;
- accidental logging or telemetry of user text.

Out of scope:

- vulnerabilities in the legacy Kontho application (report them to its owner);
- third-party dependencies without a concrete Akkhor exploit path;
- social engineering.

## Data handling

- No raw keystroke logs.
- No text in crash reports or telemetry.
- Learning data is local-only by default and encrypted at rest.
- Pause, export, and delete are required product controls.
- See `docs/security-privacy.md` for the full threat model.

## Disclosure

We will acknowledge a report, investigate, and coordinate a fix and disclosure
date. Please give us reasonable time before publishing details.

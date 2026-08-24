# Changelog

All notable user-facing changes to this project are recorded here.

The format follows the `### Features / ### Changes / ### Fixes` structure expected by
the changelog-manager agent (see `.claude/agents/changelog-manager.md`). This project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## How entries are added

New entries land via the `post-pr-create-changelog` hook (see `.claude/hooks/`).
The unreleased version for any in-flight PR is resolved from the PR's GitHub
milestone, so make sure your PR is assigned to the correct milestone before
opening it.

Entry style:

- One past-tense imperative line per change. Start with "Added", "Changed", "Fixed", or "Removed".
- Use backticks for code identifiers (`fetch_notes`, `StoredNote`, `seq`).
- Prefix breaking changes with `[BREAKING] `.
- End with the PR link in parentheses, then a period.

Example:

```
- Fixed `fetch_notes` pagination race by introducing a monotonic `seq` cursor ([#77](https://github.com/0xMiden/note-transport-service/pull/77)).
- [BREAKING] Removed deprecated `fetch_notes_legacy` ([#82](https://github.com/0xMiden/note-transport-service/pull/82)).
```

Skip the changelog only when the PR contains no runtime-affecting changes
(docs, CI, tooling, tests). In that case the hook will tell you to apply the
`no changelog` label instead.

## v0.5.0-rc.2 (unreleased)

### Features

- Added PostgreSQL storage and a verified command for copying a stopped SQLite database ([#114](https://github.com/0xMiden/note-transport-service/issues/114)).
- Added bounded storage, explicit cleanup, dependency readiness, graceful shutdown, and service metrics ([#114](https://github.com/0xMiden/note-transport-service/issues/114)).

### Changes

- [BREAKING] Changed the protobuf package to `miden_note_transport.v1` and removed the `Stats` RPC ([#114](https://github.com/0xMiden/note-transport-service/issues/114)).
- Changed `StreamNotes` to use committed database notifications instead of periodic polling ([#114](https://github.com/0xMiden/note-transport-service/issues/114)).
- Replaced Diesel with SQLx and removed the bundled telemetry deployment and obsolete book source ([#114](https://github.com/0xMiden/note-transport-service/issues/114)).

## v0.5.0-rc.1 (2026-08-18)

### Features

- Added gRPC server reflection support to the node ([#110](https://github.com/0xMiden/note-transport-service/pull/110)).

### Changes

- [BREAKING] Changed the node binary name from `miden-note-transport-node-bin` to `miden-note-transport-node`; deployment scripts referencing the old name must be updated ([#104](https://github.com/0xMiden/note-transport-service/pull/104)).
- Changed `miden-protocol` to `0.16.0-rc.5`, aligning with the `miden-node` and `miden-client` `0.16.0-rc.1` releases; the stored-note wire format and `NoteId` derivation are unchanged ([#150](https://github.com/0xMiden/note-transport-service/pull/150)).
- Changed transitive dependencies to their latest compatible versions via `cargo update` ([#108](https://github.com/0xMiden/note-transport-service/pull/108)).

### Fixes

- Fixed the `Dockerfile` to build with Rust 1.96 and pruned unnecessary build dependencies ([#104](https://github.com/0xMiden/note-transport-service/pull/104)).

## v0.5.0-alpha.1 (2026-07-16)

### Changes

- Changed `miden-protocol` to `0.16.0-alpha.2` and raised the MSRV to Rust 1.96.1; `NoteHeader` serialization stays compatible with 0.15-produced bytes ([#99](https://github.com/0xMiden/note-transport-service/pull/99)).

## v0.4.1 (2026-06-17)

### Features

- Added optional `after_block_num` to `TransportNote` so senders can give recipients a deterministic block floor for commitment scans ([#81](https://github.com/0xMiden/note-transport-service/pull/81)).
- Added structured tracing fields across the gRPC, database, and streaming layers for debuggability ([#83](https://github.com/0xMiden/note-transport-service/pull/83)).

### Changes

- Changed OpenTelemetry configuration: enable via `--enable-otel`/`--otel-endpoint` flags or the standard `OTEL_EXPORTER_OTLP_ENDPOINT`/`OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` env vars. The previous `OTEL_ENABLED`/`OTEL_TRACES_ENDPOINT` vars are no longer read ([#80](https://github.com/0xMiden/note-transport-service/pull/80)).

## v0.4.0 (2026-06-08)

Released without itemized changelog entries. See [`git log v0.3.2..v0.4.0`](https://github.com/0xMiden/note-transport-service/compare/v0.3.2...v0.4.0) for the change set.

## v0.3.1 (2026-04-08)

Released before this changelog was started. See [`git log v0.3.0..v0.3.1`](https://github.com/0xMiden/note-transport-service/compare/v0.3.0...v0.3.1) for the change set.

## v0.3.0 (2026-04-08)

Released before this changelog was started. See [`git log v0.2..v0.3.0`](https://github.com/0xMiden/note-transport-service/compare/v0.2...v0.3.0) for the change set.

## v0.2 (2026-01-24)

First tagged release. Earlier history available via `git log v0.2`.

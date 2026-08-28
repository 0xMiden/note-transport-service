---
sidebar_position: 3
title: Operators
---

# Operators

This page covers running a note transport node.

## Build from source

From the repository root:

```bash
cargo install --path bin/node --locked
```

This installs the `miden-note-transport-node` binary.

## Run the node

Set the database URL. Production deployments use PostgreSQL:

```bash
export MNT_DATABASE_URL='postgres://user:password@database/note_transport'
```

SQLite remains supported for local deployments. Its database URL is a file path such as `/var/lib/miden-note-transport/node.db`.

Create or update the database before starting the service:

```bash
miden-note-transport-node migrate \
  --database-url "$MNT_DATABASE_URL"
```

Then start the service:

```bash
miden-note-transport-node serve \
  --listen 0.0.0.0:57292 \
  --database-url "$MNT_DATABASE_URL" \
  --max-storage-bytes 1073741824 \
  --max-streams 1024
```

The service checks the migration version and checksum at startup. It does not apply migrations.

Run bounded retention cleanup as a separate operation:

```bash
miden-note-transport-node cleanup \
  --database-url "$MNT_DATABASE_URL" \
  --retention-days 30 \
  --max-rows 1000
```

## Serve flags

| Flag | Default | Description |
| --- | --- | --- |
| `--listen` | `127.0.0.1:57292` | Address and port to bind to. It can also come from `MNT_LISTEN`. |
| `--database-url` | required | Existing SQLite path or PostgreSQL URL. It can also come from `MNT_DATABASE_URL`. |
| `--max-note-size` | `512000` | Maximum envelope size in bytes. It can also come from `MNT_MAX_NOTE_SIZE`. |
| `--max-requests` | `4096` | Maximum concurrent gRPC requests. It can also come from `MNT_MAX_REQUESTS`. |
| `--request-timeout` | `4` | Unary request and stream operation timeout in seconds. It can also come from `MNT_REQUEST_TIMEOUT`. |
| `--max-streams` | `1024` | Maximum live `StreamNotes` requests. A slot remains held until its stream ends. |
| `--max-storage-bytes` | required | Maximum retained payload bytes. It can also come from `MNT_MAX_STORAGE_BYTES`. |

The `migrate` command requires `--database-url`. The `cleanup` command also accepts `--retention-days` and `--max-rows`. Those values can come from `MNT_RETENTION_DAYS` and `MNT_CLEANUP_MAX_ROWS`.

## Telemetry and logging

Telemetry is configured through environment variables:

| Variable | Default | Description |
| --- | --- | --- |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | unset | Fallback OTLP endpoint for traces and metrics. |
| `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` | unset | OTLP trace endpoint. It takes precedence over the fallback. |
| `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` | unset | OTLP metric endpoint. It takes precedence over the fallback. |
| `JSON_LOGGING` | `false` | Emits JSON logs when set to `true`. |
| `RUST_LOG` | `INFO` | Standard Rust tracing filter. |

Example:

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://collector:4317 \
JSON_LOGGING=true \
RUST_LOG=INFO \
miden-note-transport-node serve \
  --listen 0.0.0.0:57292 \
  --database-url /var/lib/miden-note-transport/node.db \
  --max-storage-bytes 1073741824
```

## Docker Compose

The repository includes a Docker Compose setup for local PostgreSQL storage:

```bash
make docker-node-up
```

Use:

```bash
make docker-node-down
```

to stop the stack.

Compose waits for PostgreSQL, runs the migration command, and starts the node as an unprivileged user. PostgreSQL data remains in the `postgres_data` volume. Compose forwards the supported OTLP endpoint variables from the host environment or `.env` file.

## Ports

| Port | Service |
| --- | --- |
| `57292` | Note transport gRPC API. |

The gRPC server exposes the note transport API and the health service on port `57292`. It serves plaintext HTTP/2 and gRPC Web because TLS terminates at Traefik. The server sends HTTP/2 keepalive probes for long-lived streams.

The empty gRPC health service name reports process liveness. The `miden_note_transport.v1.MidenNoteTransport` service reports readiness. Readiness requires a working database query and, for PostgreSQL, a working change-notification listener.

On SIGTERM or Ctrl-C, the server marks the API unavailable, stops accepting new requests, and closes active note streams. Startup and serving failures produce a nonzero exit status. Telemetry providers flush during normal shutdown.

## Database behavior

The serving process requires a migrated PostgreSQL database or an existing file-backed SQLite database. In-memory storage is available only to tests.

The database assigns monotonic cursors and tracks retained payload bytes in the same write transaction. A retry with the same note ID succeeds without adding another row. Fetches and cleanup are bounded by their configured limits.

## Operational cautions

Treat debug logs as sensitive because note IDs and tags can be correlated with user activity. Set cleanup retention to cover the expected offline window for users.

Monitor `grpc_error_count`, `grpc_rejected_write_count`, and `grpc_active_streams`. The service rejects invalid headers and details, commitment mismatches, and writes over either storage limit. Use `FetchNotes` for durable catch-up before relying on streaming for live updates.

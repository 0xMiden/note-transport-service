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
  --max-storage-bytes 1073741824
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
| `--max-connections` | `4096` | Maximum concurrent gRPC requests. It can also come from `MNT_MAX_CONNECTIONS`. |
| `--request-timeout` | `4` | Per-request timeout in seconds. It can also come from `MNT_REQUEST_TIMEOUT`. |
| `--max-storage-bytes` | required | Maximum retained payload bytes. It can also come from `MNT_MAX_STORAGE_BYTES`. |

The `migrate` command requires `--database-url`. The `cleanup` command also accepts `--retention-days` and `--max-rows`. Those values can come from `MNT_RETENTION_DAYS` and `MNT_CLEANUP_MAX_ROWS`.

## Telemetry and logging

Telemetry is configured through environment variables:

| Variable | Default | Description |
| --- | --- | --- |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | unset | OTLP endpoint for trace and metric export. Setting it enables export. |
| `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` | unset | Shared OTLP endpoint that takes precedence when both endpoint variables are set. |
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

The repository includes a Docker Compose setup for the node with persistent SQLite storage:

```bash
make docker-node-up
```

Use:

```bash
make docker-node-down
```

to stop the stack.

Compose runs the migration command before it starts the node. Both services mount `/app/data` on the `node_data` volume. Compose forwards either supported OTLP endpoint variable from the host environment or `.env` file.

## Ports

| Port | Service |
| --- | --- |
| `57292` | Note transport gRPC API. |

The gRPC server exposes the note transport API and the health service on port `57292`.

## Database behavior

The serving process requires a migrated PostgreSQL database or an existing file-backed SQLite database. In-memory storage is available only to tests.

The database assigns monotonic cursors and tracks retained payload bytes in the same write transaction. An identical envelope retry succeeds without adding another row. Fetches and cleanup are bounded by their configured limits.

## Operational cautions

Treat debug logs as sensitive because note IDs and tags can be correlated with user activity. Set cleanup retention to cover the expected offline window for users.

Monitor request errors because invalid headers, unsealed details, and writes over either storage limit are rejected. Use `FetchNotes` for durable catch-up before relying on streaming for live updates.

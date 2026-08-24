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

The default configuration binds to localhost and stores notes in an in-memory SQLite database:

```bash
miden-note-transport-node
```

For a reachable node with persistent storage:

```bash
miden-note-transport-node \
  --host 0.0.0.0 \
  --port 57292 \
  --database-url /var/lib/miden-note-transport/node.db \
  --retention-days 30
```

## CLI flags

| Flag | Default | Description |
| --- | --- | --- |
| `--host` | `127.0.0.1` | Address to bind to. |
| `--port` | `57292` | gRPC port. |
| `--database-url` | `:memory:` | SQLite database URL or file path. Use a file path for persistence. |
| `--retention-days` | `30` | How long to retain notes before cleanup. |
| `--max-note-size` | `512000` | Maximum note details size in bytes. |
| `--max-connections` | `4096` | Maximum concurrent gRPC connections. |
| `--request-timeout` | `4` | Per-request timeout in seconds. |

The CLI flags above are parsed as command-line arguments. They are not currently read from `DATABASE_URL` or similarly named environment variables.

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
miden-note-transport-node --host 0.0.0.0 --database-url /var/lib/miden-note-transport/node.db
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

The Compose node service passes `--database-url /app/data/node.db` and mounts `/app/data` on the `node_data` volume, so note storage survives container restarts. Compose forwards either supported OTLP endpoint variable from the host environment or `.env` file.

## Ports

| Port | Service |
| --- | --- |
| `57292` | Note transport gRPC API. |

The gRPC server exposes the note transport API and the health service on port `57292`.

## Database behavior

Use a file-backed SQLite path for production-like deployments. The default `:memory:` database is useful for local testing but loses all notes on restart.

The node runs embedded migrations at startup. The current schema stores note IDs with a uniqueness constraint and uses a monotonic `seq` column for pagination.

## Operational cautions

Treat debug logs as sensitive because note IDs and tags can be correlated with user activity. Configure the retention period to cover the expected offline window for users.

Monitor request errors because duplicate note IDs and invalid note headers are rejected. Use `FetchNotes` for durable catch-up before relying on streaming for live updates.

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

The database file path is required:

```bash
miden-note-transport-node --database-url mtln.db
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
| `--database-url` | Required | SQLite database file path. |
| `--retention-days` | `30` | How long to retain notes before cleanup. |
| `--max-note-size` | `512000` | Maximum note details size in bytes. |
| `--max-connections` | `4096` | Maximum concurrent gRPC connections. |
| `--request-timeout` | `4` | Per-request timeout in seconds. |

The CLI flags above are parsed as command-line arguments. They are not currently read from `DATABASE_URL` or similarly named environment variables.

## Telemetry and logging

Telemetry is configured through environment variables:

| Variable | Default | Description |
| --- | --- | --- |
| `OTEL_ENABLED` | `false` | Enables OpenTelemetry export when set to `true`. |
| `OTEL_TRACES_ENDPOINT` | `http://localhost:4317` | OTLP endpoint for trace and metric export. |
| `JSON_LOGGING` | `false` | Emits JSON logs when set to `true`. |
| `RUST_LOG` | `INFO` | Standard Rust tracing filter. |

Example:

```bash
OTEL_ENABLED=true \
OTEL_TRACES_ENDPOINT=http://otel-collector:4317 \
JSON_LOGGING=true \
RUST_LOG=INFO \
miden-note-transport-node --host 0.0.0.0 --database-url /var/lib/miden-note-transport/node.db
```

## Docker Compose

The repository includes a Docker Compose setup for the node plus telemetry services:

```bash
make docker-node-up
```

This starts:

- note transport node;
- OpenTelemetry Collector;
- Tempo;
- Prometheus;
- Grafana.

Use:

```bash
make docker-node-down
```

to stop the stack.

The Compose node service passes `--database-url /app/data/node.db` and mounts `/app/data` on the `node_data` volume, so note storage survives container restarts.

## Ports

| Port | Service |
| --- | --- |
| `57292` | Note transport gRPC API. |
| `4317` | OTLP gRPC receiver in the collector. |
| `4318` | OTLP HTTP receiver in the collector. |
| `3000` | Grafana. |
| `9090` | Prometheus. |
| `3200` | Tempo. |

The note transport node exposes gRPC health through the same gRPC server, not a separate HTTP health port.

## Database behavior

The node only supports file-backed SQLite databases and requires `--database-url` at startup. Relative paths are resolved from the node's working directory; use an absolute path for predictable production deployments.

The node runs embedded migrations at startup. The current schema stores note IDs with a uniqueness constraint and uses a monotonic `seq` column for pagination.

## Operational cautions

- Treat debug logs as sensitive. Note IDs and tags can be correlated with user activity.
- Configure a retention period that matches the expected offline window for your users.
- Monitor request errors. Duplicate note IDs or invalid note headers are rejected.
- Use `FetchNotes` for durable catch-up. Streaming is best used as a live update channel after a fetch cycle.

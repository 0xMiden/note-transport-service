---
sidebar_position: 3
title: For operators
---

# Running the NTL

This page covers deploying, configuring, and monitoring the Note Transport Layer.

## Installation

### From source

```bash
cargo install --path bin/node --locked
```

This installs the `miden-note-transport-node-bin` binary.

### Docker

A Docker Compose setup is provided with a full telemetry stack:

```bash
make docker-node-up
```

This starts the NTL node along with OpenTelemetry Collector, Tempo (traces), Prometheus (metrics), and Grafana (visualization).

To build and run standalone:

```bash
make docker-build-node
make docker-run-node
```

## Configuration

### CLI flags

| Flag | Default | Description |
|------|---------|-------------|
| `--host` | `127.0.0.1` | Address to bind to |
| `--port` | `57292` | gRPC port |
| `--database-url` | `:memory:` | SQLite database path (use a file path for persistence) |
| `--retention-days` | `30` | How long to keep notes before automatic cleanup |
| `--max-note-size` | `512000` | Maximum note payload size in bytes |
| `--max-connections` | `4096` | Maximum concurrent gRPC connections |
| `--request-timeout` | `4` | Per-request timeout in seconds |
| `--enable-otel` | `false` | Enable OpenTelemetry tracing and metrics export |
| `--otel-endpoint` | `http://localhost:4317` | OTLP collector endpoint |

### Environment variables

CLI flags can also be set via environment variables:

| Variable | Corresponding flag |
|----------|-------------------|
| `MIDEN_TLNODE_ENABLE_OTEL` | `--enable-otel` |
| `MIDEN_TLNODE_OTEL_ENDPOINT` | `--otel-endpoint` |
| `JSON_LOGGING` | Use JSON log format (default: `false`) |
| `RUST_LOG` | Log level filter (default: `INFO`) |

### Example

Run with persistent storage and telemetry:

```bash
miden-note-transport-node-bin \
  --host 0.0.0.0 \
  --port 57292 \
  --database-url /var/lib/ntl/notes.db \
  --retention-days 30 \
  --enable-otel \
  --otel-endpoint http://otel-collector:4317
```

## Database

The NTL uses SQLite for note storage. Two modes are supported:

- **In-memory** (`:memory:`): Fast but non-persistent. Good for development and testing. Uses a single-connection pool to avoid isolation issues.
- **File-backed** (e.g. `/var/lib/ntl/notes.db`): Persistent across restarts. Uses a 16-connection pool with WAL mode for concurrent access.

The database schema is managed via embedded migrations that run automatically on startup.

### Maintenance

A background task runs every 10 minutes and deletes notes older than `retention-days`. This keeps storage bounded without manual intervention. The cleanup count is logged at INFO level.

## Ports

| Port | Service |
|------|---------|
| 57292 | gRPC server (NTL API) |
| 4317 | OTLP gRPC receiver (telemetry) |
| 3000 | Grafana (visualization) |
| 9090 | Prometheus (metrics) |
| 3200 | Tempo (traces) |

## Monitoring

### Metrics

The NTL exports OpenTelemetry metrics when `--enable-otel` is set. Available metrics:

**gRPC layer:**
- `grpc_send_note_count` - total send requests
- `grpc_send_note_duration` - send request latency (seconds)
- `grpc_send_note_note_size` - incoming note size distribution (bytes)
- `grpc_fetch_notes_count` - total fetch requests
- `grpc_fetch_notes_duration` - fetch request latency (seconds)
- `grpc_fetch_notes_replied_notes_number` - notes per response
- `grpc_fetch_notes_replied_notes_size` - response size (bytes)

**Database layer:**
- `db_store_note_count`, `db_store_note_duration`
- `db_fetch_notes_count`, `db_fetch_notes_duration`
- `db_fetch_notes_legacy_cursor_reset_count` - pre-migration clients detected
- `db_maintenance_cleanup_notes_count`, `db_maintenance_cleanup_notes_duration`

### Logging

Log levels are controlled via `RUST_LOG`:

- **INFO** (default): startup config, maintenance cleanup counts, subscription lifecycle, request counts/cursors, rejection warnings
- **DEBUG**: per-request NoteId and tag values (opt-in for privacy reasons - see below)

:::caution
Setting `RUST_LOG=miden_note_transport=debug` logs NoteId and tag values on every request. These can be correlated to on-chain events via timing analysis. Avoid sending debug logs to external SaaS services (Datadog, Grafana Cloud) in production unless you understand the privacy implications.
:::

### Health check

The NTL exposes a gRPC health check endpoint via `tonic-health`. Clients can use standard gRPC health checking to verify the service is running.

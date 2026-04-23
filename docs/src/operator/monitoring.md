# Monitoring & telemetry

We provide logging to `stdout` and an optional [OpenTelemetry](https://opentelemetry.io/) exporter for our metrics and traces.

OpenTelemetry exporting can be enabled by setting the `OTEL_ENABLED=true` environment variable when operating
the node. The OTLP endpoint (default `http://localhost:4317`) can be overridden with `OTEL_TRACES_ENDPOINT`, and
structured JSON logging to `stdout` can be enabled with `JSON_LOGGING=true`.

## Metrics

Various metrics associated with the RPC requests and database operations are provided:

### RPC metrics

| name                                    | type                | description                                       |
|-----------------------------------------|---------------------|---------------------------------------------------|
| `grpc_send_note_count`                  | Counter             | number of `send_note` requests                    |
| `grpc_send_note_duration`               | Histogram (seconds) | duration of `send_note` requests                  |
| `grpc_send_note_note_size`              | Histogram (bytes)   | size of received notes in `send_note` requests    |
| `grpc_fetch_notes_count`                | Counter             | number of `fetch_notes` requests                  |
| `grpc_fetch_notes_duration`             | Histogram (seconds) | duration of `fetch_notes` requests                |
| `grpc_fetch_notes_replied_notes_number` | Histogram           | number of replied notes in `fetch_notes` requests |
| `grpc_fetch_notes_replied_notes_size`   | Histogram (bytes)   | size of replied notes in `fetch_notes` requests   |

### Database metrics

| name                                    | type                | description                                |
|-----------------------------------------|---------------------|--------------------------------------------|
| `db_store_note_count`                   | Counter             | number of `store_note` operations          |
| `db_store_note_duration`                | Histogram (seconds) | duration of `store_note` operations        |
| `db_fetch_notes_count`                  | Counter             | number of `fetch_notes` operations         |
| `db_fetch_notes_duration`               | Histogram (seconds) | duration of `fetch_notes` operations       |
| `db_maintenance_cleanup_notes_count`    | Counter             | number of `cleanup_old_notes` operations   |
| `db_maintenance_cleanup_notes_duration` | Histogram (seconds) | duration of `cleanup_old_notes` operations |

## Traces

We assign a unique trace (aka root span) to each RPC request.

<div class="warning">

Span and attribute naming is unstable and should not be relied upon. This also means changes here will not be considered
breaking, however we will do our best to document them.

</div>

### RPC traces

<details>
  <summary>Span tree</summary>

```sh
grpc.send_note.request
┕━ db.store_note

grpc.fetch_notes.request
┕━ db.fetch_notes
```

</details>


## Verbosity

We log important spans and events at `info` level or higher, which is also the default log level.

Changing this level should rarely be required - let us know if you're missing information that should be at `info`.

The available log levels are `trace`, `debug`, `info` (default), `warn`, `error` which can be configured using the
`RUST_LOG` environment variable e.g.

```sh
export RUST_LOG=debug
```

## Configuration

The OpenTelemetry trace and metrics exporters are enabled by setting `OTEL_ENABLED=true` when starting the node:

```sh
OTEL_ENABLED=true \
OTEL_TRACES_ENDPOINT=http://localhost:4317 \
miden-note-transport-node-bin
```

Further exporter behaviour can be configured using the standard OpenTelemetry environment variables as specified in the
official [documents](https://opentelemetry.io/docs/specs/otel/protocol/exporter/).

<div class="warning">
Not all options are fully supported. We are limited to what the Rust OpenTelemetry implementation supports. If you have any problems please open an issue and we'll do our best to resolve it.

</div>

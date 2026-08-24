# Miden Note Transport Node

Node/server implementation of the Miden Transport Layer for private notes.

## API

The node uses gRPC messages from the `miden-note-transport-proto` crate. `send_note()` stores an incoming note in the database. Note details may be encrypted.

`fetch_notes()` returns a page for the requested note tags and cursor. `stream_notes()` keeps a subscription open for one tag and sends note updates to that client.

SQLite and PostgreSQL implement the same storage contract. Migration and cleanup run as explicit operator commands.

## Telemetry
Metrics and traces to monitor the node state are provided.
Metrics report aggregate request data, while traces describe specific requests. The node exports both through [OpenTelemetry](https://opentelemetry.io).

## License
This project is [MIT licensed](../../LICENSE).

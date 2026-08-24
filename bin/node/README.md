# Miden Note Transport Node

Node binary wrapping the Miden Transport Layer Node library.

## Build

To build from source, run
```sh
cargo build --release --locked
```

The binary will be available on `./target/release/miden-note-transport-node`.

## Docker setup

The Docker Compose setup starts PostgreSQL, applies migrations, and then starts the node as an unprivileged user. The [operator guide](../../docs/external/src/operators.md) covers copying the earlier Compose SQLite volume before startup. Set an OTLP endpoint when the node should export telemetry to an external collector.

## License
This project is [MIT licensed](../../LICENSE).

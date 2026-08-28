# Miden Note Transport Node

Node binary wrapping the Miden Transport Layer Node library.

## Build

To build from source, run
```sh
cargo build --release --locked
```

The binary will be available on `./target/release/miden-note-transport-node`.

## Docker setup

The Docker Compose setup migrates a persistent SQLite database before it starts the node. Set an OTLP endpoint when the node should export telemetry to an external collector.

## License
This project is [MIT licensed](../../LICENSE).

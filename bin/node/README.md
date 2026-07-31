# Miden Note Transport Node

Node binary wrapping the Miden Transport Layer Node library.

## Build

To build from source, run
```sh
cargo build --release --locked
```

The binary will be available on `./target/release/miden-note-transport-node`.

## Docker setup

A docker-based setup is provided. In addition to the main node, the configured setup provides,
- OpenTelemetry Collector;
- Tempo (Traces);
- Prometheus (Metrics);
- Grafana (Visualization).

## License
This project is [MIT licensed](../../LICENSE).

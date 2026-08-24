# Miden Note Transport Layer

## Overview

The Miden Note Transport service stores private note envelopes for a configured retention period. Recipients fetch retained envelopes with durable cursors, so fetching an envelope does not delete it.

The workspace contains the node library and its protobuf runtime and build crates. The `miden-note-transport-node` binary runs the server.

Production deployments use PostgreSQL. SQLite remains supported for local use and for the offline move to PostgreSQL. The [operator guide](docs/external/src/operators.md) covers deployment and database operations.

## API reference

`SendNote` stores an envelope for its recipient. Recipients use `FetchNotes` for paged reads and `StreamNotes` for live updates.

### Telemetry

The node exports traces and metrics through OpenTelemetry when an OTLP endpoint is set. Operators provide the collector and telemetry storage used by their deployment. This repository does not bundle a telemetry stack.

## Contributing

Please read the organization [contributing guidelines](https://github.com/0xMiden/.github/blob/main/CONTRIBUTING.md). The [Makefile](Makefile) provides the local checks. Run the test suite with:

```sh
make test
```

We do not accept low effort contributions or generated code that the author has not reviewed. Please open an issue for small documentation errors.

## License

This project is [MIT licensed](./LICENSE).

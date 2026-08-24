# Miden Note Transport Layer

## Overview

The Miden Note Transport service stores private note envelopes until a recipient fetches them. It supports asynchronous note exchange between Miden clients.

The workspace contains the node library and its protobuf runtime and build crates. The `miden-note-transport-node` binary runs the server.

## API reference

`SendNote` stores an envelope for its recipient. `FetchNotes` returns stored envelopes for a tag, while `StreamNotes` sends new envelopes as they arrive.

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

# Miden Note Transport Layer

[![LICENSE](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/0xMiden/note-transport-service/blob/main/LICENSE)
[![test](https://github.com/0xMiden/note-transport-service/actions/workflows/test.yml/badge.svg)](https://github.com/0xMiden/note-transport-service/actions/workflows/test.yml)
[![RUST_VERSION](https://img.shields.io/badge/rustc-1.96.1+-lightgray.svg)](https://www.rust-lang.org/tools/install)

## Overview

The Miden Note Transport service is a communications system focusing on performance and privacy for the secure exchange of private notes.

The system is based mostly on a request-reply client-server communication scheme, supporting end-to-end encryption.
The (optionally encrypted) notes are stored on-server allowing for async note exchange between users.

### Crates

This repository contains the following crates:

- `node`: Node/server library;
- `proto`: Protobuf definitions and generated code;

### Binaries

This repository contains the following binaries, built upon the above crates:

- `node`: Node/server implementation, wrapping the respective library;
- `cli`: Client command-line-interface, wrapping the respective (Rust) library. Easy-to-use application able to send and fetch notes from the Transport Layer;
- `load-test`: Load testing tool for the node implementation.

## API Reference

Three main functions are used to interact with the Transport Layer:

- `send_note(note, address)` allows a client to push a note, directed to a recipient (identified by its address), to the Transport Layer. The note is kept in the Transport Layer for a certain retention period (30 days);
- `fetch_notes(tag)` allows a client to fetch notes associated with a certain tag;
- `stream_notes(tag)` similarly to `fetch_notes()`, but the client subscribes to a tag and receives new notes periodically.

### Telemetry

Metrics and Traces are provided for the node implementation.
Data is exported using OpenTelemetry.
A Docker-based setup is provided, with the following stack:
- OpenTelemetry Collector;
- Tempo (Traces);
- Prometheus (Metrics);
- Grafana (Visualization).

## Contributing

At minimum, please see our [contributing](https://github.com/0xMiden/.github/blob/main/CONTRIBUTING.md) guidelines and our [makefile](Makefile) for example workflows
e.g. run the testsuite using

```sh
make test
```

Note that we do _not_ accept low-effort contributions or AI generated code. For typos and documentation errors please
rather open an issue.

## License
This project is [MIT licensed](./LICENSE).

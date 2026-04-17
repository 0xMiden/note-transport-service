# Navigating the codebase

The code is organised as a Rust workspace with four members:

- `miden-note-transport-node` (at `crates/node`): Primary node library. Contains all of the node logic;
- `miden-note-transport-proto` (at `crates/proto`): Generated Rust types and service stubs for the node's gRPC API. Clients and node depend on this crate to establish communications;
- `miden-note-transport-proto-build` (at `proto`): Holds the canonical `.proto` files and the build-time code generation used by `miden-note-transport-proto`;
- `miden-note-transport-node-bin` (at `bin/node`): Running node binary. Instantiation and wrapper of the node library.

-------

> [!NOTE]
> [`miden-base`](https://github.com/0xMiden/miden-base) is an important dependency which
> contains the core Miden protocol definitions e.g. accounts, notes, transactions etc.

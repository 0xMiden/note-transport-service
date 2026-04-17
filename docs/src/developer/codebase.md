# Navigating the codebase

The code is organised using a Rust workspace with two crates (`miden-note-transport-node` and `miden-note-transport-proto`) and a binary (`miden-note-transport-node-bin`):

- `miden-note-transport-node` (at `crates/node`): Primary node library. Contains all of the node logic;
- `miden-note-transport-proto` (at `crates/proto`): gRPC protobuf definitions and associated auto-generated Rust code. Both clients and node use this crate to establish communications;
- `miden-note-transport-node-bin` (at `bin/node`): Running node binary. Instantiation and wrapper of the node library.

-------

> [!NOTE]
> [`miden-base`](https://github.com/0xMiden/miden-base) is an important dependency which
> contains the core Miden protocol definitions e.g. accounts, notes, transactions etc.

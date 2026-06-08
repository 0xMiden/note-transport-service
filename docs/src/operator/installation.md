# Installation

The Miden Transport Service currently can only be installed from source using the Rust package manager `cargo`, or by using the Docker setup provided in the repository.

## Install using `cargo`

Install Rust version **1.87** or greater using the official Rust installation
[instructions](https://www.rust-lang.org/tools/install).

Depending on the platform, you may need to install additional libraries. For example, on Ubuntu 22.04 the following
command ensures that all required libraries are installed.

```sh
sudo apt install llvm clang bindgen pkg-config libssl-dev libsqlite3-dev
```

Then use `cargo` to compile the node from the source code:

```sh
# Install latest version
cargo install --locked --git https://github.com/0xMiden/note-transport-service miden-note-transport-node-bin

# Install from a specific branch
cargo install --locked --git https://github.com/0xMiden/note-transport-service miden-note-transport-node-bin --branch <branch>

# Install a specific tag
cargo install --locked --git https://github.com/0xMiden/note-transport-service miden-note-transport-node-bin --tag <tag>

# Install a specific git revision
cargo install --locked --git https://github.com/0xMiden/note-transport-service miden-note-transport-node-bin --rev <git-sha>
```

More information on the various `cargo install` options can be found
[here](https://doc.rust-lang.org/cargo/commands/cargo-install.html#install-options).

## Docker setup

With Docker installed on your system, a Docker setup is provided that also includes a monitoring stack with: OpenTelemetry exporter, Grafana (visualization), Prometheus (metrics), and Tempo (traces).

Clone the repository:

```sh
git clone https://github.com/0xMiden/note-transport-service
```

Then, move into the directory, `cd note-transport-service`, and run `make docker-node-up` to start the node and monitoring stack.
To stop the stack, run `make docker-node-down`.

Grafana will be accessible at `localhost:3000`.


## Updating

> [!WARNING]
> We currently have no backwards compatibility guarantees. This means updating your node is destructive - your
> existing chain will not work with the new version. This will change as our protocol and database schema mature and
> settle.

Updating the node to a new version is as simple as re-running the install process and repeating the [bootstrapping](./usage.md#bootstrapping) instructions.

# Configuration and Usage

Configuration and operation of the Miden Transport Service node is simple.


## Operation

Start the node with the desired public gRPC server address.
For example,

```sh
miden-note-transport-node-bin \
  --host 0.0.0.0 \
  --port 57292 \
  --database-url mtln.db
```

> [!NOTE]
> `miden-note-transport-node-bin` provides default arguments aimed at development.

Configuration is purely made using command line arguments. Run `miden-note-transport-node-bin --help` for available options.

If using the provided Docker setup, see the [setup page](installation.md#docker-setup). Configure the node binary launch arguments accordingly before starting Docker containers.

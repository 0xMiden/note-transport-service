# Configuration and Usage

Configuration and operation of the Miden Transport Service node is simple.


## Operation

Start the node with the desired public gRPC server address.
For example,

```sh
miden-note-transport-node \
  --host 0.0.0.0 \
  --port 57292 \
  --database-url mtln.db
```

> [!NOTE]
> `miden-note-transport-node` provides default arguments aimed at development.

Configuration is purely made using command line arguments. Run `miden-note-transport-node --help` for available options.

If using the provided Docker setup, see the [setup page](installation.md#docker-setup). Configure the node binary launch arguments accordingly before starting Docker containers.

File-backed SQLite databases must already exist by default. For first-run setup, pass
`--create-database` explicitly to create the database file before migrations are applied. This
keeps a mistyped `--database-url` or an unmounted volume from silently starting an empty database.

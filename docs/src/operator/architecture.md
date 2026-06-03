# Node architecture

The node consists of two main components: RPC and database. Combined, a simple system supports the core mechanism of the transport service: the node serves public RPC requests, while using the database to store the notes associated with the requests.

While currently only supporting a centralized architecture, it is expected to evolve into a more distributed approach in order to increase the resilience of the transport service.

## RPC

The RPC component provides a public gRPC API with which users can send and fetch notes.
Requests are processed and then proxied to the database.

Note streaming is also supported through gRPC.

This is the _only_ externally facing component.

## Database

The database is responsible for storing the private notes.
As the transport service was built with a focus on user privacy, no user data is stored.

Notes are stored for a configured retention period (default 30 days, set via the [`--retention-days`](https://github.com/0xMiden/note-transport-service/blob/main/bin/node/src/main.rs) CLI flag).
An internal sub-component running in the node is responsible for the database maintenance, performing the removal of expired notes.

Currently, SQLite is the only database implementation provided.

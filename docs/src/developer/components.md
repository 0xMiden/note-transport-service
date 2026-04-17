# Node components

The node is split into two main components: RPC and database.

The following sections will describe the inner architecture of each component.

## RPC

The RPC component provides a public interface to user requests.
Essentially this is a thin gRPC server that proxies all requests to the database.

A running task is spawned with this gRPC server. The library `tonic` is used to provide gRPC support.

### Streaming

Note streaming is employed based on gRPC streams. A `NoteStreamer` task manages subscribed connections, feeding them newly received notes.

Focusing on performance, notes are not forwarded to subscribers as soon as they are received.
For subscribed tags, the `NoteStreamer` task periodically (every 500 ms) queries the database for new notes, akin to a fetch notes operation, and sends these to subscribed users.

## Database

This component persists the private notes in a SQLite database. Currently, there is only one table (named `notes`).

### Migrations

Schema migrations are embedded via `diesel_migrations` and applied automatically on node startup.

### Database maintenance

Database maintenance is provided by a separate task. Currently this is only a periodic cleanup of older notes -- notes which have exceeded the defined retention period.

A running task is spawned dedicated to this maintenance service.

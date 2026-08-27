---
sidebar_position: 2
title: Design
---

# Design

The note transport node is intentionally small: it accepts note bytes, indexes them by note tag, and returns matching notes to clients.

## Note flow

1. A sender creates a private note in a Miden transaction.
2. After the note data is available locally, the sender calls `SendNote` with a serialized note header and note details.
3. The transport node parses the header, extracts the note ID and tag, and stores the envelope.
4. A recipient calls `FetchNotes` for one or more tags and receives matching notes with a cursor.
5. The recipient stores the returned cursor and uses it on the next fetch.

The transport node does not connect to a Miden node and does not know whether a note has been committed on-chain. Clients still need to import fetched notes and sync against the Miden network.

## Stored data

The transport node stores:

- note ID, derived from the serialized header;
- note tag, derived from the serialized header;
- serialized header bytes;
- serialized details bytes;
- creation timestamp;
- `seq`, a monotonic value assigned by the database.

The full envelope digest is unique. Sending the same bytes again succeeds without adding a row. A different sealed envelope may contain the same note ID, so clients must deduplicate imported notes by note ID.

## Cursor pagination

`FetchNotes` uses a `seq` cursor:

```protobuf
message FetchNotesRequest {
    repeated fixed32 tags = 1;
    fixed64 cursor = 2;
}

message FetchNotesResponse {
    repeated TransportNote notes = 1;
    fixed64 cursor = 2;
    bool has_more = 3;
}
```

The server returns notes matching any requested tag with `seq > cursor`, ordered by ascending `seq`, up to the server row and byte limits. The response cursor is the highest `seq` returned. A client should persist that value and send it on the next request while `has_more` is true.

Current limits:

- A request may include up to 128 tags.
- A response returns up to 500 notes.
- There is no client-specified `limit` field in the protobuf API.

The multi-tag query runs in one database snapshot. This avoids a race where separate per-tag queries could advance the cursor past a note inserted between queries.

## Streaming

`StreamNotes` opens a server-side stream for one tag:

```protobuf
message StreamNotesRequest {
    fixed32 tag = 1;
    fixed64 cursor = 2;
}

message StreamNotesUpdate {
    repeated TransportNote notes = 1;
    fixed64 cursor = 2;
}
```

Each stream subscribes to storage changes before its first database read. It reads every available page, sends the durable cursor with each update, then waits for another committed write.

SQLite signals commits inside the process. PostgreSQL uses `LISTEN/NOTIFY`. These signals only wake the stream; database reads remain authoritative. Listener loss ends affected streams with `UNAVAILABLE`, and reconnection forces a fresh read for new streams.

The request cursor initializes stream delivery. Clients can reopen a stream with the last cursor they handled without a silent gap. Delivery is ordered and may repeat an update after a disconnect.

## Storage and retention

The node supports PostgreSQL and SQLite with explicit schema migration. The serving process checks migration versions and checksums without changing the schema. In-memory SQLite is reserved for tests.

Storage tracks retained payload bytes and rejects writes above the configured limit. Fetches have row and byte limits. Operators remove one bounded batch of expired notes with the cleanup command.

## Block context

The optional `after_block_num` field gives the client a lower bound for its chain scan. The sender supplies this hint, and the service stores it without checking chain state. Clients must still reconcile fetched notes with the Miden network.

## What the node does not do

The node does not:

- validate note contents against chain state;
- connect to a Miden node;
- attach commitment block context;
- attach note inclusion proofs;
- inspect or decrypt note details;
- authenticate senders or recipients;
- guarantee delivery after the retention period.

---
sidebar_position: 4
title: Users
---

# Users

This page covers integrating with the note transport gRPC API.

## API surface

The service is defined in `proto/proto/miden_note_transport.proto` under the
`miden_note_transport.v1` package.

```protobuf
service MidenNoteTransport {
    rpc SendNote(SendNoteRequest) returns (SendNoteResponse);
    rpc FetchNotes(FetchNotesRequest) returns (FetchNotesResponse);
    rpc StreamNotes(StreamNotesRequest) returns (stream StreamNotesUpdate);
}
```

## Send a note

`SendNote` stores one note:

```protobuf
message SendNoteRequest {
    TransportNote note = 1;
}

message SendNoteResponse {}

message TransportNote {
    bytes header = 1;
    bytes details = 2;
}
```

`header` must be a serialized Miden `NoteHeader`. The node parses it to extract the note ID and tag. `details` must be a sealed message produced by the sender. The node checks its framing but cannot decrypt it.

The server rejects:

- requests without a note;
- headers that cannot be parsed as `NoteHeader`;
- envelopes larger than the configured `--max-note-size`;
- details that are not a valid sealed message;
- writes that would exceed the storage byte limit.

## Fetch notes

`FetchNotes` returns notes for one or more tags:

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

Use this flow:

1. Start with `cursor = 0`.
2. Send all tags the client wants to check, up to 128 tags.
3. Import or process the returned notes.
4. Persist the response `cursor`.
5. Repeat with the stored cursor while `has_more` is true.

The response cursor is the highest server-side `seq` value returned in that response. A cursor belongs to the exact set of requested tags. Start again at `0` when that set changes.

Each response is capped at 500 notes and 3 MiB. Use `has_more` because either limit can end a page.

## Stream notes

`StreamNotes` provides a server-side stream for one tag:

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

Set the request cursor to the last update the client handled. The server first drains stored notes after that cursor, then waits for committed writes. Persist each returned cursor only after handling its notes.

Streaming behavior to account for:

- Subscriptions are per tag.
- Updates are ordered by the durable database cursor.
- Delivery is at least once, so a reconnect may repeat an update.
- Storage notification failure ends the stream with `UNAVAILABLE`.

On reconnect, open a new stream with the persisted cursor. `FetchNotes` uses the same cursor contract and remains available for explicit catch-up.

## Client sync pattern

A typical client should:

1. Configure a note transport endpoint.
2. Track the note tags it needs to monitor.
3. Fetch notes during sync using the stored transport cursor.
4. Import fetched notes into the client.
5. Sync with the Miden node to reconcile note commitments.
6. Persist the returned transport cursor only after the fetched notes have been handled successfully.

The transport node does not provide commitment block numbers or inclusion proofs. Clients must still handle chain-state reconciliation. The block-context improvement is tracked in [0xMiden/note-transport-service#68](https://github.com/0xMiden/note-transport-service/issues/68).

## Troubleshooting

### Notes do not appear

- Check that the sender actually called `SendNote`.
- Check that the recipient is fetching the same tag stored in the note header.
- Check whether the note expired under the node retention policy.
- Reset the local transport cursor to `0` if client state is suspected to be ahead of the server.

### Duplicate sends

Sending the same envelope again succeeds without adding a row. A different sealed envelope may use the same note ID. Clients deduplicate imported notes by note ID.

### Streaming misses notes

Reopen the stream with the last cursor that was handled successfully. Use `FetchNotes` with the same cursor if explicit catch-up is easier for the client.

### Large notes are rejected

The `--max-note-size` setting applies to the serialized header and sealed details together. Increase it only when the deployment is prepared to accept larger payloads.

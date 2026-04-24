---
sidebar_position: 4
title: For users
---

# Using the NTL

This page covers integrating with the Note Transport Layer from a Miden client or application.

## gRPC API

The NTL exposes a gRPC service at `MidenNoteTransport` with four RPCs:

### SendNote

Push a note to the NTL for delivery.

```protobuf
rpc SendNote(SendNoteRequest) returns (SendNoteResponse);

message SendNoteRequest {
    TransportNote note = 1;
}

message TransportNote {
    bytes header = 1;                           // Serialized NoteHeader
    bytes details = 2;                          // NoteDetails (can be encrypted)
    optional uint32 commitment_block_num = 3;   // Block where the note was committed
    optional bytes note_metadata = 4;           // Serialized NoteMetadata
}
```

**Usage**: Call after your transaction confirms on the Miden network. Serialize the `NoteHeader` and `NoteDetails`, optionally encrypt the details, and send.

### FetchNotes

Fetch notes matching one or more tags with cursor-based pagination.

```protobuf
rpc FetchNotes(FetchNotesRequest) returns (FetchNotesResponse);

message FetchNotesRequest {
    repeated fixed32 tags = 1;   // Note tags to filter by (max 128)
    fixed64 cursor = 2;          // Start after this cursor (0 for first fetch)
    optional uint32 limit = 3;   // Max notes to return (server caps at 500)
}

message FetchNotesResponse {
    repeated TransportNote notes = 1;
    fixed64 cursor = 2;          // Use as cursor in next request
}
```

**Usage**: Start with `cursor = 0`. After each response, use the returned `cursor` value for your next request. Repeat until you get an empty response (you're caught up).

### StreamNotes

Subscribe to real-time note delivery for a single tag.

```protobuf
rpc StreamNotes(StreamNotesRequest) returns (stream StreamNotesUpdate);

message StreamNotesRequest {
    fixed32 tag = 1;
    fixed64 cursor = 2;
}

message StreamNotesUpdate {
    repeated TransportNote notes = 1;
    fixed64 cursor = 2;
}
```

**Usage**: Open a streaming connection for a tag. The server pushes new notes as they arrive (~500ms polling interval). If your client can't keep up (32-message buffer), the subscription is dropped - reconnect and resume from your last cursor.

### Stats

Get aggregate statistics about the NTL instance.

```protobuf
rpc Stats(google.protobuf.Empty) returns (StatsResponse);

message StatsResponse {
    uint64 total_notes = 1;
    uint64 total_tags = 2;
}
```

## Client integration pattern

A typical Miden client interacts with the NTL during sync:

1. **After sending a transaction**: Call `SendNote` for each output note. Populate `commitment_block_num` with the block number where the transaction was included (or a lower-bound estimate).

2. **During sync**: Call `FetchNotes` with the client's registered tags and stored cursor. Process the returned notes, then store the response cursor for next time.

3. **For real-time apps**: Use `StreamNotes` instead of polling `FetchNotes`. This is useful for wallets or apps that need immediate note delivery.

## Block context

The `commitment_block_num` and `note_metadata` fields on `TransportNote` help recipients resolve on-chain note commitments without guessing:

- **Exact block number**: Set `commitment_block_num` to the block where your transaction was included. The recipient starts its commitment scan at this block.
- **Lower bound**: If you don't know the exact block, use the chain tip at send time (optionally minus a small safety margin). Any value at or below the actual commitment block works correctly.
- **Unset**: The recipient falls back to its own lookback heuristic (currently a 20-block scan window in miden-client).
- **Note metadata**: When present, the recipient can skip the commitment scan entirely and transition the note to committed immediately.

Wallets that need deterministic note delivery should always populate `commitment_block_num`.

## Cursor management

- Start with `cursor = 0` on first fetch
- After each `FetchNotes` response, store the returned `cursor` value persistently
- On restart, resume from the stored cursor
- Never fabricate cursor values - always use the value returned by the server

:::tip
If you're upgrading from a client that used timestamp-based cursors (pre-v0.4.0), the NTL automatically detects these (values above 10^12) and resets them to 0. Your client will re-fetch all stored notes on the next sync, which is safe but may be slow if many notes are stored.
:::

## Tag system

A `NoteTag` is a 32-bit value derived from the note's metadata. Tags are used for routing - the NTL has no concept of recipient addresses or accounts.

When calling `FetchNotes`, you can include up to 128 tags per request. The NTL returns all notes matching any of the provided tags in a single consistent snapshot (no interleaving race conditions).

## Troubleshooting

### Notes not arriving

- **Tags don't match**: Verify the sender's note tag matches the tags your client is fetching. The NTL filters strictly by tag.
- **Retention expired**: Notes are deleted after the retention period (default 30 days). If the recipient didn't sync in time, the note is gone.
- **Cursor too far ahead**: If your cursor is ahead of the latest note's `seq`, you'll get empty responses. Reset your cursor to 0 to re-fetch.
- **Sender didn't call SendNote**: The NTL only has notes that were explicitly sent to it. Check that the sender's client is configured to use the NTL.

### Connection issues

- **Port**: The default gRPC port is 57292. Check firewall rules and confirm the NTL is bound to the expected address.
- **Health check**: Use gRPC health checking to verify the service is alive.
- **Max connections**: The default is 4096 concurrent connections. If you're hitting this limit, the NTL rejects new connections.
- **Timeouts**: The default request timeout is 4 seconds. Large fetches or slow networks may need a higher value.

### Large notes rejected

Notes are rejected if their payload (details + metadata) exceeds `max-note-size` (default 512KB). If you need to send larger notes, the operator must increase this setting.

### Streaming disconnects

Streaming subscriptions are dropped if the client can't consume messages fast enough (32-message backpressure buffer). Reconnect and resume from your last cursor. Consider using `FetchNotes` with polling instead if your client processes notes slowly.

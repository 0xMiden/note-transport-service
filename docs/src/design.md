---
sidebar_position: 2
title: Design
---

# Design

This page describes how the Note Transport Layer works internally.

## Note flow

A note moves through the NTL in three stages:

1. **Send**: After a transaction confirms on the Miden network, the sender calls `SendNote` with the note's header and details. The NTL validates the header structure (to extract the NoteId and tag for storage), checks the note size, and persists it to SQLite.

2. **Store**: The note is stored with a monotonically increasing sequence number (`seq`), the note's tag, its serialized header and details, and a creation timestamp. The `seq` value is assigned by SQLite's `AUTOINCREMENT` and serves as the canonical pagination cursor.

3. **Fetch/Stream**: Recipients call `FetchNotes` with their tags and a cursor. The NTL returns all matching notes with `seq > cursor`, up to a batch limit (default 500). The response includes the highest `seq` seen, which the client uses as its cursor for the next call.

## Tag-based routing

Notes are routed by `NoteTag`, a 32-bit value embedded in the note's metadata. The NTL does not route by recipient address - it has no concept of accounts or identities. A client registers interest in specific tags and receives all notes matching those tags.

This design means:

- Multiple recipients can share a tag (broadcast)
- A single recipient can subscribe to multiple tags
- The NTL cannot tell who a note is "for" - only which tag it carries
- Privacy is preserved: the NTL sees tags but not the relationship between tags and accounts

## Cursor-based pagination

The NTL uses a `seq`-based cursor for pagination rather than timestamps. Each note gets a unique, monotonically increasing `seq` value at insertion time.

Clients track a cursor (initially 0) and advance it using the `cursor` value returned in each `FetchNotes` response. The server filters with `seq > cursor` and returns up to 500 notes per batch (clients can request a lower limit).

This design avoids two problems with timestamp-based cursors:

- **Timestamp collisions**: Two notes inserted in the same microsecond would get the same timestamp. With a strict `>` filter, the second note would be invisible. The `seq` column guarantees uniqueness.
- **Clock skew**: `seq` is independent of wall-clock time, so it works correctly even if the system clock jumps.

### Legacy cursor handling

Clients that stored cursors before the migration from timestamp-based to seq-based pagination carry very large cursor values (microseconds since epoch, ~1.7x10^15). The NTL detects cursors above 10^12 and resets them to 0, ensuring these clients don't stall forever waiting for `seq` to catch up.

## Streaming

The `StreamNotes` endpoint provides server-side streaming for real-time note delivery. Internally:

- A background `NoteStreamer` task polls the database every 500ms for new notes matching subscribed tags.
- Each subscription tracks its own cursor and only forwards new notes.
- Notes are delivered via bounded channels (32 items). If a subscriber falls behind (backpressure), the subscription is dropped.
- Subscriptions are cleaned up automatically when the client disconnects.

## Block context

Notes can optionally carry block context fields to help recipients resolve on-chain commitments:

- `commitment_block_num`: The block number where the note's on-chain commitment was included. The recipient uses this as the floor for its commitment scan.
- `note_metadata`: Serialized `NoteMetadata` from the commitment block. When present, the recipient can skip the commitment scan entirely.

These fields are sender-populated and optional. The NTL stores them verbatim without validation. See the [proto documentation](https://github.com/0xMiden/note-transport-service/blob/main/proto/proto/miden_note_transport.proto) for population strategies.

## Storage

The NTL uses SQLite with WAL (Write-Ahead Logging) mode for concurrent read/write access. Key characteristics:

- **Connection pooling**: 16 connections for file-backed databases, 1 for in-memory databases (to avoid isolation issues).
- **Retention**: A background maintenance task runs every 10 minutes and deletes notes older than the configured retention period (default 30 days).
- **Batch size**: Queries are capped at 500 rows to bound memory usage on both server and client.

## What the NTL does NOT do

The NTL is intentionally a minimal relay. It does not:

- Validate note contents against on-chain state
- Connect to or sync with the Miden node
- Backfill missing block context
- Inspect or decrypt note details
- Authenticate senders or recipients
- Guarantee delivery (notes expire after the retention period)
- Deduplicate notes (the same note can be sent twice)

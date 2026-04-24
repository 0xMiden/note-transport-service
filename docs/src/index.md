---
sidebar_position: 1
title: Overview
---

# Note Transport Layer

The Note Transport Layer (NTL) is an off-chain relay service for the Miden network. It enables private note exchange between users who cannot discover each other's notes on-chain.

## Why does the NTL exist?

On Miden, private notes are committed on-chain as hashes - the note contents are never published. This preserves privacy but creates a delivery problem: how does a recipient learn about a note meant for them?

The NTL solves this by acting as a temporary mailbox. Senders push notes to the NTL after their transaction confirms, and recipients poll for notes matching their tags. The NTL stores notes for a configurable retention period (default 30 days) and then discards them.

## Key properties

- **Privacy-preserving**: The NTL is a dumb relay. It stores and forwards notes without inspecting, validating, or interpreting their contents. Notes can be end-to-end encrypted between sender and recipient.
- **Asynchronous**: Sender and recipient do not need to be online at the same time. Notes are stored until fetched or until the retention period expires.
- **Tag-based routing**: Recipients subscribe to note tags rather than addresses. A tag is a 32-bit value derived from the note's metadata, allowing recipients to fetch only relevant notes.
- **No chain awareness**: The NTL does not connect to the Miden node, does not validate notes against on-chain state, and does not backfill missing data. It stores whatever the sender provides.

## Architecture at a glance

The NTL is a gRPC service backed by SQLite storage.

- **SendNote**: Clients push notes to the NTL. Each note includes a header (with NoteId and tag) and details (optionally encrypted).
- **FetchNotes**: Clients request notes matching one or more tags, using cursor-based pagination to track what they've already seen.
- **StreamNotes**: Clients subscribe to a tag and receive new notes via server-side streaming.
- **Stats**: Returns aggregate statistics about stored notes and tags.

Notes are stored with a monotonically increasing sequence number (`seq`) that serves as the pagination cursor. This avoids the timestamp-collision issues that plagued earlier designs.

## Next steps

- [Design](design.md) - how the NTL works internally
- [For operators](operators.md) - deploying and running the NTL
- [For users](users.md) - integrating with the NTL and troubleshooting

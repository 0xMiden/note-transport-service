# gRPC Reference

This is a reference of the Node's public RPC interface. It consists of a gRPC API which may be used to exchange notes with the Transport Layer.

The gRPC service definition can be found in the Miden Transport Layer's `proto`
[directory](https://github.com/0xMiden/note-transport-service/tree/main/proto/proto) in the `miden_note_transport.proto` file.

<!--toc:start-->

- [SendNote](#sendnote)
- [FetchNotes](#fetchnotes)
- [StreamNotes](#streamnotes)
- [Stats](#stats)

<!--toc:end-->

## SendNote

Pushes a note to the node.
The note is split into its header and details. The details can be encrypted.

## FetchNotes

Fetches notes from the node.
Notes with the provided tag are supplied as response.
Pagination is employed through an increasing-monotonic cursor.

## StreamNotes

Stream notes from the node to a subscribed client.
Similarly to `FetchNotes` but the node continuously provides newly received notes which have the subscribed tag.

## Stats

Gets generic statistics from the node database. Total stored notes and total (unique) tags are provided.

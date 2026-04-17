# Introduction

> [!IMPORTANT]
> The Miden Transport Layer is under heavy development. The protocol and interface may face large changes.

Welcome to the Miden Transport Layer node documentation.

This book provides two separate guides aimed at node operators and developers looking to contribute to the node
respectively. Each guide is standalone, but developers should also read through the operator guide as it provides some
additional context.

At present, the Miden Transport Layer node is the central hub responsible for exchanging private notes in the Miden ecosystem.
As Miden decentralizes, the node will morph into the official reference implementation(s) of
the various components required by a fully p2p network.

The node provides a gRPC interface for users, dApps, wallets and other entities to send and receive private notes in a secure way.
A client implementation is provided as a module in the [`miden-client`](https://github.com/0xMiden/miden-client).

## The Transport Layer

The architecture of the Transport Layer is simple.
It is based on a client-node (or, client-server) architecture, where clients exchange notes by pushing them and fetching them from the node.

The flow is as follows,
1. User sends a note to the node;
2. The note is stored for a retention period (default at 30 days). The node also labels the note with an increasing-monotonic integer cursor (currently a timestamp);
3. The recipient fetches notes by note tag. To reduce the number of fetched notes (pagination), the user may employ the cursor (only notes after this value will be provided).

The node itself may also be referred to as the transport layer.

## Feedback

Please report any issues, ask questions or leave feedback in the node repository
[here](https://github.com/0xMiden/note-transport-service/issues/new/choose).

This includes outdated, misleading, incorrect or just plain confusing information :)

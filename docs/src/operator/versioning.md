# Versioning

The Miden Transport Service follows [Semantic Versioning](https://semver.org/).

> [!IMPORTANT]
> The service is pre-1.0 and under heavy development. Until 1.0 ships, **minor** version bumps
> may include breaking changes to the gRPC API, the public Rust API of the library crates, or
> the on-disk database schema. Patch versions are non-breaking.

## Releases

Each release is published in two places:

- Source tarballs and release notes on [GitHub Releases](https://github.com/0xMiden/note-transport-service/releases).
- The four workspace crates -- `miden-note-transport-node`, `miden-note-transport-node-bin`,
  `miden-note-transport-proto`, and `miden-note-transport-proto-build` -- are versioned in
  lock-step and published to [crates.io](https://crates.io/crates/miden-note-transport-node)
  from the
  [release workflow](https://github.com/0xMiden/note-transport-service/blob/main/.github/workflows/publish-crates-release.yml).

Wire-format and schema changes between tags are called out in the release notes; operators
should consult them before upgrading.

## Compatibility with `miden-protocol`

The node depends on a specific version of [`miden-protocol`](https://github.com/0xMiden/protocol),
pinned in the workspace
[`Cargo.toml`](https://github.com/0xMiden/note-transport-service/blob/main/Cargo.toml).
`miden-protocol` defines the on-wire layout of `Note` and the other core objects travelling over
the gRPC API, so clients sending or receiving notes must serialize them with a compatible
`miden-protocol` version.

Because of that coupling, a change to how `Note` (or any other protocol object carried by the
gRPC API) is defined upstream can be a breaking change for this service, and will trigger a
corresponding version bump here. These bumps are called out in the release notes.

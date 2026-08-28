//! # Miden Transport Layer Client Library
//!
//! Implementation of the Miden Transport Layer node for private notes.
//!
//! The implementation is focused on performance and privacy.
//! Only notes with valid plaintext details are stored.
//!
//! Features include,
//! - sending and receiving notes;
//! - streaming of notes;
//! - note persistence using proven-databases and respective maintenance;
//! - metrics and traces, exported through the OpenTelemetry framework for monitoring.
//!
//! ## Database
//! Notes are stored through [`Database`](`crate::database::Database`). The storage contract is
//! private to this crate, with SQLite and PostgreSQL implementations.
//!
//! Schema migration and retention cleanup are explicit operator commands.
//!
//! ## Telemetry
//! Metrics and traces to monitor the node state are provided.
//! While metrics provide insights into general requests stats, traces can provide insights into
//! specific requests.
//! Metrics and traces can be exported following using the [OpenTelemetry](https://opentelemetry.io) framework.

#![deny(missing_docs)]

/// Database
pub mod database;
/// Error management
pub mod error;
/// Tracing, metrics export configuration
pub mod logging;
/// Metrics data structures
pub mod metrics;
/// Main node implementation
pub mod node;
/// Testing functions
///
/// Available during tests or when the `testing` feature is enabled.
#[cfg(any(test, feature = "testing"))]
pub mod test_utils;
/// Types used
pub mod types;

pub use error::{Error, Result};
pub use node::grpc::GrpcServer;
pub use node::{Node, NodeConfig};

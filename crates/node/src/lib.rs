//! # Miden Transport Layer Client Library
//!
//! Implementation of the Miden Transport Layer node for private notes.
//!
//! The implementation is focused on performance and privacy.
//! Only (optionally-encrypted) notes are stored.
//!
//! Features include,
//! - sending and receiving notes;
//! - streaming of notes;
//! - note persistence using proven-databases and respective maintenance;
//! - metrics and traces, exported through the OpenTelemetry framework for monitoring.
//!
//! ## Database
//! Notes are stored in a database, implementing the
//! [`Database`](`crate::database::DatabaseBackend`). A SQLite-based  implementation is provided.
//!
//! ### Maintenance
//! A periodic task [`DatabaseMaintenance`](`crate::database::DatabaseMaintenance`) takes care of
//! maintaining the database (cleaning up older notes).
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
/// Process shutdown signalling
pub mod shutdown;
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

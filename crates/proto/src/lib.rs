//! # Miden Note Transport Protocol Buffers
//!
//! This crate contains the generated Rust bindings for the Miden Note Transport gRPC API.

#[rustfmt::skip]
pub mod generated;

/// Encoded Protobuf file descriptor set for the Miden Note Transport API.
pub const FILE_DESCRIPTOR_SET: &[u8] =
    tonic::include_file_descriptor_set!("miden_note_transport_file_descriptor");

// RE-EXPORTS
// ================================================================================================

// Convenient re-exports for commonly used types
pub mod miden_note_transport {
    pub use super::generated::miden_note_transport::*;
}

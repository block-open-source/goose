//! Error types for the roaming transport.

use thiserror::Error;

/// Errors produced by the roaming subsystem.
#[derive(Debug, Error)]
pub enum RoamingError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("identity error: {0}")]
    Identity(String),

    #[error("card error: {0}")]
    Card(String),

    #[error("connection rejected: {0}")]
    Rejected(String),

    #[error("transport error: {0}")]
    Transport(String),
}

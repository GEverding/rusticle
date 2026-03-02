//! Error types for GIF processing operations.

use std::io;
use thiserror::Error;

/// Errors that can occur during GIF processing.
#[derive(Error, Debug)]
pub enum Error {
    /// Error during GIF decoding.
    #[error("decode error: {0}")]
    DecodeError(String),

    /// Invalid or truncated GIF data.
    #[error("invalid GIF: {0}")]
    InvalidGif(String),

    /// Error during GIF encoding.
    #[error("encode error: {0}")]
    EncodeError(String),

    /// Error during image resizing.
    #[error("resize error: {0}")]
    ResizeError(String),

    /// Invalid dimensions for resize operation.
    #[error("invalid dimensions: {0}")]
    InvalidDimensions(String),

    /// I/O error.
    #[error("io error: {0}")]
    IoError(#[from] io::Error),
}

/// Result type for GIF operations.
pub type Result<T> = std::result::Result<T, Error>;

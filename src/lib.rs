//! rusticle - High-performance GIF processing library.
//!
//! A Rust library for decoding, processing, and encoding GIF images.
//! Inspired by gifsicle, with support for resizing, optimization, and lossy compression.

pub mod decode;
pub mod encode;
pub mod error;
pub mod optimize;
pub mod resize;
pub mod types;

#[cfg(feature = "async")]
pub mod async_io;

pub use error::{Error, Result};
pub use types::{DisposalMethod, Filter, Frame, Gif, LoopCount, OptLevel, Palette};

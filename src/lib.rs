//! rusticle - High-performance GIF processing library.
//!
//! A Rust library for decoding, processing, and encoding GIF images.
//! Inspired by gifsicle, with support for resizing, optimization, and lossy compression.
//!
//! # Security
//!
//! This library includes built-in protection against GIF bombs and other malicious inputs.
//! See the [`security`] module for configurable limits.

pub mod decode;
pub mod encode;
pub mod error;
pub mod optimize;
pub mod palette_lut;
pub mod quality;
pub mod resize;
pub mod security;
pub mod types;

#[cfg(feature = "async")]
pub mod async_io;

pub use error::{Error, Result};
pub use palette_lut::{PaletteLut, PaletteMapStats};
pub use quality::QualityMetrics;
pub use security::{SecurityLimits, SecurityViolation};
pub use types::{DisposalMethod, Filter, Frame, Gif, LoopCount, OptLevel, Palette};

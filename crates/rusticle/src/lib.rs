#![feature(portable_simd)]
//! High-performance GIF processing library.
//!
//! Decode, resize, optimize, and encode GIF images. 3–6× faster than gifsicle
//! on tested inputs.
//!
//! # Example
//!
//! ```ignore
//! use rusticle::{Gif, Filter, OptLevel};
//!
//! let data = std::fs::read("input.gif")?;
//! let bytes = Gif::from_bytes(&data)?
//!     .resize(640, 480, Filter::Lanczos3)?
//!     .optimize(OptLevel::O2)
//!     .lossy(80)
//!     .to_bytes()?;
//! std::fs::write("output.gif", bytes)?;
//! ```
//!
//! # Feature flags
//!
//! - **`async`** — Async I/O via tokio (`Gif::from_async_read`, `Gif::encode_to_async_write`)
//! - **`serde`** — Serialize/deserialize `Filter`, `OptLevel`, `QualityMetrics`, etc.
//! - **`image`** — Conversions between `Frame`/`Gif` and `image::RgbaImage`

pub mod decode;
pub mod encode;
pub mod error;
pub mod optimize;
pub mod palette_lut;
pub mod quality;
pub mod resize;
pub mod simd_opt;
pub mod types;

#[cfg(feature = "async")]
pub mod async_io;

#[cfg(feature = "image")]
pub mod image_compat;

pub use encode::EncodeStats;
pub use error::{Error, Result};
pub use palette_lut::{PaletteLut, PaletteMapStats};
pub use quality::QualityMetrics;
pub use types::{DisposalMethod, Filter, Frame, Gif, LoopCount, OptLevel, Palette};

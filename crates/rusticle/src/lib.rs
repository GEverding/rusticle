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
//! - **`butteraugli`** — Perceptual image quality metrics via butteraugli

pub mod adaptive_encode;
pub mod adaptive_fallback;
pub mod adaptive_ir;
pub mod analysis_kernels;
pub mod candidate_gen;
pub mod classifier;
pub mod decode;
pub mod encode;
pub mod encode_and_measure;
pub mod error;
pub mod lut_policy;
pub mod materialize;
pub mod optimize;
pub mod palette_lut;
pub mod palette_realize;
pub mod palette_strategy;
pub mod path_a;
pub mod path_a_palette;
pub mod path_b;
pub mod profiler;
pub mod quality;
pub mod resize;
pub mod scoring;
pub mod sequence_optimizer;
pub mod simd_opt;
pub mod tier0_classifier;
pub mod tier1_pruning;
pub mod tier2_measure;
pub mod two_path_router;
pub mod types;
pub mod voyager_exact_bbox_global_palette;
pub mod voyager_exact_bbox_global_palette_with_fallback;
pub mod voyager_repr;
pub mod voyager_source_reuse;

#[cfg(feature = "async")]
pub mod async_io;

#[cfg(feature = "image")]
pub mod image_compat;

pub use adaptive_encode::{AdaptiveConfig, AdaptiveDecision};
pub use adaptive_fallback::{
    AdaptiveBytesPreparer, AdaptiveStage, FallbackReason, FallbackTelemetry,
};
pub use adaptive_ir::{
    BoundingBox, CanonicalFrame, CanonicalSequence, CanonicalSequenceBuilder, Canvas,
    ChangedRegion, SourcePatch,
};
pub use analysis_kernels::{
    analyze_changed_pixels_scalar, analyze_changed_pixels_simd, analyze_color_distance_scalar,
    analyze_color_distance_simd, analyze_transparency_scalar, analyze_transparency_simd,
    ChangedPixelStats, ColorDistanceStats, TransparencyStats,
};
pub use candidate_gen::{
    Candidate, CandidateGenerator, CandidateMetadata, CandidateRepresentation, SafetyReason,
};
pub use classifier::{
    classify_from_profile, classify_sequence, ClassificationFeatures, ClassificationResult,
    OptimizerPath,
};
pub use encode::EncodeStats;
pub use encode_and_measure::{
    EncodeAndMeasure, EncodeAndMeasureConfig, EncodeAndMeasureTelemetry, MeasuredCandidate,
};
pub use error::{Error, Result};
pub use lut_policy::{
    candidate_to_family, CandidateFamily, CpuBudgetClass, LutEligibility, PolicySignals,
    QuantizationCostClass,
};
pub use materialize::Materializer;
pub use palette_lut::{PaletteLut, PaletteMapStats};
pub use palette_realize::{PaletteRealization, PaletteRealizer, QuantizedFrameData};
pub use palette_strategy::{
    determine_palette_strategies, PaletteStrategy, PaletteStrategyMetadata, PaletteStrategySet,
    StrategyReason,
};
pub use path_a::{optimize_path_a, PathAConfig, PathAFrame};
pub use path_a_palette::{
    PathAPaletteConfig, PathAPaletteRealization, PathAPaletteRealizer, PathAPaletteStats,
    PathAQuantizedFrame,
};
pub use path_b::{optimize_path_b, optimize_path_b_lossy, PathBConfig};
pub use profiler::{
    ChangeStatistics, DeltaSignal, DisposalDistribution, GifProfile, PaletteInfo, PatchDensity,
    SequenceMetrics, SequenceTaxonomy, TransparencyAnalysis,
};
pub use quality::QualityMetrics;
pub use scoring::{
    Chooser, DecisionReason, FrameDecision, ScoreBreakdown, Scorer, SequenceDecision,
};
pub use sequence_optimizer::{SequenceOptimizer, SequenceOptimizerConfig};
pub use tier0_classifier::{Tier0Classifier, Tier0Decision};
pub use tier1_pruning::{PruneReason, PruneResult, Tier1Pruner};
pub use tier2_measure::{
    MeasuredResult, MeasurementBudget, QualityGuardrails, Tier2Measurer, Tier2Telemetry,
};
pub use two_path_router::{
    route_optimize, OptimizerStrategy, TwoPathConfig, TwoPathResult, TwoPathTelemetry,
};
pub use types::{DisposalMethod, Filter, Frame, Gif, LoopCount, OptLevel, Palette};
pub use voyager_exact_bbox_global_palette::{
    VoyagerExactBboxGlobalPaletteBuilder, VoyagerExactBboxGlobalPaletteFrame,
    VoyagerExactBboxGlobalPaletteRepr,
};
pub use voyager_exact_bbox_global_palette_with_fallback::{
    VoyagerExactBboxGlobalPaletteFallbackBuilder, VoyagerExactBboxGlobalPaletteFallbackFrame,
    VoyagerExactBboxGlobalPaletteFallbackRepr,
};
pub use voyager_repr::{VoyagerBuilder, VoyagerFrame, VoyagerRepr};
pub use voyager_source_reuse::{
    SourceReuseViability, VoyagerSourceReuseBuilder, VoyagerSourceReuseFrame,
    VoyagerSourceReuseRepr,
};

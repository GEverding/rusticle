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
//! - **`imagequant`** — Higher-quality color quantization via imagequant (GPL-3.0 licensed dependency — enables GPL obligations)
//! - **`serde`** — Serialize/deserialize `Filter`, `OptLevel`, `QualityMetrics`, etc.
//! - **`image`** — Conversions between `Frame`/`Gif` and `image::RgbaImage`
//! - **`butteraugli`** — Perceptual image quality metrics via butteraugli
//! - **`research`** — Experimental research modules (implies imagequant)

pub mod decode;
pub mod encode;
pub mod error;
pub mod gif_ops;
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

#[cfg(feature = "research")]
pub mod adaptive_encode;
#[cfg(feature = "research")]
pub mod adaptive_fallback;
#[cfg(feature = "research")]
pub mod adaptive_ir;
#[cfg(feature = "research")]
pub mod analysis_kernels;
#[cfg(feature = "research")]
pub mod candidate_gen;
#[cfg(feature = "research")]
pub mod classifier;
#[cfg(feature = "research")]
pub mod encode_and_measure;
#[cfg(feature = "research")]
pub mod lut_policy;
#[cfg(feature = "research")]
pub mod materialize;
#[cfg(feature = "research")]
pub mod palette_realize;
#[cfg(feature = "research")]
pub mod palette_strategy;
#[cfg(feature = "research")]
pub mod path_a;
#[cfg(feature = "research")]
pub mod path_a_palette;
#[cfg(feature = "research")]
pub mod path_b;
#[cfg(feature = "research")]
pub mod profiler;
#[cfg(feature = "research")]
pub mod repr_study;
#[cfg(feature = "research")]
pub mod scoring;
#[cfg(feature = "research")]
pub mod sequence_optimizer;
#[cfg(feature = "research")]
pub mod tier0_classifier;
#[cfg(feature = "research")]
pub mod tier1_pruning;
#[cfg(feature = "research")]
pub mod tier2_measure;
#[cfg(feature = "research")]
pub mod tiered_optimizer;
#[cfg(feature = "research")]
pub mod two_path;
#[cfg(feature = "research")]
pub mod two_path_router;
#[cfg(feature = "research")]
pub mod voyager_exact_bbox_global_palette;
#[cfg(feature = "research")]
pub mod voyager_exact_bbox_global_palette_with_fallback;
#[cfg(feature = "research")]
pub mod voyager_repr;
#[cfg(feature = "research")]
pub mod voyager_source_reuse;

#[cfg(feature = "research")]
pub use adaptive_encode::{AdaptiveConfig, AdaptiveDecision};
#[cfg(feature = "research")]
pub use adaptive_fallback::{
    AdaptiveBytesPreparer, AdaptiveStage, FallbackReason, FallbackTelemetry,
};
#[cfg(feature = "research")]
pub use adaptive_ir::{
    BoundingBox, CanonicalFrame, CanonicalSequence, CanonicalSequenceBuilder, Canvas,
    ChangedRegion, SourcePatch,
};
#[cfg(feature = "research")]
pub use analysis_kernels::{
    analyze_changed_pixels_scalar, analyze_changed_pixels_simd, analyze_color_distance_scalar,
    analyze_color_distance_simd, analyze_transparency_scalar, analyze_transparency_simd,
    ChangedPixelStats, ColorDistanceStats, TransparencyStats,
};
#[cfg(feature = "research")]
pub use candidate_gen::{
    Candidate, CandidateGenerator, CandidateMetadata, CandidateRepresentation, SafetyReason,
};
#[cfg(feature = "research")]
pub use classifier::{
    classify_from_profile, classify_sequence, ClassificationFeatures, ClassificationResult,
    OptimizerPath,
};
pub use encode::EncodeStats;
#[cfg(feature = "research")]
pub use encode_and_measure::{
    EncodeAndMeasure, EncodeAndMeasureConfig, EncodeAndMeasureTelemetry, MeasuredCandidate,
};
pub use error::{Error, Result};
#[cfg(feature = "research")]
pub use lut_policy::{
    candidate_to_family, CandidateFamily, CpuBudgetClass, LutEligibility, PolicySignals,
    QuantizationCostClass,
};
#[cfg(feature = "research")]
pub use materialize::Materializer;
pub use palette_lut::{PaletteLut, PaletteMapStats};
#[cfg(feature = "research")]
pub use palette_realize::{PaletteRealization, PaletteRealizer, QuantizedFrameData};
#[cfg(feature = "research")]
pub use palette_strategy::{
    determine_palette_strategies, PaletteStrategy, PaletteStrategyMetadata, PaletteStrategySet,
    StrategyReason,
};
#[cfg(feature = "research")]
pub use profiler::{
    ChangeStatistics, DeltaSignal, DisposalDistribution, GifProfile, PaletteInfo, PatchDensity,
    SequenceMetrics, SequenceTaxonomy, TransparencyAnalysis,
};
pub use quality::QualityMetrics;
#[cfg(feature = "research")]
pub use repr_study::{
    ReprStrategy, ReprStudyOutput, SourceReuseViability, VoyagerBuilder,
    VoyagerExactBboxGlobalPaletteBuilder, VoyagerExactBboxGlobalPaletteFallbackBuilder,
    VoyagerExactBboxGlobalPaletteFallbackFrame, VoyagerExactBboxGlobalPaletteFallbackRepr,
    VoyagerExactBboxGlobalPaletteFrame, VoyagerExactBboxGlobalPaletteRepr, VoyagerFrame,
    VoyagerRepr, VoyagerSourceReuseBuilder, VoyagerSourceReuseFrame, VoyagerSourceReuseRepr,
};
#[cfg(feature = "research")]
pub use scoring::{
    Chooser, DecisionReason, FrameDecision, ScoreBreakdown, Scorer, SequenceDecision,
};
#[cfg(feature = "research")]
pub use sequence_optimizer::{SequenceOptimizer, SequenceOptimizerConfig};
#[cfg(feature = "research")]
pub use tiered_optimizer::{
    MeasuredResult, MeasurementBudget, PruneReason, PruneResult, QualityGuardrails,
    Tier0Classifier, Tier0Decision, Tier1Pruner, Tier2Measurer, Tier2Telemetry, UncertaintyReason,
};
#[cfg(feature = "research")]
pub use two_path::{
    optimize_path_a, optimize_path_b, optimize_path_b_lossy, route_optimize, OptimizerStrategy,
    PathAConfig, PathAFrame, PathAPaletteConfig, PathAPaletteRealization, PathAPaletteRealizer,
    PathAPaletteStats, PathAQuantizedFrame, PathBConfig, TwoPathConfig, TwoPathResult,
    TwoPathTelemetry,
};
pub use types::{DisposalMethod, Filter, Frame, Gif, LoopCount, OptLevel, Palette};

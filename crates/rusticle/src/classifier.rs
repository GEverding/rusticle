//! Deterministic two-path classifier for routing GIF sequences.
//!
//! **STATUS**: Research / Future Opt-In (not current mainline product path)
//!
//! This module implements a simple, explainable classifier that routes a decoded GIF sequence
//! to one of two optimization paths:
//!
//! - **Path A (Conservative Opaque-Delta Reconstruction)**: For GIFs structurally close to
//!   optimal GIF-native streams (no transparency, stable/global palette, offset subframes,
//!   Keep/None disposal, low changed-area ratio).
//!
//! - **Path B (General Sparse/Transparent Optimization)**: For transparency-heavy,
//!   disposal-heavy, mixed, or non-obviously opaque-delta GIFs.
//!
//! # Classification Strategy
//!
//! The classifier is **conservative**: it only chooses Path A when the sequence strongly
//! looks like already-optimized opaque-delta/global-palette content. Everything else routes
//! to Path B.
//!
//! # Classification Features
//!
//! All features are computed from the decoded canonical sequence (before resize):
//!
//! - **Transparent GCE usage**: Whether any frame uses transparency (Graphics Control Extension).
//! - **Palette stability**: Ratio of frames using global palette vs per-frame local palettes.
//! - **Disposal mix**: Fraction of frames using Keep/None disposal vs Background/Previous.
//! - **Offset subframe prevalence**: Fraction of frames with non-full-canvas offsets.
//! - **Changed-area ratio**: Median bbox-of-change / canvas-area across consecutive displayed frames.
//!
//! # Path A Criteria (All Must Hold)
//!
//! - No transparent GCEs
//! - ≥90% frames use Keep or None disposal
//! - Global palette or ≥80% palette stability
//! - Median changed-area ratio ≤ 0.6 (authored delta behavior)
//!
//! Everything else → Path B.
//!
//! # What It Was Trying to Solve
//!
//! The two-path system explored whether a simple, deterministic classifier could route different
//! GIF classes to different optimization strategies without expensive measurement overhead.
//! This classifier tests that hypothesis: route opaque-delta GIFs to Path A, everything else to Path B.
//!
//! # Structural Assumptions
//!
//! - Classification is deterministic (same input always produces same output)
//! - Features are computed from decoded canonical sequence (before resize)
//! - Two bounded paths: Path A (conservative opaque-delta) and Path B (general sparse/transparent)
//! - No measurement overhead (classification is fast)
//!
//! # Latest Evidence
//!
//! The classifier is simple and deterministic, but may be too conservative (routes too many GIFs
//! to Path B). Validation on a larger, more diverse GIF corpus is needed before promoting to mainline.
//!
//! # Rationale
//!
//! This differs from the old adaptive/tiered approach in several key ways:
//!
//! 1. **Bounded decision space**: Only two paths, not a spectrum of tiers.
//! 2. **Deterministic**: Same input always produces same output (no randomness or measurement).
//! 3. **Explainable**: Each decision is based on structural facts we already know are useful.
//! 4. **Conservative**: Path A is only chosen when we're confident the sequence is already optimized.
//! 5. **No measurement overhead**: Classification is fast and doesn't require encode-and-measure.
//!
//! The old adaptive optimizer tried to choose a single policy (tier0 → tier1 → tier2) that
//! would work for all GIF classes. This classifier recognizes that opaque-delta GIFs and
//! transparency-heavy GIFs have fundamentally different optimal strategies, and routes them
//! accordingly.
//!
//! See `docs/RESEARCH_VOYAGER_AND_TWO_PATH.md` for full context.

use crate::adaptive_ir::CanonicalSequence;
use crate::profiler::{profile_canonical_sequence, GifProfile};
use std::fmt;

/// Optimizer path decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptimizerPath {
    /// Conservative opaque-delta reconstruction.
    /// For GIFs structurally close to optimal GIF-native streams.
    PathA,
    /// General sparse/transparent optimization.
    /// For transparency-heavy, disposal-heavy, mixed, or non-obviously opaque-delta GIFs.
    PathB,
}

impl OptimizerPath {
    /// Human-readable name for the path.
    pub fn name(&self) -> &'static str {
        match self {
            Self::PathA => "Path A (Conservative Opaque-Delta)",
            Self::PathB => "Path B (General Sparse/Transparent)",
        }
    }
}

/// Detailed classification result with feature values and reasoning.
#[derive(Debug, Clone)]
pub struct ClassificationResult {
    /// The selected optimizer path.
    pub path: OptimizerPath,
    /// Feature values used in classification.
    pub features: ClassificationFeatures,
    /// Reasons why this path was selected.
    pub reasons: Vec<ClassificationReason>,
}

/// Reason a path was selected.
#[derive(Debug, Clone, PartialEq)]
pub enum ClassificationReason {
    /// Transparent GCE was present.
    HasTransparentGce,
    /// Keep/None ratio was below the Path A threshold.
    KeepNoneDisposalRatioBelow { ratio: f32 },
    /// Keep/None ratio met the Path A threshold.
    KeepNoneDisposalRatioAtLeast { ratio: f32 },
    /// Palette stability was below the Path A threshold.
    PaletteStabilityBelow { stability: f32 },
    /// Palette stability met the Path A threshold.
    PaletteStabilityAtLeast { stability: f32 },
    /// Changed-area ratio was above the Path A threshold.
    MedianChangedAreaRatioAbove { ratio: f32 },
    /// Changed-area ratio met the Path A threshold.
    MedianChangedAreaRatioAtMost { ratio: f32 },
    /// All criteria were met.
    AllCriteriaMet,
    /// Not all criteria were met.
    NotAllCriteriaMet,
}

impl fmt::Display for ClassificationReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HasTransparentGce => write!(f, "Has transparent GCE → Path B"),
            Self::KeepNoneDisposalRatioBelow { ratio } => {
                write!(
                    f,
                    "Keep/None disposal ratio {:.1}% < 90% → Path B",
                    ratio * 100.0
                )
            }
            Self::KeepNoneDisposalRatioAtLeast { ratio } => {
                write!(f, "Keep/None disposal ratio {:.1}% ≥ 90% ✓", ratio * 100.0)
            }
            Self::PaletteStabilityBelow { stability } => {
                write!(
                    f,
                    "Palette stability {:.1}% < 80% → Path B",
                    stability * 100.0
                )
            }
            Self::PaletteStabilityAtLeast { stability } => {
                write!(f, "Palette stability {:.1}% ≥ 80% ✓", stability * 100.0)
            }
            Self::MedianChangedAreaRatioAbove { ratio } => {
                write!(f, "Median changed-area ratio {:.2} > 0.6 → Path B", ratio)
            }
            Self::MedianChangedAreaRatioAtMost { ratio } => {
                write!(f, "Median changed-area ratio {:.2} ≤ 0.6 ✓", ratio)
            }
            Self::AllCriteriaMet => write!(f, "All criteria met → Path A"),
            Self::NotAllCriteriaMet => write!(f, "Not all criteria met → Path B"),
        }
    }
}

/// Feature values extracted during classification.
#[derive(Debug, Clone)]
pub struct ClassificationFeatures {
    /// Whether any frame uses transparent GCE.
    pub has_transparent_gce: bool,
    /// Fraction of frames using Keep or None disposal (0.0 to 1.0).
    pub keep_none_disposal_ratio: f32,
    /// Palette stability: 1.0 = all global, 0.0 = all local (0.0 to 1.0).
    pub palette_stability: f32,
    /// Fraction of frames with offset patches (0.0 to 1.0).
    pub offset_patch_ratio: f32,
    /// Median changed-area ratio across consecutive frames (0.0 to 1.0).
    pub median_changed_area_ratio: f32,
}

/// Classify a canonical sequence and return the optimizer path.
///
/// # Arguments
///
/// * `seq` - The canonical sequence to classify.
///
/// # Returns
///
/// A `ClassificationResult` containing the selected path, feature values, and reasoning.
///
/// # Errors
///
/// Returns an error if profiling the sequence fails.
pub fn classify_sequence(seq: &CanonicalSequence) -> crate::Result<ClassificationResult> {
    let profile = profile_canonical_sequence(seq)?;
    Ok(classify_from_profile(seq, &profile))
}

/// Classify a sequence given a pre-computed profile.
///
/// This is useful when you already have a profile and want to avoid recomputing it.
///
/// # Arguments
///
/// * `seq` - The canonical sequence to classify.
/// * `profile` - The pre-computed profile.
///
/// # Returns
///
/// A `ClassificationResult` containing the selected path, feature values, and reasoning.
pub fn classify_from_profile(
    seq: &CanonicalSequence,
    profile: &GifProfile,
) -> ClassificationResult {
    let features = extract_features(seq, profile);
    let (path, reasons) = decide_path(&features);

    ClassificationResult {
        path,
        features,
        reasons,
    }
}

/// Extract classification features from a canonical sequence and profile.
fn extract_features(seq: &CanonicalSequence, profile: &GifProfile) -> ClassificationFeatures {
    // Feature 1: Transparent GCE usage
    let has_transparent_gce = profile.transparency_analysis.uses_gce;

    // Feature 2: Keep/None disposal ratio
    let total_frames = profile.metrics.frame_count;
    let keep_none_count =
        profile.disposal_distribution.keep_count + profile.disposal_distribution.none_count;
    let keep_none_disposal_ratio = if total_frames > 0 {
        keep_none_count as f32 / total_frames as f32
    } else {
        1.0
    };

    // Feature 3: Palette stability
    let palette_stability = profile.palette_info.palette_stability;

    // Feature 4: Offset patch ratio
    let offset_patch_ratio = if total_frames > 0 {
        profile.patch_density.offset_patch_frames as f32 / total_frames as f32
    } else {
        0.0
    };

    // Feature 5: Median changed-area ratio
    // Compute changed-area ratio for each frame (excluding frame 0 which is always full-frame)
    let mut changed_ratios = Vec::new();
    for frame in &seq.frames[1..] {
        changed_ratios.push(frame.changed_region.changed_ratio);
    }

    let median_changed_area_ratio = if !changed_ratios.is_empty() {
        changed_ratios.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mid = changed_ratios.len() / 2;
        if changed_ratios.len() % 2 == 0 && mid > 0 {
            (changed_ratios[mid - 1] + changed_ratios[mid]) / 2.0
        } else {
            changed_ratios[mid]
        }
    } else {
        0.0
    };

    ClassificationFeatures {
        has_transparent_gce,
        keep_none_disposal_ratio,
        palette_stability,
        offset_patch_ratio,
        median_changed_area_ratio,
    }
}

/// Decide which path to use based on extracted features.
///
/// Returns the selected path and a list of reasons explaining the decision.
fn decide_path(features: &ClassificationFeatures) -> (OptimizerPath, Vec<ClassificationReason>) {
    let mut reasons = Vec::new();
    let mut path_a_eligible = true;

    // Criterion 1: No transparent GCEs
    if features.has_transparent_gce {
        reasons.push(ClassificationReason::HasTransparentGce);
        path_a_eligible = false;
    } else {
        // Kept implicit in the typed reason bag.
    }

    // Criterion 2: ≥90% frames use Keep or None disposal
    if features.keep_none_disposal_ratio < 0.9 {
        reasons.push(ClassificationReason::KeepNoneDisposalRatioBelow {
            ratio: features.keep_none_disposal_ratio,
        });
        path_a_eligible = false;
    } else {
        reasons.push(ClassificationReason::KeepNoneDisposalRatioAtLeast {
            ratio: features.keep_none_disposal_ratio,
        });
    }

    // Criterion 3: Global palette or ≥80% palette stability
    if features.palette_stability < 0.8 {
        reasons.push(ClassificationReason::PaletteStabilityBelow {
            stability: features.palette_stability,
        });
        path_a_eligible = false;
    } else {
        reasons.push(ClassificationReason::PaletteStabilityAtLeast {
            stability: features.palette_stability,
        });
    }

    // Criterion 4: Median changed-area ratio ≤ 0.6
    if features.median_changed_area_ratio > 0.6 {
        reasons.push(ClassificationReason::MedianChangedAreaRatioAbove {
            ratio: features.median_changed_area_ratio,
        });
        path_a_eligible = false;
    } else {
        reasons.push(ClassificationReason::MedianChangedAreaRatioAtMost {
            ratio: features.median_changed_area_ratio,
        });
    }

    let path = if path_a_eligible {
        reasons.push(ClassificationReason::AllCriteriaMet);
        OptimizerPath::PathA
    } else {
        reasons.push(ClassificationReason::NotAllCriteriaMet);
        OptimizerPath::PathB
    };

    (path, reasons)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DisposalMethod, LoopCount};
    use std::time::Duration;

    /// Helper to create a test canonical sequence.
    fn create_test_sequence(
        width: u16,
        height: u16,
        frame_count: usize,
        has_transparency: bool,
        disposal: DisposalMethod,
        changed_ratio: f32,
    ) -> CanonicalSequence {
        use crate::adaptive_ir::{BoundingBox, CanonicalFrame, Canvas, ChangedRegion, SourcePatch};

        let canvas_area = (width as usize) * (height as usize);
        let mut frames = Vec::new();

        for i in 0..frame_count {
            let is_full_canvas = i == 0;
            let changed_pixels = ((canvas_area as f32) * changed_ratio) as usize;

            let source_patch = SourcePatch {
                pixels: vec![255; width as usize * height as usize * 4],
                left: 0,
                top: 0,
                width,
                height,
                has_transparency,
                transparent_pixel_count: if has_transparency {
                    canvas_area / 10
                } else {
                    0
                },
                opaque_pixel_count: if has_transparency {
                    canvas_area - canvas_area / 10
                } else {
                    canvas_area
                },
            };

            let canvas = Canvas::new(width, height);

            frames.push(CanonicalFrame {
                source_patch,
                pre_draw_canvas: canvas.clone_canvas(),
                displayed_canvas: canvas.clone_canvas(),
                post_disposal_canvas: canvas.clone_canvas(),
                changed_region: ChangedRegion {
                    bbox: BoundingBox::new(0, 0, width, height),
                    changed_pixel_count: changed_pixels,
                    changed_ratio,
                    is_full_canvas_patch: is_full_canvas,
                },
                delay: Duration::from_millis(100),
                dispose: disposal,
            });
        }

        CanonicalSequence {
            width,
            height,
            loop_count: LoopCount::Infinite,
            frames,
        }
    }

    #[test]
    fn test_voyager_like_sequence_path_a() {
        // Voyager-like: opaque, Keep disposal, low changed-area ratio
        let seq = create_test_sequence(
            320,
            240,
            10,
            false,                // no transparency
            DisposalMethod::Keep, // Keep disposal
            0.3,                  // low changed-area ratio
        );

        let result = classify_sequence(&seq).expect("classification failed");
        assert_eq!(
            result.path,
            OptimizerPath::PathA,
            "Voyager-like should be Path A"
        );
        assert!(!result.features.has_transparent_gce);
        assert!(result.features.keep_none_disposal_ratio >= 0.9);
        assert!(result.features.palette_stability >= 0.8);
        assert!(result.features.median_changed_area_ratio <= 0.6);
    }

    #[test]
    fn test_transparency_heavy_sequence_path_b() {
        // Transparency-heavy: has transparency
        let seq = create_test_sequence(
            320,
            240,
            10,
            true, // has transparency
            DisposalMethod::Keep,
            0.3,
        );

        let result = classify_sequence(&seq).expect("classification failed");
        assert_eq!(
            result.path,
            OptimizerPath::PathB,
            "Transparency-heavy should be Path B"
        );
        assert!(result.features.has_transparent_gce);
    }

    #[test]
    fn test_disposal_background_sequence_path_b() {
        // Disposal-heavy: Background disposal
        let seq = create_test_sequence(
            320,
            240,
            10,
            false,
            DisposalMethod::Background, // Background disposal
            0.3,
        );

        let result = classify_sequence(&seq).expect("classification failed");
        assert_eq!(
            result.path,
            OptimizerPath::PathB,
            "Disposal-heavy should be Path B"
        );
        assert!(result.features.keep_none_disposal_ratio < 0.9);
    }

    #[test]
    fn test_disposal_previous_sequence_path_b() {
        // Disposal-heavy: Previous disposal
        let seq = create_test_sequence(
            320,
            240,
            10,
            false,
            DisposalMethod::Previous, // Previous disposal
            0.3,
        );

        let result = classify_sequence(&seq).expect("classification failed");
        assert_eq!(
            result.path,
            OptimizerPath::PathB,
            "Disposal-heavy should be Path B"
        );
        assert!(result.features.keep_none_disposal_ratio < 0.9);
    }

    #[test]
    fn test_high_changed_area_sequence_path_b() {
        // High changed-area ratio
        let seq = create_test_sequence(
            320,
            240,
            10,
            false,
            DisposalMethod::Keep,
            0.8, // high changed-area ratio
        );

        let result = classify_sequence(&seq).expect("classification failed");
        assert_eq!(
            result.path,
            OptimizerPath::PathB,
            "High changed-area should be Path B"
        );
        assert!(result.features.median_changed_area_ratio > 0.6);
    }

    #[test]
    fn test_deterministic_classification() {
        // Same input should always produce same output
        let seq = create_test_sequence(320, 240, 10, false, DisposalMethod::Keep, 0.3);

        let result1 = classify_sequence(&seq).expect("classification 1 failed");
        let result2 = classify_sequence(&seq).expect("classification 2 failed");

        assert_eq!(
            result1.path, result2.path,
            "Classification should be deterministic"
        );
        assert_eq!(
            result1.features.has_transparent_gce,
            result2.features.has_transparent_gce
        );
        assert_eq!(
            result1.features.keep_none_disposal_ratio,
            result2.features.keep_none_disposal_ratio
        );
    }

    #[test]
    fn test_mixed_disposal_sequence_path_b() {
        // Mixed disposal methods should fail the ≥90% criterion
        // Create a sequence with mixed disposal by manually constructing frames
        use crate::adaptive_ir::{BoundingBox, CanonicalFrame, Canvas, ChangedRegion, SourcePatch};

        let width = 320u16;
        let height = 240u16;
        let canvas_area = (width as usize) * (height as usize);

        let mut frames = Vec::new();
        let disposals = vec![
            DisposalMethod::Keep,
            DisposalMethod::Keep,
            DisposalMethod::Keep,
            DisposalMethod::Keep,
            DisposalMethod::Keep,
            DisposalMethod::Keep,
            DisposalMethod::Keep,
            DisposalMethod::Keep,
            DisposalMethod::Background,
            DisposalMethod::Previous,
        ];

        for (i, &disposal) in disposals.iter().enumerate() {
            let is_full_canvas = i == 0;
            let source_patch = SourcePatch {
                pixels: vec![255; width as usize * height as usize * 4],
                left: 0,
                top: 0,
                width,
                height,
                has_transparency: false,
                transparent_pixel_count: 0,
                opaque_pixel_count: canvas_area,
            };

            let canvas = Canvas::new(width, height);

            frames.push(CanonicalFrame {
                source_patch,
                pre_draw_canvas: canvas.clone_canvas(),
                displayed_canvas: canvas.clone_canvas(),
                post_disposal_canvas: canvas.clone_canvas(),
                changed_region: ChangedRegion {
                    bbox: BoundingBox::new(0, 0, width, height),
                    changed_pixel_count: (canvas_area as f32 * 0.3) as usize,
                    changed_ratio: 0.3,
                    is_full_canvas_patch: is_full_canvas,
                },
                delay: Duration::from_millis(100),
                dispose: disposal,
            });
        }

        let seq = CanonicalSequence {
            width,
            height,
            loop_count: LoopCount::Infinite,
            frames,
        };

        let result = classify_sequence(&seq).expect("classification failed");
        assert_eq!(
            result.path,
            OptimizerPath::PathB,
            "Mixed disposal should be Path B"
        );
        // 8 Keep/None out of 10 = 80%, which is < 90%
        assert!(result.features.keep_none_disposal_ratio < 0.9);
    }

    #[test]
    fn test_reasons_are_populated() {
        let seq = create_test_sequence(320, 240, 10, false, DisposalMethod::Keep, 0.3);

        let result = classify_sequence(&seq).expect("classification failed");
        assert!(!result.reasons.is_empty(), "Reasons should be populated");
        assert!(
            result
                .reasons
                .iter()
                .any(|r| matches!(r, ClassificationReason::AllCriteriaMet)),
            "Should have Path A decision reason"
        );
    }
}

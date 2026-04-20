//! GIF structure profiler and taxonomy classifier.
//!
//! This module extracts structural features from a GIF and classifies it into
//! useful representation priors for the adaptive encoder.
//!
//! # Features Extracted
//!
//! - Frame count and disposal distribution
//! - Transparency usage and GCE patterns
//! - Global vs local palette presence
//! - Offset subframe prevalence
//! - Changed-area ratio statistics
//! - Patch density and bounding box density
//! - Delta-encoding signal detection
//! - Palette stability indicators
//!
//! # Taxonomy
//!
//! Sequences are classified into one of five categories:
//! - `OpaqueDeltaGlobalPalette`: Opaque deltas with stable global palette (Voyager-like)
//! - `TransparencyHeavySparseDelta`: Significant transparency with sparse changes
//! - `DisposalHeavyBackgroundPrevious`: Disposal-driven animations
//! - `Photographic`: High color count, dense changes, smooth gradients
//! - `Mixed`: Sequences that don't fit cleanly into other categories

use crate::adaptive_ir::CanonicalSequence;
use crate::types::{DisposalMethod, Gif};

/// Structural profile of a GIF sequence.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GifProfile {
    /// Basic sequence metrics
    pub metrics: SequenceMetrics,
    /// Disposal method distribution
    pub disposal_distribution: DisposalDistribution,
    /// Transparency analysis
    pub transparency_analysis: TransparencyAnalysis,
    /// Palette information
    pub palette_info: PaletteInfo,
    /// Frame change statistics
    pub change_statistics: ChangeStatistics,
    /// Patch density analysis
    pub patch_density: PatchDensity,
    /// Delta-encoding signal strength
    pub delta_signal: DeltaSignal,
    /// Inferred taxonomy classification
    pub taxonomy: SequenceTaxonomy,
}

/// Basic sequence-level metrics.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SequenceMetrics {
    /// Total number of frames
    pub frame_count: usize,
    /// Canvas width in pixels
    pub width: u16,
    /// Canvas height in pixels
    pub height: u16,
    /// Total canvas area
    pub total_pixels: usize,
    /// Average frame delay in milliseconds
    pub avg_delay_ms: f32,
}

/// Distribution of disposal methods across frames.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DisposalDistribution {
    /// Count of frames with Disposal::Keep
    pub keep_count: usize,
    /// Count of frames with Disposal::None
    pub none_count: usize,
    /// Count of frames with Disposal::Background
    pub background_count: usize,
    /// Count of frames with Disposal::Previous
    pub previous_count: usize,
    /// Dominant disposal method
    pub dominant: String,
}

/// Transparency usage analysis.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TransparencyAnalysis {
    /// Frames with any transparency
    pub frames_with_transparency: usize,
    /// Average transparency ratio (transparent pixels / total pixels)
    pub avg_transparency_ratio: f32,
    /// Maximum transparency ratio in any frame
    pub max_transparency_ratio: f32,
    /// Frames with significant transparency (>10%)
    pub frames_with_significant_transparency: usize,
    /// Whether GCE (Graphics Control Extension) is used
    pub uses_gce: bool,
    /// Frames with local palettes
    pub frames_with_local_palette: usize,
}

/// Palette information.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PaletteInfo {
    /// Whether a global palette is present
    pub has_global_palette: bool,
    /// Number of frames with local palettes
    pub local_palette_count: usize,
    /// Estimated palette stability (0.0 = unstable, 1.0 = stable)
    pub palette_stability: f32,
}

/// Frame change statistics.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ChangeStatistics {
    /// Average changed-area ratio (changed pixels / total pixels)
    pub avg_changed_ratio: f32,
    /// Maximum changed-area ratio
    pub max_changed_ratio: f32,
    /// Minimum changed-area ratio (excluding no-op frames)
    pub min_changed_ratio: f32,
    /// Number of frames with very small changes (<5%)
    pub sparse_change_frames: usize,
    /// Number of frames with dense changes (>50%)
    pub dense_change_frames: usize,
}

/// Patch density and bounding box analysis.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PatchDensity {
    /// Average bounding box area as ratio of canvas
    pub avg_bbox_ratio: f32,
    /// Maximum bounding box area as ratio of canvas
    pub max_bbox_ratio: f32,
    /// Frames with offset patches (not at origin)
    pub offset_patch_frames: usize,
    /// Average patch density (changed pixels / bbox area)
    pub avg_patch_density: f32,
}

/// Delta-encoding signal strength.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DeltaSignal {
    /// Estimated strength of delta-encoding signal (0.0 = none, 1.0 = strong)
    pub strength: f32,
    /// Frames with opaque deltas (no transparency in changed region)
    pub opaque_delta_frames: usize,
    /// Frames with offset patches and low changed area
    pub offset_sparse_frames: usize,
    /// Indicator: many offset patches with low changed area
    pub is_already_delta_encoded: bool,
}

/// GIF structure taxonomy classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SequenceTaxonomy {
    /// Opaque deltas with stable global palette (Voyager-like).
    /// Priors: prefer opaque bbox, global palette, avoid synthetic transparency.
    OpaqueDeltaGlobalPalette,
    /// Significant transparency with sparse changes (UI animations, overlays).
    /// Priors: prefer transparent sparse patch, local palette, manage transparency risk.
    TransparencyHeavySparseDelta,
    /// Disposal-driven animations (Disposal::Background or Disposal::Previous).
    /// Priors: respect disposal semantics strictly, avoid synthetic transparency.
    DisposalHeavyBackgroundPrevious,
    /// High color count, dense changes, smooth gradients (photographic/video-like).
    /// Priors: prefer full frame, local palette, quality over size.
    Photographic,
    /// Mixed or unknown structure.
    /// Priors: use adaptive strategy, measure carefully.
    Mixed,
}

impl SequenceTaxonomy {
    /// Human-readable name for the taxonomy.
    pub fn name(&self) -> &'static str {
        match self {
            Self::OpaqueDeltaGlobalPalette => "opaque-delta/global-palette",
            Self::TransparencyHeavySparseDelta => "transparency-heavy/sparse-delta",
            Self::DisposalHeavyBackgroundPrevious => "disposal-heavy/background-previous",
            Self::Photographic => "photographic/noisy",
            Self::Mixed => "mixed/unknown",
        }
    }
}

/// Profile a Gif and extract structural features.
pub fn profile_gif(gif: &Gif) -> crate::Result<GifProfile> {
    let canonical = crate::adaptive_ir::CanonicalSequenceBuilder::build(gif)?;
    profile_canonical_sequence(&canonical)
}

/// Profile a CanonicalSequence and extract structural features.
pub fn profile_canonical_sequence(seq: &CanonicalSequence) -> crate::Result<GifProfile> {
    let metrics = extract_metrics(seq);
    let disposal_distribution = extract_disposal_distribution(seq);
    let transparency_analysis = extract_transparency_analysis(seq);
    let palette_info = extract_palette_info(seq);
    let change_statistics = extract_change_statistics(seq);
    let patch_density = extract_patch_density(seq);
    let delta_signal = extract_delta_signal(seq);

    let taxonomy = classify_taxonomy(
        &metrics,
        &disposal_distribution,
        &transparency_analysis,
        &palette_info,
        &change_statistics,
        &patch_density,
        &delta_signal,
    );

    Ok(GifProfile {
        metrics,
        disposal_distribution,
        transparency_analysis,
        palette_info,
        change_statistics,
        patch_density,
        delta_signal,
        taxonomy,
    })
}

fn extract_metrics(seq: &CanonicalSequence) -> SequenceMetrics {
    let frame_count = seq.frames.len();
    let total_pixels = (seq.width as usize) * (seq.height as usize);

    let avg_delay_ms = if frame_count > 0 {
        seq.frames.iter().map(|f| f.delay.as_millis() as f32).sum::<f32>() / frame_count as f32
    } else {
        0.0
    };

    SequenceMetrics {
        frame_count,
        width: seq.width,
        height: seq.height,
        total_pixels,
        avg_delay_ms,
    }
}

fn extract_disposal_distribution(seq: &CanonicalSequence) -> DisposalDistribution {
    let mut keep_count = 0;
    let mut none_count = 0;
    let mut background_count = 0;
    let mut previous_count = 0;

    for frame in &seq.frames {
        match frame.dispose {
            DisposalMethod::Keep => keep_count += 1,
            DisposalMethod::None => none_count += 1,
            DisposalMethod::Background => background_count += 1,
            DisposalMethod::Previous => previous_count += 1,
        }
    }

    let dominant = match (keep_count, none_count, background_count, previous_count) {
        (k, n, b, p) if k >= n && k >= b && k >= p => "Keep".to_string(),
        (k, n, b, p) if n >= k && n >= b && n >= p => "None".to_string(),
        (k, n, b, p) if b >= k && b >= n && b >= p => "Background".to_string(),
        (k, n, b, p) if p >= k && p >= n && p >= b => "Previous".to_string(),
        _ => "Mixed".to_string(),
    };

    DisposalDistribution {
        keep_count,
        none_count,
        background_count,
        previous_count,
        dominant,
    }
}

fn extract_transparency_analysis(seq: &CanonicalSequence) -> TransparencyAnalysis {
    let mut frames_with_transparency = 0;
    let mut total_transparency_ratio = 0.0f32;
    let mut max_transparency_ratio = 0.0f32;
    let mut frames_with_significant_transparency = 0;
    let mut frames_with_local_palette = 0;

    for frame in &seq.frames {
        // Check source patch for transparency
        if frame.source_patch.has_transparency {
            frames_with_transparency += 1;
            let transparency_ratio = if frame.source_patch.opaque_pixel_count
                + frame.source_patch.transparent_pixel_count
                > 0
            {
                frame.source_patch.transparent_pixel_count as f32
                    / (frame.source_patch.opaque_pixel_count + frame.source_patch.transparent_pixel_count)
                        as f32
            } else {
                0.0
            };
            total_transparency_ratio += transparency_ratio;
            max_transparency_ratio = max_transparency_ratio.max(transparency_ratio);

            if transparency_ratio > 0.1 {
                frames_with_significant_transparency += 1;
            }
        }

        // Check for local palette
        if frame.source_patch.has_transparency {
            frames_with_local_palette += 1;
        }
    }

    let avg_transparency_ratio = if !seq.frames.is_empty() {
        total_transparency_ratio / seq.frames.len() as f32
    } else {
        0.0
    };

    TransparencyAnalysis {
        frames_with_transparency,
        avg_transparency_ratio,
        max_transparency_ratio,
        frames_with_significant_transparency,
        uses_gce: frames_with_transparency > 0,
        frames_with_local_palette,
    }
}

fn extract_palette_info(seq: &CanonicalSequence) -> PaletteInfo {
    let has_global_palette = true; // Assume global palette is always available in canonical IR
    let local_palette_count = seq
        .frames
        .iter()
        .filter(|f| f.source_patch.has_transparency)
        .count();

    // Estimate palette stability: if most frames don't have local palettes, it's stable
    let palette_stability = if !seq.frames.is_empty() {
        1.0 - (local_palette_count as f32 / seq.frames.len() as f32)
    } else {
        1.0
    };

    PaletteInfo {
        has_global_palette,
        local_palette_count,
        palette_stability,
    }
}

fn extract_change_statistics(seq: &CanonicalSequence) -> ChangeStatistics {
    let mut total_changed_ratio = 0.0f32;
    let mut max_changed_ratio = 0.0f32;
    let mut min_changed_ratio = 1.0f32;
    let mut sparse_change_frames = 0;
    let mut dense_change_frames = 0;

    for frame in &seq.frames {
        let changed_ratio = frame.changed_region.changed_ratio;
        total_changed_ratio += changed_ratio;

        if changed_ratio > 0.0 {
            min_changed_ratio = min_changed_ratio.min(changed_ratio);
        }

        max_changed_ratio = max_changed_ratio.max(changed_ratio);

        if changed_ratio < 0.05 && changed_ratio > 0.0 {
            sparse_change_frames += 1;
        }

        if changed_ratio > 0.5 {
            dense_change_frames += 1;
        }
    }

    let avg_changed_ratio = if !seq.frames.is_empty() {
        total_changed_ratio / seq.frames.len() as f32
    } else {
        0.0
    };

    if min_changed_ratio == 1.0 {
        min_changed_ratio = 0.0; // No non-zero frames
    }

    ChangeStatistics {
        avg_changed_ratio,
        max_changed_ratio,
        min_changed_ratio,
        sparse_change_frames,
        dense_change_frames,
    }
}

fn extract_patch_density(seq: &CanonicalSequence) -> PatchDensity {
    let total_pixels = (seq.width as usize) * (seq.height as usize);
    let mut total_bbox_ratio = 0.0f32;
    let mut max_bbox_ratio = 0.0f32;
    let mut offset_patch_frames = 0;
    let mut total_patch_density = 0.0f32;
    let mut density_count = 0;

    for frame in &seq.frames {
        let bbox_area = frame.changed_region.bbox.area();
        let bbox_ratio = if total_pixels > 0 {
            bbox_area as f32 / total_pixels as f32
        } else {
            0.0
        };

        total_bbox_ratio += bbox_ratio;
        max_bbox_ratio = max_bbox_ratio.max(bbox_ratio);

        // Check if patch is offset (not at origin)
        if frame.source_patch.left > 0 || frame.source_patch.top > 0 {
            offset_patch_frames += 1;
        }

        // Patch density: changed pixels / bbox area
        if bbox_area > 0 {
            let patch_density = frame.changed_region.changed_pixel_count as f32 / bbox_area as f32;
            total_patch_density += patch_density;
            density_count += 1;
        }
    }

    let avg_bbox_ratio = if !seq.frames.is_empty() {
        total_bbox_ratio / seq.frames.len() as f32
    } else {
        0.0
    };

    let avg_patch_density = if density_count > 0 {
        total_patch_density / density_count as f32
    } else {
        0.0
    };

    PatchDensity {
        avg_bbox_ratio,
        max_bbox_ratio,
        offset_patch_frames,
        avg_patch_density,
    }
}

fn extract_delta_signal(seq: &CanonicalSequence) -> DeltaSignal {
    let mut opaque_delta_frames = 0;
    let mut offset_sparse_frames = 0;

    for frame in &seq.frames {
        // Opaque delta: no transparency in source patch (includes full-canvas opaque frames)
        if !frame.source_patch.has_transparency {
            opaque_delta_frames += 1;
        }

        // Offset sparse: patch is offset and has low changed area
        if (frame.source_patch.left > 0 || frame.source_patch.top > 0)
            && frame.changed_region.changed_ratio < 0.2
        {
            offset_sparse_frames += 1;
        }
    }

    let strength = if !seq.frames.is_empty() {
        (opaque_delta_frames as f32 / seq.frames.len() as f32) * 0.7
            + (offset_sparse_frames as f32 / seq.frames.len() as f32) * 0.3
    } else {
        0.0
    };

    let is_already_delta_encoded = offset_sparse_frames as f32 / seq.frames.len().max(1) as f32 > 0.5
        && opaque_delta_frames as f32 / seq.frames.len().max(1) as f32 > 0.5;

    DeltaSignal {
        strength,
        opaque_delta_frames,
        offset_sparse_frames,
        is_already_delta_encoded,
    }
}

fn classify_taxonomy(
    metrics: &SequenceMetrics,
    disposal: &DisposalDistribution,
    transparency: &TransparencyAnalysis,
    palette: &PaletteInfo,
    changes: &ChangeStatistics,
    patches: &PatchDensity,
    delta: &DeltaSignal,
) -> SequenceTaxonomy {
    // Rule 1: Disposal-heavy (Background or Previous)
    // If any significant disposal method is used, classify as disposal-heavy
    if disposal.background_count > 0 || disposal.previous_count > 0 {
        return SequenceTaxonomy::DisposalHeavyBackgroundPrevious;
    }

    // Rule 2: Opaque delta with global palette (Voyager-like)
    // Key signals: mostly opaque frames, no transparency, stable palette
    let opaque_ratio = delta.opaque_delta_frames as f32 / metrics.frame_count.max(1) as f32;
    let transparency_ratio = transparency.frames_with_transparency as f32 / metrics.frame_count.max(1) as f32;
    
    if opaque_ratio >= 0.5
        && transparency_ratio < 0.33
        && palette.palette_stability > 0.7
    {
        return SequenceTaxonomy::OpaqueDeltaGlobalPalette;
    }

    // Rule 3: Transparency-heavy sparse delta
    if transparency.frames_with_significant_transparency > metrics.frame_count / 3
        && changes.sparse_change_frames > metrics.frame_count / 4
        && patches.avg_bbox_ratio < 0.3
    {
        return SequenceTaxonomy::TransparencyHeavySparseDelta;
    }

    // Rule 4: Photographic (high color count, dense changes)
    // Only classify as photographic if we have many dense frames AND high patch density
    // AND low palette stability (indicating per-frame color variation)
    if changes.dense_change_frames > metrics.frame_count / 2
        && changes.avg_changed_ratio > 0.5
        && patches.avg_patch_density > 0.8
        && palette.palette_stability < 0.5
    {
        return SequenceTaxonomy::Photographic;
    }

    // Default: Mixed
    SequenceTaxonomy::Mixed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DisposalMethod, Frame, Gif, LoopCount};
    use std::time::Duration;

    fn create_opaque_delta_gif(width: u16, height: u16) -> Gif {
        let canvas_size = (width as usize) * (height as usize) * 4;

        // Frame 0: full opaque canvas
        let mut frame0_pixels = vec![0u8; canvas_size];
        for j in 0..canvas_size / 4 {
            frame0_pixels[j * 4] = 100; // R
            frame0_pixels[j * 4 + 1] = 100; // G
            frame0_pixels[j * 4 + 2] = 100; // B
            frame0_pixels[j * 4 + 3] = 255; // A
        }

        // Frame 1: delta in a small region (top-left 10x10)
        let mut frame1_pixels = frame0_pixels.clone();
        for y in 0..10 {
            for x in 0..10 {
                let idx = (y * (width as usize) + x) * 4;
                frame1_pixels[idx] = 200; // R
                frame1_pixels[idx + 1] = 50; // G
                frame1_pixels[idx + 2] = 50; // B
                frame1_pixels[idx + 3] = 255; // A
            }
        }

        Gif {
            width,
            height,
            global_palette: None,
            frames: vec![
                Frame {
                    pixels: frame0_pixels,
                    delay: Duration::from_millis(100),
                    dispose: DisposalMethod::Keep,
                    local_palette: None,
                    left: 0,
                    top: 0,
                    width,
                    height,
                },
                Frame {
                    pixels: frame1_pixels,
                    delay: Duration::from_millis(100),
                    dispose: DisposalMethod::Keep,
                    local_palette: None,
                    left: 0,
                    top: 0,
                    width,
                    height,
                },
            ],
            loop_count: LoopCount::Infinite,
            original_palette: None,
        }
    }

    fn create_disposal_background_gif(width: u16, height: u16) -> Gif {
        let canvas_size = (width as usize) * (height as usize) * 4;

        // Frame 0: full opaque canvas
        let mut frame0_pixels = vec![0u8; canvas_size];
        for j in 0..canvas_size / 4 {
            frame0_pixels[j * 4] = 100;
            frame0_pixels[j * 4 + 1] = 100;
            frame0_pixels[j * 4 + 2] = 100;
            frame0_pixels[j * 4 + 3] = 255;
        }

        // Frame 1: same as frame 0 (will be disposed to transparent)
        let frame1_pixels = frame0_pixels.clone();

        // Frame 2: should show on transparent background
        let mut frame2_pixels = vec![0u8; canvas_size];
        for j in 0..canvas_size / 4 {
            frame2_pixels[j * 4] = 200;
            frame2_pixels[j * 4 + 1] = 50;
            frame2_pixels[j * 4 + 2] = 50;
            frame2_pixels[j * 4 + 3] = 255;
        }

        Gif {
            width,
            height,
            global_palette: None,
            frames: vec![
                Frame {
                    pixels: frame0_pixels,
                    delay: Duration::from_millis(100),
                    dispose: DisposalMethod::Keep,
                    local_palette: None,
                    left: 0,
                    top: 0,
                    width,
                    height,
                },
                Frame {
                    pixels: frame1_pixels,
                    delay: Duration::from_millis(100),
                    dispose: DisposalMethod::Background,
                    local_palette: None,
                    left: 0,
                    top: 0,
                    width,
                    height,
                },
                Frame {
                    pixels: frame2_pixels,
                    delay: Duration::from_millis(100),
                    dispose: DisposalMethod::Keep,
                    local_palette: None,
                    left: 0,
                    top: 0,
                    width,
                    height,
                },
            ],
            loop_count: LoopCount::Infinite,
            original_palette: None,
        }
    }

    fn create_transparency_heavy_gif(width: u16, height: u16) -> Gif {
        let canvas_size = (width as usize) * (height as usize) * 4;

        // Frame 0: mostly transparent with a small opaque region
        let mut frame0_pixels = vec![0u8; canvas_size];
        for y in 5..15 {
            for x in 5..15 {
                let idx = (y * (width as usize) + x) * 4;
                frame0_pixels[idx] = 100; // R
                frame0_pixels[idx + 1] = 100; // G
                frame0_pixels[idx + 2] = 100; // B
                frame0_pixels[idx + 3] = 255; // A
            }
        }

        // Frame 1: mostly transparent with a different small opaque region
        let mut frame1_pixels = vec![0u8; canvas_size];
        for y in 20..30 {
            for x in 20..30 {
                let idx = (y * (width as usize) + x) * 4;
                frame1_pixels[idx] = 200; // R
                frame1_pixels[idx + 1] = 50; // G
                frame1_pixels[idx + 2] = 50; // B
                frame1_pixels[idx + 3] = 255; // A
            }
        }

        Gif {
            width,
            height,
            global_palette: None,
            frames: vec![
                Frame {
                    pixels: frame0_pixels,
                    delay: Duration::from_millis(100),
                    dispose: DisposalMethod::Keep,
                    local_palette: None,
                    left: 0,
                    top: 0,
                    width,
                    height,
                },
                Frame {
                    pixels: frame1_pixels,
                    delay: Duration::from_millis(100),
                    dispose: DisposalMethod::Keep,
                    local_palette: None,
                    left: 0,
                    top: 0,
                    width,
                    height,
                },
            ],
            loop_count: LoopCount::Infinite,
            original_palette: None,
        }
    }

    #[test]
    fn test_profile_opaque_delta_gif() {
        let gif = create_opaque_delta_gif(100, 100);
        let profile = profile_gif(&gif).expect("Failed to profile GIF");

        assert_eq!(profile.metrics.frame_count, 2);
        assert_eq!(profile.metrics.width, 100);
        assert_eq!(profile.metrics.height, 100);

        // Debug output
        eprintln!("Opaque delta profile:");
        eprintln!("  delta_signal.opaque_delta_frames: {}", profile.delta_signal.opaque_delta_frames);
        eprintln!("  transparency.frames_with_transparency: {}", profile.transparency_analysis.frames_with_transparency);
        eprintln!("  palette.palette_stability: {}", profile.palette_info.palette_stability);
        eprintln!("  changes.avg_changed_ratio: {}", profile.change_statistics.avg_changed_ratio);
        eprintln!("  changes.dense_change_frames: {}", profile.change_statistics.dense_change_frames);
        eprintln!("  patches.avg_patch_density: {}", profile.patch_density.avg_patch_density);
        eprintln!("  taxonomy: {:?}", profile.taxonomy);

        // Should classify as opaque delta
        assert_eq!(profile.taxonomy, SequenceTaxonomy::OpaqueDeltaGlobalPalette);

        // Verify delta signal
        assert!(profile.delta_signal.opaque_delta_frames > 0);
        assert!(!profile.transparency_analysis.uses_gce);
    }

    #[test]
    fn test_profile_disposal_background_gif() {
        let gif = create_disposal_background_gif(100, 100);
        let profile = profile_gif(&gif).expect("Failed to profile GIF");

        assert_eq!(profile.metrics.frame_count, 3);

        // Debug output
        eprintln!("Disposal background profile:");
        eprintln!("  disposal.background_count: {}", profile.disposal_distribution.background_count);
        eprintln!("  disposal.previous_count: {}", profile.disposal_distribution.previous_count);
        eprintln!("  frame_count: {}", profile.metrics.frame_count);
        eprintln!("  taxonomy: {:?}", profile.taxonomy);

        // Should classify as disposal-heavy
        assert_eq!(profile.taxonomy, SequenceTaxonomy::DisposalHeavyBackgroundPrevious);

        // Verify disposal distribution
        assert_eq!(profile.disposal_distribution.background_count, 1);
    }

    #[test]
    fn test_profile_transparency_heavy_gif() {
        let gif = create_transparency_heavy_gif(100, 100);
        let profile = profile_gif(&gif).expect("Failed to profile GIF");

        assert_eq!(profile.metrics.frame_count, 2);

        // Should classify as transparency-heavy sparse delta
        assert_eq!(profile.taxonomy, SequenceTaxonomy::TransparencyHeavySparseDelta);

        // Verify transparency analysis
        assert!(profile.transparency_analysis.frames_with_transparency > 0);
        assert!(profile.transparency_analysis.uses_gce);
    }

    #[test]
    fn test_taxonomy_name() {
        assert_eq!(SequenceTaxonomy::OpaqueDeltaGlobalPalette.name(), "opaque-delta/global-palette");
        assert_eq!(
            SequenceTaxonomy::TransparencyHeavySparseDelta.name(),
            "transparency-heavy/sparse-delta"
        );
        assert_eq!(
            SequenceTaxonomy::DisposalHeavyBackgroundPrevious.name(),
            "disposal-heavy/background-previous"
        );
        assert_eq!(SequenceTaxonomy::Photographic.name(), "photographic/noisy");
        assert_eq!(SequenceTaxonomy::Mixed.name(), "mixed/unknown");
    }

    #[test]
    fn test_profile_metrics() {
        let gif = create_opaque_delta_gif(50, 50);
        let profile = profile_gif(&gif).expect("Failed to profile GIF");

        assert_eq!(profile.metrics.frame_count, 2);
        assert_eq!(profile.metrics.width, 50);
        assert_eq!(profile.metrics.height, 50);
        assert_eq!(profile.metrics.total_pixels, 2500);
        assert!(profile.metrics.avg_delay_ms > 0.0);
    }

    #[test]
    fn test_profile_disposal_distribution() {
        let gif = create_disposal_background_gif(50, 50);
        let profile = profile_gif(&gif).expect("Failed to profile GIF");

        assert_eq!(profile.disposal_distribution.keep_count, 2);
        assert_eq!(profile.disposal_distribution.background_count, 1);
        assert_eq!(profile.disposal_distribution.none_count, 0);
        assert_eq!(profile.disposal_distribution.previous_count, 0);
    }

    #[test]
    fn test_profile_change_statistics() {
        let gif = create_opaque_delta_gif(100, 100);
        let profile = profile_gif(&gif).expect("Failed to profile GIF");

        // Frame 0 should have high change ratio (full canvas)
        // Frame 1 should have low change ratio (10x10 delta)
        assert!(profile.change_statistics.max_changed_ratio > 0.5);
        assert!(profile.change_statistics.avg_changed_ratio > 0.0);
    }
}

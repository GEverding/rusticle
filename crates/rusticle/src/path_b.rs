//! Path B: General sparse/transparent optimization.
//!
//! Path B is the general-purpose optimization path for GIFs that are transparency-heavy,
//! disposal-heavy, mixed, or non-obviously opaque-delta. It preserves the corrected
//! RGB-canvas-based machinery with:
//!
//! - **Canonical display-space truth model**: Frames are compared in their displayed
//!   canvas state, accounting for disposal semantics (None/Keep/Background/Previous).
//! - **Disposal-aware handling**: Disposal methods are preserved and respected during
//!   optimization. Frames with Background/Previous disposal are never collapsed.
//! - **Structural optimization (lossless)**: Marks unchanged pixels as transparent.
//!   No perceptual thresholding here—that's in `lossy()`.
//! - **Sparse/transparent behavior allowed**: Subframes and transparency are allowed
//!   where structurally appropriate.
//!
//! # Differences from Path A
//!
//! Path A is conservative: it only emits opaque patches for sequences that look like
//! already-optimized opaque-delta GIFs. Path B is general: it handles any sequence,
//! including those with transparency, mixed disposal, and sparse patches.
//!
//! - **Path A**: Opaque bbox patches only, no synthetic transparency, Keep/None disposal only
//! - **Path B**: Allows transparency, sparse patches, all disposal methods, structural optimization

use crate::simd_opt::{find_diff_bounding_box, mark_unchanged_pixels_simd, DiffRect};
use crate::types::{DisposalMethod, Frame, OptLevel};
use rayon::prelude::*;

/// Configuration for Path B optimization.
#[derive(Debug, Clone, Copy)]
pub struct PathBConfig {
    /// Optimization level controlling cropping behavior.
    pub level: OptLevel,
}

impl Default for PathBConfig {
    fn default() -> Self {
        Self {
            level: OptLevel::O3,
        }
    }
}

/// Optimize frames using Path B (general sparse/transparent optimization).
///
/// This is the main entry point for Path B. It applies structural optimization
/// (marking unchanged pixels transparent) while respecting disposal semantics.
///
/// # Arguments
///
/// * `frames` - The frames to optimize
/// * `config` - Path B configuration (optimization level)
///
/// # Returns
///
/// Optimized frames with unchanged pixels marked transparent
pub fn optimize_path_b(frames: &[Frame], config: PathBConfig) -> Vec<Frame> {
    if frames.is_empty() {
        return vec![];
    }

    // All optimization levels use exact pixel match (threshold=0, lossless)
    let threshold = 0;

    // Only crop to bounding box for O3 (most aggressive)
    let crop_to_bbox = config.level == OptLevel::O3;

    optimize_frames_internal(frames, threshold, crop_to_bbox)
}

/// Optimize frames using Path B with lossy compression.
///
/// Applies perceptual thresholding to mark "close enough" pixels as transparent.
/// This is a lossy operation that can significantly reduce file size.
///
/// # Arguments
///
/// * `frames` - The frames to optimize
/// * `quality` - Quality level 0-100 (100 = lossless, 0 = maximum loss)
/// * `canvas_width` - Canvas width for subframe handling
/// * `canvas_height` - Canvas height for subframe handling
///
/// # Returns
///
/// Lossy-compressed frames
pub fn optimize_path_b_lossy(
    frames: &[Frame],
    quality: u8,
    canvas_width: u16,
    canvas_height: u16,
) -> Vec<Frame> {
    if frames.is_empty() {
        return vec![];
    }

    // Clamp quality to 0-100 range
    let quality = quality.min(100);

    // Calculate threshold: quality 100 = 0 (lossless), quality 0 = 20 (conservative max)
    let threshold = ((100 - quality as u16) * 20 / 100) as u8;

    // Always crop to bounding box for lossy (maximize compression)
    let crop_to_bbox = true;

    optimize_frames_internal_with_canvas(
        frames,
        threshold,
        crop_to_bbox,
        canvas_width,
        canvas_height,
    )
}

/// Internal optimization with configurable threshold and cropping.
/// Shared by both structural and lossy optimization.
fn optimize_frames_internal(frames: &[Frame], threshold: u8, crop_to_bbox: bool) -> Vec<Frame> {
    // Infer canvas dimensions from the first frame (or use a default).
    // For most GIFs, the first frame is full-canvas, so this is reliable.
    let (canvas_width, canvas_height) = if !frames.is_empty() {
        (frames[0].width, frames[0].height)
    } else {
        (0, 0)
    };

    // Precompute reference canvases by replaying disposal semantics.
    // reference_canvases[i] = canvas state immediately before frame[i] is drawn.
    let reference_canvases = precompute_reference_canvases(frames, canvas_width, canvas_height);

    frames
        .par_iter()
        .enumerate()
        .map(|(idx, frame)| {
            if idx == 0 {
                // First frame is never optimized
                frame.clone()
            } else {
                let reference_frame = &reference_canvases[idx];
                optimize_frame_internal(frame, reference_frame, threshold, crop_to_bbox)
            }
        })
        .collect()
}

/// Internal optimization with canvas dimensions for handling subframes.
/// Used by lossy optimization to correctly handle frames that may be cropped/subframed.
fn optimize_frames_internal_with_canvas(
    frames: &[Frame],
    threshold: u8,
    crop_to_bbox: bool,
    canvas_width: u16,
    canvas_height: u16,
) -> Vec<Frame> {
    // Precompute reference canvases by replaying disposal semantics.
    // reference_canvases[i] = canvas state immediately before frame[i] is drawn.
    let reference_canvases = precompute_reference_canvases(frames, canvas_width, canvas_height);

    frames
        .par_iter()
        .enumerate()
        .map(|(idx, frame)| {
            if idx == 0 {
                // First frame is never optimized
                frame.clone()
            } else {
                let reference_frame = &reference_canvases[idx];
                optimize_frame_internal_with_canvas(
                    frame,
                    reference_frame,
                    threshold,
                    crop_to_bbox,
                    canvas_width,
                    canvas_height,
                )
            }
        })
        .collect()
}

/// Precompute reference canvases by replaying disposal semantics.
///
/// For each frame N, computes the canvas state immediately before frame N is drawn.
/// This accounts for disposal methods:
/// - `None`/`Keep`: canvas stays as displayed
/// - `Background`: canvas is cleared to transparent
/// - `Previous`: canvas is restored to the state before the current frame was drawn
///
/// For subframe patches (Keep/None disposal with non-zero left/top or smaller dimensions),
/// the displayed canvas is the full canvas with the patch composited onto it, NOT the raw patch.
///
/// Returns a vector where `reference_canvases[i]` is the canvas state before frame[i].
/// `reference_canvases[0]` is always a transparent canvas (first frame drawn on empty).
fn precompute_reference_canvases(
    frames: &[Frame],
    canvas_width: u16,
    canvas_height: u16,
) -> Vec<Frame> {
    if frames.is_empty() {
        return vec![];
    }

    let mut references = vec![];
    let mut displayed_canvas: Option<Frame> = None;

    for (idx, frame) in frames.iter().enumerate() {
        // Save the canvas state before drawing the current frame
        // (needed for Previous disposal to restore to)
        let canvas_before_draw = displayed_canvas.clone();

        // The reference for frame[idx] is the current canvas state
        if idx == 0 {
            // First frame is drawn on a transparent canvas
            references.push(create_transparent_canvas(canvas_width, canvas_height));
        } else {
            // For subsequent frames, use the current canvas state
            references.push(
                displayed_canvas
                    .clone()
                    .unwrap_or_else(|| create_transparent_canvas(canvas_width, canvas_height)),
            );
        }

        // After this frame is displayed, apply its disposal to update the canvas state
        // for the next frame.
        match frame.dispose {
            DisposalMethod::None | DisposalMethod::Keep => {
                // Canvas stays as displayed.
                // CRITICAL: If the frame is a subframe patch (non-zero left/top or smaller than canvas),
                // we must composite it onto the full canvas, not store the raw patch.
                // This ensures the next frame's reference is the correct full-canvas state.
                let is_subframe = frame.left != 0
                    || frame.top != 0
                    || frame.width != canvas_width
                    || frame.height != canvas_height;

                if is_subframe {
                    // Composite the subframe patch onto the current canvas
                    let base_canvas = displayed_canvas
                        .clone()
                        .unwrap_or_else(|| create_transparent_canvas(canvas_width, canvas_height));
                    displayed_canvas = Some(composite_frame_onto_canvas(
                        &base_canvas,
                        frame,
                        canvas_width,
                        canvas_height,
                    ));
                } else {
                    // Full-canvas frame: store as-is
                    displayed_canvas = Some(frame.clone());
                }
            }
            DisposalMethod::Background => {
                // Canvas is cleared to transparent
                displayed_canvas = Some(create_transparent_canvas(canvas_width, canvas_height));
            }
            DisposalMethod::Previous => {
                // Canvas is restored to the state before the current frame was drawn.
                let restored = canvas_before_draw
                    .clone()
                    .unwrap_or_else(|| create_transparent_canvas(canvas_width, canvas_height));
                displayed_canvas = Some(restored);
            }
        }
    }

    references
}

/// Create a transparent canvas frame with the given dimensions.
fn create_transparent_canvas(width: u16, height: u16) -> Frame {
    let size = (width as usize) * (height as usize) * 4;
    Frame {
        pixels: vec![0u8; size],
        delay: std::time::Duration::from_millis(0),
        dispose: DisposalMethod::Keep,
        local_palette: None,
        left: 0,
        top: 0,
        width,
        height,
    }
}

/// Composite a subframe patch onto a full canvas.
///
/// Takes a base canvas (full-sized) and a subframe patch (with left/top offset),
/// and returns a new full-sized canvas with the patch alpha-composited at its position.
///
/// For pixels with alpha > 0, we overwrite the base canvas pixels.
/// For pixels with alpha = 0 (fully transparent), we preserve the base canvas.
/// For pixels with 0 < alpha < 255, we perform proper alpha blending.
fn composite_frame_onto_canvas(
    base_canvas: &Frame,
    patch: &Frame,
    canvas_width: u16,
    canvas_height: u16,
) -> Frame {
    let mut result = base_canvas.clone();
    let canvas_w = canvas_width as usize;
    let canvas_h = canvas_height as usize;
    let patch_w = patch.width as usize;
    let patch_h = patch.height as usize;
    let patch_left = patch.left as usize;
    let patch_top = patch.top as usize;

    // Composite the patch onto the result canvas
    for py in 0..patch_h {
        for px in 0..patch_w {
            // Canvas coordinates
            let cx = patch_left + px;
            let cy = patch_top + py;

            // Bounds check
            if cx >= canvas_w || cy >= canvas_h {
                continue;
            }

            // Indices
            let patch_idx = (py * patch_w + px) * 4;
            let canvas_idx = (cy * canvas_w + cx) * 4;

            // Ensure indices are in bounds
            if patch_idx + 3 >= patch.pixels.len() || canvas_idx + 3 >= result.pixels.len() {
                continue;
            }

            let patch_alpha = patch.pixels[patch_idx + 3];

            if patch_alpha == 0 {
                // Fully transparent: keep base canvas pixel
                continue;
            } else if patch_alpha == 255 {
                // Fully opaque: overwrite with patch pixel
                result.pixels[canvas_idx..canvas_idx + 4]
                    .copy_from_slice(&patch.pixels[patch_idx..patch_idx + 4]);
            } else {
                // Partial alpha: blend
                let patch_r = patch.pixels[patch_idx] as u16;
                let patch_g = patch.pixels[patch_idx + 1] as u16;
                let patch_b = patch.pixels[patch_idx + 2] as u16;
                let patch_a = patch_alpha as u16;

                let base_r = result.pixels[canvas_idx] as u16;
                let base_g = result.pixels[canvas_idx + 1] as u16;
                let base_b = result.pixels[canvas_idx + 2] as u16;
                let base_a = result.pixels[canvas_idx + 3] as u16;

                // Alpha blend: out = patch + base * (1 - patch_alpha)
                let inv_patch_a = 255 - patch_a;
                let out_r = ((patch_r * patch_a + base_r * inv_patch_a) / 255) as u8;
                let out_g = ((patch_g * patch_a + base_g * inv_patch_a) / 255) as u8;
                let out_b = ((patch_b * patch_a + base_b * inv_patch_a) / 255) as u8;
                let out_a = ((patch_a * 255 + base_a * inv_patch_a) / 255) as u8;

                result.pixels[canvas_idx] = out_r;
                result.pixels[canvas_idx + 1] = out_g;
                result.pixels[canvas_idx + 2] = out_b;
                result.pixels[canvas_idx + 3] = out_a;
            }
        }
    }

    result
}

/// Optimize a single frame by comparing to the previous frame.
/// Uses SIMD-accelerated pixel comparison and diff-based bounding box.
///
/// # Disposal Method Semantics
///
/// The disposal method is preserved from the source frame. Optimization only marks
/// unchanged pixels as transparent; it does not change the disposal semantics:
/// - `DisposalMethod::Background`: Still means "clear this frame region before next"
/// - `DisposalMethod::Previous`: Still means "restore to previous canvas state"
/// - `DisposalMethod::Keep`: Still means "keep this frame on canvas"
/// - `DisposalMethod::None`: Still means "no disposal specified"
///
/// When frames are identical (no diff), we return a minimal 1x1 transparent frame
/// with `DisposalMethod::Keep` because the frame contributes nothing visually.
fn optimize_frame_internal(
    frame: &Frame,
    prev_frame: &Frame,
    threshold: u8,
    crop_to_bbox: bool,
) -> Frame {
    // Only optimize if frames represent the same region.
    // For cropped/sub-frame pipelines, same dimensions are not enough:
    // left/top must also match, otherwise pixel-wise comparison is spatially invalid.
    if frame.width != prev_frame.width
        || frame.height != prev_frame.height
        || frame.left != prev_frame.left
        || frame.top != prev_frame.top
    {
        // Misaligned frames: return unchanged (preserve original disposal)
        return frame.clone();
    }

    let width = frame.width as usize;
    let height = frame.height as usize;

    // Find diff bounding box FIRST
    let diff_rect =
        match find_diff_bounding_box(&prev_frame.pixels, &frame.pixels, width, height, threshold) {
            Some(rect) => rect,
            None => {
                // Frames are identical - check if we can safely collapse to minimal 1x1 transparent.
                //
                // CRITICAL: Collapsing is only safe if the disposal method does NOT change
                // the canvas state in a way that affects the next frame.
                //
                // Safe to collapse:
                //   - DisposalMethod::Keep: canvas stays as displayed (no change)
                //   - DisposalMethod::None: no disposal specified (canvas stays as displayed)
                //
                // UNSAFE to collapse:
                //   - DisposalMethod::Background: canvas is cleared after this frame.
                //     The next frame will be drawn on a transparent canvas, which is a real change.
                //   - DisposalMethod::Previous: canvas is restored to the pre-frame state.
                //     The next frame will be drawn on a different canvas, which is a real change.
                //
                // For unsafe disposal methods, we must keep the frame as-is to preserve
                // the visual semantics of the disposal operation.
                match frame.dispose {
                    DisposalMethod::Keep | DisposalMethod::None => {
                        // Safe to collapse: canvas state is unchanged after disposal
                        return Frame {
                            pixels: vec![0, 0, 0, 0],
                            delay: frame.delay,
                            dispose: frame.dispose,
                            local_palette: None,
                            left: 0,
                            top: 0,
                            width: 1,
                            height: 1,
                        };
                    }
                    DisposalMethod::Background | DisposalMethod::Previous => {
                        // UNSAFE to collapse: disposal changes canvas state for next frame.
                        // Return the frame unchanged to preserve disposal semantics.
                        return frame.clone();
                    }
                }
            }
        };

    // If crop_to_bbox: extract and process only the diff region
    // Otherwise: process full frame but mark unchanged pixels
    if crop_to_bbox {
        extract_and_optimize_subframe(frame, prev_frame, &diff_rect, threshold)
    } else {
        // Full frame processing: mark unchanged pixels transparent, preserve disposal
        let mut optimized = frame.clone();
        let min_len = optimized.pixels.len().min(prev_frame.pixels.len());
        if min_len >= 4 && min_len.is_multiple_of(4) {
            mark_unchanged_pixels_simd(
                &mut optimized.pixels[..min_len],
                &prev_frame.pixels[..min_len],
                threshold,
            );
        }
        // Preserve original disposal method
        optimized
    }
}

/// Optimize a single frame with canvas-aware subframe handling.
/// For subframes, compares only the overlapping region for correctness.
fn optimize_frame_internal_with_canvas(
    frame: &Frame,
    prev_frame: &Frame,
    threshold: u8,
    crop_to_bbox: bool,
    _canvas_width: u16,
    _canvas_height: u16,
) -> Frame {
    // Check if frames are subframes (not covering full canvas)
    let frame_is_subframe = frame.left != 0 || frame.top != 0;
    let prev_is_subframe = prev_frame.left != 0 || prev_frame.top != 0;

    // Also check if frames have different dimensions (one is a subframe of the other)
    let different_dimensions = frame.width != prev_frame.width || frame.height != prev_frame.height;

    // If either frame is a subframe or they have different dimensions, use overlapping region comparison
    if frame_is_subframe || prev_is_subframe || different_dimensions {
        return optimize_subframe_overlapping(frame, prev_frame, threshold, crop_to_bbox);
    }

    // Both frames are at (0, 0) with same dimensions: use standard optimization
    optimize_frame_internal(frame, prev_frame, threshold, crop_to_bbox)
}

/// Optimize subframes by comparing only the overlapping region.
/// For subframes that don't fully overlap, we only compare the overlapping area.
///
/// # Disposal Method Semantics
///
/// The disposal method is preserved from the source frame. Optimization only affects
/// pixel transparency, not the disposal semantics.
fn optimize_subframe_overlapping(
    frame: &Frame,
    prev_frame: &Frame,
    threshold: u8,
    crop_to_bbox: bool,
) -> Frame {
    // Find the overlapping region
    let overlap_left = frame.left.max(prev_frame.left);
    let overlap_top = frame.top.max(prev_frame.top);
    let overlap_right = (frame.left + frame.width).min(prev_frame.left + prev_frame.width);
    let overlap_bottom = (frame.top + frame.height).min(prev_frame.top + prev_frame.height);

    // If no overlap, frames are completely different - preserve original disposal
    if overlap_left >= overlap_right || overlap_top >= overlap_bottom {
        return frame.clone();
    }

    // Extract overlapping regions from both frames
    let overlap_width = (overlap_right - overlap_left) as usize;
    let overlap_height = (overlap_bottom - overlap_top) as usize;

    let mut frame_overlap = vec![0u8; overlap_width * overlap_height * 4];
    let mut prev_overlap = vec![0u8; overlap_width * overlap_height * 4];

    // Copy overlapping pixels from frame
    for y in 0..overlap_height {
        for x in 0..overlap_width {
            let frame_x = (overlap_left - frame.left) as usize + x;
            let frame_y = (overlap_top - frame.top) as usize + y;
            let frame_idx = (frame_y * frame.width as usize + frame_x) * 4;
            let overlap_idx = (y * overlap_width + x) * 4;

            if frame_idx + 3 < frame.pixels.len() {
                frame_overlap[overlap_idx..overlap_idx + 4]
                    .copy_from_slice(&frame.pixels[frame_idx..frame_idx + 4]);
            }
        }
    }

    // Copy overlapping pixels from prev_frame
    for y in 0..overlap_height {
        for x in 0..overlap_width {
            let prev_x = (overlap_left - prev_frame.left) as usize + x;
            let prev_y = (overlap_top - prev_frame.top) as usize + y;
            let prev_idx = (prev_y * prev_frame.width as usize + prev_x) * 4;
            let overlap_idx = (y * overlap_width + x) * 4;

            if prev_idx + 3 < prev_frame.pixels.len() {
                prev_overlap[overlap_idx..overlap_idx + 4]
                    .copy_from_slice(&prev_frame.pixels[prev_idx..prev_idx + 4]);
            }
        }
    }

    // Find diff in overlapping region
    let diff_rect = match find_diff_bounding_box(
        &prev_overlap,
        &frame_overlap,
        overlap_width,
        overlap_height,
        threshold,
    ) {
        Some(rect) => rect,
        None => {
            // Overlapping regions are identical - check if we can safely collapse to minimal 1x1 transparent.
            //
            // CRITICAL: Collapsing is only safe if the disposal method does NOT change
            // the canvas state in a way that affects the next frame.
            //
            // Safe to collapse:
            //   - DisposalMethod::Keep: canvas stays as displayed (no change)
            //   - DisposalMethod::None: no disposal specified (canvas stays as displayed)
            //
            // UNSAFE to collapse:
            //   - DisposalMethod::Background: canvas is cleared after this frame.
            //     The next frame will be drawn on a transparent canvas, which is a real change.
            //   - DisposalMethod::Previous: canvas is restored to the pre-frame state.
            //     The next frame will be drawn on a different canvas, which is a real change.
            match frame.dispose {
                DisposalMethod::Keep | DisposalMethod::None => {
                    // Safe to collapse: canvas state is unchanged after disposal
                    return Frame {
                        pixels: vec![0, 0, 0, 0],
                        delay: frame.delay,
                        dispose: frame.dispose,
                        local_palette: None,
                        left: 0,
                        top: 0,
                        width: 1,
                        height: 1,
                    };
                }
                DisposalMethod::Background | DisposalMethod::Previous => {
                    // UNSAFE to collapse: disposal changes canvas state for next frame.
                    // Return the frame unchanged to preserve disposal semantics.
                    return frame.clone();
                }
            }
        }
    };

    if crop_to_bbox {
        // Extract diff region from overlapping area
        let new_width = diff_rect.width as usize;
        let new_height = diff_rect.height as usize;

        let mut new_pixels = vec![0u8; new_width * new_height * 4];
        let mut prev_pixels = vec![0u8; new_width * new_height * 4];

        for y in 0..new_height {
            for x in 0..new_width {
                let overlap_x = diff_rect.left as usize + x;
                let overlap_y = diff_rect.top as usize + y;
                let overlap_idx = (overlap_y * overlap_width + overlap_x) * 4;
                let dst_idx = (y * new_width + x) * 4;

                if overlap_idx + 3 < frame_overlap.len() {
                    new_pixels[dst_idx..dst_idx + 4]
                        .copy_from_slice(&frame_overlap[overlap_idx..overlap_idx + 4]);
                    prev_pixels[dst_idx..dst_idx + 4]
                        .copy_from_slice(&prev_overlap[overlap_idx..overlap_idx + 4]);
                }
            }
        }

        // Apply transparency marking
        let min_len = new_pixels.len();
        if min_len >= 4 && min_len.is_multiple_of(4) {
            mark_unchanged_pixels_simd(&mut new_pixels, &prev_pixels, threshold);
        }

        Frame {
            pixels: new_pixels,
            delay: frame.delay,
            dispose: frame.dispose, // Preserve original disposal method
            local_palette: None,
            left: overlap_left + diff_rect.left,
            top: overlap_top + diff_rect.top,
            width: diff_rect.width,
            height: diff_rect.height,
        }
    } else {
        // Mark unchanged pixels in overlapping region, preserve disposal
        let mut optimized = frame.clone();

        // Mark unchanged pixels in the overlapping region
        for y in 0..overlap_height {
            for x in 0..overlap_width {
                let frame_x = (overlap_left - frame.left) as usize + x;
                let frame_y = (overlap_top - frame.top) as usize + y;
                let frame_idx = (frame_y * frame.width as usize + frame_x) * 4;
                let overlap_idx = (y * overlap_width + x) * 4;

                if frame_idx + 3 < optimized.pixels.len()
                    && overlap_idx + 3 < frame_overlap.len()
                    && pixels_similar(
                        &[
                            frame_overlap[overlap_idx],
                            frame_overlap[overlap_idx + 1],
                            frame_overlap[overlap_idx + 2],
                            frame_overlap[overlap_idx + 3],
                        ],
                        &[
                            prev_overlap[overlap_idx],
                            prev_overlap[overlap_idx + 1],
                            prev_overlap[overlap_idx + 2],
                            prev_overlap[overlap_idx + 3],
                        ],
                        threshold,
                    )
                {
                    optimized.pixels[frame_idx + 3] = 0; // Mark as transparent
                }
            }
        }

        // Preserve original disposal method
        optimized
    }
}

/// Extract sub-frame at diff_rect and mark unchanged pixels transparent.
///
/// # Disposal Method Semantics
///
/// When extracting a subframe, we must preserve the original disposal method.
/// The disposal method describes what happens after the frame is displayed,
/// independent of whether the frame is cropped or has transparent pixels.
fn extract_and_optimize_subframe(
    frame: &Frame,
    prev_frame: &Frame,
    diff_rect: &DiffRect,
    threshold: u8,
) -> Frame {
    let src_width = frame.width as usize;
    let new_width = diff_rect.width as usize;
    let new_height = diff_rect.height as usize;

    // Extract pixels from both frames at the diff region
    let mut new_pixels = vec![0u8; new_width * new_height * 4];
    let mut prev_pixels = vec![0u8; new_width * new_height * 4];

    for y in 0..new_height {
        for x in 0..new_width {
            let src_x = diff_rect.left as usize + x;
            let src_y = diff_rect.top as usize + y;
            let src_idx = (src_y * src_width + src_x) * 4;
            let dst_idx = (y * new_width + x) * 4;

            new_pixels[dst_idx..dst_idx + 4].copy_from_slice(&frame.pixels[src_idx..src_idx + 4]);
            prev_pixels[dst_idx..dst_idx + 4]
                .copy_from_slice(&prev_frame.pixels[src_idx..src_idx + 4]);
        }
    }

    // Apply transparency marking within the extracted region
    let min_len = new_pixels.len();
    if min_len >= 4 && min_len.is_multiple_of(4) {
        mark_unchanged_pixels_simd(&mut new_pixels, &prev_pixels, threshold);
    }

    Frame {
        pixels: new_pixels,
        delay: frame.delay,
        dispose: frame.dispose, // Preserve original disposal method
        local_palette: None,
        left: diff_rect.left,
        top: diff_rect.top,
        width: diff_rect.width,
        height: diff_rect.height,
    }
}

/// Check if two pixels are similar within threshold (per-channel).
#[inline]
fn pixels_similar(a: &[u8; 4], b: &[u8; 4], threshold: u8) -> bool {
    a[0].abs_diff(b[0]) <= threshold
        && a[1].abs_diff(b[1]) <= threshold
        && a[2].abs_diff(b[2]) <= threshold
        && a[3].abs_diff(b[3]) <= threshold
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn make_frame(width: u16, height: u16, color: [u8; 4]) -> Frame {
        let mut pixels = Vec::new();
        for _ in 0..(width as usize * height as usize) {
            pixels.extend_from_slice(&color);
        }
        Frame {
            pixels,
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::None,
            local_palette: None,
            left: 0,
            top: 0,
            width,
            height,
        }
    }

    fn make_frame_with_disposal(
        width: u16,
        height: u16,
        color: [u8; 4],
        dispose: DisposalMethod,
    ) -> Frame {
        let mut frame = make_frame(width, height, color);
        frame.dispose = dispose;
        frame
    }

    #[test]
    fn test_path_b_transparency_heavy_sequence() {
        // Create a sequence with transparency-heavy frames
        let frame1 = make_frame(100, 100, [255, 0, 0, 255]); // Red, opaque
        let mut frame2 = make_frame(100, 100, [0, 255, 0, 255]); // Green, different from frame1
                                                                 // Make some pixels match frame1 (so they can be marked transparent)
        for i in 0..50 {
            frame2.pixels[i * 4] = 255; // Set red channel to match frame1
            frame2.pixels[i * 4 + 1] = 0; // Set green to 0
            frame2.pixels[i * 4 + 2] = 0; // Set blue to 0
            frame2.pixels[i * 4 + 3] = 255; // Keep opaque
        }

        let frames = vec![frame1, frame2];
        let config = PathBConfig::default();
        let optimized = optimize_path_b(&frames, config);

        // Should have 2 frames
        assert_eq!(optimized.len(), 2);
        // First frame should be unchanged
        assert_eq!(optimized[0].width, 100);
        // Second frame should be optimized (some pixels marked transparent where they match frame1)
        assert!(optimized[1]
            .pixels
            .iter()
            .step_by(4)
            .skip(3)
            .any(|&a| a == 0));
    }

    #[test]
    fn test_path_b_disposal_background_preserved() {
        // Create a sequence with Background disposal
        let frame1 = make_frame(100, 100, [255, 0, 0, 255]); // Red, opaque
        let frame2 =
            make_frame_with_disposal(100, 100, [0, 255, 0, 255], DisposalMethod::Background);

        let frames = vec![frame1, frame2];
        let config = PathBConfig::default();
        let optimized = optimize_path_b(&frames, config);

        // Should have 2 frames
        assert_eq!(optimized.len(), 2);
        // Second frame should preserve Background disposal
        assert_eq!(optimized[1].dispose, DisposalMethod::Background);
    }

    #[test]
    fn test_path_b_disposal_previous_preserved() {
        // Create a sequence with Previous disposal
        let frame1 = make_frame(100, 100, [255, 0, 0, 255]); // Red
        let frame2 = make_frame_with_disposal(100, 100, [0, 255, 0, 255], DisposalMethod::Previous);
        let frame3 = make_frame(100, 100, [0, 0, 255, 255]); // Blue

        let frames = vec![frame1, frame2, frame3];
        let config = PathBConfig::default();
        let optimized = optimize_path_b(&frames, config);

        // Should have 3 frames
        assert_eq!(optimized.len(), 3);
        // Second frame should preserve Previous disposal
        assert_eq!(optimized[1].dispose, DisposalMethod::Previous);
    }

    #[test]
    fn test_path_b_optimize_lossy_semantics_separated() {
        // Test that structural and lossy optimization are separate
        let frame1 = make_frame(100, 100, [255, 0, 0, 255]); // Red
        let frame2 = make_frame(100, 100, [255, 0, 0, 255]); // Identical

        let frames = vec![frame1, frame2];

        // Structural optimization (lossless)
        let config = PathBConfig::default();
        let structural = optimize_path_b(&frames, config);
        assert_eq!(structural.len(), 2);

        // Lossy optimization
        let lossy = optimize_path_b_lossy(&frames, 80, 100, 100);
        assert_eq!(lossy.len(), 2);

        // Both should preserve the frame structure
        assert_eq!(structural.len(), lossy.len());
    }

    #[test]
    fn test_path_b_identical_frames_with_keep_disposal() {
        // Identical frames with Keep disposal should collapse to minimal 1x1
        let frame1 = make_frame_with_disposal(100, 100, [255, 0, 0, 255], DisposalMethod::Keep);
        let frame2 = make_frame_with_disposal(100, 100, [255, 0, 0, 255], DisposalMethod::Keep);

        let frames = vec![frame1, frame2];
        let config = PathBConfig::default();
        let optimized = optimize_path_b(&frames, config);

        // Second frame should be collapsed to 1x1
        assert_eq!(optimized[1].width, 1);
        assert_eq!(optimized[1].height, 1);
    }

    #[test]
    fn test_path_b_identical_frames_with_background_disposal_not_collapsed() {
        // Identical frames with Background disposal should NOT collapse
        let frame1 = make_frame_with_disposal(100, 100, [255, 0, 0, 255], DisposalMethod::Keep);
        let frame2 =
            make_frame_with_disposal(100, 100, [255, 0, 0, 255], DisposalMethod::Background);

        let frames = vec![frame1, frame2];
        let config = PathBConfig::default();
        let optimized = optimize_path_b(&frames, config);

        // Second frame should NOT be collapsed (Background disposal changes canvas state)
        assert_eq!(optimized[1].width, 100);
        assert_eq!(optimized[1].height, 100);
    }

    #[test]
    fn test_path_b_o3_crops_to_bbox() {
        // O3 should crop to bounding box
        let frame1 = make_frame(100, 100, [255, 0, 0, 255]);
        let mut frame2 = make_frame(100, 100, [255, 0, 0, 255]);

        // Make only a small region different in frame2
        for i in 0..10 {
            for j in 0..10 {
                let idx = (i * 100 + j) * 4;
                frame2.pixels[idx] = 0; // Change red to black
            }
        }

        let frames = vec![frame1, frame2];
        let config = PathBConfig {
            level: OptLevel::O3,
        };
        let optimized = optimize_path_b(&frames, config);

        // Second frame should be cropped (smaller than 100x100)
        assert!(optimized[1].width < 100 || optimized[1].height < 100);
    }
}

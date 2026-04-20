//! Frame optimization - structural transparency optimization for GIF frames.
//!
//! Marks unchanged pixels as transparent to reduce file size. All optimization levels
//! are **lossless** — they only mark pixels transparent, never modifying color values.
//!
//! # Optimization Levels
//!
//! - **O1**: Exact pixel match only. No cropping. Minimal compression.
//! - **O2**: Exact pixel match only. No cropping. Same as O1 (for consistency).
//! - **O3**: Exact pixel match only. Crops to bounding box of changed pixels.
//!   Subframes use `DisposalMethod::Keep` to preserve canvas state.
//!
//! # Perceptual Thresholding
//!
//! Perceptual pixel-thresholding (marking "close enough" pixels as transparent)
//! is **not** part of `optimize()`. Use `lossy(quality)` for perceptual compression:
//!
//! ```ignore
//! let gif = Gif::from_bytes(&data)?;
//! let optimized = gif.optimize(OptLevel::O3);  // Structural only
//! let compressed = optimized.lossy(80);         // Add perceptual thresholding
//! ```
//!
//! # Historical Note
//!
//! Prior to 2026-04-20, O3 used `threshold=8` (perceptual) + `crop_to_bbox=true`.
//! This caused severe quality loss on opaque-delta / global-palette sequences
//! (e.g., voyager: 26.74 Butteraugli). The threshold was removed to make
//! `optimize()` purely structural. Perceptual compression now lives exclusively
//! in `lossy()`.

use crate::simd_opt::{find_diff_bounding_box, mark_unchanged_pixels_simd, DiffRect};
use crate::types::{DisposalMethod, Frame, Gif, OptLevel};
use rayon::prelude::*;

impl Gif {
    /// Optimize frames by marking unchanged pixels transparent.
    ///
    /// For each frame (except the first), compares pixels to the previous frame
    /// and marks unchanged pixels as transparent. This is a **lossless** operation
    /// that only affects transparency, never modifying color values.
    ///
    /// # Optimization Levels
    ///
    /// - **O1**: Exact pixel match. No cropping.
    /// - **O2**: Exact pixel match. No cropping. (Same as O1 for consistency.)
    /// - **O3**: Exact pixel match. Crops to bounding box of changed pixels.
    ///
    /// For perceptual compression (marking "close enough" pixels as transparent),
    /// use `lossy(quality)` after `optimize()`.
    ///
    /// # Arguments
    /// * `level` - Optimization level controlling cropping behavior
    ///
    /// # Returns
    /// A new Gif with optimized frames
    ///
    /// # Example
    /// ```ignore
    /// let gif = Gif::from_bytes(&data)?;
    /// let optimized = gif.optimize(OptLevel::O3);  // Structural only
    /// let compressed = optimized.lossy(80);         // Add perceptual compression
    /// ```
    #[must_use]
    pub fn optimize(self, level: OptLevel) -> Gif {
        if self.frames.is_empty() {
            return self.clone();
        }

        // All optimization levels use exact pixel match (threshold=0, lossless)
        let threshold = 0;

        // Only crop to bounding box for O3 (most aggressive)
        let crop_to_bbox = level == OptLevel::O3;

        let optimized_frames = optimize_frames_internal(&self.frames, threshold, crop_to_bbox);

        Gif {
            width: self.width,
            height: self.height,
            global_palette: self.global_palette.clone(),
            frames: optimized_frames,
            loop_count: self.loop_count,
            // Optimization changes pixel content and can introduce sparse/cropped frames,
            // so the decode-time source palette is no longer a reliable fast-path palette.
            // Force re-quantization on encode to avoid stale-palette quality collapse.
            original_palette: None,
        }
    }

    /// Lossy compression - mark "close enough" pixels as transparent.
    ///
    /// Compares each frame to the previous frame and marks pixels that are
    /// within a perceptual distance threshold as transparent. This allows
    /// the encoder to skip storing those pixels, resulting in significant
    /// size reduction.
    ///
    /// Quality 100 is lossless (threshold 0). Lower quality values increase
    /// the threshold, making more pixels transparent and reducing file size.
    ///
    /// Works correctly even on subframed GIFs (e.g., output of `optimize(O3)`).
    /// For subframes, the comparison is done in the full canvas coordinate space
    /// to ensure spatial correctness.
    ///
    /// # Arguments
    /// * `quality` - Quality level 0-100 (100 = lossless, 0 = maximum loss)
    ///
    /// # Returns
    /// A new Gif with lossy compression applied
    ///
    /// # Examples
    /// ```ignore
    /// let gif = Gif::from_bytes(&data)?;
    /// let compressed = gif.lossy(80); // 20% quality loss for size reduction
    /// ```
    #[must_use]
    pub fn lossy(self, quality: u8) -> Gif {
        if self.frames.is_empty() {
            return self.clone();
        }

        // Clamp quality to 0-100 range
        let quality = quality.min(100);

        // Calculate threshold: quality 100 = 0 (lossless), quality 0 = 20 (conservative max)
        let threshold = ((100 - quality as u16) * 20 / 100) as u8;

        // Always crop to bounding box for lossy (maximize compression)
        let crop_to_bbox = true;

        let lossy_frames = optimize_frames_internal_with_canvas(
            &self.frames,
            threshold,
            crop_to_bbox,
            self.width,
            self.height,
        );

        Gif {
            width: self.width,
            height: self.height,
            global_palette: self.global_palette.clone(),
            frames: lossy_frames,
            loop_count: self.loop_count,
            // Lossy modifies frame pixels by thresholding to transparency.
            // Keep encode quality stable by disabling stale decode-palette fast path.
            original_palette: None,
        }
    }
}

/// Internal optimization with configurable threshold and cropping.
/// Shared by both optimize() and lossy() methods.
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
/// Used by lossy() to correctly handle frames that may be cropped/subframed.
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
fn precompute_reference_canvases(frames: &[Frame], canvas_width: u16, canvas_height: u16) -> Vec<Frame> {
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
                let is_subframe = frame.left != 0 || frame.top != 0 || frame.width != canvas_width || frame.height != canvas_height;
                
                if is_subframe {
                    // Composite the subframe patch onto the current canvas
                    let base_canvas = displayed_canvas
                        .clone()
                        .unwrap_or_else(|| create_transparent_canvas(canvas_width, canvas_height));
                    displayed_canvas = Some(composite_frame_onto_canvas(&base_canvas, frame, canvas_width, canvas_height));
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
    let diff_rect = match find_diff_bounding_box(&prev_overlap, &frame_overlap, overlap_width, overlap_height, threshold) {
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
            dispose: frame.dispose,  // Preserve original disposal method
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
        dispose: frame.dispose,  // Preserve original disposal method
        local_palette: None,
        left: diff_rect.left,
        top: diff_rect.top,
        width: diff_rect.width,
        height: diff_rect.height,
    }
}

/// Check if two pixels are similar within threshold (per-channel).
#[inline]
#[allow(dead_code)]
fn pixels_similar(a: &[u8; 4], b: &[u8; 4], threshold: u8) -> bool {
    a[0].abs_diff(b[0]) <= threshold
        && a[1].abs_diff(b[1]) <= threshold
        && a[2].abs_diff(b[2]) <= threshold
        && a[3].abs_diff(b[3]) <= threshold
}

/// Check if two colors are similar using perceptual distance.
///
/// Uses weighted RGB distance where human eye is more sensitive to green:
/// - R: weight 0.3
/// - G: weight 0.5
/// - B: weight 0.2
#[inline]
#[allow(dead_code)]
fn colors_similar(a: &[u8; 4], b: &[u8; 4], threshold: u8) -> bool {
    // Ignore alpha channel for color comparison
    let dr = a[0].abs_diff(b[0]) as u32;
    let dg = a[1].abs_diff(b[1]) as u32;
    let db = a[2].abs_diff(b[2]) as u32;

    // Perceptual distance: weighted RGB
    // Weights: R=0.3, G=0.5, B=0.2 (normalized to sum to 1.0)
    // Multiply by 10 to avoid floating point: R=3, G=5, B=2
    let weighted_distance = (dr * 3 + dg * 5 + db * 2) / 10;

    weighted_distance <= threshold as u32
}

/// Find bounding box of non-transparent pixels.
/// Returns (left, top, right, bottom) or None if all pixels are transparent.
#[allow(dead_code)]
fn find_bounding_box(pixels: &[u8], width: usize, height: usize) -> Option<(u16, u16, u16, u16)> {
    let mut left = width;
    let mut top = height;
    let mut right = 0;
    let mut bottom = 0;

    for y in 0..height {
        for x in 0..width {
            let pixel_idx = (y * width + x) * 4;
            let alpha = pixels.get(pixel_idx + 3).copied().unwrap_or(0);

            if alpha > 0 {
                left = left.min(x);
                top = top.min(y);
                right = right.max(x);
                bottom = bottom.max(y);
            }
        }
    }

    if left <= right && top <= bottom {
        Some((
            left as u16,
            top as u16,
            (right + 1) as u16,
            (bottom + 1) as u16,
        ))
    } else {
        None
    }
}

/// Crop frame to specified bounds.
#[allow(dead_code)]
fn crop_frame(frame: &Frame, left: u16, top: u16, right: u16, bottom: u16) -> Frame {
    let new_width = (right - left) as usize;
    let new_height = (bottom - top) as usize;
    let old_width = frame.width as usize;

    let mut new_pixels = vec![0u8; new_width * new_height * 4];

    for y in 0..new_height {
        for x in 0..new_width {
            let old_x = left as usize + x;
            let old_y = top as usize + y;

            let old_idx = (old_y * old_width + old_x) * 4;
            let new_idx = (y * new_width + x) * 4;

            if old_idx + 3 < frame.pixels.len() {
                new_pixels[new_idx] = frame.pixels[old_idx];
                new_pixels[new_idx + 1] = frame.pixels[old_idx + 1];
                new_pixels[new_idx + 2] = frame.pixels[old_idx + 2];
                new_pixels[new_idx + 3] = frame.pixels[old_idx + 3];
            }
        }
    }

    Frame {
        pixels: new_pixels,
        delay: frame.delay,
        dispose: frame.dispose,
        local_palette: frame.local_palette.clone(),
        left: frame.left + left,
        top: frame.top + top,
        width: new_width as u16,
        height: new_height as u16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DisposalMethod;
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

    #[test]
    fn test_pixels_similar_exact() {
        let a = [255, 128, 64, 255];
        let b = [255, 128, 64, 255];
        assert!(pixels_similar(&a, &b, 0));
    }

    #[test]
    fn test_pixels_similar_threshold() {
        let a = [255, 128, 64, 255];
        let b = [254, 129, 65, 254];
        assert!(pixels_similar(&a, &b, 2));
        assert!(!pixels_similar(&a, &b, 0));
    }

    #[test]
    fn test_colors_similar_exact() {
        let a = [255, 128, 64, 255];
        let b = [255, 128, 64, 255];
        assert!(colors_similar(&a, &b, 0));
    }

    #[test]
    fn test_colors_similar_perceptual() {
        // Same color difference in different channels should have different distances
        let base = [128, 128, 128, 255];
        let red_diff = [138, 128, 128, 255]; // +10 red
        let green_diff = [128, 138, 128, 255]; // +10 green
        let blue_diff = [128, 128, 138, 255]; // +10 blue

        // Green difference should be largest (weight 0.5)
        // Red difference should be medium (weight 0.3)
        // Blue difference should be smallest (weight 0.2)
        let threshold = 5;
        assert!(colors_similar(&base, &red_diff, threshold)); // 10*3/10 = 3, should pass
        assert!(colors_similar(&base, &green_diff, threshold)); // 10*5/10 = 5, should pass
        assert!(colors_similar(&base, &blue_diff, threshold)); // 10*2/10 = 2, should pass

        // With lower threshold, green diff should fail
        let threshold = 4;
        assert!(colors_similar(&base, &red_diff, threshold)); // 3 <= 4, pass
        assert!(!colors_similar(&base, &green_diff, threshold)); // 5 > 4, fail
        assert!(colors_similar(&base, &blue_diff, threshold)); // 2 <= 4, pass
    }

    #[test]
    fn test_optimize_first_frame_unchanged() {
        let frame1 = make_frame(2, 2, [255, 0, 0, 255]);
        let frame2 = make_frame(2, 2, [0, 255, 0, 255]);

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame1.clone(), frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let optimized = gif.optimize(OptLevel::O1);
        assert_eq!(optimized.frames[0].pixels, frame1.pixels);
    }

    #[test]
    fn test_optimize_marks_unchanged_transparent() {
        let frame1 = make_frame(2, 2, [255, 0, 0, 255]);
        let mut frame2_pixels = Vec::new();
        for _ in 0..4 {
            frame2_pixels.extend_from_slice(&[255, 0, 0, 255]);
        }
        // Change one pixel
        frame2_pixels[0] = 0;
        frame2_pixels[1] = 255;

        let frame2 = Frame {
            pixels: frame2_pixels,
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::None,
            local_palette: None,
            left: 0,
            top: 0,
            width: 2,
            height: 2,
        };

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame1, frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let optimized = gif.optimize(OptLevel::O1);
        let frame2_opt = &optimized.frames[1];

        // First pixel should be opaque (changed)
        assert_eq!(frame2_opt.pixels[3], 255);

        // Other pixels should be transparent (unchanged)
        assert_eq!(frame2_opt.pixels[7], 0); // Second pixel alpha
        assert_eq!(frame2_opt.pixels[11], 0); // Third pixel alpha
        assert_eq!(frame2_opt.pixels[15], 0); // Fourth pixel alpha
    }

    #[test]
    fn test_optimize_preserves_disposal_none() {
        let frame1 = make_frame(2, 2, [255, 0, 0, 255]);
        let mut frame2 = make_frame(2, 2, [255, 0, 0, 255]);
        frame2.dispose = DisposalMethod::None;

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame1, frame2.clone()],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let optimized = gif.optimize(OptLevel::O1);
        assert_eq!(optimized.frames[1].dispose, DisposalMethod::None);
    }

    #[test]
    fn test_optimize_preserves_disposal_background() {
        let frame1 = make_frame(2, 2, [255, 0, 0, 255]);
        let mut frame2 = make_frame(2, 2, [255, 0, 0, 255]);
        frame2.dispose = DisposalMethod::Background;

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame1, frame2.clone()],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let optimized = gif.optimize(OptLevel::O1);
        assert_eq!(optimized.frames[1].dispose, DisposalMethod::Background);
    }

    #[test]
    fn test_optimize_preserves_disposal_previous() {
        let frame1 = make_frame(2, 2, [255, 0, 0, 255]);
        let mut frame2 = make_frame(2, 2, [255, 0, 0, 255]);
        frame2.dispose = DisposalMethod::Previous;

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame1, frame2.clone()],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let optimized = gif.optimize(OptLevel::O1);
        assert_eq!(optimized.frames[1].dispose, DisposalMethod::Previous);
    }

    #[test]
    fn test_optimize_identical_frames_with_keep_disposal_collapses() {
        // When frames are identical and disposal is Keep, we can safely collapse to 1x1
        // because the canvas state is unchanged after disposal
        let frame1 = make_frame(2, 2, [255, 0, 0, 255]);
        let mut frame2 = make_frame(2, 2, [255, 0, 0, 255]);
        frame2.dispose = DisposalMethod::Keep;

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame1, frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let optimized = gif.optimize(OptLevel::O3);
        // Identical frames with Keep disposal can safely collapse to minimal (1x1)
        assert_eq!(optimized.frames[1].width, 1);
        assert_eq!(optimized.frames[1].height, 1);
        assert_eq!(optimized.frames[1].dispose, DisposalMethod::Keep);
    }

    #[test]
    fn test_optimize_identical_frames_with_background_disposal_no_collapse() {
        // When frames are identical but disposal is Background, we must NOT collapse
        // because the canvas is cleared after this frame, making the next frame a real change
        let frame1 = make_frame(2, 2, [255, 0, 0, 255]);
        let mut frame2 = make_frame(2, 2, [255, 0, 0, 255]);
        frame2.dispose = DisposalMethod::Background;

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame1, frame2.clone()],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let optimized = gif.optimize(OptLevel::O3);
        // Identical frames with Background disposal must NOT collapse
        // because the disposal changes the canvas state for the next frame
        assert_eq!(optimized.frames[1].width, frame2.width);
        assert_eq!(optimized.frames[1].height, frame2.height);
        assert_eq!(optimized.frames[1].dispose, DisposalMethod::Background);
    }

    #[test]
    fn test_optimize_identical_frames_with_previous_disposal_no_collapse() {
        // When frames are identical but disposal is Previous, we must NOT collapse
        // because the canvas is restored after this frame, making the next frame a real change
        let frame1 = make_frame(2, 2, [255, 0, 0, 255]);
        let mut frame2 = make_frame(2, 2, [255, 0, 0, 255]);
        frame2.dispose = DisposalMethod::Previous;

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame1, frame2.clone()],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let optimized = gif.optimize(OptLevel::O3);
        // Identical frames with Previous disposal must NOT collapse
        // because the disposal changes the canvas state for the next frame
        assert_eq!(optimized.frames[1].width, frame2.width);
        assert_eq!(optimized.frames[1].height, frame2.height);
        assert_eq!(optimized.frames[1].dispose, DisposalMethod::Previous);
    }

    #[test]
    fn test_lossy_first_frame_unchanged() {
        let frame1 = make_frame(2, 2, [255, 0, 0, 255]);
        let frame2 = make_frame(2, 2, [0, 255, 0, 255]);

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame1.clone(), frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let lossy = gif.clone().lossy(80);
        assert_eq!(lossy.frames[0].pixels, frame1.pixels);
    }

    #[test]
    fn test_lossy_quality_100_lossless() {
        let frame1 = make_frame(2, 2, [255, 0, 0, 255]);
        let frame2 = make_frame(2, 2, [254, 1, 1, 255]);

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame1, frame2.clone()],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let lossy = gif.lossy(100);
        // Quality 100 should have threshold 0, so no pixels should be marked transparent
        assert_eq!(lossy.frames[1].pixels, frame2.pixels);
    }

    #[test]
    fn test_lossy_quality_0_maximum_loss() {
        let frame1 = make_frame(2, 2, [128, 128, 128, 255]);
        let frame2 = make_frame(2, 2, [129, 129, 129, 255]);

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame1, frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let lossy = gif.lossy(0);
        // Quality 0 should have threshold 20, so pixels with diff 1 should be transparent.
        // With cropping enabled for lossy, all pixels are identical within threshold,
        // so the frame becomes minimal (1x1 transparent).
        let frame2_lossy = &lossy.frames[1];
        assert_eq!(frame2_lossy.width, 1);
        assert_eq!(frame2_lossy.height, 1);
        assert_eq!(frame2_lossy.pixels.len(), 4); // 1x1 RGBA
    }

    #[test]
    fn test_lossy_threshold_calculation() {
        // New formula: threshold = (100 - quality) * 20 / 100
        // quality 80 = 4, quality 50 = 10, quality 0 = 20

        let frame1 = make_frame(1, 1, [128, 128, 128, 255]);
        let frame2 = make_frame(1, 1, [130, 128, 128, 255]); // +2 red

        let gif = Gif {
            width: 1,
            height: 1,
            global_palette: None,
            frames: vec![frame1, frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        // Quality 80: threshold = 4
        // Red diff: 2, should be transparent (2 <= 4)
        let lossy = gif.clone().lossy(80);
        assert_eq!(
            lossy.frames[1].pixels[3], 0,
            "Pixel should be transparent at quality 80 (diff 2 <= threshold 4)"
        );

        // Quality 95: threshold = 1
        // Red diff: 2, should NOT be transparent (2 > 1)
        let lossy = gif.lossy(95);
        assert_eq!(
            lossy.frames[1].pixels[3], 255,
            "Pixel should be opaque at quality 95 (diff 2 > threshold 1)"
        );
    }

    #[test]
    fn test_optimize_identical_frames_returns_minimal() {
        let frame1 = make_frame(10, 10, [255, 0, 0, 255]);
        let frame2 = make_frame(10, 10, [255, 0, 0, 255]); // identical

        let gif = Gif {
            width: 10,
            height: 10,
            global_palette: None,
            frames: vec![frame1, frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let optimized = gif.optimize(OptLevel::O3);
        // Second frame should be minimal (1x1)
        assert_eq!(optimized.frames[1].width, 1);
        assert_eq!(optimized.frames[1].height, 1);
        assert_eq!(optimized.frames[1].pixels.len(), 4); // 1x1 RGBA
    }

    #[test]
    fn test_optimize_clears_original_palette_for_safe_requantization() {
        let frame1 = make_frame(2, 2, [255, 0, 0, 255]);
        let frame2 = make_frame(2, 2, [0, 255, 0, 255]);

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame1, frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: Some(vec![[255, 0, 0], [0, 255, 0]]),
        };

        let optimized = gif.optimize(OptLevel::O3);
        assert!(optimized.original_palette.is_none());
    }

    #[test]
    fn test_lossy_clears_original_palette_for_safe_requantization() {
        let frame1 = make_frame(2, 2, [255, 0, 0, 255]);
        let frame2 = make_frame(2, 2, [254, 1, 1, 255]);

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame1, frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: Some(vec![[255, 0, 0], [0, 255, 0]]),
        };

        let lossy = gif.lossy(80);
        assert!(lossy.original_palette.is_none());
    }

    #[test]
    fn test_optimize_skips_misaligned_subframes() {
        let prev_frame = Frame {
            pixels: vec![
                255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
            ],
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::Keep,
            local_palette: None,
            left: 0,
            top: 0,
            width: 2,
            height: 2,
        };

        let frame = Frame {
            pixels: vec![
                0, 255, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255,
            ],
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::Keep,
            local_palette: None,
            left: 1,
            top: 0,
            width: 2,
            height: 2,
        };

        let optimized = optimize_frame_internal(&frame, &prev_frame, 8, true);
        assert_eq!(optimized.pixels, frame.pixels);
        assert_eq!(optimized.left, frame.left);
        assert_eq!(optimized.top, frame.top);
        assert_eq!(optimized.width, frame.width);
        assert_eq!(optimized.height, frame.height);
    }

    #[test]
    fn test_lossy_works_on_subframes() {
        // Create two full-canvas frames, then manually create subframes to test lossy on them
        // Frame 1: full canvas with one color
        let frame1 = make_frame(4, 4, [128, 128, 128, 255]);

        // Frame 2: subframe at (1, 0) with slightly different color
        let frame2 = Frame {
            pixels: vec![
                129, 128, 128, 255, 129, 128, 128, 255, 129, 128, 128, 255, 129, 128, 128, 255,
            ],
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::Keep,
            local_palette: None,
            left: 1,
            top: 0,
            width: 2,
            height: 2,
        };

        let gif = Gif {
            width: 4,
            height: 4,
            global_palette: None,
            frames: vec![frame1, frame2.clone()],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        // Apply lossy with quality 80 (threshold = 4)
        // Pixel diff is 1 in red channel, which is <= 4, so should be marked transparent
        let lossy = gif.lossy(80);
        
        // First frame should be unchanged (first frames are never optimized)
        assert_eq!(lossy.frames[0].width, 4);
        assert_eq!(lossy.frames[0].height, 4);
        
        // Second frame should have pixels marked transparent due to lossy compression
        // The diff is small (1 in red), so with threshold 4, pixels should be transparent
        assert_eq!(lossy.frames[1].pixels[3], 0, "Pixel should be transparent after lossy compression");
        
        // Frame should be a subframe (not full canvas)
        assert!(lossy.frames[1].width < 4 || lossy.frames[1].height < 4 || lossy.frames[1].left != 0 || lossy.frames[1].top != 0);
    }

    #[test]
    fn test_optimize_o3_then_lossy_applies_compression() {
        // This is the critical test case: optimize(O3) creates subframes (lossless),
        // then lossy() should apply perceptual compression on top
        let frame1 = make_frame(4, 4, [128, 128, 128, 255]);
        let mut frame2_pixels = Vec::new();
        for _ in 0..16 {
            frame2_pixels.extend_from_slice(&[128, 128, 128, 255]);
        }
        // Change one pixel to create a small diff region (diff=2 in red)
        frame2_pixels[0] = 130;
        frame2_pixels[1] = 128;
        frame2_pixels[2] = 128;

        let frame2 = Frame {
            pixels: frame2_pixels,
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::None,
            local_palette: None,
            left: 0,
            top: 0,
            width: 4,
            height: 4,
        };

        let gif = Gif {
            width: 4,
            height: 4,
            global_palette: None,
            frames: vec![frame1, frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        // First: optimize with O3 (creates subframes via cropping, lossless)
        let optimized = gif.optimize(OptLevel::O3);
        
        // Verify that O3 created a subframe (cropped to diff region)
        // The second frame should be smaller than 4x4
        assert!(
            optimized.frames[1].width < 4 || optimized.frames[1].height < 4,
            "O3 should create subframes via cropping"
        );
        
        // After O3 (lossless), the changed pixel should still be opaque
        // because diff=2 > threshold=0
        let o3_has_opaque = optimized.frames[1].pixels.iter().skip(3).step_by(4).any(|&a| a == 255);
        assert!(
            o3_has_opaque,
            "O3 (lossless) should preserve opaque pixels with diff > 0"
        );
        
        // Now apply lossy(80) - this should mark the small diff transparent
        // lossy(80) has threshold = (100-80)*20/100 = 4
        // diff=2 <= 4, so the pixel should become transparent
        let lossy = optimized.lossy(80);
        
        // The lossy result should have some transparent pixels
        let has_transparent = lossy.frames[1].pixels.iter().skip(3).step_by(4).any(|&a| a == 0);
        assert!(
            has_transparent,
            "Lossy(80) should mark pixels with diff <= 4 as transparent"
        );
    }

    #[test]
    fn test_optimize_o3_preserves_disposal_with_subframes() {
        // Test that disposal method is preserved even when O3 creates subframes
        let frame1 = make_frame(4, 4, [128, 128, 128, 255]);
        let mut frame2_pixels = Vec::new();
        for _ in 0..16 {
            frame2_pixels.extend_from_slice(&[128, 128, 128, 255]);
        }
        // Change one pixel to create a small diff region
        frame2_pixels[0] = 130;
        frame2_pixels[1] = 128;
        frame2_pixels[2] = 128;

        let frame2 = Frame {
            pixels: frame2_pixels,
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::Background,
            local_palette: None,
            left: 0,
            top: 0,
            width: 4,
            height: 4,
        };

        let gif = Gif {
            width: 4,
            height: 4,
            global_palette: None,
            frames: vec![frame1, frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let optimized = gif.optimize(OptLevel::O3);
        
        // Even though O3 created a subframe, disposal method should be preserved
        assert_eq!(optimized.frames[1].dispose, DisposalMethod::Background);
    }

    #[test]
    fn test_lossy_preserves_disposal_background() {
        let frame1 = make_frame(2, 2, [255, 0, 0, 255]);
        let mut frame2 = make_frame(2, 2, [254, 1, 1, 255]);
        frame2.dispose = DisposalMethod::Background;

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame1, frame2.clone()],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let lossy = gif.lossy(80);
        assert_eq!(lossy.frames[1].dispose, DisposalMethod::Background);
    }

    #[test]
    fn test_lossy_preserves_disposal_previous() {
        let frame1 = make_frame(2, 2, [255, 0, 0, 255]);
        let mut frame2 = make_frame(2, 2, [254, 1, 1, 255]);
        frame2.dispose = DisposalMethod::Previous;

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame1, frame2.clone()],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let lossy = gif.lossy(80);
        assert_eq!(lossy.frames[1].dispose, DisposalMethod::Previous);
    }

    #[test]
    fn test_optimize_misaligned_subframes_preserves_disposal() {
        // When frames don't align spatially, optimization is skipped
        // and disposal method should be preserved
        let prev_frame = Frame {
            pixels: vec![
                255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
            ],
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::Keep,
            local_palette: None,
            left: 0,
            top: 0,
            width: 2,
            height: 2,
        };

        let frame = Frame {
            pixels: vec![
                0, 255, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255,
            ],
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::Background,
            local_palette: None,
            left: 1,
            top: 0,
            width: 2,
            height: 2,
        };

        let optimized = optimize_frame_internal(&frame, &prev_frame, 8, true);
        // Misaligned frames should be returned unchanged
        assert_eq!(optimized.pixels, frame.pixels);
        assert_eq!(optimized.dispose, DisposalMethod::Background);
    }

    // -------------------------------------------------------------------------
    // Regression tests: disposal-aware reference state for optimization
    //
    // These tests encode the CORRECT expected behavior for disposal-heavy GIFs.
    // They are expected to FAIL on the current implementation because
    // optimize_frames_internal always uses frame[N-1] as the reference, ignoring
    // that Background/Previous disposal changes the canvas state before frame[N]
    // is drawn.
    //
    // Bug model:
    //   Current:  reference = displayed(frame[N-1])
    //   Correct:  reference = canvas state AFTER applying frame[N-1]'s disposal
    //             (i.e. the canvas that frame[N] is drawn onto)
    // -------------------------------------------------------------------------

    /// Helper: build a 2×2 canvas-sized frame with uniform color.
    fn make_canvas_frame(color: [u8; 4], dispose: DisposalMethod) -> Frame {
        Frame {
            pixels: vec![
                color[0], color[1], color[2], color[3],
                color[0], color[1], color[2], color[3],
                color[0], color[1], color[2], color[3],
                color[0], color[1], color[2], color[3],
            ],
            delay: Duration::from_millis(100),
            dispose,
            local_palette: None,
            left: 0,
            top: 0,
            width: 2,
            height: 2,
        }
    }

    /// REGRESSION: Background disposal — frame[N] must be compared against the
    /// cleared canvas, NOT against the displayed pixels of frame[N-1].
    ///
    /// Scenario (2×2 canvas):
    ///   frame[0]: solid RED,  dispose=Background  → canvas cleared to transparent after display
    ///   frame[1]: solid RED,  dispose=Keep        → drawn onto a transparent canvas
    ///
    /// Correct reference for optimizing frame[1] = transparent canvas (all zeros).
    /// Because frame[1] is RED and the reference is transparent, every pixel differs
    /// → frame[1] should NOT be collapsed to a minimal transparent frame.
    ///
    /// Current (buggy) behavior: reference = frame[0] pixels = RED.
    /// RED == RED → all pixels "unchanged" → frame[1] incorrectly becomes 1×1 transparent.
    #[test]
    fn test_regression_background_disposal_reference_is_cleared_canvas() {
        let red = [255u8, 0, 0, 255];
        // frame[0]: red, will be disposed by clearing to background
        let frame0 = make_canvas_frame(red, DisposalMethod::Background);
        // frame[1]: red again, drawn onto the now-transparent canvas
        let frame1 = make_canvas_frame(red, DisposalMethod::Keep);

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame0.clone(), frame1.clone()],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let optimized = gif.optimize(OptLevel::O1);
        let opt1 = &optimized.frames[1];

        // Correct reference = post-disposal canvas = transparent.
        // frame[1] pixels are RED, which differs from transparent on every pixel.
        // Therefore frame[1] must be kept fully opaque — no pixels should be
        // marked transparent, and the frame must not be collapsed to 1×1.
        assert_ne!(
            opt1.width, 1,
            "BUG: frame[1] was collapsed to 1×1 because optimizer used frame[0] \
             (RED) as reference instead of the post-Background-disposal canvas \
             (transparent). After Background disposal the canvas is cleared, so \
             frame[1] (RED) differs from the reference on every pixel."
        );
        // All pixels in the optimized frame must remain opaque
        for (i, chunk) in opt1.pixels.chunks(4).enumerate() {
            assert_eq!(
                chunk[3], 255,
                "pixel {i} alpha should be 255 (opaque): frame[1] is RED drawn \
                 onto a transparent canvas, so every pixel is a real change"
            );
        }
    }

    /// REGRESSION: Background disposal — identical-looking frame after disposal
    /// must NOT be silently dropped.
    ///
    /// Scenario (2×2 canvas):
    ///   frame[0]: solid BLUE, dispose=Background  → canvas cleared to transparent
    ///   frame[1]: solid BLUE, dispose=Keep        → drawn onto transparent canvas
    ///   frame[2]: solid BLUE, dispose=Keep        → drawn onto BLUE canvas (frame[1] kept)
    ///
    /// Correct references:
    ///   frame[1] ref = transparent  → BLUE ≠ transparent → keep all pixels opaque
    ///   frame[2] ref = BLUE         → BLUE == BLUE       → collapse to minimal (OK)
    ///
    /// Current (buggy) behavior for frame[1]:
    ///   ref = frame[0] = BLUE → BLUE == BLUE → incorrectly collapses frame[1].
    #[test]
    fn test_regression_background_disposal_three_frame_sequence() {
        let blue = [0u8, 0, 255, 255];

        let frame0 = make_canvas_frame(blue, DisposalMethod::Background);
        let frame1 = make_canvas_frame(blue, DisposalMethod::Keep);
        let frame2 = make_canvas_frame(blue, DisposalMethod::Keep);

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame0, frame1, frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let optimized = gif.optimize(OptLevel::O1);

        // frame[1]: reference should be transparent (post-Background disposal of frame[0]).
        // BLUE ≠ transparent → must NOT be collapsed.
        assert_ne!(
            optimized.frames[1].width, 1,
            "BUG: frame[1] collapsed — optimizer used frame[0] (BLUE) as reference \
             instead of the post-Background-disposal canvas (transparent)"
        );

        // frame[2]: reference should be BLUE (frame[1] kept on canvas).
        // BLUE == BLUE → correctly collapses to minimal.
        assert_eq!(
            optimized.frames[2].width, 1,
            "frame[2] should collapse: reference is BLUE (frame[1] kept) and \
             frame[2] is also BLUE — no visible change"
        );
    }

    /// REGRESSION: Previous disposal — frame[N] must be compared against the
    /// restored canvas, NOT against the displayed pixels of frame[N-1].
    ///
    /// Scenario (2×2 canvas):
    ///   frame[0]: solid GREEN, dispose=Keep      → canvas stays GREEN
    ///   frame[1]: solid RED,   dispose=Previous  → canvas restored to GREEN after display
    ///   frame[2]: solid GREEN, dispose=Keep      → drawn onto GREEN (restored) canvas
    ///
    /// Correct reference for optimizing frame[2]:
    ///   = post-Previous-disposal canvas = GREEN (restored from before frame[1])
    ///   GREEN == GREEN → frame[2] should collapse to minimal.
    ///
    /// Current (buggy) behavior:
    ///   ref = frame[1] = RED → GREEN ≠ RED → frame[2] is NOT collapsed (wrong).
    #[test]
    fn test_regression_previous_disposal_reference_is_restored_canvas() {
        let green = [0u8, 255, 0, 255];
        let red = [255u8, 0, 0, 255];

        let frame0 = make_canvas_frame(green, DisposalMethod::Keep);
        let frame1 = make_canvas_frame(red, DisposalMethod::Previous);
        let frame2 = make_canvas_frame(green, DisposalMethod::Keep);

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame0, frame1, frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let optimized = gif.optimize(OptLevel::O1);

        // frame[2] reference = post-Previous-disposal canvas = GREEN.
        // frame[2] is GREEN → no change → should collapse to minimal 1×1.
        assert_eq!(
            optimized.frames[2].width, 1,
            "BUG: frame[2] was NOT collapsed. After Previous disposal of frame[1] \
             the canvas is restored to GREEN (frame[0]). frame[2] is also GREEN, \
             so it should be a no-op and collapse to 1×1. The optimizer incorrectly \
             used frame[1] (RED) as the reference."
        );
    }

    /// REGRESSION: Previous disposal — frame that differs from restored canvas
    /// must NOT be collapsed.
    ///
    /// Scenario (2×2 canvas):
    ///   frame[0]: solid GREEN, dispose=Keep      → canvas stays GREEN
    ///   frame[1]: solid RED,   dispose=Previous  → canvas restored to GREEN after display
    ///   frame[2]: solid RED,   dispose=Keep      → drawn onto GREEN (restored) canvas
    ///
    /// Correct reference for frame[2] = GREEN (restored canvas).
    /// RED ≠ GREEN → frame[2] must NOT be collapsed.
    ///
    /// Current (buggy) behavior:
    ///   ref = frame[1] = RED → RED == RED → incorrectly collapses frame[2].
    #[test]
    fn test_regression_previous_disposal_different_frame_not_collapsed() {
        let green = [0u8, 255, 0, 255];
        let red = [255u8, 0, 0, 255];

        let frame0 = make_canvas_frame(green, DisposalMethod::Keep);
        let frame1 = make_canvas_frame(red, DisposalMethod::Previous);
        let frame2 = make_canvas_frame(red, DisposalMethod::Keep);

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame0, frame1, frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let optimized = gif.optimize(OptLevel::O1);

        // frame[2] reference = GREEN (restored canvas after Previous disposal).
        // frame[2] is RED → RED ≠ GREEN → must NOT collapse.
        assert_ne!(
            optimized.frames[2].width, 1,
            "BUG: frame[2] was collapsed. After Previous disposal of frame[1] the \
             canvas is restored to GREEN. frame[2] is RED, which differs from GREEN \
             on every pixel — it must not be collapsed. The optimizer incorrectly \
             used frame[1] (RED) as the reference."
        );
        // All pixels must remain opaque
        for (i, chunk) in optimized.frames[2].pixels.chunks(4).enumerate() {
            assert_eq!(
                chunk[3], 255,
                "pixel {i} alpha should be 255: frame[2] (RED) differs from the \
                 restored canvas (GREEN) on every pixel"
            );
        }
    }

    /// REGRESSION: Minimal frame under Background disposal must not be silently
    /// treated as a no-op when the next frame redraws the same content.
    ///
    /// Scenario (2×2 canvas):
    ///   frame[0]: solid WHITE, dispose=Background → canvas cleared to transparent
    ///   frame[1]: solid WHITE, dispose=Background → canvas cleared to transparent
    ///   frame[2]: solid WHITE, dispose=Keep
    ///
    /// Each frame is drawn onto a transparent canvas, so each is a real change.
    /// No frame after frame[0] should be collapsed to minimal.
    ///
    /// Current (buggy) behavior: frame[1] ref = frame[0] = WHITE → collapses.
    #[test]
    fn test_regression_repeated_background_disposal_no_collapse() {
        let white = [255u8, 255, 255, 255];

        let frame0 = make_canvas_frame(white, DisposalMethod::Background);
        let frame1 = make_canvas_frame(white, DisposalMethod::Background);
        let frame2 = make_canvas_frame(white, DisposalMethod::Keep);

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame0, frame1, frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let optimized = gif.optimize(OptLevel::O1);

        // frame[1]: reference = transparent (post-Background disposal of frame[0]).
        // WHITE ≠ transparent → must NOT collapse.
        assert_ne!(
            optimized.frames[1].width, 1,
            "BUG: frame[1] collapsed. After Background disposal of frame[0] the \
             canvas is transparent. frame[1] is WHITE, which is a real change — \
             it must not be collapsed."
        );

        // frame[2]: reference = transparent (post-Background disposal of frame[1]).
        // WHITE ≠ transparent → must NOT collapse either.
        assert_ne!(
            optimized.frames[2].width, 1,
            "BUG: frame[2] collapsed. After Background disposal of frame[1] the \
             canvas is transparent. frame[2] is WHITE, which is a real change — \
             it must not be collapsed."
        );
    }

    /// Sanity check: Keep disposal still works correctly (not a regression).
    ///
    /// With Keep disposal the post-disposal canvas IS the displayed frame,
    /// so the current behavior (compare against previous displayed frame) is correct.
    /// This test must continue to pass before and after the fix.
    #[test]
    fn test_keep_disposal_optimization_still_correct() {
        let red = [255u8, 0, 0, 255];

        let frame0 = make_canvas_frame(red, DisposalMethod::Keep);
        let frame1 = make_canvas_frame(red, DisposalMethod::Keep); // identical

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame0, frame1],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let optimized = gif.optimize(OptLevel::O1);

        // frame[1] is identical to frame[0] and Keep disposal means the canvas
        // stays as-is. Collapsing to minimal is correct here.
        assert_eq!(
            optimized.frames[1].width, 1,
            "frame[1] should collapse: Keep disposal means canvas stays RED, \
             and frame[1] is also RED — no visible change"
        );
    }

    // -------------------------------------------------------------------------
    // Regression tests: subframe compositing for Keep/None disposal
    //
    // Bug: precompute_reference_canvases() stores `frame.clone()` as
    // `displayed_canvas` for Keep/None disposal. When the frame is a subframe
    // (non-zero left/top, smaller than canvas), the raw patch is NOT the full
    // displayed canvas. The next frame's reference must be the full canvas with
    // the patch composited onto it.
    //
    // These tests encode CORRECT expected behavior and are expected to FAIL
    // before the fix is applied.
    // -------------------------------------------------------------------------

    /// Build a frame with arbitrary position, size, and uniform color.
    fn make_subframe(
        left: u16,
        top: u16,
        width: u16,
        height: u16,
        color: [u8; 4],
        dispose: DisposalMethod,
    ) -> Frame {
        let pixels = color
            .iter()
            .cloned()
            .cycle()
            .take(width as usize * height as usize * 4)
            .collect();
        Frame {
            pixels,
            delay: Duration::from_millis(100),
            dispose,
            local_palette: None,
            left,
            top,
            width,
            height,
        }
    }

    /// Build a full-canvas frame (left=0, top=0) with uniform color.
    fn make_full_frame(
        canvas_w: u16,
        canvas_h: u16,
        color: [u8; 4],
        dispose: DisposalMethod,
    ) -> Frame {
        make_subframe(0, 0, canvas_w, canvas_h, color, dispose)
    }

    /// REGRESSION: Keep disposal with subframe patch — reference for next frame
    /// must be the full composited canvas, not the raw patch.
    ///
    /// Canvas: 4×4
    ///
    /// frame[0]: full 4×4 canvas, solid RED, dispose=Keep
    ///   → displayed canvas = 4×4 RED
    ///
    /// frame[1]: subframe patch 2×2 at (2,2), solid BLUE, dispose=Keep
    ///   → displayed canvas should be: 4×4 with top-left 2×2 still RED,
    ///     bottom-right 2×2 now BLUE (composite)
    ///
    /// frame[2]: full 4×4 canvas, solid RED in top-left + BLUE in bottom-right
    ///   (i.e. the correct composite), dispose=Keep
    ///
    /// Correct reference for frame[2] = the composited canvas (RED+BLUE).
    /// frame[2] matches the composite exactly → should collapse to minimal 1×1.
    ///
    /// Buggy behavior: reference = raw patch frame[1] (2×2 BLUE at offset 2,2).
    /// The reference is a 2×2 subframe, not the full 4×4 canvas, so the
    /// comparison is spatially wrong and frame[2] will NOT collapse correctly.
    #[test]
    fn test_regression_keep_subframe_reference_is_composited_canvas() {
        // Canvas 4×4
        let cw: u16 = 4;
        let ch: u16 = 4;

        let red = [255u8, 0, 0, 255];
        let blue = [0u8, 0, 255, 255];

        // frame[0]: full canvas RED, Keep
        let frame0 = make_full_frame(cw, ch, red, DisposalMethod::Keep);

        // frame[1]: 2×2 patch at (2,2) BLUE, Keep
        // After display: top-left 2×2 = RED (from frame[0]), bottom-right 2×2 = BLUE
        let frame1 = make_subframe(2, 2, 2, 2, blue, DisposalMethod::Keep);

        // frame[2]: full 4×4 canvas matching the composite (RED top-left, BLUE bottom-right)
        // Pixels laid out row-major:
        //   row 0: RED RED RED RED
        //   row 1: RED RED RED RED
        //   row 2: RED RED BLUE BLUE
        //   row 3: RED RED BLUE BLUE
        let mut composite_pixels = Vec::with_capacity(cw as usize * ch as usize * 4);
        for row in 0..ch as usize {
            for col in 0..cw as usize {
                if row >= 2 && col >= 2 {
                    composite_pixels.extend_from_slice(&blue);
                } else {
                    composite_pixels.extend_from_slice(&red);
                }
            }
        }
        let frame2 = Frame {
            pixels: composite_pixels,
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::Keep,
            local_palette: None,
            left: 0,
            top: 0,
            width: cw,
            height: ch,
        };

        let gif = Gif {
            width: cw,
            height: ch,
            global_palette: None,
            frames: vec![frame0, frame1, frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let optimized = gif.optimize(OptLevel::O1);

        // frame[2] matches the composited canvas exactly.
        // Correct reference = composited canvas → no diff → should collapse to 1×1.
        //
        // BUG: precompute_reference_canvases stores the raw 2×2 patch (frame[1])
        // as the reference. The reference is a 2×2 subframe at (2,2), not the
        // full 4×4 canvas. optimize_frame_internal sees mismatched dimensions
        // (4×4 vs 2×2) and returns frame[2] unchanged — it does NOT collapse.
        assert_eq!(
            optimized.frames[2].width, 1,
            "BUG: frame[2] should collapse to 1×1 because it matches the composited \
             canvas (RED top-left + BLUE bottom-right). The reference for frame[2] \
             must be the full composited canvas, not the raw 2×2 patch stored by \
             precompute_reference_canvases."
        );
        assert_eq!(
            optimized.frames[2].height, 1,
            "BUG: frame[2] height should be 1 after collapsing"
        );
    }

    /// REGRESSION: None disposal with subframe patch — same semantics as Keep.
    ///
    /// Canvas: 4×4
    ///
    /// frame[0]: full 4×4 canvas, solid GREEN, dispose=None
    ///   → displayed canvas = 4×4 GREEN (None = no disposal = canvas stays)
    ///
    /// frame[1]: subframe 2×2 at (0,0), solid YELLOW, dispose=None
    ///   → displayed canvas should be: top-left 2×2 YELLOW, rest GREEN (composite)
    ///
    /// frame[2]: full 4×4 canvas matching the composite, dispose=Keep
    ///
    /// Correct reference for frame[2] = composited canvas.
    /// frame[2] matches → should collapse to 1×1.
    ///
    /// Buggy behavior: reference = raw 2×2 patch (frame[1]), dimension mismatch
    /// → frame[2] returned unchanged, no collapse.
    #[test]
    fn test_regression_none_disposal_subframe_reference_is_composited_canvas() {
        let cw: u16 = 4;
        let ch: u16 = 4;

        let green = [0u8, 255, 0, 255];
        let yellow = [255u8, 255, 0, 255];

        // frame[0]: full canvas GREEN, None disposal
        let frame0 = make_full_frame(cw, ch, green, DisposalMethod::None);

        // frame[1]: 2×2 patch at (0,0) YELLOW, None disposal
        let frame1 = make_subframe(0, 0, 2, 2, yellow, DisposalMethod::None);

        // frame[2]: full 4×4 composite (top-left 2×2 YELLOW, rest GREEN)
        let mut composite_pixels = Vec::with_capacity(cw as usize * ch as usize * 4);
        for row in 0..ch as usize {
            for col in 0..cw as usize {
                if row < 2 && col < 2 {
                    composite_pixels.extend_from_slice(&yellow);
                } else {
                    composite_pixels.extend_from_slice(&green);
                }
            }
        }
        let frame2 = Frame {
            pixels: composite_pixels,
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::Keep,
            local_palette: None,
            left: 0,
            top: 0,
            width: cw,
            height: ch,
        };

        let gif = Gif {
            width: cw,
            height: ch,
            global_palette: None,
            frames: vec![frame0, frame1, frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let optimized = gif.optimize(OptLevel::O1);

        // frame[2] matches the composited canvas exactly → should collapse.
        // BUG: reference is the raw 2×2 patch, not the full 4×4 composite.
        assert_eq!(
            optimized.frames[2].width, 1,
            "BUG: frame[2] should collapse to 1×1 because it matches the composited \
             canvas (YELLOW top-left + GREEN rest). None disposal has the same \
             semantics as Keep — canvas stays as displayed. The reference must be \
             the full composited canvas, not the raw 2×2 patch."
        );
        assert_eq!(
            optimized.frames[2].height, 1,
            "BUG: frame[2] height should be 1 after collapsing"
        );
    }

    /// REGRESSION: Keep disposal subframe — pixel that differs from composite
    /// must NOT be marked transparent.
    ///
    /// Canvas: 4×4
    ///
    /// frame[0]: full 4×4 canvas, solid RED, dispose=Keep
    ///   → displayed canvas = 4×4 RED
    ///
    /// frame[1]: subframe 2×2 at (2,2), solid BLUE, dispose=Keep
    ///   → composited canvas: top-left 2×2 RED, bottom-right 2×2 BLUE
    ///
    /// frame[2]: full 4×4 canvas, solid GREEN everywhere, dispose=Keep
    ///   → GREEN differs from the composite on ALL pixels
    ///
    /// Correct reference for frame[2] = composited canvas (RED+BLUE).
    /// GREEN ≠ RED and GREEN ≠ BLUE → all pixels differ → frame[2] must be
    /// fully opaque (no pixels marked transparent).
    ///
    /// Buggy behavior: reference = raw 2×2 BLUE patch at (2,2).
    /// Dimension mismatch (4×4 vs 2×2) → optimize_frame_internal returns
    /// frame[2] unchanged (which accidentally passes the opaque check), BUT
    /// the reference is still wrong — the test documents the correct semantics.
    #[test]
    fn test_regression_keep_subframe_next_frame_differs_stays_opaque() {
        let cw: u16 = 4;
        let ch: u16 = 4;

        let red = [255u8, 0, 0, 255];
        let blue = [0u8, 0, 255, 255];
        let green = [0u8, 255, 0, 255];

        let frame0 = make_full_frame(cw, ch, red, DisposalMethod::Keep);
        let frame1 = make_subframe(2, 2, 2, 2, blue, DisposalMethod::Keep);
        let frame2 = make_full_frame(cw, ch, green, DisposalMethod::Keep);

        let gif = Gif {
            width: cw,
            height: ch,
            global_palette: None,
            frames: vec![frame0, frame1, frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let optimized = gif.optimize(OptLevel::O1);

        // frame[2] is GREEN; the composited canvas is RED+BLUE.
        // Every pixel differs → frame[2] must remain fully opaque.
        let opt2 = &optimized.frames[2];
        assert_ne!(
            opt2.width, 1,
            "frame[2] must NOT collapse: GREEN differs from the composited canvas \
             (RED top-left + BLUE bottom-right) on every pixel"
        );
        for (i, chunk) in opt2.pixels.chunks(4).enumerate() {
            assert_eq!(
                chunk[3], 255,
                "pixel {i} alpha must be 255: frame[2] (GREEN) differs from the \
                 composited reference on every pixel"
            );
        }
    }

    /// SANITY: Full-canvas Keep disposal — behavior unchanged by fix.
    ///
    /// When all frames are full-canvas (left=0, top=0, same dimensions),
    /// the composited canvas IS the frame itself, so the fix must not change
    /// the existing correct behavior.
    ///
    /// frame[0]: full 4×4 RED, Keep
    /// frame[1]: full 4×4 RED, Keep  → identical → should collapse to 1×1
    /// frame[2]: full 4×4 BLUE, Keep → differs   → must NOT collapse
    #[test]
    fn test_sanity_full_canvas_keep_disposal_unchanged() {
        let cw: u16 = 4;
        let ch: u16 = 4;

        let red = [255u8, 0, 0, 255];
        let blue = [0u8, 0, 255, 255];

        let frame0 = make_full_frame(cw, ch, red, DisposalMethod::Keep);
        let frame1 = make_full_frame(cw, ch, red, DisposalMethod::Keep);
        let frame2 = make_full_frame(cw, ch, blue, DisposalMethod::Keep);

        let gif = Gif {
            width: cw,
            height: ch,
            global_palette: None,
            frames: vec![frame0, frame1, frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let optimized = gif.optimize(OptLevel::O1);

        // frame[1] identical to frame[0] → collapse
        assert_eq!(
            optimized.frames[1].width, 1,
            "frame[1] should collapse: full-canvas RED identical to reference RED"
        );

        // frame[2] differs from frame[1] (BLUE vs RED) → must NOT collapse
        assert_ne!(
            optimized.frames[2].width, 1,
            "frame[2] must NOT collapse: BLUE differs from RED reference"
        );
        for (i, chunk) in optimized.frames[2].pixels.chunks(4).enumerate() {
            assert_eq!(
                chunk[3], 255,
                "pixel {i} alpha must be 255: frame[2] (BLUE) differs from reference (RED)"
            );
        }
    }

    /// REGRESSION: precompute_reference_canvases direct unit test for Keep subframe.
    ///
    /// Directly tests the function that is buggy. After frame[1] (a 2×2 subframe
    /// at (2,2) with Keep disposal), the reference for frame[2] must be a full
    /// 4×4 canvas with the patch composited in, NOT the raw 2×2 patch.
    ///
    /// Specifically:
    ///   - reference_canvases[2].width  must equal canvas width (4)
    ///   - reference_canvases[2].height must equal canvas height (4)
    ///   - reference_canvases[2].left   must be 0
    ///   - reference_canvases[2].top    must be 0
    ///   - Pixels at (2,2)–(3,3) must be BLUE (from the patch)
    ///   - Pixels at (0,0)–(1,1) must be RED  (from frame[0])
    #[test]
    fn test_regression_precompute_reference_canvases_keep_subframe_direct() {
        let cw: u16 = 4;
        let ch: u16 = 4;

        let red = [255u8, 0, 0, 255];
        let blue = [0u8, 0, 255, 255];

        let frame0 = make_full_frame(cw, ch, red, DisposalMethod::Keep);
        let frame1 = make_subframe(2, 2, 2, 2, blue, DisposalMethod::Keep);
        // frame[2] is a placeholder — we only care about the reference computed for it
        let frame2 = make_full_frame(cw, ch, red, DisposalMethod::Keep);

        let frames = vec![frame0, frame1, frame2];
        let refs = precompute_reference_canvases(&frames, cw, ch);

        // refs[2] = canvas state before frame[2] is drawn
        // = frame[0] (RED) with frame[1] patch (BLUE at 2,2) composited on top
        let ref2 = &refs[2];

        assert_eq!(
            ref2.width, cw,
            "BUG: reference canvas width is {} but should be {} (full canvas). \
             precompute_reference_canvases stored the raw 2×2 patch instead of \
             compositing it onto the full canvas.",
            ref2.width, cw
        );
        assert_eq!(
            ref2.height, ch,
            "BUG: reference canvas height is {} but should be {} (full canvas).",
            ref2.height, ch
        );
        assert_eq!(
            ref2.left, 0,
            "BUG: reference canvas left offset is {} but should be 0.",
            ref2.left
        );
        assert_eq!(
            ref2.top, 0,
            "BUG: reference canvas top offset is {} but should be 0.",
            ref2.top
        );

        // Verify pixel content: top-left 2×2 should be RED (from frame[0])
        for row in 0..2usize {
            for col in 0..2usize {
                let idx = (row * cw as usize + col) * 4;
                assert_eq!(
                    &ref2.pixels[idx..idx + 4],
                    &red,
                    "pixel ({col},{row}) should be RED (from frame[0], not overwritten by patch)"
                );
            }
        }

        // Bottom-right 2×2 should be BLUE (from frame[1] patch)
        for row in 2..4usize {
            for col in 2..4usize {
                let idx = (row * cw as usize + col) * 4;
                assert_eq!(
                    &ref2.pixels[idx..idx + 4],
                    &blue,
                    "pixel ({col},{row}) should be BLUE (composited from frame[1] patch)"
                );
            }
        }
    }

    // =========================================================================
    // Voyager-class regression tests: O3 opaque-delta / global-palette damage
    //
    // These tests encode the semantic problem isolated by the voyager experiment
    // matrix (2026-04-20).  The root cause is that `optimize(O3)` bundles two
    // behaviors that are individually harmless but destructive together:
    //
    //   (a) perceptual thresholding (threshold=8) — silently discards color info
    //   (b) bounding-box crop (crop_to_bbox=true) — creates subframes that force
    //       the encoder to re-quantize without full-canvas color context
    //
    // Contrast with O1 (threshold=0, no crop) and O2 (threshold=2, no crop):
    //   O1: BA 0.00 — lossless, no crop
    //   O2: BA 1.16 — mild threshold, no crop
    //   O3: BA 26.74 — threshold=8 AND crop, catastrophic on global-palette GIFs
    //
    // The tests below verify the *pixel-level* semantics that O3 violates.
    // They do NOT test encode quality (that requires a full round-trip), but they
    // document exactly which pixels O3 corrupts vs O1/O2 on the same input.
    // =========================================================================

    /// Build a synthetic opaque-delta sequence: a sequence of full-canvas frames
    /// where each frame differs from the previous by a small opaque patch.
    ///
    /// This is the voyager-class pattern: all pixels are fully opaque (alpha=255),
    /// the diff between consecutive frames is a small region, and the GIF would
    /// normally use a global palette.  The encoder must see the full canvas to
    /// quantize correctly.
    fn make_opaque_delta_gif(
        canvas_w: u16,
        canvas_h: u16,
        base_color: [u8; 4],
        patch_color: [u8; 4],
        patch_left: u16,
        patch_top: u16,
        patch_w: u16,
        patch_h: u16,
    ) -> Gif {
        // frame[0]: full canvas, base_color everywhere
        let frame0 = make_full_frame(canvas_w, canvas_h, base_color, DisposalMethod::Keep);

        // frame[1]: full canvas, base_color everywhere except patch region = patch_color
        let mut pixels1 = vec![0u8; canvas_w as usize * canvas_h as usize * 4];
        for y in 0..canvas_h as usize {
            for x in 0..canvas_w as usize {
                let idx = (y * canvas_w as usize + x) * 4;
                let in_patch = x >= patch_left as usize
                    && x < (patch_left + patch_w) as usize
                    && y >= patch_top as usize
                    && y < (patch_top + patch_h) as usize;
                let color = if in_patch { patch_color } else { base_color };
                pixels1[idx..idx + 4].copy_from_slice(&color);
            }
        }
        let frame1 = Frame {
            pixels: pixels1,
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::Keep,
            local_palette: None,
            left: 0,
            top: 0,
            width: canvas_w,
            height: canvas_h,
        };

        Gif {
            width: canvas_w,
            height: canvas_h,
            global_palette: None,
            frames: vec![frame0, frame1],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        }
    }

    /// VOYAGER REGRESSION: O3 introduces transparency into opaque-delta sequences.
    ///
    /// O3 is now lossless: marks only exact-match pixels transparent.
    ///
    /// After the fix (2026-04-20), O3 uses threshold=0 (exact match only).
    /// Pixels with diff > 0 are always kept opaque, preserving color information.
    ///
    /// O1 (threshold=0) and O3 (threshold=0) now have identical thresholding behavior.
    /// The difference is that O3 crops to the bounding box while O1 does not.
    ///
    /// This test verifies the FIXED behavior where O3 is lossless.
    #[test]
    fn test_voyager_o3_marks_near_identical_opaque_pixels_transparent() {
        // Canvas 8×8, base=gray(128), patch=gray(134) — diff=6 per channel
        let base = [128u8, 128, 128, 255];
        let patch = [134u8, 134, 134, 255]; // diff=6, now preserved by both O1 and O3

        let gif = make_opaque_delta_gif(8, 8, base, patch, 2, 2, 4, 4);

        let o1 = gif.clone().optimize(OptLevel::O1);
        let o3 = gif.optimize(OptLevel::O3);

        // O1: patch pixels (diff=6 > threshold=0) must remain opaque in frame[1]
        let o1_frame1 = &o1.frames[1];
        // O1 does not crop, so frame[1] is still full-canvas
        assert_eq!(
            o1_frame1.width, 8,
            "O1 must not crop: frame[1] should remain full-canvas width"
        );
        assert_eq!(
            o1_frame1.height, 8,
            "O1 must not crop: frame[1] should remain full-canvas height"
        );
        // Patch pixel at (2,2): must be opaque under O1 (diff=6 > threshold=0)
        let patch_idx = (2 * 8 + 2) * 4;
        assert_eq!(
            o1_frame1.pixels[patch_idx + 3], 255,
            "O1: patch pixel at (2,2) must be opaque — diff=6 exceeds threshold=0"
        );
        // Non-patch pixel at (0,0): must be transparent under O1 (identical to prev)
        assert_eq!(
            o1_frame1.pixels[3], 0,
            "O1: non-patch pixel at (0,0) must be transparent — identical to prev frame"
        );

        // O3: crops to the diff bounding box (patch region only)
        // The resulting subframe covers only the patch area
        assert!(
            o3.frames[1].width <= 4 && o3.frames[1].height <= 4,
            "O3 must crop frame[1] to the diff bounding box (patch region ≤ 4×4), \
             got {}×{}",
            o3.frames[1].width,
            o3.frames[1].height
        );

        // O3 with threshold=0 (fixed): patch pixels have diff=6 > 0, so they are kept opaque.
        // The color [134,134,134] is preserved. This is the lossless behavior.
        let o3_frame1 = &o3.frames[1];
        let all_opaque = o3_frame1.pixels.chunks(4).all(|p| p[3] == 255);
        assert!(
            all_opaque,
            "O3 (fixed): all patch pixels (diff=6) must be opaque because threshold=0. \
             This verifies the lossless behavior where O3 preserves color information."
        );
    }

    /// O2 and O3 now have identical thresholding behavior (both lossless).
    ///
    /// After the fix (2026-04-20), both O2 and O3 use threshold=0 (exact match only).
    /// The difference is that O3 crops to the bounding box while O2 does not.
    ///
    /// This test verifies that both preserve patch pixels with diff > 0.
    #[test]
    fn test_voyager_o2_less_aggressive_than_o3_on_opaque_delta() {
        // diff=6 per channel: preserved by both O2 and O3 (both now use threshold=0)
        let base = [100u8, 100, 100, 255];
        let patch = [106u8, 106, 106, 255]; // diff=6

        let gif = make_opaque_delta_gif(8, 8, base, patch, 0, 0, 4, 4);

        let o2 = gif.clone().optimize(OptLevel::O2);
        let o3 = gif.optimize(OptLevel::O3);

        // O2: threshold=0, diff=6 > 0 → patch pixels must remain opaque
        // O2 does not crop, so frame[1] is full-canvas
        let o2_frame1 = &o2.frames[1];
        assert_eq!(
            o2_frame1.width, 8,
            "O2 must not crop: frame[1] should remain full-canvas"
        );
        // Patch pixel at (0,0): diff=6 > threshold=0 → must be opaque
        assert_eq!(
            o2_frame1.pixels[3], 255,
            "O2: patch pixel at (0,0) must be opaque — diff=6 exceeds O2 threshold=0"
        );

        // O3: threshold=0, diff=6 > 0 → patch pixels must remain opaque
        // O3 crops, so frame[1] is a subframe covering the patch
        let o3_frame1 = &o3.frames[1];
        // All pixels in the O3 subframe should be opaque (diff=6 > threshold=0)
        let o3_all_opaque = o3_frame1.pixels.chunks(4).all(|p| p[3] == 255);
        assert!(
            o3_all_opaque,
            "O3 (fixed): patch pixels (diff=6) must be opaque because threshold=0. \
             Both O2 and O3 now preserve color information."
        );

        // Verify both preserve the same color information (just different frame sizes)
        let o2_opaque_count = o2_frame1.pixels.chunks(4).filter(|p| p[3] == 255).count();
        let o3_opaque_count = o3_frame1.pixels.chunks(4).filter(|p| p[3] == 255).count();
        // O2 has full canvas (64 pixels), O3 has subframe (16 pixels), but both preserve patch
        assert!(
            o3_opaque_count > 0,
            "O3 must preserve opaque pixels in the patch region: \
             O2 opaque={o2_opaque_count}, O3 opaque={o3_opaque_count}"
        );
    }

    /// VOYAGER REGRESSION: O3 crop creates subframes; O1/O2 do not.
    ///
    /// The bounding-box crop in O3 is the second half of the damage mechanism.
    /// Even with threshold=0 (hypothetically), cropping to a subframe forces the
    /// encoder to re-quantize only the subframe pixels, losing global palette context.
    ///
    /// This test verifies that O1 and O2 never crop (frame dimensions unchanged),
    /// while O3 always crops when there is a localized diff.
    #[test]
    fn test_voyager_o3_crops_o1_o2_do_not() {
        // Large canvas with a tiny 2×2 patch diff — maximizes crop impact
        let base = [200u8, 100, 50, 255];
        let patch = [50u8, 200, 100, 255]; // clearly different, diff >> threshold

        let gif = make_opaque_delta_gif(16, 16, base, patch, 7, 7, 2, 2);

        let o1 = gif.clone().optimize(OptLevel::O1);
        let o2 = gif.clone().optimize(OptLevel::O2);
        let o3 = gif.optimize(OptLevel::O3);

        // O1: no crop — frame[1] must remain 16×16
        assert_eq!(
            o1.frames[1].width, 16,
            "O1 must not crop: frame[1] width must remain 16"
        );
        assert_eq!(
            o1.frames[1].height, 16,
            "O1 must not crop: frame[1] height must remain 16"
        );

        // O2: no crop — frame[1] must remain 16×16
        assert_eq!(
            o2.frames[1].width, 16,
            "O2 must not crop: frame[1] width must remain 16"
        );
        assert_eq!(
            o2.frames[1].height, 16,
            "O2 must not crop: frame[1] height must remain 16"
        );

        // O3: crops to diff bounding box — frame[1] must be smaller than 16×16
        assert!(
            o3.frames[1].width < 16 || o3.frames[1].height < 16,
            "O3 must crop frame[1] to the diff bounding box (2×2 patch), \
             got {}×{}",
            o3.frames[1].width,
            o3.frames[1].height
        );
        // The crop should be tight around the 2×2 patch
        assert!(
            o3.frames[1].width <= 2 && o3.frames[1].height <= 2,
            "O3 crop should be tight around the 2×2 patch, got {}×{}",
            o3.frames[1].width,
            o3.frames[1].height
        );
    }

    /// VOYAGER REGRESSION: O3 crop position is correct (left/top offsets).
    ///
    /// When O3 crops to the diff bounding box, the resulting subframe must have
    /// the correct left/top offsets so the decoder places it at the right position.
    /// This tests that the crop geometry is correct, not just the size.
    #[test]
    fn test_voyager_o3_crop_position_correct() {
        // 10×10 canvas, patch at (4,3) size 3×2
        let base = [80u8, 80, 80, 255];
        let patch = [200u8, 50, 50, 255];

        let gif = make_opaque_delta_gif(10, 10, base, patch, 4, 3, 3, 2);
        let o3 = gif.optimize(OptLevel::O3);

        let f1 = &o3.frames[1];
        assert_eq!(
            f1.left, 4,
            "O3 subframe left must be 4 (patch left), got {}",
            f1.left
        );
        assert_eq!(
            f1.top, 3,
            "O3 subframe top must be 3 (patch top), got {}",
            f1.top
        );
        assert_eq!(
            f1.width, 3,
            "O3 subframe width must be 3 (patch width), got {}",
            f1.width
        );
        assert_eq!(
            f1.height, 2,
            "O3 subframe height must be 2 (patch height), got {}",
            f1.height
        );
    }

    /// VOYAGER REGRESSION: O3 subframe disposal is preserved.
    ///
    /// The subframe produced by O3 must retain the original disposal method.
    /// If the source frame had DisposalMethod::Keep, the subframe must also have
    /// DisposalMethod::Keep — otherwise the decoder will misinterpret the sequence.
    #[test]
    fn test_voyager_o3_subframe_preserves_keep_disposal() {
        let base = [60u8, 120, 180, 255];
        let patch = [180u8, 60, 120, 255];

        let gif = make_opaque_delta_gif(8, 8, base, patch, 1, 1, 3, 3);
        let o3 = gif.optimize(OptLevel::O3);

        assert_eq!(
            o3.frames[1].dispose,
            DisposalMethod::Keep,
            "O3 subframe must preserve DisposalMethod::Keep from the source frame"
        );
    }

    /// O1 and O3 now preserve exact pixel values (both lossless).
    ///
    /// After the fix (2026-04-20), both O1 and O3 use threshold=0 (exact match only).
    /// Both preserve the color of any pixel that differs from the previous frame.
    ///
    /// Scenario: patch pixels have diff=5 per channel.
    ///   O1: diff=5 > threshold=0 → pixel kept opaque with original color
    ///   O3: diff=5 > threshold=0 → pixel kept opaque with original color
    ///
    /// The difference is that O3 crops to the bounding box while O1 does not.
    #[test]
    fn test_voyager_o1_preserves_exact_pixel_values_o3_does_not() {
        let base = [100u8, 100, 100, 255];
        let patch = [105u8, 105, 105, 255]; // diff=5 per channel

        let gif = make_opaque_delta_gif(4, 4, base, patch, 0, 0, 2, 2);

        let o1 = gif.clone().optimize(OptLevel::O1);
        let o3 = gif.optimize(OptLevel::O3);

        // O1: patch pixel at (0,0) must retain its exact color [105,105,105,255]
        let o1_f1 = &o1.frames[1];
        assert_eq!(
            o1_f1.pixels[3], 255,
            "O1: patch pixel at (0,0) must be opaque (diff=5 > threshold=0)"
        );
        assert_eq!(
            &o1_f1.pixels[0..3],
            &[105u8, 105, 105],
            "O1: patch pixel at (0,0) must retain exact color [105,105,105]"
        );

        // O3 (fixed): patch pixel at (0,0) is in the subframe; with threshold=0 and diff=5,
        // it is kept opaque with the original color [105,105,105]
        let o3_f1 = &o3.frames[1];
        // The subframe should be at (0,0) covering the patch
        assert_eq!(
            o3_f1.pixels[3], 255,
            "O3 (fixed): patch pixel (diff=5) must be opaque because threshold=0. \
             Color information is preserved."
        );
        assert_eq!(
            &o3_f1.pixels[0..3],
            &[105u8, 105, 105],
            "O3 (fixed): patch pixel must retain exact color [105,105,105]"
        );
    }

    /// VOYAGER REGRESSION: three-frame opaque-delta sequence.
    ///
    /// Extends the two-frame case to three frames to verify that the damage
    /// accumulates correctly and that O1 remains clean throughout.
    ///
    /// frame[0]: full canvas, gray(100)
    /// frame[1]: full canvas, gray(100) except patch at (2,2) = gray(106) — diff=6
    /// frame[2]: full canvas, gray(100) everywhere (back to base)
    ///
    /// O1: frame[1] patch pixels opaque, frame[2] patch pixels opaque (changed back)
    /// O3: frame[1] patch pixels transparent (diff=6 ≤ 8), frame[2] behavior depends
    ///     on what O3 thinks the reference is after the subframe
    #[test]
    fn test_voyager_three_frame_opaque_delta_o1_clean() {
        let cw: u16 = 8;
        let ch: u16 = 8;
        let base = [100u8, 100, 100, 255];
        let patch_color = [106u8, 106, 106, 255]; // diff=6

        // frame[0]: all base
        let frame0 = make_full_frame(cw, ch, base, DisposalMethod::Keep);

        // frame[1]: base everywhere except 2×2 patch at (2,2) = patch_color
        let mut pixels1 = vec![0u8; cw as usize * ch as usize * 4];
        for y in 0..ch as usize {
            for x in 0..cw as usize {
                let idx = (y * cw as usize + x) * 4;
                let in_patch = x >= 2 && x < 4 && y >= 2 && y < 4;
                let color = if in_patch { patch_color } else { base };
                pixels1[idx..idx + 4].copy_from_slice(&color);
            }
        }
        let frame1 = Frame {
            pixels: pixels1,
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::Keep,
            local_palette: None,
            left: 0,
            top: 0,
            width: cw,
            height: ch,
        };

        // frame[2]: all base (back to original)
        let frame2 = make_full_frame(cw, ch, base, DisposalMethod::Keep);

        let gif = Gif {
            width: cw,
            height: ch,
            global_palette: None,
            frames: vec![frame0, frame1, frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let o1 = gif.optimize(OptLevel::O1);

        // O1 frame[1]: patch pixels (diff=6 > 0) must be opaque
        let o1_f1 = &o1.frames[1];
        assert_eq!(
            o1_f1.width, cw,
            "O1 frame[1] must not be cropped"
        );
        let patch_idx = (2 * cw as usize + 2) * 4;
        assert_eq!(
            o1_f1.pixels[patch_idx + 3], 255,
            "O1 frame[1]: patch pixel at (2,2) must be opaque (diff=6 > threshold=0)"
        );

        // O1 frame[2]: patch pixels changed back to base (diff=6 from frame[1])
        // Reference for frame[2] = frame[1] displayed canvas (patch_color at patch region)
        // frame[2] patch pixels = base, which differs from patch_color by 6 → opaque
        let o1_f2 = &o1.frames[2];
        assert_eq!(
            o1_f2.width, cw,
            "O1 frame[2] must not be cropped"
        );
        assert_eq!(
            o1_f2.pixels[patch_idx + 3], 255,
            "O1 frame[2]: patch pixel at (2,2) must be opaque (changed back from patch_color)"
        );

        // Non-patch pixels in frame[2] must be transparent (identical to frame[1] base)
        let non_patch_idx = 0; // pixel (0,0) is not in patch
        assert_eq!(
            o1_f2.pixels[non_patch_idx + 3], 0,
            "O1 frame[2]: non-patch pixel at (0,0) must be transparent (unchanged)"
        );
    }

    /// REGRESSION: precompute_reference_canvases direct unit test for None subframe.
    ///
    /// None disposal has identical semantics to Keep — canvas stays as displayed.
    /// After a None-disposal subframe, the reference for the next frame must be
    /// the full composited canvas.
    #[test]
    fn test_regression_precompute_reference_canvases_none_subframe_direct() {
        let cw: u16 = 4;
        let ch: u16 = 4;

        let green = [0u8, 255, 0, 255];
        let yellow = [255u8, 255, 0, 255];

        let frame0 = make_full_frame(cw, ch, green, DisposalMethod::None);
        // 2×2 patch at (1,1) with None disposal
        let frame1 = make_subframe(1, 1, 2, 2, yellow, DisposalMethod::None);
        let frame2 = make_full_frame(cw, ch, green, DisposalMethod::Keep);

        let frames = vec![frame0, frame1, frame2];
        let refs = precompute_reference_canvases(&frames, cw, ch);

        let ref2 = &refs[2];

        assert_eq!(
            ref2.width, cw,
            "BUG: reference canvas width is {} but should be {} (full canvas). \
             None disposal has same semantics as Keep — canvas stays as displayed. \
             The raw 2×2 patch must be composited onto the full canvas.",
            ref2.width, cw
        );
        assert_eq!(
            ref2.height, ch,
            "BUG: reference canvas height is {} but should be {}.",
            ref2.height, ch
        );

        // Pixel at (1,1) should be YELLOW (from patch)
        let idx_11 = (1 * cw as usize + 1) * 4;
        assert_eq!(
            &ref2.pixels[idx_11..idx_11 + 4],
            &yellow,
            "pixel (1,1) should be YELLOW (composited from None-disposal patch)"
        );

        // Pixel at (0,0) should be GREEN (not overwritten by patch)
        let idx_00 = 0;
        assert_eq!(
            &ref2.pixels[idx_00..idx_00 + 4],
            &green,
            "pixel (0,0) should be GREEN (not covered by patch at (1,1))"
        );
    }
}

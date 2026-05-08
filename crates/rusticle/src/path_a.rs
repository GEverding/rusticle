//! Path A: Conservative opaque-delta reconstruction from resized displayed canvases.
//!
//! **STATUS**: Research / Future Opt-In (part of two-path routing, not current mainline product path)
//!
//! Path A is for GIF sequences that are structurally close to optimal GIF-native streams:
//! - No transparent GCEs
//! - Stable/global palette
//! - Many offset subframes
//! - Mostly Keep/None disposal
//! - Low changed-area ratio
//!
//! # Core Algorithm
//!
//! 1. Input: sequence of resized full displayed canvases (RGBA, already composited correctly).
//! 2. For each frame after the first, compute the exact changed bbox between consecutive displayed canvases.
//! 3. Emit one of:
//!    - Exact opaque bbox patch (preferred) if bbox area ≤ threshold fraction of canvas
//!    - Full frame fallback when bbox is too large or patch form is unsafe
//!    - Safe minimal/no-op only when semantically valid
//! 4. Preserve `None/Keep` semantics.
//! 5. **Do not** introduce synthetic transparency thresholding as an optimization strategy.
//!    Transparency is only allowed where it already exists structurally in the candidate/materialized data.
//!
//! # What It Was Trying to Solve
//!
//! The two-path system explored whether routing opaque-delta GIFs to a specialized conservative
//! strategy could improve compression. Path A tests that hypothesis: exact bbox patches with
//! stable palette for already-optimized sequences.
//!
//! # Structural Assumptions
//!
//! - Input is resized displayed canvases (RGBA, already composited correctly)
//! - Output is opaque pixel data (no synthetic transparency)
//! - Bbox patches are computed exactly (no thresholding or fallback logic in core algorithm)
//! - Frame timing and disposal are preserved exactly
//! - Bbox area threshold (default 0.7) determines when to fall back to full-frame
//!
//! # Latest Evidence
//!
//! Path A works well for already-optimized opaque-delta sequences, but generality is not established.
//! Validation on a larger, more diverse GIF corpus is needed before promoting to mainline.
//!
//! See `docs/RESEARCH_VOYAGER_AND_TWO_PATH.md` for full context.
//!
//! # Thresholds
//!
//! - **Bbox area threshold**: 0.7 (70% of canvas area)
//!   If changed bbox area exceeds this, emit full frame instead of patch.
//! - **Identical frame handling**: Emit 1x1 pixel patch at (0,0) or skip-frame marker.

use crate::adaptive_ir::{BoundingBox, Canvas};
use crate::error::Result;
use crate::types::DisposalMethod;
use std::time::Duration;

/// Configuration for Path A optimization.
#[derive(Debug, Clone, Copy)]
pub struct PathAConfig {
    /// Threshold for bbox area as fraction of canvas (0.0 to 1.0).
    /// If changed bbox area exceeds this, emit full frame instead of patch.
    pub bbox_area_threshold: f32,
}

impl Default for PathAConfig {
    fn default() -> Self {
        Self {
            bbox_area_threshold: 0.7,
        }
    }
}

/// Result of Path A optimization for a single frame.
#[derive(Debug, Clone)]
pub struct PathAFrame {
    /// RGBA pixel data (opaque only, no synthetic transparency).
    pub pixels: Vec<u8>,
    /// Horizontal offset on canvas.
    pub left: u16,
    /// Vertical offset on canvas.
    pub top: u16,
    /// Width of the frame.
    pub width: u16,
    /// Height of the frame.
    pub height: u16,
    /// Delay before displaying the next frame.
    pub delay: Duration,
    /// Disposal method (always None/Keep for Path A).
    pub dispose: DisposalMethod,
}

/// Compute the exact changed bounding box between two canvases.
///
/// Returns the minimal bounding box of pixels that differ between the two canvases.
/// If canvases are identical, returns an empty bbox (0, 0, 0, 0).
fn compute_changed_bbox(prev_canvas: &Canvas, curr_canvas: &Canvas) -> BoundingBox {
    debug_assert_eq!(prev_canvas.width, curr_canvas.width);
    debug_assert_eq!(prev_canvas.height, curr_canvas.height);

    let width = prev_canvas.width as usize;
    let height = prev_canvas.height as usize;

    let mut min_x = width as u16;
    let mut min_y = height as u16;
    let mut max_x = 0u16;
    let mut max_y = 0u16;
    let mut found_change = false;

    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) * 4;
            let prev_pixel = &prev_canvas.pixels[idx..idx + 4];
            let curr_pixel = &curr_canvas.pixels[idx..idx + 4];

            if prev_pixel != curr_pixel {
                found_change = true;
                min_x = min_x.min(x as u16);
                min_y = min_y.min(y as u16);
                max_x = max_x.max((x + 1) as u16);
                max_y = max_y.max((y + 1) as u16);
            }
        }
    }

    if found_change {
        BoundingBox::new(min_x, min_y, max_x, max_y)
    } else {
        BoundingBox::new(0, 0, 0, 0)
    }
}

/// Extract a bbox region from a canvas as opaque RGBA pixels.
///
/// Copies pixels from the bbox region of the canvas into a new buffer.
/// All pixels are opaque (alpha = 255) — no synthetic transparency is introduced.
fn extract_bbox_region(canvas: &Canvas, bbox: BoundingBox) -> Vec<u8> {
    let width = bbox.width() as usize;
    let height = bbox.height() as usize;
    let mut pixels = Vec::with_capacity(width * height * 4);

    for y in 0..height {
        for x in 0..width {
            let canvas_x = bbox.left as usize + x;
            let canvas_y = bbox.top as usize + y;
            let idx = (canvas_y * (canvas.width as usize) + canvas_x) * 4;

            // Copy pixel as-is from canvas (preserving alpha)
            pixels.extend_from_slice(&canvas.pixels[idx..idx + 4]);
        }
    }

    pixels
}

/// Optimize a sequence of resized displayed canvases using Path A.
///
/// # Arguments
///
/// * `canvases` - Sequence of resized full displayed canvases (RGBA, already composited).
/// * `delays` - Frame delays (must match canvases length).
/// * `config` - Path A configuration (thresholds, etc.).
///
/// # Returns
///
/// A vector of `PathAFrame` with exact opaque bbox patches or full-frame fallbacks.
///
/// # Invariants
///
/// - Frame 0 is always full-frame.
/// - Each subsequent frame is either:
///   - Exact opaque bbox patch (if changed area ≤ threshold)
///   - Full frame fallback (if changed area > threshold)
///   - 1x1 minimal patch at (0,0) (if frames are identical)
/// - No synthetic transparency is introduced.
/// - All disposal methods are `None` (keep semantics).
pub fn optimize_path_a(
    canvases: &[Canvas],
    delays: &[Duration],
    config: PathAConfig,
) -> Result<Vec<PathAFrame>> {
    if canvases.is_empty() {
        return Ok(Vec::new());
    }

    debug_assert_eq!(canvases.len(), delays.len());

    let mut result = Vec::new();
    let canvas_area = (canvases[0].width as usize) * (canvases[0].height as usize);
    let bbox_area_threshold = (canvas_area as f32 * config.bbox_area_threshold) as usize;

    // Frame 0: always full-frame
    let frame0 = PathAFrame {
        pixels: canvases[0].pixels.clone(),
        left: 0,
        top: 0,
        width: canvases[0].width,
        height: canvases[0].height,
        delay: delays[0],
        dispose: DisposalMethod::None,
    };
    result.push(frame0);

    // Subsequent frames: compute changed bbox and decide representation
    for i in 1..canvases.len() {
        let prev_canvas = &canvases[i - 1];
        let curr_canvas = &canvases[i];

        let changed_bbox = compute_changed_bbox(prev_canvas, curr_canvas);

        let frame = if changed_bbox.area() == 0 {
            // Identical frames: emit 1x1 minimal patch at (0,0) using the actual canvas pixel
            // This preserves the displayed canvas semantics without introducing synthetic changes.
            let pixel_at_origin = curr_canvas.pixels[0..4].to_vec();
            PathAFrame {
                pixels: pixel_at_origin,
                left: 0,
                top: 0,
                width: 1,
                height: 1,
                delay: delays[i],
                dispose: DisposalMethod::None,
            }
        } else if changed_bbox.area() <= bbox_area_threshold {
            // Changed area is small: emit exact opaque bbox patch
            let pixels = extract_bbox_region(curr_canvas, changed_bbox);
            PathAFrame {
                pixels,
                left: changed_bbox.left,
                top: changed_bbox.top,
                width: changed_bbox.width(),
                height: changed_bbox.height(),
                delay: delays[i],
                dispose: DisposalMethod::None,
            }
        } else {
            // Changed area is large: emit full-frame fallback
            PathAFrame {
                pixels: curr_canvas.pixels.clone(),
                left: 0,
                top: 0,
                width: curr_canvas.width,
                height: curr_canvas.height,
                delay: delays[i],
                dispose: DisposalMethod::None,
            }
        };

        result.push(frame);
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a test canvas with a specific color.
    fn create_solid_canvas(width: u16, height: u16, r: u8, g: u8, b: u8) -> Canvas {
        let size = (width as usize) * (height as usize) * 4;
        let mut pixels = vec![0u8; size];
        for chunk in pixels.chunks_exact_mut(4) {
            chunk[0] = r;
            chunk[1] = g;
            chunk[2] = b;
            chunk[3] = 255; // opaque
        }
        Canvas {
            pixels,
            width,
            height,
        }
    }

    /// Create a test canvas with a colored rectangle.
    fn create_canvas_with_rect(
        width: u16,
        height: u16,
        bg_r: u8,
        bg_g: u8,
        bg_b: u8,
        rect_left: u16,
        rect_top: u16,
        rect_width: u16,
        rect_height: u16,
        rect_r: u8,
        rect_g: u8,
        rect_b: u8,
    ) -> Canvas {
        let mut canvas = create_solid_canvas(width, height, bg_r, bg_g, bg_b);

        for y in 0..rect_height {
            for x in 0..rect_width {
                let canvas_x = rect_left as usize + x as usize;
                let canvas_y = rect_top as usize + y as usize;
                if canvas_x < width as usize && canvas_y < height as usize {
                    let idx = (canvas_y * (width as usize) + canvas_x) * 4;
                    canvas.pixels[idx] = rect_r;
                    canvas.pixels[idx + 1] = rect_g;
                    canvas.pixels[idx + 2] = rect_b;
                    canvas.pixels[idx + 3] = 255;
                }
            }
        }

        canvas
    }

    #[test]
    fn test_compute_changed_bbox_identical() {
        let canvas = create_solid_canvas(100, 100, 255, 0, 0);
        let bbox = compute_changed_bbox(&canvas, &canvas);
        assert_eq!(bbox.area(), 0, "Identical canvases should have empty bbox");
    }

    #[test]
    fn test_compute_changed_bbox_small_change() {
        let canvas1 = create_solid_canvas(100, 100, 255, 0, 0);
        let canvas2 = create_canvas_with_rect(100, 100, 255, 0, 0, 10, 10, 20, 20, 0, 255, 0);

        let bbox = compute_changed_bbox(&canvas1, &canvas2);
        assert_eq!(bbox.left, 10);
        assert_eq!(bbox.top, 10);
        assert_eq!(bbox.right, 30);
        assert_eq!(bbox.bottom, 30);
        assert_eq!(bbox.area(), 400);
    }

    #[test]
    fn test_voyager_like_opaque_delta() {
        // Simulate a voyager-like sequence: small offset changes
        let config = PathAConfig::default();
        let width = 640u16;
        let height = 480u16;

        // Frame 0: full canvas
        let frame0 = create_solid_canvas(width, height, 200, 200, 200);

        // Frame 1: small change in top-left corner
        let frame1 =
            create_canvas_with_rect(width, height, 200, 200, 200, 0, 0, 50, 50, 100, 100, 100);

        // Frame 2: same as frame 1 (no change from frame 1)
        let frame2 = frame1.clone_canvas();

        let canvases = vec![frame0, frame1, frame2];
        let delays = vec![Duration::from_millis(100); 3];

        let result = optimize_path_a(&canvases, &delays, config).expect("optimize_path_a failed");

        assert_eq!(result.len(), 3, "Should have 3 frames");

        // Frame 0 should be full-frame
        assert_eq!(result[0].left, 0);
        assert_eq!(result[0].top, 0);
        assert_eq!(result[0].width, width);
        assert_eq!(result[0].height, height);

        // Frame 1 should be bbox patch (50x50 at 0,0)
        assert_eq!(result[1].left, 0);
        assert_eq!(result[1].top, 0);
        assert_eq!(result[1].width, 50);
        assert_eq!(result[1].height, 50);

        // Frame 2 should be 1x1 minimal patch (identical to frame 1)
        assert_eq!(result[2].width, 1);
        assert_eq!(result[2].height, 1);
    }

    #[test]
    fn test_large_changed_area_fallback() {
        // Test that large changes fall back to full-frame
        let config = PathAConfig::default();
        let width = 100u16;
        let height = 100u16;

        let frame0 = create_solid_canvas(width, height, 255, 0, 0);
        // Change 80% of canvas (exceeds 70% threshold)
        let frame1 = create_canvas_with_rect(width, height, 255, 0, 0, 0, 0, 100, 80, 0, 255, 0);

        let canvases = vec![frame0, frame1];
        let delays = vec![Duration::from_millis(100); 2];

        let result = optimize_path_a(&canvases, &delays, config).expect("optimize_path_a failed");

        assert_eq!(result.len(), 2);
        // Frame 1 should be full-frame fallback
        assert_eq!(result[1].left, 0);
        assert_eq!(result[1].top, 0);
        assert_eq!(result[1].width, width);
        assert_eq!(result[1].height, height);
    }

    #[test]
    fn test_identical_consecutive_frames() {
        // Test handling of identical consecutive frames
        let config = PathAConfig::default();
        let width = 100u16;
        let height = 100u16;

        let frame0 = create_solid_canvas(width, height, 255, 0, 0);
        let frame1 = frame0.clone_canvas();

        let canvases = vec![frame0, frame1];
        let delays = vec![Duration::from_millis(100); 2];

        let result = optimize_path_a(&canvases, &delays, config).expect("optimize_path_a failed");

        assert_eq!(result.len(), 2);
        // Frame 1 should be 1x1 minimal patch
        assert_eq!(result[1].width, 1);
        assert_eq!(result[1].height, 1);
        assert_eq!(result[1].left, 0);
        assert_eq!(result[1].top, 0);
    }

    #[test]
    fn test_no_synthetic_transparency() {
        // Verify that Path A does not introduce synthetic transparency
        let config = PathAConfig::default();
        let width = 100u16;
        let height = 100u16;

        let frame0 = create_solid_canvas(width, height, 255, 0, 0);
        let frame1 = create_canvas_with_rect(width, height, 255, 0, 0, 10, 10, 20, 20, 0, 255, 0);

        let canvases = vec![frame0, frame1];
        let delays = vec![Duration::from_millis(100); 2];

        let result = optimize_path_a(&canvases, &delays, config).expect("optimize_path_a failed");

        // Frame 1 is a bbox patch
        let frame1_pixels = &result[1].pixels;
        // All pixels should be opaque (alpha = 255)
        for chunk in frame1_pixels.chunks_exact(4) {
            assert_eq!(chunk[3], 255, "All pixels in Path A output must be opaque");
        }
    }

    #[test]
    fn test_disposal_always_none() {
        // Verify that all emitted frames use None disposal
        let config = PathAConfig::default();
        let width = 100u16;
        let height = 100u16;

        let frame0 = create_solid_canvas(width, height, 255, 0, 0);
        let frame1 = create_canvas_with_rect(width, height, 255, 0, 0, 10, 10, 20, 20, 0, 255, 0);

        let canvases = vec![frame0, frame1];
        let delays = vec![Duration::from_millis(100); 2];

        let result = optimize_path_a(&canvases, &delays, config).expect("optimize_path_a failed");

        for frame in &result {
            assert_eq!(
                frame.dispose,
                DisposalMethod::None,
                "All Path A frames must use None disposal"
            );
        }
    }

    #[test]
    fn test_exact_changed_bbox_preserved() {
        // Verify that the exact changed bbox is preserved (no dropped pixels)
        let config = PathAConfig::default();
        let width = 200u16;
        let height = 200u16;

        let frame0 = create_solid_canvas(width, height, 255, 0, 0);
        let frame1 = create_canvas_with_rect(width, height, 255, 0, 0, 50, 50, 30, 40, 0, 255, 0);

        let canvases = vec![frame0, frame1];
        let delays = vec![Duration::from_millis(100); 2];

        let result = optimize_path_a(&canvases, &delays, config).expect("optimize_path_a failed");

        // Frame 1 should have exact bbox
        assert_eq!(result[1].left, 50);
        assert_eq!(result[1].top, 50);
        assert_eq!(result[1].width, 30);
        assert_eq!(result[1].height, 40);

        // Verify pixel count matches
        let expected_pixel_count = (30 * 40 * 4) as usize;
        assert_eq!(result[1].pixels.len(), expected_pixel_count);
    }
}

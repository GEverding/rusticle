//! Canonical sequence IR with canvas-state invariants.
//!
//! This module defines the ground-truth representation of a GIF animation in display/canvas space.
//! All downstream adaptive encoding passes reference this IR to ensure correctness.
//!
//! # Invariants
//!
//! - `displayed_canvas` is what the viewer sees for that frame.
//! - `pre_draw_canvas[n+1] == post_disposal_canvas[n]`.
//! - `post_disposal_canvas` reflects disposal, not merely displayed pixels.
//! - Changed bbox/count facts are computed from canonical canvases, not cropped frame clones.

use std::time::Duration;

use crate::error::Result;
use crate::types::{DisposalMethod, Gif, LoopCount};

/// Canonical sequence IR: ground truth for all adaptive decisions.
#[derive(Debug, Clone)]
pub struct CanonicalSequence {
    /// Canvas width in pixels.
    pub width: u16,
    /// Canvas height in pixels.
    pub height: u16,
    /// Loop count for the animation.
    pub loop_count: LoopCount,
    /// Per-frame canonical state.
    pub frames: Vec<CanonicalFrame>,
}

/// Per-frame canonical state in display/canvas space.
#[derive(Debug, Clone)]
pub struct CanonicalFrame {
    /// Source patch metadata: original frame position and dimensions.
    pub source_patch: SourcePatch,
    /// Canvas state before drawing this frame (post-disposal of previous frame).
    pub pre_draw_canvas: Canvas,
    /// Canvas state after drawing this frame (before disposal).
    pub displayed_canvas: Canvas,
    /// Canvas state after disposal (before next frame's pre_draw).
    pub post_disposal_canvas: Canvas,
    /// Changed region facts.
    pub changed_region: ChangedRegion,
    /// Frame timing and disposal.
    pub delay: Duration,
    pub dispose: DisposalMethod,
}

/// Source patch metadata: the original frame from the GIF.
#[derive(Debug, Clone)]
pub struct SourcePatch {
    /// RGBA pixel data of the source frame.
    pub pixels: Vec<u8>,
    /// Horizontal offset on canvas.
    pub left: u16,
    /// Vertical offset on canvas.
    pub top: u16,
    /// Width of the source frame.
    pub width: u16,
    /// Height of the source frame.
    pub height: u16,
    /// Whether the source patch has any transparent pixels.
    pub has_transparency: bool,
    /// Count of transparent pixels (alpha == 0).
    pub transparent_pixel_count: usize,
    /// Count of opaque pixels (alpha == 255).
    pub opaque_pixel_count: usize,
}

/// Full canvas RGBA state.
#[derive(Debug, Clone)]
pub struct Canvas {
    /// Full canvas RGBA pixels: width * height * 4 bytes.
    pub pixels: Vec<u8>,
    /// Canvas width.
    pub width: u16,
    /// Canvas height.
    pub height: u16,
}

/// Changed region facts between pre_draw and displayed canvas.
#[derive(Debug, Clone)]
pub struct ChangedRegion {
    /// Bounding box of changed pixels.
    pub bbox: BoundingBox,
    /// Count of pixels that changed.
    pub changed_pixel_count: usize,
    /// Ratio of changed pixels to total canvas pixels.
    pub changed_ratio: f32,
    /// Whether the source patch is full-canvas (left=0, top=0, width=canvas.width, height=canvas.height).
    pub is_full_canvas_patch: bool,
}

/// Bounding box of a region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundingBox {
    /// Left edge (inclusive).
    pub left: u16,
    /// Top edge (inclusive).
    pub top: u16,
    /// Right edge (exclusive).
    pub right: u16,
    /// Bottom edge (exclusive).
    pub bottom: u16,
}

impl BoundingBox {
    /// Create a new bounding box.
    pub fn new(left: u16, top: u16, right: u16, bottom: u16) -> Self {
        Self { left, top, right, bottom }
    }

    /// Width of the bounding box.
    pub fn width(&self) -> u16 {
        self.right.saturating_sub(self.left)
    }

    /// Height of the bounding box.
    pub fn height(&self) -> u16 {
        self.bottom.saturating_sub(self.top)
    }

    /// Area of the bounding box.
    pub fn area(&self) -> usize {
        (self.width() as usize) * (self.height() as usize)
    }
}

impl Canvas {
    /// Create a new canvas with the given dimensions, initialized to transparent.
    pub fn new(width: u16, height: u16) -> Self {
        let size = (width as usize) * (height as usize) * 4;
        Self {
            pixels: vec![0u8; size],
            width,
            height,
        }
    }

    /// Clone the canvas.
    pub fn clone_canvas(&self) -> Self {
        Self {
            pixels: self.pixels.clone(),
            width: self.width,
            height: self.height,
        }
    }

    /// Get a pixel at (x, y) as [r, g, b, a].
    pub fn get_pixel(&self, x: u16, y: u16) -> [u8; 4] {
        let idx = ((y as usize) * (self.width as usize) + (x as usize)) * 4;
        [
            self.pixels[idx],
            self.pixels[idx + 1],
            self.pixels[idx + 2],
            self.pixels[idx + 3],
        ]
    }

    /// Set a pixel at (x, y) to [r, g, b, a].
    pub fn set_pixel(&mut self, x: u16, y: u16, pixel: [u8; 4]) {
        let idx = ((y as usize) * (self.width as usize) + (x as usize)) * 4;
        self.pixels[idx] = pixel[0];
        self.pixels[idx + 1] = pixel[1];
        self.pixels[idx + 2] = pixel[2];
        self.pixels[idx + 3] = pixel[3];
    }

    /// Clear a region to transparent.
    pub fn clear_region(&mut self, left: u16, top: u16, width: u16, height: u16) {
        for y in 0..height {
            for x in 0..width {
                let canvas_y = top.saturating_add(y);
                let canvas_x = left.saturating_add(x);
                if canvas_y >= self.height || canvas_x >= self.width {
                    continue;
                }
                self.set_pixel(canvas_x, canvas_y, [0, 0, 0, 0]);
            }
        }
    }

    /// Composite a frame onto this canvas at the specified position with alpha blending.
    pub fn composite_frame(
        &mut self,
        frame_pixels: &[u8],
        frame_width: u16,
        frame_height: u16,
        left: u16,
        top: u16,
    ) {
        for y in 0..frame_height {
            let canvas_y = top.saturating_add(y);
            if canvas_y >= self.height {
                break;
            }

            for x in 0..frame_width {
                let canvas_x = left.saturating_add(x);
                if canvas_x >= self.width {
                    break;
                }

                let frame_idx = ((y as usize) * (frame_width as usize) + (x as usize)) * 4;
                let r = frame_pixels[frame_idx];
                let g = frame_pixels[frame_idx + 1];
                let b = frame_pixels[frame_idx + 2];
                let a = frame_pixels[frame_idx + 3];

                if a == 255 {
                    // Fully opaque - replace
                    self.set_pixel(canvas_x, canvas_y, [r, g, b, a]);
                } else if a > 0 {
                    // Partially transparent - blend
                    let [bg_r, bg_g, bg_b, bg_a] = self.get_pixel(canvas_x, canvas_y);
                    let bg_r = bg_r as u16;
                    let bg_g = bg_g as u16;
                    let bg_b = bg_b as u16;
                    let bg_a = bg_a as u16;

                    let alpha = a as u16;
                    let inv_alpha = 255 - alpha;

                    let new_r = ((r as u16 * alpha + bg_r * inv_alpha) / 255) as u8;
                    let new_g = ((g as u16 * alpha + bg_g * inv_alpha) / 255) as u8;
                    let new_b = ((b as u16 * alpha + bg_b * inv_alpha) / 255) as u8;
                    let new_a = ((alpha + bg_a * inv_alpha / 255).min(255)) as u8;

                    self.set_pixel(canvas_x, canvas_y, [new_r, new_g, new_b, new_a]);
                }
                // If a == 0, leave canvas pixel unchanged
            }
        }
    }
}

impl SourcePatch {
    /// Analyze transparency in the source patch.
    fn analyze_transparency(pixels: &[u8]) -> (bool, usize, usize) {
        let mut has_transparency = false;
        let mut transparent_count = 0;
        let mut opaque_count = 0;

        for chunk in pixels.chunks_exact(4) {
            let alpha = chunk[3];
            if alpha == 0 {
                has_transparency = true;
                transparent_count += 1;
            } else if alpha == 255 {
                opaque_count += 1;
            } else {
                has_transparency = true;
            }
        }

        (has_transparency, transparent_count, opaque_count)
    }
}

impl ChangedRegion {
    /// Compute changed region between two canvases.
    fn compute(pre_draw: &Canvas, displayed: &Canvas) -> Self {
        let width = pre_draw.width as usize;
        let height = pre_draw.height as usize;

        let mut min_x = width as u16;
        let mut min_y = height as u16;
        let mut max_x = 0u16;
        let mut max_y = 0u16;
        let mut changed_count = 0;

        for y in 0..height {
            for x in 0..width {
                let idx = (y * width + x) * 4;
                let pre = &pre_draw.pixels[idx..idx + 4];
                let dis = &displayed.pixels[idx..idx + 4];

                if pre != dis {
                    changed_count += 1;
                    min_x = min_x.min(x as u16);
                    min_y = min_y.min(y as u16);
                    max_x = max_x.max((x + 1) as u16);
                    max_y = max_y.max((y + 1) as u16);
                }
            }
        }

        let bbox = if changed_count == 0 {
            BoundingBox::new(0, 0, 0, 0)
        } else {
            BoundingBox::new(min_x, min_y, max_x, max_y)
        };

        let total_pixels = width * height;
        let changed_ratio = if total_pixels > 0 {
            changed_count as f32 / total_pixels as f32
        } else {
            0.0
        };

        Self {
            bbox,
            changed_pixel_count: changed_count,
            changed_ratio,
            is_full_canvas_patch: false, // Will be set by builder
        }
    }
}

/// Builder for canonical sequence IR from a decoded Gif.
pub struct CanonicalSequenceBuilder;

impl CanonicalSequenceBuilder {
    /// Build a canonical sequence IR from a decoded Gif.
    ///
    /// This reconstructs the canonical canvas states for each frame by:
    /// 1. Tracking the pre-draw canvas (post-disposal of previous frame).
    /// 2. Compositing the source patch onto the pre-draw canvas.
    /// 3. Recording the displayed canvas (after compositing, before disposal).
    /// 4. Applying the disposal method to get the post-disposal canvas.
    /// 5. Computing changed region facts from canonical canvases.
    ///
    /// # Invariants Maintained
    ///
    /// - `displayed_canvas` is computed by compositing source patch onto pre_draw_canvas.
    /// - `post_disposal_canvas` is computed by applying disposal to displayed_canvas.
    /// - `pre_draw_canvas[n+1] == post_disposal_canvas[n]`.
    /// - Changed regions are computed from canonical canvases, not frame clones.
    pub fn build(gif: &Gif) -> Result<CanonicalSequence> {
        let width = gif.width;
        let height = gif.height;
        let loop_count = gif.loop_count;

        let mut frames = Vec::new();
        let mut pre_draw_canvas = Canvas::new(width, height);

        for frame in &gif.frames {
            // Snapshot the pre-draw canvas
            let pre_draw_canvas_snapshot = pre_draw_canvas.clone_canvas();

            // Composite the frame onto the pre-draw canvas
            let mut displayed_canvas = pre_draw_canvas.clone_canvas();
            displayed_canvas.composite_frame(
                &frame.pixels,
                frame.width,
                frame.height,
                frame.left,
                frame.top,
            );

            // Compute changed region
            let mut changed_region = ChangedRegion::compute(&pre_draw_canvas_snapshot, &displayed_canvas);

            // Check if source patch is full-canvas
            changed_region.is_full_canvas_patch =
                frame.left == 0 && frame.top == 0 && frame.width == width && frame.height == height;

            // Apply disposal method to get post-disposal canvas
            let mut post_disposal_canvas = displayed_canvas.clone_canvas();
            match frame.dispose {
                DisposalMethod::Background => {
                    post_disposal_canvas.clear_region(frame.left, frame.top, frame.width, frame.height);
                }
                DisposalMethod::Previous => {
                    // Restore to pre-draw canvas
                    post_disposal_canvas = pre_draw_canvas_snapshot.clone_canvas();
                }
                DisposalMethod::Keep | DisposalMethod::None => {
                    // Leave as-is
                }
            }

            // Extract source patch from the full frame
            // Note: current Gif stores full-canvas frames, so we need to extract the patch
            let source_patch = extract_source_patch(frame, width, height);

            let canonical_frame = CanonicalFrame {
                source_patch,
                pre_draw_canvas: pre_draw_canvas_snapshot,
                displayed_canvas,
                post_disposal_canvas: post_disposal_canvas.clone_canvas(),
                changed_region,
                delay: frame.delay,
                dispose: frame.dispose,
            };

            frames.push(canonical_frame);

            // Update pre_draw_canvas for next iteration
            pre_draw_canvas = post_disposal_canvas;
        }

        Ok(CanonicalSequence {
            width,
            height,
            loop_count,
            frames,
        })
    }
}

/// Extract source patch metadata from a frame.
///
/// The frame.pixels buffer can be either:
/// 1. Full-canvas-sized: width * height * 4 bytes (legacy/decoded frames)
/// 2. Subframe-sized: frame.width * frame.height * 4 bytes (subframe patches)
///
/// We detect which case we're in and extract the patch accordingly.
fn extract_source_patch(frame: &crate::types::Frame, canvas_width: u16, canvas_height: u16) -> SourcePatch {
    let frame_width = frame.width as usize;
    let frame_height = frame.height as usize;
    let left = frame.left;
    let top = frame.top;

    let expected_subframe_size = frame_width * frame_height * 4;
    let expected_full_canvas_size = (canvas_width as usize) * (canvas_height as usize) * 4;

    // Determine if frame.pixels is full-canvas-sized or subframe-sized
    let is_full_canvas = frame.pixels.len() == expected_full_canvas_size;
    let is_subframe = frame.pixels.len() == expected_subframe_size;

    let patch_pixels = if is_full_canvas {
        // Full-canvas buffer: extract the subframe region
        let mut patch = Vec::with_capacity(expected_subframe_size);
        for y in 0..frame_height {
            for x in 0..frame_width {
                let canvas_x = left as usize + x;
                let canvas_y = top as usize + y;

                if canvas_x < canvas_width as usize && canvas_y < canvas_height as usize {
                    let frame_idx = (canvas_y * (canvas_width as usize) + canvas_x) * 4;
                    patch.extend_from_slice(&frame.pixels[frame_idx..frame_idx + 4]);
                } else {
                    // Out of bounds - fill with transparent
                    patch.extend_from_slice(&[0, 0, 0, 0]);
                }
            }
        }
        patch
    } else if is_subframe {
        // Subframe buffer: use directly (already the right size and content)
        frame.pixels.clone()
    } else {
        // Invalid size: neither full-canvas nor subframe
        // Treat as subframe and hope for the best (will likely fail downstream)
        frame.pixels.clone()
    };

    let (has_transparency, transparent_count, opaque_count) = SourcePatch::analyze_transparency(&patch_pixels);

    SourcePatch {
        pixels: patch_pixels,
        left,
        top,
        width: frame.width,
        height: frame.height,
        has_transparency,
        transparent_pixel_count: transparent_count,
        opaque_pixel_count: opaque_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Frame;
    use std::time::Duration;

    /// Create a test Gif with specified dimensions and frame count.
    fn create_test_gif(width: u16, height: u16, frame_count: usize) -> Gif {
        let canvas_size = (width as usize) * (height as usize) * 4;
        let mut frames = Vec::new();

        for i in 0..frame_count {
            let mut pixels = vec![0u8; canvas_size];
            // Fill with a simple pattern
            for j in 0..canvas_size / 4 {
                pixels[j * 4] = (i * 50) as u8; // R
                pixels[j * 4 + 1] = (j % 256) as u8; // G
                pixels[j * 4 + 2] = 100; // B
                pixels[j * 4 + 3] = 255; // A (opaque)
            }

            frames.push(Frame {
                pixels,
                delay: Duration::from_millis(100),
                dispose: DisposalMethod::Keep,
                local_palette: None,
                left: 0,
                top: 0,
                width,
                height,
            });
        }

        Gif {
            width,
            height,
            global_palette: None,
            frames,
            loop_count: LoopCount::Infinite,
            original_palette: None,
        }
    }

    /// Create a test Gif with a single opaque delta frame.
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

    /// Create a test Gif with Background disposal.
    fn create_background_disposal_gif(width: u16, height: u16) -> Gif {
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

    /// Create a test Gif with Previous disposal.
    fn create_previous_disposal_gif(width: u16, height: u16) -> Gif {
        let canvas_size = (width as usize) * (height as usize) * 4;

        // Frame 0: full opaque canvas (red)
        let mut frame0_pixels = vec![0u8; canvas_size];
        for j in 0..canvas_size / 4 {
            frame0_pixels[j * 4] = 255; // R
            frame0_pixels[j * 4 + 1] = 0; // G
            frame0_pixels[j * 4 + 2] = 0; // B
            frame0_pixels[j * 4 + 3] = 255; // A
        }

        // Frame 1: overlay on top (green)
        let mut frame1_pixels = frame0_pixels.clone();
        for j in 0..canvas_size / 4 {
            frame1_pixels[j * 4] = 0; // R
            frame1_pixels[j * 4 + 1] = 255; // G
            frame1_pixels[j * 4 + 2] = 0; // B
            frame1_pixels[j * 4 + 3] = 255; // A
        }

        // Frame 2: should restore to frame 0 (red) after frame 1 is disposed
        let frame2_pixels = frame0_pixels.clone();

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
                    dispose: DisposalMethod::Previous,
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

    #[test]
    fn test_canvas_new() {
        let canvas = Canvas::new(100, 100);
        assert_eq!(canvas.width, 100);
        assert_eq!(canvas.height, 100);
        assert_eq!(canvas.pixels.len(), 100 * 100 * 4);
        // Should be initialized to transparent (all zeros)
        assert!(canvas.pixels.iter().all(|&b| b == 0));
    }

    #[test]
    fn test_canvas_pixel_operations() {
        let mut canvas = Canvas::new(10, 10);
        let pixel = [255, 128, 64, 255];
        canvas.set_pixel(5, 5, pixel);
        assert_eq!(canvas.get_pixel(5, 5), pixel);
    }

    #[test]
    fn test_canvas_composite_opaque() {
        let mut canvas = Canvas::new(10, 10);
        // Set background to red
        for y in 0..10 {
            for x in 0..10 {
                canvas.set_pixel(x, y, [255, 0, 0, 255]);
            }
        }

        // Composite a blue frame at (2, 2) with size 4x4
        let mut frame_pixels = vec![0u8; 4 * 4 * 4];
        for i in 0..16 {
            frame_pixels[i * 4] = 0; // R
            frame_pixels[i * 4 + 1] = 0; // G
            frame_pixels[i * 4 + 2] = 255; // B
            frame_pixels[i * 4 + 3] = 255; // A
        }

        canvas.composite_frame(&frame_pixels, 4, 4, 2, 2);

        // Check that the composited region is blue
        for y in 2..6 {
            for x in 2..6 {
                assert_eq!(canvas.get_pixel(x, y), [0, 0, 255, 255]);
            }
        }

        // Check that outside the region is still red
        assert_eq!(canvas.get_pixel(1, 1), [255, 0, 0, 255]);
        assert_eq!(canvas.get_pixel(7, 7), [255, 0, 0, 255]);
    }

    #[test]
    fn test_canvas_clear_region() {
        let mut canvas = Canvas::new(10, 10);
        // Fill with opaque white
        for y in 0..10 {
            for x in 0..10 {
                canvas.set_pixel(x, y, [255, 255, 255, 255]);
            }
        }

        // Clear a region
        canvas.clear_region(2, 2, 4, 4);

        // Check that the cleared region is transparent
        for y in 2..6 {
            for x in 2..6 {
                assert_eq!(canvas.get_pixel(x, y), [0, 0, 0, 0]);
            }
        }

        // Check that outside is still white
        assert_eq!(canvas.get_pixel(1, 1), [255, 255, 255, 255]);
        assert_eq!(canvas.get_pixel(7, 7), [255, 255, 255, 255]);
    }

    #[test]
    fn test_bounding_box() {
        let bbox = BoundingBox::new(10, 20, 30, 40);
        assert_eq!(bbox.width(), 20);
        assert_eq!(bbox.height(), 20);
        assert_eq!(bbox.area(), 400);
    }

    #[test]
    fn test_canonical_sequence_simple() {
        let gif = create_test_gif(50, 50, 2);
        let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");

        assert_eq!(seq.width, 50);
        assert_eq!(seq.height, 50);
        assert_eq!(seq.frames.len(), 2);

        // Check invariant: pre_draw_canvas[1] == post_disposal_canvas[0]
        assert_eq!(
            seq.frames[0].post_disposal_canvas.pixels,
            seq.frames[1].pre_draw_canvas.pixels
        );
    }

    #[test]
    fn test_canonical_sequence_opaque_delta() {
        let gif = create_opaque_delta_gif(50, 50);
        let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");

        assert_eq!(seq.frames.len(), 2);

        // Frame 0: full canvas (all pixels changed from transparent background)
        assert!(seq.frames[0].changed_region.changed_pixel_count > 0);

        // Frame 1: 10x10 delta (only the changed region)
        assert!(seq.frames[1].changed_region.changed_pixel_count > 0);
        assert!(seq.frames[1].changed_region.changed_pixel_count <= 100);

        // Check invariant: displayed_canvas[0] == pre_draw_canvas[1]
        // (since disposal is Keep)
        assert_eq!(
            seq.frames[0].displayed_canvas.pixels,
            seq.frames[1].pre_draw_canvas.pixels
        );
    }

    #[test]
    fn test_canonical_sequence_background_disposal() {
        let gif = create_background_disposal_gif(50, 50);
        let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");

        assert_eq!(seq.frames.len(), 3);

        // Frame 0: full opaque
        let frame0_opaque_count = seq.frames[0]
            .displayed_canvas
            .pixels
            .chunks_exact(4)
            .filter(|chunk| chunk[3] == 255)
            .count();
        assert!(frame0_opaque_count > 0);

        // Frame 1: same as frame 0 (displayed)
        assert_eq!(
            seq.frames[0].displayed_canvas.pixels,
            seq.frames[1].displayed_canvas.pixels
        );

        // After frame 1's Background disposal, canvas should be transparent
        let frame1_post_disposal_transparent = seq.frames[1]
            .post_disposal_canvas
            .pixels
            .chunks_exact(4)
            .all(|chunk| chunk[3] == 0);
        assert!(frame1_post_disposal_transparent);

        // Frame 2's pre_draw should be transparent
        let frame2_pre_draw_transparent = seq.frames[2]
            .pre_draw_canvas
            .pixels
            .chunks_exact(4)
            .all(|chunk| chunk[3] == 0);
        assert!(frame2_pre_draw_transparent);

        // Invariant: pre_draw_canvas[2] == post_disposal_canvas[1]
        assert_eq!(
            seq.frames[1].post_disposal_canvas.pixels,
            seq.frames[2].pre_draw_canvas.pixels
        );
    }

    #[test]
    fn test_canonical_sequence_previous_disposal() {
        let gif = create_previous_disposal_gif(50, 50);
        let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");

        assert_eq!(seq.frames.len(), 3);

        // Frame 0: red
        let frame0_red = seq.frames[0].displayed_canvas.pixels[0] == 255;
        assert!(frame0_red);

        // Frame 1: green (displayed)
        let frame1_green = seq.frames[1].displayed_canvas.pixels[0] == 0
            && seq.frames[1].displayed_canvas.pixels[1] == 255;
        assert!(frame1_green);

        // After frame 1's Previous disposal, canvas should restore to frame 0 (red)
        let frame1_post_disposal_red = seq.frames[1].post_disposal_canvas.pixels[0] == 255
            && seq.frames[1].post_disposal_canvas.pixels[1] == 0;
        assert!(frame1_post_disposal_red);

        // Frame 2's pre_draw should be red (same as frame 0)
        let frame2_pre_draw_red = seq.frames[2].pre_draw_canvas.pixels[0] == 255
            && seq.frames[2].pre_draw_canvas.pixels[1] == 0;
        assert!(frame2_pre_draw_red);

        // Invariant: pre_draw_canvas[2] == post_disposal_canvas[1]
        assert_eq!(
            seq.frames[1].post_disposal_canvas.pixels,
            seq.frames[2].pre_draw_canvas.pixels
        );
    }

    #[test]
    fn test_canonical_sequence_invariant_pre_draw_post_disposal() {
        let gif = create_test_gif(50, 50, 3);
        let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");

        // Check invariant for all consecutive frames
        for i in 0..seq.frames.len() - 1 {
            assert_eq!(
                seq.frames[i].post_disposal_canvas.pixels,
                seq.frames[i + 1].pre_draw_canvas.pixels,
                "Invariant violated at frame {}: post_disposal[{}] != pre_draw[{}]",
                i,
                i,
                i + 1
            );
        }
    }

    #[test]
    fn test_source_patch_transparency_analysis() {
        // Opaque patch: 100 pixels of [255, 0, 0, 255]
        let mut opaque_pixels = Vec::new();
        for _ in 0..100 {
            opaque_pixels.extend_from_slice(&[255u8, 0, 0, 255]);
        }
        let (has_trans, trans_count, opaque_count) = SourcePatch::analyze_transparency(&opaque_pixels);
        assert!(!has_trans);
        assert_eq!(trans_count, 0);
        assert_eq!(opaque_count, 100);

        // Transparent patch: 100 pixels of [0, 0, 0, 0]
        let transparent_pixels = vec![0u8; 400];
        let (has_trans, trans_count, opaque_count) = SourcePatch::analyze_transparency(&transparent_pixels);
        assert!(has_trans);
        assert_eq!(trans_count, 100);
        assert_eq!(opaque_count, 0);

        // Mixed patch: 100 opaque, 100 transparent (200 pixels total = 800 bytes)
        let mut mixed_pixels = vec![0u8; 800];
        for i in 0..100 {
            mixed_pixels[i * 4 + 3] = 255; // First 100 opaque
        }
        // Last 100 are transparent (alpha=0 by default)
        let (has_trans, trans_count, opaque_count) = SourcePatch::analyze_transparency(&mixed_pixels);
        assert!(has_trans);
        assert_eq!(trans_count, 100);
        assert_eq!(opaque_count, 100);
    }

    #[test]
    fn test_changed_region_full_canvas() {
        let mut canvas1 = Canvas::new(10, 10);
        let mut canvas2 = Canvas::new(10, 10);

        // Fill canvas1 with red
        for y in 0..10 {
            for x in 0..10 {
                canvas1.set_pixel(x, y, [255, 0, 0, 255]);
            }
        }

        // Fill canvas2 with blue
        for y in 0..10 {
            for x in 0..10 {
                canvas2.set_pixel(x, y, [0, 0, 255, 255]);
            }
        }

        let changed = ChangedRegion::compute(&canvas1, &canvas2);
        assert_eq!(changed.changed_pixel_count, 100);
        assert_eq!(changed.changed_ratio, 1.0);
    }

    #[test]
    fn test_changed_region_partial() {
        let canvas1 = Canvas::new(10, 10);
        let mut canvas2 = canvas1.clone_canvas();

        // Change only a 5x5 region
        for y in 2..7 {
            for x in 2..7 {
                canvas2.set_pixel(x, y, [255, 0, 0, 255]);
            }
        }

        let changed = ChangedRegion::compute(&canvas1, &canvas2);
        assert_eq!(changed.changed_pixel_count, 25);
        assert_eq!(changed.bbox.left, 2);
        assert_eq!(changed.bbox.top, 2);
        assert_eq!(changed.bbox.right, 7);
        assert_eq!(changed.bbox.bottom, 7);
    }

    #[test]
    fn test_changed_region_no_change() {
        let canvas1 = Canvas::new(10, 10);
        let canvas2 = canvas1.clone_canvas();

        let changed = ChangedRegion::compute(&canvas1, &canvas2);
        assert_eq!(changed.changed_pixel_count, 0);
        assert_eq!(changed.bbox.area(), 0);
    }
}

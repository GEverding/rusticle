//! GIF decoding with canvas compositing.
//!
//! Handles frame disposal methods, alpha blending, and subframe positioning
//! to produce full-canvas RGBA frames.

use std::io::Read;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::types::{DisposalMethod, Frame, Gif, LoopCount, Palette};

/// Composite a frame onto the canvas at the specified position.
#[allow(clippy::too_many_arguments)]
fn composite_frame(
    canvas: &mut [u8],
    frame_data: &[u8],
    canvas_width: usize,
    canvas_height: usize,
    left: usize,
    top: usize,
    frame_width: usize,
    frame_height: usize,
) {
    for y in 0..frame_height {
        let canvas_y = top + y;
        if canvas_y >= canvas_height {
            break;
        }

        for x in 0..frame_width {
            let canvas_x = left + x;
            if canvas_x >= canvas_width {
                break;
            }

            let frame_idx = (y * frame_width + x) * 4;
            let canvas_idx = (canvas_y * canvas_width + canvas_x) * 4;

            // Get frame pixel
            let r = frame_data[frame_idx];
            let g = frame_data[frame_idx + 1];
            let b = frame_data[frame_idx + 2];
            let a = frame_data[frame_idx + 3];

            // Alpha blend onto canvas
            if a == 255 {
                // Fully opaque - replace
                canvas[canvas_idx] = r;
                canvas[canvas_idx + 1] = g;
                canvas[canvas_idx + 2] = b;
                canvas[canvas_idx + 3] = a;
            } else if a > 0 {
                // Partially transparent - blend
                let bg_r = canvas[canvas_idx] as u16;
                let bg_g = canvas[canvas_idx + 1] as u16;
                let bg_b = canvas[canvas_idx + 2] as u16;
                let bg_a = canvas[canvas_idx + 3] as u16;

                let alpha = a as u16;
                let inv_alpha = 255 - alpha;

                canvas[canvas_idx] = ((r as u16 * alpha + bg_r * inv_alpha) / 255) as u8;
                canvas[canvas_idx + 1] = ((g as u16 * alpha + bg_g * inv_alpha) / 255) as u8;
                canvas[canvas_idx + 2] = ((b as u16 * alpha + bg_b * inv_alpha) / 255) as u8;
                canvas[canvas_idx + 3] = ((alpha + bg_a * inv_alpha / 255).min(255)) as u8;
            }
            // If a == 0, leave canvas pixel unchanged
        }
    }
}

/// Clear a region of the canvas to transparent.
fn clear_region(
    canvas: &mut [u8],
    canvas_width: usize,
    left: usize,
    top: usize,
    width: usize,
    height: usize,
) {
    for y in 0..height {
        for x in 0..width {
            let canvas_idx = ((top + y) * canvas_width + (left + x)) * 4;
            canvas[canvas_idx] = 0;
            canvas[canvas_idx + 1] = 0;
            canvas[canvas_idx + 2] = 0;
            canvas[canvas_idx + 3] = 0;
        }
    }
}

impl Gif {
    /// Decode a GIF from a byte slice.
    ///
    /// Reads the entire GIF, compositing each frame onto a canvas according to
    /// the disposal method. Returns a [`Gif`] with full-canvas RGBA frames.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DecodeError`] if the data is not a valid GIF or contains
    /// malformed frame data.
    ///
    /// Returns [`Error::InvalidGif`] if the GIF header is missing or corrupt.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use rusticle::Gif;
    ///
    /// let data = std::fs::read("animation.gif")?;
    /// let gif = Gif::from_bytes(&data)?;
    /// println!("{}x{}, {} frames", gif.width, gif.height, gif.frames.len());
    /// ```
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        Self::from_read(data)
    }

    /// Decode a GIF from any [`Read`] implementation.
    ///
    /// Same as [`from_bytes`](Self::from_bytes) but accepts a reader (file, network stream, etc.).
    ///
    /// # Errors
    ///
    /// Returns [`Error::DecodeError`] if the stream does not contain a valid GIF.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use rusticle::Gif;
    /// use std::fs::File;
    ///
    /// let file = File::open("animation.gif")?;
    /// let gif = Gif::from_read(file)?;
    /// ```
    pub fn from_read<R: Read>(reader: R) -> Result<Self> {
        let mut decoder = gif::DecodeOptions::new();
        decoder.set_color_output(gif::ColorOutput::RGBA);

        let mut reader = decoder
            .read_info(reader)
            .map_err(|e| Error::DecodeError(e.to_string()))?;

        let width = reader.width();
        let height = reader.height();

        // Extract global palette if present
        let global_palette = reader.global_palette().map(|palette| {
            let colors = palette
                .chunks_exact(3)
                .map(|chunk| [chunk[0], chunk[1], chunk[2]])
                .collect();
            Palette { colors }
        });

        // Store original palette for fast resize path
        let original_palette = global_palette.as_ref().map(|p| p.colors.clone());

        // Default loop count is infinite
        let loop_count = LoopCount::Infinite;

        let mut frames = Vec::new();

        // Canvas for compositing frames
        let canvas_size = (width as usize) * (height as usize) * 4;
        let mut canvas = vec![0u8; canvas_size];
        let mut prev_canvas = vec![0u8; canvas_size];

        // Decode all frames
        while let Some(frame) = reader
            .read_next_frame()
            .map_err(|e| Error::DecodeError(e.to_string()))?
        {
            let frame_width = frame.width as usize;
            let frame_height = frame.height as usize;
            let left = frame.left as usize;
            let top = frame.top as usize;

            // Convert delay from 10ms units to Duration
            let delay = Duration::from_millis(frame.delay as u64 * 10);

            // Map disposal method
            let dispose = match frame.dispose {
                gif::DisposalMethod::Any => DisposalMethod::None,
                gif::DisposalMethod::Keep => DisposalMethod::Keep,
                gif::DisposalMethod::Background => DisposalMethod::Background,
                gif::DisposalMethod::Previous => DisposalMethod::Previous,
            };

            // Extract local palette if present
            let local_palette = frame.palette.as_ref().map(|palette| {
                let colors = palette
                    .chunks_exact(3)
                    .map(|chunk| [chunk[0], chunk[1], chunk[2]])
                    .collect();
                Palette { colors }
            });

            // Save previous canvas state if disposal method is Previous
            if dispose == DisposalMethod::Previous {
                prev_canvas.copy_from_slice(&canvas);
            }

            // Composite frame onto canvas
            composite_frame(
                &mut canvas,
                &frame.buffer,
                width as usize,
                height as usize,
                left,
                top,
                frame_width,
                frame_height,
            );

            // Store the composited canvas
            frames.push(Frame {
                pixels: canvas.clone(),
                delay,
                dispose,
                local_palette,
                left: 0,
                top: 0,
                width,
                height,
            });

            // Apply disposal method for next frame
            match dispose {
                DisposalMethod::Background => {
                    // Clear the frame region to transparent
                    clear_region(
                        &mut canvas,
                        width as usize,
                        left,
                        top,
                        frame_width,
                        frame_height,
                    );
                }
                DisposalMethod::Previous => {
                    // Restore to previous canvas state
                    canvas.copy_from_slice(&prev_canvas);
                }
                _ => {
                    // Keep or None - leave canvas as-is
                }
            }
        }

        Ok(Gif {
            width,
            height,
            global_palette,
            frames,
            loop_count,
            original_palette,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_empty_bytes() {
        let result = Gif::from_bytes(&[]);
        assert!(result.is_err(), "Empty bytes should fail to decode");
    }

    #[test]
    fn test_decode_invalid_data() {
        let invalid_data = b"This is not a GIF file";
        let result = Gif::from_bytes(invalid_data);
        assert!(result.is_err(), "Invalid data should fail to decode");
    }

    #[test]
    fn test_decode_from_read_empty() {
        let data: &[u8] = &[];
        let result = Gif::from_read(data);
        assert!(result.is_err(), "Empty read should fail");
    }

    #[test]
    fn test_decode_from_read_invalid() {
        let data = b"Invalid GIF data";
        let result = Gif::from_read(&data[..]);
        assert!(result.is_err(), "Invalid read should fail");
    }
}

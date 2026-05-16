//! GIF encoding - convert frames back to GIF format.

use crate::error::{Error, Result};
use crate::palette_lut::PaletteLut;
use crate::types::{DisposalMethod, Gif};
use rayon::prelude::*;
use std::io::Write;
use std::sync::OnceLock;
use std::time::Instant;

/// Statistics from GIF encoding, measuring time spent in each stage.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EncodeStats {
    /// Time spent building the palette LUT (nanoseconds).
    pub lut_build_ns: u64,
    /// Total time spent quantizing all frames (nanoseconds).
    pub quantize_ns: u64,
    /// Number of frames that used the fast path (LUT-based quantization).
    pub quantize_fast_path_count: usize,
    /// Number of frames that used imagequant (full quantization).
    pub quantize_imagequant_count: usize,
    /// Total time spent writing quantized frames to GIF (nanoseconds).
    pub write_ns: u64,
    /// Total encode time (nanoseconds).
    pub total_ns: u64,
}

/// Intermediate representation of a quantized frame.
struct QuantizedFrame {
    palette: Vec<u8>, // Flat RGB: [r0,g0,b0,r1,g1,b1,...]
    indices: Vec<u8>, // Palette indices
    transparent_idx: Option<u8>,
    delay: std::time::Duration,
    dispose: DisposalMethod,
    left: u16,
    top: u16,
    width: u16,
    height: u16,
}

/// Result of quantizing a frame: (palette, indices, transparent_index, used_fast_path).
type QuantizeResult = (Vec<u8>, Vec<u8>, Option<u8>, bool);

impl Gif {
    /// Encode the GIF to bytes.
    ///
    /// Quantizes all frames in parallel (using rayon), then writes them sequentially.
    /// Uses the palette LUT fast path when a global palette is available, falling back
    /// to imagequant if quality would be insufficient.
    ///
    /// # Errors
    ///
    /// Returns [`Error::EncodeError`] if quantization or GIF writing fails.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # fn main() -> rusticle::Result<()> {
    /// use rusticle::{Filter, Gif, OptLevel};
    ///
    /// let data = std::fs::read("input.gif")?;
    /// let gif = Gif::from_bytes(&data)?;
    /// let bytes = gif
    ///     .resize(320, 240, Filter::Lanczos3)?
    ///     .optimize(OptLevel::O2)
    ///     .to_bytes()?;
    /// std::fs::write("output.gif", bytes)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut buffer = Vec::new();
        self.encode_to(&mut buffer)?;
        Ok(buffer)
    }

    /// Encode the GIF to bytes, consuming `self`.
    ///
    /// Equivalent to [`to_bytes`](Self::to_bytes). Provided for method chaining.
    ///
    /// # Errors
    ///
    /// Returns [`Error::EncodeError`] if encoding fails.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # fn main() -> rusticle::Result<()> {
    /// use rusticle::{Filter, Gif};
    ///
    /// let data = std::fs::read("input.gif")?;
    /// let bytes = Gif::from_bytes(&data)?
    ///     .resize(320, 240, Filter::Lanczos3)?
    ///     .into_bytes()?;
    /// std::fs::write("output.gif", bytes)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn into_bytes(self) -> Result<Vec<u8>> {
        self.to_bytes()
    }

    /// Encode to bytes and return timing statistics.
    ///
    /// Returns both the encoded bytes and [`EncodeStats`] with per-stage timing.
    ///
    /// # Errors
    ///
    /// Returns [`Error::EncodeError`] if encoding fails.
    pub fn to_bytes_with_stats(&self) -> Result<(Vec<u8>, EncodeStats)> {
        let total_start = Instant::now();
        let mut buffer = Vec::new();
        let stats = self.encode_to_with_stats(&mut buffer)?;
        let bytes = buffer;
        let mut stats = stats;
        stats.total_ns = total_start.elapsed().as_nanos() as u64;
        Ok((bytes, stats))
    }

    /// Encode the GIF to any [`Write`] implementation.
    ///
    /// # Errors
    ///
    /// Returns [`Error::EncodeError`] if quantization or writing fails.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # fn main() -> rusticle::Result<()> {
    /// use rusticle::Gif;
    /// use std::fs::File;
    ///
    /// let data = std::fs::read("input.gif")?;
    /// let gif = Gif::from_bytes(&data)?;
    /// let mut file = File::create("output.gif")?;
    /// gif.encode_to(&mut file)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn encode_to<W: Write>(&self, writer: W) -> Result<()> {
        let mut encoder = gif::Encoder::new(writer, self.width, self.height, &[])
            .map_err(|e| Error::EncodeError(format!("failed to create encoder: {}", e)))?;

        // Set loop count
        let repeat = match self.loop_count {
            crate::types::LoopCount::Infinite => gif::Repeat::Infinite,
            crate::types::LoopCount::Finite(n) => gif::Repeat::Finite(n),
        };
        encoder
            .set_repeat(repeat)
            .map_err(|e| Error::EncodeError(format!("failed to set repeat: {}", e)))?;

        // Build LUT once if we have original palette
        let lut = self.original_palette.as_ref().map(|p| PaletteLut::new(p));
        let lut_ref = lut.as_ref();

        // Quantize all frames in parallel (expensive part)
        let quantized_frames: Vec<QuantizedFrame> = self
            .frames
            .par_iter()
            .map(|frame| quantize_frame_parallel(frame, lut_ref))
            .collect::<Result<Vec<_>>>()?;

        // Write quantized frames sequentially (GIF format requires order)
        for qframe in quantized_frames {
            write_quantized_frame(&mut encoder, &qframe)?;
        }

        Ok(())
    }

    /// Encode to any [`Write`] implementation, returning [`EncodeStats`].
    ///
    /// Reports timing for LUT build, quantization, and frame writing stages.
    ///
    /// # Errors
    ///
    /// Returns [`Error::EncodeError`] if quantization or writing fails.
    pub fn encode_to_with_stats<W: Write>(&self, writer: W) -> Result<EncodeStats> {
        let mut stats = EncodeStats::default();
        let mut encoder = gif::Encoder::new(writer, self.width, self.height, &[])
            .map_err(|e| Error::EncodeError(format!("failed to create encoder: {}", e)))?;

        // Set loop count
        let repeat = match self.loop_count {
            crate::types::LoopCount::Infinite => gif::Repeat::Infinite,
            crate::types::LoopCount::Finite(n) => gif::Repeat::Finite(n),
        };
        encoder
            .set_repeat(repeat)
            .map_err(|e| Error::EncodeError(format!("failed to set repeat: {}", e)))?;

        // Build LUT once if we have original palette
        let lut_start = Instant::now();
        let lut = self.original_palette.as_ref().map(|p| PaletteLut::new(p));
        if self.original_palette.is_some() {
            stats.lut_build_ns = lut_start.elapsed().as_nanos() as u64;
        }
        let lut_ref = lut.as_ref();

        // Quantize all frames in parallel (expensive part)
        let quantize_start = Instant::now();
        let quantized_frames: Vec<(QuantizedFrame, bool)> = self
            .frames
            .par_iter()
            .map(|frame| {
                let (qframe, used_fast_path) = quantize_frame_parallel_with_stats(frame, lut_ref)?;
                Ok((qframe, used_fast_path))
            })
            .collect::<Result<Vec<_>>>()?;
        stats.quantize_ns = quantize_start.elapsed().as_nanos() as u64;

        // Count fast path vs imagequant
        for (_, used_fast_path) in &quantized_frames {
            if *used_fast_path {
                stats.quantize_fast_path_count += 1;
            } else {
                stats.quantize_imagequant_count += 1;
            }
        }

        // Write quantized frames sequentially (GIF format requires order)
        let write_start = Instant::now();
        for (qframe, _) in quantized_frames {
            write_quantized_frame(&mut encoder, &qframe)?;
        }
        stats.write_ns = write_start.elapsed().as_nanos() as u64;

        Ok(stats)
    }
}

/// Quantize a frame in parallel context (no encoder access).
fn quantize_frame_parallel(
    frame: &crate::types::Frame,
    lut: Option<&PaletteLut>,
) -> Result<QuantizedFrame> {
    // Quantize RGBA to indexed color
    let (palette, indexed, transparent_idx) = quantize_rgba(
        &frame.pixels,
        frame.width as usize,
        frame.height as usize,
        lut,
    )?;

    Ok(QuantizedFrame {
        palette,
        indices: indexed,
        transparent_idx,
        delay: frame.delay,
        dispose: frame.dispose,
        left: frame.left,
        top: frame.top,
        width: frame.width,
        height: frame.height,
    })
}

/// Quantize a frame in parallel context with tracking of fast path usage.
fn quantize_frame_parallel_with_stats(
    frame: &crate::types::Frame,
    lut: Option<&PaletteLut>,
) -> Result<(QuantizedFrame, bool)> {
    // Quantize RGBA to indexed color
    let (palette, indexed, transparent_idx, used_fast_path) = quantize_rgba_with_stats(
        &frame.pixels,
        frame.width as usize,
        frame.height as usize,
        lut,
    )?;

    Ok((
        QuantizedFrame {
            palette,
            indices: indexed,
            transparent_idx,
            delay: frame.delay,
            dispose: frame.dispose,
            left: frame.left,
            top: frame.top,
            width: frame.width,
            height: frame.height,
        },
        used_fast_path,
    ))
}

/// Write a pre-quantized frame to the encoder (sequential).
fn write_quantized_frame<W: Write>(
    encoder: &mut gif::Encoder<W>,
    qframe: &QuantizedFrame,
) -> Result<()> {
    // Convert delay from Duration to gif units (10ms increments)
    let delay_ms = qframe.delay.as_millis() as u16;
    let delay_units = (delay_ms + 5) / 10; // Round to nearest 10ms unit

    // Map disposal method
    let disposal = match qframe.dispose {
        DisposalMethod::None => gif::DisposalMethod::Any,
        DisposalMethod::Keep => gif::DisposalMethod::Keep,
        DisposalMethod::Background => gif::DisposalMethod::Background,
        DisposalMethod::Previous => gif::DisposalMethod::Previous,
    };

    // Create gif frame
    let mut gif_frame = gif::Frame::from_indexed_pixels(
        qframe.width,
        qframe.height,
        qframe.indices.clone(),
        qframe.transparent_idx,
    );

    // Set the palette on the frame
    gif_frame.palette = Some(qframe.palette.clone());

    // Set transparent index if we have transparent pixels
    if let Some(idx) = qframe.transparent_idx {
        gif_frame.transparent = Some(idx);
    }

    gif_frame.delay = delay_units;
    gif_frame.dispose = disposal;
    gif_frame.left = qframe.left;
    gif_frame.top = qframe.top;

    encoder
        .write_frame(&gif_frame)
        .map_err(|e| Error::EncodeError(format!("failed to write frame: {}", e)))?;

    Ok(())
}

/// Check if diagnostic logging is enabled via RUSTICLE_DEBUG env var.
fn debug_enabled() -> bool {
    static DEBUG: OnceLock<bool> = OnceLock::new();
    *DEBUG.get_or_init(|| std::env::var("RUSTICLE_DEBUG").is_ok())
}

/// Try to encode frame using pre-built palette LUT.
/// Returns None if quality is unacceptable (fallback to full quantization).
fn try_fast_encode(rgba_pixels: &[u8], lut: &PaletteLut) -> Option<(Vec<u8>, Vec<u8>)> {
    let (indices, stats) = lut.map_buffer(rgba_pixels);

    // Diagnostic logging when RUSTICLE_DEBUG is set
    if debug_enabled() {
        let accepted = stats.is_acceptable();
        eprintln!(
            "[fast_path] pixels={} accepted={} avg_dist={:.1} outliers={:.2}% util={:.2}%",
            rgba_pixels.len() / 4,
            accepted,
            stats.avg_distance_sq,
            stats.outlier_ratio * 100.0,
            stats.palette_utilization * 100.0
        );
        if !accepted {
            // Show which threshold(s) failed
            let mut reasons = Vec::new();
            if stats.avg_distance_sq >= 150.0 {
                reasons.push(format!("avg_dist {:.1} >= 150", stats.avg_distance_sq));
            }
            if stats.outlier_ratio >= 0.05 {
                reasons.push(format!(
                    "outliers {:.1}% >= 5%",
                    stats.outlier_ratio * 100.0
                ));
            }
            if stats.palette_utilization <= 0.3 {
                reasons.push(format!(
                    "util {:.1}% <= 30%",
                    stats.palette_utilization * 100.0
                ));
            }
            eprintln!("[fast_path] rejected: {}", reasons.join(", "));
        }
    }

    if !stats.is_acceptable() {
        return None; // Quality too low, fallback
    }

    // Convert palette to flat RGB format
    let palette_rgb: Vec<u8> = lut
        .palette()
        .iter()
        .flat_map(|c| c.iter().copied())
        .collect();

    Some((palette_rgb, indices))
}

/// Find transparent index and remap transparent pixels to a dedicated palette entry.
///
/// Kept local because this variant mutates and may extend the palette itself so the
/// encoder can swap entries without a separate LUT.
fn find_transparent_index_and_remap(
    rgba_pixels: &[u8],
    indices: &mut [u8],
    palette: &mut Vec<u8>,
) -> Option<u8> {
    // Check if there are any transparent pixels
    let has_transparent = rgba_pixels.chunks_exact(4).any(|p| p[3] < 128);

    if !has_transparent {
        return None;
    }

    let palette_len = palette.len() / 3;

    // Guard against empty palette
    if palette_len == 0 {
        return None;
    }

    // Count usage of each palette index by OPAQUE pixels only
    let mut opaque_usage = vec![0usize; palette_len];
    for (i, pixel) in rgba_pixels.chunks_exact(4).enumerate() {
        if pixel[3] >= 128 {
            opaque_usage[indices[i] as usize] += 1;
        }
    }

    // Prefer index 0 for transparency (GIF convention, better LZW compression)
    let transparent_idx = if opaque_usage[0] == 0 {
        // Index 0 is unused by opaque pixels - perfect!
        0
    } else {
        // Index 0 is used - find an unused index to swap with
        if let Some(unused_offset) = opaque_usage.iter().skip(1).position(|&count| count == 0) {
            let swap_idx = (unused_offset + 1) as u8; // +1 because we skipped index 0

            // Swap palette entries: palette[0] <-> palette[swap_idx]
            let swap_offset = (swap_idx as usize) * 3;
            let (r0, g0, b0) = (palette[0], palette[1], palette[2]);
            palette[0] = palette[swap_offset];
            palette[1] = palette[swap_offset + 1];
            palette[2] = palette[swap_offset + 2];
            palette[swap_offset] = r0;
            palette[swap_offset + 1] = g0;
            palette[swap_offset + 2] = b0;

            // Remap indices: 0 <-> swap_idx
            for idx in indices.iter_mut() {
                if *idx == 0 {
                    *idx = swap_idx;
                } else if *idx == swap_idx {
                    *idx = 0;
                }
            }
            0
        } else if palette_len < 256 {
            // No unused index - add new entry and swap with index 0
            let new_idx = palette_len as u8;

            // Copy index 0's color to new slot
            palette.extend_from_slice(&[palette[0], palette[1], palette[2]]);

            // Remap: all index 0 -> new_idx
            for idx in indices.iter_mut() {
                if *idx == 0 {
                    *idx = new_idx;
                }
            }
            0
        } else {
            // Full palette, all used - find least used as fallback.
            // CRITICAL: we must remap opaque pixels off this index before assigning it to transparency.
            let chosen_idx = opaque_usage
                .iter()
                .enumerate()
                .min_by_key(|(_, &count)| count)
                .map(|(idx, _)| idx as u8)
                .unwrap_or(0);

            // Find a replacement index for opaque pixels currently using chosen_idx.
            // Prefer an index with the same color (if one exists), otherwise pick the
            // second-least-used index and copy the color from chosen_idx to it.
            let chosen_offset = (chosen_idx as usize) * 3;
            let chosen_color = (
                palette[chosen_offset],
                palette[chosen_offset + 1],
                palette[chosen_offset + 2],
            );

            let replacement_idx = if let Some(idx) = (0..palette_len).find(|&idx| {
                idx != chosen_idx as usize
                    && (palette[idx * 3], palette[idx * 3 + 1], palette[idx * 3 + 2])
                        == chosen_color
            }) {
                // Found an index with the same color - use it
                idx as u8
            } else {
                // No exact color match - pick the second-least-used index and copy the color
                let replacement_idx = opaque_usage
                    .iter()
                    .enumerate()
                    .filter(|(idx, _)| *idx != chosen_idx as usize)
                    .min_by_key(|(_, &count)| count)
                    .map(|(idx, _)| idx as u8)
                    .unwrap_or(0); // Fallback to 0 if somehow all indices are the same

                // Copy the color from chosen_idx to replacement_idx so opaque pixels preserve their color
                let replacement_offset = (replacement_idx as usize) * 3;
                palette[replacement_offset] = palette[chosen_offset];
                palette[replacement_offset + 1] = palette[chosen_offset + 1];
                palette[replacement_offset + 2] = palette[chosen_offset + 2];

                replacement_idx
            };

            // Remap opaque pixels from chosen_idx to replacement_idx
            for (i, pixel) in rgba_pixels.chunks_exact(4).enumerate() {
                if pixel[3] >= 128 && indices[i] == chosen_idx {
                    indices[i] = replacement_idx;
                }
            }

            chosen_idx
        }
    };

    // Remap all transparent pixels to use the transparent index
    for (i, pixel) in rgba_pixels.chunks_exact(4).enumerate() {
        if pixel[3] < 128 {
            indices[i] = transparent_idx;
        }
    }

    Some(transparent_idx)
}

/// Quantize RGBA pixels to indexed color using imagequant or fast path.
/// Returns (palette, indices, transparent_index).
fn quantize_rgba(
    rgba_pixels: &[u8],
    width: usize,
    height: usize,
    lut: Option<&PaletteLut>,
) -> Result<(Vec<u8>, Vec<u8>, Option<u8>)> {
    if rgba_pixels.len() != width * height * 4 {
        return Err(Error::EncodeError(format!(
            "pixel data size mismatch: expected {}, got {}",
            width * height * 4,
            rgba_pixels.len()
        )));
    }

    // Try fast path if we have pre-built LUT
    if let Some(lut) = lut {
        if let Some((mut palette_rgb, mut indices)) = try_fast_encode(rgba_pixels, lut) {
            let transparent_idx =
                find_transparent_index_and_remap(rgba_pixels, &mut indices, &mut palette_rgb);
            return Ok((palette_rgb, indices, transparent_idx));
        }
    }

    #[cfg(feature = "imagequant")]
    {
        // Convert raw bytes to RGBA structs
        let rgba_data: Vec<imagequant::RGBA> = rgba_pixels
            .chunks_exact(4)
            .map(|chunk| imagequant::RGBA {
                r: chunk[0],
                g: chunk[1],
                b: chunk[2],
                a: chunk[3],
            })
            .collect();

        // Create imagequant attributes
        let mut attr = imagequant::Attributes::new();
        attr.set_max_colors(256)
            .map_err(|e| Error::EncodeError(format!("failed to set max colors: {}", e)))?;
        attr.set_quality(0, 100)
            .map_err(|e| Error::EncodeError(format!("failed to set quality: {}", e)))?;

        // Create image from RGBA pixels
        let mut img = attr
            .new_image_borrowed(&rgba_data, width, height, 0.0)
            .map_err(|e| Error::EncodeError(format!("failed to create image: {}", e)))?;

        // Quantize
        let mut result = attr
            .quantize(&mut img)
            .map_err(|e| Error::EncodeError(format!("failed to quantize: {}", e)))?;

        // Enable dithering for better visual quality
        result
            .set_dithering_level(1.0)
            .map_err(|e| Error::EncodeError(format!("failed to set dithering: {}", e)))?;

        // Remap pixels to palette indices
        let (palette, mut indices) = result
            .remapped(&mut img)
            .map_err(|e| Error::EncodeError(format!("failed to remap: {}", e)))?;

        // Convert palette to flat RGB format
        let mut palette_rgb = Vec::with_capacity(palette.len() * 3);
        for color in palette {
            palette_rgb.push(color.r);
            palette_rgb.push(color.g);
            palette_rgb.push(color.b);
        }

        // Handle transparency - find/create dedicated index for transparent pixels
        let transparent_idx =
            find_transparent_index_and_remap(rgba_pixels, &mut indices, &mut palette_rgb);

        Ok((palette_rgb, indices, transparent_idx))
    }

    #[cfg(not(feature = "imagequant"))]
    {
        let (mut palette_rgb, mut indices) =
            crate::quantize::quantize_rgba(rgba_pixels, width, height, 80);

        let transparent_idx =
            find_transparent_index_and_remap(rgba_pixels, &mut indices, &mut palette_rgb);

        Ok((palette_rgb, indices, transparent_idx))
    }
}

/// Quantize RGBA pixels to indexed color with tracking of fast path usage.
/// Returns (palette, indices, transparent_index, used_fast_path).
fn quantize_rgba_with_stats(
    rgba_pixels: &[u8],
    width: usize,
    height: usize,
    lut: Option<&PaletteLut>,
) -> Result<QuantizeResult> {
    if rgba_pixels.len() != width * height * 4 {
        return Err(Error::EncodeError(format!(
            "pixel data size mismatch: expected {}, got {}",
            width * height * 4,
            rgba_pixels.len()
        )));
    }

    // Try fast path if we have pre-built LUT
    if let Some(lut) = lut {
        if let Some((mut palette_rgb, mut indices)) = try_fast_encode(rgba_pixels, lut) {
            let transparent_idx =
                find_transparent_index_and_remap(rgba_pixels, &mut indices, &mut palette_rgb);
            return Ok((palette_rgb, indices, transparent_idx, true));
        }
    }

    #[cfg(feature = "imagequant")]
    {
        // Convert raw bytes to RGBA structs
        let rgba_data: Vec<imagequant::RGBA> = rgba_pixels
            .chunks_exact(4)
            .map(|chunk| imagequant::RGBA {
                r: chunk[0],
                g: chunk[1],
                b: chunk[2],
                a: chunk[3],
            })
            .collect();

        // Create imagequant attributes
        let mut attr = imagequant::Attributes::new();
        attr.set_max_colors(256)
            .map_err(|e| Error::EncodeError(format!("failed to set max colors: {}", e)))?;
        attr.set_quality(0, 100)
            .map_err(|e| Error::EncodeError(format!("failed to set quality: {}", e)))?;

        // Create image from RGBA pixels
        let mut img = attr
            .new_image_borrowed(&rgba_data, width, height, 0.0)
            .map_err(|e| Error::EncodeError(format!("failed to create image: {}", e)))?;

        // Quantize
        let mut result = attr
            .quantize(&mut img)
            .map_err(|e| Error::EncodeError(format!("failed to quantize: {}", e)))?;

        // Enable dithering for better visual quality
        result
            .set_dithering_level(1.0)
            .map_err(|e| Error::EncodeError(format!("failed to set dithering: {}", e)))?;

        // Remap pixels to palette indices
        let (palette, mut indices) = result
            .remapped(&mut img)
            .map_err(|e| Error::EncodeError(format!("failed to remap: {}", e)))?;

        // Convert palette to flat RGB format
        let mut palette_rgb = Vec::with_capacity(palette.len() * 3);
        for color in palette {
            palette_rgb.push(color.r);
            palette_rgb.push(color.g);
            palette_rgb.push(color.b);
        }

        // Handle transparency - find/create dedicated index for transparent pixels
        let transparent_idx =
            find_transparent_index_and_remap(rgba_pixels, &mut indices, &mut palette_rgb);

        Ok((palette_rgb, indices, transparent_idx, false))
    }

    #[cfg(not(feature = "imagequant"))]
    {
        let (mut palette_rgb, mut indices) =
            crate::quantize::quantize_rgba(rgba_pixels, width, height, 80);

        let transparent_idx =
            find_transparent_index_and_remap(rgba_pixels, &mut indices, &mut palette_rgb);

        Ok((palette_rgb, indices, transparent_idx, false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quantize_rgba_valid_data() {
        let rgba_pixels = vec![
            255, 0, 0, 255, // Red
            0, 255, 0, 255, // Green
            0, 0, 255, 255, // Blue
            255, 255, 0, 255, // Yellow
        ];

        let result = quantize_rgba(&rgba_pixels, 2, 2, None);
        assert!(result.is_ok(), "Should quantize valid RGBA data");
        let (palette, indexed, _) = result.unwrap();
        assert!(!palette.is_empty(), "Palette should not be empty");
        assert_eq!(indexed.len(), 4, "Indexed data should have 4 pixels");
    }

    #[test]
    fn test_quantize_rgba_invalid_size() {
        let rgba_pixels = vec![255, 0, 0, 255]; // Only 1 pixel

        let result = quantize_rgba(&rgba_pixels, 2, 2, None); // Expects 4 pixels
        assert!(result.is_err(), "Should fail with size mismatch");
    }

    #[test]
    fn test_quantize_rgba_with_transparency() {
        let rgba_pixels = vec![
            255, 0, 0, 255, // Red, opaque
            0, 255, 0, 255, // Green, opaque
            0, 0, 255, 0, // Blue, transparent
            255, 255, 0, 255, // Yellow, opaque
        ];

        let result = quantize_rgba(&rgba_pixels, 2, 2, None);
        assert!(result.is_ok());
        let (_, indices, transparent_idx) = result.unwrap();

        // Should have a transparent index
        assert!(transparent_idx.is_some(), "Should have transparent index");
        let trans_idx = transparent_idx.unwrap();

        // The transparent pixel (index 2) should use the transparent index
        assert_eq!(
            indices[2], trans_idx,
            "Transparent pixel should use transparent index"
        );

        // Opaque pixels should NOT use the transparent index
        assert_ne!(
            indices[0], trans_idx,
            "Opaque pixel should not use transparent index"
        );
        assert_ne!(
            indices[1], trans_idx,
            "Opaque pixel should not use transparent index"
        );
        assert_ne!(
            indices[3], trans_idx,
            "Opaque pixel should not use transparent index"
        );
    }

    #[test]
    fn test_quantize_rgba_no_transparency() {
        let rgba_pixels = vec![
            255, 0, 0, 255, // Red, opaque
            0, 255, 0, 255, // Green, opaque
            0, 0, 255, 255, // Blue, opaque
            255, 255, 0, 255, // Yellow, opaque
        ];

        let result = quantize_rgba(&rgba_pixels, 2, 2, None);
        assert!(result.is_ok());
        let (_, _, transparent_idx) = result.unwrap();

        // Should NOT have a transparent index
        assert!(
            transparent_idx.is_none(),
            "Should not have transparent index"
        );
    }

    #[test]
    fn test_find_transparent_index_prefers_zero() {
        // Test that index 0 is preferred when unused by opaque pixels
        let rgba_pixels = vec![
            255, 0, 0, 255, // Red, opaque
            0, 255, 0, 255, // Green, opaque
            0, 0, 255, 0, // Blue, transparent
            255, 255, 0, 255, // Yellow, opaque
        ];

        // Create a palette where index 0 is unused by opaque pixels
        let mut palette = vec![
            100, 100, 100, // Index 0 - unused by opaque
            255, 0, 0, // Index 1 - red
            0, 255, 0, // Index 2 - green
            255, 255, 0, // Index 3 - yellow
        ];
        let mut indices = vec![1, 2, 3, 3]; // Opaque pixels use indices 1,2,3

        let result = find_transparent_index_and_remap(&rgba_pixels, &mut indices, &mut palette);

        // Should prefer index 0 for transparency
        assert_eq!(result, Some(0), "Should prefer index 0 for transparency");
        // Transparent pixel should be remapped to 0
        assert_eq!(indices[2], 0, "Transparent pixel should use index 0");
    }

    #[test]
    fn test_find_transparent_index_swaps_when_zero_used() {
        // Test that palette is swapped when index 0 is used by opaque pixels
        let rgba_pixels = vec![
            255, 0, 0, 255, // Red, opaque
            0, 255, 0, 255, // Green, opaque
            0, 0, 255, 0, // Blue, transparent
            255, 255, 0, 255, // Yellow, opaque
        ];

        // Create a palette where index 0 IS used by opaque pixels
        let mut palette = vec![
            255, 0, 0, // Index 0 - red (used by opaque)
            100, 100, 100, // Index 1 - unused
            0, 255, 0, // Index 2 - green
            255, 255, 0, // Index 3 - yellow
        ];
        let mut indices = vec![0, 2, 3, 3]; // Opaque pixels use indices 0,2,3

        let result = find_transparent_index_and_remap(&rgba_pixels, &mut indices, &mut palette);

        // Should still prefer index 0 for transparency (after swapping)
        assert_eq!(result, Some(0), "Should prefer index 0 for transparency");
        // Transparent pixel should be remapped to 0
        assert_eq!(indices[2], 0, "Transparent pixel should use index 0");
        // Index 0 should now have the color that was at index 1 (or swapped)
        // The important thing is that opaque pixels were remapped correctly
        assert_eq!(indices[0], 1, "Opaque pixel at index 0 should be remapped");
    }

    /// Build a 256-color flat-RGB palette where every entry is a distinct opaque color.
    /// Colors are spread across the RGB cube so none collide.
    fn make_full_256_palette() -> Vec<u8> {
        let mut palette = Vec::with_capacity(256 * 3);
        for i in 0u8..=255 {
            // Spread across the cube: R cycles fast, G mid, B slow
            palette.push(i); // R
            palette.push(i / 4); // G
            palette.push(i / 16); // B
        }
        palette
    }

    /// Regression test: full 256-color palette, all indices used by opaque pixels.
    ///
    /// Bug: `find_transparent_index_and_remap` falls into the "full palette" branch and
    /// picks the least-used index as `transparent_idx` WITHOUT remapping the opaque pixels
    /// that were already using that index.  After the call those opaque pixels still carry
    /// the chosen index, so the GIF decoder treats them as transparent — silent corruption.
    ///
    /// Correct behaviour: every opaque pixel that was using the chosen transparent index
    /// must be remapped to a *different* index (or the palette must be extended / the
    /// function must refuse to clobber opaque pixels).
    #[test]
    fn test_find_transparent_index_full_palette_opaque_pixels_not_clobbered() {
        // --- Build a 256-entry palette (all slots occupied by opaque colors) ---
        let mut palette = make_full_256_palette();

        // --- Build pixel data: 256 opaque pixels (one per palette entry) + 1 transparent ---
        // Layout: pixels 0..255 are opaque, each using its own palette index.
        //         pixel 256 is transparent (alpha = 0).
        let pixel_count = 257usize;
        let mut rgba_pixels = Vec::with_capacity(pixel_count * 4);
        for i in 0u8..=255 {
            rgba_pixels.extend_from_slice(&[i, i / 4, i / 16, 255]); // opaque
        }
        rgba_pixels.extend_from_slice(&[0, 0, 0, 0]); // transparent pixel

        // Indices: pixel i → palette index i (for i in 0..256), transparent pixel → 0
        let mut indices: Vec<u8> = (0u8..=255).collect();
        indices.push(0); // transparent pixel initially mapped to index 0

        // --- Call the function under test ---
        let transparent_idx =
            find_transparent_index_and_remap(&rgba_pixels, &mut indices, &mut palette)
                .expect("should return Some when transparent pixels are present");

        // --- Invariant 1: the transparent pixel must use the designated transparent index ---
        assert_eq!(
            indices[256], transparent_idx,
            "transparent pixel must be mapped to the transparent index"
        );

        // --- Invariant 2: NO opaque pixel may use the transparent index ---
        // This is the invariant that the bug violates: opaque pixels that happened to
        // use the chosen index are left with it, making them appear transparent.
        for pixel_pos in 0..256usize {
            let alpha = rgba_pixels[pixel_pos * 4 + 3];
            if alpha >= 128 {
                assert_ne!(
                    indices[pixel_pos], transparent_idx,
                    "opaque pixel at position {pixel_pos} must NOT use the transparent index \
                     ({transparent_idx}); it would be rendered as transparent (bug)"
                );
            }
        }
    }

    /// Complementary check: when the full-palette branch is taken, opaque pixels that
    /// were remapped must not use the transparent index. Color preservation is best-effort:
    /// if the palette has duplicate colors, the original color is preserved; otherwise,
    /// the palette entry at the new index is overwritten with the original color.
    #[test]
    fn test_find_transparent_index_full_palette_opaque_colors_best_effort() {
        let mut palette = make_full_256_palette();

        // Snapshot original palette colors before mutation
        let original_palette = palette.clone();

        // 256 opaque pixels (one per index) + 1 transparent
        let mut rgba_pixels = Vec::with_capacity(257 * 4);
        for i in 0u8..=255 {
            rgba_pixels.extend_from_slice(&[i, i / 4, i / 16, 255]);
        }
        rgba_pixels.extend_from_slice(&[0, 0, 0, 0]);

        let mut indices: Vec<u8> = (0u8..=255).collect();
        indices.push(0);

        let transparent_idx =
            find_transparent_index_and_remap(&rgba_pixels, &mut indices, &mut palette)
                .expect("should return Some");

        // For every opaque pixel:
        // 1. It must NOT use the transparent index (primary invariant)
        // 2. Its color should be preserved if possible (best-effort)
        for pixel_pos in 0..256usize {
            let orig_idx = pixel_pos as u8; // pixel i originally used index i
            let new_idx = indices[pixel_pos] as usize;

            // Primary invariant: no opaque pixel uses the transparent index
            assert_ne!(
                new_idx as u8, transparent_idx,
                "opaque pixel at position {pixel_pos} must not use transparent index"
            );

            let orig_r = original_palette[orig_idx as usize * 3];
            let orig_g = original_palette[orig_idx as usize * 3 + 1];
            let orig_b = original_palette[orig_idx as usize * 3 + 2];

            let new_r = palette[new_idx * 3];
            let new_g = palette[new_idx * 3 + 1];
            let new_b = palette[new_idx * 3 + 2];

            // Secondary: color preservation is best-effort. If the pixel was remapped
            // (new_idx != orig_idx), the palette entry at new_idx should have been
            // updated to match the original color (or it already had that color).
            if new_idx != orig_idx as usize {
                // Pixel was remapped - the palette should have been updated
                assert_eq!(
                    (new_r, new_g, new_b),
                    (orig_r, orig_g, orig_b),
                    "opaque pixel at position {pixel_pos} was remapped from index {orig_idx} \
                     to {new_idx}; palette entry should have been updated to preserve color"
                );
            }
        }
    }

    /// Edge-case: exactly one transparent pixel, palette fully saturated, the
    /// least-used opaque index has exactly 1 user.  The single opaque pixel that
    /// shared that chosen transparent index must be remapped, not silently turned
    /// transparent.
    ///
    /// Construction: give index 0 only ONE opaque user so `min_by_key` picks it as
    /// `transparent_idx`, then verify that one opaque pixel is not left pointing at 0.
    #[test]
    fn test_find_transparent_index_full_palette_single_opaque_user_remapped() {
        // Palette: 256 entries, all distinct
        let mut palette = make_full_256_palette();

        // We want index 0 to be the least-used (1 user) so the buggy branch picks it.
        // Give every other index 2 opaque users, index 0 exactly 1 opaque user.
        let mut rgba_pixels: Vec<u8> = Vec::new();
        let mut indices: Vec<u8> = Vec::new();

        // Index 0: exactly ONE opaque pixel (the victim the bug will clobber)
        rgba_pixels.extend_from_slice(&[0, 0, 0, 255]); // opaque, index 0
        let opaque_idx0_pos = indices.len();
        indices.push(0u8);

        // Indices 1..=255: two opaque pixels each (so index 0 is the minimum)
        for i in 1u8..=255 {
            rgba_pixels.extend_from_slice(&[i, i / 4, i / 16, 255]);
            indices.push(i);
            rgba_pixels.extend_from_slice(&[i, i / 4, i / 16, 255]);
            indices.push(i);
        }

        // One transparent pixel
        rgba_pixels.extend_from_slice(&[0, 0, 0, 0]);
        let transparent_pos = indices.len();
        indices.push(255u8); // initial index doesn't matter

        let transparent_idx =
            find_transparent_index_and_remap(&rgba_pixels, &mut indices, &mut palette)
                .expect("should return Some");

        // The buggy branch picks index 0 (least used) as transparent_idx.
        // Verify the invariant: the opaque pixel that was at index 0 must NOT
        // still be pointing at transparent_idx after the call.
        assert_eq!(
            indices[transparent_pos], transparent_idx,
            "transparent pixel must use transparent index"
        );

        assert_ne!(
            indices[opaque_idx0_pos], transparent_idx,
            "the single opaque pixel that used index 0 (chosen as transparent_idx={transparent_idx}) \
             must be remapped off it — leaving it there makes it appear transparent (bug)"
        );
    }
}

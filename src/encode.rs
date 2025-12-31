use crate::error::{Error, Result};
use crate::palette_lut::PaletteLut;
use crate::types::{DisposalMethod, Gif};
use std::io::Write;

impl Gif {
    /// Encode to byte vector.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        self.encode_to(&mut buf)?;
        Ok(buf)
    }

    /// Encode to byte vector, consuming self.
    ///
    /// Convenience method for chaining operations that returns owned bytes.
    ///
    /// # Example
    /// ```ignore
    /// let gif = Gif::from_bytes(&data)?;
    /// let bytes = gif
    ///     .resize(640, 480, Filter::Lanczos3)?
    ///     .optimize(OptLevel::O3)
    ///     .lossy(80)
    ///     .into_bytes()?;
    /// ```
    pub fn into_bytes(self) -> Result<Vec<u8>> {
        self.to_bytes()
    }

    /// Encode to any Write implementation.
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

        // Encode each frame
        for frame in &self.frames {
            encode_frame(&mut encoder, frame, lut_ref)?;
        }

        Ok(())
    }
}

fn encode_frame<W: Write>(
    encoder: &mut gif::Encoder<W>,
    frame: &crate::types::Frame,
    lut: Option<&PaletteLut>,
) -> Result<()> {
    // Quantize RGBA to indexed color
    let (palette, indexed, transparent_idx) = quantize_rgba(
        &frame.pixels,
        frame.width as usize,
        frame.height as usize,
        lut,
    )?;

    // Convert delay from Duration to gif units (10ms increments)
    let delay_ms = frame.delay.as_millis() as u16;
    let delay_units = (delay_ms + 5) / 10; // Round to nearest 10ms unit

    // Map disposal method
    let disposal = match frame.dispose {
        DisposalMethod::None => gif::DisposalMethod::Any,
        DisposalMethod::Keep => gif::DisposalMethod::Keep,
        DisposalMethod::Background => gif::DisposalMethod::Background,
        DisposalMethod::Previous => gif::DisposalMethod::Previous,
    };

    // Create gif frame
    let mut gif_frame = gif::Frame::from_indexed_pixels(
        frame.width,
        frame.height,
        indexed,
        Some(palette.len() as u8),
    );

    // Set the palette on the frame
    gif_frame.palette = Some(palette);

    // Set transparent index if we have transparent pixels
    if let Some(idx) = transparent_idx {
        gif_frame.transparent = Some(idx);
    }

    gif_frame.delay = delay_units;
    gif_frame.dispose = disposal;
    gif_frame.left = frame.left;
    gif_frame.top = frame.top;

    encoder
        .write_frame(&gif_frame)
        .map_err(|e| Error::EncodeError(format!("failed to write frame: {}", e)))?;

    Ok(())
}

/// Try to encode frame using pre-built palette LUT.
/// Returns None if quality is unacceptable (fallback to full quantization).
fn try_fast_encode(rgba_pixels: &[u8], lut: &PaletteLut) -> Option<(Vec<u8>, Vec<u8>)> {
    let (indices, stats) = lut.map_buffer(rgba_pixels);

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

/// Find transparent index by looking for low-alpha pixels in source.
fn find_transparent_index(rgba_pixels: &[u8], indices: &[u8], palette: &[[u8; 3]]) -> Option<u8> {
    // Look for pixels with alpha < 128 in the source
    let mut transparent_pixels = std::collections::HashSet::new();
    for (i, pixel) in rgba_pixels.chunks_exact(4).enumerate() {
        if pixel[3] < 128 {
            transparent_pixels.insert(indices[i]);
        }
    }

    // If we found transparent pixels, pick the least-used palette color
    if !transparent_pixels.is_empty() {
        // Count usage of each palette color
        let mut color_usage = vec![0usize; palette.len()];
        for &idx in indices {
            color_usage[idx as usize] += 1;
        }

        // Find least-used color among transparent pixels
        transparent_pixels
            .iter()
            .min_by_key(|&&idx| color_usage[idx as usize])
            .copied()
    } else {
        None
    }
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
        if let Some((palette_rgb, indices)) = try_fast_encode(rgba_pixels, lut) {
            // Find transparent index (look for low-alpha pixels in source)
            let transparent_idx = find_transparent_index(rgba_pixels, &indices, lut.palette());
            return Ok((palette_rgb, indices, transparent_idx));
        }
    }

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
    let (palette, indices) = result
        .remapped(&mut img)
        .map_err(|e| Error::EncodeError(format!("failed to remap: {}", e)))?;

    // Find transparent color index (alpha < 128)
    let mut transparent_idx = None;
    for (i, color) in palette.iter().enumerate() {
        if color.a < 128 {
            transparent_idx = Some(i as u8);
            break;
        }
    }

    // Convert palette to flat RGB format (3 bytes per color)
    let mut palette_rgb = Vec::with_capacity(palette.len() * 3);
    for color in palette {
        palette_rgb.push(color.r);
        palette_rgb.push(color.g);
        palette_rgb.push(color.b);
    }

    Ok((palette_rgb, indices, transparent_idx))
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
    fn test_quantize_rgba_size_mismatch() {
        let rgba_pixels = vec![255, 0, 0, 255, 0, 255]; // Only 6 bytes, not 4*4=16
        let result = quantize_rgba(&rgba_pixels, 2, 2, None);
        assert!(result.is_err(), "Should error on size mismatch");
    }

    #[test]
    fn test_quantize_rgba_single_pixel() {
        let rgba_pixels = vec![128, 64, 32, 255];
        let result = quantize_rgba(&rgba_pixels, 1, 1, None);
        assert!(result.is_ok(), "Should quantize single pixel");
        let (palette, indexed, _) = result.unwrap();
        assert!(!palette.is_empty(), "Palette should not be empty");
        assert_eq!(indexed.len(), 1, "Indexed data should have 1 pixel");
    }

    #[test]
    fn test_quantize_rgba_large_image() {
        let mut rgba_pixels = Vec::new();
        // Create 100 pixels (10x10)
        for i in 0..100 {
            rgba_pixels.push((i % 256) as u8);
            rgba_pixels.push(((i * 2) % 256) as u8);
            rgba_pixels.push(((i * 3) % 256) as u8);
            rgba_pixels.push(255);
        }
        let result = quantize_rgba(&rgba_pixels, 10, 10, None);
        assert!(result.is_ok(), "Should quantize large image");
        let (palette, indexed, _) = result.unwrap();
        assert!(!palette.is_empty(), "Palette should not be empty");
        assert_eq!(indexed.len(), 100, "Indexed data should have 100 pixels");
    }

    #[test]
    fn test_quantize_rgba_with_transparency() {
        let rgba_pixels = vec![
            255, 0, 0, 255, // Red (opaque)
            0, 255, 0, 0, // Green (fully transparent)
            0, 0, 255, 255, // Blue (opaque)
            255, 255, 0, 0, // Yellow (fully transparent)
        ];

        let result = quantize_rgba(&rgba_pixels, 2, 2, None);
        assert!(result.is_ok(), "Should quantize data with transparency");
        let (palette, indexed, transparent_idx) = result.unwrap();
        assert!(!palette.is_empty(), "Palette should not be empty");
        assert_eq!(indexed.len(), 4, "Indexed data should have 4 pixels");
        assert!(
            transparent_idx.is_some(),
            "Should have transparent index for transparent pixels"
        );
    }
}

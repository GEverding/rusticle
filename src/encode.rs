use crate::error::{Error, Result};
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

        // Encode each frame
        for frame in &self.frames {
            encode_frame(&mut encoder, frame)?;
        }

        Ok(())
    }
}

fn encode_frame<W: Write>(
    encoder: &mut gif::Encoder<W>,
    frame: &crate::types::Frame,
) -> Result<()> {
    // Quantize RGBA to indexed color
    let (palette, indexed) =
        quantize_rgba(&frame.pixels, frame.width as usize, frame.height as usize)?;

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

    gif_frame.delay = delay_units;
    gif_frame.dispose = disposal;
    gif_frame.left = frame.left;
    gif_frame.top = frame.top;

    encoder
        .write_frame(&gif_frame)
        .map_err(|e| Error::EncodeError(format!("failed to write frame: {}", e)))?;

    Ok(())
}

/// Quantize RGBA pixels to indexed color using color_quant.
fn quantize_rgba(rgba_pixels: &[u8], width: usize, height: usize) -> Result<(Vec<u8>, Vec<u8>)> {
    if rgba_pixels.len() != width * height * 4 {
        return Err(Error::EncodeError(format!(
            "pixel data size mismatch: expected {}, got {}",
            width * height * 4,
            rgba_pixels.len()
        )));
    }

    // Quantize to 256 colors using RGBA pixels directly
    let nq = color_quant::NeuQuant::new(10, 256, rgba_pixels);
    let palette = nq.color_map_rgb().to_vec();

    // Map each pixel to its palette index
    let mut indexed = Vec::with_capacity(width * height);
    for chunk in rgba_pixels.chunks_exact(4) {
        let idx = nq.index_of(chunk);
        indexed.push(idx as u8);
    }

    Ok((palette, indexed))
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

        let result = quantize_rgba(&rgba_pixels, 2, 2);
        assert!(result.is_ok(), "Should quantize valid RGBA data");
        let (palette, indexed) = result.unwrap();
        assert!(!palette.is_empty(), "Palette should not be empty");
        assert_eq!(indexed.len(), 4, "Indexed data should have 4 pixels");
    }

    #[test]
    fn test_quantize_rgba_size_mismatch() {
        let rgba_pixels = vec![255, 0, 0, 255, 0, 255]; // Only 6 bytes, not 4*4=16
        let result = quantize_rgba(&rgba_pixels, 2, 2);
        assert!(result.is_err(), "Should error on size mismatch");
    }

    #[test]
    fn test_quantize_rgba_single_pixel() {
        let rgba_pixels = vec![128, 64, 32, 255];
        let result = quantize_rgba(&rgba_pixels, 1, 1);
        assert!(result.is_ok(), "Should quantize single pixel");
        let (palette, indexed) = result.unwrap();
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
        let result = quantize_rgba(&rgba_pixels, 10, 10);
        assert!(result.is_ok(), "Should quantize large image");
        let (palette, indexed) = result.unwrap();
        assert!(!palette.is_empty(), "Palette should not be empty");
        assert_eq!(indexed.len(), 100, "Indexed data should have 100 pixels");
    }
}

//! Color quantization internals.

use crate::quantize::dither::{dither_floyd_steinberg_serpentine, dither_ordered};
use crate::quantize::kmeans::{expand_palette_with_farthest_points, refine_palette};
use crate::quantize::wu::generate_palette;

pub(crate) mod dither;
pub(crate) mod kmeans;
pub(crate) mod wu;

/// Quantize RGBA pixels to an indexed palette.
///
/// Returns `(palette_rgb_flat, indices)`.
pub(crate) fn quantize_rgba(
    rgba_pixels: &[u8],
    width: usize,
    height: usize,
    quality: u8,
) -> (Vec<u8>, Vec<u8>) {
    if rgba_pixels.is_empty() || width == 0 || height == 0 {
        return (Vec::new(), Vec::new());
    }

    let has_transparency = rgba_pixels.chunks_exact(4).any(|px| px[3] < 128);
    let max_colors = if has_transparency { 255 } else { 256 };
    let initial_palette = generate_palette(rgba_pixels, max_colors);
    let expanded_palette = if initial_palette.len() < max_colors {
        expand_palette_with_farthest_points(rgba_pixels, &initial_palette, max_colors)
    } else {
        initial_palette
    };
    let iterations = match quality {
        0..=30 => 0,
        31..=70 => 1,
        _ => 4,
    };

    let palette = refine_palette(rgba_pixels, &expanded_palette, iterations);
    let indices = match quality {
        0..=70 => dither_ordered(&palette, rgba_pixels, width, height, quality as f32 / 200.0),
        _ => dither_floyd_steinberg_serpentine(&palette, rgba_pixels, width, height),
    };

    (palette.to_flat_rgb(), indices)
}

/// Derive a 256-color palette from RGBA pixels.
pub(crate) fn derive_palette(rgba_pixels: &[u8]) -> Vec<u8> {
    if rgba_pixels.is_empty() {
        return Vec::new();
    }

    let initial_palette = generate_palette(rgba_pixels, 256);
    refine_palette(rgba_pixels, &initial_palette, 1).to_flat_rgb()
}

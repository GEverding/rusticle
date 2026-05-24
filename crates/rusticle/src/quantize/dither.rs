//! Dithering strategies for palette quantization.

#[cfg(test)]
use crate::quantize::kmeans::map_pixels;
use crate::quantize::kmeans::{nearest_color, PaletteSoA};
use crate::quantize::OPAQUE_ALPHA_THRESHOLD;

/// 8×8 Bayer threshold matrix normalized to [-0.5, +0.5).
const BAYER_8X8: [[f32; 8]; 8] = [
    [
        -0.4921875, 0.2578125, -0.3046875, 0.4453125, -0.4453125, 0.3046875, -0.2578125, 0.4921875,
    ],
    [
        0.0078125, -0.2421875, 0.1953125, -0.0546875, 0.0546875, -0.1953125, 0.2421875, -0.0078125,
    ],
    [
        -0.3671875, 0.3828125, -0.4296875, 0.3203125, -0.3203125, 0.4296875, -0.3828125, 0.3671875,
    ],
    [
        0.1328125, -0.1171875, 0.0703125, -0.1796875, 0.1796875, -0.0703125, 0.1484375, -0.1328125,
    ],
    [
        -0.4609375, 0.2890625, -0.2734375, 0.4765625, -0.4765625, 0.2734375, -0.2890625, 0.4609375,
    ],
    [
        0.0390625, -0.2109375, 0.2265625, -0.0234375, 0.0234375, -0.2265625, 0.2109375, -0.0390625,
    ],
    [
        -0.3359375, 0.4140625, -0.3984375, 0.3515625, -0.3515625, 0.3984375, -0.4140625, 0.3359375,
    ],
    [
        0.1640625, -0.0859375, 0.1015625, -0.1484375, 0.1484375, -0.1015625, 0.0859375, -0.1640625,
    ],
];

#[inline]
fn clamp_to_u8(value: f32) -> i16 {
    value.clamp(0.0, 255.0).round() as i16
}

#[inline]
fn add_error(dst: &mut [f32; 3], delta_r: f32, delta_g: f32, delta_b: f32) {
    dst[0] += delta_r;
    dst[1] += delta_g;
    dst[2] += delta_b;
}

/// Apply ordered (Bayer) dithering to RGBA pixels against a palette.
///
/// `strength`: dithering intensity, 0.0 = no dither, 1.0 = full dither.
/// Typical: 0.5 for GIF (balances quality vs LZW compression).
///
/// Returns palette indices, one per pixel.
#[must_use]
pub(crate) fn dither_ordered(
    palette: &PaletteSoA,
    rgba_pixels: &[u8],
    width: usize,
    height: usize,
    strength: f32,
) -> Vec<u8> {
    if rgba_pixels.is_empty() || width == 0 || height == 0 {
        return Vec::new();
    }

    let mut indices = Vec::with_capacity(width.saturating_mul(height));
    let spread = 32.0 * strength;

    for (i, px) in rgba_pixels.chunks_exact(4).enumerate() {
        if px[3] < OPAQUE_ALPHA_THRESHOLD {
            indices.push(0);
            continue;
        }

        let x = i % width;
        let y = i / width;
        let threshold = BAYER_8X8[y & 7][x & 7] * spread;

        let r = clamp_to_u8(f32::from(px[0]) + threshold);
        let g = clamp_to_u8(f32::from(px[1]) + threshold);
        let b = clamp_to_u8(f32::from(px[2]) + threshold);

        indices.push(nearest_color(palette, r, g, b) as u8);
    }

    indices
}

/// Apply Floyd-Steinberg error-diffusion dithering.
///
/// Returns palette indices, one per pixel.
#[cfg(test)]
#[must_use]
pub(crate) fn dither_floyd_steinberg(
    palette: &PaletteSoA,
    rgba_pixels: &[u8],
    width: usize,
    height: usize,
) -> Vec<u8> {
    if rgba_pixels.is_empty() || width == 0 || height == 0 {
        return Vec::new();
    }

    let pixel_count = width.saturating_mul(height);
    let mut errors = vec![[0.0_f32; 3]; pixel_count];

    for (i, px) in rgba_pixels.chunks_exact(4).enumerate().take(pixel_count) {
        errors[i] = [f32::from(px[0]), f32::from(px[1]), f32::from(px[2])];
    }

    let mut indices = Vec::with_capacity(pixel_count);

    for y in 0..height {
        for x in 0..width {
            let i = y * width + x;

            let px = &rgba_pixels[i * 4..i * 4 + 4];
            if px[3] < OPAQUE_ALPHA_THRESHOLD {
                indices.push(0);
                continue;
            }

            let [r, g, b] = errors[i];
            let r = clamp_to_u8(r);
            let g = clamp_to_u8(g);
            let b = clamp_to_u8(b);
            let idx = nearest_color(palette, r, g, b);
            indices.push(idx as u8);

            let err_r = f32::from(r) - f32::from(palette.r[idx]);
            let err_g = f32::from(g) - f32::from(palette.g[idx]);
            let err_b = f32::from(b) - f32::from(palette.b[idx]);

            if x + 1 < width {
                let j = i + 1;
                add_error(
                    &mut errors[j],
                    err_r * (7.0 / 16.0),
                    err_g * (7.0 / 16.0),
                    err_b * (7.0 / 16.0),
                );
            }

            if y + 1 < height {
                let row = (y + 1) * width;
                if x > 0 {
                    let j = row + x - 1;
                    add_error(
                        &mut errors[j],
                        err_r * (3.0 / 16.0),
                        err_g * (3.0 / 16.0),
                        err_b * (3.0 / 16.0),
                    );
                }

                let j = row + x;
                add_error(
                    &mut errors[j],
                    err_r * (5.0 / 16.0),
                    err_g * (5.0 / 16.0),
                    err_b * (5.0 / 16.0),
                );

                if x + 1 < width {
                    let j = row + x + 1;
                    add_error(
                        &mut errors[j],
                        err_r * (1.0 / 16.0),
                        err_g * (1.0 / 16.0),
                        err_b * (1.0 / 16.0),
                    );
                }
            }
        }
    }

    indices
}

/// Apply Floyd-Steinberg error-diffusion dithering with serpentine scanlines.
///
/// Returns palette indices, one per pixel.
#[must_use]
pub(crate) fn dither_floyd_steinberg_serpentine(
    palette: &PaletteSoA,
    rgba_pixels: &[u8],
    width: usize,
    height: usize,
) -> Vec<u8> {
    if rgba_pixels.is_empty() || width == 0 || height == 0 {
        return Vec::new();
    }

    let pixel_count = width.saturating_mul(height);
    let mut errors = vec![[0.0_f32; 3]; pixel_count];

    for (i, px) in rgba_pixels.chunks_exact(4).enumerate().take(pixel_count) {
        errors[i] = [f32::from(px[0]), f32::from(px[1]), f32::from(px[2])];
    }

    let mut indices = vec![0_u8; pixel_count];

    for y in 0..height {
        let left_to_right = y & 1 == 0;
        if left_to_right {
            for x in 0..width {
                let i = y * width + x;
                let px = &rgba_pixels[i * 4..i * 4 + 4];
                if px[3] < OPAQUE_ALPHA_THRESHOLD {
                    continue;
                }

                let [r, g, b] = errors[i];
                let r = clamp_to_u8(r);
                let g = clamp_to_u8(g);
                let b = clamp_to_u8(b);
                let idx = nearest_color(palette, r, g, b);
                indices[i] = idx as u8;

                let err_r = f32::from(r) - f32::from(palette.r[idx]);
                let err_g = f32::from(g) - f32::from(palette.g[idx]);
                let err_b = f32::from(b) - f32::from(palette.b[idx]);

                if x + 1 < width {
                    add_error(
                        &mut errors[i + 1],
                        err_r * (7.0 / 16.0),
                        err_g * (7.0 / 16.0),
                        err_b * (7.0 / 16.0),
                    );
                }

                if y + 1 < height {
                    let row = (y + 1) * width;
                    if x > 0 {
                        add_error(
                            &mut errors[row + x - 1],
                            err_r * (3.0 / 16.0),
                            err_g * (3.0 / 16.0),
                            err_b * (3.0 / 16.0),
                        );
                    }

                    add_error(
                        &mut errors[row + x],
                        err_r * (5.0 / 16.0),
                        err_g * (5.0 / 16.0),
                        err_b * (5.0 / 16.0),
                    );

                    if x + 1 < width {
                        add_error(
                            &mut errors[row + x + 1],
                            err_r * (1.0 / 16.0),
                            err_g * (1.0 / 16.0),
                            err_b * (1.0 / 16.0),
                        );
                    }
                }
            }
        } else {
            for x in (0..width).rev() {
                let i = y * width + x;
                let px = &rgba_pixels[i * 4..i * 4 + 4];
                if px[3] < OPAQUE_ALPHA_THRESHOLD {
                    continue;
                }

                let [r, g, b] = errors[i];
                let r = clamp_to_u8(r);
                let g = clamp_to_u8(g);
                let b = clamp_to_u8(b);
                let idx = nearest_color(palette, r, g, b);
                indices[i] = idx as u8;

                let err_r = f32::from(r) - f32::from(palette.r[idx]);
                let err_g = f32::from(g) - f32::from(palette.g[idx]);
                let err_b = f32::from(b) - f32::from(palette.b[idx]);

                if x > 0 {
                    add_error(
                        &mut errors[i - 1],
                        err_r * (7.0 / 16.0),
                        err_g * (7.0 / 16.0),
                        err_b * (7.0 / 16.0),
                    );
                }

                if y + 1 < height {
                    let row = (y + 1) * width;
                    if x + 1 < width {
                        add_error(
                            &mut errors[row + x + 1],
                            err_r * (3.0 / 16.0),
                            err_g * (3.0 / 16.0),
                            err_b * (3.0 / 16.0),
                        );
                    }

                    add_error(
                        &mut errors[row + x],
                        err_r * (5.0 / 16.0),
                        err_g * (5.0 / 16.0),
                        err_b * (5.0 / 16.0),
                    );

                    if x > 0 {
                        add_error(
                            &mut errors[row + x - 1],
                            err_r * (1.0 / 16.0),
                            err_g * (1.0 / 16.0),
                            err_b * (1.0 / 16.0),
                        );
                    }
                }
            }
        }
    }

    indices
}

/// No dithering — simple nearest-color mapping.
#[cfg(test)]
#[must_use]
pub(crate) fn dither_none(palette: &PaletteSoA, rgba_pixels: &[u8]) -> Vec<u8> {
    if rgba_pixels.is_empty() {
        return Vec::new();
    }

    let mut indices = Vec::with_capacity(rgba_pixels.len() / 4);

    for px in rgba_pixels.chunks_exact(4) {
        if px[3] < OPAQUE_ALPHA_THRESHOLD {
            indices.push(0);
            continue;
        }

        let idx = nearest_color(
            palette,
            i16::from(px[0]),
            i16::from(px[1]),
            i16::from(px[2]),
        );
        indices.push(idx as u8);
    }

    indices
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba(pixels: &[(u8, u8, u8, u8)]) -> Vec<u8> {
        let mut out = Vec::with_capacity(pixels.len() * 4);
        for &(r, g, b, a) in pixels {
            out.extend_from_slice(&[r, g, b, a]);
        }
        out
    }

    fn gradient(width: usize, height: usize) -> Vec<u8> {
        let mut pixels = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            for x in 0..width {
                let v = ((x + y * width) * 160 / (width * height - 1)) as u8;
                pixels.extend_from_slice(&[v, v, v, 255]);
            }
        }
        pixels
    }

    fn unique_count(indices: &[u8]) -> usize {
        let mut seen = [false; 256];
        let mut count = 0;
        for &idx in indices {
            let slot = &mut seen[idx as usize];
            if !*slot {
                *slot = true;
                count += 1;
            }
        }
        count
    }

    #[test]
    fn test_ordered_no_strength() {
        let palette = PaletteSoA::from_tuples(&[(0, 0, 0), (255, 255, 255)]);
        let pixels = gradient(8, 8);

        assert_eq!(
            dither_ordered(&palette, &pixels, 8, 8, 0.0),
            dither_none(&palette, &pixels)
        );
    }

    #[test]
    fn test_ordered_deterministic() {
        let palette = PaletteSoA::from_tuples(&[(0, 0, 0), (128, 128, 128), (255, 255, 255)]);
        let pixels = gradient(8, 8);

        assert_eq!(
            dither_ordered(&palette, &pixels, 8, 8, 0.5),
            dither_ordered(&palette, &pixels, 8, 8, 0.5)
        );
    }

    #[test]
    fn test_ordered_indices_valid() {
        let palette = PaletteSoA::from_tuples(&[(0, 0, 0), (120, 120, 120), (255, 255, 255)]);
        let pixels = gradient(8, 8);

        assert!(dither_ordered(&palette, &pixels, 8, 8, 0.5)
            .iter()
            .all(|&idx| usize::from(idx) < palette.len));
    }

    #[test]
    fn test_fs_indices_valid() {
        let palette = PaletteSoA::from_tuples(&[(0, 0, 0), (120, 120, 120), (255, 255, 255)]);
        let pixels = gradient(8, 8);

        assert!(dither_floyd_steinberg(&palette, &pixels, 8, 8)
            .iter()
            .all(|&idx| usize::from(idx) < palette.len));
    }

    #[test]
    fn test_fs_reduces_banding() {
        let palette = PaletteSoA::from_tuples(&[(0, 0, 0), (255, 255, 255)]);
        let pixels = rgba(&vec![(128, 128, 128, 255); 16]);
        let none = dither_none(&palette, &pixels);
        let fs = dither_floyd_steinberg(&palette, &pixels, 16, 1);

        assert!(unique_count(&fs) > unique_count(&none));
    }

    #[test]
    fn test_dither_none_matches_map_pixels() {
        let palette = PaletteSoA::from_tuples(&[(0, 0, 0), (255, 255, 255)]);
        let pixels = gradient(8, 8);

        assert_eq!(
            dither_none(&palette, &pixels),
            map_pixels(&palette, &pixels)
        );
    }

    #[test]
    fn test_transparent_preserved() {
        let palette = PaletteSoA::from_tuples(&[(0, 0, 0), (255, 255, 255)]);
        let pixels = rgba(&[(255, 0, 0, 0), (0, 255, 0, 127), (255, 255, 255, 255)]);
        let none = dither_none(&palette, &pixels);

        assert_eq!(none, vec![0, 0, 1]);
        assert!(none.iter().all(|&idx| usize::from(idx) < palette.len));
        assert_eq!(dither_ordered(&palette, &pixels, 3, 1, 0.5), vec![0, 0, 1]);
        assert_eq!(
            dither_floyd_steinberg(&palette, &pixels, 3, 1),
            vec![0, 0, 1]
        );
    }
}

//! Color quantization internals.

use crate::quantize::dither::{dither_floyd_steinberg_serpentine, dither_ordered};
use crate::quantize::kmeans::{
    expand_palette_with_farthest_points, nearest_color, refine_palette_weighted_unique, PaletteSoA,
};
use crate::quantize::wu::generate_palette;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) mod dither;
pub(crate) mod kmeans;
pub(crate) mod wu;

pub(crate) const OPAQUE_ALPHA_THRESHOLD: u8 = 128;

const QUALITY_REFINEMENT_NONE_MAX: u8 = 30;
const QUALITY_REFINEMENT_SINGLE_MIN: u8 = QUALITY_REFINEMENT_NONE_MAX + 1;
const QUALITY_REFINEMENT_SINGLE_MAX: u8 = 70;
const QUALITY_REFINEMENT_HIGH_MIN: u8 = QUALITY_REFINEMENT_SINGLE_MAX + 1;
const SEEDED_ZERO_REFINE_FREE_SLOTS: usize = 16;
const SEEDED_ZERO_REFINE_MAX_MEAN_SAMPLE_SSE: u64 = 8;
const SEEDED_ZERO_REFINE_MAX_SAMPLE_SSE: u64 = 64;
const SAMPLED_PALETTE_ERROR_SAMPLE_LIMIT: usize = 1024;
const DITHER_MIN_SAMPLES: u64 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SampledPaletteErrorStats {
    mean_sse: u64,
    max_sse: u64,
}

struct InitialPalette {
    palette: Vec<(u8, u8, u8)>,
    deduped_seed_count: usize,
}

#[inline]
fn heuristic_refinement_iterations(quality: u8, deduped_seed_count: usize) -> u32 {
    match quality {
        0..=QUALITY_REFINEMENT_NONE_MAX => 0,
        QUALITY_REFINEMENT_SINGLE_MIN..=QUALITY_REFINEMENT_SINGLE_MAX => 1,
        _ if deduped_seed_count > 0 => {
            if deduped_seed_count <= 192 {
                1
            } else {
                3
            }
        }
        _ => 4,
    }
}

#[inline]
fn refinement_iterations(quality: u8, deduped_seed_count: usize) -> u32 {
    heuristic_refinement_iterations(quality, deduped_seed_count)
}

#[inline]
fn sampled_palette_error_stats(
    rgba_pixels: &[u8],
    palette: &PaletteSoA,
) -> Option<SampledPaletteErrorStats> {
    if palette.len == 0 {
        return None;
    }

    let stride = (rgba_pixels.len() / 4)
        .div_ceil(SAMPLED_PALETTE_ERROR_SAMPLE_LIMIT)
        .max(1);
    let mut samples = 0_u64;
    let mut total_error = 0_u64;
    let mut max_error = 0_u64;

    for (index, px) in rgba_pixels.chunks_exact(4).enumerate() {
        if index % stride != 0 || px[3] < OPAQUE_ALPHA_THRESHOLD {
            continue;
        }

        let nearest = nearest_color(
            palette,
            i16::from(px[0]),
            i16::from(px[1]),
            i16::from(px[2]),
        );
        let dr = i32::from(px[0]) - i32::from(palette.r[nearest]);
        let dg = i32::from(px[1]) - i32::from(palette.g[nearest]);
        let db = i32::from(px[2]) - i32::from(palette.b[nearest]);
        let error = (dr * dr + dg * dg + db * db) as u64;
        total_error += error;
        max_error = max_error.max(error);
        samples += 1;

        if samples >= SAMPLED_PALETTE_ERROR_SAMPLE_LIMIT as u64 {
            break;
        }
    }

    if samples < DITHER_MIN_SAMPLES {
        return None;
    }

    Some(SampledPaletteErrorStats {
        mean_sse: total_error / samples,
        max_sse: max_error,
    })
}

#[inline]
fn sampled_seeded_palette_error_is_low(rgba_pixels: &[u8], palette: &[(u8, u8, u8)]) -> bool {
    let palette = PaletteSoA::from_tuples(palette);

    sampled_palette_error_stats(rgba_pixels, &palette).is_some_and(|stats| {
        stats.mean_sse <= SEEDED_ZERO_REFINE_MAX_MEAN_SAMPLE_SSE
            && stats.max_sse <= SEEDED_ZERO_REFINE_MAX_SAMPLE_SSE
    })
}

#[inline]
fn uses_ordered_dither(quality: u8) -> bool {
    quality <= QUALITY_REFINEMENT_SINGLE_MAX
}

#[inline]
fn seeded_zero_refine_shortcut(
    rgba_pixels: &[u8],
    initial_palette: &[(u8, u8, u8)],
    quality: u8,
    deduped_seed_count: usize,
    max_colors: usize,
) -> bool {
    if quality < QUALITY_REFINEMENT_HIGH_MIN || deduped_seed_count == 0 {
        return false;
    }

    if deduped_seed_count < max_colors.saturating_sub(SEEDED_ZERO_REFINE_FREE_SLOTS) {
        return false;
    }

    sampled_seeded_palette_error_is_low(rgba_pixels, initial_palette)
}

#[inline]
fn pack_rgb(r: u8, g: u8, b: u8) -> u32 {
    (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
}

#[inline]
fn unpack_rgb(rgb: u32) -> (u8, u8, u8) {
    ((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

fn exact_palette_if_small(rgba_pixels: &[u8], max_colors: usize) -> Option<(Vec<u8>, Vec<u8>)> {
    let pixel_count = rgba_pixels.len() / 4;
    let mut colors = BTreeSet::new();

    for pixel in rgba_pixels.chunks_exact(4) {
        if pixel[3] < OPAQUE_ALPHA_THRESHOLD {
            continue;
        }

        colors.insert(pack_rgb(pixel[0], pixel[1], pixel[2]));
        if colors.len() > max_colors {
            return None;
        }
    }

    let palette: Vec<u8> = colors
        .iter()
        .flat_map(|&rgb| {
            let (r, g, b) = unpack_rgb(rgb);
            [r, g, b]
        })
        .collect();

    let index_by_color: BTreeMap<u32, u8> = colors
        .iter()
        .enumerate()
        .map(|(index, &rgb)| (rgb, index as u8))
        .collect();

    let mut indices = Vec::with_capacity(pixel_count);
    for pixel in rgba_pixels.chunks_exact(4) {
        if pixel[3] < OPAQUE_ALPHA_THRESHOLD {
            indices.push(0);
        } else {
            let rgb = pack_rgb(pixel[0], pixel[1], pixel[2]);
            indices.push(index_by_color[&rgb]);
        }
    }

    Some((palette, indices))
}

fn dedup_seed_colors_from_arrays(seed_colors: &[[u8; 3]], max_colors: usize) -> Vec<(u8, u8, u8)> {
    let mut packed = BTreeSet::new();

    for color in seed_colors {
        packed.insert(pack_rgb(color[0], color[1], color[2]));
    }

    packed
        .into_iter()
        .map(unpack_rgb)
        .take(max_colors)
        .collect()
}

fn initial_palette_for_quantization(
    rgba_pixels: &[u8],
    max_colors: usize,
    seed_colors: Option<&[[u8; 3]]>,
) -> InitialPalette {
    match seed_colors {
        Some(seed_colors) => {
            let initial_palette = dedup_seed_colors_from_arrays(seed_colors, max_colors);
            let deduped_seed_count = initial_palette.len();

            if initial_palette.is_empty() {
                let initial_palette = generate_palette(rgba_pixels, max_colors);

                let palette = if initial_palette.len() < max_colors {
                    expand_palette_with_farthest_points(rgba_pixels, &initial_palette, max_colors)
                } else {
                    initial_palette
                };

                InitialPalette {
                    palette,
                    deduped_seed_count,
                }
            } else if initial_palette.len() >= max_colors {
                InitialPalette {
                    palette: initial_palette,
                    deduped_seed_count,
                }
            } else {
                InitialPalette {
                    palette: expand_palette_with_farthest_points(
                        rgba_pixels,
                        &initial_palette,
                        max_colors,
                    ),
                    deduped_seed_count,
                }
            }
        }
        None => {
            let initial_palette = generate_palette(rgba_pixels, max_colors);

            let palette = if initial_palette.len() < max_colors {
                expand_palette_with_farthest_points(rgba_pixels, &initial_palette, max_colors)
            } else {
                initial_palette
            };

            InitialPalette {
                palette,
                deduped_seed_count: 0,
            }
        }
    }
}

/// Quantize RGBA pixels to an indexed palette, optionally seeded from source palette colors.
///
/// Returns `(palette_rgb_flat, indices)`.
pub(crate) fn quantize_rgba_with_seed_colors(
    rgba_pixels: &[u8],
    width: usize,
    height: usize,
    quality: u8,
    seed_colors: Option<&[[u8; 3]]>,
) -> (Vec<u8>, Vec<u8>) {
    if rgba_pixels.is_empty() || width == 0 || height == 0 {
        return (Vec::new(), Vec::new());
    }

    let has_transparency = rgba_pixels
        .chunks_exact(4)
        .any(|px| px[3] < OPAQUE_ALPHA_THRESHOLD);
    let max_colors = if has_transparency { 255 } else { 256 };

    if let Some((palette, indices)) = exact_palette_if_small(rgba_pixels, max_colors) {
        return (palette, indices);
    }

    let initial_palette = initial_palette_for_quantization(rgba_pixels, max_colors, seed_colors);
    let iterations = if seed_colors.is_some()
        && seeded_zero_refine_shortcut(
            rgba_pixels,
            &initial_palette.palette,
            quality,
            initial_palette.deduped_seed_count,
            max_colors,
        ) {
        0
    } else {
        refinement_iterations(quality, initial_palette.deduped_seed_count)
    };

    let palette = refine_palette_weighted_unique(rgba_pixels, &initial_palette.palette, iterations);
    let indices = if uses_ordered_dither(quality) {
        dither_ordered(&palette, rgba_pixels, width, height, quality as f32 / 200.0)
    } else {
        dither_floyd_steinberg_serpentine(&palette, rgba_pixels, width, height)
    };

    (palette.to_flat_rgb(), indices)
}

/// Derive a 256-color palette from RGBA pixels.
pub(crate) fn derive_palette(rgba_pixels: &[u8]) -> Vec<u8> {
    if rgba_pixels.is_empty() {
        return Vec::new();
    }

    let initial_palette = generate_palette(rgba_pixels, 256);
    let palette = refine_palette_weighted_unique(rgba_pixels, &initial_palette, 1);

    palette.to_flat_rgb()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_palette_dedups_deterministically() {
        let seeds = [[9, 9, 9], [1, 2, 3], [9, 9, 9], [0, 255, 0], [1, 2, 3]];
        let palette = initial_palette_for_quantization(&[], 256, Some(&seeds)).palette;

        assert_eq!(palette, vec![(0, 255, 0), (1, 2, 3), (9, 9, 9)]);
    }

    #[test]
    fn empty_seed_palette_falls_back_to_wu() {
        let pixels = vec![255, 0, 0, 255, 0, 0, 255, 255];
        let expected = initial_palette_for_quantization(&pixels, 256, None).palette;
        let actual = initial_palette_for_quantization(&pixels, 256, Some(&[])).palette;

        assert_eq!(actual, expected);
    }

    #[test]
    fn seeded_palette_respects_max_colors() {
        let seeds = [[9, 9, 9], [1, 2, 3], [0, 255, 0], [5, 6, 7]];
        let palette = initial_palette_for_quantization(&[], 2, Some(&seeds)).palette;

        assert_eq!(palette, vec![(0, 255, 0), (1, 2, 3)]);
        assert_eq!(palette.len(), 2);
    }

    #[test]
    fn exact_palette_hits_for_small_opaque_input() {
        let rgba_pixels = [9, 1, 2, 255, 7, 8, 9, 255, 9, 1, 2, 255, 7, 8, 9, 255];

        let result = exact_palette_if_small(&rgba_pixels, 256).unwrap();

        assert_eq!(result.0, vec![7, 8, 9, 9, 1, 2]);
        assert_eq!(result.1, vec![1, 0, 1, 0]);
    }

    #[test]
    fn exact_palette_keeps_transparency_placeholder_zero() {
        let rgba_pixels = [1, 2, 3, 0, 9, 9, 9, 255, 4, 5, 6, 0, 9, 9, 9, 255];

        let result = exact_palette_if_small(&rgba_pixels, 255).unwrap();

        assert_eq!(result.0, vec![9, 9, 9]);
        assert_eq!(result.1, vec![0, 0, 0, 0]);
    }

    #[test]
    fn exact_palette_rejects_more_than_max_colors() {
        let mut rgba_pixels = Vec::new();
        for value in 0u8..=255 {
            rgba_pixels.extend_from_slice(&[value, 0, 0, 255]);
        }
        rgba_pixels.extend_from_slice(&[0, 0, 1, 255]);

        assert!(exact_palette_if_small(&rgba_pixels, 256).is_none());
    }

    #[test]
    fn exact_palette_is_deterministic() {
        let rgba_pixels = [4, 5, 6, 255, 1, 2, 3, 255, 9, 9, 9, 255, 1, 2, 3, 255];

        let first = exact_palette_if_small(&rgba_pixels, 256).unwrap();
        let second = exact_palette_if_small(&rgba_pixels, 256).unwrap();

        assert_eq!(first, second);
    }

    #[test]
    fn heuristic_uses_one_pass_for_small_seeded_palettes() {
        assert_eq!(heuristic_refinement_iterations(80, 192), 1);
    }

    #[test]
    fn heuristic_uses_three_passes_for_large_seeded_palettes() {
        assert_eq!(heuristic_refinement_iterations(80, 193), 3);
    }

    #[test]
    fn heuristic_keeps_generic_path_at_four_passes() {
        assert_eq!(heuristic_refinement_iterations(80, 0), 4);
    }

    #[test]
    fn seeded_zero_refine_shortcut_requires_high_quality() {
        let pixels = [0, 0, 0, 255, 255, 255, 255, 255];
        let palette = vec![(0, 0, 0), (255, 255, 255)];

        assert!(!seeded_zero_refine_shortcut(
            &pixels, &palette, 70, 255, 256
        ));
    }

    #[test]
    fn seeded_zero_refine_shortcut_accepts_low_error_near_cap_palettes() {
        let mut palette = Vec::with_capacity(240);
        let mut pixels = Vec::with_capacity(240 * 4);

        for value in 0_u8..=239 {
            palette.push((value, value, value));
            pixels.extend_from_slice(&[value, value, value, 255]);
        }

        assert!(seeded_zero_refine_shortcut(&pixels, &palette, 80, 240, 256));
    }

    #[test]
    fn seeded_zero_refine_shortcut_rejects_high_mean_palettes() {
        let mut palette = Vec::with_capacity(240);
        let mut pixels = Vec::with_capacity(240 * 4);

        for value in 0_u8..=255 {
            if (8..=23).contains(&value) {
                continue;
            }

            palette.push((value, 0, 0));
            pixels.extend_from_slice(&[20, 0, 0, 255]);
        }

        assert_eq!(palette.len(), 240);
        assert!(!seeded_zero_refine_shortcut(
            &pixels, &palette, 80, 240, 256
        ));
    }

    #[test]
    fn seeded_zero_refine_shortcut_rejects_high_max_palettes() {
        let mut palette = Vec::with_capacity(240);
        let mut pixels = Vec::with_capacity(240 * 4);

        for value in 0_u8..=238 {
            palette.push((value, value, value));
            pixels.extend_from_slice(&[value, value, value, 255]);
        }

        palette.push((239, 239, 239));
        pixels.extend_from_slice(&[255, 255, 255, 255]);

        assert!(!seeded_zero_refine_shortcut(
            &pixels, &palette, 80, 240, 256
        ));
        assert_eq!(refinement_iterations(80, 240), 3);
    }

    #[test]
    fn seeded_zero_refine_shortcut_ignores_empty_seed_fallback() {
        let pixels = vec![1, 2, 3, 255, 4, 5, 6, 255];
        let initial = initial_palette_for_quantization(&pixels, 256, Some(&[]));

        assert_eq!(initial.deduped_seed_count, 0);
        assert!(!seeded_zero_refine_shortcut(
            &pixels,
            &initial.palette,
            80,
            initial.deduped_seed_count,
            256,
        ));
        assert_eq!(refinement_iterations(80, initial.deduped_seed_count), 4);
    }

    #[test]
    fn refinement_iterations_keeps_low_and_mid_quality_stable() {
        assert_eq!(refinement_iterations(20, 240), 0);
        assert_eq!(refinement_iterations(50, 240), 1);
    }

    #[test]
    fn uses_ordered_dither_up_to_seventy() {
        assert!(uses_ordered_dither(70));
        assert!(!uses_ordered_dither(71));
    }

    #[test]
    fn sampled_palette_error_stats_ignores_transparent_pixels() {
        let palette = PaletteSoA::from_tuples(&[(0, 0, 0), (255, 255, 255)]);
        let mut pixels = Vec::new();

        for _ in 0..16 {
            pixels.extend_from_slice(&[0, 0, 0, 255]);
        }
        pixels.extend_from_slice(&[255, 0, 0, 0]);

        let stats = sampled_palette_error_stats(&pixels, &palette).unwrap();

        assert_eq!(stats.mean_sse, 0);
        assert_eq!(stats.max_sse, 0);
    }
}

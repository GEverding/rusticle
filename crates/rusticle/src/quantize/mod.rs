//! Color quantization internals.

use crate::quantize::dither::{dither_floyd_steinberg_serpentine, dither_ordered};
use crate::quantize::kmeans::{expand_palette_with_farthest_points, refine_palette};
use crate::quantize::wu::generate_palette;
use std::collections::{BTreeMap, BTreeSet};
use std::env;

pub(crate) mod dither;
pub(crate) mod kmeans;
pub(crate) mod wu;

pub(crate) const OPAQUE_ALPHA_THRESHOLD: u8 = 128;

const QUALITY_REFINEMENT_NONE_MAX: u8 = 30;
const QUALITY_REFINEMENT_SINGLE_MIN: u8 = QUALITY_REFINEMENT_NONE_MAX + 1;
const QUALITY_REFINEMENT_SINGLE_MAX: u8 = 70;

struct InitialPalette {
    palette: Vec<(u8, u8, u8)>,
    seeded_path_active: bool,
    deduped_seed_count: usize,
}

#[inline]
fn heuristic_refinement_iterations(
    quality: u8,
    seeded_path_active: bool,
    deduped_seed_count: usize,
) -> u32 {
    match quality {
        0..=QUALITY_REFINEMENT_NONE_MAX => 0,
        QUALITY_REFINEMENT_SINGLE_MIN..=QUALITY_REFINEMENT_SINGLE_MAX => 1,
        _ if seeded_path_active => {
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
fn refinement_iterations(quality: u8, seeded_path_active: bool, deduped_seed_count: usize) -> u32 {
    match env::var("RUSTICLE_EXPERIMENT_KMEANS_ITERS")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
    {
        Some(value) => value.clamp(1, 4),
        None => heuristic_refinement_iterations(quality, seeded_path_active, deduped_seed_count),
    }
}

#[inline]
fn uses_ordered_dither(quality: u8) -> bool {
    quality <= QUALITY_REFINEMENT_SINGLE_MAX
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

fn dedup_seed_colors(seed_colors: &[(u8, u8, u8)], max_colors: usize) -> Vec<(u8, u8, u8)> {
    let mut packed = BTreeSet::new();

    for &(r, g, b) in seed_colors {
        packed.insert(pack_rgb(r, g, b));
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
            let seed_colors: Vec<(u8, u8, u8)> = seed_colors
                .iter()
                .map(|color| (color[0], color[1], color[2]))
                .collect();
            let initial_palette = dedup_seed_colors(&seed_colors, max_colors);
            let seeded_path_active = !initial_palette.is_empty();
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
                    seeded_path_active,
                    deduped_seed_count,
                }
            } else if initial_palette.len() >= max_colors {
                InitialPalette {
                    palette: initial_palette,
                    seeded_path_active,
                    deduped_seed_count,
                }
            } else {
                InitialPalette {
                    palette: expand_palette_with_farthest_points(
                        rgba_pixels,
                        &initial_palette,
                        max_colors,
                    ),
                    seeded_path_active,
                    deduped_seed_count,
                }
            }
        }
        None => {
            let initial_palette = generate_palette(rgba_pixels, max_colors);
            let seeded_path_active = false;

            let palette = if initial_palette.len() < max_colors {
                expand_palette_with_farthest_points(rgba_pixels, &initial_palette, max_colors)
            } else {
                initial_palette
            };

            InitialPalette {
                palette,
                seeded_path_active,
                deduped_seed_count: 0,
            }
        }
    }
}

/// Quantize RGBA pixels to an indexed palette.
///
/// Returns `(palette_rgb_flat, indices)`.
#[allow(dead_code)]
pub(crate) fn quantize_rgba(
    rgba_pixels: &[u8],
    width: usize,
    height: usize,
    quality: u8,
) -> (Vec<u8>, Vec<u8>) {
    quantize_rgba_with_seed_colors(rgba_pixels, width, height, quality, None)
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
    let iterations = refinement_iterations(
        quality,
        initial_palette.seeded_path_active,
        initial_palette.deduped_seed_count,
    );

    let palette = refine_palette(rgba_pixels, &initial_palette.palette, iterations);
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
    refine_palette(rgba_pixels, &initial_palette, 1).to_flat_rgb()
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
        assert_eq!(heuristic_refinement_iterations(80, true, 192), 1);
    }

    #[test]
    fn heuristic_uses_three_passes_for_large_seeded_palettes() {
        assert_eq!(heuristic_refinement_iterations(80, true, 193), 3);
    }

    #[test]
    fn heuristic_keeps_generic_path_at_four_passes() {
        assert_eq!(heuristic_refinement_iterations(80, false, 0), 4);
    }
}

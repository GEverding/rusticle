//! K-means palette refinement.

use std::collections::BTreeSet;
use std::sync::OnceLock;

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

#[cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;

use crate::quantize::OPAQUE_ALPHA_THRESHOLD;

#[derive(Clone, Copy, Debug)]
struct WeightedColor {
    r: i16,
    g: i16,
    b: i16,
    weight: i64,
}

/// SoA palette layout for autovectorization-friendly distance computation.
#[derive(Clone, Debug)]
pub(crate) struct PaletteSoA {
    pub(crate) r: Vec<i16>,
    pub(crate) g: Vec<i16>,
    pub(crate) b: Vec<i16>,
    pub(crate) len: usize,
}

impl PaletteSoA {
    /// Create from a slice of `(r, g, b)` tuples.
    #[must_use]
    pub(crate) fn from_tuples(colors: &[(u8, u8, u8)]) -> Self {
        let len = colors.len().min(256);
        let mut r = Vec::with_capacity(len);
        let mut g = Vec::with_capacity(len);
        let mut b = Vec::with_capacity(len);

        for &(cr, cg, cb) in &colors[..len] {
            r.push(i16::from(cr));
            g.push(i16::from(cg));
            b.push(i16::from(cb));
        }

        Self { r, g, b, len }
    }

    /// Convert back to tuple form.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn to_tuples(&self) -> Vec<(u8, u8, u8)> {
        let mut colors = Vec::with_capacity(self.len);

        for i in 0..self.len {
            colors.push((
                self.r[i].clamp(0, 255) as u8,
                self.g[i].clamp(0, 255) as u8,
                self.b[i].clamp(0, 255) as u8,
            ));
        }

        colors
    }

    /// Convert to flat RGB bytes.
    #[must_use]
    pub(crate) fn to_flat_rgb(&self) -> Vec<u8> {
        let mut rgb = Vec::with_capacity(self.len * 3);

        for i in 0..self.len {
            rgb.push(self.r[i].clamp(0, 255) as u8);
            rgb.push(self.g[i].clamp(0, 255) as u8);
            rgb.push(self.b[i].clamp(0, 255) as u8);
        }

        rgb
    }
}

#[inline]
#[must_use]
fn pack_rgb(r: u8, g: u8, b: u8) -> u32 {
    (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
}

#[inline]
#[must_use]
fn pack_rgb_tuple(color: (u8, u8, u8)) -> u32 {
    pack_rgb(color.0, color.1, color.2)
}

#[inline]
#[must_use]
fn unpack_rgb(rgb: u32) -> (u8, u8, u8) {
    ((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

#[inline]
#[must_use]
fn dist_sq_rgb(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
    let dr = i32::from(a.0) - i32::from(b.0);
    let dg = i32::from(a.1) - i32::from(b.1);
    let db = i32::from(a.2) - i32::from(b.2);
    (dr * dr + dg * dg + db * db) as u32
}

fn unique_opaque_colors(rgba_pixels: &[u8]) -> Vec<WeightedColor> {
    let mut counts = std::collections::BTreeMap::new();
    for px in rgba_pixels.chunks_exact(4) {
        if px[3] >= OPAQUE_ALPHA_THRESHOLD {
            let rgb = pack_rgb(px[0], px[1], px[2]);
            *counts.entry(rgb).or_insert(0_i64) += 1;
        }
    }

    counts
        .into_iter()
        .map(|(rgb, weight)| {
            let (r, g, b) = unpack_rgb(rgb);
            WeightedColor {
                r: i16::from(r),
                g: i16::from(g),
                b: i16::from(b),
                weight,
            }
        })
        .collect()
}

#[inline]
#[must_use]
fn nearest_color_scalar(palette: &PaletteSoA, r: i16, g: i16, b: i16) -> usize {
    let mut best_idx = 0_usize;
    let mut best_dist = i32::MAX;

    for i in 0..palette.len {
        let dr = (r - palette.r[i]) as i32;
        let dg = (g - palette.g[i]) as i32;
        let db = (b - palette.b[i]) as i32;
        let dist = dr * dr + dg * dg + db * db;

        if dist < best_dist {
            best_dist = dist;
            best_idx = i;
        }
    }

    best_idx
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn nearest_color_avx2(palette: &PaletteSoA, r: i16, g: i16, b: i16) -> usize {
    let mut best_idx = 0_usize;
    let mut best_dist = i32::MAX;
    let mut i = 0_usize;

    let qr = _mm_set1_epi16(r);
    let qg = _mm_set1_epi16(g);
    let qb = _mm_set1_epi16(b);

    while i + 8 <= palette.len {
        let pr = _mm_loadu_si128(palette.r.as_ptr().add(i) as *const __m128i);
        let pg = _mm_loadu_si128(palette.g.as_ptr().add(i) as *const __m128i);
        let pb = _mm_loadu_si128(palette.b.as_ptr().add(i) as *const __m128i);

        let dr = _mm_sub_epi16(qr, pr);
        let dg = _mm_sub_epi16(qg, pg);
        let db = _mm_sub_epi16(qb, pb);

        let dr = _mm256_cvtepi16_epi32(dr);
        let dg = _mm256_cvtepi16_epi32(dg);
        let db = _mm256_cvtepi16_epi32(db);

        let dist = _mm256_add_epi32(
            _mm256_add_epi32(_mm256_mullo_epi32(dr, dr), _mm256_mullo_epi32(dg, dg)),
            _mm256_mullo_epi32(db, db),
        );

        let mut lanes = [0_i32; 8];
        _mm256_storeu_si256(lanes.as_mut_ptr() as *mut __m256i, dist);
        for (lane, &dist) in lanes.iter().enumerate() {
            if dist < best_dist {
                best_dist = dist;
                best_idx = i + lane;
            }
        }

        i += 8;
    }

    while i < palette.len {
        let dr = (r - palette.r[i]) as i32;
        let dg = (g - palette.g[i]) as i32;
        let db = (b - palette.b[i]) as i32;
        let dist = dr * dr + dg * dg + db * db;

        if dist < best_dist {
            best_dist = dist;
            best_idx = i;
        }

        i += 1;
    }

    best_idx
}

#[cfg(target_arch = "aarch64")]
unsafe fn nearest_color_neon(palette: &PaletteSoA, r: i16, g: i16, b: i16) -> usize {
    let mut best_idx = 0_usize;
    let mut best_dist = i32::MAX;
    let mut i = 0_usize;

    let qr = vdupq_n_s16(r);
    let qg = vdupq_n_s16(g);
    let qb = vdupq_n_s16(b);

    while i + 8 <= palette.len {
        let pr = vld1q_s16(palette.r.as_ptr().add(i));
        let pg = vld1q_s16(palette.g.as_ptr().add(i));
        let pb = vld1q_s16(palette.b.as_ptr().add(i));

        let dr = vsubq_s16(qr, pr);
        let dg = vsubq_s16(qg, pg);
        let db = vsubq_s16(qb, pb);

        let dr_lo = vmovl_s16(vget_low_s16(dr));
        let dg_lo = vmovl_s16(vget_low_s16(dg));
        let db_lo = vmovl_s16(vget_low_s16(db));
        let dr_hi = vmovl_s16(vget_high_s16(dr));
        let dg_hi = vmovl_s16(vget_high_s16(dg));
        let db_hi = vmovl_s16(vget_high_s16(db));

        let dist_lo = vaddq_s32(
            vaddq_s32(vmulq_s32(dr_lo, dr_lo), vmulq_s32(dg_lo, dg_lo)),
            vmulq_s32(db_lo, db_lo),
        );
        let dist_hi = vaddq_s32(
            vaddq_s32(vmulq_s32(dr_hi, dr_hi), vmulq_s32(dg_hi, dg_hi)),
            vmulq_s32(db_hi, db_hi),
        );

        let mut lanes = [0_i32; 8];
        vst1q_s32(lanes.as_mut_ptr(), dist_lo);
        vst1q_s32(lanes.as_mut_ptr().add(4), dist_hi);

        for (lane, &dist) in lanes.iter().enumerate() {
            if dist < best_dist {
                best_dist = dist;
                best_idx = i + lane;
            }
        }

        i += 8;
    }

    while i < palette.len {
        let dr = (r - palette.r[i]) as i32;
        let dg = (g - palette.g[i]) as i32;
        let db = (b - palette.b[i]) as i32;
        let dist = dr * dr + dg * dg + db * db;

        if dist < best_dist {
            best_dist = dist;
            best_idx = i;
        }

        i += 1;
    }

    best_idx
}

#[cfg(target_arch = "x86_64")]
type NearestColorImpl = fn(&PaletteSoA, i16, i16, i16) -> usize;

#[cfg(target_arch = "aarch64")]
type NearestColorImpl = fn(&PaletteSoA, i16, i16, i16) -> usize;

#[cfg(target_arch = "x86_64")]
#[inline]
fn nearest_color_avx2_dispatch(palette: &PaletteSoA, r: i16, g: i16, b: i16) -> usize {
    unsafe { nearest_color_avx2(palette, r, g, b) }
}

#[cfg(target_arch = "aarch64")]
#[inline]
fn nearest_color_neon_dispatch(palette: &PaletteSoA, r: i16, g: i16, b: i16) -> usize {
    unsafe { nearest_color_neon(palette, r, g, b) }
}

#[cfg(target_arch = "x86_64")]
#[inline]
fn nearest_color_impl() -> NearestColorImpl {
    static IMPL: OnceLock<NearestColorImpl> = OnceLock::new();

    *IMPL.get_or_init(|| {
        if is_x86_feature_detected!("avx2") {
            nearest_color_avx2_dispatch
        } else {
            nearest_color_scalar
        }
    })
}

#[cfg(target_arch = "aarch64")]
#[inline]
fn nearest_color_impl() -> NearestColorImpl {
    static IMPL: OnceLock<NearestColorImpl> = OnceLock::new();

    *IMPL.get_or_init(|| nearest_color_neon_dispatch)
}

/// Expand a palette using deterministic farthest-point sampling from opaque source pixels.
#[must_use]
pub(crate) fn expand_palette_with_farthest_points(
    rgba_pixels: &[u8],
    initial_palette: &[(u8, u8, u8)],
    target_colors: usize,
) -> Vec<(u8, u8, u8)> {
    let target_colors = target_colors.max(1);
    let mut palette: Vec<(u8, u8, u8)> = initial_palette
        .iter()
        .copied()
        .take(target_colors)
        .collect();

    if palette.len() >= target_colors {
        return palette;
    }

    let mut candidates = BTreeSet::new();
    for px in rgba_pixels.chunks_exact(4) {
        if px[3] >= OPAQUE_ALPHA_THRESHOLD {
            candidates.insert(pack_rgb(px[0], px[1], px[2]));
        }
    }

    for color in &palette {
        candidates.remove(&pack_rgb_tuple(*color));
    }

    let candidates: Vec<(u8, u8, u8)> = candidates.into_iter().map(unpack_rgb).collect();

    while palette.len() < target_colors {
        let mut best_candidate = None;
        let mut best_dist = 0_u32;

        for &candidate in &candidates {
            if palette.contains(&candidate) {
                continue;
            }

            let nearest = palette
                .iter()
                .map(|&color| dist_sq_rgb(candidate, color))
                .min()
                .unwrap_or(0);

            match best_candidate {
                None => {
                    best_candidate = Some(candidate);
                    best_dist = nearest;
                }
                Some(current) => {
                    if nearest > best_dist
                        || (nearest == best_dist
                            && pack_rgb_tuple(candidate) < pack_rgb_tuple(current))
                    {
                        best_candidate = Some(candidate);
                        best_dist = nearest;
                    }
                }
            }
        }

        let Some(candidate) = best_candidate else {
            break;
        };

        if best_dist == 0 {
            break;
        }

        palette.push(candidate);
    }

    palette
}

/// Find the index of the nearest palette color to `(r, g, b)`.
#[inline]
#[must_use]
pub(crate) fn nearest_color(palette: &PaletteSoA, r: i16, g: i16, b: i16) -> usize {
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    {
        nearest_color_impl()(palette, r, g, b)
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        nearest_color_scalar(palette, r, g, b)
    }
}

/// Refine a palette using k-means iterations.
#[cfg(test)]
#[must_use]
pub(crate) fn refine_palette(
    rgba_pixels: &[u8],
    initial_palette: &[(u8, u8, u8)],
    iterations: u32,
) -> PaletteSoA {
    let mut palette = PaletteSoA::from_tuples(initial_palette);

    if iterations == 0 || palette.len == 0 {
        return palette;
    }

    for _ in 0..iterations {
        let assignments = map_pixels(&palette, rgba_pixels);

        let mut sum_r = vec![0_i64; palette.len];
        let mut sum_g = vec![0_i64; palette.len];
        let mut sum_b = vec![0_i64; palette.len];
        let mut counts = vec![0_i64; palette.len];

        for (px, &idx) in rgba_pixels.chunks_exact(4).zip(assignments.iter()) {
            if px[3] < OPAQUE_ALPHA_THRESHOLD {
                continue;
            }

            let idx = usize::from(idx);
            if idx >= palette.len {
                continue;
            }

            sum_r[idx] += i64::from(px[0]);
            sum_g[idx] += i64::from(px[1]);
            sum_b[idx] += i64::from(px[2]);
            counts[idx] += 1;
        }

        for i in 0..palette.len {
            let count = counts[i];
            if count == 0 {
                continue;
            }

            palette.r[i] = ((sum_r[i] + count / 2) / count) as i16;
            palette.g[i] = ((sum_g[i] + count / 2) / count) as i16;
            palette.b[i] = ((sum_b[i] + count / 2) / count) as i16;
        }
    }

    palette
}

/// Refine a palette using k-means iterations over unique opaque RGB colors.
#[must_use]
pub(crate) fn refine_palette_weighted_unique(
    rgba_pixels: &[u8],
    initial_palette: &[(u8, u8, u8)],
    iterations: u32,
) -> PaletteSoA {
    let mut palette = PaletteSoA::from_tuples(initial_palette);

    if iterations == 0 || palette.len == 0 {
        return palette;
    }

    let colors = unique_opaque_colors(rgba_pixels);
    if colors.is_empty() {
        return palette;
    }

    for _ in 0..iterations {
        let mut sum_r = vec![0_i64; palette.len];
        let mut sum_g = vec![0_i64; palette.len];
        let mut sum_b = vec![0_i64; palette.len];
        let mut counts = vec![0_i64; palette.len];

        for color in &colors {
            let idx = nearest_color(&palette, color.r, color.g, color.b);

            sum_r[idx] += i64::from(color.r) * color.weight;
            sum_g[idx] += i64::from(color.g) * color.weight;
            sum_b[idx] += i64::from(color.b) * color.weight;
            counts[idx] += color.weight;
        }

        for i in 0..palette.len {
            let count = counts[i];
            if count == 0 {
                continue;
            }

            palette.r[i] = ((sum_r[i] + count / 2) / count) as i16;
            palette.g[i] = ((sum_g[i] + count / 2) / count) as i16;
            palette.b[i] = ((sum_b[i] + count / 2) / count) as i16;
        }
    }

    palette
}

/// Assign every pixel to its nearest palette color, returning indices.
#[cfg(test)]
#[must_use]
pub(crate) fn map_pixels(palette: &PaletteSoA, rgba_pixels: &[u8]) -> Vec<u8> {
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

    fn rgb_set(colors: &[(u8, u8, u8)]) -> BTreeSet<u32> {
        colors.iter().map(|&(r, g, b)| pack_rgb(r, g, b)).collect()
    }

    fn sse(palette: &PaletteSoA, pixels: &[u8]) -> i64 {
        let mut total = 0_i64;

        for px in pixels.chunks_exact(4) {
            if px[3] < OPAQUE_ALPHA_THRESHOLD {
                continue;
            }

            let idx = nearest_color(
                palette,
                i16::from(px[0]),
                i16::from(px[1]),
                i16::from(px[2]),
            );
            let dr = i32::from(px[0]) - i32::from(palette.r[idx]);
            let dg = i32::from(px[1]) - i32::from(palette.g[idx]);
            let db = i32::from(px[2]) - i32::from(palette.b[idx]);
            total += i64::from(dr * dr + dg * dg + db * db);
        }

        total
    }

    #[test]
    fn test_palette_soa_roundtrip() {
        let colors = [(1, 2, 3), (255, 0, 128), (12, 34, 56)];
        let palette = PaletteSoA::from_tuples(&colors);

        assert_eq!(palette.to_tuples(), colors);
    }

    #[test]
    fn test_nearest_color_exact() {
        let palette = PaletteSoA::from_tuples(&[(10, 20, 30), (100, 110, 120)]);

        assert_eq!(nearest_color(&palette, 100, 110, 120), 1);
    }

    #[test]
    fn test_nearest_color_closest() {
        let palette = PaletteSoA::from_tuples(&[(0, 0, 0), (100, 0, 0)]);

        assert_eq!(nearest_color(&palette, 60, 0, 0), 1);
    }

    #[test]
    fn test_nearest_color_tie_keeps_first() {
        let palette = PaletteSoA::from_tuples(&[(0, 0, 0), (2, 0, 0)]);

        assert_eq!(nearest_color(&palette, 1, 0, 0), 0);
    }

    #[test]
    fn test_nearest_color_matches_scalar() {
        let palette = PaletteSoA::from_tuples(&[
            (0, 0, 0),
            (255, 255, 255),
            (64, 128, 192),
            (192, 64, 32),
            (32, 192, 64),
            (128, 32, 192),
            (16, 16, 240),
            (240, 16, 16),
            (16, 240, 16),
        ]);

        for r in [0_i16, 1, 15, 32, 63, 64, 127, 128, 191, 192, 223, 255] {
            for g in [0_i16, 17, 64, 96, 128, 160, 192, 224, 255] {
                for b in [0_i16, 8, 16, 31, 64, 127, 128, 191, 240, 255] {
                    let expected = nearest_color_scalar(&palette, r, g, b);
                    assert_eq!(nearest_color(&palette, r, g, b), expected);
                }
            }
        }
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn test_nearest_color_matches_scalar_aarch64() {
        let palette = PaletteSoA::from_tuples(&[
            (0, 0, 0),
            (255, 255, 255),
            (64, 128, 192),
            (192, 64, 32),
            (32, 192, 64),
            (128, 32, 192),
            (16, 16, 240),
            (240, 16, 16),
            (16, 240, 16),
        ]);

        for r in [0_i16, 1, 15, 32, 63, 64, 127, 128, 191, 192, 223, 255] {
            for g in [0_i16, 17, 64, 96, 128, 160, 192, 224, 255] {
                for b in [0_i16, 8, 16, 31, 64, 127, 128, 191, 240, 255] {
                    let expected = nearest_color_scalar(&palette, r, g, b);
                    assert_eq!(nearest_color(&palette, r, g, b), expected);
                }
            }
        }
    }

    #[test]
    fn test_refine_no_iterations() {
        let initial = [(10, 20, 30), (40, 50, 60)];
        let refined = refine_palette(&[], &initial, 0);

        assert_eq!(refined.to_tuples(), initial);
    }

    #[test]
    fn test_refine_converges() {
        let pixels = rgba(&[
            (18, 20, 22, 255),
            (20, 21, 19, 255),
            (23, 19, 20, 255),
            (219, 221, 220, 255),
            (222, 218, 221, 255),
            (224, 223, 219, 255),
        ]);
        let initial = [(0, 0, 0), (255, 255, 255)];
        let before = sse(&PaletteSoA::from_tuples(&initial), &pixels);
        let refined = refine_palette(&pixels, &initial, 1);
        let after = sse(&refined, &pixels);

        assert!(after < before);
    }

    #[test]
    fn test_weighted_unique_matches_pixelwise_for_duplicates() {
        let pixels = rgba(&[
            (12, 18, 24, 255),
            (12, 18, 24, 255),
            (12, 18, 24, 255),
            (220, 210, 200, 255),
            (220, 210, 200, 255),
        ]);
        let initial = [(0, 0, 0), (255, 255, 255)];

        let pixelwise = refine_palette(&pixels, &initial, 1).to_tuples();
        let weighted = refine_palette_weighted_unique(&pixels, &initial, 1).to_tuples();

        assert_eq!(weighted, pixelwise);
    }

    #[test]
    fn test_weighted_unique_handles_empty_and_transparent_input() {
        let initial = [(10, 20, 30), (40, 50, 60)];
        let transparent = rgba(&[(1, 2, 3, 0), (4, 5, 6, 127)]);

        assert_eq!(
            refine_palette_weighted_unique(&[], &initial, 1).to_tuples(),
            initial
        );
        assert_eq!(
            refine_palette_weighted_unique(&transparent, &initial, 1).to_tuples(),
            initial
        );
    }

    #[test]
    fn test_map_pixels_transparent() {
        let palette = PaletteSoA::from_tuples(&[(255, 255, 255), (0, 0, 0)]);
        let pixels = rgba(&[(255, 0, 0, 0), (0, 255, 0, 127), (0, 0, 0, 255)]);

        assert_eq!(map_pixels(&palette, &pixels), vec![0, 0, 1]);
    }

    #[test]
    fn test_empty_cluster_preserved() {
        let initial = [(10, 10, 10), (250, 250, 250)];
        let pixels = rgba(&[(12, 10, 11, 255), (9, 10, 10, 255), (11, 9, 10, 255)]);
        let refined = refine_palette(&pixels, &initial, 1);

        assert_eq!(refined.to_tuples()[1], initial[1]);
    }

    #[test]
    fn test_expand_palette_adds_source_colors() {
        let pixels = rgba(&[
            (10, 20, 30, 255),
            (40, 50, 60, 255),
            (200, 210, 220, 255),
            (240, 10, 80, 255),
        ]);
        let expanded =
            expand_palette_with_farthest_points(&pixels, &[(10, 20, 30), (40, 50, 60)], 4);

        assert_eq!(expanded.len(), 4);
        assert!(rgb_set(&expanded).is_superset(&rgb_set(&[(10, 20, 30), (40, 50, 60)])));
        assert!(rgb_set(&expanded).is_subset(&rgb_set(&[
            (10, 20, 30),
            (40, 50, 60),
            (200, 210, 220),
            (240, 10, 80)
        ])));
    }

    #[test]
    fn test_expand_palette_is_deterministic() {
        let pixels = rgba(&[
            (5, 5, 5, 255),
            (250, 250, 250, 255),
            (128, 0, 0, 255),
            (0, 128, 0, 255),
        ]);
        let initial = [(5, 5, 5), (250, 250, 250)];

        let a = expand_palette_with_farthest_points(&pixels, &initial, 4);
        let b = expand_palette_with_farthest_points(&pixels, &initial, 4);

        assert_eq!(a, b);
    }

    #[test]
    fn test_expand_palette_respects_target() {
        let pixels = rgba(&[(0, 0, 0, 255), (255, 255, 255, 255)]);
        let expanded = expand_palette_with_farthest_points(&pixels, &[(0, 0, 0)], 2);

        assert!(expanded.len() <= 2);
    }

    #[test]
    fn test_expand_palette_noop_when_no_new_colors() {
        let pixels = rgba(&[(10, 20, 30, 255), (10, 20, 30, 255)]);
        let initial = [(10, 20, 30), (40, 50, 60)];

        let expanded = expand_palette_with_farthest_points(&pixels, &initial, 4);

        assert_eq!(expanded, initial);
    }
}

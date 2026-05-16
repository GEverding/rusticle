//! Wu-style 3D histogram and cumulative moment tables.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::quantize::OPAQUE_ALPHA_THRESHOLD;

const HIST_BITS: u8 = 5;
const HIST_SIZE: usize = 33;
const HIST_MAX_BIN: usize = HIST_SIZE - 1;
const TOTAL_BINS: usize = HIST_SIZE * HIST_SIZE * HIST_SIZE;

/// 3D histogram and moment tables for Wu color quantization.
#[derive(Clone, Debug)]
pub(crate) struct Histogram3D {
    /// Weight (pixel count) per bin.
    pub(crate) wt: Vec<i64>,
    /// Sum of red values per bin.
    pub(crate) mr: Vec<i64>,
    /// Sum of green values per bin.
    pub(crate) mg: Vec<i64>,
    /// Sum of blue values per bin.
    pub(crate) mb: Vec<i64>,
    /// Sum of squared color values per bin.
    pub(crate) m2: Vec<f64>,
}

impl Histogram3D {
    /// Create an empty histogram.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            wt: vec![0; TOTAL_BINS],
            mr: vec![0; TOTAL_BINS],
            mg: vec![0; TOTAL_BINS],
            mb: vec![0; TOTAL_BINS],
            m2: vec![0.0; TOTAL_BINS],
        }
    }
}

/// Axis-aligned color box in histogram bin coordinates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ColorBox {
    pub(crate) r0: usize,
    pub(crate) r1: usize,
    pub(crate) g0: usize,
    pub(crate) g1: usize,
    pub(crate) b0: usize,
    pub(crate) b1: usize,
}

/// Map RGB values to a flattened histogram bin.
#[inline]
#[must_use]
fn bin_index(r: u8, g: u8, b: u8) -> usize {
    let shift = 8 - HIST_BITS;
    let ri = (r >> shift) as usize + 1;
    let gi = (g >> shift) as usize + 1;
    let bi = (b >> shift) as usize + 1;
    ri * HIST_SIZE * HIST_SIZE + gi * HIST_SIZE + bi
}

#[inline]
#[must_use]
fn bin_coord_index(r: usize, g: usize, b: usize) -> usize {
    r * HIST_SIZE * HIST_SIZE + g * HIST_SIZE + b
}

/// Build a raw histogram from RGBA pixels.
///
/// Pixels with alpha below the opaque threshold are skipped.
#[must_use]
pub(crate) fn build_histogram(rgba_pixels: &[u8]) -> Histogram3D {
    let mut hist = Histogram3D::new();

    for px in rgba_pixels.chunks_exact(4) {
        let r = px[0];
        let g = px[1];
        let b = px[2];
        let a = px[3];

        if a < OPAQUE_ALPHA_THRESHOLD {
            continue;
        }

        let idx = bin_index(r, g, b);
        hist.wt[idx] += 1;
        hist.mr[idx] += i64::from(r);
        hist.mg[idx] += i64::from(g);
        hist.mb[idx] += i64::from(b);
        let rf = f64::from(r);
        let gf = f64::from(g);
        let bf = f64::from(b);
        hist.m2[idx] += rf * rf + gf * gf + bf * bf;
    }

    hist
}

/// Convert raw moment tables into cumulative prefix sums in-place.
pub(crate) fn compute_cumulative_moments(hist: &mut Histogram3D) {
    prefix_b(hist);
    prefix_g(hist);
    prefix_r(hist);
}

#[inline]
fn prefix_b(hist: &mut Histogram3D) {
    for r in 1..HIST_SIZE {
        for g in 1..HIST_SIZE {
            let mut wt = 0_i64;
            let mut mr = 0_i64;
            let mut mg = 0_i64;
            let mut mb = 0_i64;
            let mut m2 = 0.0_f64;

            for b in 1..HIST_SIZE {
                let idx = bin_coord_index(r, g, b);
                wt += hist.wt[idx];
                mr += hist.mr[idx];
                mg += hist.mg[idx];
                mb += hist.mb[idx];
                m2 += hist.m2[idx];
                hist.wt[idx] = wt;
                hist.mr[idx] = mr;
                hist.mg[idx] = mg;
                hist.mb[idx] = mb;
                hist.m2[idx] = m2;
            }
        }
    }
}

#[inline]
fn prefix_g(hist: &mut Histogram3D) {
    for r in 1..HIST_SIZE {
        for b in 1..HIST_SIZE {
            let mut wt = 0_i64;
            let mut mr = 0_i64;
            let mut mg = 0_i64;
            let mut mb = 0_i64;
            let mut m2 = 0.0_f64;

            for g in 1..HIST_SIZE {
                let idx = bin_coord_index(r, g, b);
                wt += hist.wt[idx];
                mr += hist.mr[idx];
                mg += hist.mg[idx];
                mb += hist.mb[idx];
                m2 += hist.m2[idx];
                hist.wt[idx] = wt;
                hist.mr[idx] = mr;
                hist.mg[idx] = mg;
                hist.mb[idx] = mb;
                hist.m2[idx] = m2;
            }
        }
    }
}

#[inline]
fn prefix_r(hist: &mut Histogram3D) {
    for g in 1..HIST_SIZE {
        for b in 1..HIST_SIZE {
            let mut wt = 0_i64;
            let mut mr = 0_i64;
            let mut mg = 0_i64;
            let mut mb = 0_i64;
            let mut m2 = 0.0_f64;

            for r in 1..HIST_SIZE {
                let idx = bin_coord_index(r, g, b);
                wt += hist.wt[idx];
                mr += hist.mr[idx];
                mg += hist.mg[idx];
                mb += hist.mb[idx];
                m2 += hist.m2[idx];
                hist.wt[idx] = wt;
                hist.mr[idx] = mr;
                hist.mg[idx] = mg;
                hist.mb[idx] = mb;
                hist.m2[idx] = m2;
            }
        }
    }
}

fn prefix_at(hist: &Histogram3D, r: usize, g: usize, b: usize) -> (i64, i64, i64, i64, f64) {
    let idx = bin_coord_index(r, g, b);
    (
        hist.wt[idx],
        hist.mr[idx],
        hist.mg[idx],
        hist.mb[idx],
        hist.m2[idx],
    )
}

#[inline]
fn volume_with_upper(
    hist: &Histogram3D,
    box_: &ColorBox,
    r1: usize,
    g1: usize,
    b1: usize,
) -> (i64, i64, i64, i64, f64) {
    let (a, ar, ag, ab, am2) = prefix_at(hist, r1, g1, b1);
    let (b, br, bg, bb, bm2) = prefix_at(hist, box_.r0, g1, b1);
    let (c, cr, cg, cb, cm2) = prefix_at(hist, r1, box_.g0, b1);
    let (d, dr, dg, db, dm2) = prefix_at(hist, r1, g1, box_.b0);
    let (e, er, eg, eb, em2) = prefix_at(hist, box_.r0, box_.g0, b1);
    let (f, fr, fg, fb, fm2) = prefix_at(hist, box_.r0, g1, box_.b0);
    let (g, gr, gg, gb, gm2) = prefix_at(hist, r1, box_.g0, box_.b0);
    let (h, hr, hg, hb, hm2) = prefix_at(hist, box_.r0, box_.g0, box_.b0);

    (
        a - b - c - d + e + f + g - h,
        ar - br - cr - dr + er + fr + gr - hr,
        ag - bg - cg - dg + eg + fg + gg - hg,
        ab - bb - cb - db + eb + fb + gb - hb,
        am2 - bm2 - cm2 - dm2 + em2 + fm2 + gm2 - hm2,
    )
}

/// Query the total weight and moments in a box.
#[must_use]
pub(crate) fn volume(hist: &Histogram3D, box_: &ColorBox) -> (i64, i64, i64, i64, f64) {
    volume_with_upper(hist, box_, box_.r1, box_.g1, box_.b1)
}

/// Moments of the bottom half when cutting a box along the red axis.
#[inline]
#[must_use]
fn bottom_r(hist: &Histogram3D, box_: &ColorBox, cut: usize) -> (i64, i64, i64, i64, f64) {
    volume_with_upper(hist, box_, cut, box_.g1, box_.b1)
}

/// Moments of the bottom half when cutting a box along the green axis.
#[inline]
#[must_use]
fn bottom_g(hist: &Histogram3D, box_: &ColorBox, cut: usize) -> (i64, i64, i64, i64, f64) {
    volume_with_upper(hist, box_, box_.r1, cut, box_.b1)
}

/// Moments of the bottom half when cutting a box along the blue axis.
#[inline]
#[must_use]
fn bottom_b(hist: &Histogram3D, box_: &ColorBox, cut: usize) -> (i64, i64, i64, i64, f64) {
    volume_with_upper(hist, box_, box_.r1, box_.g1, cut)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Axis {
    Red,
    Green,
    Blue,
}

#[inline]
fn benefit(mr: i64, mg: i64, mb: i64, wt: i64) -> f64 {
    if wt == 0 {
        return 0.0;
    }

    let wt = wt as f64;
    let mr = mr as f64;
    let mg = mg as f64;
    let mb = mb as f64;
    (mr * mr + mg * mg + mb * mb) / wt
}

#[inline]
fn axis_cut_range(box_: &ColorBox, axis: Axis) -> Option<(usize, usize)> {
    let (lo, hi) = match axis {
        Axis::Red => (box_.r0, box_.r1),
        Axis::Green => (box_.g0, box_.g1),
        Axis::Blue => (box_.b0, box_.b1),
    };

    (lo + 1 < hi).then_some((lo + 1, hi - 1))
}

/// Find the best cut position along one axis that maximizes variance reduction.
#[must_use]
fn maximize_cut(
    hist: &Histogram3D,
    box_: &ColorBox,
    whole: (i64, i64, i64, i64, f64),
    axis: Axis,
) -> Option<(usize, f64)> {
    let (start, end) = axis_cut_range(box_, axis)?;
    let mut best_cut = None;
    let mut best_benefit = f64::NEG_INFINITY;

    for cut in start..=end {
        let bottom = match axis {
            Axis::Red => bottom_r(hist, box_, cut),
            Axis::Green => bottom_g(hist, box_, cut),
            Axis::Blue => bottom_b(hist, box_, cut),
        };

        let top = (
            whole.0 - bottom.0,
            whole.1 - bottom.1,
            whole.2 - bottom.2,
            whole.3 - bottom.3,
            whole.4 - bottom.4,
        );

        if bottom.0 == 0 || top.0 == 0 {
            continue;
        }

        let score =
            benefit(bottom.1, bottom.2, bottom.3, bottom.0) + benefit(top.1, top.2, top.3, top.0);

        if score > best_benefit {
            best_benefit = score;
            best_cut = Some(cut);
        }
    }

    best_cut.map(|cut| (cut, best_benefit))
}

/// Split a box into two along the axis/position that maximizes variance reduction.
#[must_use]
fn split_box(hist: &Histogram3D, box_: &ColorBox) -> Option<(ColorBox, ColorBox)> {
    let whole = volume(hist, box_);
    if whole.0 == 0 {
        return None;
    }

    let mut best: Option<(Axis, usize, f64)> = None;
    for axis in [Axis::Red, Axis::Green, Axis::Blue] {
        if let Some((cut, score)) = maximize_cut(hist, box_, whole, axis) {
            let replace = match best {
                None => true,
                Some((_, _, best_score)) => score > best_score,
            };

            if replace {
                best = Some((axis, cut, score));
            }
        }
    }

    let (axis, cut, _) = best?;

    let mut left = *box_;
    let mut right = *box_;
    match axis {
        Axis::Red => {
            left.r1 = cut;
            right.r0 = cut;
        }
        Axis::Green => {
            left.g1 = cut;
            right.g0 = cut;
        }
        Axis::Blue => {
            left.b1 = cut;
            right.b0 = cut;
        }
    }

    Some((left, right))
}

#[derive(Clone, Copy, Debug)]
struct QueueEntry {
    variance: f64,
    box_: ColorBox,
}

impl PartialEq for QueueEntry {
    fn eq(&self, other: &Self) -> bool {
        self.variance.total_cmp(&other.variance) == Ordering::Equal && self.box_ == other.box_
    }
}

impl Eq for QueueEntry {}

impl PartialOrd for QueueEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for QueueEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.variance
            .total_cmp(&other.variance)
            .then_with(|| self.box_.r0.cmp(&other.box_.r0))
            .then_with(|| self.box_.r1.cmp(&other.box_.r1))
            .then_with(|| self.box_.g0.cmp(&other.box_.g0))
            .then_with(|| self.box_.g1.cmp(&other.box_.g1))
            .then_with(|| self.box_.b0.cmp(&other.box_.b0))
            .then_with(|| self.box_.b1.cmp(&other.box_.b1))
    }
}

#[inline]
fn box_entry(hist: &Histogram3D, box_: ColorBox) -> QueueEntry {
    QueueEntry {
        variance: variance(hist, &box_),
        box_,
    }
}

#[inline]
fn centroid(hist: &Histogram3D, box_: &ColorBox) -> (u8, u8, u8) {
    let (wt, mr, mg, mb, _) = volume(hist, box_);
    if wt == 0 {
        return (0, 0, 0);
    }

    let wt = wt as f64;
    let r = (mr as f64 / wt).round().clamp(0.0, 255.0) as u8;
    let g = (mg as f64 / wt).round().clamp(0.0, 255.0) as u8;
    let b = (mb as f64 / wt).round().clamp(0.0, 255.0) as u8;
    (r, g, b)
}

/// Generate an optimal palette of up to `max_colors` colors.
#[must_use]
pub(crate) fn generate_palette(rgba_pixels: &[u8], max_colors: usize) -> Vec<(u8, u8, u8)> {
    let max_colors = max_colors.max(1);
    let mut hist = build_histogram(rgba_pixels);

    if hist.wt.iter().sum::<i64>() == 0 {
        return vec![(0, 0, 0)];
    }

    compute_cumulative_moments(&mut hist);

    let root = ColorBox {
        r0: 0,
        r1: HIST_MAX_BIN,
        g0: 0,
        g1: HIST_MAX_BIN,
        b0: 0,
        b1: HIST_MAX_BIN,
    };

    let mut heap = BinaryHeap::new();
    heap.push(box_entry(&hist, root));

    let mut final_boxes = Vec::new();
    let mut box_count = 1_usize;

    while box_count < max_colors {
        let Some(entry) = heap.pop() else {
            break;
        };

        if let Some((left, right)) = split_box(&hist, &entry.box_) {
            heap.push(box_entry(&hist, left));
            heap.push(box_entry(&hist, right));
            box_count += 1;
        } else {
            final_boxes.push(entry.box_);
        }
    }

    final_boxes.extend(heap.into_iter().map(|entry| entry.box_));
    final_boxes.sort_by(|a, b| {
        volume(&hist, b)
            .0
            .cmp(&volume(&hist, a).0)
            .then_with(|| variance(&hist, b).total_cmp(&variance(&hist, a)))
            .then_with(|| a.r0.cmp(&b.r0))
            .then_with(|| a.r1.cmp(&b.r1))
            .then_with(|| a.g0.cmp(&b.g0))
            .then_with(|| a.g1.cmp(&b.g1))
            .then_with(|| a.b0.cmp(&b.b0))
            .then_with(|| a.b1.cmp(&b.b1))
    });

    final_boxes
        .into_iter()
        .map(|box_| centroid(&hist, &box_))
        .collect()
}

/// Generate a flat RGB palette (3 bytes per color, max 768 bytes).
#[cfg(test)]
#[must_use]
pub(crate) fn generate_palette_flat(rgba_pixels: &[u8], max_colors: usize) -> Vec<u8> {
    generate_palette(rgba_pixels, max_colors)
        .into_iter()
        .flat_map(|(r, g, b)| [r, g, b])
        .collect()
}

/// Compute the weighted variance of colors in a box.
#[must_use]
pub(crate) fn variance(hist: &Histogram3D, box_: &ColorBox) -> f64 {
    let (wt, mr, mg, mb, m2) = volume(hist, box_);
    if wt == 0 {
        return 0.0;
    }

    let wt_f = wt as f64;
    m2 - ((mr as f64 * mr as f64) + (mg as f64 * mg as f64) + (mb as f64 * mb as f64)) / wt_f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bin_index() {
        assert_eq!(bin_index(0, 0, 0), bin_coord_index(1, 1, 1));
        assert_eq!(bin_index(255, 255, 255), bin_coord_index(32, 32, 32));
        assert_eq!(bin_index(7, 7, 7), bin_coord_index(1, 1, 1));
        assert_eq!(bin_index(8, 8, 8), bin_coord_index(2, 2, 2));
    }

    #[test]
    fn test_histogram_basic() {
        let mut pixels = Vec::new();
        for _ in 0..4 {
            pixels.extend_from_slice(&[255, 0, 0, 255]);
        }
        for _ in 0..4 {
            pixels.extend_from_slice(&[0, 0, 255, 255]);
        }

        let hist = build_histogram(&pixels);
        let red = bin_index(255, 0, 0);
        let blue = bin_index(0, 0, 255);

        assert_eq!(hist.wt[red], 4);
        assert_eq!(hist.mr[red], 1020);
        assert_eq!(hist.mb[red], 0);
        assert_eq!(hist.wt[blue], 4);
        assert_eq!(hist.mr[blue], 0);
        assert_eq!(hist.mb[blue], 1020);
    }

    #[test]
    fn test_histogram_transparent_excluded() {
        let pixels = [
            255, 0, 0, 255, // opaque
            0, 255, 0, 0, // transparent
            0, 0, 255, 127, // transparent
            0, 0, 255, 128, // included
        ];

        let hist = build_histogram(&pixels);
        let red = bin_index(255, 0, 0);
        let green = bin_index(0, 255, 0);
        let blue = bin_index(0, 0, 255);

        assert_eq!(hist.wt[red], 1);
        assert_eq!(hist.wt[green], 0);
        assert_eq!(hist.wt[blue], 1);
    }

    #[test]
    fn test_cumulative_moments() {
        let mut hist = Histogram3D::new();
        let a = bin_coord_index(1, 1, 1);
        let b = bin_coord_index(1, 2, 1);
        let c = bin_coord_index(2, 1, 1);

        hist.wt[a] = 1;
        hist.mr[a] = 2;
        hist.mg[a] = 3;
        hist.mb[a] = 4;
        hist.m2[a] = 29.0;

        hist.wt[b] = 4;
        hist.mr[b] = 40;
        hist.mg[b] = 50;
        hist.mb[b] = 60;
        hist.m2[b] = 770.0;

        hist.wt[c] = 2;
        hist.mr[c] = 10;
        hist.mg[c] = 20;
        hist.mb[c] = 30;
        hist.m2[c] = 140.0;

        compute_cumulative_moments(&mut hist);

        let cell_a = prefix_at(&hist, 1, 1, 1);
        assert_eq!(cell_a, (1, 2, 3, 4, 29.0));

        let cell_b = prefix_at(&hist, 1, 2, 1);
        assert_eq!(cell_b, (5, 42, 53, 64, 799.0));

        let cell_c = prefix_at(&hist, 2, 2, 1);
        assert_eq!(cell_c, (7, 52, 73, 94, 939.0));
    }

    #[test]
    fn test_volume_query() {
        let mut hist = Histogram3D::new();
        let a = bin_coord_index(1, 1, 1);
        let b = bin_coord_index(1, 2, 1);
        let c = bin_coord_index(2, 1, 1);

        hist.wt[a] = 1;
        hist.mr[a] = 2;
        hist.mg[a] = 3;
        hist.mb[a] = 4;
        hist.m2[a] = 29.0;

        hist.wt[b] = 4;
        hist.mr[b] = 40;
        hist.mg[b] = 50;
        hist.mb[b] = 60;
        hist.m2[b] = 770.0;

        hist.wt[c] = 2;
        hist.mr[c] = 10;
        hist.mg[c] = 20;
        hist.mb[c] = 30;
        hist.m2[c] = 140.0;

        compute_cumulative_moments(&mut hist);

        let box_ = ColorBox {
            r0: 0,
            r1: 1,
            g0: 0,
            g1: 2,
            b0: 0,
            b1: 1,
        };

        assert_eq!(volume(&hist, &box_), (5, 42, 53, 64, 799.0));
    }

    #[test]
    fn test_variance() {
        let mut hist = Histogram3D::new();
        let a = bin_coord_index(1, 1, 1);
        let b = bin_coord_index(1, 1, 2);

        hist.wt[a] = 2;
        hist.mr[a] = 20;
        hist.mg[a] = 40;
        hist.mb[a] = 60;
        hist.m2[a] = 2800.0;

        hist.wt[b] = 1;
        hist.mr[b] = 5;
        hist.mg[b] = 6;
        hist.mb[b] = 7;
        hist.m2[b] = 110.0;

        compute_cumulative_moments(&mut hist);

        let single = ColorBox {
            r0: 0,
            r1: 1,
            g0: 0,
            g1: 1,
            b0: 0,
            b1: 1,
        };
        assert_eq!(variance(&hist, &single), 0.0);

        let multi = ColorBox {
            r0: 0,
            r1: 1,
            g0: 0,
            g1: 1,
            b0: 0,
            b1: 2,
        };
        assert!(variance(&hist, &multi) > 0.0);
    }

    #[test]
    fn test_split_two_colors() {
        let mut pixels = Vec::new();
        for _ in 0..8 {
            pixels.extend_from_slice(&[255, 0, 0, 255]);
            pixels.extend_from_slice(&[0, 0, 255, 255]);
        }

        let mut hist = build_histogram(&pixels);
        compute_cumulative_moments(&mut hist);

        let box_ = ColorBox {
            r0: 0,
            r1: HIST_MAX_BIN,
            g0: 0,
            g1: HIST_MAX_BIN,
            b0: 0,
            b1: HIST_MAX_BIN,
        };

        let (left, right) = split_box(&hist, &box_).expect("split");
        let red = bin_coord_index(32, 1, 1);
        let blue = bin_coord_index(1, 1, 32);

        let left_red = left.r0 < 32
            && 32 <= left.r1
            && left.g0 < 1
            && 1 <= left.g1
            && left.b0 < 1
            && 1 <= left.b1;
        let right_red = right.r0 < 32
            && 32 <= right.r1
            && right.g0 < 1
            && 1 <= right.g1
            && right.b0 < 1
            && 1 <= right.b1;
        let left_blue = left.r0 < 1
            && 1 <= left.r1
            && left.g0 < 1
            && 1 <= left.g1
            && left.b0 < 32
            && 32 <= left.b1;
        let right_blue = right.r0 < 1
            && 1 <= right.r1
            && right.g0 < 1
            && 1 <= right.g1
            && right.b0 < 32
            && 32 <= right.b1;

        assert!((left_red || right_red) && (left_blue || right_blue));
        assert!(left_red != left_blue);
        assert_eq!(hist.wt[red], 8);
        assert_eq!(hist.wt[blue], 8);
    }

    #[test]
    fn test_generate_palette_two_colors() {
        let mut pixels = Vec::new();
        for _ in 0..8 {
            pixels.extend_from_slice(&[255, 0, 0, 255]);
            pixels.extend_from_slice(&[0, 0, 255, 255]);
        }

        let palette = generate_palette(&pixels, 2);
        assert_eq!(palette.len(), 2);
        assert!(palette
            .iter()
            .any(|&(r, g, b)| r >= 250 && g <= 5 && b <= 5));
        assert!(palette
            .iter()
            .any(|&(r, g, b)| r <= 5 && g <= 5 && b >= 250));
    }

    #[test]
    fn test_generate_palette_single_color() {
        let mut pixels = Vec::new();
        for _ in 0..16 {
            pixels.extend_from_slice(&[255, 0, 0, 255]);
        }

        let palette = generate_palette(&pixels, 8);
        assert_eq!(palette.len(), 1);
        assert_eq!(palette[0], (255, 0, 0));
    }

    #[test]
    fn test_generate_palette_empty() {
        assert_eq!(generate_palette(&[], 8), vec![(0, 0, 0)]);
    }

    #[test]
    fn test_generate_palette_max_256() {
        let mut pixels = Vec::new();
        for r in 0..16_u8 {
            for g in 0..16_u8 {
                pixels.extend_from_slice(&[r * 16, g * 16, 0, 255]);
            }
        }

        let palette = generate_palette(&pixels, 256);
        assert_eq!(palette.len(), 256);
    }

    #[test]
    fn test_palette_flat_format() {
        let mut pixels = Vec::new();
        for _ in 0..4 {
            pixels.extend_from_slice(&[255, 0, 0, 255]);
            pixels.extend_from_slice(&[0, 0, 255, 255]);
        }

        let palette = generate_palette(&pixels, 2);
        let flat = generate_palette_flat(&pixels, 2);
        assert_eq!(flat.len(), palette.len() * 3);
    }
}

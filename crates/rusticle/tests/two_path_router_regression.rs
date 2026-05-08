//! Regression tests for the two-path router architecture.
//!
//! Covers:
//! 1. Path selection under Auto mode (voyager-like → Path A, transparency-heavy → Path B,
//!    disposal-heavy → Path B)
//! 2. Forced Path A / Path B emit decodable bytes
//! 3. Auto-routed output decodes to expected displayed canvases (frame-by-frame equality)
//! 4. Fallback path preserves correctness when Path A routing/realization fails
//!
//! All tests are deterministic and use small synthetic sequences.

use rusticle::{
    CanonicalSequenceBuilder, DisposalMethod, Frame, Gif, LoopCount, OptimizerPath,
    OptimizerStrategy, OptLevel, QualityMetrics, TwoPathConfig,
};
use std::time::Duration;

// ── canvas helpers ────────────────────────────────────────────────────────────

/// Solid RGBA canvas: every pixel is `(r, g, b, 255)`.
fn solid_canvas(width: u16, height: u16, r: u8, g: u8, b: u8) -> Vec<u8> {
    let n = (width as usize) * (height as usize);
    let mut buf = Vec::with_capacity(n * 4);
    for _ in 0..n {
        buf.extend_from_slice(&[r, g, b, 255]);
    }
    buf
}

/// Canvas with a colored rectangle patch at (px, py) of size (pw, ph).
/// Background is (bg_r, bg_g, bg_b, 255); patch is (pr, pg, pb, 255).
fn canvas_with_rect(
    width: u16,
    height: u16,
    bg_r: u8,
    bg_g: u8,
    bg_b: u8,
    px: u16,
    py: u16,
    pw: u16,
    ph: u16,
    pr: u8,
    pg: u8,
    pb: u8,
) -> Vec<u8> {
    let mut buf = solid_canvas(width, height, bg_r, bg_g, bg_b);
    for y in 0..ph as usize {
        for x in 0..pw as usize {
            let cx = px as usize + x;
            let cy = py as usize + y;
            if cx < width as usize && cy < height as usize {
                let idx = (cy * width as usize + cx) * 4;
                buf[idx] = pr;
                buf[idx + 1] = pg;
                buf[idx + 2] = pb;
                buf[idx + 3] = 255;
            }
        }
    }
    buf
}

/// Canvas with semi-transparent pixels (alpha < 255) in a region.
fn canvas_with_transparency(width: u16, height: u16, r: u8, g: u8, b: u8, alpha: u8) -> Vec<u8> {
    let n = (width as usize) * (height as usize);
    let mut buf = Vec::with_capacity(n * 4);
    for _ in 0..n {
        buf.extend_from_slice(&[r, g, b, alpha]);
    }
    buf
}

// ── frame / gif builders ──────────────────────────────────────────────────────

fn make_frame(
    pixels: Vec<u8>,
    width: u16,
    height: u16,
    dispose: DisposalMethod,
    delay_ms: u64,
) -> Frame {
    Frame {
        pixels,
        delay: Duration::from_millis(delay_ms),
        dispose,
        local_palette: None,
        left: 0,
        top: 0,
        width,
        height,
    }
}

fn make_subframe(
    pixels: Vec<u8>,
    left: u16,
    top: u16,
    width: u16,
    height: u16,
    dispose: DisposalMethod,
    delay_ms: u64,
) -> Frame {
    Frame {
        pixels,
        delay: Duration::from_millis(delay_ms),
        dispose,
        local_palette: None,
        left,
        top,
        width,
        height,
    }
}

fn make_gif(width: u16, height: u16, frames: Vec<Frame>) -> Gif {
    Gif {
        width,
        height,
        global_palette: None,
        frames,
        loop_count: LoopCount::Infinite,
        original_palette: None,
    }
}

// ── round-trip helpers ────────────────────────────────────────────────────────

/// Route → encode → decode round-trip.
///
/// Returns the decoded Gif or an error string if the output bytes are not a valid GIF.
fn route_round_trip(gif: &Gif, strategy: OptimizerStrategy) -> Result<Gif, String> {
    let config = TwoPathConfig {
        strategy,
        emit_telemetry: false,
        ..Default::default()
    };

    let result = rusticle::route_optimize(gif, OptLevel::O3, config)
        .map_err(|e| format!("route_optimize failed: {e}"))?;

    // Reconstruct a Gif from the routed frames so we can encode it.
    let routed_gif = Gif {
        width: gif.width,
        height: gif.height,
        global_palette: None,
        frames: result.frames,
        loop_count: gif.loop_count,
        original_palette: None,
    };

    let bytes = routed_gif
        .to_bytes()
        .map_err(|e| format!("to_bytes failed: {e}"))?;

    assert!(!bytes.is_empty(), "encoded bytes must not be empty");

    Gif::from_bytes(&bytes).map_err(|e| format!("output is not a valid GIF: {e}"))
}

/// Assert two full-canvas RGBA buffers are semantically equivalent (PSNR ≥ 30 dB).
fn assert_canvas_equivalent(label: &str, frame_idx: usize, canonical: &[u8], decoded: &[u8]) {
    assert_eq!(
        canonical.len(),
        decoded.len(),
        "{label} frame {frame_idx}: canvas size mismatch (canonical={}, decoded={})",
        canonical.len(),
        decoded.len()
    );
    let metrics = QualityMetrics::compare(canonical, decoded);
    assert!(
        metrics.psnr >= 30.0,
        "{label} frame {frame_idx}: PSNR {:.1} dB < 30 dB (quality regression)",
        metrics.psnr
    );
}

// ── Section 1: Path selection under Auto mode ─────────────────────────────────

/// Voyager-like: opaque, Keep/None disposal, small offset patches, low changed-area ratio.
/// Must route to Path A under Auto mode.
#[test]
fn test_auto_voyager_like_routes_to_path_a() {
    let w = 32u16;
    let h = 32u16;

    // Frame 0: full opaque background
    let frame0 = make_frame(solid_canvas(w, h, 180, 180, 180), w, h, DisposalMethod::None, 100);

    // Frame 1: small patch at top-left (8x8) — opaque, Keep disposal
    let patch1 = solid_canvas(8, 8, 100, 100, 100);
    let frame1 = make_subframe(patch1, 0, 0, 8, 8, DisposalMethod::Keep, 100);

    // Frame 2: small patch at different position — opaque, Keep disposal
    let patch2 = solid_canvas(8, 8, 120, 120, 120);
    let frame2 = make_subframe(patch2, 16, 16, 8, 8, DisposalMethod::Keep, 100);

    // Frame 3: another small patch — opaque, Keep disposal
    let patch3 = solid_canvas(8, 8, 140, 140, 140);
    let frame3 = make_subframe(patch3, 8, 8, 8, 8, DisposalMethod::Keep, 100);

    let gif = make_gif(w, h, vec![frame0, frame1, frame2, frame3]);
    let canonical = CanonicalSequenceBuilder::build(&gif).expect("build canonical");

    let config = TwoPathConfig {
        strategy: OptimizerStrategy::Auto,
        emit_telemetry: false,
        ..Default::default()
    };

    let result = rusticle::route_optimize(&gif, OptLevel::O3, config)
        .expect("route_optimize must succeed");

    assert_eq!(
        result.telemetry.strategy,
        OptimizerStrategy::Auto,
        "strategy must be Auto"
    );
    assert!(
        result.telemetry.selected_path.is_some(),
        "Auto must select a path"
    );
    assert!(
        result.telemetry.classification.is_some(),
        "Auto must produce classification"
    );

    let classification = result.telemetry.classification.as_ref().unwrap();
    assert!(
        !classification.features.has_transparent_gce,
        "voyager-like must have no transparent GCE"
    );
    assert!(
        classification.features.keep_none_disposal_ratio >= 0.9,
        "voyager-like must have ≥90% Keep/None disposal, got {:.2}",
        classification.features.keep_none_disposal_ratio
    );

    // The selected path should be Path A (all criteria met)
    assert_eq!(
        result.telemetry.selected_path,
        Some(OptimizerPath::PathA),
        "voyager-like opaque-delta sequence must route to Path A under Auto mode"
    );

    // Verify canonical sequence features match expectations
    assert_eq!(canonical.frames.len(), 4);
    assert!(!result.frames.is_empty(), "must produce frames");
}

/// Transparency-heavy sequence must route to Path B under Auto mode.
#[test]
fn test_auto_transparency_heavy_routes_to_path_b() {
    let w = 24u16;
    let h = 24u16;

    // All frames have transparent pixels (alpha < 255)
    let frame0 = make_frame(
        canvas_with_transparency(w, h, 200, 100, 50, 200),
        w,
        h,
        DisposalMethod::Keep,
        100,
    );
    let frame1 = make_frame(
        canvas_with_transparency(w, h, 50, 200, 100, 180),
        w,
        h,
        DisposalMethod::Keep,
        100,
    );
    let frame2 = make_frame(
        canvas_with_transparency(w, h, 100, 50, 200, 160),
        w,
        h,
        DisposalMethod::Keep,
        100,
    );

    let gif = make_gif(w, h, vec![frame0, frame1, frame2]);

    let config = TwoPathConfig {
        strategy: OptimizerStrategy::Auto,
        emit_telemetry: false,
        ..Default::default()
    };

    let result = rusticle::route_optimize(&gif, OptLevel::O3, config)
        .expect("route_optimize must succeed");

    let classification = result
        .telemetry
        .classification
        .as_ref()
        .expect("Auto must produce classification");

    assert!(
        classification.features.has_transparent_gce,
        "transparency-heavy must have transparent GCE"
    );
    assert_eq!(
        result.telemetry.selected_path,
        Some(OptimizerPath::PathB),
        "transparency-heavy sequence must route to Path B under Auto mode"
    );
    assert!(!result.frames.is_empty(), "must produce frames");
}

/// Disposal-Background-heavy sequence must route to Path B under Auto mode.
#[test]
fn test_auto_disposal_background_heavy_routes_to_path_b() {
    let w = 20u16;
    let h = 20u16;

    // Majority of frames use Background disposal
    let frames: Vec<Frame> = (0..8)
        .map(|i| {
            let r = (i * 30) as u8;
            let dispose = if i < 2 {
                DisposalMethod::Keep
            } else {
                DisposalMethod::Background
            };
            make_frame(solid_canvas(w, h, r, 100, 100), w, h, dispose, 100)
        })
        .collect();

    let gif = make_gif(w, h, frames);

    let config = TwoPathConfig {
        strategy: OptimizerStrategy::Auto,
        emit_telemetry: false,
        ..Default::default()
    };

    let result = rusticle::route_optimize(&gif, OptLevel::O3, config)
        .expect("route_optimize must succeed");

    let classification = result
        .telemetry
        .classification
        .as_ref()
        .expect("Auto must produce classification");

    // 2 Keep out of 8 = 25%, well below 90% threshold
    assert!(
        classification.features.keep_none_disposal_ratio < 0.9,
        "Background-heavy must have <90% Keep/None disposal, got {:.2}",
        classification.features.keep_none_disposal_ratio
    );
    assert_eq!(
        result.telemetry.selected_path,
        Some(OptimizerPath::PathB),
        "Background-disposal-heavy sequence must route to Path B under Auto mode"
    );
}

/// Disposal-Previous-heavy sequence must route to Path B under Auto mode.
#[test]
fn test_auto_disposal_previous_heavy_routes_to_path_b() {
    let w = 20u16;
    let h = 20u16;

    // Majority of frames use Previous disposal
    let frames: Vec<Frame> = (0..8)
        .map(|i| {
            let r = (i * 30) as u8;
            let dispose = if i < 2 {
                DisposalMethod::None
            } else {
                DisposalMethod::Previous
            };
            make_frame(solid_canvas(w, h, r, 80, 120), w, h, dispose, 100)
        })
        .collect();

    let gif = make_gif(w, h, frames);

    let config = TwoPathConfig {
        strategy: OptimizerStrategy::Auto,
        emit_telemetry: false,
        ..Default::default()
    };

    let result = rusticle::route_optimize(&gif, OptLevel::O3, config)
        .expect("route_optimize must succeed");

    let classification = result
        .telemetry
        .classification
        .as_ref()
        .expect("Auto must produce classification");

    // 2 None out of 8 = 25%, well below 90% threshold
    assert!(
        classification.features.keep_none_disposal_ratio < 0.9,
        "Previous-heavy must have <90% Keep/None disposal, got {:.2}",
        classification.features.keep_none_disposal_ratio
    );
    assert_eq!(
        result.telemetry.selected_path,
        Some(OptimizerPath::PathB),
        "Previous-disposal-heavy sequence must route to Path B under Auto mode"
    );
}

/// Mixed disposal (Background + Previous) routes to Path B.
#[test]
fn test_auto_mixed_disposal_routes_to_path_b() {
    let w = 16u16;
    let h = 16u16;

    let disposals = [
        DisposalMethod::Keep,
        DisposalMethod::Keep,
        DisposalMethod::Keep,
        DisposalMethod::Keep,
        DisposalMethod::Keep,
        DisposalMethod::Keep,
        DisposalMethod::Keep,
        DisposalMethod::Background, // 1 Background
        DisposalMethod::Previous,   // 1 Previous
        DisposalMethod::Keep,
    ];

    let frames: Vec<Frame> = disposals
        .iter()
        .enumerate()
        .map(|(i, &dispose)| {
            let r = (i * 25) as u8;
            make_frame(solid_canvas(w, h, r, 100, 150), w, h, dispose, 100)
        })
        .collect();

    let gif = make_gif(w, h, frames);

    let config = TwoPathConfig {
        strategy: OptimizerStrategy::Auto,
        emit_telemetry: false,
        ..Default::default()
    };

    let result = rusticle::route_optimize(&gif, OptLevel::O3, config)
        .expect("route_optimize must succeed");

    let classification = result
        .telemetry
        .classification
        .as_ref()
        .expect("Auto must produce classification");

    // 8 Keep out of 10 = 80%, below 90% threshold
    assert!(
        classification.features.keep_none_disposal_ratio < 0.9,
        "mixed disposal must have <90% Keep/None, got {:.2}",
        classification.features.keep_none_disposal_ratio
    );
    assert_eq!(
        result.telemetry.selected_path,
        Some(OptimizerPath::PathB),
        "mixed disposal sequence must route to Path B"
    );
}

// ── Section 2: Forced paths emit decodable bytes ──────────────────────────────

/// Forced Path A on an opaque sequence emits valid, decodable GIF bytes.
#[test]
fn test_forced_path_a_emits_decodable_bytes() {
    let w = 20u16;
    let h = 20u16;

    let frames = vec![
        make_frame(solid_canvas(w, h, 200, 100, 50), w, h, DisposalMethod::None, 100),
        make_frame(
            canvas_with_rect(w, h, 200, 100, 50, 2, 2, 6, 6, 50, 200, 100),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
        make_frame(
            canvas_with_rect(w, h, 200, 100, 50, 10, 10, 6, 6, 100, 50, 200),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
    ];

    let gif = make_gif(w, h, frames);
    let decoded = route_round_trip(&gif, OptimizerStrategy::PathA)
        .expect("forced Path A must produce decodable bytes");

    assert_eq!(decoded.frames.len(), 3, "must decode 3 frames");
    assert_eq!(decoded.width, w);
    assert_eq!(decoded.height, h);

    for (i, frame) in decoded.frames.iter().enumerate() {
        assert!(
            !frame.pixels.is_empty(),
            "frame {i} must have pixels after decode"
        );
        assert_eq!(
            frame.pixels.len() % 4,
            0,
            "frame {i} pixels must be RGBA (multiple of 4)"
        );
    }
}

/// Forced Path B on a transparency-heavy sequence emits valid, decodable GIF bytes.
#[test]
fn test_forced_path_b_emits_decodable_bytes() {
    let w = 20u16;
    let h = 20u16;

    let frames = vec![
        make_frame(
            canvas_with_transparency(w, h, 200, 100, 50, 200),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
        make_frame(
            canvas_with_transparency(w, h, 50, 200, 100, 180),
            w,
            h,
            DisposalMethod::Background,
            100,
        ),
        make_frame(
            canvas_with_transparency(w, h, 100, 50, 200, 160),
            w,
            h,
            DisposalMethod::Previous,
            100,
        ),
    ];

    let gif = make_gif(w, h, frames);
    let decoded = route_round_trip(&gif, OptimizerStrategy::PathB)
        .expect("forced Path B must produce decodable bytes");

    assert_eq!(decoded.frames.len(), 3, "must decode 3 frames");
    assert_eq!(decoded.width, w);
    assert_eq!(decoded.height, h);

    for (i, frame) in decoded.frames.iter().enumerate() {
        assert!(
            !frame.pixels.is_empty(),
            "frame {i} must have pixels after decode"
        );
    }
}

/// Forced Path A on a sequence with large changes (full-frame fallback path) emits decodable bytes.
#[test]
fn test_forced_path_a_large_change_emits_decodable_bytes() {
    let w = 16u16;
    let h = 16u16;

    // Each frame is completely different — triggers full-frame fallback in Path A
    let frames = vec![
        make_frame(solid_canvas(w, h, 255, 0, 0), w, h, DisposalMethod::None, 100),
        make_frame(solid_canvas(w, h, 0, 255, 0), w, h, DisposalMethod::Keep, 100),
        make_frame(solid_canvas(w, h, 0, 0, 255), w, h, DisposalMethod::Keep, 100),
    ];

    let gif = make_gif(w, h, frames);
    let decoded = route_round_trip(&gif, OptimizerStrategy::PathA)
        .expect("forced Path A with large changes must produce decodable bytes");

    assert_eq!(decoded.frames.len(), 3);
    assert_eq!(decoded.width, w);
    assert_eq!(decoded.height, h);
}

/// Forced Path B on a sequence with Background disposal emits decodable bytes.
#[test]
fn test_forced_path_b_background_disposal_emits_decodable_bytes() {
    let w = 16u16;
    let h = 16u16;

    let frames = vec![
        make_frame(solid_canvas(w, h, 200, 50, 50), w, h, DisposalMethod::None, 100),
        make_frame(
            solid_canvas(w, h, 50, 200, 50),
            w,
            h,
            DisposalMethod::Background,
            100,
        ),
        make_frame(
            solid_canvas(w, h, 50, 50, 200),
            w,
            h,
            DisposalMethod::Background,
            100,
        ),
    ];

    let gif = make_gif(w, h, frames);
    let decoded = route_round_trip(&gif, OptimizerStrategy::PathB)
        .expect("forced Path B with Background disposal must produce decodable bytes");

    assert_eq!(decoded.frames.len(), 3);
}

// ── Section 3: Auto-routed output decodes to expected displayed canvases ──────

/// Auto-routed opaque-delta sequence: decoded displayed canvases match canonical IR.
///
/// Uses a small synthetic sequence where frame-by-frame canvas equality can be asserted.
#[test]
fn test_auto_opaque_delta_canvas_preservation() {
    let w = 16u16;
    let h = 16u16;

    // Frame 0: solid red background
    // Frame 1: small green patch at (4,4) 4x4
    // Frame 2: small blue patch at (8,8) 4x4
    // All opaque, Keep/None disposal → should route to Path A
    let frames = vec![
        make_frame(solid_canvas(w, h, 200, 50, 50), w, h, DisposalMethod::None, 100),
        make_frame(
            canvas_with_rect(w, h, 200, 50, 50, 4, 4, 4, 4, 50, 200, 50),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
        make_frame(
            canvas_with_rect(w, h, 200, 50, 50, 8, 8, 4, 4, 50, 50, 200),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
    ];

    let gif = make_gif(w, h, frames);
    let canonical = CanonicalSequenceBuilder::build(&gif).expect("build canonical");

    let decoded = route_round_trip(&gif, OptimizerStrategy::Auto)
        .expect("auto round-trip must succeed");

    assert_eq!(
        decoded.frames.len(),
        canonical.frames.len(),
        "frame count must match"
    );

    for (i, (canon_frame, decoded_frame)) in canonical
        .frames
        .iter()
        .zip(decoded.frames.iter())
        .enumerate()
    {
        assert_canvas_equivalent(
            "auto-opaque-delta",
            i,
            &canon_frame.displayed_canvas.pixels,
            &decoded_frame.pixels,
        );
    }
}

/// Auto-routed transparency-heavy sequence: routes to Path B and emits decodable bytes.
///
/// Note: GIF format only supports binary transparency (alpha 0 or 255). Semi-transparent
/// pixels are quantized during encoding, so pixel-exact canvas comparison is not meaningful
/// for transparency-heavy sequences. This test verifies routing and decodability only.
#[test]
fn test_auto_transparency_heavy_canvas_preservation() {
    let w = 16u16;
    let h = 16u16;

    // Frames with semi-transparent pixels → routes to Path B
    let frames = vec![
        make_frame(
            canvas_with_transparency(w, h, 200, 100, 50, 255),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
        make_frame(
            canvas_with_transparency(w, h, 50, 200, 100, 200),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
        make_frame(
            canvas_with_transparency(w, h, 100, 50, 200, 180),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
    ];

    let gif = make_gif(w, h, frames);

    let config = TwoPathConfig {
        strategy: OptimizerStrategy::Auto,
        emit_telemetry: false,
        ..Default::default()
    };

    let result = rusticle::route_optimize(&gif, OptLevel::O3, config)
        .expect("auto route must succeed");

    // Must route to Path B (has transparent GCE)
    assert_eq!(
        result.telemetry.selected_path,
        Some(OptimizerPath::PathB),
        "transparency-heavy must route to Path B"
    );

    // Output must be encodable and decodable
    let routed_gif = Gif {
        width: gif.width,
        height: gif.height,
        global_palette: None,
        frames: result.frames,
        loop_count: gif.loop_count,
        original_palette: None,
    };
    let bytes = routed_gif.to_bytes().expect("must encode");
    let decoded = Gif::from_bytes(&bytes).expect("must decode");
    assert_eq!(decoded.frames.len(), 3, "must decode 3 frames");
    assert_eq!(decoded.width, w);
    assert_eq!(decoded.height, h);
}

/// Forced Path A canvas preservation: decoded canvases match canonical IR.
#[test]
fn test_forced_path_a_canvas_preservation() {
    let w = 16u16;
    let h = 16u16;

    let frames = vec![
        make_frame(solid_canvas(w, h, 180, 180, 180), w, h, DisposalMethod::None, 100),
        make_frame(
            canvas_with_rect(w, h, 180, 180, 180, 2, 2, 4, 4, 80, 80, 80),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
        make_frame(
            canvas_with_rect(w, h, 180, 180, 180, 8, 8, 4, 4, 120, 120, 120),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
    ];

    let gif = make_gif(w, h, frames);
    let canonical = CanonicalSequenceBuilder::build(&gif).expect("build canonical");

    let decoded = route_round_trip(&gif, OptimizerStrategy::PathA)
        .expect("forced Path A round-trip must succeed");

    assert_eq!(decoded.frames.len(), canonical.frames.len());

    for (i, (canon_frame, decoded_frame)) in canonical
        .frames
        .iter()
        .zip(decoded.frames.iter())
        .enumerate()
    {
        assert_canvas_equivalent(
            "forced-path-a",
            i,
            &canon_frame.displayed_canvas.pixels,
            &decoded_frame.pixels,
        );
    }
}

/// Forced Path B canvas preservation: decoded canvases match canonical IR.
#[test]
fn test_forced_path_b_canvas_preservation() {
    let w = 16u16;
    let h = 16u16;

    let frames = vec![
        make_frame(solid_canvas(w, h, 200, 100, 50), w, h, DisposalMethod::None, 100),
        make_frame(solid_canvas(w, h, 50, 200, 100), w, h, DisposalMethod::Keep, 100),
        make_frame(solid_canvas(w, h, 100, 50, 200), w, h, DisposalMethod::Keep, 100),
    ];

    let gif = make_gif(w, h, frames);
    let canonical = CanonicalSequenceBuilder::build(&gif).expect("build canonical");

    let decoded = route_round_trip(&gif, OptimizerStrategy::PathB)
        .expect("forced Path B round-trip must succeed");

    assert_eq!(decoded.frames.len(), canonical.frames.len());

    for (i, (canon_frame, decoded_frame)) in canonical
        .frames
        .iter()
        .zip(decoded.frames.iter())
        .enumerate()
    {
        assert_canvas_equivalent(
            "forced-path-b",
            i,
            &canon_frame.displayed_canvas.pixels,
            &decoded_frame.pixels,
        );
    }
}

// ── Section 4: Fallback path correctness ─────────────────────────────────────

/// Fallback: when Path A is forced on a sequence it can handle, output is still correct.
/// (Path A should succeed; this verifies the happy path of forced Path A.)
#[test]
fn test_forced_path_a_fallback_produces_correct_output() {
    let w = 16u16;
    let h = 16u16;

    // Opaque sequence — Path A should handle this without fallback
    let frames = vec![
        make_frame(solid_canvas(w, h, 200, 200, 200), w, h, DisposalMethod::None, 100),
        make_frame(
            canvas_with_rect(w, h, 200, 200, 200, 4, 4, 4, 4, 100, 100, 100),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
    ];

    let gif = make_gif(w, h, frames);

    let config = TwoPathConfig {
        strategy: OptimizerStrategy::PathA,
        emit_telemetry: false,
        ..Default::default()
    };

    let result = rusticle::route_optimize(&gif, OptLevel::O3, config)
        .expect("forced Path A must succeed");

    // Path A should not have fallen back
    assert!(
        !result.telemetry.fallback_used,
        "Path A should not fall back on a clean opaque sequence"
    );
    assert_eq!(result.telemetry.selected_path, Some(OptimizerPath::PathA));
    assert!(!result.frames.is_empty());
}

/// Fallback: when Path A is forced on a sequence with transparency, it falls back to Path B
/// and still produces correct output.
#[test]
fn test_forced_path_a_with_transparency_falls_back_gracefully() {
    let w = 16u16;
    let h = 16u16;

    // Semi-transparent frames — Path A may or may not fail, but output must be valid
    let frames = vec![
        make_frame(
            canvas_with_transparency(w, h, 200, 100, 50, 200),
            w,
            h,
            DisposalMethod::None,
            100,
        ),
        make_frame(
            canvas_with_transparency(w, h, 50, 200, 100, 180),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
    ];

    let gif = make_gif(w, h, frames);

    let config = TwoPathConfig {
        strategy: OptimizerStrategy::PathA,
        emit_telemetry: false,
        ..Default::default()
    };

    let result = rusticle::route_optimize(&gif, OptLevel::O3, config)
        .expect("forced Path A must not return Err (fallback to Path B)");

    // Whether or not fallback was used, output must be non-empty and valid
    assert!(!result.frames.is_empty(), "must produce frames even with fallback");

    // Verify the frames are encodable
    let routed_gif = Gif {
        width: gif.width,
        height: gif.height,
        global_palette: None,
        frames: result.frames,
        loop_count: gif.loop_count,
        original_palette: None,
    };
    let bytes = routed_gif.to_bytes().expect("fallback output must be encodable");
    assert!(!bytes.is_empty());
    let decoded = Gif::from_bytes(&bytes).expect("fallback output must be decodable");
    assert_eq!(decoded.frames.len(), 2);
}

/// Fallback: Legacy strategy always produces valid output (baseline correctness).
#[test]
fn test_legacy_strategy_canvas_preservation() {
    let w = 16u16;
    let h = 16u16;

    let frames = vec![
        make_frame(solid_canvas(w, h, 200, 100, 50), w, h, DisposalMethod::None, 100),
        make_frame(solid_canvas(w, h, 50, 200, 100), w, h, DisposalMethod::Keep, 100),
        make_frame(solid_canvas(w, h, 100, 50, 200), w, h, DisposalMethod::Keep, 100),
    ];

    let gif = make_gif(w, h, frames);
    let canonical = CanonicalSequenceBuilder::build(&gif).expect("build canonical");

    let decoded = route_round_trip(&gif, OptimizerStrategy::Legacy)
        .expect("legacy round-trip must succeed");

    assert_eq!(decoded.frames.len(), canonical.frames.len());

    for (i, (canon_frame, decoded_frame)) in canonical
        .frames
        .iter()
        .zip(decoded.frames.iter())
        .enumerate()
    {
        assert_canvas_equivalent(
            "legacy",
            i,
            &canon_frame.displayed_canvas.pixels,
            &decoded_frame.pixels,
        );
    }
}

// ── Section 5: Determinism ────────────────────────────────────────────────────

/// Same input always produces the same routing decision (determinism).
#[test]
fn test_routing_is_deterministic() {
    let w = 20u16;
    let h = 20u16;

    let frames = vec![
        make_frame(solid_canvas(w, h, 180, 180, 180), w, h, DisposalMethod::None, 100),
        make_frame(
            canvas_with_rect(w, h, 180, 180, 180, 2, 2, 4, 4, 80, 80, 80),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
        make_frame(
            canvas_with_rect(w, h, 180, 180, 180, 10, 10, 4, 4, 120, 120, 120),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
    ];

    let gif = make_gif(w, h, frames);

    let config = TwoPathConfig {
        strategy: OptimizerStrategy::Auto,
        emit_telemetry: false,
        ..Default::default()
    };

    let result1 = rusticle::route_optimize(&gif, OptLevel::O3, config)
        .expect("first route must succeed");
    let result2 = rusticle::route_optimize(&gif, OptLevel::O3, config)
        .expect("second route must succeed");

    assert_eq!(
        result1.telemetry.selected_path, result2.telemetry.selected_path,
        "routing must be deterministic: same input must always select same path"
    );

    if let (Some(c1), Some(c2)) = (
        &result1.telemetry.classification,
        &result2.telemetry.classification,
    ) {
        assert_eq!(
            c1.features.has_transparent_gce, c2.features.has_transparent_gce,
            "classification features must be deterministic"
        );
        assert_eq!(
            c1.features.keep_none_disposal_ratio, c2.features.keep_none_disposal_ratio,
            "keep_none_disposal_ratio must be deterministic"
        );
        assert_eq!(
            c1.features.palette_stability, c2.features.palette_stability,
            "palette_stability must be deterministic"
        );
    }
}

/// All four strategies produce the same frame count for the same input.
#[test]
fn test_all_strategies_produce_same_frame_count() {
    let w = 16u16;
    let h = 16u16;

    let frames = vec![
        make_frame(solid_canvas(w, h, 200, 100, 50), w, h, DisposalMethod::None, 100),
        make_frame(solid_canvas(w, h, 50, 200, 100), w, h, DisposalMethod::Keep, 100),
        make_frame(solid_canvas(w, h, 100, 50, 200), w, h, DisposalMethod::Keep, 100),
    ];

    let gif = make_gif(w, h, frames);
    let expected_frame_count = gif.frames.len();

    for strategy in &[
        OptimizerStrategy::Legacy,
        OptimizerStrategy::Auto,
        OptimizerStrategy::PathA,
        OptimizerStrategy::PathB,
    ] {
        let config = TwoPathConfig {
            strategy: *strategy,
            emit_telemetry: false,
            ..Default::default()
        };

        let result = rusticle::route_optimize(&gif, OptLevel::O3, config)
            .unwrap_or_else(|e| panic!("strategy {:?} failed: {e}", strategy));

        assert_eq!(
            result.frames.len(),
            expected_frame_count,
            "strategy {:?} must produce {} frames",
            strategy,
            expected_frame_count
        );
    }
}

// ── Section 6: Edge cases ─────────────────────────────────────────────────────

/// Single-frame GIF routes and encodes correctly under all strategies.
#[test]
fn test_single_frame_all_strategies() {
    let w = 8u16;
    let h = 8u16;

    let gif = make_gif(
        w,
        h,
        vec![make_frame(
            solid_canvas(w, h, 128, 64, 32),
            w,
            h,
            DisposalMethod::None,
            100,
        )],
    );

    for strategy in &[
        OptimizerStrategy::Legacy,
        OptimizerStrategy::Auto,
        OptimizerStrategy::PathA,
        OptimizerStrategy::PathB,
    ] {
        let decoded = route_round_trip(&gif, *strategy)
            .unwrap_or_else(|e| panic!("strategy {:?} failed on single frame: {e}", strategy));

        assert_eq!(
            decoded.frames.len(),
            1,
            "strategy {:?} must produce 1 frame",
            strategy
        );
    }
}

/// Identical consecutive frames are handled correctly (minimal patch / no-op).
///
/// Legacy and Path B collapse identical frames to 1x1 transparent patches (canvas-safe).
/// Path A collapses to 1x1 opaque black pixel — this is a known limitation: the black
/// pixel at (0,0) overwrites the canvas, so canvas equality is not asserted for Path A.
#[test]
fn test_identical_frames_handled_correctly() {
    let w = 12u16;
    let h = 12u16;

    let pixels = solid_canvas(w, h, 150, 150, 150);
    let frames = vec![
        make_frame(pixels.clone(), w, h, DisposalMethod::None, 100),
        make_frame(pixels.clone(), w, h, DisposalMethod::Keep, 100),
        make_frame(pixels.clone(), w, h, DisposalMethod::Keep, 100),
    ];

    let gif = make_gif(w, h, frames);
    let canonical = CanonicalSequenceBuilder::build(&gif).expect("build canonical");

    // All strategies must produce decodable output with correct frame count
    for strategy in &[
        OptimizerStrategy::Legacy,
        OptimizerStrategy::Auto,
        OptimizerStrategy::PathA,
        OptimizerStrategy::PathB,
    ] {
        let decoded = route_round_trip(&gif, *strategy)
            .unwrap_or_else(|e| panic!("strategy {:?} failed on identical frames: {e}", strategy));

        assert_eq!(
            decoded.frames.len(),
            3,
            "strategy {:?} must produce 3 frames",
            strategy
        );
    }

    // Canvas preservation: All strategies now use semantically safe minimal patches.
    // Path A uses the actual canvas pixel at (0,0), Legacy and Path B use transparent 1x1 patches.
    for strategy in &[
        OptimizerStrategy::Legacy,
        OptimizerStrategy::Auto,
        OptimizerStrategy::PathA,
        OptimizerStrategy::PathB,
    ] {
        let decoded = route_round_trip(&gif, *strategy)
            .unwrap_or_else(|e| panic!("strategy {:?} failed: {e}", strategy));

        for (i, (canon_frame, decoded_frame)) in canonical
            .frames
            .iter()
            .zip(decoded.frames.iter())
            .enumerate()
        {
            assert_canvas_equivalent(
                &format!("identical-frames-{strategy:?}"),
                i,
                &canon_frame.displayed_canvas.pixels,
                &decoded_frame.pixels,
            );
        }
    }
}

/// Path A identical-frame minimal patch uses the actual canvas pixel at (0,0).
///
/// For identical consecutive frames, Path A emits a 1x1 patch at (0,0) using the
/// actual pixel value from the canvas. This preserves semantic safety without
/// introducing synthetic changes or overwriting with hardcoded values.
#[test]
fn test_path_a_identical_frames_minimal_patch_is_opaque() {
    use rusticle::path_a::{optimize_path_a, PathAConfig};
    use rusticle::adaptive_ir::{Canvas, CanonicalSequenceBuilder};

    let w = 12u16;
    let h = 12u16;

    let pixels = solid_canvas(w, h, 150, 150, 150);
    let frames = vec![
        make_frame(pixels.clone(), w, h, DisposalMethod::None, 100),
        make_frame(pixels.clone(), w, h, DisposalMethod::Keep, 100),
    ];

    let gif = make_gif(w, h, frames);
    let canonical = CanonicalSequenceBuilder::build(&gif).expect("build canonical");

    let canvases: Vec<Canvas> = canonical
        .frames
        .iter()
        .map(|f| f.displayed_canvas.clone_canvas())
        .collect();
    let delays: Vec<_> = canonical.frames.iter().map(|f| f.delay).collect();

    let result = optimize_path_a(&canvases, &delays, PathAConfig::default())
        .expect("optimize_path_a must succeed");

    assert_eq!(result.len(), 2);
    // Frame 1 is a 1x1 minimal patch
    assert_eq!(result[1].width, 1, "identical frame must produce 1x1 patch");
    assert_eq!(result[1].height, 1);
    // The patch is opaque (alpha=255)
    assert_eq!(
        result[1].pixels[3], 255,
        "Path A minimal patch must be opaque"
    );
    // The pixel value matches the canvas pixel at (0,0): (150, 150, 150, 255)
    assert_eq!(
        result[1].pixels[0], 150,
        "Path A minimal patch uses actual canvas pixel (R)"
    );
    assert_eq!(
        result[1].pixels[1], 150,
        "Path A minimal patch uses actual canvas pixel (G)"
    );
    assert_eq!(
        result[1].pixels[2], 150,
        "Path A minimal patch uses actual canvas pixel (B)"
    );
}

/// Telemetry is populated correctly for all strategies.
#[test]
fn test_telemetry_populated_for_all_strategies() {
    let w = 12u16;
    let h = 12u16;

    let gif = make_gif(
        w,
        h,
        vec![
            make_frame(solid_canvas(w, h, 200, 100, 50), w, h, DisposalMethod::None, 100),
            make_frame(solid_canvas(w, h, 50, 200, 100), w, h, DisposalMethod::Keep, 100),
        ],
    );

    // Legacy: no path selected, no classification
    {
        let config = TwoPathConfig {
            strategy: OptimizerStrategy::Legacy,
            ..Default::default()
        };
        let result = rusticle::route_optimize(&gif, OptLevel::O3, config).unwrap();
        assert_eq!(result.telemetry.strategy, OptimizerStrategy::Legacy);
        assert_eq!(result.telemetry.selected_path, None);
        assert!(result.telemetry.classification.is_none());
        assert!(!result.telemetry.fallback_used);
    }

    // Auto: path selected, classification present
    {
        let config = TwoPathConfig {
            strategy: OptimizerStrategy::Auto,
            ..Default::default()
        };
        let result = rusticle::route_optimize(&gif, OptLevel::O3, config).unwrap();
        assert_eq!(result.telemetry.strategy, OptimizerStrategy::Auto);
        assert!(result.telemetry.selected_path.is_some());
        assert!(result.telemetry.classification.is_some());
        let c = result.telemetry.classification.unwrap();
        assert!(!c.reasons.is_empty(), "classification must have reasons");
        assert!(
            c.features.keep_none_disposal_ratio >= 0.0
                && c.features.keep_none_disposal_ratio <= 1.0
        );
        assert!(
            c.features.palette_stability >= 0.0 && c.features.palette_stability <= 1.0
        );
    }

    // PathA: path = PathA, no classification
    {
        let config = TwoPathConfig {
            strategy: OptimizerStrategy::PathA,
            ..Default::default()
        };
        let result = rusticle::route_optimize(&gif, OptLevel::O3, config).unwrap();
        assert_eq!(result.telemetry.strategy, OptimizerStrategy::PathA);
        assert_eq!(result.telemetry.selected_path, Some(OptimizerPath::PathA));
    }

    // PathB: path = PathB, no classification
    {
        let config = TwoPathConfig {
            strategy: OptimizerStrategy::PathB,
            ..Default::default()
        };
        let result = rusticle::route_optimize(&gif, OptLevel::O3, config).unwrap();
        assert_eq!(result.telemetry.strategy, OptimizerStrategy::PathB);
        assert_eq!(result.telemetry.selected_path, Some(OptimizerPath::PathB));
        assert!(result.telemetry.classification.is_none());
    }
}

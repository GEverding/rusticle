#![cfg(feature = "research")]

//! Regression tests: adaptive emitted bytes preserve canonical display semantics.
//!
//! Each test:
//! 1. Constructs a small synthetic GIF with known canvas semantics.
//! 2. Encodes via `encode_adaptive(enabled: true)`.
//! 3. Decodes the output bytes back to full-canvas composited frames.
//! 4. Compares each decoded frame's displayed canvas against the canonical IR's
//!    `displayed_canvas` (frame-by-frame pixel equality or PSNR ≥ 30 dB).
//!
//! Tests are deterministic, self-contained, and fast (small synthetic GIFs only).

use rusticle::{
    AdaptiveConfig, CanonicalSequenceBuilder, DisposalMethod, Frame, Gif, LoopCount, QualityMetrics,
};
use std::time::Duration;

// ── helpers ──────────────────────────────────────────────────────────────────

/// Solid RGBA canvas: every pixel is `(r, g, b, 255)`.
fn solid_canvas(width: u16, height: u16, r: u8, g: u8, b: u8) -> Vec<u8> {
    let n = (width as usize) * (height as usize);
    let mut buf = Vec::with_capacity(n * 4);
    for _ in 0..n {
        buf.extend_from_slice(&[r, g, b, 255]);
    }
    buf
}

/// Build a `Frame` with full-canvas pixels and given disposal.
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

/// Build a `Frame` with a subframe offset.
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

/// Wrap frames into a `Gif`.
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

/// Run adaptive encode → decode and return the decoded Gif.
///
/// Returns `Err(String)` if encoding succeeds but the output bytes are not a valid GIF
/// (e.g., due to known bugs in the adaptive encoder). Panics if encoding itself fails.
fn adaptive_round_trip(gif: &Gif) -> Result<(rusticle::AdaptiveDecision, Gif), String> {
    let config = AdaptiveConfig {
        enabled: true,
        emit_telemetry: false,
    };
    let (decision, bytes) = gif
        .encode_adaptive(&config)
        .expect("encode_adaptive must not fail");
    assert!(!bytes.is_empty(), "adaptive output must not be empty");
    let decoded =
        Gif::from_bytes(&bytes).map_err(|e| format!("adaptive output is not a valid GIF: {e}"))?;
    Ok((decision, decoded))
}

/// Assert that two full-canvas RGBA buffers are semantically equivalent.
///
/// "Equivalent" means PSNR ≥ 30 dB (accounting for palette quantization loss).
/// For exact-match cases (no quantization expected) we assert pixel equality.
fn assert_canvas_equivalent(
    label: &str,
    frame_idx: usize,
    canonical: &[u8],
    decoded: &[u8],
    exact: bool,
) {
    assert_eq!(
        canonical.len(),
        decoded.len(),
        "{label} frame {frame_idx}: canvas size mismatch (canonical={}, decoded={})",
        canonical.len(),
        decoded.len()
    );

    if exact {
        assert_eq!(
            canonical, decoded,
            "{label} frame {frame_idx}: pixel mismatch (expected exact match)"
        );
    } else {
        let metrics = QualityMetrics::compare(canonical, decoded);
        assert!(
            metrics.psnr >= 30.0,
            "{label} frame {frame_idx}: PSNR {:.1} dB < 30 dB threshold (quality regression)",
            metrics.psnr
        );
    }
}

/// Reconstruct displayed canvases from a decoded Gif.
///
/// `Gif::from_bytes` already composites frames onto a canvas and stores the
/// full-canvas RGBA in each `Frame::pixels`. So `decoded.frames[i].pixels`
/// IS the displayed canvas for frame i.
fn displayed_canvases(decoded: &Gif) -> Vec<&[u8]> {
    decoded.frames.iter().map(|f| f.pixels.as_slice()).collect()
}

// ── test 1: single-frame GIF (degenerate case) ───────────────────────────────

#[test]
fn test_adaptive_single_frame_canvas_preserved() {
    let w = 20u16;
    let h = 20u16;
    let pixels = solid_canvas(w, h, 200, 100, 50);
    let gif = make_gif(
        w,
        h,
        vec![make_frame(pixels.clone(), w, h, DisposalMethod::Keep, 100)],
    );

    let canonical_seq = CanonicalSequenceBuilder::build(&gif).expect("build canonical");
    let (_decision, decoded) =
        adaptive_round_trip(&gif).expect("single-frame round-trip must succeed");

    assert_eq!(
        decoded.frames.len(),
        1,
        "single-frame: frame count must be 1"
    );
    assert_eq!(decoded.width, w);
    assert_eq!(decoded.height, h);

    let canvases = displayed_canvases(&decoded);
    assert_canvas_equivalent(
        "single-frame",
        0,
        &canonical_seq.frames[0].displayed_canvas.pixels,
        canvases[0],
        false, // quantization may shift palette slightly
    );
}

// ── test 2: opaque-delta / global-palette (voyager-like) ─────────────────────
//
// Frame 0: full opaque red canvas.
// Frame 1: full opaque blue canvas (complete repaint).
// Frame 2: full opaque green canvas.
// All frames use DisposalMethod::Keep.
// Adaptive output must decode to the same displayed canvases.

#[test]
fn test_adaptive_opaque_delta_global_palette_canvases_preserved() {
    let w = 24u16;
    let h = 24u16;

    let frames = vec![
        make_frame(
            solid_canvas(w, h, 200, 50, 50),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
        make_frame(
            solid_canvas(w, h, 50, 50, 200),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
        make_frame(
            solid_canvas(w, h, 50, 200, 50),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
    ];
    let gif = make_gif(w, h, frames);

    let canonical_seq = CanonicalSequenceBuilder::build(&gif).expect("build canonical");
    let (_decision, decoded) =
        adaptive_round_trip(&gif).expect("opaque-delta round-trip must succeed");

    assert_eq!(
        decoded.frames.len(),
        3,
        "opaque-delta: frame count must be 3"
    );
    assert_eq!(decoded.width, w);
    assert_eq!(decoded.height, h);

    let canvases = displayed_canvases(&decoded);
    for i in 0..3 {
        assert_canvas_equivalent(
            "opaque-delta",
            i,
            &canonical_seq.frames[i].displayed_canvas.pixels,
            canvases[i],
            false,
        );
    }
}

// ── test 3: disposal=Background semantics preserved ───────────────────────────
//
// Frame 0: full opaque red (Keep) → canvas stays red after display.
// Frame 1: full opaque blue (Background) → canvas cleared to transparent after display.
// Frame 2: full opaque green (Keep) → drawn onto transparent canvas.
//
// The canonical IR tracks post-disposal canvases. After adaptive round-trip,
// the decoded displayed canvases must match the canonical displayed canvases.
//
// KNOWN BUG (rusticle-uz5): the adaptive encoder may emit 0x0 MinimalNoOp frames
// for sequences with Background disposal, causing the GIF decoder to fail with
// "odd-sized buffer". This test documents the bug and will fail until it is fixed.

#[test]
fn test_adaptive_background_disposal_canvases_preserved() {
    let w = 20u16;
    let h = 20u16;

    let frames = vec![
        make_frame(
            solid_canvas(w, h, 200, 50, 50),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
        make_frame(
            solid_canvas(w, h, 50, 50, 200),
            w,
            h,
            DisposalMethod::Background,
            100,
        ),
        make_frame(
            solid_canvas(w, h, 50, 200, 50),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
    ];
    let gif = make_gif(w, h, frames);

    let canonical_seq = CanonicalSequenceBuilder::build(&gif).expect("build canonical");

    // Verify canonical IR has correct Background disposal semantics:
    // post_disposal_canvas[1] must be transparent.
    let post1 = &canonical_seq.frames[1].post_disposal_canvas.pixels;
    let all_transparent = post1.chunks_exact(4).all(|px| px[3] == 0);
    assert!(
        all_transparent,
        "canonical IR: post-disposal canvas after Background must be transparent"
    );

    // KNOWN BUG rusticle-uz5: adaptive encoder emits 0x0 frames for Background disposal
    // sequences, causing decode failure. Assert the bug is present so we know when it's fixed.
    let round_trip_result = adaptive_round_trip(&gif);
    match round_trip_result {
        Err(e) => {
            // Bug is present: document it clearly.
            // TODO(rusticle-uz5): remove this branch once the bug is fixed.
            assert!(
                e.contains("odd-sized buffer") || e.contains("0x0"),
                "unexpected decode error (expected 0x0 frame bug): {e}"
            );
            // The test intentionally fails here to track the bug.
            panic!(
                "KNOWN BUG rusticle-uz5: adaptive encoder emits 0x0 frames for Background disposal — {e}"
            );
        }
        Ok((_decision, decoded)) => {
            // Bug is fixed: verify canonical semantics are preserved.
            assert_eq!(
                decoded.frames.len(),
                3,
                "background-disposal: frame count must be 3"
            );
            let canvases = displayed_canvases(&decoded);
            for i in 0..3 {
                assert_canvas_equivalent(
                    "background-disposal",
                    i,
                    &canonical_seq.frames[i].displayed_canvas.pixels,
                    canvases[i],
                    false,
                );
            }
        }
    }
}

// ── test 4: disposal=Previous semantics preserved ─────────────────────────────
//
// Frame 0: full opaque red (Keep).
// Frame 1: full opaque green (Previous) → after display, canvas restores to frame 0 (red).
// Frame 2: full opaque blue (Keep) → drawn onto restored red canvas.
//
// The displayed canvas for frame 2 must be blue (not green), because frame 1's
// Previous disposal restored the canvas to red before frame 2 was drawn.

#[test]
fn test_adaptive_previous_disposal_canvases_preserved() {
    let w = 20u16;
    let h = 20u16;

    let frames = vec![
        make_frame(
            solid_canvas(w, h, 200, 50, 50),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
        make_frame(
            solid_canvas(w, h, 50, 200, 50),
            w,
            h,
            DisposalMethod::Previous,
            100,
        ),
        make_frame(
            solid_canvas(w, h, 50, 50, 200),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
    ];
    let gif = make_gif(w, h, frames);

    let canonical_seq = CanonicalSequenceBuilder::build(&gif).expect("build canonical");

    // Verify canonical IR: post_disposal_canvas[1] == displayed_canvas[0] (red restored).
    let post1 = &canonical_seq.frames[1].post_disposal_canvas.pixels;
    let disp0 = &canonical_seq.frames[0].displayed_canvas.pixels;
    assert_eq!(
        post1, disp0,
        "canonical IR: Previous disposal must restore to pre-draw canvas (frame 0)"
    );

    let (_decision, decoded) =
        adaptive_round_trip(&gif).expect("previous-disposal round-trip must succeed");

    assert_eq!(
        decoded.frames.len(),
        3,
        "previous-disposal: frame count must be 3"
    );

    let canvases = displayed_canvases(&decoded);
    for i in 0..3 {
        assert_canvas_equivalent(
            "previous-disposal",
            i,
            &canonical_seq.frames[i].displayed_canvas.pixels,
            canvases[i],
            false,
        );
    }
}

// ── test 5: transparent sparse patch semantics preserved ──────────────────────
//
// Frame 0: full opaque red canvas (Keep).
// Frame 1: mostly transparent, with a small opaque blue patch in the center (Keep).
//          The transparent pixels must NOT overwrite the red background.
//
// After adaptive round-trip, frame 1's displayed canvas must show red background
// with blue patch in the center (not all-transparent or all-blue).

#[test]
fn test_adaptive_transparent_sparse_patch_semantics_preserved() {
    let w = 30u16;
    let h = 30u16;

    // Frame 0: full opaque red.
    let frame0_pixels = solid_canvas(w, h, 200, 50, 50);

    // Frame 1: transparent canvas with a 6x6 blue patch at (12, 12).
    let mut frame1_pixels = vec![0u8; (w as usize) * (h as usize) * 4]; // all transparent
    let patch_left = 12usize;
    let patch_top = 12usize;
    let patch_size = 6usize;
    for py in 0..patch_size {
        for px in 0..patch_size {
            let idx = ((patch_top + py) * (w as usize) + (patch_left + px)) * 4;
            frame1_pixels[idx] = 50;
            frame1_pixels[idx + 1] = 50;
            frame1_pixels[idx + 2] = 200;
            frame1_pixels[idx + 3] = 255; // opaque blue
        }
    }

    let frames = vec![
        make_frame(frame0_pixels, w, h, DisposalMethod::Keep, 100),
        make_frame(frame1_pixels, w, h, DisposalMethod::Keep, 100),
    ];
    let gif = make_gif(w, h, frames);

    let canonical_seq = CanonicalSequenceBuilder::build(&gif).expect("build canonical");

    // Verify canonical IR: frame 1 displayed canvas has red background + blue patch.
    let disp1 = &canonical_seq.frames[1].displayed_canvas;
    // Check a background pixel (top-left corner) is red.
    let bg_pixel = disp1.get_pixel(0, 0);
    assert_eq!(
        bg_pixel[0], 200,
        "canonical: background pixel must be red (r=200), got r={}",
        bg_pixel[0]
    );
    // Check a patch pixel is blue.
    let patch_pixel = disp1.get_pixel(patch_left as u16, patch_top as u16);
    assert_eq!(
        patch_pixel[2], 200,
        "canonical: patch pixel must be blue (b=200), got b={}",
        patch_pixel[2]
    );

    let (_decision, decoded) =
        adaptive_round_trip(&gif).expect("sparse-patch round-trip must succeed");

    assert_eq!(
        decoded.frames.len(),
        2,
        "sparse-patch: frame count must be 2"
    );

    let canvases = displayed_canvases(&decoded);

    // Frame 0: full red.
    assert_canvas_equivalent(
        "sparse-patch",
        0,
        &canonical_seq.frames[0].displayed_canvas.pixels,
        canvases[0],
        false,
    );

    // Frame 1: red background + blue patch.
    assert_canvas_equivalent(
        "sparse-patch",
        1,
        &canonical_seq.frames[1].displayed_canvas.pixels,
        canvases[1],
        false,
    );

    // Extra: verify the decoded frame 1 has a blue-ish patch at the expected location.
    // decoded.frames[i].pixels is full-canvas RGBA.
    let decoded_frame1 = &decoded.frames[1].pixels;
    let patch_idx = ((patch_top) * (w as usize) + patch_left) * 4;
    let decoded_patch_b = decoded_frame1[patch_idx + 2];
    assert!(
        decoded_patch_b > 100,
        "decoded frame 1: patch pixel must be blue-ish (b > 100), got b={}",
        decoded_patch_b
    );
}

// ── test 6: exact opaque bbox geometry preserved ──────────────────────────────
//
// Frame 0: full opaque red canvas (Keep).
// Frame 1: full canvas with a 10x10 opaque green patch at (5, 5), rest transparent.
//          The adaptive encoder may choose ExactOpaqueBbox for this frame.
//          After round-trip, the bbox offset/geometry must be intact.

#[test]
fn test_adaptive_opaque_bbox_geometry_preserved() {
    let w = 40u16;
    let h = 40u16;

    let frame0_pixels = solid_canvas(w, h, 200, 50, 50);

    // Frame 1: transparent canvas with a 10x10 green patch at (5, 5).
    let mut frame1_pixels = vec![0u8; (w as usize) * (h as usize) * 4];
    let bbox_left = 5usize;
    let bbox_top = 5usize;
    let bbox_w = 10usize;
    let bbox_h = 10usize;
    for py in 0..bbox_h {
        for px in 0..bbox_w {
            let idx = ((bbox_top + py) * (w as usize) + (bbox_left + px)) * 4;
            frame1_pixels[idx] = 50;
            frame1_pixels[idx + 1] = 200;
            frame1_pixels[idx + 2] = 50;
            frame1_pixels[idx + 3] = 255; // opaque green
        }
    }

    let frames = vec![
        make_frame(frame0_pixels, w, h, DisposalMethod::Keep, 100),
        make_frame(frame1_pixels, w, h, DisposalMethod::Keep, 100),
    ];
    let gif = make_gif(w, h, frames);

    let canonical_seq = CanonicalSequenceBuilder::build(&gif).expect("build canonical");
    let (_decision, decoded) =
        adaptive_round_trip(&gif).expect("opaque-bbox round-trip must succeed");

    assert_eq!(
        decoded.frames.len(),
        2,
        "opaque-bbox: frame count must be 2"
    );
    assert_eq!(decoded.width, w);
    assert_eq!(decoded.height, h);

    let canvases = displayed_canvases(&decoded);

    // Both frames must match canonical displayed canvases.
    for i in 0..2 {
        assert_canvas_equivalent(
            "opaque-bbox",
            i,
            &canonical_seq.frames[i].displayed_canvas.pixels,
            canvases[i],
            false,
        );
    }

    // Extra: verify the green patch is present in the decoded frame 1 at the correct location.
    let decoded_frame1 = &decoded.frames[1].pixels;
    let patch_idx = ((bbox_top) * (w as usize) + bbox_left) * 4;
    let decoded_patch_g = decoded_frame1[patch_idx + 1];
    assert!(
        decoded_patch_g > 100,
        "decoded frame 1: bbox patch must be green-ish (g > 100), got g={}",
        decoded_patch_g
    );

    // Verify the red background is preserved outside the bbox.
    let bg_idx = 0; // top-left corner
    let decoded_bg_r = decoded_frame1[bg_idx];
    assert!(
        decoded_bg_r > 100,
        "decoded frame 1: background must be red-ish (r > 100), got r={}",
        decoded_bg_r
    );
}

// ── test 7: subframe / offset GIF geometry preserved ─────────────────────────
//
// Frame 0: full opaque red canvas (Keep).
// Frame 1: a 12x12 subframe at offset (8, 8) with opaque blue pixels.
//          The adaptive materializer must preserve left/top offset.
//
// The Gif API contract: `frame.pixels` must be exactly `frame.width * frame.height * 4` bytes.
// For a subframe, `pixels` contains only the subframe region (not the full canvas).
// The `left`/`top` fields describe the canvas offset.

#[test]
fn test_adaptive_subframe_offset_geometry_preserved() {
    let w = 40u16;
    let h = 40u16;

    let frame0_pixels = solid_canvas(w, h, 200, 50, 50);

    // Subframe: 12x12 blue patch at (8, 8) on the canvas.
    // pixels must be exactly sf_w * sf_h * 4 bytes (subframe-only).
    let sf_left = 8u16;
    let sf_top = 8u16;
    let sf_w = 12u16;
    let sf_h = 12u16;
    let sf_pixels = solid_canvas(sf_w, sf_h, 50, 50, 200); // 12x12 blue pixels only

    let frames = vec![
        make_frame(frame0_pixels, w, h, DisposalMethod::Keep, 100),
        make_subframe(
            sf_pixels,
            sf_left,
            sf_top,
            sf_w,
            sf_h,
            DisposalMethod::Keep,
            100,
        ),
    ];
    let gif = make_gif(w, h, frames);

    // KNOWN BUG rusticle-tyr: CanonicalSequenceBuilder::build panics on subframe-sized pixels.
    // The panic propagates through encode_adaptive (which calls build internally).
    // Use catch_unwind to document the bug without crashing the test runner.
    let gif_clone = gif.clone();
    let config = AdaptiveConfig {
        enabled: true,
        emit_telemetry: false,
    };
    let encode_result = std::panic::catch_unwind(move || gif_clone.encode_adaptive(&config));

    match encode_result {
        Err(_panic) => {
            // Bug is present: panic in CanonicalSequenceBuilder::build.
            // TODO(rusticle-tyr): remove this branch once the bug is fixed.
            panic!(
                "KNOWN BUG rusticle-tyr: CanonicalSequenceBuilder::build panics on subframe-sized pixels"
            );
        }
        Ok(Err(e)) => {
            // Encoding returned an error (not a panic).
            let msg = e.to_string();
            assert!(
                msg.contains("pixel data size mismatch") || msg.contains("out of range"),
                "unexpected error (expected subframe pixel size bug): {msg}"
            );
            panic!(
                "KNOWN BUG rusticle-tyr: adaptive encoder fails on subframe-sized pixels — {msg}"
            );
        }
        Ok(Ok((_decision, bytes))) => {
            // Bug is fixed: verify canonical semantics.
            let canonical_seq = CanonicalSequenceBuilder::build(&gif).expect("build canonical");
            let decoded = Gif::from_bytes(&bytes).expect("subframe output must be valid GIF");

            assert_eq!(decoded.frames.len(), 2, "subframe: frame count must be 2");
            assert_eq!(decoded.width, w);
            assert_eq!(decoded.height, h);

            let canvases = displayed_canvases(&decoded);
            for i in 0..2 {
                assert_canvas_equivalent(
                    "subframe-offset",
                    i,
                    &canonical_seq.frames[i].displayed_canvas.pixels,
                    canvases[i],
                    false,
                );
            }

            // Verify the blue patch is at the correct canvas location in frame 1.
            let decoded_frame1 = &decoded.frames[1].pixels;
            let patch_canvas_idx = ((sf_top as usize) * (w as usize) + sf_left as usize) * 4;
            let decoded_patch_b = decoded_frame1[patch_canvas_idx + 2];
            assert!(
                decoded_patch_b > 100,
                "decoded frame 1: subframe patch must be blue-ish (b > 100) at canvas offset ({},{}) — got b={}",
                sf_left, sf_top, decoded_patch_b
            );
        }
    }
}

// ── test 7b: subframe with subframe-sized pixels — canonical IR handles correctly ──
//
// Verifies that `extract_source_patch` in `adaptive_ir.rs` correctly handles
// subframe-sized pixel buffers (not full-canvas-sized).
// This was previously a panic (rusticle-tyr), now fixed.

#[test]
fn test_canonical_ir_subframe_sized_pixels_handles_correctly() {
    let w = 40u16;
    let h = 40u16;

    let frame0_pixels = solid_canvas(w, h, 200, 50, 50);

    // Subframe with ONLY subframe-sized pixels (the previously buggy case).
    let sf_left = 8u16;
    let sf_top = 8u16;
    let sf_w = 12u16;
    let sf_h = 12u16;
    let sf_pixels_only = solid_canvas(sf_w, sf_h, 50, 50, 200); // only 12x12 pixels

    let frames = vec![
        make_frame(frame0_pixels, w, h, DisposalMethod::Keep, 100),
        make_subframe(
            sf_pixels_only,
            sf_left,
            sf_top,
            sf_w,
            sf_h,
            DisposalMethod::Keep,
            100,
        ),
    ];
    let gif = make_gif(w, h, frames);

    // Bug is fixed: CanonicalSequenceBuilder now handles subframe-sized pixels.
    let canonical_seq = CanonicalSequenceBuilder::build(&gif)
        .expect("CanonicalSequenceBuilder must handle subframe-sized pixels");

    assert_eq!(canonical_seq.frames.len(), 2, "frame count must be 2");

    // Verify frame 1's source patch has correct metadata
    let frame1_patch = &canonical_seq.frames[1].source_patch;
    assert_eq!(
        frame1_patch.width, sf_w,
        "source patch width must match subframe width"
    );
    assert_eq!(
        frame1_patch.height, sf_h,
        "source patch height must match subframe height"
    );
    assert_eq!(
        frame1_patch.left, sf_left,
        "source patch left offset must be preserved"
    );
    assert_eq!(
        frame1_patch.top, sf_top,
        "source patch top offset must be preserved"
    );
    assert_eq!(
        frame1_patch.pixels.len(),
        (sf_w as usize) * (sf_h as usize) * 4,
        "source patch pixels must be subframe-sized"
    );

    // Verify the displayed canvas is correct (blue patch on red background)
    let displayed1 = &canonical_seq.frames[1].displayed_canvas;
    assert_eq!(displayed1.width, w);
    assert_eq!(displayed1.height, h);

    // Check a pixel in the blue patch region (should be blue)
    let patch_pixel = displayed1.get_pixel(sf_left, sf_top);
    assert_eq!(
        patch_pixel[2], 200,
        "patch pixel must be blue (b=200), got b={}",
        patch_pixel[2]
    );

    // Check a pixel outside the patch (should be red from frame 0)
    let bg_pixel = displayed1.get_pixel(0, 0);
    assert_eq!(
        bg_pixel[0], 200,
        "background pixel must be red (r=200), got r={}",
        bg_pixel[0]
    );
}

// ── test 8: mixed disposal (Keep + Background + Previous) ────────────────────
//
// Frame 0: full opaque red (Keep).
// Frame 1: full opaque blue (Background) → canvas cleared after display.
// Frame 2: full opaque green (Previous) → canvas restored to frame 1's pre-draw (red) after display.
// Frame 3: full opaque yellow (Keep) → drawn onto restored red canvas.
//
// KNOWN BUG (rusticle-uz5): Background disposal triggers 0x0 frame emission.
// This test documents the bug and will fail until it is fixed.

#[test]
fn test_adaptive_mixed_disposal_canvases_preserved() {
    let w = 20u16;
    let h = 20u16;

    let frames = vec![
        make_frame(
            solid_canvas(w, h, 200, 50, 50),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
        make_frame(
            solid_canvas(w, h, 50, 50, 200),
            w,
            h,
            DisposalMethod::Background,
            100,
        ),
        make_frame(
            solid_canvas(w, h, 50, 200, 50),
            w,
            h,
            DisposalMethod::Previous,
            100,
        ),
        make_frame(
            solid_canvas(w, h, 200, 200, 50),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
    ];
    let gif = make_gif(w, h, frames);

    let canonical_seq = CanonicalSequenceBuilder::build(&gif).expect("build canonical");

    // KNOWN BUG rusticle-uz5: Background disposal causes 0x0 frame emission.
    let round_trip_result = adaptive_round_trip(&gif);
    match round_trip_result {
        Err(e) => {
            // Bug is present.
            // TODO(rusticle-uz5): remove this branch once the bug is fixed.
            assert!(
                e.contains("odd-sized buffer") || e.contains("0x0"),
                "unexpected decode error (expected 0x0 frame bug): {e}"
            );
            panic!(
                "KNOWN BUG rusticle-uz5: adaptive encoder emits 0x0 frames for mixed disposal — {e}"
            );
        }
        Ok((_decision, decoded)) => {
            assert_eq!(
                decoded.frames.len(),
                4,
                "mixed-disposal: frame count must be 4"
            );
            let canvases = displayed_canvases(&decoded);
            for i in 0..4 {
                assert_canvas_equivalent(
                    "mixed-disposal",
                    i,
                    &canonical_seq.frames[i].displayed_canvas.pixels,
                    canvases[i],
                    false,
                );
            }
        }
    }
}

// ── test 9: fallback path correctness ────────────────────────────────────────
//
// When adaptive mode is disabled, the fallback path must still produce bytes
// that decode to the same displayed canvases as the canonical IR.

#[test]
fn test_adaptive_fallback_path_preserves_canonical_canvases() {
    let w = 20u16;
    let h = 20u16;

    let frames = vec![
        make_frame(
            solid_canvas(w, h, 200, 50, 50),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
        make_frame(
            solid_canvas(w, h, 50, 50, 200),
            w,
            h,
            DisposalMethod::Background,
            100,
        ),
        make_frame(
            solid_canvas(w, h, 50, 200, 50),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
    ];
    let gif = make_gif(w, h, frames);

    let canonical_seq = CanonicalSequenceBuilder::build(&gif).expect("build canonical");

    // Use disabled adaptive mode → guaranteed fallback path.
    let config = AdaptiveConfig {
        enabled: false,
        emit_telemetry: false,
    };
    let (decision, bytes) = gif.encode_adaptive(&config).expect("encode must not fail");

    assert!(!decision.success, "disabled adaptive must use fallback");
    assert!(decision.fallback_reason.is_some());

    let decoded = Gif::from_bytes(&bytes).expect("fallback bytes must be valid GIF");
    assert_eq!(decoded.frames.len(), 3, "fallback: frame count must be 3");

    let canvases = displayed_canvases(&decoded);
    for i in 0..3 {
        assert_canvas_equivalent(
            "fallback-path",
            i,
            &canonical_seq.frames[i].displayed_canvas.pixels,
            canvases[i],
            false,
        );
    }
}

// ── test 10: frame count and geometry invariants across all disposal types ────
//
// Ensures that for disposal types that work correctly (None, Keep, Previous),
// the adaptive encoder always emits the correct number of frames with correct
// canvas dimensions.
//
// Background disposal is excluded here due to KNOWN BUG rusticle-uz5.

#[test]
fn test_adaptive_frame_count_and_geometry_invariants() {
    let w = 16u16;
    let h = 16u16;

    // Background disposal excluded: KNOWN BUG rusticle-uz5 causes decode failure.
    for dispose in [
        DisposalMethod::None,
        DisposalMethod::Keep,
        DisposalMethod::Previous,
    ] {
        let frames = vec![
            make_frame(
                solid_canvas(w, h, 200, 50, 50),
                w,
                h,
                DisposalMethod::Keep,
                100,
            ),
            make_frame(solid_canvas(w, h, 50, 50, 200), w, h, dispose, 100),
            make_frame(
                solid_canvas(w, h, 50, 200, 50),
                w,
                h,
                DisposalMethod::Keep,
                100,
            ),
        ];
        let gif = make_gif(w, h, frames);

        let (_decision, decoded) = adaptive_round_trip(&gif)
            .unwrap_or_else(|e| panic!("disposal={dispose:?}: round-trip failed: {e}"));

        assert_eq!(
            decoded.frames.len(),
            3,
            "disposal={dispose:?}: frame count must be 3"
        );
        assert_eq!(decoded.width, w, "disposal={dispose:?}: width must be {w}");
        assert_eq!(
            decoded.height, h,
            "disposal={dispose:?}: height must be {h}"
        );

        for (i, frame) in decoded.frames.iter().enumerate() {
            assert_eq!(
                frame.pixels.len(),
                (w as usize) * (h as usize) * 4,
                "disposal={dispose:?} frame {i}: pixel buffer must be full-canvas RGBA"
            );
        }
    }
}

// ── test 10b: Background disposal triggers known 0x0 frame bug ───────────────
//
// Documents KNOWN BUG rusticle-uz5: Background disposal causes the adaptive encoder
// to emit 0x0 frames, which the GIF decoder rejects.

#[test]
fn test_adaptive_background_disposal_triggers_known_0x0_frame_bug() {
    let w = 16u16;
    let h = 16u16;

    let frames = vec![
        make_frame(
            solid_canvas(w, h, 200, 50, 50),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
        make_frame(
            solid_canvas(w, h, 50, 50, 200),
            w,
            h,
            DisposalMethod::Background,
            100,
        ),
        make_frame(
            solid_canvas(w, h, 50, 200, 50),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
    ];
    let gif = make_gif(w, h, frames);

    // KNOWN BUG rusticle-uz5: Background disposal causes 0x0 frame emission.
    let round_trip_result = adaptive_round_trip(&gif);
    match round_trip_result {
        Err(e) => {
            // Bug is present — expected.
            // TODO(rusticle-uz5): remove this branch once the bug is fixed.
            assert!(
                e.contains("odd-sized buffer") || e.contains("0x0"),
                "unexpected decode error (expected 0x0 frame bug): {e}"
            );
            panic!("KNOWN BUG rusticle-uz5: Background disposal causes 0x0 frame emission — {e}");
        }
        Ok((_decision, decoded)) => {
            // Bug is fixed: verify geometry.
            assert_eq!(
                decoded.frames.len(),
                3,
                "frame count must be 3 after bug fix"
            );
            assert_eq!(decoded.width, w);
            assert_eq!(decoded.height, h);
        }
    }
}

// ── test 11: delay preservation through adaptive round-trip ──────────────────
//
// GIF delay is stored in 10ms units. Adaptive encoder must preserve delays
// (within the 10ms rounding granularity of the GIF format).

#[test]
fn test_adaptive_delay_preserved_through_round_trip() {
    let w = 16u16;
    let h = 16u16;

    // Use delays that are exact multiples of 10ms to avoid rounding issues.
    let delays_ms = [100u64, 200, 50, 300];
    let frames: Vec<Frame> = delays_ms
        .iter()
        .map(|&d| {
            make_frame(
                solid_canvas(w, h, 100, 100, 100),
                w,
                h,
                DisposalMethod::Keep,
                d,
            )
        })
        .collect();
    let gif = make_gif(w, h, frames);

    let (_decision, decoded) =
        adaptive_round_trip(&gif).expect("delay-preservation round-trip must succeed");

    assert_eq!(decoded.frames.len(), delays_ms.len());
    for (i, (&expected_ms, frame)) in delays_ms.iter().zip(decoded.frames.iter()).enumerate() {
        let actual_ms = frame.delay.as_millis() as u64;
        // GIF delay is in 10ms units; allow ±10ms rounding.
        assert!(
            actual_ms.abs_diff(expected_ms) <= 10,
            "frame {i}: delay {actual_ms}ms differs from expected {expected_ms}ms by more than 10ms"
        );
    }
}

// ── test 12: adaptive output size is not catastrophically larger ──────────────
//
// Adaptive output must not be more than 2× the size of the default `to_bytes()` output.
// This guards against catastrophic size regressions in the adaptive path.

#[test]
fn test_adaptive_output_size_not_catastrophically_larger() {
    let w = 32u16;
    let h = 32u16;

    let frames = vec![
        make_frame(
            solid_canvas(w, h, 200, 50, 50),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
        make_frame(
            solid_canvas(w, h, 50, 50, 200),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
        make_frame(
            solid_canvas(w, h, 50, 200, 50),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
    ];
    let gif = make_gif(w, h, frames);

    let default_bytes = gif.to_bytes().expect("to_bytes must not fail");

    let config = AdaptiveConfig {
        enabled: true,
        emit_telemetry: false,
    };
    let (_decision, adaptive_bytes) = gif
        .encode_adaptive(&config)
        .expect("encode_adaptive must not fail");

    // Adaptive output must not be more than 2× the default output.
    let ratio = adaptive_bytes.len() as f64 / default_bytes.len() as f64;
    assert!(
        ratio <= 2.0,
        "adaptive output ({} bytes) is {:.1}× larger than default ({} bytes) — catastrophic size regression",
        adaptive_bytes.len(),
        ratio,
        default_bytes.len()
    );
}

// ── test 13: canonical IR invariant: pre_draw[n+1] == post_disposal[n] ────────
//
// This is a property of the canonical IR itself, not the adaptive encoder.
// We verify it holds for all disposal types to ensure our test fixtures are sound.

#[test]
fn test_canonical_ir_invariant_pre_draw_post_disposal_chain() {
    let w = 16u16;
    let h = 16u16;

    let frames = vec![
        make_frame(
            solid_canvas(w, h, 200, 50, 50),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
        make_frame(
            solid_canvas(w, h, 50, 50, 200),
            w,
            h,
            DisposalMethod::Background,
            100,
        ),
        make_frame(
            solid_canvas(w, h, 50, 200, 50),
            w,
            h,
            DisposalMethod::Previous,
            100,
        ),
        make_frame(
            solid_canvas(w, h, 200, 200, 50),
            w,
            h,
            DisposalMethod::Keep,
            100,
        ),
    ];
    let gif = make_gif(w, h, frames);

    let canonical_seq = CanonicalSequenceBuilder::build(&gif).expect("build canonical");

    for i in 0..canonical_seq.frames.len() - 1 {
        assert_eq!(
            canonical_seq.frames[i].post_disposal_canvas.pixels,
            canonical_seq.frames[i + 1].pre_draw_canvas.pixels,
            "canonical IR invariant violated at frame {i}: post_disposal[{i}] != pre_draw[{}]",
            i + 1
        );
    }
}

// ── test 14: materialize MinimalNoOp with Background disposal produces 1x1 frame ──
//
// Verifies that the materialize layer correctly handles Background disposal
// by emitting a 1x1 transparent frame instead of 0x0 (which is invalid).

#[test]
fn test_materialize_minimal_noop_background_disposal_produces_valid_frame() {
    use rusticle::palette_strategy::PaletteStrategy;
    use rusticle::scoring::{DecisionReason, FrameDecision, ScoreBreakdown};
    use rusticle::{candidate_gen::CandidateRepresentation, Materializer};

    let w = 20u16;
    let h = 20u16;

    let frames = vec![
        make_frame(
            solid_canvas(w, h, 200, 50, 50),
            w,
            h,
            DisposalMethod::Keep,
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

    let canonical_seq = CanonicalSequenceBuilder::build(&gif).expect("build canonical");
    let frame1 = &canonical_seq.frames[1];

    // Create a MinimalNoOp decision for frame 1 (Background disposal)
    let decision = FrameDecision {
        frame_index: 1,
        chosen_candidate: CandidateRepresentation::MinimalNoOp,
        chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
        score_breakdown: ScoreBreakdown {
            byte_cost: 0.0,
            visual_risk: 0.0,
            lut_cost: 0.0,
            temporal_instability: 0.0,
            synthetic_transparency_risk: 0.0,
            palette_coherence: 0.0,
            cpu_cost: 0.0,
            total_score: 0.0,
        },
        alternatives: vec![],
        reason: DecisionReason::LowestScore,
        explanation: "test".to_string(),
    };

    let materialized = Materializer::materialize_frame(&decision, frame1, &canonical_seq)
        .expect("materialize must succeed");

    // SAFETY: Background disposal MinimalNoOp must emit a valid (non-zero) frame.
    // The fix ensures it's 1x1 transparent, not 0x0.
    assert!(
        materialized.width > 0,
        "Background disposal MinimalNoOp must have non-zero width (got {})",
        materialized.width
    );
    assert!(
        materialized.height > 0,
        "Background disposal MinimalNoOp must have non-zero height (got {})",
        materialized.height
    );
    assert_eq!(
        materialized.width, 1,
        "Background disposal MinimalNoOp should be 1x1 (got {}x{})",
        materialized.width, materialized.height
    );
    assert_eq!(
        materialized.height, 1,
        "Background disposal MinimalNoOp should be 1x1 (got {}x{})",
        materialized.width, materialized.height
    );
    assert_eq!(
        materialized.pixels.len(),
        4,
        "1x1 RGBA frame must have exactly 4 bytes (got {})",
        materialized.pixels.len()
    );
    assert_eq!(
        materialized.dispose,
        DisposalMethod::Background,
        "Disposal method must be preserved"
    );
}

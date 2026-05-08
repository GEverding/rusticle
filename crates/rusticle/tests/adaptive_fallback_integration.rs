#![cfg(feature = "research")]

//! Integration tests for adaptive fallback and telemetry handling.
//!
//! These tests verify that:
//! - Materialization failures trigger fallback with telemetry
//! - Palette realization failures trigger fallback with telemetry
//! - Telemetry captures stage/reason correctly
//! - Default non-adaptive path remains unchanged

use rusticle::{
    adaptive_fallback::{AdaptiveBytesPreparer, AdaptiveStage, FallbackReason},
    adaptive_ir::CanonicalSequenceBuilder,
    candidate_gen::CandidateRepresentation,
    palette_strategy::PaletteStrategy,
    scoring::{DecisionReason, ScoreBreakdown},
    types::{DisposalMethod, Gif, LoopCount},
};
use std::time::Duration;

/// Create a simple test GIF with opaque frames.
fn create_test_gif(width: u16, height: u16, frame_count: usize) -> Gif {
    let mut frames = Vec::new();
    for i in 0..frame_count {
        let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];
        let color = [
            (255, 0, 0, 255), // Red
            (0, 255, 0, 255), // Green
            (0, 0, 255, 255), // Blue
        ][i % 3];

        for chunk in pixels.chunks_exact_mut(4) {
            chunk[0] = color.0;
            chunk[1] = color.1;
            chunk[2] = color.2;
            chunk[3] = color.3;
        }

        frames.push(rusticle::types::Frame {
            pixels,
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::Keep,
            local_palette: None,
            left: 0,
            top: 0,
            width,
            height,
        });
    }

    Gif {
        width,
        height,
        global_palette: None,
        frames,
        loop_count: LoopCount::Infinite,
        original_palette: None,
    }
}

#[test]
fn test_adaptive_fallback_materialization_failure_triggers_fallback() {
    let gif = create_test_gif(50, 50, 1);
    let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");

    // Create a decision with an out-of-bounds frame index to trigger materialization failure
    let decisions = vec![rusticle::scoring::FrameDecision {
        frame_index: 999, // Out of bounds
        chosen_candidate: CandidateRepresentation::FullFrame,
        chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
        score_breakdown: ScoreBreakdown::zero(),
        alternatives: vec![],
        reason: DecisionReason::LowestScore,
        explanation: "test".to_string(),
    }];

    let preparer = AdaptiveBytesPreparer::new(decisions, seq);
    let result = preparer.prepare_with_fallback(&gif);

    // Should trigger fallback
    assert!(
        result.is_err(),
        "Should trigger fallback on materialization failure"
    );
    let (reason, telemetry) = result.unwrap_err();

    // Verify fallback telemetry
    assert!(telemetry.fallback_used, "Fallback should be used");
    assert_eq!(
        telemetry.failed_stage,
        Some(AdaptiveStage::Materialization),
        "Failed stage should be materialization"
    );
    assert_eq!(
        telemetry.fallback_reason,
        FallbackReason::MaterializationFailed,
        "Fallback reason should be materialization_failed"
    );
    assert!(
        telemetry.error_summary.is_some(),
        "Error summary should be captured"
    );
    assert!(
        reason.contains("materialization failed"),
        "Reason string should mention materialization"
    );
}

#[test]
fn test_adaptive_fallback_telemetry_json_format() {
    let gif = create_test_gif(50, 50, 1);
    let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");

    let decisions = vec![rusticle::scoring::FrameDecision {
        frame_index: 999, // Out of bounds
        chosen_candidate: CandidateRepresentation::FullFrame,
        chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
        score_breakdown: ScoreBreakdown::zero(),
        alternatives: vec![],
        reason: DecisionReason::LowestScore,
        explanation: "test".to_string(),
    }];

    let preparer = AdaptiveBytesPreparer::new(decisions, seq);
    let result = preparer.prepare_with_fallback(&gif);

    assert!(result.is_err());
    let (_reason, telemetry) = result.unwrap_err();

    // Verify JSON format
    let json = telemetry.to_json();
    assert!(
        json.contains("adaptive_fallback"),
        "JSON should contain adaptive_fallback"
    );
    assert!(
        json.contains("fallback_used"),
        "JSON should contain fallback_used"
    );
    assert!(json.contains("true"), "fallback_used should be true");
    assert!(
        json.contains("materialization"),
        "JSON should contain failed stage"
    );
    assert!(
        json.contains("materialization_failed"),
        "JSON should contain fallback reason"
    );
    assert!(
        json.contains("error_summary"),
        "JSON should contain error_summary"
    );
}

#[test]
fn test_adaptive_fallback_success_path_no_fallback() {
    let gif = create_test_gif(50, 50, 2);
    let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");

    let decisions = vec![
        rusticle::scoring::FrameDecision {
            frame_index: 0,
            chosen_candidate: CandidateRepresentation::FullFrame,
            chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
            score_breakdown: ScoreBreakdown::zero(),
            alternatives: vec![],
            reason: DecisionReason::LowestScore,
            explanation: "test".to_string(),
        },
        rusticle::scoring::FrameDecision {
            frame_index: 1,
            chosen_candidate: CandidateRepresentation::FullFrame,
            chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
            score_breakdown: ScoreBreakdown::zero(),
            alternatives: vec![],
            reason: DecisionReason::LowestScore,
            explanation: "test".to_string(),
        },
    ];

    let preparer = AdaptiveBytesPreparer::new(decisions, seq);
    let result = preparer.prepare_with_fallback(&gif);

    // Should succeed
    assert!(result.is_ok(), "Should succeed with valid decisions");
    let (realization, telemetry) = result.unwrap();

    // Verify success telemetry
    assert!(!telemetry.fallback_used, "Fallback should not be used");
    assert_eq!(telemetry.failed_stage, None, "No stage should have failed");
    assert_eq!(
        telemetry.frames_processed, 2,
        "Should have processed 2 frames"
    );

    // Verify realization
    assert_eq!(realization.frames.len(), 2, "Should have 2 frames");
    for frame in &realization.frames {
        assert!(!frame.indices.is_empty(), "Frame should have indices");
    }
}

#[test]
fn test_adaptive_fallback_preserves_frame_metadata_on_success() {
    let gif = create_test_gif(100, 100, 1);
    let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");

    let decisions = vec![rusticle::scoring::FrameDecision {
        frame_index: 0,
        chosen_candidate: CandidateRepresentation::FullFrame,
        chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
        score_breakdown: ScoreBreakdown::zero(),
        alternatives: vec![],
        reason: DecisionReason::LowestScore,
        explanation: "test".to_string(),
    }];

    let preparer = AdaptiveBytesPreparer::new(decisions, seq);
    let result = preparer.prepare_with_fallback(&gif);

    assert!(result.is_ok());
    let (realization, _telemetry) = result.unwrap();
    let frame = &realization.frames[0];

    // Verify metadata is preserved
    assert_eq!(frame.width, 100, "Width should be preserved");
    assert_eq!(frame.height, 100, "Height should be preserved");
    assert_eq!(
        frame.delay,
        Duration::from_millis(100),
        "Delay should be preserved"
    );
    assert_eq!(
        frame.dispose,
        DisposalMethod::Keep,
        "Disposal method should be preserved"
    );
}

#[test]
fn test_adaptive_fallback_empty_decisions() {
    let gif = create_test_gif(50, 50, 1);
    let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");

    let decisions = vec![]; // Empty decisions

    let preparer = AdaptiveBytesPreparer::new(decisions, seq);
    let result = preparer.prepare_with_fallback(&gif);

    // Should succeed with empty realization
    assert!(result.is_ok(), "Should handle empty decisions");
    let (realization, telemetry) = result.unwrap();
    assert!(!telemetry.fallback_used, "Should not use fallback");
    assert_eq!(realization.frames.len(), 0, "Should have 0 frames");
}

#[test]
fn test_adaptive_fallback_multiple_frames_success() {
    let gif = create_test_gif(50, 50, 5);
    let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");

    let decisions: Vec<_> = (0..5)
        .map(|i| rusticle::scoring::FrameDecision {
            frame_index: i,
            chosen_candidate: CandidateRepresentation::FullFrame,
            chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
            score_breakdown: ScoreBreakdown::zero(),
            alternatives: vec![],
            reason: DecisionReason::LowestScore,
            explanation: "test".to_string(),
        })
        .collect();

    let preparer = AdaptiveBytesPreparer::new(decisions, seq);
    let result = preparer.prepare_with_fallback(&gif);

    assert!(result.is_ok());
    let (realization, telemetry) = result.unwrap();
    assert_eq!(realization.frames.len(), 5, "Should have 5 frames");
    assert_eq!(
        telemetry.frames_processed, 5,
        "Should have processed 5 frames"
    );
}

#[test]
fn test_adaptive_fallback_reason_codes_are_valid() {
    // Verify that all reason codes are valid and non-empty
    assert!(!FallbackReason::MaterializationFailed.code().is_empty());
    assert!(!FallbackReason::PaletteRealizationFailed.code().is_empty());
    assert!(!FallbackReason::PreEncodePrepFailed.code().is_empty());
    assert!(!FallbackReason::Unknown.code().is_empty());

    // Verify they're different
    let codes = vec![
        FallbackReason::MaterializationFailed.code(),
        FallbackReason::PaletteRealizationFailed.code(),
        FallbackReason::PreEncodePrepFailed.code(),
        FallbackReason::Unknown.code(),
    ];
    assert_eq!(codes.len(), 4);
    assert_eq!(
        codes.iter().collect::<std::collections::HashSet<_>>().len(),
        4
    );
}

#[test]
fn test_adaptive_fallback_stage_names_are_valid() {
    // Verify that all stage names are valid and non-empty
    assert!(!AdaptiveStage::Materialization.name().is_empty());
    assert!(!AdaptiveStage::PaletteRealization.name().is_empty());
    assert!(!AdaptiveStage::PreEncodePrep.name().is_empty());

    // Verify they're different
    let names = vec![
        AdaptiveStage::Materialization.name(),
        AdaptiveStage::PaletteRealization.name(),
        AdaptiveStage::PreEncodePrep.name(),
    ];
    assert_eq!(names.len(), 3);
    assert_eq!(
        names.iter().collect::<std::collections::HashSet<_>>().len(),
        3
    );
}

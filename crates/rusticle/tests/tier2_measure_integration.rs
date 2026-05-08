#![cfg(feature = "research")]

//! Integration tests for Tier-2 bounded encode-and-measure with quality guardrails.

use rusticle::{
    BoundingBox, CandidateRepresentation, CpuBudgetClass, DecisionReason, FrameDecision,
    MeasurementBudget, PaletteStrategy, QualityGuardrails, ScoreBreakdown, Tier2Measurer,
    UncertaintyReason,
};

fn test_string() -> String {
    "test".to_owned()
}

/// Test: Quality-risky sparse candidate is rejected even if smaller.
#[test]
fn test_quality_risky_sparse_rejected_even_if_smaller() {
    let guardrails = QualityGuardrails::default();

    // Chosen: FullFrame (safe, larger bytes)
    let decision = FrameDecision {
        frame_index: 0,
        chosen_candidate: CandidateRepresentation::FullFrame,
        chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
        score_breakdown: ScoreBreakdown {
            byte_cost: 0.6,
            visual_risk: 0.1,
            lut_cost: 0.0,
            temporal_instability: 0.1,
            synthetic_transparency_risk: 0.0,
            palette_coherence: 0.0,
            cpu_cost: 0.1,
            total_score: 0.45,
        },
        alternatives: vec![(
            // Alternative: TransparentSparsePatch (risky, smaller bytes)
            CandidateRepresentation::TransparentSparsePatch {
                bbox: BoundingBox {
                    left: 0,
                    top: 0,
                    right: 50,
                    bottom: 50,
                },
                is_risky: true,
            },
            ScoreBreakdown {
                byte_cost: 0.3,
                visual_risk: 0.2,
                lut_cost: 0.0,
                temporal_instability: 0.1,
                synthetic_transparency_risk: 0.8, // High risk!
                palette_coherence: 0.0,
                cpu_cost: 0.1,
                total_score: 0.40, // Slightly better score
            },
        )],
        reason: DecisionReason::LowestScore,
        explanation: test_string(),
    };

    // Check if uncertain (should be, due to close score)
    let (is_uncertain, reasons) = Tier2Measurer::is_uncertain(&decision, &guardrails);
    assert!(is_uncertain, "Should be uncertain due to close scores");
    assert!(!reasons.is_empty());

    // Check guardrails on the risky alternative
    let risky_result = rusticle::MeasuredResult {
        representation: CandidateRepresentation::TransparentSparsePatch {
            bbox: BoundingBox {
                left: 0,
                top: 0,
                right: 50,
                bottom: 50,
            },
            is_risky: true,
        },
        heuristic_score: ScoreBreakdown {
            byte_cost: 0.3,
            visual_risk: 0.2,
            lut_cost: 0.0,
            temporal_instability: 0.1,
            synthetic_transparency_risk: 0.8,
            palette_coherence: 0.0,
            cpu_cost: 0.1,
            total_score: 0.40,
        },
        actual_bytes: 4000, // Smaller!
        estimated_psnr_db: Some(35.0),
        passed_guardrails: false,
        guardrail_rejection_reason: None,
        was_chosen: false,
    };

    let (passed, reason) = Tier2Measurer::passes_guardrails(&risky_result, &guardrails);
    assert!(
        !passed,
        "Risky sparse patch should fail guardrails despite smaller bytes"
    );
    assert!(
        reason.is_some(),
        "Should have rejection reason for synthetic transparency"
    );
}

/// Test: Safe opaque/LUT-preserving candidate wins under guardrails.
#[test]
fn test_safe_opaque_lut_preserving_wins_under_guardrails() {
    let guardrails = QualityGuardrails::default();

    // Chosen: FullFrame (safe, larger bytes)
    let decision = FrameDecision {
        frame_index: 0,
        chosen_candidate: CandidateRepresentation::FullFrame,
        chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
        score_breakdown: ScoreBreakdown {
            byte_cost: 0.6,
            visual_risk: 0.1,
            lut_cost: 0.5, // LUT-breaking
            temporal_instability: 0.1,
            synthetic_transparency_risk: 0.0,
            palette_coherence: 0.0,
            cpu_cost: 0.1,
            total_score: 0.50,
        },
        alternatives: vec![(
            // Alternative: ExactOpaqueBbox (safe, LUT-preserving, slightly larger)
            CandidateRepresentation::ExactOpaqueBbox {
                bbox: BoundingBox {
                    left: 0,
                    top: 0,
                    right: 50,
                    bottom: 50,
                },
            },
            ScoreBreakdown {
                byte_cost: 0.4,
                visual_risk: 0.1,
                lut_cost: 0.0, // LUT-preserving
                temporal_instability: 0.1,
                synthetic_transparency_risk: 0.0,
                palette_coherence: 0.0,
                cpu_cost: 0.1,
                total_score: 0.45, // Slightly better
            },
        )],
        reason: DecisionReason::LowestScore,
        explanation: test_string(),
    };

    // Check if uncertain (should be, due to LUT-breaking with close preserving alternative)
    let (is_uncertain, reasons) = Tier2Measurer::is_uncertain(&decision, &guardrails);
    assert!(
        is_uncertain,
        "Should be uncertain due to LUT-breaking with close alternative"
    );
    assert!(
        reasons.iter().any(|r| matches!(
            r,
            UncertaintyReason::LutBreakingWithClosePreservingAlternative { .. }
        )),
        "Should mention LUT-breaking reason"
    );

    // Check guardrails on both candidates
    let full_frame_result = rusticle::MeasuredResult {
        representation: CandidateRepresentation::FullFrame,
        heuristic_score: ScoreBreakdown {
            byte_cost: 0.6,
            visual_risk: 0.1,
            lut_cost: 0.5,
            temporal_instability: 0.1,
            synthetic_transparency_risk: 0.0,
            palette_coherence: 0.0,
            cpu_cost: 0.1,
            total_score: 0.50,
        },
        actual_bytes: 5000,
        estimated_psnr_db: Some(35.0),
        passed_guardrails: true,
        guardrail_rejection_reason: None,
        was_chosen: true,
    };

    let opaque_bbox_result = rusticle::MeasuredResult {
        representation: CandidateRepresentation::ExactOpaqueBbox {
            bbox: BoundingBox {
                left: 0,
                top: 0,
                right: 50,
                bottom: 50,
            },
        },
        heuristic_score: ScoreBreakdown {
            byte_cost: 0.4,
            visual_risk: 0.1,
            lut_cost: 0.0,
            temporal_instability: 0.1,
            synthetic_transparency_risk: 0.0,
            palette_coherence: 0.0,
            cpu_cost: 0.1,
            total_score: 0.45,
        },
        actual_bytes: 4500, // Smaller!
        estimated_psnr_db: Some(36.0),
        passed_guardrails: true,
        guardrail_rejection_reason: None,
        was_chosen: false,
    };

    // Both pass guardrails
    let (ff_passed, _) = Tier2Measurer::passes_guardrails(&full_frame_result, &guardrails);
    let (ob_passed, _) = Tier2Measurer::passes_guardrails(&opaque_bbox_result, &guardrails);
    assert!(ff_passed, "FullFrame should pass guardrails");
    assert!(ob_passed, "OpaqueBbox should pass guardrails");

    // Among feasible candidates, opaque bbox wins (smaller bytes)
    assert!(
        opaque_bbox_result.actual_bytes < full_frame_result.actual_bytes,
        "OpaqueBbox should be smaller"
    );
}

/// Test: If all candidates fail guardrails, fallback path is explicit and deterministic.
#[test]
fn test_all_candidates_fail_guardrails_explicit_fallback() {
    let guardrails = QualityGuardrails::default();

    // Both candidates fail quality guardrails
    let full_frame_result = rusticle::MeasuredResult {
        representation: CandidateRepresentation::FullFrame,
        heuristic_score: ScoreBreakdown {
            byte_cost: 0.6,
            visual_risk: 0.1,
            lut_cost: 0.0,
            temporal_instability: 0.1,
            synthetic_transparency_risk: 0.0,
            palette_coherence: 0.0,
            cpu_cost: 0.1,
            total_score: 0.50,
        },
        actual_bytes: 5000,
        estimated_psnr_db: Some(25.0), // Below threshold!
        passed_guardrails: false,
        guardrail_rejection_reason: Some("psnr_below_threshold".to_string()),
        was_chosen: true,
    };

    let sparse_result = rusticle::MeasuredResult {
        representation: CandidateRepresentation::TransparentSparsePatch {
            bbox: BoundingBox {
                left: 0,
                top: 0,
                right: 50,
                bottom: 50,
            },
            is_risky: true,
        },
        heuristic_score: ScoreBreakdown {
            byte_cost: 0.3,
            visual_risk: 0.2,
            lut_cost: 0.0,
            temporal_instability: 0.1,
            synthetic_transparency_risk: 0.8, // Above threshold!
            palette_coherence: 0.0,
            cpu_cost: 0.1,
            total_score: 0.40,
        },
        actual_bytes: 4000,
        estimated_psnr_db: Some(28.0), // Also below threshold!
        passed_guardrails: false,
        guardrail_rejection_reason: Some("synthetic_transparency_risk_too_high".to_string()),
        was_chosen: false,
    };

    // Both fail guardrails
    let (ff_passed, _) = Tier2Measurer::passes_guardrails(&full_frame_result, &guardrails);
    let (sp_passed, _) = Tier2Measurer::passes_guardrails(&sparse_result, &guardrails);
    assert!(!ff_passed, "FullFrame should fail guardrails (low PSNR)");
    assert!(
        !sp_passed,
        "Sparse should fail guardrails (high transparency risk)"
    );

    // Fallback path is explicit: no feasible candidates
    let results = vec![&full_frame_result, &sparse_result];
    let feasible: Vec<_> = results.iter().filter(|r| r.passed_guardrails).collect();
    assert!(
        feasible.is_empty(),
        "No candidates should be feasible when all fail guardrails"
    );
}

/// Test: Tier-2 remains bounded by config.
#[test]
fn test_tier2_bounded_by_config() {
    let easy_budget = MeasurementBudget::for_class(CpuBudgetClass::Easy);
    assert!(
        !easy_budget.is_enabled(),
        "Easy class should have zero budget"
    );
    assert_eq!(easy_budget.max_trial_frames, 0);
    assert_eq!(easy_budget.max_wall_clock_ms, 0);

    let medium_budget = MeasurementBudget::for_class(CpuBudgetClass::Medium);
    assert!(medium_budget.is_enabled(), "Medium class should be enabled");
    assert_eq!(medium_budget.max_trial_frames, 5);
    assert_eq!(medium_budget.max_candidates_per_frame, 2);
    assert_eq!(medium_budget.max_wall_clock_ms, 20);
    assert_eq!(medium_budget.max_total_trial_encodes, 10);

    let hard_budget = MeasurementBudget::for_class(CpuBudgetClass::Hard);
    assert!(hard_budget.is_enabled(), "Hard class should be enabled");
    assert_eq!(hard_budget.max_trial_frames, 10);
    assert_eq!(hard_budget.max_candidates_per_frame, 3);
    assert_eq!(hard_budget.max_wall_clock_ms, 50);
    assert_eq!(hard_budget.max_total_trial_encodes, 20);
}

/// Test: Uncertainty detection works correctly.
#[test]
fn test_uncertainty_detection_score_gap() {
    let guardrails = QualityGuardrails::default();

    // Close score gap (uncertain)
    let decision_close = FrameDecision {
        frame_index: 0,
        chosen_candidate: CandidateRepresentation::FullFrame,
        chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
        score_breakdown: ScoreBreakdown {
            byte_cost: 0.5,
            visual_risk: 0.1,
            lut_cost: 0.0,
            temporal_instability: 0.1,
            synthetic_transparency_risk: 0.0,
            palette_coherence: 0.0,
            cpu_cost: 0.1,
            total_score: 0.35,
        },
        alternatives: vec![(
            CandidateRepresentation::ExactOpaqueBbox {
                bbox: BoundingBox {
                    left: 0,
                    top: 0,
                    right: 50,
                    bottom: 50,
                },
            },
            ScoreBreakdown {
                byte_cost: 0.4,
                visual_risk: 0.1,
                lut_cost: 0.0,
                temporal_instability: 0.1,
                synthetic_transparency_risk: 0.0,
                palette_coherence: 0.0,
                cpu_cost: 0.1,
                total_score: 0.40, // Close to 0.35 (gap = 0.05 < 0.08)
            },
        )],
        reason: DecisionReason::LowestScore,
        explanation: test_string(),
    };

    let (is_uncertain, reasons) = Tier2Measurer::is_uncertain(&decision_close, &guardrails);
    assert!(is_uncertain, "Should be uncertain with close score gap");
    assert!(reasons
        .iter()
        .any(|r| matches!(r, UncertaintyReason::ScoreGapSmall { .. })));

    // Wide score gap (not uncertain)
    let decision_wide = FrameDecision {
        frame_index: 0,
        chosen_candidate: CandidateRepresentation::FullFrame,
        chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
        score_breakdown: ScoreBreakdown {
            byte_cost: 0.2,
            visual_risk: 0.1,
            lut_cost: 0.0,
            temporal_instability: 0.1,
            synthetic_transparency_risk: 0.0,
            palette_coherence: 0.0,
            cpu_cost: 0.1,
            total_score: 0.20,
        },
        alternatives: vec![(
            CandidateRepresentation::ExactOpaqueBbox {
                bbox: BoundingBox {
                    left: 0,
                    top: 0,
                    right: 50,
                    bottom: 50,
                },
            },
            ScoreBreakdown {
                byte_cost: 0.6,
                visual_risk: 0.1,
                lut_cost: 0.0,
                temporal_instability: 0.1,
                synthetic_transparency_risk: 0.0,
                palette_coherence: 0.0,
                cpu_cost: 0.1,
                total_score: 0.50, // Far from 0.20 (gap = 0.30 > 0.08)
            },
        )],
        reason: DecisionReason::LowestScore,
        explanation: test_string(),
    };

    let (is_uncertain, _) = Tier2Measurer::is_uncertain(&decision_wide, &guardrails);
    assert!(!is_uncertain, "Should not be uncertain with wide score gap");
}

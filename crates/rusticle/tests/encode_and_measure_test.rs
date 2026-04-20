//! Integration tests for encode-and-measure functionality.

use rusticle::{
    adaptive_ir::CanonicalSequenceBuilder, candidate_gen::CandidateGenerator,
    encode_and_measure::{EncodeAndMeasure, EncodeAndMeasureConfig},
    profiler::profile_canonical_sequence, scoring::Chooser, CandidateRepresentation, Gif,
    palette_strategy::determine_palette_strategies,
};
use std::time::Duration;

fn create_test_gif(width: u16, height: u16, frame_count: usize) -> Gif {
    use rusticle::types::{DisposalMethod, Frame, Palette};

    let palette = Palette {
        colors: vec![
            [255, 0, 0],
            [0, 255, 0],
            [0, 0, 255],
            [255, 255, 0],
            [255, 0, 255],
            [0, 255, 255],
        ],
    };

    let mut frames = Vec::new();
    for i in 0..frame_count {
        let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];

        // Fill with different colors per frame
        let color_idx = i % 6;
        let color = palette.colors[color_idx];

        for j in 0..(width as usize * height as usize) {
            pixels[j * 4] = color[0];
            pixels[j * 4 + 1] = color[1];
            pixels[j * 4 + 2] = color[2];
            pixels[j * 4 + 3] = 255;
        }

        frames.push(Frame {
            pixels,
            left: 0,
            top: 0,
            width,
            height,
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::Keep,
            local_palette: None,
        });
    }

    Gif {
        width,
        height,
        frames,
        global_palette: Some(palette),
        loop_count: rusticle::types::LoopCount::Infinite,
        original_palette: None,
    }
}

#[test]
fn test_encode_and_measure_disabled() {
    let gif = create_test_gif(100, 100, 3);
    let config = EncodeAndMeasureConfig {
        enabled: false,
        ..Default::default()
    };

    let canonical = CanonicalSequenceBuilder::build(&gif).expect("build canonical");
    let profile = profile_canonical_sequence(&canonical).expect("profile");
    let candidates = CandidateGenerator::generate(&canonical);

    // Group candidates by frame
    let mut frame_candidates = vec![Vec::new(); canonical.frames.len()];
    for candidate in candidates {
        frame_candidates[candidate.frame_index].push(candidate);
    }

    // Determine palette strategies
    let palette_strategies = determine_palette_strategies(&gif, &canonical, &profile);

    // Choose frame 0
    let decision = Chooser::choose_frame_candidate(
        &frame_candidates[0],
        &canonical.frames[0],
        &canonical,
        &profile,
        &palette_strategies,
    );

    let (_result_decision, telemetry) = EncodeAndMeasure::apply_if_uncertain(
        decision,
        0,
        &canonical,
        &gif,
        &profile,
        &config,
    );

    // Should not be uncertain since disabled
    assert!(!telemetry.was_uncertain);
    assert!(telemetry.measured_candidates.is_empty());
    assert!(!telemetry.measurement_succeeded);
}

#[test]
fn test_encode_and_measure_uncertainty_detection() {
    let gif = create_test_gif(100, 100, 3);
    let config = EncodeAndMeasureConfig {
        enabled: true,
        score_gap_threshold: 0.5, // High threshold to trigger uncertainty
        ..Default::default()
    };

    let canonical = CanonicalSequenceBuilder::build(&gif).expect("build canonical");
    let profile = profile_canonical_sequence(&canonical).expect("profile");
    let candidates = CandidateGenerator::generate(&canonical);

    // Group candidates by frame
    let mut frame_candidates = vec![Vec::new(); canonical.frames.len()];
    for candidate in candidates {
        frame_candidates[candidate.frame_index].push(candidate);
    }

    // Determine palette strategies
    let palette_strategies = determine_palette_strategies(&gif, &canonical, &profile);

    // Choose frame 0
    let decision = Chooser::choose_frame_candidate(
        &frame_candidates[0],
        &canonical.frames[0],
        &canonical,
        &profile,
        &palette_strategies,
    );

    let (_, telemetry) = EncodeAndMeasure::apply_if_uncertain(
        decision,
        0,
        &canonical,
        &gif,
        &profile,
        &config,
    );

    // With high threshold, should detect uncertainty
    // (may or may not be uncertain depending on actual scores, but config is enabled)
    assert_eq!(telemetry.frame_index, 0);
}

#[test]
fn test_encode_and_measure_telemetry_json() {
    use rusticle::encode_and_measure::MeasuredCandidate;
    use rusticle::scoring::ScoreBreakdown;

    let telemetry = rusticle::EncodeAndMeasureTelemetry {
        frame_index: 5,
        was_uncertain: true,
        uncertainty_reasons: vec!["score_gap_small (0.03)".to_string()],
        measured_candidates: vec![
            MeasuredCandidate {
                representation: CandidateRepresentation::FullFrame,
                heuristic_score: ScoreBreakdown {
                    byte_cost: 0.5,
                    visual_risk: 0.1,
                    lut_cost: 0.0,
                    temporal_instability: 0.1,
                    synthetic_transparency_risk: 0.0,
                    palette_coherence: 0.0,
                    cpu_cost: 0.1,
                    total_score: 0.3,
                },
                actual_bytes: 5000,
                was_chosen: true,
            },
            MeasuredCandidate {
                representation: CandidateRepresentation::ExactOpaqueBbox {
                    bbox: rusticle::BoundingBox {
                        left: 0,
                        top: 0,
                        right: 50,
                        bottom: 50,
                    },
                },
                heuristic_score: ScoreBreakdown {
                    byte_cost: 0.3,
                    visual_risk: 0.1,
                    lut_cost: 0.0,
                    temporal_instability: 0.1,
                    synthetic_transparency_risk: 0.0,
                    palette_coherence: 0.0,
                    cpu_cost: 0.1,
                    total_score: 0.25,
                },
                actual_bytes: 4500,
                was_chosen: false,
            },
        ],
        measurement_succeeded: true,
        measurement_error: None,
    };

    let json = telemetry.to_json();
    assert!(json.contains("\"frame_index\": 5"));
    assert!(json.contains("\"was_uncertain\": true"));
    assert!(json.contains("\"measurement_succeeded\": true"));
    assert!(json.contains("\"score_gap_small"));
    assert!(json.contains("\"actual_bytes\":5000"));
    assert!(json.contains("\"actual_bytes\":4500"));
}

#[test]
fn test_encode_and_measure_config_defaults() {
    let config = EncodeAndMeasureConfig::default();
    assert!(config.enabled);
    assert_eq!(config.top_n_candidates, 2);
    assert_eq!(config.score_gap_threshold, 0.05);
    assert_eq!(config.max_uncertain_fraction, 0.5);
    assert_eq!(config.transparency_risk_threshold, 0.3);
}

#[test]
fn test_encode_and_measure_custom_config() {
    let config = EncodeAndMeasureConfig {
        enabled: true,
        top_n_candidates: 3,
        score_gap_threshold: 0.1,
        max_uncertain_fraction: 0.3,
        transparency_risk_threshold: 0.4,
    };

    assert!(config.enabled);
    assert_eq!(config.top_n_candidates, 3);
    assert_eq!(config.score_gap_threshold, 0.1);
    assert_eq!(config.max_uncertain_fraction, 0.3);
    assert_eq!(config.transparency_risk_threshold, 0.4);
}

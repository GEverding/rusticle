mod common;

use common::create_test_gif;
use rusticle::{AdaptiveConfig, Gif};

#[test]
fn test_adaptive_mode_disabled_uses_default_path() {
    let gif = create_test_gif(100, 100, 3);

    let config = AdaptiveConfig {
        enabled: false,
        emit_telemetry: false,
    };

    let (decision, bytes) = gif.encode_adaptive(&config).expect("Failed to encode");

    // Should not succeed in adaptive mode
    assert!(!decision.success);
    assert!(decision.fallback_reason.is_some());
    assert_eq!(
        decision.fallback_reason.as_ref().unwrap(),
        "adaptive mode disabled"
    );
    assert!(decision.telemetry_json.is_none());

    // But should still produce valid bytes
    assert!(!bytes.is_empty());

    // Verify it's valid GIF by decoding
    let decoded = Gif::from_bytes(&bytes).expect("Failed to decode");
    assert_eq!(decoded.width, 100);
    assert_eq!(decoded.height, 100);
    assert_eq!(decoded.frames.len(), 3);
}

#[test]
fn test_adaptive_mode_enabled_produces_telemetry() {
    let gif = create_test_gif(100, 100, 3);

    let config = AdaptiveConfig {
        enabled: true,
        emit_telemetry: false, // Don't print to stderr in test
    };

    let (decision, bytes) = gif.encode_adaptive(&config).expect("Failed to encode");

    // Should succeed in adaptive mode
    assert!(decision.success);
    assert!(decision.fallback_reason.is_none());

    // Should have telemetry JSON
    assert!(decision.telemetry_json.is_some());
    let telemetry = decision.telemetry_json.unwrap();
    assert!(telemetry.contains("\"mode\":\"adaptive\""));
    assert!(telemetry.contains("\"status\":\"success\""));
    assert!(telemetry.contains("\"frame_decisions\""));

    // Should still produce valid bytes
    assert!(!bytes.is_empty());

    // Verify it's valid GIF by decoding
    let decoded = Gif::from_bytes(&bytes).expect("Failed to decode");
    assert_eq!(decoded.width, 100);
    assert_eq!(decoded.height, 100);
    assert_eq!(decoded.frames.len(), 3);
}

#[test]
fn test_adaptive_telemetry_contains_frame_decisions() {
    let gif = create_test_gif(100, 100, 5);

    let config = AdaptiveConfig {
        enabled: true,
        emit_telemetry: false,
    };

    let (decision, _bytes) = gif.encode_adaptive(&config).expect("Failed to encode");

    assert!(decision.success);
    let telemetry = decision.telemetry_json.unwrap();

    // Should contain frame decisions for all 5 frames
    assert!(telemetry.contains("\"frame_index\":0"));
    assert!(telemetry.contains("\"frame_index\":1"));
    assert!(telemetry.contains("\"frame_index\":2"));
    assert!(telemetry.contains("\"frame_index\":3"));
    assert!(telemetry.contains("\"frame_index\":4"));

    // Should contain score breakdowns
    assert!(telemetry.contains("\"byte_cost\""));
    assert!(telemetry.contains("\"visual_risk\""));
    assert!(telemetry.contains("\"temporal_instability\""));
    assert!(telemetry.contains("\"synthetic_transparency_risk\""));
    assert!(telemetry.contains("\"cpu_cost\""));
    assert!(telemetry.contains("\"total_score\""));

    // Should contain chosen representations
    assert!(telemetry.contains("\"chosen_representation\""));

    // Should contain decision reasons
    assert!(telemetry.contains("\"reason\""));
}

#[test]
fn test_adaptive_telemetry_json_is_valid() {
    let gif = create_test_gif(100, 100, 2);

    let config = AdaptiveConfig {
        enabled: true,
        emit_telemetry: false,
    };

    let (decision, _bytes) = gif.encode_adaptive(&config).expect("Failed to encode");

    assert!(decision.success);
    let telemetry = decision.telemetry_json.unwrap();

    // Try to parse as JSON (basic validation)
    // We can't use serde_json without adding it as a dependency,
    // but we can do basic structural checks
    assert!(telemetry.starts_with('{'));
    assert!(telemetry.ends_with('}'));
    assert!(telemetry.contains("\"mode\""));
    assert!(telemetry.contains("\"status\""));
    assert!(telemetry.contains("\"sequence\""));
    assert!(telemetry.contains("\"frame_decisions\""));
}

#[test]
fn test_adaptive_handles_empty_gif() {
    // Create a minimal GIF with no frames
    let gif = Gif {
        width: 100,
        height: 100,
        global_palette: None,
        frames: vec![],
        loop_count: rusticle::LoopCount::Infinite,
        original_palette: None,
    };

    let config = AdaptiveConfig {
        enabled: true,
        emit_telemetry: false,
    };

    let (_decision, bytes) = gif.encode_adaptive(&config).expect("Failed to encode");

    // Empty GIF should still be handled (either success with empty telemetry or fallback)
    // The important thing is that it doesn't panic and produces valid bytes
    assert!(!bytes.is_empty());
}

#[test]
fn test_adaptive_mode_does_not_change_output_bytes() {
    let gif = create_test_gif(100, 100, 3);

    // Encode with adaptive disabled
    let config_disabled = AdaptiveConfig {
        enabled: false,
        emit_telemetry: false,
    };
    let (_decision1, bytes1) = gif.encode_adaptive(&config_disabled).expect("Failed to encode");

    // Encode with adaptive enabled
    let config_enabled = AdaptiveConfig {
        enabled: true,
        emit_telemetry: false,
    };
    let (_decision2, bytes2) = gif.encode_adaptive(&config_enabled).expect("Failed to encode");

    // For now, both should produce the same bytes since adaptive mode
    // only emits telemetry but doesn't change the actual encoding
    // (This is the "experimental harness" behavior)
    assert_eq!(bytes1, bytes2);
}

#[test]
fn test_adaptive_telemetry_includes_sequence_info() {
    let gif = create_test_gif(200, 150, 4);

    let config = AdaptiveConfig {
        enabled: true,
        emit_telemetry: false,
    };

    let (decision, _bytes) = gif.encode_adaptive(&config).expect("Failed to encode");

    assert!(decision.success);
    let telemetry = decision.telemetry_json.unwrap();

    // Should contain sequence dimensions
    assert!(telemetry.contains("\"width\":200"));
    assert!(telemetry.contains("\"height\":150"));
    assert!(telemetry.contains("\"frame_count\":4"));

    // Should contain taxonomy
    assert!(telemetry.contains("\"taxonomy\""));

    // Should contain average score
    assert!(telemetry.contains("\"avg_score\""));

    // Should contain estimated bytes
    assert!(telemetry.contains("\"estimated_bytes\""));
}

#[test]
fn test_adaptive_telemetry_includes_decision_reasons() {
    let gif = create_test_gif(100, 100, 3);

    let config = AdaptiveConfig {
        enabled: true,
        emit_telemetry: false,
    };

    let (decision, _bytes) = gif.encode_adaptive(&config).expect("Failed to encode");

    assert!(decision.success);
    let telemetry = decision.telemetry_json.unwrap();

    // Should contain reason codes
    assert!(telemetry.contains("\"reason\""));

    // Reason should be one of the valid values
    let valid_reasons = [
        "lowest-score",
        "taxonomy-preferred",
        "safety-constraint",
        "palette-strategy-alignment",
        "tie-breaker",
        "fallback",
    ];

    let has_valid_reason = valid_reasons.iter().any(|r| telemetry.contains(&format!("\"reason\":\"{}\"", r)));
    assert!(has_valid_reason, "Telemetry should contain a valid reason code");
}

#[test]
fn test_adaptive_telemetry_includes_palette_strategy() {
    let gif = create_test_gif(100, 100, 3);

    let config = AdaptiveConfig {
        enabled: true,
        emit_telemetry: false,
    };

    let (decision, _bytes) = gif.encode_adaptive(&config).expect("Failed to encode");

    assert!(decision.success);
    let telemetry = decision.telemetry_json.unwrap();

    // Should contain chosen palette strategy
    assert!(telemetry.contains("\"chosen_palette_strategy\""));

    // Strategy should be one of the valid values
    let valid_strategies = [
        "reuse-global-preferred",
        "derive-sequence-global-preferred",
        "local-palette-fallback",
    ];

    let has_valid_strategy = valid_strategies
        .iter()
        .any(|s| telemetry.contains(&format!("\"chosen_palette_strategy\":\"{}\"", s)));
    assert!(
        has_valid_strategy,
        "Telemetry should contain a valid palette strategy"
    );
}

#[test]
fn test_adaptive_telemetry_includes_chosen_representation() {
    let gif = create_test_gif(100, 100, 3);

    let config = AdaptiveConfig {
        enabled: true,
        emit_telemetry: false,
    };

    let (decision, _bytes) = gif.encode_adaptive(&config).expect("Failed to encode");

    assert!(decision.success);
    let telemetry = decision.telemetry_json.unwrap();

    // Should contain chosen representation
    assert!(telemetry.contains("\"chosen_representation\""));

    // Representation should be one of the valid values
    let valid_reprs = [
        "full-frame",
        "opaque-bbox",
        "sparse-patch",
        "minimal-noop",
    ];

    let has_valid_repr = valid_reprs
        .iter()
        .any(|r| telemetry.contains(&format!("\"chosen_representation\":\"{}\"", r)));
    assert!(
        has_valid_repr,
        "Telemetry should contain a valid representation"
    );
}

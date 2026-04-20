# Adaptive Encoding Integration - Implementation Summary

## Overview

This document describes the integration of the experimental adaptive encoding pipeline behind an explicit opt-in flag with decision telemetry and safe fallback behavior.

## Implementation

### 1. New Module: `adaptive_encode.rs`

**Location:** `crates/rusticle/src/adaptive_encode.rs`

**Purpose:** Experimental harness that integrates the adaptive encoding pipeline with telemetry and fallback.

**Key Types:**

- `AdaptiveConfig`: Configuration struct for enabling/disabling adaptive mode and telemetry
  ```rust
  pub struct AdaptiveConfig {
      pub enabled: bool,           // Enable adaptive encoding mode
      pub emit_telemetry: bool,    // Emit telemetry to stderr
  }
  ```

- `AdaptiveDecision`: Result of adaptive encoding attempt
  ```rust
  pub struct AdaptiveDecision {
      pub success: bool,                    // Whether adaptive path succeeded
      pub fallback_reason: Option<String>,  // Reason if fallback occurred
      pub telemetry_json: Option<String>,   // Decision telemetry as JSON
  }
  ```

**Public API:**

```rust
impl Gif {
    pub fn encode_adaptive(&self, config: &AdaptiveConfig) 
        -> Result<(AdaptiveDecision, Vec<u8>)>
}
```

### 2. Adaptive Pipeline Integration

The `encode_adaptive` method orchestrates the full adaptive encoding pipeline:

1. **Build Canonical IR** - `CanonicalSequenceBuilder::build()`
   - Converts GIF frames to canonical display/canvas space
   - Ensures correctness invariants

2. **Profile Sequence** - `profile_canonical_sequence()`
   - Extracts structural features (disposal, transparency, palette info, etc.)
   - Classifies sequence into taxonomy (OpaqueDeltaGlobalPalette, TransparencyHeavy, etc.)

3. **Generate Candidates** - `CandidateGenerator::generate()`
   - Produces candidate representations for each frame
   - Types: FullFrame, ExactOpaqueBbox, TransparentSparsePatch, MinimalNoOp

4. **Determine Palette Strategies** - `determine_palette_strategies()`
   - Selects palette strategy based on sequence characteristics
   - Strategies: ReuseGlobalPreferred, DeriveSequenceGlobalPreferred, LocalPaletteFallback

5. **Score and Choose** - `Chooser::choose_sequence()`
   - Scores candidates across multiple dimensions
   - Selects best representation per frame with reasoning

6. **Emit Telemetry** - `build_telemetry_json()`
   - Generates structured JSON with all decisions and scores

### 3. Telemetry Format

Decision telemetry is emitted as JSON with the following structure:

```json
{
  "mode": "adaptive",
  "status": "success" | "fallback",
  "sequence": {
    "width": 320,
    "height": 240,
    "frame_count": 24,
    "taxonomy": "opaque-delta/global-palette",
    "avg_score": 0.086,
    "estimated_bytes": 480
  },
  "frame_decisions": [
    {
      "frame_index": 0,
      "chosen_representation": "minimal-noop" | "full-frame" | "opaque-bbox" | "sparse-patch",
      "chosen_palette_strategy": "reuse-global-preferred" | "derive-sequence-global-preferred" | "local-palette-fallback",
      "score_breakdown": {
        "byte_cost": 0.020,
        "visual_risk": 0.100,
        "temporal_instability": 0.230,
        "synthetic_transparency_risk": 0.100,
        "cpu_cost": 0.050,
        "total_score": 0.087
      },
      "reason": "lowest-score" | "taxonomy-preferred" | "safety-constraint" | "palette-strategy-alignment" | "tie-breaker" | "fallback",
      "explanation": "Frame 0: minimal-noop (score: 0.087, ...) - lowest total score - palette: reuse-global-preferred"
    }
  ]
}
```

### 4. Safe Fallback Behavior

If any step in the adaptive pipeline fails:
- The encoder falls back to the current (non-adaptive) encoding path
- The fallback reason is recorded in telemetry
- Telemetry is emitted with `"status": "fallback"` and `"fallback_reason": "..."`
- The output GIF is still valid and correct

Example fallback telemetry:
```json
{
  "mode": "adaptive",
  "status": "fallback",
  "fallback_reason": "adaptive path failed: ..."
}
```

### 5. CLI Integration

**New flags for `rusticle resize` command:**

```bash
--adaptive                  # Enable experimental adaptive encoding mode
--adaptive-telemetry        # Emit adaptive encoding telemetry to stderr
```

**Example usage:**

```bash
# Enable adaptive mode with telemetry
rusticle resize input.gif --width 320 --height 240 --adaptive --adaptive-telemetry

# Enable adaptive mode without telemetry
rusticle resize input.gif --width 320 --height 240 --adaptive
```

**Output:**

When `--adaptive-telemetry` is enabled, telemetry is printed to stderr with prefix `ADAPTIVE_TELEMETRY:`:

```
ADAPTIVE_TELEMETRY: {"mode":"adaptive","status":"success",...}
Adaptive: success=true, fallback_reason=None
```

### 6. Library API Integration

**Export in `lib.rs`:**

```rust
pub use adaptive_encode::{AdaptiveConfig, AdaptiveDecision};
```

**Usage example:**

```rust
use rusticle::{Gif, AdaptiveConfig};

let gif = Gif::from_bytes(&data)?;
let config = AdaptiveConfig {
    enabled: true,
    emit_telemetry: true,
};

let (decision, bytes) = gif.encode_adaptive(&config)?;

if decision.success {
    println!("Adaptive encoding succeeded!");
    if let Some(json) = decision.telemetry_json {
        println!("Telemetry: {}", json);
    }
} else {
    println!("Fell back to default path: {:?}", decision.fallback_reason);
}
```

## Design Decisions

### 1. Opt-In Behavior

- Adaptive mode is **disabled by default**
- Must be explicitly enabled via `AdaptiveConfig { enabled: true }`
- Default behavior is unchanged

### 2. Telemetry-Only (For Now)

- The adaptive path currently **emits telemetry but does not change encoding**
- This is intentional: the harness is experimental and must not destabilize the default path
- Future work can use the decisions to guide actual quantization/encoding

### 3. Safe Fallback

- Any error in the adaptive pipeline triggers fallback
- Fallback is explicit in telemetry
- No silent failures or undefined behavior

### 4. Structured Telemetry

- JSON format for easy parsing and analysis
- Includes all decision dimensions and reasoning
- Can be piped to analysis tools or stored for later review

## Testing

### Unit Tests

All existing tests pass without modification.

### Integration Tests

New test file: `crates/rusticle/tests/adaptive_encoding_test.rs`

**Test Coverage:**

1. `test_adaptive_mode_disabled_uses_default_path` - Verify disabled mode doesn't change behavior
2. `test_adaptive_mode_enabled_produces_telemetry` - Verify telemetry is generated
3. `test_adaptive_telemetry_contains_frame_decisions` - Verify all frames are in telemetry
4. `test_adaptive_telemetry_json_is_valid` - Verify JSON structure
5. `test_adaptive_fallback_on_empty_gif` - Verify graceful handling of edge cases
6. `test_adaptive_mode_does_not_change_output_bytes` - Verify output is identical
7. `test_adaptive_telemetry_includes_sequence_info` - Verify sequence metadata
8. `test_adaptive_telemetry_includes_decision_reasons` - Verify reason codes
9. `test_adaptive_telemetry_includes_palette_strategy` - Verify strategy selection
10. `test_adaptive_telemetry_includes_chosen_representation` - Verify representation selection

**All tests pass:**
```
running 10 tests
test result: ok. 10 passed; 0 failed
```

### CLI Testing

Tested with real GIF:
```bash
$ rusticle resize test_gifs/benchmark_suite/cartoon_01.gif \
    --width 320 --height 240 --adaptive --adaptive-telemetry

Input:  test_gifs/benchmark_suite/cartoon_01.gif (480x480, 24 frames, 1.78 MB)
ADAPTIVE_TELEMETRY: {"mode":"adaptive","status":"success",...}
Adaptive: success=true, fallback_reason=None
Output: cartoon_01_rusticle.gif (320x240, 1.30 MB, 73.4% of input)
```

## Validation

### Compilation

```bash
$ cargo check -p rusticle
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.03s

$ cargo check -p rusticle-cli
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.54s
```

### Linting

```bash
$ cargo clippy -p rusticle -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.03s

$ cargo clippy -p rusticle-cli -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.71s
```

### Tests

```bash
$ cargo test -p rusticle --lib
test result: ok. 156 passed; 0 failed

$ cargo test -p rusticle --test adaptive_encoding_test
test result: ok. 10 passed; 0 failed
```

## Files Changed

### New Files

- `crates/rusticle/src/adaptive_encode.rs` - Adaptive encoding harness (286 lines)
- `crates/rusticle/tests/adaptive_encoding_test.rs` - Comprehensive tests (300+ lines)

### Modified Files

- `crates/rusticle/src/lib.rs` - Added module and exports
- `crates/rusticle-cli/src/main.rs` - Added CLI flags and integration

## Future Work

1. **Actual Encoding Integration** - Use adaptive decisions to guide quantization
2. **Performance Tuning** - Optimize scoring weights based on real-world data
3. **Extended Telemetry** - Add timing information, memory usage, etc.
4. **Adaptive Quantization** - Apply different quantization strategies per frame
5. **Palette Optimization** - Use adaptive decisions to optimize palette selection

## Summary

The adaptive encoding integration provides:

✅ **Explicit opt-in** via `AdaptiveConfig`
✅ **Full pipeline integration** (IR → profiler → candidates → palette → scorer → chooser)
✅ **Structured telemetry** with JSON output
✅ **Safe fallback** with explicit error tracking
✅ **Comprehensive tests** (10 integration tests, all passing)
✅ **CLI support** with `--adaptive` and `--adaptive-telemetry` flags
✅ **Zero impact on default behavior** (disabled by default)
✅ **Production-ready code** (no unwrap(), proper error handling)

The implementation is modest and experimental, as intended. It provides a clean harness for the adaptive encoding pipeline without destabilizing the current path.

# Rusticle - Agent Guidelines

High-performance GIF processing library for decoding, resizing, optimizing, and encoding.

## Build & Test

```bash
cargo check                              # Check all crates
cargo check -p rusticle                  # Check library only
cargo build --release -p rusticle-cli    # Build CLI binary

cargo test -p rusticle                   # Library tests
cargo test -p rusticle --lib             # Unit tests only
cargo test -p rusticle test_decode_empty # Single test by name

cargo clippy --workspace -- -D warnings  # Lint all crates
cargo fmt --all                          # Format all crates

cargo run -p rusticle-bench              # Run regression benchmarks
cargo bench -p rusticle                  # criterion benchmarks
```

## Project Structure

```
crates/
├── rusticle/              # Library crate
│   ├── src/
│   │   ├── lib.rs         # Public API, re-exports
│   │   ├── decode.rs      # GIF decoding with compositing
│   │   ├── encode.rs      # GIF encoding with quantization
│   │   ├── resize.rs      # Frame resizing (fast_image_resize)
│   │   ├── optimize.rs    # Frame optimization, lossy compression
│   │   ├── types.rs       # Core types: Gif, Frame, Palette, etc.
│   │   ├── error.rs       # Error types via thiserror
│   │   ├── quality.rs     # PSNR/SSIM quality metrics
│   │   ├── palette_lut.rs # Palette LUT for fast color mapping
│   │   ├── simd_opt.rs    # SIMD-accelerated pixel operations
│   │   └── async_io.rs    # Async I/O (optional "async" feature)
│   ├── tests/             # Integration tests
│   ├── benches/           # criterion benchmarks
│   └── examples/          # Library examples
├── rusticle-cli/          # CLI binary crate
│   └── src/main.rs        # clap-based resize/quality CLI
└── rusticle-bench/        # Internal benchmark crate (publish=false)
    └── src/main.rs        # Regression bench vs gifsicle

scripts/
├── download_test_gifs.py
└── check_benchmark_baseline.py

docs/
├── BENCHMARKS.md
└── bench_baseline.json
```

## Error Handling

- Use `thiserror` for error definitions
- Return `Result<T, Error>` or `crate::Result<T>`
- No `unwrap()` in library code—use `?` operator
- `unwrap()` acceptable only in tests

```rust
// Good
pub fn from_bytes(data: &[u8]) -> Result<Self> {
    let reader = decoder.read_info(data)
        .map_err(|e| Error::DecodeError(e.to_string()))?;
    Ok(...)
}
```

## Types & Signatures

- Use `&str` not `&String`, `&[T]` not `&Vec<T>`
- Add `#[must_use]` on methods returning values that shouldn't be ignored
- Consuming methods take `self`, non-consuming take `&self`

## Imports

Group: std, external crates, crate internals. Use `crate::` prefix.

```rust
use std::io::Write;

use rayon::prelude::*;

use crate::error::{Error, Result};
use crate::types::{Frame, Gif};
```

## Documentation

Doc comments on all public items. Include `# Example` with `ignore` attribute.

```rust
/// Resize to exact dimensions.
///
/// # Errors
/// Returns `Error::InvalidDimensions` if width is zero.
///
/// # Example
/// ```ignore
/// let resized = gif.resize(640, 480, Filter::Lanczos3)?;
/// ```
pub fn resize(self, width: u32, ...) -> Result<Gif, Error>
```

## Naming

- Types: `PascalCase` (Gif, Frame, OptLevel)
- Functions: `snake_case` (from_bytes, resize_fit)
- Constants: `SCREAMING_SNAKE_CASE`
- Enum variants: `PascalCase` (OptLevel::O1, Filter::Lanczos3)

## Performance

- Use `rayon` for parallel frame processing
- Use `#[inline]` on small, frequently-called functions
- Platform allocator: jemalloc on non-MSVC targets

## Testing

Unit tests in `#[cfg(test)] mod tests`. Integration tests in `tests/`.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resize_zero_width_error() {
        let gif = make_test_gif();
        let result = gif.resize(0, 50, Filter::Lanczos3);
        assert!(result.is_err());
    }
}
```

## Key Dependencies

| Crate | Purpose |
|-------|---------|
| gif | GIF encode/decode (library) |
| thiserror | Error derive (library) |
| imagequant | Color quantization (library) |
| fast_image_resize | Frame resizing (library) |
| rayon | Parallel processing (library) |
| clap | CLI argument parsing (rusticle-cli) |
| tikv-jemallocator | Memory allocator (rusticle-cli) |
| serde/chrono | Benchmark serialization (rusticle-bench) |
| criterion | Benchmarking (library dev-dep) |

## Common Patterns

```rust
// Method chaining
let bytes = gif
    .resize(640, 480, Filter::Lanczos3)?
    .optimize(OptLevel::O3)
    .lossy(80)
    .into_bytes()?;

// Parallel frame processing
let frames: Vec<Frame> = self.frames
    .par_iter()
    .map(|frame| process_frame(frame))
    .collect::<Result<Vec<_>, _>>()?;

// Test GIF creation
use crate::tests::common::{create_test_gif, create_gradient_gif};
let gif = create_test_gif(100, 100, 3);  // width, height, frame_count
```

## Pre-commit

1. `cargo fmt`
2. `cargo clippy -- -D warnings`
3. `cargo test`
4. Update docs if public API changed

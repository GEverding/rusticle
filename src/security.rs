//! Security limits and validation for GIF processing.
//!
//! This module provides protection against GIF bombs and other malicious inputs
//! that could cause memory exhaustion or denial of service.

/// Maximum allowed width/height in pixels (4K resolution).
pub const MAX_DIMENSION: u16 = 4096;

/// Maximum number of frames allowed in a GIF.
/// This prevents "movie GIF" attacks (e.g., entire movies encoded as GIFs).
pub const MAX_FRAME_COUNT: usize = 10_000;

/// Maximum total memory budget for a single GIF in bytes (1 GB).
/// Calculated as: width * height * 4 (RGBA) * frame_count
pub const MAX_MEMORY_BYTES: usize = 1024 * 1024 * 1024;

/// Security limits configuration for GIF decoding.
///
/// Use `SecurityLimits::default()` for recommended safe defaults,
/// or customize limits for specific use cases.
#[derive(Debug, Clone, Copy)]
pub struct SecurityLimits {
    /// Maximum width in pixels.
    pub max_width: u16,
    /// Maximum height in pixels.
    pub max_height: u16,
    /// Maximum number of frames.
    pub max_frame_count: usize,
    /// Maximum total memory in bytes.
    pub max_memory_bytes: usize,
}

impl Default for SecurityLimits {
    fn default() -> Self {
        Self {
            max_width: MAX_DIMENSION,
            max_height: MAX_DIMENSION,
            max_frame_count: MAX_FRAME_COUNT,
            max_memory_bytes: MAX_MEMORY_BYTES,
        }
    }
}

impl SecurityLimits {
    /// Create limits with no restrictions (use with caution!).
    ///
    /// # Safety
    /// This can allow processing of malicious GIFs that may exhaust memory.
    /// Only use when you trust the input source completely.
    pub fn unrestricted() -> Self {
        Self {
            max_width: u16::MAX,
            max_height: u16::MAX,
            max_frame_count: usize::MAX,
            max_memory_bytes: usize::MAX,
        }
    }

    /// Create limits optimized for thumbnails (256x256, 100 frames).
    pub fn thumbnail() -> Self {
        Self {
            max_width: 256,
            max_height: 256,
            max_frame_count: 100,
            max_memory_bytes: 64 * 1024 * 1024, // 64 MB
        }
    }

    /// Create limits optimized for web use (1024x1024, 500 frames).
    pub fn web() -> Self {
        Self {
            max_width: 1024,
            max_height: 1024,
            max_frame_count: 500,
            max_memory_bytes: 256 * 1024 * 1024, // 256 MB
        }
    }

    /// Validate dimensions against limits.
    pub fn validate_dimensions(&self, width: u16, height: u16) -> Result<(), SecurityViolation> {
        if width > self.max_width {
            return Err(SecurityViolation::DimensionExceeded {
                dimension: "width",
                value: width as usize,
                limit: self.max_width as usize,
            });
        }
        if height > self.max_height {
            return Err(SecurityViolation::DimensionExceeded {
                dimension: "height",
                value: height as usize,
                limit: self.max_height as usize,
            });
        }
        Ok(())
    }

    /// Validate frame count against limit.
    pub fn validate_frame_count(&self, count: usize) -> Result<(), SecurityViolation> {
        if count > self.max_frame_count {
            return Err(SecurityViolation::FrameCountExceeded {
                count,
                limit: self.max_frame_count,
            });
        }
        Ok(())
    }

    /// Calculate memory required for a GIF and validate against limit.
    ///
    /// Returns the calculated memory size if within limits.
    pub fn validate_memory(
        &self,
        width: u16,
        height: u16,
        frame_count: usize,
    ) -> Result<usize, SecurityViolation> {
        let memory = calculate_memory_safe(width, height, frame_count)?;
        if memory > self.max_memory_bytes {
            return Err(SecurityViolation::MemoryExceeded {
                required: memory,
                limit: self.max_memory_bytes,
            });
        }
        Ok(memory)
    }

    /// Validate a single frame's canvas allocation.
    pub fn validate_canvas_size(&self, width: u16, height: u16) -> Result<usize, SecurityViolation> {
        calculate_canvas_size_safe(width, height)
    }
}

/// Security violation errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecurityViolation {
    /// Image dimension exceeds limit.
    DimensionExceeded {
        dimension: &'static str,
        value: usize,
        limit: usize,
    },
    /// Frame count exceeds limit.
    FrameCountExceeded { count: usize, limit: usize },
    /// Memory requirement exceeds limit.
    MemoryExceeded { required: usize, limit: usize },
    /// Integer overflow in memory calculation.
    MemoryOverflow,
}

impl std::fmt::Display for SecurityViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecurityViolation::DimensionExceeded {
                dimension,
                value,
                limit,
            } => {
                write!(
                    f,
                    "GIF {} {} exceeds maximum allowed {} (possible GIF bomb)",
                    dimension, value, limit
                )
            }
            SecurityViolation::FrameCountExceeded { count, limit } => {
                write!(
                    f,
                    "GIF has {} frames, exceeding maximum {} (possible GIF bomb)",
                    count, limit
                )
            }
            SecurityViolation::MemoryExceeded { required, limit } => {
                write!(
                    f,
                    "GIF would require {} bytes, exceeding {} byte limit (possible GIF bomb)",
                    required, limit
                )
            }
            SecurityViolation::MemoryOverflow => {
                write!(f, "GIF dimensions would cause integer overflow (malicious input)")
            }
        }
    }
}

impl std::error::Error for SecurityViolation {}

/// Calculate canvas size with overflow protection.
///
/// Returns `Err(SecurityViolation::MemoryOverflow)` if the calculation would overflow.
pub fn calculate_canvas_size_safe(width: u16, height: u16) -> Result<usize, SecurityViolation> {
    let w = width as usize;
    let h = height as usize;

    // width * height
    let pixels = w.checked_mul(h).ok_or(SecurityViolation::MemoryOverflow)?;

    // pixels * 4 (RGBA)
    pixels
        .checked_mul(4)
        .ok_or(SecurityViolation::MemoryOverflow)
}

/// Calculate total memory for a GIF with overflow protection.
///
/// Returns `Err(SecurityViolation::MemoryOverflow)` if the calculation would overflow.
pub fn calculate_memory_safe(
    width: u16,
    height: u16,
    frame_count: usize,
) -> Result<usize, SecurityViolation> {
    let canvas_size = calculate_canvas_size_safe(width, height)?;

    // canvas_size * frame_count
    canvas_size
        .checked_mul(frame_count)
        .ok_or(SecurityViolation::MemoryOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_limits() {
        let limits = SecurityLimits::default();
        assert_eq!(limits.max_width, 4096);
        assert_eq!(limits.max_height, 4096);
        assert_eq!(limits.max_frame_count, 10_000);
    }

    #[test]
    fn test_validate_dimensions_ok() {
        let limits = SecurityLimits::default();
        assert!(limits.validate_dimensions(1920, 1080).is_ok());
        assert!(limits.validate_dimensions(4096, 4096).is_ok());
    }

    #[test]
    fn test_validate_dimensions_exceeded() {
        let limits = SecurityLimits::default();
        let result = limits.validate_dimensions(4097, 1080);
        assert!(matches!(
            result,
            Err(SecurityViolation::DimensionExceeded { dimension: "width", .. })
        ));
    }

    #[test]
    fn test_validate_frame_count_ok() {
        let limits = SecurityLimits::default();
        assert!(limits.validate_frame_count(100).is_ok());
        assert!(limits.validate_frame_count(10_000).is_ok());
    }

    #[test]
    fn test_validate_frame_count_exceeded() {
        let limits = SecurityLimits::default();
        let result = limits.validate_frame_count(10_001);
        assert!(matches!(
            result,
            Err(SecurityViolation::FrameCountExceeded { .. })
        ));
    }

    #[test]
    fn test_validate_memory_ok() {
        let limits = SecurityLimits::default();
        // 100x100 * 4 * 100 frames = 4MB
        assert!(limits.validate_memory(100, 100, 100).is_ok());
    }

    #[test]
    fn test_validate_memory_exceeded() {
        let limits = SecurityLimits::default();
        // 4096x4096 * 4 * 10000 frames = 671 GB - way over limit
        let result = limits.validate_memory(4096, 4096, 10_000);
        assert!(matches!(
            result,
            Err(SecurityViolation::MemoryExceeded { .. })
        ));
    }

    #[test]
    fn test_canvas_size_safe() {
        assert_eq!(calculate_canvas_size_safe(100, 100).unwrap(), 40_000);
        assert_eq!(calculate_canvas_size_safe(1920, 1080).unwrap(), 8_294_400);
    }

    #[test]
    fn test_thumbnail_limits() {
        let limits = SecurityLimits::thumbnail();
        assert_eq!(limits.max_width, 256);
        assert_eq!(limits.max_height, 256);
        assert_eq!(limits.max_frame_count, 100);

        // Should reject 512x512
        assert!(limits.validate_dimensions(512, 512).is_err());
    }

    #[test]
    fn test_web_limits() {
        let limits = SecurityLimits::web();
        assert_eq!(limits.max_width, 1024);
        assert_eq!(limits.max_height, 1024);
        assert_eq!(limits.max_frame_count, 500);
    }

    #[test]
    fn test_unrestricted_limits() {
        let limits = SecurityLimits::unrestricted();
        assert_eq!(limits.max_width, u16::MAX);
        assert_eq!(limits.max_height, u16::MAX);
        assert_eq!(limits.max_frame_count, usize::MAX);
    }

    #[test]
    fn test_security_violation_display() {
        let violation = SecurityViolation::DimensionExceeded {
            dimension: "width",
            value: 5000,
            limit: 4096,
        };
        let msg = violation.to_string();
        assert!(msg.contains("5000"));
        assert!(msg.contains("4096"));
        assert!(msg.contains("GIF bomb"));
    }
}

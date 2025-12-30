use std::time::Duration;

/// A decoded GIF with all its frames.
#[derive(Debug, Clone)]
pub struct Gif {
    /// Canvas width in pixels.
    pub width: u16,
    /// Canvas height in pixels.
    pub height: u16,
    /// Global color palette, if present.
    pub global_palette: Option<Palette>,
    /// All frames in the animation.
    pub frames: Vec<Frame>,
    /// Loop count for the animation.
    pub loop_count: LoopCount,
}

/// A single frame in a GIF animation.
#[derive(Debug, Clone)]
pub struct Frame {
    /// RGBA pixel data: width * height * 4 bytes.
    pub pixels: Vec<u8>,
    /// Delay before displaying the next frame.
    pub delay: Duration,
    /// How to dispose of this frame before showing the next.
    pub dispose: DisposalMethod,
    /// Local color palette for this frame, if present.
    pub local_palette: Option<Palette>,
    /// Horizontal offset of this frame on the canvas.
    pub left: u16,
    /// Vertical offset of this frame on the canvas.
    pub top: u16,
    /// Width of this frame.
    pub width: u16,
    /// Height of this frame.
    pub height: u16,
}

/// A color palette.
#[derive(Debug, Clone)]
pub struct Palette {
    /// RGB colors in the palette.
    pub colors: Vec<[u8; 3]>,
}

/// Loop count for GIF animation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopCount {
    /// Loop infinitely.
    Infinite,
    /// Loop a specific number of times.
    Finite(u16),
}

/// Frame disposal method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisposalMethod {
    /// No disposal specified.
    None,
    /// Keep the frame on the canvas.
    Keep,
    /// Restore to background color.
    Background,
    /// Restore to previous frame.
    Previous,
}

/// Resize filter algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    /// Nearest neighbor (fastest, lowest quality).
    Nearest,
    /// Bilinear interpolation.
    Bilinear,
    /// Mitchell cubic filter.
    Mitchell,
    /// Lanczos3 (highest quality, slowest).
    Lanczos3,
}

/// Optimization level for frame optimization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptLevel {
    /// Basic optimization - exact pixel match only.
    O1,
    /// Standard optimization - small color differences.
    O2,
    /// Aggressive optimization - best compression.
    O3,
}

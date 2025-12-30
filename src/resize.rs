use crate::{Error, Filter, Frame, Gif};
use fast_image_resize::{self as fir, images::Image, PixelType, ResizeOptions, Resizer};
use rayon::prelude::*;
use std::num::NonZeroU32;

/// Map our Filter enum to fast_image_resize FilterType.
fn to_fir_filter(filter: Filter) -> fir::FilterType {
    match filter {
        Filter::Nearest => fir::FilterType::Box,
        Filter::Bilinear => fir::FilterType::Bilinear,
        Filter::Mitchell => fir::FilterType::Mitchell,
        Filter::Lanczos3 => fir::FilterType::Lanczos3,
    }
}

/// Resize a single frame to new dimensions.
fn resize_frame(
    frame: &Frame,
    new_width: u32,
    new_height: u32,
    filter: Filter,
) -> Result<Frame, Error> {
    // If frame is full canvas, resize directly
    if frame.left == 0
        && frame.top == 0
        && frame.width as u32 == new_width
        && frame.height as u32 == new_height
    {
        // Frame already matches target dimensions
        return Ok(frame.clone());
    }

    // Calculate scale factors
    let scale_x = new_width as f64 / frame.width as f64;
    let scale_y = new_height as f64 / frame.height as f64;

    // New frame dimensions after scaling
    let scaled_width = (frame.width as f64 * scale_x).ceil() as u32;
    let scaled_height = (frame.height as f64 * scale_y).ceil() as u32;

    // Ensure non-zero dimensions
    let scaled_width = scaled_width.max(1);
    let scaled_height = scaled_height.max(1);

    // Create source image from frame pixels
    let src_width = NonZeroU32::new(frame.width as u32)
        .ok_or_else(|| Error::InvalidDimensions("frame width is zero".to_string()))?;
    let src_height = NonZeroU32::new(frame.height as u32)
        .ok_or_else(|| Error::InvalidDimensions("frame height is zero".to_string()))?;

    let src_image = Image::from_vec_u8(
        src_width.get(),
        src_height.get(),
        frame.pixels.clone(),
        PixelType::U8x4,
    )
    .map_err(|e| Error::ResizeError(format!("failed to create source image: {}", e)))?;

    // Create destination image
    let dst_width = NonZeroU32::new(scaled_width)
        .ok_or_else(|| Error::InvalidDimensions("scaled width is zero".to_string()))?;
    let dst_height = NonZeroU32::new(scaled_height)
        .ok_or_else(|| Error::InvalidDimensions("scaled height is zero".to_string()))?;

    let mut dst_image = Image::new(dst_width.get(), dst_height.get(), PixelType::U8x4);

    // Perform resize with filter
    let mut resizer = Resizer::new();
    let options =
        ResizeOptions::new().resize_alg(fir::ResizeAlg::Convolution(to_fir_filter(filter)));

    resizer
        .resize(&src_image, &mut dst_image, &options)
        .map_err(|e| Error::ResizeError(format!("resize operation failed: {}", e)))?;

    // Scale frame position
    let new_left = (frame.left as f64 * scale_x).round() as u16;
    let new_top = (frame.top as f64 * scale_y).round() as u16;

    Ok(Frame {
        pixels: dst_image.into_vec(),
        delay: frame.delay,
        dispose: frame.dispose,
        local_palette: frame.local_palette.clone(),
        left: new_left,
        top: new_top,
        width: scaled_width as u16,
        height: scaled_height as u16,
    })
}

impl Gif {
    /// Resize to exact dimensions.
    ///
    /// All frames are resized to the specified width and height.
    /// Frame positions are scaled proportionally.
    ///
    /// # Arguments
    /// * `width` - Target width in pixels
    /// * `height` - Target height in pixels
    /// * `filter` - Resize filter algorithm
    ///
    /// # Errors
    /// Returns `Error::InvalidDimensions` if width or height is zero,
    /// or `Error::ResizeError` if the resize operation fails.
    ///
    /// # Example
    /// ```ignore
    /// let gif = Gif::from_bytes(&data)?;
    /// let resized = gif.resize(640, 480, Filter::Lanczos3)?;
    /// ```
    pub fn resize(self, width: u32, height: u32, filter: Filter) -> Result<Gif, Error> {
        if width == 0 || height == 0 {
            return Err(Error::InvalidDimensions(
                "width and height must be > 0".to_string(),
            ));
        }

        // Process frames in parallel
        let resized_frames: Vec<Frame> = self
            .frames
            .par_iter()
            .map(|frame| resize_frame(frame, width, height, filter))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Gif {
            width: width as u16,
            height: height as u16,
            global_palette: self.global_palette.clone(),
            frames: resized_frames,
            loop_count: self.loop_count,
        })
    }

    /// Resize maintaining aspect ratio, fitting within bounds.
    ///
    /// Scales the GIF to fit within the specified maximum dimensions
    /// while maintaining the original aspect ratio.
    ///
    /// # Arguments
    /// * `max_width` - Maximum width in pixels
    /// * `max_height` - Maximum height in pixels
    /// * `filter` - Resize filter algorithm
    ///
    /// # Errors
    /// Returns `Error::InvalidDimensions` if max_width or max_height is zero,
    /// or `Error::ResizeError` if the resize operation fails.
    ///
    /// # Example
    /// ```ignore
    /// let gif = Gif::from_bytes(&data)?;
    /// let resized = gif.resize_fit(640, 480, Filter::Lanczos3)?;
    /// ```
    pub fn resize_fit(self, max_width: u32, max_height: u32, filter: Filter) -> Result<Gif, Error> {
        if max_width == 0 || max_height == 0 {
            return Err(Error::InvalidDimensions(
                "max_width and max_height must be > 0".to_string(),
            ));
        }

        // Calculate scale factor to fit within bounds
        let ratio = f64::min(
            max_width as f64 / self.width as f64,
            max_height as f64 / self.height as f64,
        );

        let new_width = (self.width as f64 * ratio) as u32;
        let new_height = (self.height as f64 * ratio) as u32;

        // Ensure at least 1x1
        let new_width = new_width.max(1);
        let new_height = new_height.max(1);

        self.resize(new_width, new_height, filter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn make_test_frame(width: u16, height: u16) -> Frame {
        let mut pixels = Vec::new();
        for y in 0..height {
            for x in 0..width {
                // Create a gradient pattern for testing
                let r = ((x as f32 / width as f32) * 255.0) as u8;
                let g = ((y as f32 / height as f32) * 255.0) as u8;
                let b = 128u8;
                let a = 255u8;
                pixels.extend_from_slice(&[r, g, b, a]);
            }
        }
        Frame {
            pixels,
            delay: Duration::from_millis(100),
            dispose: crate::types::DisposalMethod::None,
            local_palette: None,
            left: 0,
            top: 0,
            width,
            height,
        }
    }

    #[test]
    fn test_resize_exact_dimensions() {
        let frame = make_test_frame(100, 100);
        let gif = Gif {
            width: 100,
            height: 100,
            global_palette: None,
            frames: vec![frame],
            loop_count: crate::types::LoopCount::Infinite,
        };

        let resized = gif.resize(50, 50, Filter::Lanczos3).unwrap();
        assert_eq!(resized.width, 50);
        assert_eq!(resized.height, 50);
        assert_eq!(resized.frames.len(), 1);
    }

    #[test]
    fn test_resize_zero_width_error() {
        let frame = make_test_frame(100, 100);
        let gif = Gif {
            width: 100,
            height: 100,
            global_palette: None,
            frames: vec![frame],
            loop_count: crate::types::LoopCount::Infinite,
        };

        let result = gif.resize(0, 50, Filter::Lanczos3);
        assert!(result.is_err());
    }

    #[test]
    fn test_resize_zero_height_error() {
        let frame = make_test_frame(100, 100);
        let gif = Gif {
            width: 100,
            height: 100,
            global_palette: None,
            frames: vec![frame],
            loop_count: crate::types::LoopCount::Infinite,
        };

        let result = gif.resize(50, 0, Filter::Lanczos3);
        assert!(result.is_err());
    }

    #[test]
    fn test_resize_fit_maintains_aspect_ratio() {
        let frame = make_test_frame(200, 100);
        let gif = Gif {
            width: 200,
            height: 100,
            global_palette: None,
            frames: vec![frame],
            loop_count: crate::types::LoopCount::Infinite,
        };

        let original_ratio = 200.0 / 100.0;
        let resized = gif.resize_fit(100, 100, Filter::Lanczos3).unwrap();
        let new_ratio = resized.width as f64 / resized.height as f64;

        assert!((original_ratio - new_ratio).abs() < 0.01);
        assert!(resized.width <= 100);
        assert!(resized.height <= 100);
    }

    #[test]
    fn test_resize_fit_zero_max_width_error() {
        let frame = make_test_frame(100, 100);
        let gif = Gif {
            width: 100,
            height: 100,
            global_palette: None,
            frames: vec![frame],
            loop_count: crate::types::LoopCount::Infinite,
        };

        let result = gif.resize_fit(0, 100, Filter::Lanczos3);
        assert!(result.is_err());
    }

    #[test]
    fn test_resize_fit_zero_max_height_error() {
        let frame = make_test_frame(100, 100);
        let gif = Gif {
            width: 100,
            height: 100,
            global_palette: None,
            frames: vec![frame],
            loop_count: crate::types::LoopCount::Infinite,
        };

        let result = gif.resize_fit(100, 0, Filter::Lanczos3);
        assert!(result.is_err());
    }

    #[test]
    fn test_resize_multiple_frames() {
        let frame1 = make_test_frame(100, 100);
        let frame2 = make_test_frame(100, 100);
        let gif = Gif {
            width: 100,
            height: 100,
            global_palette: None,
            frames: vec![frame1, frame2],
            loop_count: crate::types::LoopCount::Infinite,
        };

        let resized = gif.resize(50, 50, Filter::Lanczos3).unwrap();
        assert_eq!(resized.frames.len(), 2);
        assert_eq!(resized.width, 50);
        assert_eq!(resized.height, 50);
    }

    #[test]
    fn test_resize_with_different_filters() {
        let frame = make_test_frame(100, 100);
        let gif = Gif {
            width: 100,
            height: 100,
            global_palette: None,
            frames: vec![frame],
            loop_count: crate::types::LoopCount::Infinite,
        };

        let filters = [
            Filter::Nearest,
            Filter::Bilinear,
            Filter::Mitchell,
            Filter::Lanczos3,
        ];

        for filter in filters {
            let resized = gif.clone().resize(50, 50, filter).unwrap();
            assert_eq!(resized.width, 50);
            assert_eq!(resized.height, 50);
        }
    }

    #[test]
    fn test_resize_upscale() {
        let frame = make_test_frame(50, 50);
        let gif = Gif {
            width: 50,
            height: 50,
            global_palette: None,
            frames: vec![frame],
            loop_count: crate::types::LoopCount::Infinite,
        };

        let resized = gif.resize(100, 100, Filter::Lanczos3).unwrap();
        assert_eq!(resized.width, 100);
        assert_eq!(resized.height, 100);
    }

    #[test]
    fn test_resize_fit_portrait_to_square_bounds() {
        let frame = make_test_frame(100, 200);
        let gif = Gif {
            width: 100,
            height: 200,
            global_palette: None,
            frames: vec![frame],
            loop_count: crate::types::LoopCount::Infinite,
        };

        let resized = gif.resize_fit(100, 100, Filter::Lanczos3).unwrap();
        assert_eq!(resized.width, 50);
        assert_eq!(resized.height, 100);
    }

    #[test]
    fn test_resize_fit_landscape_to_square_bounds() {
        let frame = make_test_frame(200, 100);
        let gif = Gif {
            width: 200,
            height: 100,
            global_palette: None,
            frames: vec![frame],
            loop_count: crate::types::LoopCount::Infinite,
        };

        let resized = gif.resize_fit(100, 100, Filter::Lanczos3).unwrap();
        assert_eq!(resized.width, 100);
        assert_eq!(resized.height, 50);
    }
}

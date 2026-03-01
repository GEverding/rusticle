//! Image crate compatibility (requires "image" feature).
//!
//! Zero-cost conversions between rusticle types and `image::RgbaImage`.

use std::time::Duration;

use image::RgbaImage;

use crate::error::Error;
use crate::types::{DisposalMethod, Frame, Gif, LoopCount};

impl TryFrom<Frame> for RgbaImage {
    type Error = Error;

    /// Convert a [`Frame`] into an [`RgbaImage`], consuming the frame.
    ///
    /// Moves pixel data without cloning.
    ///
    /// # Errors
    /// Returns [`Error::InvalidDimensions`] if the pixel buffer size does not match
    /// the declared frame dimensions.
    fn try_from(frame: Frame) -> crate::Result<Self> {
        RgbaImage::from_raw(frame.width as u32, frame.height as u32, frame.pixels)
            .ok_or_else(|| Error::InvalidDimensions("pixel buffer size mismatch".into()))
    }
}

impl TryFrom<&Frame> for RgbaImage {
    type Error = Error;

    /// Convert a [`Frame`] reference into an [`RgbaImage`], cloning pixel data.
    ///
    /// # Errors
    /// Returns [`Error::InvalidDimensions`] if the pixel buffer size does not match
    /// the declared frame dimensions.
    fn try_from(frame: &Frame) -> crate::Result<Self> {
        RgbaImage::from_raw(
            frame.width as u32,
            frame.height as u32,
            frame.pixels.clone(),
        )
        .ok_or_else(|| Error::InvalidDimensions("pixel buffer size mismatch".into()))
    }
}

impl TryFrom<RgbaImage> for Frame {
    type Error = Error;

    /// Convert an [`RgbaImage`] into a [`Frame`], consuming the image.
    ///
    /// Moves pixel data without cloning. Sets a default delay of 100ms and
    /// [`DisposalMethod::None`].
    ///
    /// # Errors
    /// Returns [`Error::InvalidDimensions`] if either dimension exceeds `u16::MAX`.
    fn try_from(img: RgbaImage) -> crate::Result<Self> {
        let (width, height) = img.dimensions();
        if width > u16::MAX as u32 || height > u16::MAX as u32 {
            return Err(Error::InvalidDimensions(
                "dimensions exceed u16::MAX".into(),
            ));
        }
        Ok(Frame {
            pixels: img.into_raw(),
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::None,
            local_palette: None,
            left: 0,
            top: 0,
            width: width as u16,
            height: height as u16,
        })
    }
}

impl Gif {
    /// Build a [`Gif`] from a sequence of [`RgbaImage`] frames.
    ///
    /// All images must have the same dimensions. The first image's dimensions
    /// set the canvas size.
    ///
    /// # Errors
    /// Returns [`Error::InvalidDimensions`] if images are empty, dimensions exceed `u16::MAX`,
    /// or images have inconsistent dimensions.
    ///
    /// # Example
    /// ```ignore
    /// use image::RgbaImage;
    /// use rusticle::Gif;
    /// use std::time::Duration;
    ///
    /// let frames: Vec<RgbaImage> = generate_frames();
    /// let delay = Duration::from_millis(100);
    /// let gif = Gif::from_rgba_images(frames, delay)?;
    /// let bytes = gif.to_bytes()?;
    /// ```
    pub fn from_rgba_images(images: Vec<RgbaImage>, delay: Duration) -> crate::Result<Gif> {
        if images.is_empty() {
            return Err(Error::InvalidDimensions("images list is empty".into()));
        }

        let (w0, h0) = images[0].dimensions();
        if w0 > u16::MAX as u32 || h0 > u16::MAX as u32 {
            return Err(Error::InvalidDimensions(
                "dimensions exceed u16::MAX".into(),
            ));
        }
        let canvas_width = w0 as u16;
        let canvas_height = h0 as u16;

        let frames = images
            .into_iter()
            .map(|img| {
                if img.dimensions() != (w0, h0) {
                    return Err(Error::InvalidDimensions(
                        "inconsistent frame dimensions".into(),
                    ));
                }
                let mut frame = Frame::try_from(img)?;
                frame.delay = delay;
                Ok(frame)
            })
            .collect::<crate::Result<Vec<_>>>()?;

        Ok(Gif {
            width: canvas_width,
            height: canvas_height,
            global_palette: None,
            frames,
            loop_count: LoopCount::Infinite,
            original_palette: None,
        })
    }

    /// Extract all frames as [`RgbaImage`].
    ///
    /// Consumes the [`Gif`], moving pixel data without cloning.
    ///
    /// # Errors
    /// Returns [`Error::InvalidDimensions`] if any frame has an invalid pixel buffer size.
    ///
    /// # Example
    /// ```ignore
    /// let gif = Gif::from_bytes(&data)?;
    /// let images = gif.into_rgba_images()?;
    /// for img in &images {
    ///     img.save("frame.png").unwrap();
    /// }
    /// ```
    pub fn into_rgba_images(self) -> crate::Result<Vec<RgbaImage>> {
        self.frames.into_iter().map(RgbaImage::try_from).collect()
    }

    /// Extract all frames as [`RgbaImage`], cloning pixel data.
    ///
    /// # Errors
    /// Returns [`Error::InvalidDimensions`] if any frame has an invalid pixel buffer size.
    pub fn to_rgba_images(&self) -> crate::Result<Vec<RgbaImage>> {
        self.frames.iter().map(RgbaImage::try_from).collect()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use image::RgbaImage;

    use super::*;
    use crate::types::{DisposalMethod, Frame, Gif, LoopCount};

    // ── helpers ──────────────────────────────────────────────────────────────

    /// 2×2 RGBA pixel buffer: R, G, B, A for each of the four pixels.
    fn pixels_2x2() -> Vec<u8> {
        vec![
            255, 0, 0, 255, // top-left:     red
            0, 255, 0, 255, // top-right:    green
            0, 0, 255, 255, // bottom-left:  blue
            255, 255, 0, 255, // bottom-right: yellow
        ]
    }

    fn make_frame_2x2() -> Frame {
        Frame {
            pixels: pixels_2x2(),
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::None,
            local_palette: None,
            left: 0,
            top: 0,
            width: 2,
            height: 2,
        }
    }

    fn make_rgba_2x2() -> RgbaImage {
        RgbaImage::from_raw(2, 2, pixels_2x2()).expect("valid 2×2 image")
    }

    // ── 1. Frame → RgbaImage (consuming) ─────────────────────────────────────

    #[test]
    fn test_frame_to_rgba_image_dimensions_and_pixels() {
        let frame = make_frame_2x2();
        let img = RgbaImage::try_from(frame).expect("conversion should succeed");

        assert_eq!(img.dimensions(), (2, 2), "dimensions must match");
        assert_eq!(img.into_raw(), pixels_2x2(), "pixel data must match");
    }

    // ── 2. &Frame → RgbaImage (borrowing) ────────────────────────────────────

    #[test]
    fn test_frame_ref_to_rgba_image_does_not_consume() {
        let frame = make_frame_2x2();
        let img = RgbaImage::try_from(&frame).expect("conversion should succeed");

        // original frame still accessible
        assert_eq!(
            frame.width, 2,
            "frame must still be accessible after borrow"
        );
        assert_eq!(img.dimensions(), (2, 2));
        assert_eq!(img.into_raw(), pixels_2x2());
    }

    // ── 3. RgbaImage → Frame ──────────────────────────────────────────────────

    #[test]
    fn test_rgba_image_to_frame_defaults() {
        let img = make_rgba_2x2();
        let frame = Frame::try_from(img).expect("conversion should succeed");

        assert_eq!(frame.width, 2, "width must be 2");
        assert_eq!(frame.height, 2, "height must be 2");
        assert_eq!(frame.pixels, pixels_2x2(), "pixel data must match");
        assert_eq!(
            frame.delay,
            Duration::from_millis(100),
            "default delay must be 100ms"
        );
        assert_eq!(
            frame.dispose,
            DisposalMethod::None,
            "default disposal must be None"
        );
        assert_eq!(frame.left, 0, "default left offset must be 0");
        assert_eq!(frame.top, 0, "default top offset must be 0");
        assert!(frame.local_palette.is_none(), "local palette must be None");
    }

    // ── 4. Frame → RgbaImage → Frame round-trip ───────────────────────────────

    #[test]
    fn test_round_trip_frame_pixels_preserved() {
        let original = make_frame_2x2();
        let original_pixels = original.pixels.clone();

        let img = RgbaImage::try_from(original).expect("frame → image");
        let recovered = Frame::try_from(img).expect("image → frame");

        assert_eq!(
            recovered.pixels, original_pixels,
            "pixel data must survive round-trip"
        );
    }

    // ── 5. Oversized RgbaImage → Frame returns Err ────────────────────────────

    #[test]
    fn test_oversized_image_to_frame_returns_err() {
        let oversized_width = u16::MAX as u32 + 1; // 65536
        let pixel_count = oversized_width as usize * 4; // 1 row, 4 bytes/pixel
        let img = RgbaImage::from_raw(oversized_width, 1, vec![0u8; pixel_count])
            .expect("image crate should accept this allocation");

        let result = Frame::try_from(img);
        assert!(
            result.is_err(),
            "dimensions exceeding u16::MAX must return Err"
        );
        assert!(
            matches!(result.unwrap_err(), Error::InvalidDimensions(_)),
            "error must be InvalidDimensions"
        );
    }

    // ── 6. Frame with wrong pixel buffer → RgbaImage returns Err ─────────────

    #[test]
    fn test_invalid_frame_pixel_buffer_returns_err() {
        let frame = Frame {
            pixels: vec![0u8; 8], // 2×2 needs 16 bytes
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::None,
            local_palette: None,
            left: 0,
            top: 0,
            width: 2,
            height: 2,
        };

        let result = RgbaImage::try_from(frame);
        assert!(result.is_err(), "mismatched pixel buffer must return Err");
        assert!(
            matches!(result.unwrap_err(), Error::InvalidDimensions(_)),
            "error must be InvalidDimensions"
        );
    }

    // ── 7. Gif::from_rgba_images happy path ───────────────────────────────────

    #[test]
    fn test_gif_from_rgba_images_builds_correctly() {
        let delay = Duration::from_millis(50);
        let images: Vec<RgbaImage> = (0..3).map(|_| make_rgba_2x2()).collect();

        let gif = Gif::from_rgba_images(images, delay).expect("should build Gif");

        assert_eq!(gif.width, 2, "canvas width");
        assert_eq!(gif.height, 2, "canvas height");
        assert_eq!(gif.frames.len(), 3, "frame count");
        assert_eq!(gif.loop_count, LoopCount::Infinite, "loop count");
        for (i, frame) in gif.frames.iter().enumerate() {
            assert_eq!(frame.delay, delay, "frame {i} delay must be 50ms");
        }
    }

    // ── 8. Gif::from_rgba_images with empty vec returns Err ──────────────────

    #[test]
    fn test_gif_from_rgba_images_empty_returns_err() {
        let result = Gif::from_rgba_images(vec![], Duration::from_millis(100));
        assert!(result.is_err(), "empty image list must return Err");
    }

    // ── 9. Gif::from_rgba_images with inconsistent dimensions returns Err ─────

    #[test]
    fn test_gif_from_rgba_images_inconsistent_dimensions_returns_err() {
        let img_2x2 = make_rgba_2x2();
        let img_4x4 = RgbaImage::from_raw(4, 4, vec![0u8; 4 * 4 * 4]).expect("valid 4×4 image");

        let result = Gif::from_rgba_images(vec![img_2x2, img_4x4], Duration::from_millis(100));
        assert!(result.is_err(), "inconsistent dimensions must return Err");
    }

    // ── 10. Gif::into_rgba_images (consuming) ────────────────────────────────

    #[test]
    fn test_gif_into_rgba_images_extracts_frames() {
        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![make_frame_2x2(), make_frame_2x2()],
            loop_count: LoopCount::Infinite,
            original_palette: None,
        };

        let images = gif.into_rgba_images().expect("extraction should succeed");

        assert_eq!(images.len(), 2, "must extract 2 images");
        for (i, img) in images.iter().enumerate() {
            assert_eq!(img.dimensions(), (2, 2), "image {i} dimensions");
            assert_eq!(img.as_raw(), &pixels_2x2(), "image {i} pixel data");
        }
    }

    // ── 11. Gif::to_rgba_images (borrowing) ──────────────────────────────────

    #[test]
    fn test_gif_to_rgba_images_does_not_consume() {
        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![make_frame_2x2()],
            loop_count: LoopCount::Finite(3),
            original_palette: None,
        };

        let images = gif.to_rgba_images().expect("extraction should succeed");

        // gif still accessible
        assert_eq!(gif.frames.len(), 1, "gif must still be accessible");
        assert_eq!(images.len(), 1, "must extract 1 image");
        assert_eq!(images[0].dimensions(), (2, 2));
        assert_eq!(images[0].as_raw(), &pixels_2x2());
    }
}

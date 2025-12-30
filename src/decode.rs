use std::io::Read;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::types::{DisposalMethod, Frame, Gif, LoopCount, Palette};

impl Gif {
    /// Decode a GIF from a byte slice.
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        Self::from_read(data)
    }

    /// Decode a GIF from any Read implementation.
    pub fn from_read<R: Read>(reader: R) -> Result<Self> {
        let mut decoder = gif::DecodeOptions::new();
        decoder.set_color_output(gif::ColorOutput::RGBA);

        let mut reader = decoder
            .read_info(reader)
            .map_err(|e| Error::DecodeError(e.to_string()))?;

        let width = reader.width();
        let height = reader.height();

        // Extract global palette if present
        let global_palette = reader.global_palette().map(|palette| {
            let colors = palette
                .chunks_exact(3)
                .map(|chunk| [chunk[0], chunk[1], chunk[2]])
                .collect();
            Palette { colors }
        });

        // Default loop count is infinite
        let loop_count = LoopCount::Infinite;

        let mut frames = Vec::new();

        // Decode all frames
        while let Some(frame) = reader
            .read_next_frame()
            .map_err(|e| Error::DecodeError(e.to_string()))?
        {
            let frame_width = frame.width;
            let frame_height = frame.height;
            let left = frame.left;
            let top = frame.top;

            // The gif crate with ColorOutput::RGBA gives us RGBA directly
            let pixels = frame.buffer.to_vec();

            // Convert delay from 10ms units to Duration
            let delay = Duration::from_millis(frame.delay as u64 * 10);

            // Map disposal method
            let dispose = match frame.dispose {
                gif::DisposalMethod::Any => DisposalMethod::None,
                gif::DisposalMethod::Keep => DisposalMethod::Keep,
                gif::DisposalMethod::Background => DisposalMethod::Background,
                gif::DisposalMethod::Previous => DisposalMethod::Previous,
            };

            // Extract local palette if present
            let local_palette = frame.palette.as_ref().map(|palette| {
                let colors = palette
                    .chunks_exact(3)
                    .map(|chunk| [chunk[0], chunk[1], chunk[2]])
                    .collect();
                Palette { colors }
            });

            frames.push(Frame {
                pixels,
                delay,
                dispose,
                local_palette,
                left,
                top,
                width: frame_width,
                height: frame_height,
            });
        }

        Ok(Gif {
            width,
            height,
            global_palette,
            frames,
            loop_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_empty_bytes() {
        let result = Gif::from_bytes(&[]);
        assert!(result.is_err(), "Empty bytes should fail to decode");
    }

    #[test]
    fn test_decode_invalid_data() {
        let invalid_data = b"This is not a GIF file";
        let result = Gif::from_bytes(invalid_data);
        assert!(result.is_err(), "Invalid data should fail to decode");
    }

    #[test]
    fn test_decode_from_read_empty() {
        let data: &[u8] = &[];
        let result = Gif::from_read(data);
        assert!(result.is_err(), "Empty read should fail");
    }

    #[test]
    fn test_decode_from_read_invalid() {
        let data = b"Invalid GIF data";
        let result = Gif::from_read(&data[..]);
        assert!(result.is_err(), "Invalid read should fail");
    }
}

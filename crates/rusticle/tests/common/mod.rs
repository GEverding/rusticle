#![allow(dead_code)]

use rusticle::{DisposalMethod, Frame, Gif, LoopCount};
use std::time::Duration;

/// Create a simple test frame with a solid color.
pub fn make_frame(width: u16, height: u16, color: [u8; 4]) -> Frame {
    let mut pixels = Vec::new();
    for _ in 0..(width as usize * height as usize) {
        pixels.extend_from_slice(&color);
    }

    Frame {
        pixels,
        delay: Duration::from_millis(100),
        dispose: DisposalMethod::None,
        local_palette: None,
        left: 0,
        top: 0,
        width,
        height,
    }
}

/// Create a simple test GIF with solid color frames
pub fn create_test_gif(width: u16, height: u16, frame_count: usize) -> Gif {
    let mut frames = Vec::new();

    for i in 0..frame_count {
        let mut pixels = Vec::new();
        let color_val = ((i * 255) / frame_count.max(1)) as u8;

        for _ in 0..(width as usize * height as usize) {
            pixels.extend_from_slice(&[color_val, color_val, color_val, 255]);
        }

        frames.push(Frame {
            pixels,
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::None,
            local_palette: None,
            left: 0,
            top: 0,
            width,
            height,
        });
    }

    Gif {
        width,
        height,
        global_palette: None,
        frames,
        loop_count: LoopCount::Infinite,
        original_palette: None,
    }
}

/// Create a test GIF with a gradient pattern
pub fn create_gradient_gif(width: u16, height: u16, frame_count: usize) -> Gif {
    let mut frames = Vec::new();

    for frame_idx in 0..frame_count {
        let mut pixels = Vec::new();

        for y in 0..height {
            for x in 0..width {
                let r = ((x as f32 / width as f32) * 255.0) as u8;
                let g = ((y as f32 / height as f32) * 255.0) as u8;
                let b = ((frame_idx as f32 / frame_count.max(1) as f32) * 255.0) as u8;
                let a = 255u8;
                pixels.extend_from_slice(&[r, g, b, a]);
            }
        }

        frames.push(Frame {
            pixels,
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::None,
            local_palette: None,
            left: 0,
            top: 0,
            width,
            height,
        });
    }

    Gif {
        width,
        height,
        global_palette: None,
        frames,
        loop_count: LoopCount::Infinite,
        original_palette: None,
    }
}

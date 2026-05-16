//! Shared GIF helpers.
#![cfg_attr(not(feature = "research"), allow(dead_code))]

use crate::error::Result;

#[cfg(feature = "imagequant")]
use crate::error::Error;

#[cfg(feature = "imagequant")]
/// Derive a 256-color palette from RGBA pixels using imagequant.
#[cfg(feature = "imagequant")]
pub fn derive_palette_from_rgba(rgba_pixels: &[u8]) -> Result<Vec<u8>> {
    let rgba_data: Vec<imagequant::RGBA> = rgba_pixels
        .chunks_exact(4)
        .map(|chunk| imagequant::RGBA {
            r: chunk[0],
            g: chunk[1],
            b: chunk[2],
            a: chunk[3],
        })
        .collect();

    if rgba_data.is_empty() {
        return Ok(vec![]);
    }

    let mut attr = imagequant::Attributes::new();
    attr.set_max_colors(256)
        .map_err(|e| Error::EncodeError(format!("failed to set max colors: {}", e)))?;
    attr.set_quality(0, 100)
        .map_err(|e| Error::EncodeError(format!("failed to set quality: {}", e)))?;

    let width = rgba_data.len();
    let height = 1;
    let mut img = attr
        .new_image_borrowed(&rgba_data, width, height, 0.0)
        .map_err(|e| Error::EncodeError(format!("failed to create image: {}", e)))?;

    let mut result = attr
        .quantize(&mut img)
        .map_err(|e| Error::EncodeError(format!("failed to quantize: {}", e)))?;

    result
        .set_dithering_level(1.0)
        .map_err(|e| Error::EncodeError(format!("failed to set dithering: {}", e)))?;

    let (palette, _) = result
        .remapped(&mut img)
        .map_err(|e| Error::EncodeError(format!("failed to remap: {}", e)))?;

    let mut palette_rgb = Vec::with_capacity(palette.len() * 3);
    for color in palette {
        palette_rgb.push(color.r);
        palette_rgb.push(color.g);
        palette_rgb.push(color.b);
    }

    Ok(palette_rgb)
}

#[cfg(not(feature = "imagequant"))]
/// Derive a 256-color palette from RGBA pixels using Wu quantization.
pub fn derive_palette_from_rgba(rgba_pixels: &[u8]) -> Result<Vec<u8>> {
    if rgba_pixels.is_empty() {
        return Ok(vec![]);
    }

    Ok(crate::quantize::derive_palette(rgba_pixels))
}

/// Find transparent index and remap transparent pixels to it.
/// Prefers index 0 for transparency (GIF convention, better LZW compression).
#[allow(clippy::ptr_arg)]
pub fn find_transparent_index_and_remap(
    rgba_pixels: &[u8],
    indices: &mut [u8],
    palette: &mut Vec<u8>,
) -> Option<u8> {
    let has_transparent = rgba_pixels.chunks_exact(4).any(|p| p[3] < 128);

    if !has_transparent {
        return None;
    }

    let palette_len = palette.len() / 3;
    if palette_len == 0 {
        return None;
    }

    let mut opaque_usage = vec![0usize; palette_len];
    for (i, pixel) in rgba_pixels.chunks_exact(4).enumerate() {
        if i < indices.len() && pixel[3] >= 128 {
            opaque_usage[indices[i] as usize] += 1;
        }
    }

    let transparent_idx = if opaque_usage[0] == 0 {
        0
    } else if let Some(unused_offset) = opaque_usage.iter().skip(1).position(|&count| count == 0) {
        (unused_offset + 1) as u8
    } else {
        opaque_usage
            .iter()
            .enumerate()
            .min_by_key(|(_, &count)| count)
            .map(|(idx, _)| idx as u8)
            .unwrap_or(0)
    };

    for (i, pixel) in rgba_pixels.chunks_exact(4).enumerate() {
        if i < indices.len() && pixel[3] < 128 {
            indices[i] = transparent_idx;
        }
    }

    Some(transparent_idx)
}

#[cfg(feature = "research")]
use crate::adaptive_ir::BoundingBox;

#[cfg(feature = "research")]
/// Compute the exact changed bounding box between two canvases.
pub fn compute_changed_bbox(prev: &[u8], curr: &[u8], width: usize, height: usize) -> BoundingBox {
    debug_assert_eq!(prev.len(), curr.len());
    debug_assert_eq!(prev.len(), width.saturating_mul(height).saturating_mul(4));

    let mut min_x = width as u16;
    let mut min_y = height as u16;
    let mut max_x = 0u16;
    let mut max_y = 0u16;
    let mut found_change = false;

    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) * 4;
            if prev[idx..idx + 4] != curr[idx..idx + 4] {
                found_change = true;
                min_x = min_x.min(x as u16);
                min_y = min_y.min(y as u16);
                max_x = max_x.max((x + 1) as u16);
                max_y = max_y.max((y + 1) as u16);
            }
        }
    }

    if found_change {
        BoundingBox::new(min_x, min_y, max_x, max_y)
    } else {
        BoundingBox::new(0, 0, 0, 0)
    }
}

#[cfg(feature = "research")]
/// Extract a bounding-box region from a canvas.
pub fn extract_bbox_region(canvas: &[u8], width: usize, bbox: &BoundingBox) -> Vec<u8> {
    let bbox_width = bbox.width() as usize;
    let bbox_height = bbox.height() as usize;
    let mut pixels = Vec::with_capacity(bbox_width * bbox_height * 4);

    for y in 0..bbox_height {
        for x in 0..bbox_width {
            let canvas_x = bbox.left as usize + x;
            let canvas_y = bbox.top as usize + y;
            let idx = (canvas_y * width + canvas_x) * 4;
            pixels.extend_from_slice(&canvas[idx..idx + 4]);
        }
    }

    pixels
}

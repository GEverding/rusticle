use criterion::{criterion_group, criterion_main, Criterion};
use rusticle::{DisposalMethod, Filter, Frame, Gif, LoopCount, OptLevel};
use std::time::Duration;

fn create_test_gif(width: u16, height: u16, frame_count: usize) -> Gif {
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

fn bench_encode(c: &mut Criterion) {
    let gif = create_test_gif(200, 200, 3);
    c.bench_function("encode_200x200_3frames", |b| {
        b.iter(|| gif.to_bytes().unwrap())
    });
}

fn bench_resize_nearest(c: &mut Criterion) {
    let gif = create_test_gif(200, 200, 2);
    c.bench_function("resize_nearest_100x100", |b| {
        b.iter(|| gif.clone().resize(100, 100, Filter::Nearest).unwrap())
    });
}

fn bench_resize_bilinear(c: &mut Criterion) {
    let gif = create_test_gif(200, 200, 2);
    c.bench_function("resize_bilinear_100x100", |b| {
        b.iter(|| gif.clone().resize(100, 100, Filter::Bilinear).unwrap())
    });
}

fn bench_resize_mitchell(c: &mut Criterion) {
    let gif = create_test_gif(200, 200, 2);
    c.bench_function("resize_mitchell_100x100", |b| {
        b.iter(|| gif.clone().resize(100, 100, Filter::Mitchell).unwrap())
    });
}

fn bench_resize_lanczos3(c: &mut Criterion) {
    let gif = create_test_gif(200, 200, 2);
    c.bench_function("resize_lanczos3_100x100", |b| {
        b.iter(|| gif.clone().resize(100, 100, Filter::Lanczos3).unwrap())
    });
}

fn bench_resize_fit(c: &mut Criterion) {
    let gif = create_test_gif(200, 200, 2);
    c.bench_function("resize_fit_100x100", |b| {
        b.iter(|| gif.clone().resize_fit(100, 100, Filter::Lanczos3).unwrap())
    });
}

fn bench_optimize_o1(c: &mut Criterion) {
    let gif = create_test_gif(200, 200, 3);
    c.bench_function("optimize_o1", |b| {
        b.iter(|| gif.clone().optimize(OptLevel::O1))
    });
}

fn bench_optimize_o2(c: &mut Criterion) {
    let gif = create_test_gif(200, 200, 3);
    c.bench_function("optimize_o2", |b| {
        b.iter(|| gif.clone().optimize(OptLevel::O2))
    });
}

fn bench_optimize_o3(c: &mut Criterion) {
    let gif = create_test_gif(200, 200, 3);
    c.bench_function("optimize_o3", |b| {
        b.iter(|| gif.clone().optimize(OptLevel::O3))
    });
}

fn bench_lossy_20(c: &mut Criterion) {
    let gif = create_test_gif(200, 200, 3);
    c.bench_function("lossy_quality_20", |b| b.iter(|| gif.clone().lossy(20)));
}

fn bench_lossy_50(c: &mut Criterion) {
    let gif = create_test_gif(200, 200, 3);
    c.bench_function("lossy_quality_50", |b| b.iter(|| gif.clone().lossy(50)));
}

fn bench_lossy_80(c: &mut Criterion) {
    let gif = create_test_gif(200, 200, 3);
    c.bench_function("lossy_quality_80", |b| b.iter(|| gif.clone().lossy(80)));
}

fn bench_full_pipeline(c: &mut Criterion) {
    let gif = create_test_gif(200, 200, 3);
    c.bench_function("full_pipeline_resize_optimize_lossy", |b| {
        b.iter(|| {
            gif.clone()
                .resize(100, 100, Filter::Lanczos3)
                .unwrap()
                .optimize(OptLevel::O2)
                .lossy(80)
        })
    });
}

criterion_group!(
    benches,
    bench_encode,
    bench_resize_nearest,
    bench_resize_bilinear,
    bench_resize_mitchell,
    bench_resize_lanczos3,
    bench_resize_fit,
    bench_optimize_o1,
    bench_optimize_o2,
    bench_optimize_o3,
    bench_lossy_20,
    bench_lossy_50,
    bench_lossy_80,
    bench_full_pipeline,
);
criterion_main!(benches, simd_benches, quantize_benches);

// SIMD vs scalar benchmarks
use rusticle::simd_opt::{
    find_diff_bounding_box, find_diff_bounding_box_scalar, mark_unchanged_pixels_scalar,
    mark_unchanged_pixels_simd,
};

fn bench_simd_pixel_compare(c: &mut Criterion) {
    // 200x200 = 40000 pixels = 160000 bytes
    let size = 160_000;
    let prev: Vec<u8> = (0..size).map(|i| ((i * 7) % 256) as u8).collect();
    let curr_orig: Vec<u8> = (0..size).map(|i| ((i * 7 + 2) % 256) as u8).collect();

    c.bench_function("simd_pixel_compare_200x200", |b| {
        b.iter(|| {
            let mut curr = curr_orig.clone();
            mark_unchanged_pixels_simd(&mut curr, &prev, 5)
        })
    });
}

fn bench_scalar_pixel_compare(c: &mut Criterion) {
    let size = 160_000;
    let prev: Vec<u8> = (0..size).map(|i| ((i * 7) % 256) as u8).collect();
    let curr_orig: Vec<u8> = (0..size).map(|i| ((i * 7 + 2) % 256) as u8).collect();

    c.bench_function("scalar_pixel_compare_200x200", |b| {
        b.iter(|| {
            let mut curr = curr_orig.clone();
            mark_unchanged_pixels_scalar(&mut curr, &prev, 5)
        })
    });
}

fn bench_diff_bbox_small(c: &mut Criterion) {
    // 100x100 frame, small diff in center
    let width = 100;
    let height = 100;
    let size = width * height * 4;
    let prev: Vec<u8> = vec![128; size];
    let mut curr = prev.clone();
    // Change center 10x10 region
    for y in 45..55 {
        for x in 45..55 {
            let idx = (y * width + x) * 4;
            curr[idx] = 255;
        }
    }

    c.bench_function("diff_bbox_100x100_center", |b| {
        b.iter(|| find_diff_bounding_box(&prev, &curr, width, height, 0))
    });
}

fn bench_diff_bbox_medium(c: &mut Criterion) {
    // 320x240 frame
    let width = 320;
    let height = 240;
    let size = width * height * 4;
    let prev: Vec<u8> = vec![128; size];
    let mut curr = prev.clone();
    // Change bottom-right corner
    for y in 200..240 {
        for x in 280..320 {
            let idx = (y * width + x) * 4;
            curr[idx] = 255;
        }
    }

    c.bench_function("diff_bbox_320x240_corner", |b| {
        b.iter(|| find_diff_bounding_box(&prev, &curr, width, height, 0))
    });
}

fn bench_diff_bbox_large(c: &mut Criterion) {
    // 640x480 frame, diff in bottom-right corner (worst case - full scan)
    let width = 640;
    let height = 480;
    let size = width * height * 4;
    let prev: Vec<u8> = vec![128; size];
    let mut curr = prev.clone();

    // Change only bottom-right 10x10 region
    for y in (height - 10)..height {
        for x in (width - 10)..width {
            let idx = (y * width + x) * 4;
            curr[idx] = 255;
        }
    }

    c.bench_function("diff_bbox_640x480_bottom_right", |b| {
        b.iter(|| find_diff_bounding_box(&prev, &curr, width, height, 0))
    });
}

fn bench_diff_bbox_identical(c: &mut Criterion) {
    // 320x240 identical frames (best case - early exit)
    let width = 320;
    let height = 240;
    let size = width * height * 4;
    let prev: Vec<u8> = vec![128; size];
    let curr = prev.clone();

    c.bench_function("diff_bbox_320x240_identical", |b| {
        b.iter(|| find_diff_bounding_box(&prev, &curr, width, height, 0))
    });
}

fn bench_diff_bbox_simd_vs_scalar_small(c: &mut Criterion) {
    // 100x100 frame, diff in center
    let width = 100;
    let height = 100;
    let size = width * height * 4;
    let prev: Vec<u8> = vec![128; size];
    let mut curr = prev.clone();
    // Change center 10x10 region
    for y in 45..55 {
        for x in 45..55 {
            let idx = (y * width + x) * 4;
            curr[idx] = 255;
        }
    }

    c.bench_function("diff_bbox_100x100_simd", |b| {
        b.iter(|| find_diff_bounding_box(&prev, &curr, width, height, 0))
    });

    c.bench_function("diff_bbox_100x100_scalar", |b| {
        b.iter(|| find_diff_bounding_box_scalar(&prev, &curr, width, height, 0))
    });
}

fn bench_diff_bbox_simd_vs_scalar_medium(c: &mut Criterion) {
    // 320x240 frame, diff in bottom-right corner
    let width = 320;
    let height = 240;
    let size = width * height * 4;
    let prev: Vec<u8> = vec![128; size];
    let mut curr = prev.clone();

    for y in (height - 40)..height {
        for x in (width - 40)..width {
            let idx = (y * width + x) * 4;
            curr[idx] = 255;
        }
    }

    c.bench_function("diff_bbox_320x240_simd", |b| {
        b.iter(|| find_diff_bounding_box(&prev, &curr, width, height, 0))
    });

    c.bench_function("diff_bbox_320x240_scalar", |b| {
        b.iter(|| find_diff_bounding_box_scalar(&prev, &curr, width, height, 0))
    });
}

fn bench_diff_bbox_simd_vs_scalar_large(c: &mut Criterion) {
    // 640x480 frame, diff in bottom-right corner (worst case - full scan)
    let width = 640;
    let height = 480;
    let size = width * height * 4;
    let prev: Vec<u8> = vec![128; size];
    let mut curr = prev.clone();

    // Change only bottom-right 10x10 region
    for y in (height - 10)..height {
        for x in (width - 10)..width {
            let idx = (y * width + x) * 4;
            curr[idx] = 255;
        }
    }

    c.bench_function("diff_bbox_640x480_simd", |b| {
        b.iter(|| find_diff_bounding_box(&prev, &curr, width, height, 0))
    });

    c.bench_function("diff_bbox_640x480_scalar", |b| {
        b.iter(|| find_diff_bounding_box_scalar(&prev, &curr, width, height, 0))
    });
}

criterion_group!(
    simd_benches,
    bench_simd_pixel_compare,
    bench_scalar_pixel_compare,
    bench_diff_bbox_small,
    bench_diff_bbox_medium,
    bench_diff_bbox_large,
    bench_diff_bbox_identical,
    bench_diff_bbox_simd_vs_scalar_small,
    bench_diff_bbox_simd_vs_scalar_medium,
    bench_diff_bbox_simd_vs_scalar_large,
);

// ============================================================================
// Quantization benchmarks: exoquant vs imagequant
// ============================================================================

use exoquant::{convert_to_indexed, ditherer, optimizer, Color as ExoColor};

/// Create realistic test image with gradients and varied colors
fn create_test_image(width: usize, height: usize) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            // Create gradient with some variation
            let r = ((x * 255) / width) as u8;
            let g = ((y * 255) / height) as u8;
            let b = (((x + y) * 127) / (width + height)) as u8;
            pixels.extend_from_slice(&[r, g, b, 255]);
        }
    }
    pixels
}

/// Create photographic-like test image with noise and color clusters
fn create_photo_like_image(width: usize, height: usize) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            // Multiple color regions like a photo
            let region_x = x * 4 / width;
            let region_y = y * 4 / height;
            let base = match (region_x + region_y * 4) % 8 {
                0 => [180, 60, 60],   // Red-ish
                1 => [60, 180, 60],   // Green-ish
                2 => [60, 60, 180],   // Blue-ish
                3 => [180, 180, 60],  // Yellow-ish
                4 => [180, 60, 180],  // Magenta-ish
                5 => [60, 180, 180],  // Cyan-ish
                6 => [120, 120, 120], // Gray
                _ => [200, 150, 100], // Skin tone-ish
            };
            // Add some variation within region
            let noise = ((x * 7 + y * 13) % 30) as i16 - 15;
            let r = (base[0] as i16 + noise).clamp(0, 255) as u8;
            let g = (base[1] as i16 + noise).clamp(0, 255) as u8;
            let b = (base[2] as i16 + noise).clamp(0, 255) as u8;
            pixels.extend_from_slice(&[r, g, b, 255]);
        }
    }
    pixels
}

fn bench_imagequant_200x200(c: &mut Criterion) {
    let width = 200;
    let height = 200;
    let pixels = create_test_image(width, height);

    c.bench_function("quantize_imagequant_200x200", |b| {
        b.iter(|| {
            let rgba_data: Vec<imagequant::RGBA> = pixels
                .chunks_exact(4)
                .map(|chunk| imagequant::RGBA {
                    r: chunk[0],
                    g: chunk[1],
                    b: chunk[2],
                    a: chunk[3],
                })
                .collect();

            let mut attr = imagequant::Attributes::new();
            attr.set_max_colors(256).unwrap();
            attr.set_quality(0, 100).unwrap();

            let mut img = attr
                .new_image_borrowed(&rgba_data, width, height, 0.0)
                .unwrap();
            let mut result = attr.quantize(&mut img).unwrap();
            result.set_dithering_level(1.0).unwrap();
            let (_palette, _indices) = result.remapped(&mut img).unwrap();
        })
    });
}

fn bench_exoquant_200x200(c: &mut Criterion) {
    let width = 200;
    let height = 200;
    let pixels = create_test_image(width, height);

    c.bench_function("quantize_exoquant_200x200", |b| {
        b.iter(|| {
            let exo_pixels: Vec<ExoColor> = pixels
                .chunks_exact(4)
                .map(|chunk| ExoColor::new(chunk[0], chunk[1], chunk[2], chunk[3]))
                .collect();

            let (_palette, _indexed) = convert_to_indexed(
                &exo_pixels,
                width,
                256,
                &optimizer::KMeans,
                &ditherer::FloydSteinberg::new(),
            );
        })
    });
}

fn bench_imagequant_400x400(c: &mut Criterion) {
    let width = 400;
    let height = 400;
    let pixels = create_photo_like_image(width, height);

    c.bench_function("quantize_imagequant_400x400", |b| {
        b.iter(|| {
            let rgba_data: Vec<imagequant::RGBA> = pixels
                .chunks_exact(4)
                .map(|chunk| imagequant::RGBA {
                    r: chunk[0],
                    g: chunk[1],
                    b: chunk[2],
                    a: chunk[3],
                })
                .collect();

            let mut attr = imagequant::Attributes::new();
            attr.set_max_colors(256).unwrap();
            attr.set_quality(0, 100).unwrap();

            let mut img = attr
                .new_image_borrowed(&rgba_data, width, height, 0.0)
                .unwrap();
            let mut result = attr.quantize(&mut img).unwrap();
            result.set_dithering_level(1.0).unwrap();
            let (_palette, _indices) = result.remapped(&mut img).unwrap();
        })
    });
}

fn bench_exoquant_400x400(c: &mut Criterion) {
    let width = 400;
    let height = 400;
    let pixels = create_photo_like_image(width, height);

    c.bench_function("quantize_exoquant_400x400", |b| {
        b.iter(|| {
            let exo_pixels: Vec<ExoColor> = pixels
                .chunks_exact(4)
                .map(|chunk| ExoColor::new(chunk[0], chunk[1], chunk[2], chunk[3]))
                .collect();

            let (_palette, _indexed) = convert_to_indexed(
                &exo_pixels,
                width,
                256,
                &optimizer::KMeans,
                &ditherer::FloydSteinberg::new(),
            );
        })
    });
}

// Also benchmark exoquant without k-means optimization (faster but lower quality)
fn bench_exoquant_no_kmeans_400x400(c: &mut Criterion) {
    let width = 400;
    let height = 400;
    let pixels = create_photo_like_image(width, height);

    c.bench_function("quantize_exoquant_no_kmeans_400x400", |b| {
        b.iter(|| {
            let exo_pixels: Vec<ExoColor> = pixels
                .chunks_exact(4)
                .map(|chunk| ExoColor::new(chunk[0], chunk[1], chunk[2], chunk[3]))
                .collect();

            let (_palette, _indexed) = convert_to_indexed(
                &exo_pixels,
                width,
                256,
                &optimizer::None, // No k-means optimization
                &ditherer::FloydSteinberg::new(),
            );
        })
    });
}

// Benchmark with ordered dithering (faster than Floyd-Steinberg)
fn bench_exoquant_ordered_dither_400x400(c: &mut Criterion) {
    let width = 400;
    let height = 400;
    let pixels = create_photo_like_image(width, height);

    c.bench_function("quantize_exoquant_ordered_400x400", |b| {
        b.iter(|| {
            let exo_pixels: Vec<ExoColor> = pixels
                .chunks_exact(4)
                .map(|chunk| ExoColor::new(chunk[0], chunk[1], chunk[2], chunk[3]))
                .collect();

            let (_palette, _indexed) = convert_to_indexed(
                &exo_pixels,
                width,
                256,
                &optimizer::KMeans,
                &ditherer::Ordered,
            );
        })
    });
}

// Benchmark imagequant with different speed settings (1=slowest/best, 10=fastest)
fn bench_imagequant_speed_1(c: &mut Criterion) {
    let width = 400;
    let height = 400;
    let pixels = create_photo_like_image(width, height);

    c.bench_function("quantize_imagequant_speed_1", |b| {
        b.iter(|| {
            let rgba_data: Vec<imagequant::RGBA> = pixels
                .chunks_exact(4)
                .map(|chunk| imagequant::RGBA {
                    r: chunk[0],
                    g: chunk[1],
                    b: chunk[2],
                    a: chunk[3],
                })
                .collect();

            let mut attr = imagequant::Attributes::new();
            attr.set_max_colors(256).unwrap();
            attr.set_speed(1).unwrap(); // Slowest, best quality
            attr.set_quality(0, 100).unwrap();

            let mut img = attr
                .new_image_borrowed(&rgba_data, width, height, 0.0)
                .unwrap();
            let mut result = attr.quantize(&mut img).unwrap();
            result.set_dithering_level(1.0).unwrap();
            let (_palette, _indices) = result.remapped(&mut img).unwrap();
        })
    });
}

fn bench_imagequant_speed_3(c: &mut Criterion) {
    let width = 400;
    let height = 400;
    let pixels = create_photo_like_image(width, height);

    c.bench_function("quantize_imagequant_speed_3_default", |b| {
        b.iter(|| {
            let rgba_data: Vec<imagequant::RGBA> = pixels
                .chunks_exact(4)
                .map(|chunk| imagequant::RGBA {
                    r: chunk[0],
                    g: chunk[1],
                    b: chunk[2],
                    a: chunk[3],
                })
                .collect();

            let mut attr = imagequant::Attributes::new();
            attr.set_max_colors(256).unwrap();
            attr.set_speed(3).unwrap(); // Default
            attr.set_quality(0, 100).unwrap();

            let mut img = attr
                .new_image_borrowed(&rgba_data, width, height, 0.0)
                .unwrap();
            let mut result = attr.quantize(&mut img).unwrap();
            result.set_dithering_level(1.0).unwrap();
            let (_palette, _indices) = result.remapped(&mut img).unwrap();
        })
    });
}

fn bench_imagequant_speed_10(c: &mut Criterion) {
    let width = 400;
    let height = 400;
    let pixels = create_photo_like_image(width, height);

    c.bench_function("quantize_imagequant_speed_10", |b| {
        b.iter(|| {
            let rgba_data: Vec<imagequant::RGBA> = pixels
                .chunks_exact(4)
                .map(|chunk| imagequant::RGBA {
                    r: chunk[0],
                    g: chunk[1],
                    b: chunk[2],
                    a: chunk[3],
                })
                .collect();

            let mut attr = imagequant::Attributes::new();
            attr.set_max_colors(256).unwrap();
            attr.set_speed(10).unwrap(); // Fastest
            attr.set_quality(0, 100).unwrap();

            let mut img = attr
                .new_image_borrowed(&rgba_data, width, height, 0.0)
                .unwrap();
            let mut result = attr.quantize(&mut img).unwrap();
            result.set_dithering_level(1.0).unwrap();
            let (_palette, _indices) = result.remapped(&mut img).unwrap();
        })
    });
}

// Benchmark different quality ranges
fn bench_imagequant_quality_low(c: &mut Criterion) {
    let width = 400;
    let height = 400;
    let pixels = create_photo_like_image(width, height);

    c.bench_function("quantize_imagequant_quality_0_50", |b| {
        b.iter(|| {
            let rgba_data: Vec<imagequant::RGBA> = pixels
                .chunks_exact(4)
                .map(|chunk| imagequant::RGBA {
                    r: chunk[0],
                    g: chunk[1],
                    b: chunk[2],
                    a: chunk[3],
                })
                .collect();

            let mut attr = imagequant::Attributes::new();
            attr.set_max_colors(256).unwrap();
            attr.set_quality(0, 50).unwrap(); // Lower quality acceptable

            let mut img = attr
                .new_image_borrowed(&rgba_data, width, height, 0.0)
                .unwrap();
            let mut result = attr.quantize(&mut img).unwrap();
            result.set_dithering_level(1.0).unwrap();
            let (_palette, _indices) = result.remapped(&mut img).unwrap();
        })
    });
}

criterion_group!(
    quantize_benches,
    bench_imagequant_200x200,
    bench_exoquant_200x200,
    bench_imagequant_400x400,
    bench_exoquant_400x400,
    bench_exoquant_no_kmeans_400x400,
    bench_exoquant_ordered_dither_400x400,
    bench_imagequant_speed_1,
    bench_imagequant_speed_3,
    bench_imagequant_speed_10,
    bench_imagequant_quality_low,
);

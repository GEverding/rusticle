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
criterion_main!(benches, simd_benches);

// SIMD vs scalar benchmarks
use rusticle::simd_opt::{mark_unchanged_pixels_scalar, mark_unchanged_pixels_simd};

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

criterion_group!(
    simd_benches,
    bench_simd_pixel_compare,
    bench_scalar_pixel_compare,
);

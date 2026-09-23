//! Run: rustc -O --edition=2024 tools/readback-bench.rs -o /tmp/bro-readback-bench && /tmp/bro-readback-bench
//! Measures only CPU row flipping, not rendering, GL readback, or GPU upload.
#[path = "../servo/components/shared/paint/rendering_context/rows.rs"]
mod rows;
use std::{hint::black_box, time::Instant};

fn old_flip(pixels: &mut [u8], stride: usize) {
    let original = pixels.to_vec();
    let height = pixels.len() / stride;
    for y in 0..height {
        pixels[y * stride..(y + 1) * stride]
            .copy_from_slice(&original[(height - y - 1) * stride..(height - y) * stride]);
    }
}

fn main() {
    for (width, height) in [(1918, 1530), (3840, 2160)] {
        let mut pixels: Vec<u8> = (0..width * height * 4).map(|i| (i % 251) as u8).collect();
        let rounds = 200;
        for _ in 0..10 {
            rows::flip_rows(black_box(&mut pixels), width * 4);
        }
        let start = Instant::now();
        for _ in 0..rounds {
            old_flip(black_box(&mut pixels), width * 4);
        }
        let old = start.elapsed().as_secs_f64() * 1000.0 / rounds as f64;
        let start = Instant::now();
        for _ in 0..rounds {
            rows::flip_rows(black_box(&mut pixels), width * 4);
        }
        let new = start.elapsed().as_secs_f64() * 1000.0 / rounds as f64;
        println!(
            "{width}x{height}: clone+copy {old:.3} ms, in-place {new:.3} ms, {:.2}x; {} bytes/frame allocation removed",
            old / new,
            pixels.len()
        );
        black_box(pixels);
    }
}

use std::path::Path;

use anyhow::{Context, Result};
use image::imageops::FilterType;

pub fn load_dummy_yuyv(path: &Path, width: u32, height: u32) -> Result<Vec<u8>> {
    let img = image::open(path)
        .with_context(|| format!("failed to load dummy image: {}", path.display()))?;
    let resized = if img.width() != width || img.height() != height {
        img.resize_exact(width, height, FilterType::Lanczos3)
    } else {
        img
    };

    let rgb = resized.to_rgb8();
    let mut out = vec![0u8; (width as usize) * (height as usize) * 2];

    for y in 0..height {
        for x in (0..width).step_by(2) {
            let p0 = rgb.get_pixel(x, y);
            let p1 = rgb.get_pixel((x + 1).min(width - 1), y);

            let (y0, u0, v0) = rgb_to_yuv(p0[0], p0[1], p0[2]);
            let (y1, u1, v1) = rgb_to_yuv(p1[0], p1[1], p1[2]);
            let u = ((u0 as u16 + u1 as u16) / 2) as u8;
            let v = ((v0 as u16 + v1 as u16) / 2) as u8;

            let idx = ((y * width + x) as usize) * 2;
            out[idx] = y0;
            out[idx + 1] = u;
            out[idx + 2] = y1;
            out[idx + 3] = v;
        }
    }

    Ok(out)
}

fn rgb_to_yuv(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let r = r as f32;
    let g = g as f32;
    let b = b as f32;

    let y = (0.299 * r + 0.587 * g + 0.114 * b).round();
    let u = (-0.169 * r - 0.331 * g + 0.5 * b + 128.0).round();
    let v = (0.5 * r - 0.419 * g - 0.081 * b + 128.0).round();

    (clamp_u8(y), clamp_u8(u), clamp_u8(v))
}

fn clamp_u8(v: f32) -> u8 {
    v.clamp(0.0, 255.0) as u8
}

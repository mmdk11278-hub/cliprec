use std::io::Cursor;

use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder};

/// Renders the ClipRec brand icon as an RGBA8 buffer of `size x size` pixels.
///
/// Design: Rounded purple-gradient squircle (#5100A4 -> #7F54F8), crisp white
/// capture ring, and vivid recording red center dot (#FF3B30).
pub fn render_brand_icon_rgba(size: u32) -> Vec<u8> {
    let s = size.max(1) as f32;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let half = s * 0.5;
    let corner_r = s * 0.22;
    let box_half = half - s * 0.04;

    let ring_r = s * 0.29;
    let ring_thick = (s * 0.075).max(1.2);
    let dot_r = s * 0.135;

    for y in 0..size {
        for x in 0..size {
            let px = x as f32 + 0.5 - half;
            let py = y as f32 + 0.5 - half;

            // Signed distance to rounded squircle
            let qx = px.abs() - (box_half - corner_r);
            let qy = py.abs() - (box_half - corner_r);
            let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0)
                - corner_r;

            let bg_alpha = (0.5 - outside).clamp(0.0, 1.0);
            if bg_alpha <= 0.0 {
                continue;
            }

            // Diagonal purple gradient (#5100A4 -> #7F54F8)
            let t = ((x as f32 / s) * 0.55 + (y as f32 / s) * 0.45).clamp(0.0, 1.0);
            let mut r = 81.0 + (127.0 - 81.0) * t;
            let mut g = 0.0 + 84.0 * t;
            let mut b = 164.0 + (248.0 - 164.0) * t;

            // Subtle inner rim highlight
            if outside > -s * 0.035 {
                r = (r + 28.0).min(255.0);
                g = (g + 24.0).min(255.0);
                b = (b + 36.0).min(255.0);
            }

            let dist = (px * px + py * py).sqrt();

            // White outer capture ring
            let ring_dist = (dist - ring_r).abs() - ring_thick * 0.5;
            let ring_alpha = (0.5 - ring_dist).clamp(0.0, 1.0);
            if ring_alpha > 0.0 {
                r = r * (1.0 - ring_alpha) + 255.0 * ring_alpha;
                g = g * (1.0 - ring_alpha) + 255.0 * ring_alpha;
                b = b * (1.0 - ring_alpha) + 255.0 * ring_alpha;
            }

            // Vivid recording red center dot (#FF3B30)
            let dot_alpha = (0.5 - (dist - dot_r)).clamp(0.0, 1.0);
            if dot_alpha > 0.0 {
                r = r * (1.0 - dot_alpha) + 255.0 * dot_alpha;
                g = g * (1.0 - dot_alpha) + 59.0 * dot_alpha;
                b = b * (1.0 - dot_alpha) + 48.0 * dot_alpha;
            }

            let idx = ((y * size + x) * 4) as usize;
            rgba[idx] = r.round().clamp(0.0, 255.0) as u8;
            rgba[idx + 1] = g.round().clamp(0.0, 255.0) as u8;
            rgba[idx + 2] = b.round().clamp(0.0, 255.0) as u8;
            rgba[idx + 3] = (bg_alpha * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    rgba
}

/// Returns a 64x64 `egui::IconData` for runtime window & taskbar icons.
pub fn app_icon_data() -> egui::IconData {
    egui::IconData {
        rgba: render_brand_icon_rgba(64),
        width: 64,
        height: 64,
    }
}

/// Encodes a multi-resolution `.ico` file (256, 64, 48, 32, 16) using PNG frames.
pub fn generate_ico_bytes() -> Vec<u8> {
    let sizes = [256u32, 64, 48, 32, 16];
    let mut pngs: Vec<(u32, Vec<u8>)> = Vec::with_capacity(sizes.len());
    for &sz in &sizes {
        let rgba = render_brand_icon_rgba(sz);
        let mut buf = Cursor::new(Vec::new());
        let enc = PngEncoder::new(&mut buf);
        let _ = enc.write_image(&rgba, sz, sz, ExtendedColorType::Rgba8);
        pngs.push((sz, buf.into_inner()));
    }

    let count = pngs.len() as u16;
    let mut out = Vec::new();
    // ICONDIR header: reserved=0, type=1 (icon), count
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());

    let mut offset = 6u32 + (count as u32) * 16;
    for (sz, data) in &pngs {
        let dim = if *sz >= 256 { 0u8 } else { *sz as u8 };
        out.push(dim); // bWidth
        out.push(dim); // bHeight
        out.push(0); // bColorCount
        out.push(0); // bReserved
        out.extend_from_slice(&1u16.to_le_bytes()); // wPlanes
        out.extend_from_slice(&32u16.to_le_bytes()); // wBitCount
        out.extend_from_slice(&(data.len() as u32).to_le_bytes()); // dwBytesInRes
        out.extend_from_slice(&offset.to_le_bytes()); // dwImageOffset
        offset += data.len() as u32;
    }
    for (_, data) in pngs {
        out.extend_from_slice(&data);
    }
    out
}

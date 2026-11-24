// ============================================================================
// Video Editor v1 — trim / cut / reorder / merge / export to MP4 (Media Foundation)
// ============================================================================
use std::path::{Path, PathBuf};

use image::RgbaImage;

use crate::capture::{decode_jpeg_to_rgba, read_mjpeg_avi};
use crate::mf::{Mp4Reader, Mp4Writer};
use crate::recorder::bitrate_for_height;

/// One timeline segment taken from a source video file.
#[derive(Clone, Debug)]
pub struct ClipDef {
    pub source: PathBuf,
    pub label: String,
    pub start_sec: f64,
    pub end_sec: f64,
    /// Full source duration in seconds (used by the UI sliders).
    pub total_sec: f64,
}

impl ClipDef {
    pub fn new(source: PathBuf, label: String, total_sec: f64) -> Self {
        let total = total_sec.max(0.0);
        Self {
            source,
            label,
            start_sec: 0.0,
            end_sec: total,
            total_sec: total,
        }
    }

    pub fn duration_sec(&self) -> f64 {
        (self.end_sec - self.start_sec).max(0.0)
    }
}

/// Media properties of a source file: (width, height, fps, audio_rate, audio_channels, duration_sec).
pub fn probe(path: &Path) -> Result<(u32, u32, u32, u32, u16, f64), String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    if ext == "mp4" {
        let mut r = Mp4Reader::new(path).map_err(|e| format!("Cannot read MP4: {e}"))?;
        // Draining audio gives the total audio length for an accurate duration.
        let dur = if r.audio_channels > 0 && r.audio_rate > 0 {
            r.audio_samples.len() as f64
                / (r.audio_rate as f64 * r.audio_channels as f64)
        } else {
            // No audio track: count frames (full decode, bounded).
            let mut n = 0u32;
            while n < 120_000 && r.next_video_frame().is_some() {
                n += 1;
            }
            n as f64 / r.fps.max(1) as f64
        };
        Ok((r.width.max(2), r.height.max(2), r.fps.max(1), r.audio_rate, r.audio_channels, dur))
    } else if let Some(v) = read_mjpeg_avi(path) {
        let dur = if v.fps > 0 {
            v.frames.len() as f64 / v.fps as f64
        } else {
            0.0
        };
        Ok((v.width.max(2), v.height.max(2), v.fps.max(1), v.audio_sample_rate, v.audio_channels, dur))
    } else {
        Err("Unsupported video file".into())
    }
}

/// Fully decoded clip content.
struct DecodedClip {
    frames: Vec<RgbaImage>,
    audio: Vec<i16>,
    rate: u32,
    channels: u16,
    fps: u32,
    width: u32,
    height: u32,
}

/// Decodes `clip` and keeps only the [start_sec, end_sec) range.
fn decode_clip(clip: &ClipDef) -> Result<DecodedClip, String> {
    let start_sec = clip.start_sec.max(0.0);
    let end_sec = clip.end_sec.max(start_sec);
    let ext = clip
        .source
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();

    if ext == "mp4" {
        let mut r = Mp4Reader::new(&clip.source).map_err(|e| format!("Cannot read {}: {e}", clip.label))?;
        let fps = r.fps.max(1);
        let f0 = (start_sec * fps as f64).round() as usize;
        let f1 = (end_sec * fps as f64).round() as usize;
        let mut frames: Vec<RgbaImage> = Vec::new();
        let mut idx = 0usize;
        while let Some(frame) = r.next_video_frame() {
            if idx >= f1 {
                break;
            }
            if idx >= f0 {
                frames.push(frame);
            }
            idx += 1;
        }
        let (rate, ch) = (r.audio_rate, r.audio_channels);
        let audio = if rate > 0 && ch > 0 {
            let s0 = (start_sec * rate as f64).round() as usize * ch as usize;
            let s1 = (end_sec * rate as f64).round() as usize * ch as usize;
            r.audio_samples
                .get(s0..s1.min(r.audio_samples.len()))
                .unwrap_or_default()
                .to_vec()
        } else {
            Vec::new()
        };
        Ok(DecodedClip {
            frames,
            audio,
            rate,
            channels: ch,
            fps,
            width: r.width.max(2),
            height: r.height.max(2),
        })
    } else if let Some(v) = read_mjpeg_avi(&clip.source) {
        let fps = v.fps.max(1);
        let f0 = (start_sec * fps as f64).round() as usize;
        let f1 = (end_sec * fps as f64).round() as usize;
        let mut frames: Vec<RgbaImage> = Vec::new();
        for (i, jpeg) in v.frames.iter().enumerate() {
            if i >= f1 {
                break;
            }
            if i >= f0 {
                if let Some(img) = decode_jpeg_to_rgba(jpeg) {
                    frames.push(img);
                }
            }
        }
        let (rate, ch) = (v.audio_sample_rate, v.audio_channels);
        let audio = if rate > 0 && ch > 0 {
            let s0 = (start_sec * rate as f64).round() as usize * ch as usize;
            let s1 = (end_sec * rate as f64).round() as usize * ch as usize;
            v.audio_samples
                .get(s0..s1.min(v.audio_samples.len()))
                .unwrap_or_default()
                .to_vec()
        } else {
            Vec::new()
        };
        Ok(DecodedClip {
            frames,
            audio,
            rate,
            channels: ch,
            fps,
            width: v.width.max(2),
            height: v.height.max(2),
        })
    } else {
        Err(format!("Unsupported video file: {}", clip.label))
    }
}

/// Linear resampler: i16 -> i16, changing rate and/or channel count.
fn resample_i16(src: &[i16], src_rate: u32, src_ch: u16, dst_rate: u32, dst_ch: u16) -> Vec<i16> {
    if src.is_empty() {
        return Vec::new();
    }
    let src_rate = src_rate.max(1) as f64;
    let dst_rate = dst_rate.max(1) as f64;
    let src_ch = src_ch.max(1) as usize;
    let dst_ch = dst_ch.max(1) as usize;
    let frames = src.len() / src_ch;
    let out_frames = (frames as f64 * dst_rate / src_rate) as usize;
    let mut out = Vec::with_capacity(out_frames * dst_ch);
    for of in 0..out_frames {
        let pos = of as f64 * src_rate / dst_rate;
        let i = pos.floor() as usize;
        let frac = pos - i as f64;
        let i = i.min(frames.saturating_sub(1));
        let j = (i + 1).min(frames.saturating_sub(1));
        for dc in 0..dst_ch {
            let sc = match (dc, src_ch) {
                (0, 2) | (0, 1) => 0,
                (1, 2) => 1,
                _ => dc % src_ch,
            };
            let a = src[i * src_ch + sc] as f32 / 32768.0;
            let b = src[j * src_ch + sc] as f32 / 32768.0;
            let v = a * (1.0 - frac as f32) + b * frac as f32;
            out.push((v.clamp(-1.0, 1.0) * 32767.0) as i16);
        }
    }
    out
}

/// Re-encodes the timeline to a single MP4 at `out` via Media Foundation.
/// `base_w/base_h/fps` are the output geometry; frames are scaled to fit.
/// Audio comes from the first clip that has a track, resampled per clip.
/// `progress` receives 0.0..=1.0 as frames are written.
pub fn build_mp4(
    clips: &[ClipDef],
    out: &Path,
    base_w: u32,
    base_h: u32,
    base_fps: u32,
    progress: impl FnMut(f32),
) -> Result<(), String> {
    // Pick audio configuration from the first clip that has a track.
    let mut audio_rate = 0u32;
    let mut audio_channels = 0u16;
    for c in clips {
        if let Ok(p) = probe(&c.source) {
            if p.3 > 0 && p.4 > 0 {
                audio_rate = p.3;
                audio_channels = p.4;
                break;
            }
        }
    }

    let w = crate::mf::even_dim(base_w).max(2);
    let h = crate::mf::even_dim(base_h).max(2);
    let fps = base_fps.clamp(1, 60);
    let bitrate = bitrate_for_height(h);

    let mut writer = Mp4Writer::new(out, w, h, fps, bitrate, audio_rate, audio_channels)
        .map_err(|e| format!("MP4 writer failed: {e}"))?;

    let total_frames_hint: usize = clips
        .iter()
        .map(|c| (c.duration_sec() * fps as f64) as usize)
        .sum::<usize>()
        .max(1);
    let mut progress = progress;
    let mut done_frames = 0usize;
    let mut push = |done: usize| {
        progress((done as f32 / total_frames_hint as f32).clamp(0.0, 1.0));
    };

    for clip in clips {
        let decoded = decode_clip(clip)?;
        if decoded.frames.is_empty() {
            continue;
        }
        let dart = resample_i16(
            &decoded.audio,
            decoded.rate.max(1),
            decoded.channels,
            audio_rate.max(1),
            audio_channels.max(1),
        );
        let per_frame = if audio_rate > 0 && audio_channels > 0 {
            (audio_rate as usize / fps as usize).max(1)
        } else {
            0
        };
        let ch = audio_channels.max(1) as usize;
        let scale = |frame: &RgbaImage| -> RgbaImage {
            if frame.width() == w && frame.height() == h {
                frame.clone()
            } else {
                image::imageops::resize(frame, w, h, image::imageops::FilterType::Triangle)
            }
        };
        for (fi, frame) in decoded.frames.iter().enumerate() {
            let scaled = scale(frame);
            writer
                .write_video_frame(&scaled)
                .map_err(|e| format!("Video encode failed: {e}"))?;
            if per_frame > 0 {
                let a0 = fi * per_frame * ch;
                let a1 = ((fi + 1) * per_frame * ch).min(dart.len());
                if a1 > a0 {
                    writer
                        .write_audio(&dart[a0..a1])
                        .map_err(|e| format!("Audio encode failed: {e}"))?;
                }
            }
            done_frames += 1;
            push(done_frames);
        }
    }
    push(total_frames_hint);
    writer.finish().map_err(|e| format!("MP4 finalize failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_preserves_frame_count_ratio() {
        let src: Vec<i16> = (0..4800i16).collect();
        let out = resample_i16(&src, 48_000, 2, 24_000, 2);
        assert_eq!(out.len(), 2400);
        let mono = resample_i16(&src, 48_000, 2, 48_000, 1);
        assert_eq!(mono.len(), 2400);
    }

    #[test]
    fn clip_duration_clamps() {
        let mut c = ClipDef::new(PathBuf::from("x.mp4"), "x".into(), 10.0);
        c.start_sec = 12.0;
        c.end_sec = 9.0;
        assert_eq!(c.duration_sec(), 0.0);
        assert_eq!(c.total_sec, 10.0);
    }
}

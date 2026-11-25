//! Minimal Windows Media Foundation bindings: record/list MP4 (H.264 + AAC)
//! without any external FFmpeg dependency.
//!
//! - `Mp4Writer`  : `SinkWriter` that writes `.mp4` while recording / exporting.
//! - `Mp4Reader`  : `SourceReader` that decodes video frames (RGB) + PCM audio.
//! - `h264_encoder_available` : probe for the built-in H.264 encoder MFT.

use std::path::Path;
use std::sync::OnceLock;

use image::RgbaImage;
use windows::core::PCWSTR;
use windows::Win32::Graphics::Dxgi::DXGI_ERROR_ACCESS_LOST;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, COINIT, COINIT_MULTITHREADED};

const MF_VERSION: u32 = 0x0002_0070;

fn hres(err: impl std::fmt::Debug) -> String {
    format!("Media Foundation error: {err:?}")
}

/// One-time `CoInitializeEx(MTA)` + `MFStartup`.
fn ensure_mf() -> Result<(), String> {
    static ONCE: OnceLock<Result<(), String>> = OnceLock::new();
    ONCE.get_or_init(|| {
        unsafe {
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            // RPC_E_CHANGED_MODE means the thread is already STA — still usable.
            if hr.is_err() && hr.0 as u32 != 0x8001_0106 {
                return Err(format!("COM init failed: {hr:?}"));
            }
            MFStartup(MF_VERSION, 0).map_err(|e| format!("MFStartup failed: {e}"))?;
        }
        Ok(())
    })
    .clone()
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn pack64(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

pub fn even_dim(v: u32) -> u32 {
    let v = v.max(2);
    v - (v % 2)
}

/// Converts an RGBA frame into an NV12 byte buffer (even dimensions required).
///
/// Single-pass `i32` fixed-point BT.601 conversion over 2×2 pixel blocks —
/// zero floating-point math.
pub fn rgba_to_nv12(img: &RgbaImage) -> Vec<u8> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let w = (w.max(2) / 2) * 2;
    let h = (h.max(2) / 2) * 2;
    let raw = img.as_raw();
    let mut out = vec![0u8; w * h + w * h / 2];
    let uv_off = w * h;
    let (ww, hh) = (w / 2, h / 2);
    for yy in 0..hh {
        let src = (yy * 2 * w) * 4;
        let dst = yy * 2 * w;
        for xx in 0..ww {
            let c = src + xx * 8;
            let (r0, g0, b0) = (raw[c], raw[c + 1], raw[c + 2]);
            let (r1, g1, b1) = (raw[c + 4], raw[c + 5], raw[c + 6]);
            let (r2, g2, b2) = (raw[c + w * 4], raw[c + w * 4 + 1], raw[c + w * 4 + 2]);
            let (r3, g3, b3) = (raw[c + w * 4 + 4], raw[c + w * 4 + 5], raw[c + w * 4 + 6]);

            // BT.601 Y: ((66R + 129G + 25B + 128) >> 8) + 16
            let p = dst + xx * 2;
            out[p] = (((66 * r0 as i32 + 129 * g0 as i32 + 25 * b0 as i32 + 128) >> 8) + 16)
                as u8;
            out[p + 1] = (((66 * r1 as i32 + 129 * g1 as i32 + 25 * b1 as i32 + 128) >> 8) + 16)
                as u8;
            out[p + w] = (((66 * r2 as i32 + 129 * g2 as i32 + 25 * b2 as i32 + 128) >> 8) + 16)
                as u8;
            out[p + w + 1] = (((66 * r3 as i32 + 129 * g3 as i32 + 25 * b3 as i32 + 128) >> 8) + 16)
                as u8;

            // BT.601 U/V from the 2×2 block average.
            let r_avg = (r0 as i32 + r1 as i32 + r2 as i32 + r3 as i32) >> 2;
            let g_avg = (g0 as i32 + g1 as i32 + g2 as i32 + g3 as i32) >> 2;
            let b_avg = (b0 as i32 + b1 as i32 + b2 as i32 + b3 as i32) >> 2;
            let u = (((-38 * r_avg - 74 * g_avg + 112 * b_avg + 128) >> 8) + 128) as u8;
            let v = (((112 * r_avg - 94 * g_avg - 18 * b_avg + 128) >> 8) + 128) as u8;
            let o = uv_off + (yy * ww + xx) * 2;
            out[o] = u;
            out[o + 1] = v;
        }
    }
    out
}

/// Whether the built-in H.264 video encoder MFT is registered (Win10/11 ship it;
/// Windows N editions may need the Media Feature Pack).
pub fn h264_encoder_available() -> bool {
    if ensure_mf().is_err() {
        return false;
    }
    unsafe {
        let input = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Video,
            guidSubtype: MFVideoFormat_NV12,
        };
        let output = MFT_REGISTER_TYPE_INFO {
            guidMajorType: MFMediaType_Video,
            guidSubtype: MFVideoFormat_H264,
        };
        let mut acts: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0u32;
        let hr = MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_LOCALMFT | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(&input),
            Some(&output),
            &mut acts,
            &mut count,
        );
        let ok = hr.is_ok() && count > 0;
        if ok && !acts.is_null() {
            for i in 0..count {
                if let Some(act_opt) = acts.add(i as usize).as_ref() {
                    if let Some(act) = act_opt {
                        let _ = act.ShutdownObject();
                    }
                }
            }
            windows::Win32::System::Com::CoTaskMemFree(Some(acts as *const _));
        }
        ok
    }
}

fn make_sample(data: &[u8], pts_hns: i64, dur_hns: i64) -> windows::core::Result<IMFSample> {
    unsafe {
        let sample = MFCreateSample()?;
        let buffer = MFCreateMemoryBuffer(data.len().max(1) as u32)?;
        let mut ptr: *mut u8 = std::ptr::null_mut();
        buffer.Lock(&mut ptr, None, None)?;
        if !data.is_empty() {
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
        }
        buffer.Unlock()?;
        buffer.SetCurrentLength(data.len() as u32)?;
        sample.AddBuffer(&buffer)?;
        sample.SetSampleTime(pts_hns)?;
        sample.SetSampleDuration(dur_hns)?;
        Ok(sample)
    }
}

/// Media Foundation MP4 sink writer: video stream NV12 -> H.264, optional PCM -> AAC.
pub struct Mp4Writer {
    writer: IMFSinkWriter,
    video_stream: u32,
    audio_stream: Option<u32>,
    width: u32,
    height: u32,
    fps: u32,
    audio_rate: u32,
    audio_channels: u16,
    frame_hns: i64,
    frames_written: u64,
    audio_frames_written: usize,
    audio_pending: Vec<i16>,
    samples_per_video_frame: usize,
    last_video: Option<Vec<u8>>,
}

impl Mp4Writer {
    /// `audio_rate=0` or `audio_channels=0` writes a video-only file.
    pub fn new(
        path: &Path,
        width: u32,
        height: u32,
        fps: u32,
        bitrate: u32,
        audio_rate: u32,
        audio_channels: u16,
    ) -> Result<Self, String> {
        ensure_mf()?;
        let (w, h) = (even_dim(width), even_dim(height));
        let fps = fps.clamp(1, 120);

        unsafe {
            let mut attrs: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attrs, 4).map_err(hres)?;
            let attrs = attrs.ok_or_else(|| "attributes is none".to_string())?;
            // CPU multi-threaded H.264 MFT — no GPU driver stalls.
            attrs
                .SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 0)
                .map_err(hres)?;

            let url = wide(&path.to_string_lossy());
            let writer = MFCreateSinkWriterFromURL(PCWSTR::from_raw(url.as_ptr()), None, Some(&attrs))
                .map_err(hres)?;

            // --- Video output (H.264) ---
            let vout = MFCreateMediaType().map_err(hres)?;
            vout.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(hres)?;
            vout.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264).map_err(hres)?;
            vout.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .map_err(hres)?;
            vout.SetUINT64(&MF_MT_FRAME_SIZE, pack64(w, h)).map_err(hres)?;
            vout.SetUINT64(&MF_MT_FRAME_RATE, pack64(fps, 1)).map_err(hres)?;
            vout.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack64(1, 1)).map_err(hres)?;
            vout.SetUINT32(&MF_MT_AVG_BITRATE, bitrate.max(100_000)).map_err(hres)?;
            vout.SetUINT32(&MF_MT_COMPRESSED, 1).map_err(hres)?;
            let video_stream = writer.AddStream(&vout).map_err(hres)?;

            let vin = MFCreateMediaType().map_err(hres)?;
            vin.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(hres)?;
            vin.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).map_err(hres)?;
            vin.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .map_err(hres)?;
            vin.SetUINT64(&MF_MT_FRAME_SIZE, pack64(w, h)).map_err(hres)?;
            vin.SetUINT64(&MF_MT_FRAME_RATE, pack64(fps, 1)).map_err(hres)?;
            writer.SetInputMediaType(video_stream, &vin, None).map_err(hres)?;

            // --- Audio output (AAC) / input (PCM) ---
            let mut audio_stream = None;
            let rate = audio_rate.max(8000);
            let ch = audio_channels.max(1) as u32;
            if audio_rate > 0 && audio_channels > 0 {
                let aout = MFCreateMediaType().map_err(hres)?;
                aout.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).map_err(hres)?;
                aout.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC).map_err(hres)?;
                aout.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, ch).map_err(hres)?;
                aout.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate).map_err(hres)?;
                aout.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).map_err(hres)?;
                aout.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 20_000)
                    .map_err(hres)?;
                let aidx = writer.AddStream(&aout).map_err(hres)?;

                let ain = MFCreateMediaType().map_err(hres)?;
                ain.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).map_err(hres)?;
                ain.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM).map_err(hres)?;
                ain.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, ch).map_err(hres)?;
                ain.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate).map_err(hres)?;
                ain.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).map_err(hres)?;
                ain.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, ch * 2).map_err(hres)?;
                ain.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, rate * ch * 2)
                    .map_err(hres)?;
                writer.SetInputMediaType(aidx, &ain, None).map_err(hres)?;
                audio_stream = Some(aidx);
            }

            writer.BeginWriting().map_err(hres)?;

            let samples_per_video_frame = if audio_stream.is_some() {
                (rate as usize / fps as usize).max(1)
            } else {
                0
            };

            Ok(Self {
                writer,
                video_stream,
                audio_stream,
                width: w,
                height: h,
                fps,
                audio_rate: if audio_stream.is_some() { rate } else { 0 },
                audio_channels: if audio_stream.is_some() { audio_channels.max(1) } else { 0 },
                frame_hns: 10_000_000 / fps as i64,
                frames_written: 0,
                audio_frames_written: 0,
                audio_pending: Vec::new(),
                samples_per_video_frame,
                last_video: None,
            })
        }
    }

    pub fn write_video_frame(&mut self, rgba: &RgbaImage) -> Result<(), String> {
        let nv12 = rgba_to_nv12(rgba);
        self.write_video_nv12(&nv12)?;
        self.last_video = Some(nv12);
        Ok(())
    }

    pub fn duplicate_last_frame(&mut self) -> Result<(), String> {
        if let Some(nv12) = self.last_video.clone() {
            self.write_video_nv12(&nv12)?;
        }
        Ok(())
    }

    fn write_video_nv12(&mut self, nv12: &[u8]) -> Result<(), String> {
        let pts = self.frames_written as i64 * self.frame_hns;
        let sample = make_sample(nv12, pts, self.frame_hns).map_err(hres)?;
        unsafe {
            self.writer.WriteSample(self.video_stream, &sample).map_err(hres)?;
        }
        self.frames_written += 1;
        Ok(())
    }

    /// Append PCM samples captured during the latest video frame interval.
    /// Writes continuous 2048-frame AAC-aligned chunks immediately without gating on video frames.
    pub fn write_audio(&mut self, chunk: &[i16]) -> Result<(), String> {
        if self.audio_stream.is_none() || chunk.is_empty() {
            return Ok(());
        }
        self.audio_pending.extend_from_slice(chunk);
        let ch = self.audio_channels as usize;
        let per = 2048usize;
        while self.audio_pending.len() >= per * ch {
            let n = per * ch;
            let data: Vec<i16> = self.audio_pending.drain(..n).collect();
            let pts = (self.audio_frames_written as i64 * 10_000_000) / self.audio_rate as i64;
            let dur = (per as i64 * 10_000_000) / self.audio_rate as i64;
            let mut bytes = Vec::with_capacity(n * 2);
            for s in &data {
                bytes.extend_from_slice(&s.to_le_bytes());
            }
            let sample = make_sample(&bytes, pts, dur).map_err(hres)?;
            unsafe {
                self.writer
                    .WriteSample(self.audio_stream.unwrap(), &sample)
                    .map_err(hres)?;
            }
            self.audio_frames_written += per;
        }
        Ok(())
    }

    /// Flush remaining audio + video, then finalize the MP4.
    pub fn finish(mut self) -> Result<(), String> {
        if self.audio_stream.is_some() && !self.audio_pending.is_empty() {
            let ch = self.audio_channels as usize;
            let n = self.audio_pending.len() / ch * ch;
            let data: Vec<i16> = self.audio_pending[..n].to_vec();
            self.audio_pending.drain(..n);
            if !data.is_empty() {
                let pts = (self.audio_frames_written as i64 * 10_000_000) / self.audio_rate as i64;
                let dur = (data.len() as i64 / ch as i64 * 10_000_000) / self.audio_rate as i64;
                let bytes: Vec<u8> = data.iter().flat_map(|s| s.to_le_bytes()).collect();
                let sample = make_sample(&bytes, pts, dur).map_err(hres)?;
                unsafe {
                    self.writer
                        .WriteSample(self.audio_stream.unwrap(), &sample)
                        .map_err(hres)?;
                }
            }
        }
        unsafe { self.writer.Finalize().map_err(hres) }
    }

    pub fn fps(&self) -> u32 {
        self.fps
    }

    pub fn frames_written(&self) -> u64 {
        self.frames_written
    }
}

/// Frame + audio metadata decoded from an MP4 via SourceReader.
pub struct Mp4Reader {
    reader: IMFSourceReader,
    video_stream: Option<u32>,
    _audio_stream: Option<u32>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub audio_rate: u32,
    pub audio_channels: u16,
    pub audio_samples: Vec<i16>,
    eof: bool,
}

impl Mp4Reader {
    pub fn new(path: &Path) -> Result<Self, String> {
        ensure_mf()?;
        unsafe {
            let mut attrs: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attrs, 2).map_err(hres)?;
            let attrs = attrs.ok_or_else(|| "attributes is none".to_string())?;
            attrs
                .SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)
                .map_err(hres)?;

            let url = wide(&path.to_string_lossy());
            let reader = MFCreateSourceReaderFromURL(PCWSTR::from_raw(url.as_ptr()), Some(&attrs))
                .map_err(hres)?;

            // Discover stream indices by native media type.
            let mut video_stream = None;
            let mut audio_stream = None;
            for i in 0..32u32 {
                match reader.GetNativeMediaType(i, 0) {
                    Ok(mt) => {
                        let major = mt.GetMajorType().unwrap_or_default();
                        if major == MFMediaType_Video {
                            video_stream = Some(i);
                        } else if major == MFMediaType_Audio {
                            audio_stream = Some(i);
                        }
                    }
                    Err(_) => break,
                }
                if video_stream.is_some() && audio_stream.is_some() {
                    break;
                }
            }

            // --- Video output: RGB32, capped to 960 px wide ---
            let mut width = 640u32;
            let mut height = 360u32;
            let mut fps = 25u32;
            if let Some(vidx) = video_stream {
                let native = reader.GetCurrentMediaType(vidx).map_err(hres)?;
                let fsize = native.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(pack64(640, 360));
                width = (fsize >> 32) as u32;
                height = fsize as u32;
                if let Ok(frate) = native.GetUINT64(&MF_MT_FRAME_RATE) {
                    let num = (frate >> 32) as u32;
                    let den = (frate as u32).max(1);
                    fps = (num / den).max(1);
                }
                // Cap decoded output at 960 px wide (aspect-preserving, then
                // even_dim) so Media Foundation downscales in native SIMD.
                const MAX_DECODE_WIDTH: u32 = 960;
                if width > MAX_DECODE_WIDTH {
                    height =
                        ((height as u64 * MAX_DECODE_WIDTH as u64 + width as u64 / 2)
                            / width as u64) as u32;
                    width = MAX_DECODE_WIDTH;
                }
                if width > 960 {
                    let s = 960.0 / width as f32;
                    width = even_dim(960);
                    height = even_dim((height as f32 * s) as u32);
                }
                let vout = MFCreateMediaType().map_err(hres)?;
                vout.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(hres)?;
                vout.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).map_err(hres)?;
                vout.SetUINT64(&MF_MT_FRAME_SIZE, pack64(even_dim(width), even_dim(height)))
                    .map_err(hres)?;
                vout.SetUINT64(&MF_MT_FRAME_RATE, pack64(fps, 1)).map_err(hres)?;
                reader.SetCurrentMediaType(vidx, None, &vout).map_err(hres)?;
            }

            // --- Audio output: PCM 16-bit, drained eagerly ---
            let mut audio_rate = 0u32;
            let mut audio_channels = 0u16;
            let mut audio_samples: Vec<i16> = Vec::new();
            if let Some(aidx) = audio_stream {
                let native = reader.GetCurrentMediaType(aidx).map_err(hres)?;
                audio_rate = native
                    .GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)
                    .unwrap_or(48_000);
                audio_channels = native.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(2) as u16;
                let aout = MFCreateMediaType().map_err(hres)?;
                aout.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).map_err(hres)?;
                aout.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM).map_err(hres)?;
                aout.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, audio_channels as u32)
                    .map_err(hres)?;
                aout.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, audio_rate)
                    .map_err(hres)?;
                aout.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).map_err(hres)?;
                aout.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, audio_channels as u32 * 2)
                    .map_err(hres)?;
                aout.SetUINT32(
                    &MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
                    audio_rate * audio_channels as u32 * 2,
                )
                .map_err(hres)?;
                reader.SetCurrentMediaType(aidx, None, &aout).map_err(hres)?;

                loop {
                    let mut flags = 0u32;
                    let mut sample: Option<IMFSample> = None;
                    let hr = reader.ReadSample(
                        aidx,
                        0,
                        None,
                        Some(&mut flags),
                        None,
                        Some(&mut sample),
                    );
                    if hr.is_err() || (flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32) != 0 {
                        break;
                    }
                    if let Some(s) = sample {
                        if let Ok(buf) = s.ConvertToContiguousBuffer() {
                            let mut ptr: *mut u8 = std::ptr::null_mut();
                            let mut len = 0u32;
                            if buf.Lock(&mut ptr, None, Some(&mut len)).is_ok() {
                                let n = (len as usize) / 2;
                                let slice = std::slice::from_raw_parts(ptr, len as usize);
                                for ii in 0..n {
                                    audio_samples.push(i16::from_le_bytes([
                                        slice[ii * 2],
                                        slice[ii * 2 + 1],
                                    ]));
                                }
                                let _ = buf.Unlock();
                            }
                        }
                    }
                }
            }

            Ok(Self {
                reader,
                video_stream,
                _audio_stream: audio_stream,
                width: even_dim(width),
                height: even_dim(height),
                fps: fps.max(1),
                audio_rate,
                audio_channels,
                audio_samples,
                eof: false,
            })
        }
    }

    pub fn has_video(&self) -> bool {
        self.video_stream.is_some()
    }

    /// Next decoded frame as RGBA, or `None` at end of stream / failure.
    pub fn next_video_frame(&mut self) -> Option<RgbaImage> {
        let vidx = self.video_stream?;
        if self.eof {
            return None;
        }
        loop {
            let mut flags = 0u32;
            let mut sample: Option<IMFSample> = None;
            unsafe {
                let hr = self.reader.ReadSample(
                    vidx,
                    0,
                    None,
                    Some(&mut flags),
                    None,
                    Some(&mut sample),
                );
                if hr.is_err() || (flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32) != 0 {
                    self.eof = true;
                    return None;
                }
                if (flags & MF_SOURCE_READERF_STREAMTICK.0 as u32) != 0 {
                    continue;
                }
                let s = sample?;
                let buf = s.ConvertToContiguousBuffer().ok()?;
                let mut ptr: *mut u8 = std::ptr::null_mut();
                let mut len = 0u32;
                if buf.Lock(&mut ptr, None, Some(&mut len)).is_err() {
                    return None;
                }
                let (w, h) = (self.width as usize, self.height as usize);
                let bytes = std::slice::from_raw_parts(ptr, len as usize);
                let mut rgba = vec![0u8; w * h * 4];
                let n = (bytes.len() / 4).min(w * h);
                for ii in 0..n {
                    rgba[ii * 4] = bytes[ii * 4 + 2];
                    rgba[ii * 4 + 1] = bytes[ii * 4 + 1];
                    rgba[ii * 4 + 2] = bytes[ii * 4];
                    rgba[ii * 4 + 3] = 255;
                }
                let _ = buf.Unlock();
                return RgbaImage::from_raw(w as u32, h as u32, rgba);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nv12_dimensions_and_planes() {
        let mut img = RgbaImage::new(64, 48);
        for y in 0..48 {
            for x in 0..64 {
                img.put_pixel(x, y, image::Rgba([200, 100, 50, 255]));
            }
        }
        let nv = rgba_to_nv12(&img);
        assert_eq!(nv.len(), 64 * 48 + 64 * 48 / 2);
        assert!(nv[0] > 16); // Y plane present
        assert!(nv[64 * 48] != 0 || nv[64 * 48 + 1] != 0); // chroma present
    }

    // Round-trip test only runs when Windows and an H.264 encoder are present.
    #[test]
    fn mp4_write_read_roundtrip() {
        if !h264_encoder_available() {
            eprintln!("H.264 encoder unavailable — skipping roundtrip");
            return;
        }
        let tmp = std::env::temp_dir().join("cliprec_mf_roundtrip.mp4");
        let mut img = RgbaImage::new(64, 48);
        for y in 0..48 {
            for x in 0..64 {
                img.put_pixel(x, y, image::Rgba([(x * 3) as u8, (y * 4) as u8, 120, 255]));
            }
        }
        {
            let mut w = Mp4Writer::new(&tmp, 64, 48, 10, 800_000, 48_000, 1).expect("open writer");
            for _ in 0..6 {
                w.write_video_frame(&img).expect("write frame");
                w.write_audio(&vec![0i16; 4800]).expect("write audio");
            }
            w.finish().expect("finalize");
        }
        assert!(tmp.exists() && tmp.metadata().unwrap().len() > 0);
        let r = Mp4Reader::new(&tmp).expect("open reader");
        assert_eq!(r.width, 64);
        assert_eq!(r.height, 48);
        assert!(r.audio_rate >= 8000);
        assert!(!r.audio_samples.is_empty());
        drop(r);
        let _ = std::fs::remove_file(&tmp);
    }

    #[allow(dead_code)]
    fn _dxgi_err_bounds(e: windows::core::Error) {
        let _ = e.code() == DXGI_ERROR_ACCESS_LOST;
    }
}

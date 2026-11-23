//! Background screen recorder + live webcam feed + microphone/system audio capture.
//!
//! All heavy work (GDI capture, scaling, webcam compositing, JPEG encoding, audio capture)
//! runs on worker threads so the egui UI thread never blocks.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use image::{imageops, Rgba, RgbaImage};

use crate::capture::{capture_screen_region, encode_jpeg, even_dim};
use crate::storage::WebcamSizePreset;

pub type SharedFrame = Arc<Mutex<Option<Arc<RgbaImage>>>>;
pub type LiveWebcam = Arc<Mutex<Option<(SharedFrame, WebcamSizePreset)>>>;

// Cap the buffered mic audio so a long pause cannot grow memory without bound.
const MAX_AUDIO_SAMPLES: usize = 48_000 * 2 * 60; // ~60s safety cap (drained every frame)
const DEFAULT_SAMPLE_RATE: u32 = 48_000;

// ============================================================================
// Webcam
// ============================================================================

pub struct WebcamFeed {
    pub latest: SharedFrame,
    pub error: Arc<Mutex<Option<String>>>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl WebcamFeed {
    pub fn start() -> Self {
        let latest: SharedFrame = Arc::new(Mutex::new(None));
        let error = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let (l, e, s) = (latest.clone(), error.clone(), stop.clone());
        let join = std::thread::Builder::new()
            .name("webcam".into())
            .spawn(move || webcam_loop(l, e, s))
            .ok();
        Self { latest, error, stop, join }
    }

    pub fn frame(&self) -> Option<Arc<RgbaImage>> {
        self.latest.lock().ok().and_then(|g| g.clone())
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().ok().and_then(|g| g.clone())
    }
}

impl Drop for WebcamFeed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn webcam_loop(latest: SharedFrame, error: Arc<Mutex<Option<String>>>, stop: Arc<AtomicBool>) {
    use nokhwa::pixel_format::RgbFormat;
    use nokhwa::utils::{
        CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType, Resolution,
    };
    use nokhwa::Camera;

    let set_err = |msg: String| {
        if let Ok(mut g) = error.lock() {
            *g = Some(msg);
        }
    };

    let req = RequestedFormat::new::<RgbFormat>(RequestedFormatType::Closest(CameraFormat::new(
        Resolution::new(640, 480),
        FrameFormat::YUYV,
        30,
    )));
    let mut cam = match Camera::new(CameraIndex::Index(0), req) {
        Ok(c) => c,
        Err(e) => {
            set_err(format!("No webcam available: {e}"));
            return;
        }
    };
    if let Err(e) = cam.open_stream() {
        set_err(format!("Could not open webcam: {e}"));
        return;
    }

    let mut fails = 0;
    while !stop.load(Ordering::Relaxed) {
        let buf = match cam.frame() {
            Ok(b) => b,
            Err(e) => {
                fails += 1;
                if fails > 50 {
                    set_err(format!("Webcam stopped: {e}"));
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
        };
        fails = 0;
        let img: Option<RgbaImage> = match buf.source_frame_format() {
            FrameFormat::MJPEG => image::load_from_memory_with_format(buf.buffer(), image::ImageFormat::Jpeg)
                .ok()
                .map(|d| d.to_rgba8()),
            _ => buf.decode_image::<RgbFormat>().ok().and_then(|rgb| {
                let (w, h) = (rgb.width(), rgb.height());
                let raw = rgb.into_raw();
                let mut rgba = Vec::with_capacity((w * h * 4) as usize);
                for px in raw.chunks_exact(3) {
                    rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
                }
                RgbaImage::from_raw(w, h, rgba)
            }),
        };
        if let Some(mut img) = img {
            imageops::flip_horizontal_in_place(&mut img); // mirror like a selfie view
            if let Ok(mut g) = latest.lock() {
                *g = Some(Arc::new(img));
            }
        }
    }
    let _ = cam.stop_stream();
}

/// Draws the webcam onto `frame` (circular bubble bottom-right, or full frame).
pub fn composite_webcam(frame: &mut RgbaImage, cam: &RgbaImage, size: WebcamSizePreset) {
    let (fw, fh) = frame.dimensions();
    if size == WebcamSizePreset::Full {
        let scaled = cover_resize(cam, fw, fh);
        imageops::replace(frame, &scaled, 0, 0);
        return;
    }
    let frac = if size == WebcamSizePreset::Small { 0.24 } else { 0.38 };
    let d = ((fw.min(fh) as f32) * frac).round().max(48.0) as u32;
    let margin = (fw.min(fh) as f32 * 0.03).round() as i64;
    let ox = fw as i64 - d as i64 - margin;
    let oy = fh as i64 - d as i64 - margin;
    if ox < 0 || oy < 0 {
        return;
    }
    let bubble = cover_resize(cam, d, d);
    let r = d as f32 / 2.0;
    let border = (d as f32 * 0.035).max(2.0);
    for y in 0..d {
        for x in 0..d {
            let dx = x as f32 + 0.5 - r;
            let dy = y as f32 + 0.5 - r;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist > r {
                continue;
            }
            let alpha = (r - dist).clamp(0.0, 1.0); // anti-aliased rim
            let src = if dist > r - border {
                Rgba([255, 255, 255, 255])
            } else {
                *bubble.get_pixel(x, y)
            };
            let px = (ox as u32 + x, oy as u32 + y);
            let dst = frame.get_pixel_mut(px.0, px.1);
            for c in 0..3 {
                dst[c] = (src[c] as f32 * alpha + dst[c] as f32 * (1.0 - alpha)) as u8;
            }
        }
    }
}

fn cover_resize(src: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    let (sw, sh) = src.dimensions();
    let scale = (w as f32 / sw as f32).max(h as f32 / sh as f32);
    let cw = ((w as f32 / scale) as u32).clamp(1, sw);
    let ch = ((h as f32 / scale) as u32).clamp(1, sh);
    let crop = imageops::crop_imm(src, (sw - cw) / 2, (sh - ch) / 2, cw, ch).to_image();
    imageops::resize(&crop, w, h, imageops::FilterType::Triangle)
}

// ============================================================================
// Microphone audio capture (cpal)
// ============================================================================

/// Returns the name of the default microphone input device, if any.
pub fn probe_default_mic_name() -> Option<String> {
    let host = cpal::default_host();
    let device = host.default_input_device()?;
    device.name().ok()
}

/// Captures the default microphone input device into a shared sample buffer.
///
/// The buffer is lock-protected and size-capped; the recorder thread drains it once
/// per video frame so audio stays interleaved with the video frames in the AVI.
struct AudioInput {
    samples: Arc<Mutex<Vec<i16>>>,
    level: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    _stream: Option<cpal::Stream>,
    sample_rate: u32,
    channels: u16,
    device_name: String,
    gain: f32,
}

impl AudioInput {
    fn start(mic_gain: f32, _ai_enhance: bool) -> Option<Self> {
        let host = cpal::default_host();
        let device = host.default_input_device()?;
        let device_name = device.name().unwrap_or_else(|_| "Unknown input device".to_string());
        let config = device.default_input_config().ok()?;
        let sample_rate = config.sample_rate() as u32;
        let channels = config.channels() as u16;

        let samples: Arc<Mutex<Vec<i16>>> = Arc::new(Mutex::new(Vec::new()));
        let level = Arc::new(AtomicU32::new(0));
        let stop: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
        let (s, l, st) = (samples.clone(), level.clone(), stop.clone());

        let stream_config = cpal::StreamConfig {
            channels,
            sample_rate,
            buffer_size: cpal::BufferSize::Default,
        };

        // Transparent soft-limiter only above 0.85 so normal speech is 100% pure and artifact-free;
        // studio noise reduction & EQ/compression are applied cleanly by FFmpeg during MP4 encoding.
        let clean_sample = move |raw: f32| -> f32 {
            let v = raw * mic_gain;
            let a = v.abs();
            if a <= 0.85 {
                v
            } else {
                v.signum() * (0.85 + 0.15 * ((a - 0.85) / 0.15).tanh())
            }
        };

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device
                .build_input_stream::<f32, _, _>(
                    &stream_config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        if st.load(Ordering::Relaxed) {
                            return;
                        }
                        let mut peak: f32 = 0.0;
                        let mut g = match s.lock() {
                            Ok(g) => g,
                            Err(_) => return,
                        };
                        let cap = g.len() + data.len();
                        if cap > MAX_AUDIO_SAMPLES {
                            let excess = cap.saturating_sub(MAX_AUDIO_SAMPLES);
                            g.drain(..excess);
                        }
                        for &raw in data {
                            let v = clean_sample(raw);
                            let x = (v * i16::MAX as f32).clamp(-32768.0, 32767.0) as i16;
                            let a = v.abs();
                            if a > peak {
                                peak = a;
                            }
                            g.push(x);
                        }
                        if g.len() > MAX_AUDIO_SAMPLES {
                            let excess = g.len().saturating_sub(MAX_AUDIO_SAMPLES);
                            g.drain(..excess);
                        }
                        drop(g);
                        let cur = f32::from_bits(l.load(Ordering::Relaxed));
                        if peak > cur {
                            l.store(peak.to_bits(), Ordering::Relaxed);
                        }
                    },
                    move |e| {
                        let _ = e;
                    },
                    None,
                )
                .ok()?,
            cpal::SampleFormat::I16 => device
                .build_input_stream::<i16, _, _>(
                    &stream_config,
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        if st.load(Ordering::Relaxed) {
                            return;
                        }
                        let mut peak: f32 = 0.0;
                        let mut g = match s.lock() {
                            Ok(g) => g,
                            Err(_) => return,
                        };
                        let cap = g.len() + data.len();
                        if cap > MAX_AUDIO_SAMPLES {
                            let excess = cap.saturating_sub(MAX_AUDIO_SAMPLES);
                            g.drain(..excess);
                        }
                        for &raw_i16 in data {
                            let v = clean_sample(raw_i16 as f32 / 32768.0);
                            let x = (v * i16::MAX as f32).clamp(-32768.0, 32767.0) as i16;
                            let a = v.abs();
                            if a > peak {
                                peak = a;
                            }
                            g.push(x);
                        }
                        if g.len() > MAX_AUDIO_SAMPLES {
                            let excess = g.len().saturating_sub(MAX_AUDIO_SAMPLES);
                            g.drain(..excess);
                        }
                        drop(g);
                        let cur = f32::from_bits(l.load(Ordering::Relaxed));
                        if peak > cur {
                            l.store(peak.to_bits(), Ordering::Relaxed);
                        }
                    },
                    move |e| {
                        let _ = e;
                    },
                    None,
                )
                .ok()?,
            _ => return None,
        };
        stream.play().ok()?;

        Some(Self {
            samples,
            level,
            stop,
            _stream: Some(stream),
            sample_rate,
            channels,
            device_name,
            gain: mic_gain,
        })
    }

    fn device_name(&self) -> &str {
        &self.device_name
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    /// Latest peak mic level (0.0..=1.0).
    fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }
}

/// Drains all samples currently in the buffer and leaves it empty.
/// Called once per video frame so audio stays interleaved with video frames.
fn drain_all(samples: &Arc<Mutex<Vec<i16>>>) -> Vec<i16> {
    let mut g = match samples.lock() {
        Ok(g) => g,
        Err(_) => return Vec::new(),
    };
    std::mem::take(&mut *g)
}

// ============================================================================
// Recorder
// ============================================================================
// Recorder
// ============================================================================

pub struct RecorderOutput {
    pub frames: Vec<Vec<u8>>,
    pub frame_audio: Vec<Vec<i16>>, // PCM i16 captured during each frame's interval
    pub first_frame: Option<RgbaImage>,
    pub audio_sample_rate: u32, // 0 when no audio track
    pub audio_channels: u16,
}

pub struct RecorderHandle {
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    webcam: LiveWebcam,
    pub preview: SharedFrame,
    #[allow(dead_code)]
    pub frame_count: Arc<AtomicUsize>,
    audio: Option<AudioInput>,
    pub audio_level: Arc<AtomicU32>,
    mic_device: Option<String>,
    join: Option<JoinHandle<RecorderOutput>>,
}

pub struct RecorderConfig {
    pub region: [u32; 4],
    pub fps: u32,
    pub max_height: u32,
    pub webcam: Option<(SharedFrame, WebcamSizePreset)>,
    pub record_mic: bool,
    pub record_system_audio: bool,
    pub mic_gain: f32,
    pub ai_mic_enhance: bool,
}

pub const MAX_FRAMES: usize = 108_000;

impl RecorderHandle {
    pub fn start(mut cfg: RecorderConfig) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let webcam: LiveWebcam = Arc::new(Mutex::new(cfg.webcam.take()));
        let preview: SharedFrame = Arc::new(Mutex::new(None));
        let frame_count = Arc::new(AtomicUsize::new(0));

        let use_audio_track = cfg.record_mic || cfg.record_system_audio;
        let audio = if cfg.record_mic {
            AudioInput::start(cfg.mic_gain, cfg.ai_mic_enhance)
        } else {
            None
        };

        let (audio_samples, audio_level, audio_rate, audio_channels, mic_device) = match &audio {
            Some(a) => (
                a.samples.clone(),
                a.level.clone(),
                a.sample_rate,
                a.channels,
                Some(a.device_name.clone()),
            ),
            None => (
                Arc::new(Mutex::new(Vec::new())),
                Arc::new(AtomicU32::new(0)),
                if use_audio_track {
                    DEFAULT_SAMPLE_RATE
                } else {
                    0
                },
                if use_audio_track { 1 } else { 0 },
                None,
            ),
        };

        let audio_level_for_handle = audio_level.clone();
        let (s, p, w, pv, fc, as_, al, ar, ac) = (
            stop.clone(),
            paused.clone(),
            webcam.clone(),
            preview.clone(),
            frame_count.clone(),
            audio_samples,
            audio_level,
            audio_rate,
            audio_channels,
        );
        let join = std::thread::Builder::new()
            .name("recorder".into())
            .spawn(move || {
                record_loop(cfg, s, p, w, pv, fc, as_, al, ar, ac)
            })
            .ok();
        Self {
            stop,
            paused,
            webcam,
            preview,
            frame_count,
            audio,
            audio_level: audio_level_for_handle,
            mic_device,
            join,
        }
    }

    pub fn set_paused(&self, v: bool) {
        self.paused.store(v, Ordering::SeqCst);
    }

    pub fn set_webcam(&self, webcam: Option<(SharedFrame, WebcamSizePreset)>) {
        if let Ok(mut g) = self.webcam.lock() {
            *g = webcam;
        }
    }

    /// Latest peak mic level (0.0..=1.0); 0.0 when mic capture is unavailable/off.
    pub fn mic_level(&self) -> f32 {
        f32::from_bits(self.audio_level.load(Ordering::Relaxed))
    }

    /// Whether an audio track will be written to the output file.
    pub fn has_audio(&self) -> bool {
        match &self.audio {
            Some(a) => a.sample_rate() > 0,
            None => false,
        }
    }

    /// Name of the microphone input device, if capture is active.
    pub fn mic_device_name(&self) -> Option<&str> {
        self.mic_device.as_deref()
    }

    pub fn take_preview(&self) -> Option<Arc<RgbaImage>> {
        self.preview.lock().ok().and_then(|mut g| g.take())
    }

    pub fn finish(mut self) -> RecorderOutput {
        self.stop.store(true, Ordering::SeqCst);
        self.join
            .take()
            .and_then(|j| j.join().ok())
            .unwrap_or(RecorderOutput {
                frames: Vec::new(),
                frame_audio: Vec::new(),
                first_frame: None,
                audio_sample_rate: 0,
                audio_channels: 0,
            })
    }
}

impl Drop for RecorderHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

pub fn grab_frame(region: [u32; 4], max_height: u32, webcam: Option<&(SharedFrame, WebcamSizePreset)>) -> RgbaImage {
    let [rx, ry, rw, rh] = region;
    let raw = capture_screen_region(rx as i32, ry as i32, rw, rh);
    let mut frame = if raw.height() > max_height {
        let scale = max_height as f32 / raw.height() as f32;
        let nw = even_dim((raw.width() as f32 * scale) as u32);
        imageops::resize(&raw, nw, even_dim(max_height), imageops::FilterType::Nearest)
    } else {
        raw
    };
    if let Some((shared, size)) = webcam {
        let cam = shared.lock().ok().and_then(|g| g.clone());
        if let Some(cam) = cam {
            composite_webcam(&mut frame, &cam, *size);
        }
    }
    frame
}

fn sample_frame_hash(img: &RgbaImage) -> u64 {
    let raw = img.as_raw();
    let mut h: u64 = 0xcbf29ce484222325;
    let step = (raw.len() / 256).max(4);
    for b in raw.iter().step_by(step) {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn record_loop(
    cfg: RecorderConfig,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    webcam: LiveWebcam,
    preview: SharedFrame,
    frame_count: Arc<AtomicUsize>,
    audio_samples: Arc<Mutex<Vec<i16>>>,
    _audio_level: Arc<AtomicU32>,
    audio_sample_rate: u32,
    audio_channels: u16,
) -> RecorderOutput {
    let _ = _audio_level; // level is updated by the cpal callback; read via mic_level()
    enum EncoderCmd {
        Frame(RgbaImage),
        DuplicateLast,
    }

    let target_fps = cfg.fps.clamp(1, 60);
    let interval = Duration::from_secs_f64(1.0 / target_fps as f64);
    let (enc_tx, enc_rx) = std::sync::mpsc::sync_channel::<EncoderCmd>(8);
    let enc_join = std::thread::Builder::new()
        .name("jpeg_encoder".into())
        .spawn(move || {
            let mut encoded_frames: Vec<Vec<u8>> = Vec::new();
            let mut last_hash: Option<u64> = None;
            while let Ok(cmd) = enc_rx.recv() {
                match cmd {
                    EncoderCmd::DuplicateLast => {
                        if let Some(prev) = encoded_frames.last() {
                            encoded_frames.push(prev.clone());
                        }
                    }
                    EncoderCmd::Frame(frame) => {
                        let h = sample_frame_hash(&frame);
                        if last_hash == Some(h) {
                            if let Some(prev) = encoded_frames.last() {
                                encoded_frames.push(prev.clone());
                                continue;
                            }
                        }
                        last_hash = Some(h);
                        encoded_frames.push(encode_jpeg(&frame, 58));
                    }
                }
            }
            encoded_frames
        })
        .ok();

    let mut captured_count = 0usize;
    let mut frame_audio: Vec<Vec<i16>> = Vec::new();
    let mut first_frame = None;
    let mut next = Instant::now();
    let use_audio_track = audio_sample_rate > 0 && audio_channels > 0;
    let mut active_elapsed = Duration::ZERO;
    let mut last_active_tick = Instant::now();
    let mut total_audio_samples_captured: usize = 0;

    while !stop.load(Ordering::Relaxed) {
        if paused.load(Ordering::Relaxed) {
            // Discard audio captured while paused so it is not recorded.
            drain_all(&audio_samples);
            std::thread::sleep(Duration::from_millis(25));
            next = Instant::now();
            last_active_tick = Instant::now();
            continue;
        }
        let now = Instant::now();
        active_elapsed += now.saturating_duration_since(last_active_tick);
        last_active_tick = now;

        if now < next {
            std::thread::sleep((next - now).min(Duration::from_millis(5)));
            continue;
        }
        next += interval;
        if now > next + interval * 3 {
            next = now + interval; // fell behind — don't burst
        }
        if captured_count >= MAX_FRAMES {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }

        let current_cam = webcam.lock().ok().and_then(|g| g.clone());
        let frame = grab_frame(cfg.region, cfg.max_height, current_cam.as_ref());
        captured_count += 1;
        frame_count.store(captured_count, Ordering::Relaxed);

        // Decay the level meter so it reflects current audio, not peak-ever.
        let cur = f32::from_bits(_audio_level.load(Ordering::Relaxed));
        _audio_level.store((cur * 0.70).to_bits(), Ordering::Relaxed);

        // Drain all microphone audio captured since the last frame for interleaving.
        if use_audio_track {
            let chunk = if cfg.record_mic {
                drain_all(&audio_samples)
            } else {
                // System-audio-only: silence (WASAPI loopback capture is not available
                // in this pure-Rust build, so system audio is recorded as silence).
                let samples_per_interval =
                    (interval.as_secs_f64() * audio_sample_rate as f64 * audio_channels as f64)
                        .round() as usize;
                vec![0i16; samples_per_interval]
            };
            total_audio_samples_captured += chunk.len();
            frame_audio.push(chunk);
        }

        // Small preview for the UI (cheap to upload as a texture)
        if captured_count % 2 == 1 {
            let thumb = if frame.width() > 640 {
                let h = (frame.height() as f32 * 640.0 / frame.width() as f32) as u32;
                imageops::thumbnail(&frame, 640, h.max(1))
            } else {
                frame.clone()
            };
            if let Ok(mut g) = preview.lock() {
                *g = Some(Arc::new(thumb));
            }
        }
        if first_frame.is_none() {
            first_frame = Some(frame.clone());
        }
        let _ = enc_tx.send(EncoderCmd::Frame(frame));

        // Lock video frame count to the hardware audio clock (or active wall clock)
        // so video never drifts behind or ahead of audio.
        let auth_elapsed_secs = if use_audio_track && cfg.record_mic && total_audio_samples_captured > 0 {
            total_audio_samples_captured as f64
                / (audio_sample_rate as f64 * audio_channels.max(1) as f64)
        } else {
            active_elapsed.as_secs_f64()
        };
        let expected_frames = (auth_elapsed_secs * target_fps as f64).floor() as usize;
        let mut catchup = 0usize;
        while captured_count < expected_frames && captured_count < MAX_FRAMES && catchup < 4 {
            let _ = enc_tx.send(EncoderCmd::DuplicateLast);
            if use_audio_track {
                frame_audio.push(Vec::new());
            }
            captured_count += 1;
            catchup += 1;
            frame_count.store(captured_count, Ordering::Relaxed);
        }
    }
    drop(enc_tx);
    let frames = enc_join
        .and_then(|j| j.join().ok())
        .unwrap_or_default();
    let total_audio: usize = frame_audio.iter().map(|c| c.len()).sum();
    eprintln!("[RECORD] done: frames={} audio_chunks={} total_audio_samples={} rate={} channels={}",
        frames.len(), frame_audio.len(), total_audio, audio_sample_rate, audio_channels);
    RecorderOutput {
        frames,
        frame_audio,
        first_frame,
        audio_sample_rate,
        audio_channels,
    }
}

// ============================================================================
// Synchronized Audio Playback for the In-App Video Player (cpal)
// ============================================================================

pub struct VideoAudioPlayer {
    out_rate: u32,
    out_channels: u16,
    total_samples: usize,
    cursor: Arc<AtomicUsize>,
    playing: Arc<AtomicBool>,
    muted: Arc<AtomicBool>,
    volume_bits: Arc<AtomicU32>,
    _stream: Option<cpal::Stream>,
}

impl VideoAudioPlayer {
    pub fn new(samples: Vec<i16>, src_rate: u32, src_channels: u16) -> Option<Self> {
        if samples.is_empty() || src_rate == 0 || src_channels == 0 {
            return None;
        }
        let host = cpal::default_host();
        let device = host.default_output_device()?;
        let config = device.default_output_config().ok()?;
        let out_rate = config.sample_rate() as u32;
        let out_channels = config.channels() as u16;

        let resampled = resample_to_f32(&samples, src_rate, src_channels, out_rate, out_channels);
        let total_samples = resampled.len();
        let data = Arc::new(resampled);
        let cursor = Arc::new(AtomicUsize::new(0));
        let playing = Arc::new(AtomicBool::new(true));
        let muted = Arc::new(AtomicBool::new(false));
        let volume_bits = Arc::new(AtomicU32::new(1.0f32.to_bits()));

        let stream_config = cpal::StreamConfig {
            channels: out_channels,
            sample_rate: out_rate,
            buffer_size: cpal::BufferSize::Default,
        };

        let (d, cur, pl, mu, vb) = (
            data.clone(),
            cursor.clone(),
            playing.clone(),
            muted.clone(),
            volume_bits.clone(),
        );

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device
                .build_output_stream::<f32, _, _>(
                    &stream_config,
                    move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                        let is_playing = pl.load(Ordering::Relaxed);
                        let is_muted = mu.load(Ordering::Relaxed);
                        let vol = if is_playing && !is_muted {
                            f32::from_bits(vb.load(Ordering::Relaxed))
                        } else {
                            0.0
                        };
                        if !is_playing {
                            out.fill(0.0);
                            return;
                        }
                        let mut idx = cur.load(Ordering::Relaxed);
                        for slot in out.iter_mut() {
                            if idx < d.len() {
                                *slot = (d[idx] * vol).clamp(-1.0, 1.0);
                                idx += 1;
                            } else {
                                *slot = 0.0;
                            }
                        }
                        cur.store(idx, Ordering::Relaxed);
                    },
                    |_| {},
                    None,
                )
                .ok()?,
            cpal::SampleFormat::I16 => device
                .build_output_stream::<i16, _, _>(
                    &stream_config,
                    move |out: &mut [i16], _: &cpal::OutputCallbackInfo| {
                        let is_playing = pl.load(Ordering::Relaxed);
                        let is_muted = mu.load(Ordering::Relaxed);
                        let vol = if is_playing && !is_muted {
                            f32::from_bits(vb.load(Ordering::Relaxed))
                        } else {
                            0.0
                        };
                        if !is_playing {
                            out.fill(0);
                            return;
                        }
                        let mut idx = cur.load(Ordering::Relaxed);
                        for slot in out.iter_mut() {
                            if idx < d.len() {
                                *slot = ((d[idx] * vol).clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                                idx += 1;
                            } else {
                                *slot = 0;
                            }
                        }
                        cur.store(idx, Ordering::Relaxed);
                    },
                    |_| {},
                    None,
                )
                .ok()?,
            _ => return None,
        };
        stream.play().ok()?;

        Some(Self {
            out_rate,
            out_channels,
            total_samples,
            cursor,
            playing,
            muted,
            volume_bits,
            _stream: Some(stream),
        })
    }

    pub fn set_playing(&self, p: bool) {
        self.playing.store(p, Ordering::Relaxed);
    }

    pub fn set_muted(&self, m: bool) {
        self.muted.store(m, Ordering::Relaxed);
    }

    pub fn set_volume(&self, v: f32) {
        self.volume_bits.store(v.clamp(0.0, 1.5).to_bits(), Ordering::Relaxed);
    }

    pub fn seek_seconds(&self, secs: f32) {
        let frame_idx = (secs.max(0.0) * self.out_rate as f32).round() as usize;
        let sample_idx = (frame_idx * self.out_channels as usize).min(self.total_samples);
        self.cursor.store(sample_idx, Ordering::Relaxed);
    }

    pub fn position_seconds(&self) -> f32 {
        let idx = self.cursor.load(Ordering::Relaxed);
        let frames = idx / (self.out_channels.max(1) as usize);
        frames as f32 / (self.out_rate.max(1) as f32)
    }

    pub fn is_finished(&self) -> bool {
        self.cursor.load(Ordering::Relaxed) >= self.total_samples
    }
}

fn resample_to_f32(
    src: &[i16],
    src_rate: u32,
    src_ch: u16,
    dst_rate: u32,
    dst_ch: u16,
) -> Vec<f32> {
    let sc = src_ch.max(1) as usize;
    let dc = dst_ch.max(1) as usize;
    let src_frames = src.len() / sc;
    if src_frames == 0 {
        return Vec::new();
    }
    let ratio = src_rate as f64 / dst_rate.max(1) as f64;
    let dst_frames = ((src_frames as f64) / ratio).round().max(1.0) as usize;
    let mut out = Vec::with_capacity(dst_frames * dc);
    for df in 0..dst_frames {
        let sf_pos = df as f64 * ratio;
        let sf0 = (sf_pos.floor() as usize).min(src_frames - 1);
        let sf1 = (sf0 + 1).min(src_frames - 1);
        let frac = (sf_pos - sf0 as f64) as f32;
        for c in 0..dc {
            let s_c = c.min(sc - 1);
            let v0 = src[sf0 * sc + s_c] as f32 / 32768.0;
            let v1 = src[sf1 * sc + s_c] as f32 / 32768.0;
            out.push(v0 + (v1 - v0) * frac);
        }
    }
    out
}

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

use crate::audio_loopback::SystemAudioInput;
use crate::capture::{
    apply_cursor_overlay, blit_cursor, capture_cursor_sprite, capture_screen_region,
    cursor_position, encode_jpeg, even_dim, frame_is_black, mouse_left_down, ClickRipple,
    DxgiCapturer,
};
use crate::mf::Mp4Writer;
use crate::storage::{CursorStyle, EncoderKind, WebcamSizePreset};

pub type SharedFrame = Arc<Mutex<Option<Arc<RgbaImage>>>>;
pub type LiveWebcam = Arc<Mutex<Option<(SharedFrame, WebcamSizePreset)>>>;

// Cap the buffered mic audio so a long pause cannot grow memory without bound.
const MAX_AUDIO_SAMPLES: usize = 48_000 * 2 * 60; // ~60s safety cap (drained every frame)
const DEFAULT_SAMPLE_RATE: u32 = 48_000;
/// All tracks are normalized to this rate/channel count before encoding.
const AUDIO_OUT_RATE: u32 = 48_000;
const AUDIO_OUT_CHANNELS: u16 = 2;

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

    let req_mjpeg = RequestedFormat::new::<RgbFormat>(RequestedFormatType::Closest(CameraFormat::new(
        Resolution::new(640, 480),
        FrameFormat::MJPEG,
        30,
    )));
    let req_yuyv = RequestedFormat::new::<RgbFormat>(RequestedFormatType::Closest(CameraFormat::new(
        Resolution::new(640, 480),
        FrameFormat::YUYV,
        30,
    )));
    let req_any = RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate);

    let mut cam = match Camera::new(CameraIndex::Index(0), req_mjpeg)
        .or_else(|_| Camera::new(CameraIndex::Index(0), req_yuyv))
        .or_else(|_| Camera::new(CameraIndex::Index(0), req_any))
    {
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
    /// `Some(Ok(()))` = MP4 written straight to `mp4_path` during recording.
    pub mp4_result: Option<Result<(), String>>,
    pub mp4_path: Option<std::path::PathBuf>,
    pub events: Vec<String>,
    pub total_frames: usize,
    pub total_audio_samples: usize,
}

pub struct RecorderHandle {
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    webcam: LiveWebcam,
    pub preview: SharedFrame,
    pub latest_frame: SharedFrame,
    #[allow(dead_code)]
    pub frame_count: Arc<AtomicUsize>,
    audio: Option<AudioInput>,
    system_audio: Option<SystemAudioInput>,
    pub audio_level: Arc<AtomicU32>,
    pub sys_audio_level: Arc<AtomicU32>,
    mic_device: Option<String>,
    events: Arc<Mutex<Vec<String>>>,
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
    /// Try DXGI Desktop Duplication first (fixes black screens on GPU apps).
    pub use_dxgi: bool,
    /// Monitor index (virtual-desktop rect) used by the DXGI capturer.
    pub monitor_index: usize,
    /// Cursor highlight style; `None` disables the cursor overlay.
    pub cursor_style: Option<CursorStyle>,
    /// Native MP4 (Media Foundation) vs legacy MJPEG AVI.
    pub encoder: EncoderKind,
    /// Pre-computed output path when `encoder == NativeMp4` (or `None` for AVI).
    pub mp4_path: Option<std::path::PathBuf>,
}

pub const MAX_FRAMES: usize = 108_000;

impl RecorderHandle {
    pub fn start(mut cfg: RecorderConfig) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let webcam: LiveWebcam = Arc::new(Mutex::new(cfg.webcam.take()));
        let preview: SharedFrame = Arc::new(Mutex::new(None));
        let latest_frame: SharedFrame = Arc::new(Mutex::new(None));
        let frame_count = Arc::new(AtomicUsize::new(0));
        let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        let use_audio_track = cfg.record_mic || cfg.record_system_audio;
        let audio = if cfg.record_mic {
            AudioInput::start(cfg.mic_gain, cfg.ai_mic_enhance)
        } else {
            None
        };
        let system_audio = if cfg.record_system_audio {
            SystemAudioInput::start()
        } else {
            None
        };

        let (audio_samples, audio_level, mic_device) = match &audio {
            Some(a) => (a.samples.clone(), a.level.clone(), Some(a.device_name.clone())),
            None => (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicU32::new(0)), None),
        };
        let (sys_samples, sys_level) = match &system_audio {
            Some(s) => (s.samples_arc(), s.level_arc()),
            None => (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicU32::new(0))),
        };
        let mic_rate = audio.as_ref().map(|a| a.sample_rate()).unwrap_or(AUDIO_OUT_RATE);
        let mic_channels = audio.as_ref().map(|a| a.channels()).unwrap_or(AUDIO_OUT_CHANNELS);
        let sys_rate = system_audio.as_ref().map(|s| s.sample_rate()).unwrap_or(AUDIO_OUT_RATE);
        let sys_channels = system_audio.as_ref().map(|s| s.channels()).unwrap_or(AUDIO_OUT_CHANNELS);
        // Audio is normalized to 48 kHz stereo before encoding.
        let audio_rate = if use_audio_track { AUDIO_OUT_RATE } else { 0 };
        let audio_channels = if use_audio_track { AUDIO_OUT_CHANNELS } else { 0 };

        let audio_level_for_handle = audio_level.clone();
        let sys_level_for_handle = sys_level.clone();
        let (s, p, w, pv, fc, as_, al, mr, mc, ar, ac, ss, sl, sr, sc, ev) = (
            stop.clone(),
            paused.clone(),
            webcam.clone(),
            preview.clone(),
            frame_count.clone(),
            audio_samples,
            audio_level,
            mic_rate,
            mic_channels,
            audio_rate,
            audio_channels,
            sys_samples,
            sys_level,
            sys_rate,
            sys_channels,
            events.clone(),
        );
        let latest_frame_for_thread = latest_frame.clone();
        let join = std::thread::Builder::new()
            .name("recorder".into())
            .spawn(move || {
                record_loop(cfg, s, p, w, pv, latest_frame_for_thread, fc, as_, al, mr, mc, ar, ac, ss, sl, sr, sc, ev)
            })
            .ok();
        Self {
            stop,
            paused,
            webcam,
            preview,
            latest_frame,
            frame_count,
            audio,
            system_audio,
            audio_level: audio_level_for_handle,
            sys_audio_level: sys_level_for_handle,
            mic_device,
            events,
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

    /// Latest peak system-audio (loopback) level (0.0..=1.0).
    pub fn system_audio_level(&self) -> f32 {
        f32::from_bits(self.sys_audio_level.load(Ordering::Relaxed))
    }

    /// Whether the WASAPI loopback capture thread is running.
    pub fn system_audio_active(&self) -> bool {
        self.system_audio.is_some()
    }

    /// Drains recorder events (capture source switches, warnings) for the UI.
    pub fn drain_events(&self) -> Vec<String> {
        let mut g = match self.events.lock() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        std::mem::take(&mut *g)
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

    pub fn latest_frame(&self) -> Option<Arc<RgbaImage>> {
        self.latest_frame.lock().ok().and_then(|g| g.clone())
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
                mp4_result: None,
                mp4_path: None,
                events: Vec::new(),
                total_frames: 0,
                total_audio_samples: 0,
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
    latest_frame: SharedFrame,
    frame_count: Arc<AtomicUsize>,
    audio_samples: Arc<Mutex<Vec<i16>>>,
    audio_level: Arc<AtomicU32>,
    mic_rate: u32,
    mic_channels: u16,
    audio_sample_rate: u32,
    audio_channels: u16,
    sys_samples: Arc<Mutex<Vec<i16>>>,
    sys_level: Arc<AtomicU32>,
    sys_rate: u32,
    sys_channels: u16,
    events: Arc<Mutex<Vec<String>>>,
) -> RecorderOutput {
    // Mic + system levels are updated by their callbacks; decay them here.
    enum EncoderCmd {
        Frame(Arc<RgbaImage>),
        DuplicateLast,
        Audio(Vec<i16>),
    }

    let target_fps = cfg.fps.clamp(1, 60);
    let interval = Duration::from_secs_f64(1.0 / target_fps as f64);
    let mut push_event = |msg: String| {
        if let Ok(mut g) = events.lock() {
            if g.len() >= 8 {
                g.remove(0);
            }
            g.push(msg);
        }
    };

    // --- Native MP4 encoder (Media Foundation) or legacy JPEG pipeline ---
    let (enc_tx, enc_rx) = std::sync::mpsc::sync_channel::<EncoderCmd>(8);
    let mp4_path_for_enc = cfg.mp4_path.clone();
    let mp4_path_result = cfg.mp4_path.clone();
    let out_w = even_dim(cfg.region[2]);
    let out_h = even_dim(cfg.region[3].min(cfg.max_height.max(2)));
    let bitrate = bitrate_for_height(cfg.max_height);
    let fps = target_fps;
    let use_audio_track = audio_sample_rate > 0 && audio_channels > 0;
    let enc_join = std::thread::Builder::new()
        .name("encoder".into())
        .spawn(move || {
            let mut encoded_frames: Vec<Vec<u8>> = Vec::new();
            let mut frame_audio: Vec<Vec<i16>> = Vec::new();
            let mut last_hash: Option<u64> = None;
            let mut mp4 = if let Some(path) = &mp4_path_for_enc {
                match Mp4Writer::new(
                    path,
                    out_w,
                    out_h,
                    fps,
                    bitrate,
                    if use_audio_track { audio_sample_rate } else { 0 },
                    if use_audio_track { audio_channels } else { 0 },
                ) {
                    Ok(w) => Some(w),
                    Err(e) => {
                        eprintln!("[RECORD] MP4 writer failed to start: {e}");
                        None
                    }
                }
            } else {
                None
            };
            let mut mp4_error: Option<String> = None;
            let mut mp4_active = mp4.is_some();
            while let Ok(cmd) = enc_rx.recv() {
                match cmd {
                    EncoderCmd::DuplicateLast => {
                        if mp4_active {
                            if let Some(w) = mp4.as_mut() {
                                if let Err(e) = w.duplicate_last_frame() {
                                    mp4_error.get_or_insert_with(|| format!("MP4 encoder error: {e}"));
                                    mp4_active = false;
                                }
                            }
                        } else if let Some(prev) = encoded_frames.last() {
                            encoded_frames.push(prev.clone());
                            frame_audio.push(Vec::new());
                        }
                    }
                    EncoderCmd::Frame(frame) => {
                        if mp4_active {
                            if let Some(w) = mp4.as_mut() {
                                if let Err(e) = w.write_video_frame(&frame) {
                                    mp4_error.get_or_insert_with(|| format!("MP4 encoder error: {e}"));
                                    mp4_active = false;
                                } else {
                                    continue;
                                }
                            }
                        }
                        let h = sample_frame_hash(&frame);
                        if last_hash == Some(h) {
                            if let Some(prev) = encoded_frames.last() {
                                encoded_frames.push(prev.clone());
                                frame_audio.push(Vec::new());
                                continue;
                            }
                        }
                        last_hash = Some(h);
                        encoded_frames.push(encode_jpeg(&frame, 58));
                    }
                    EncoderCmd::Audio(chunk) => {
                        if mp4_active {
                            if let Some(w) = mp4.as_mut() {
                                if let Err(e) = w.write_audio(&chunk) {
                                    mp4_error.get_or_insert_with(|| format!("MP4 audio error: {e}"));
                                    mp4_active = false;
                                } else {
                                    continue;
                                }
                            }
                        }
                        frame_audio.push(chunk);
                    }
                }
            }
            let mp4_result = match mp4 {
                Some(w) => Some({
                    if let Some(err) = mp4_error {
                        Err(err)
                    } else {
                        w.finish().map_err(|e| format!("MP4 finalize failed: {e}"))
                    }
                }),
                None => None,
            };
            (encoded_frames, frame_audio, mp4_result)
        })
        .ok();

    let mut captured_count = 0usize;
    let mut frame_audio: Vec<Vec<i16>> = Vec::new();
    let mut first_frame = None;
    let mut next = Instant::now();
    let mut active_elapsed = Duration::ZERO;
    let mut last_active_tick = Instant::now();
    let mut total_audio_samples_captured: usize = 0;

    // DXGI Desktop Duplication: real GPU-desktop capture with GDI fallback.
    let mut dxgi: Option<DxgiCapturer> = if cfg.use_dxgi {
        match DxgiCapturer::new(cfg.monitor_index) {
            Ok(c) => Some(c),
            Err(e) => {
                push_event(format!(
                    "DXGI capture unavailable on this machine ({e}) — using GDI."
                ));
                None
            }
        }
    } else {
        None
    };
    let mut dxgi_failures = 0u32;
    let mut black_streak: u32 = 0;
    let mut gdi_until: Option<Instant> = None;

    // Cursor overlay state.
    let mut prev_click_down = false;
    let mut ripple: Option<ClickRipple> = None;
    let mut cursor_sprite = None;

    while !stop.load(Ordering::Relaxed) {
        if paused.load(Ordering::Relaxed) {
            // Discard audio captured while paused so it is not recorded.
            drain_all(&audio_samples);
            if cfg.record_system_audio {
                let _ = sys_samples.lock().map(|mut g| g.clear());
            }
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
        let mut frame = grab_frame_dxgi(
            cfg.region,
            cfg.max_height,
            dxgi.as_mut(),
            &mut dxgi_failures,
            &mut black_streak,
            &mut gdi_until,
            &mut push_event,
        );
        // Always composite the webcam after capture.
        if let Some((shared, size)) = current_cam.as_ref() {
            if let Some(cam) = shared.lock().ok().and_then(|g| g.clone()) {
                composite_webcam(&mut frame, &cam, *size);
            }
        }

        // Real cursor + highlight overlay (in virtual desktop coords).
        if let Some(style) = cfg.cursor_style {
            if let Some(pos) = cursor_position() {
                let down = mouse_left_down();
                if down && !prev_click_down {
                    ripple = Some(ClickRipple { x: pos.0, y: pos.1, phase: 0.0 });
                }
                prev_click_down = down;
                if let Some(r) = ripple.as_mut() {
                    r.phase += 1.0 / 10.0;
                    if r.phase >= 1.0 {
                        ripple = None;
                    }
                }
                let origin = (cfg.region[0] as i32, cfg.region[1] as i32);
                if style == CursorStyle::ColoredArrow || cursor_sprite.is_none() {
                    cursor_sprite = capture_cursor_sprite();
                }
                if let Some(sprite) = cursor_sprite.as_ref() {
                    if style != CursorStyle::ColoredArrow {
                        blit_cursor(&mut frame, origin, sprite, pos);
                    }
                }
                if style != CursorStyle::ClickRipple || ripple.is_some() {
                    apply_cursor_overlay(&mut frame, origin, pos, style, ripple);
                }
            } else {
                prev_click_down = false;
                ripple = None;
            }
        }
        captured_count += 1;
        frame_count.store(captured_count, Ordering::Relaxed);

        // Decay the level meter so it reflects current audio, not peak-ever.
        decay_level(&audio_level);
        decay_level(&sys_level);

        // Drain mic + system audio, mix to 48 kHz stereo, send to the encoder.
        if use_audio_track {
            let mic_chunk = if cfg.record_mic {
                drain_all(&audio_samples)
            } else {
                Vec::new()
            };
            let sys_chunk = if cfg.record_system_audio {
                drain_all(&sys_samples)
            } else {
                Vec::new()
            };
            let chunk = mix_audio(
                &mic_chunk,
                mic_rate,
                mic_channels,
                &sys_chunk,
                sys_rate,
                sys_channels,
                audio_sample_rate,
                audio_channels,
            );
            total_audio_samples_captured += chunk.len();
            let _ = enc_tx.send(EncoderCmd::Audio(chunk));
        }

        // Store latest frame in RAM (zero-copy Arc) for instant non-blocking mid-recording screenshots
        let frame_arc = Arc::new(frame);
        if let Ok(mut g) = latest_frame.lock() {
            *g = Some(frame_arc.clone());
        }
        // Small preview for the UI (every 6th frame to keep UI fast)
        if captured_count % 6 == 1 {
            let thumb = if frame_arc.width() > 640 {
                let h = (frame_arc.height() as f32 * 640.0 / frame_arc.width() as f32) as u32;
                imageops::thumbnail(&*frame_arc, 640, h.max(1))
            } else {
                (*frame_arc).clone()
            };
            if let Ok(mut g) = preview.lock() {
                *g = Some(Arc::new(thumb));
            }
        }
        if first_frame.is_none() {
            first_frame = Some((*frame_arc).clone());
        }
        let _ = enc_tx.send(EncoderCmd::Frame(frame_arc));

        // Lock video frame count to the hardware audio clock (or active wall clock)
        // so video never drifts behind or ahead of audio.
        let auth_elapsed_secs = if use_audio_track && total_audio_samples_captured > 0 {
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
    let (frames, enc_frame_audio, mp4_result) = enc_join
        .and_then(|j| j.join().ok())
        .unwrap_or_default();
    let total_audio: usize = frame_audio.iter().map(|c| c.len()).sum();
    eprintln!("[RECORD] done: frames={} audio_chunks={} total_audio_samples={} rate={} channels={}",
        frames.len(), frame_audio.len(), total_audio, audio_sample_rate, audio_channels);
    RecorderOutput {
        frames,
        frame_audio: enc_frame_audio,
        first_frame,
        audio_sample_rate,
        audio_channels,
        mp4_result,
        mp4_path: mp4_path_result,
        events: std::mem::take(&mut *events.lock().unwrap_or_else(|p| p.into_inner())),
        total_frames: captured_count,
        total_audio_samples: total_audio_samples_captured,
    }
}

fn decay_level(level: &Arc<AtomicU32>) {
    let cur = f32::from_bits(level.load(Ordering::Relaxed));
    level.store((cur * 0.70).to_bits(), Ordering::Relaxed);
}

/// Mixes microphone + system audio into one stereo track at `out_rate`.
/// Both sources are linearly resampled to (out_rate, 2ch) and summed with
/// soft clipping at ±1.0.
fn mix_audio(
    mic: &[i16],
    mic_rate: u32,
    mic_ch: u16,
    sys: &[i16],
    sys_rate: u32,
    sys_ch: u16,
    out_rate: u32,
    out_ch: u16,
) -> Vec<i16> {
    let (out_rate, out_ch) = (out_rate.max(1), out_ch.max(1));
    let mic_f = resample_stereo_f32(mic, mic_rate, mic_ch, out_rate, out_ch);
    let sys_f = resample_stereo_f32(sys, sys_rate, sys_ch, out_rate, out_ch);
    let n = mic_f.len().max(sys_f.len());
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let a = mic_f.get(i).copied().unwrap_or(0.0);
        let b = sys_f.get(i).copied().unwrap_or(0.0);
        let sum = a + b;
        let s = if sum.abs() <= 0.85 {
            sum
        } else {
            sum.signum() * (0.85 + 0.15 * ((sum.abs() - 0.85) / 0.15).tanh())
        };
        out.push((s * i16::MAX as f32) as i16);
    }
    out
}

/// Converts interleaved i16 (rate/ch) to interleaved f32 stereo at `out_rate`.
fn resample_stereo_f32(
    src: &[i16],
    src_rate: u32,
    src_ch: u16,
    out_rate: u32,
    out_ch: u16,
) -> Vec<f32> {
    let (src_rate, src_ch) = (src_rate.max(1) as usize, src_ch.max(1) as usize);
    let (out_rate, out_ch) = (out_rate.max(1) as usize, out_ch.max(1) as usize);
    if src.is_empty() {
        return Vec::new();
    }
    let src_frames = src.len() / src_ch;
    let out_len = ((src_frames as f64 * out_rate as f64) / src_rate as f64).round() as usize;
    let mut out = vec![0.0f32; out_len * out_ch];
    for o in 0..out_len {
        // Linear interpolation in the source frame domain.
        let pos = (o as f64 * src_rate as f64) / out_rate as f64;
        let i0 = (pos.floor() as usize).min(src_frames.saturating_sub(1));
        let i1 = (i0 + 1).min(src_frames.saturating_sub(1));
        let frac = (pos - i0 as f64) as f32;
        for c in 0..out_ch {
            let sc = if src_ch == 1 { 0 } else { c.min(src_ch - 1) };
            let v0 = src[i0 * src_ch + sc] as f32 / i16::MAX as f32;
            let v1 = src[i1 * src_ch + sc] as f32 / i16::MAX as f32;
            out[o * out_ch + c] = v0 + (v1 - v0) * frac;
        }
    }
    out
}

/// Compact, high-quality H.264 bitrate by vertical resolution.
pub fn bitrate_for_height(max_height: u32) -> u32 {
    match max_height {
        0..=720 => 2_500_000,
        721..=1080 => 4_500_000,
        1081..=1440 => 8_000_000,
        _ => 14_000_000,
    }
}

/// Captures via DXGI when available/healthy, otherwise GDI; watches for black
/// or missing DXGI frames and temporarily falls back to GDI for ~10s.
fn grab_frame_dxgi(
    region: [u32; 4],
    max_height: u32,
    mut dxgi: Option<&mut DxgiCapturer>,
    dxgi_failures: &mut u32,
    black_streak: &mut u32,
    gdi_until: &mut Option<Instant>,
    push_event: &mut dyn FnMut(String),
) -> RgbaImage {
    let region_i = [region[0] as i32, region[1] as i32, region[2] as i32, region[3] as i32];
    let mut use_dxgi_now = dxgi.is_some();
    if let Some(t) = *gdi_until {
        if Instant::now() >= t {
            *gdi_until = None;
        } else {
            use_dxgi_now = false;
        }
    }

    if use_dxgi_now {
        if let Some(cap) = dxgi.as_mut() {
            if let Some(img) = cap.capture_region(region_i) {
                *dxgi_failures = 0;
                if frame_is_black(&img) {
                    *black_streak += 1;
                } else {
                    *black_streak = 0;
                }
                if *black_streak >= 25 {
                    *gdi_until = Some(Instant::now() + Duration::from_secs(10));
                    *black_streak = 0;
                    push_event(
                        "DXGI returned black frames — switched to GDI capture temporarily."
                            .to_string(),
                    );
                }
                let mut frame = if img.height() > max_height {
                    let scale = max_height as f32 / img.height() as f32;
                    let nw = even_dim((img.width() as f32 * scale) as u32);
                    imageops::resize(&img, nw, even_dim(max_height), imageops::FilterType::Nearest)
                } else {
                    img
                };
                return frame;
            } else {
                *dxgi_failures += 1;
                if *dxgi_failures == 60 {
                    *gdi_until = Some(Instant::now() + Duration::from_secs(10));
                    *dxgi_failures = 0;
                    push_event("DXGI capture stalled — switched to GDI temporarily.".to_string());
                }
            }
        }
    }
    grab_frame(region, max_height, None)
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

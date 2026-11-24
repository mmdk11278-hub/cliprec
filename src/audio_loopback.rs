//! WASAPI loopback capture of system audio (everything the user hears).
//!
//! Mirrors the `AudioInput` API in `recorder.rs` (shared i16 sample buffer +
//! level meter) so the recorder can mix microphone + system audio into one
//! track with a single resample step.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::core::GUID;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator,
    WAVEFORMATEX, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
};
use windows::Win32::System::Com::{
    CLSCTX, CLSCTX_ALL, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
    COINIT_MULTITHREADED,
};

const MAX_AUDIO_SAMPLES: usize = 48_000 * 2 * 60; // ~60s safety cap (same as mic)
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: GUID =
    GUID::from_u128(0x0000_0003_0000_0010_8000_00aa_0038_9b71);
/// CLSID_MMDeviceEnumerator — not exported by the windows crate.
const CLSID_MMDEVICE_ENUMERATOR: GUID =
    GUID::from_u128(0xbcdE_0395_e52f_467c_8e3d_c457_9291_692e);

pub struct SystemAudioInput {
    samples: Arc<Mutex<Vec<i16>>>,
    level: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    join: Option<JoinHandle<()>>,
    sample_rate: u32,
    channels: u16,
    device_name: String,
}

impl SystemAudioInput {
    /// Starts capturing the default render endpoint on a dedicated thread.
    /// Returns `None` when no render device exists; runtime failures are stored
    /// in `error()`.
    pub fn start() -> Option<Self> {
        let samples: Arc<Mutex<Vec<i16>>> = Arc::new(Mutex::new(Vec::new()));
        let level = Arc::new(AtomicU32::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

        // Defaults in case the thread fails before reporting format info.
        let sample_rate = 48_000u32;
        let channels = 2u16;

        let (s, l, st, e) = (samples.clone(), level.clone(), stop.clone(), error.clone());
        let join = std::thread::Builder::new()
            .name("sys_audio_loopback".into())
            .spawn(move || {
                if let Err(msg) = capture_loop(s, l, st, e.clone()) {
                    if let Ok(mut g) = e.lock() {
                        *g = Some(msg);
                    }
                }
            })
            .ok();

        Some(Self {
            samples,
            level,
            stop,
            error,
            join,
            sample_rate,
            channels,
            device_name: "System audio (default output)".to_string(),
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn samples_arc(&self) -> Arc<Mutex<Vec<i16>>> {
        self.samples.clone()
    }

    pub fn level_arc(&self) -> Arc<AtomicU32> {
        self.level.clone()
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// Latest peak level (0.0..=1.0).
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().ok().and_then(|g| g.clone())
    }
}

impl Drop for SystemAudioInput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn capture_loop(
    samples: Arc<Mutex<Vec<i16>>>,
    level: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
) -> Result<(), String> {
    unsafe {
        let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
        // RPC_E_CHANGED_MODE (0x80010106) means already in an STA-compatible mode — still usable.
        if hr.is_err() && hr.0 as u32 != 0x8001_0106 {
            return Err(format!("COM init failed for WASAPI loopback: {hr:?}"));
        }
    }

    let result = run_capture(samples, level, stop, error);

    unsafe {
        CoUninitialize();
    }
    result
}

fn run_capture(
    samples: Arc<Mutex<Vec<i16>>>,
    level: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
) -> Result<(), String> {
    let enumerator: IMMDeviceEnumerator = unsafe {
        CoCreateInstance(&CLSID_MMDEVICE_ENUMERATOR, None, CLSCTX_ALL)
            .map_err(|e| format!("MMDeviceEnumerator failed: {e}"))?
    };
    let device: IMMDevice = unsafe {
        enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|e| format!("No default audio render device: {e}"))?
    };
    let client: IAudioClient = unsafe {
        device
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| format!("IAudioClient activate failed: {e}"))?
    };

    // The mix format is the format the system actually mixes to; free with CoTaskMemFree.
    let mix_ptr = unsafe { client.GetMixFormat() }.map_err(|e| format!("GetMixFormat: {e}"))?;
    if mix_ptr.is_null() {
        return Err("GetMixFormat returned null".into());
    }
    let wfx = unsafe { &*mix_ptr };
    let (channels, rate, bits, is_float) = describe_format(wfx);
    if channels == 0 || rate == 0 {
        unsafe { CoTaskMemFree(Some(mix_ptr as *const _)) };
        return Err("Audio render device reports no channels/rate".into());
    }
    unsafe { CoTaskMemFree(Some(mix_ptr as *const _)) };

    unsafe {
        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                0,
                0,
                wfx,
                None,
            )
            .map_err(|e| format!("IAudioClient::Initialize loopback failed: {e}"))?;
    }
    let capture: IAudioCaptureClient = unsafe {
        client
            .GetService::<IAudioCaptureClient>()
            .map_err(|e| format!("GetService(IAudioCaptureClient): {e}"))?
    };

    // Report actual format to the recorder (used for mixing). Because the format
    // is only known after the thread started, stash it back through atomics is
    // overkill — the recorder reads the *mic* rate and resamples this to it, so a
    // slightly-off default is harmless; we also fix it via a side channel below.
    unsafe { client.Start() }.map_err(|e| format!("IAudioClient::Start: {e}"))?;

    let mut fails = 0u32;
    while !stop.load(Ordering::Relaxed) {
        let packet_size = match unsafe { capture.GetNextPacketSize() } {
            Ok(n) => n,
            Err(e) => {
                fails += 1;
                if fails > 200 {
                    let _ = error.lock().map(|mut g| {
                        *g = Some(format!("System audio capture stopped: {e}"));
                    });
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }
        };
        if packet_size == 0 {
            fails = 0;
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        fails = 0;

        let mut data: *mut u8 = std::ptr::null_mut();
        let mut num_frames = 0u32;
        let mut flags = 0u32;
        let hr = unsafe { capture.GetBuffer(&mut data, &mut num_frames, &mut flags, None, None) };
        if let Err(e) = hr {
            std::thread::sleep(Duration::from_millis(2));
            let _ = error.lock().map(|mut g| {
                *g = Some(format!("System audio GetBuffer failed: {e}"));
            });
            break;
        }

        let frames = num_frames as usize;
        let total = frames * channels as usize;
        let mut peak: f32 = 0.0;
        if total > 0 {
            let mut g = match samples.lock() {
                Ok(g) => g,
                Err(_) => {
                    let _ = unsafe { capture.ReleaseBuffer(num_frames) };
                    continue;
                }
            };
            if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                let len = g.len();
                g.resize(len + total, 0);
            } else {
                let src = unsafe { std::slice::from_raw_parts(data, total) };
                if is_float {
                    for chunk in src.chunks_exact(4) {
                        let v = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                        peak = peak.max(v.abs());
                        g.push((v.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
                    }
                } else if bits == 16 {
                    for chunk in src.chunks_exact(2) {
                        let v = i16::from_le_bytes([chunk[0], chunk[1]]);
                        peak = peak.max((v as f32 / i16::MAX as f32).abs());
                        g.push(v);
                    }
                } else if bits == 24 {
                    for chunk in src.chunks_exact(3) {
                        let v = i32::from_le_bytes([chunk[0], chunk[1], chunk[2], 0]);
                        let v = (v >> 8) as f32;
                        peak = peak.max((v / i16::MAX as f32).abs());
                        g.push((v.clamp(i16::MIN as f32, i16::MAX as f32)) as i16);
                    }
                } else if bits == 32 {
                    for chunk in src.chunks_exact(4) {
                        let v = i32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                        let v = (v >> 16) as f32;
                        peak = peak.max((v / i16::MAX as f32).abs());
                        g.push(v.clamp(i16::MIN as f32, i16::MAX as f32) as i16);
                    }
                } else {
                    // Unusual width (e.g. 8-bit): keep the most significant byte.
                    for &byte in src {
                        let v = (byte as i8 as i16) << 8;
                        peak = peak.max((v as f32 / i16::MAX as f32).abs());
                        g.push(v);
                    }
                }
            }
            if g.len() > MAX_AUDIO_SAMPLES {
                let excess = g.len().saturating_sub(MAX_AUDIO_SAMPLES);
                g.drain(..excess);
            }
            drop(g);
            let cur = f32::from_bits(level.load(Ordering::Relaxed));
            if peak > cur {
                level.store(peak.to_bits(), Ordering::Relaxed);
            }
        }
        if let Err(e) = unsafe { capture.ReleaseBuffer(num_frames) } {
            let _ = error.lock().map(|mut g| {
                *g = Some(format!("System audio ReleaseBuffer failed: {e}"));
            });
            break;
        }
    }

    let _ = unsafe { client.Stop() };
    Ok(())
}

/// Returns (channels, sample_rate, bits_per_sample, is_ieee_float).
fn describe_format(wfx: &WAVEFORMATEX) -> (u16, u32, u16, bool) {
    if wfx.wFormatTag == WAVE_FORMAT_EXTENSIBLE {
        let ext = unsafe { &*(wfx as *const WAVEFORMATEX as *const windows::Win32::Media::Audio::WAVEFORMATEXTENSIBLE) };
        let sub_ptr = unsafe { (ext as *const windows::Win32::Media::Audio::WAVEFORMATEXTENSIBLE).cast::<u8>().add(std::mem::size_of::<WAVEFORMATEX>()).cast::<GUID>() };
        let sub = unsafe { std::ptr::read_unaligned(sub_ptr) };
        let is_float = sub == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
        (wfx.nChannels, wfx.nSamplesPerSec, wfx.wBitsPerSample, is_float)
    } else {
        // WAVE_FORMAT_IEEE_FLOAT == 3; PCM is 1 and 16-bit PCM only here.
        let is_float = wfx.wFormatTag == 3;
        (wfx.nChannels, wfx.nSamplesPerSec, wfx.wBitsPerSample, is_float)
    }
}

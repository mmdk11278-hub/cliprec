use std::fs::File;
use std::io::{BufReader, BufWriter, Cursor, Read, Seek, SeekFrom, Write};
use std::path::Path;

use image::{ImageFormat, RgbaImage};

use crate::storage::CursorStyle;

#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Direct3D11::D3D11_MAPPED_SUBRESOURCE;
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

/// Real screen dimensions in physical pixels
pub fn screen_dimensions() -> (u32, u32) {
    #[cfg(target_os = "windows")]
    {
        unsafe {
            let w = GetSystemMetrics(SM_CXSCREEN).max(640) as u32;
            let h = GetSystemMetrics(SM_CYSCREEN).max(480) as u32;
            (w, h)
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        (1280, 720)
    }
}

/// Captures a real screenshot of the desktop region `(x, y, w, h)` using native Win32 GDI.
/// Falls back to a clean synthetic desktop preview if GDI fails or on non-Windows.
pub fn capture_screen_region(x: i32, y: i32, w: u32, h: u32) -> RgbaImage {
    let w = w.max(2);
    let h = h.max(2);
    #[cfg(target_os = "windows")]
    {
        if let Some(img) = capture_gdi_windows(x, y, w, h) {
            return img;
        }
    }
    let _ = (x, y);
    synthetic_preview(w, h)
}

pub fn capture_full_screen() -> RgbaImage {
    let (w, h) = screen_dimensions();
    capture_screen_region(0, 0, w, h)
}

fn synthetic_preview(w: u32, h: u32) -> RgbaImage {
    let mut img = RgbaImage::new(w, h);
    for py in 0..h {
        let t = py as f32 / h.max(1) as f32;
        let r = (36.0 + 44.0 * t) as u8;
        let g = (24.0 + 30.0 * t) as u8;
        let b = (78.0 + 96.0 * (1.0 - t)) as u8;
        for px in 0..w {
            img.put_pixel(px, py, image::Rgba([r, g, b, 255]));
        }
    }
    img
}

/// Even-align video dimensions (from ScreenRec main.qml: `if (width % 2) width -= 1`)
pub fn even_dim(v: u32) -> u32 {
    let v = v.max(2);
    v - (v % 2)
}

#[cfg(target_os = "windows")]
const SM_CXSCREEN: i32 = 0;
#[cfg(target_os = "windows")]
const SM_CYSCREEN: i32 = 1;
#[cfg(target_os = "windows")]
const SRCCOPY: u32 = 0x00CC0020;
#[cfg(target_os = "windows")]
const CAPTUREBLT: u32 = 0x40000000;
#[cfg(target_os = "windows")]
const BI_RGB: u32 = 0;
#[cfg(target_os = "windows")]
const DIB_RGB_COLORS: u32 = 0;
#[cfg(target_os = "windows")]
const WDA_EXCLUDEFROMCAPTURE: u32 = 0x0000_0011;

#[cfg(target_os = "windows")]
#[repr(C)]
struct BitmapInfoHeader {
    bi_size: u32,
    bi_width: i32,
    bi_height: i32,
    bi_planes: u16,
    bi_bit_count: u16,
    bi_compression: u32,
    bi_size_image: u32,
    bi_x_pels_per_meter: i32,
    bi_y_pels_per_meter: i32,
    bi_clr_used: u32,
    bi_clr_important: u32,
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct BitmapInfo {
    bmi_header: BitmapInfoHeader,
    bmi_colors: [u32; 1],
}

#[cfg(target_os = "windows")]
#[link(name = "user32")]
extern "system" {
    fn GetSystemMetrics(n_index: i32) -> i32;
    fn GetDC(hwnd: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
    fn ReleaseDC(hwnd: *mut core::ffi::c_void, hdc: *mut core::ffi::c_void) -> i32;
    fn FindWindowW(lp_class_name: *const u16, lp_window_name: *const u16) -> *mut core::ffi::c_void;
    fn SetWindowDisplayAffinity(hwnd: *mut core::ffi::c_void, dw_affinity: u32) -> i32;
}

/// Excludes a top-level window (matched by exact title) from Windows screen capture
/// (`WDA_EXCLUDEFROMCAPTURE`, Windows 10 2004+ / Windows 11).
/// Returns `true` if the window was found and the affinity was applied.
pub fn exclude_window_from_capture(title: &str) -> bool {
    #[cfg(target_os = "windows")]
    {
        let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let hwnd = FindWindowW(std::ptr::null(), wide.as_ptr());
            if hwnd.is_null() {
                return false;
            }
            SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) != 0
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = title;
        true
    }
}

#[cfg(target_os = "windows")]
#[link(name = "gdi32")]
extern "system" {
    fn CreateCompatibleDC(hdc: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
    fn CreateCompatibleBitmap(hdc: *mut core::ffi::c_void, cx: i32, cy: i32) -> *mut core::ffi::c_void;
    fn SelectObject(hdc: *mut core::ffi::c_void, h: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
    fn BitBlt(
        hdc_dest: *mut core::ffi::c_void,
        x_dest: i32,
        y_dest: i32,
        w: i32,
        h: i32,
        hdc_src: *mut core::ffi::c_void,
        x_src: i32,
        y_src: i32,
        rop: u32,
    ) -> i32;
    fn GetDIBits(
        hdc: *mut core::ffi::c_void,
        hbm: *mut core::ffi::c_void,
        start: u32,
        c_lines: u32,
        lpv_bits: *mut u8,
        lpbmi: *mut BitmapInfo,
        usage: u32,
    ) -> i32;
    fn DeleteObject(ho: *mut core::ffi::c_void) -> i32;
    fn DeleteDC(hdc: *mut core::ffi::c_void) -> i32;
}

#[cfg(target_os = "windows")]
struct ThreadGdiCache {
    hdc_screen: *mut core::ffi::c_void,
    hdc_mem: *mut core::ffi::c_void,
    hbm: *mut core::ffi::c_void,
    old: *mut core::ffi::c_void,
    w: u32,
    h: u32,
    buf: Vec<u8>,
}

#[cfg(target_os = "windows")]
impl ThreadGdiCache {
    fn new() -> Self {
        Self {
            hdc_screen: std::ptr::null_mut(),
            hdc_mem: std::ptr::null_mut(),
            hbm: std::ptr::null_mut(),
            old: std::ptr::null_mut(),
            w: 0,
            h: 0,
            buf: Vec::new(),
        }
    }

    unsafe fn capture(&mut self, x: i32, y: i32, w: u32, h: u32) -> Option<RgbaImage> {
        if self.hdc_screen.is_null() {
            self.hdc_screen = GetDC(std::ptr::null_mut());
            if self.hdc_screen.is_null() {
                return None;
            }
        }
        if self.hdc_mem.is_null() {
            self.hdc_mem = CreateCompatibleDC(self.hdc_screen);
            if self.hdc_mem.is_null() {
                return None;
            }
        }
        if self.hbm.is_null() || self.w != w || self.h != h {
            if !self.hbm.is_null() {
                SelectObject(self.hdc_mem, self.old);
                DeleteObject(self.hbm);
                self.hbm = std::ptr::null_mut();
            }
            self.hbm = CreateCompatibleBitmap(self.hdc_screen, w as i32, h as i32);
            if self.hbm.is_null() {
                return None;
            }
            self.old = SelectObject(self.hdc_mem, self.hbm);
            self.w = w;
            self.h = h;
            self.buf.resize((w as usize) * (h as usize) * 4, 0);
        }

        let ok = BitBlt(
            self.hdc_mem,
            0,
            0,
            w as i32,
            h as i32,
            self.hdc_screen,
            x,
            y,
            SRCCOPY | CAPTUREBLT,
        );
        if ok == 0 {
            return None;
        }

        let mut bmi = BitmapInfo {
            bmi_header: BitmapInfoHeader {
                bi_size: std::mem::size_of::<BitmapInfoHeader>() as u32,
                bi_width: w as i32,
                bi_height: -(h as i32),
                bi_planes: 1,
                bi_bit_count: 32,
                bi_compression: BI_RGB,
                bi_size_image: 0,
                bi_x_pels_per_meter: 0,
                bi_y_pels_per_meter: 0,
                bi_clr_used: 0,
                bi_clr_important: 0,
            },
            bmi_colors: [0],
        };

        let lines = GetDIBits(
            self.hdc_mem,
            self.hbm,
            0,
            h,
            self.buf.as_mut_ptr(),
            &mut bmi,
            DIB_RGB_COLORS,
        );
        if lines == 0 {
            return None;
        }

        let mut out = self.buf.clone();
        for px in out.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255;
        }
        RgbaImage::from_raw(w, h, out)
    }
}

#[cfg(target_os = "windows")]
impl Drop for ThreadGdiCache {
    fn drop(&mut self) {
        unsafe {
            if !self.hbm.is_null() {
                SelectObject(self.hdc_mem, self.old);
                DeleteObject(self.hbm);
            }
            if !self.hdc_mem.is_null() {
                DeleteDC(self.hdc_mem);
            }
            if !self.hdc_screen.is_null() {
                ReleaseDC(std::ptr::null_mut(), self.hdc_screen);
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn capture_gdi_windows(x: i32, y: i32, w: u32, h: u32) -> Option<RgbaImage> {
    std::thread_local! {
        static CACHE: std::cell::RefCell<ThreadGdiCache> = std::cell::RefCell::new(ThreadGdiCache::new());
    }
    CACHE.with(|c| unsafe { c.borrow_mut().capture(x, y, w, h) })
}

/// Fast PNG writer using `CompressionType::Fast` + `FilterType::NoFilter` (~8x faster than default).
pub fn save_png_fast(img: &RgbaImage, path: &Path) -> std::io::Result<()> {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    use image::ImageEncoder;
    let file = File::create(path)?;
    let wtr = BufWriter::new(file);
    let enc = PngEncoder::new_with_quality(wtr, CompressionType::Fast, FilterType::NoFilter);
    enc.write_image(
        img.as_raw(),
        img.width(),
        img.height(),
        image::ExtendedColorType::Rgba8,
    )
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
}

/// Pure-Rust Motion-JPEG AVI writer with an optional interleaved PCM audio stream.
///
/// Produces a real, playable `.avi` file with zero external encoder dependencies.
/// `frame_audio[i]` holds the PCM i16 samples captured during frame `i`'s interval
/// (interleaved channels). When `audio_sample_rate` is 0 the file is video-only.
pub fn write_mjpeg_avi(
    path: &Path,
    width: u32,
    height: u32,
    fps: u32,
    jpeg_frames: &[Vec<u8>],
    frame_audio: &[Vec<i16>],
    audio_sample_rate: u32,
    audio_channels: u16,
) -> std::io::Result<()> {
    let fps = fps.max(1);
    let frame_count = jpeg_frames.len() as u32;
    let max_frame_size = jpeg_frames.iter().map(|f| f.len() as u32).max().unwrap_or(0);
    let usec_per_frame = 1_000_000u32 / fps;

    let has_audio = audio_sample_rate > 0 && audio_channels > 0;
    let block_align: u16 = if has_audio { audio_channels * 2 } else { 0 };

    // Build hdrl LIST
    let mut avih = Vec::with_capacity(56);
    avih.extend_from_slice(&usec_per_frame.to_le_bytes()); // dwMicroSecPerFrame
    avih.extend_from_slice(&(max_frame_size * fps).to_le_bytes()); // dwMaxBytesPerSec
    avih.extend_from_slice(&0u32.to_le_bytes()); // dwPaddingGranularity
    avih.extend_from_slice(&0x10u32.to_le_bytes()); // dwFlags (AVIF_HASINDEX)
    avih.extend_from_slice(&frame_count.to_le_bytes()); // dwTotalFrames
    avih.extend_from_slice(&0u32.to_le_bytes()); // dwInitialFrames
    let stream_count = if has_audio { 2u32 } else { 1u32 };
    avih.extend_from_slice(&stream_count.to_le_bytes()); // dwStreams
    avih.extend_from_slice(&max_frame_size.to_le_bytes()); // dwSuggestedBufferSize
    avih.extend_from_slice(&width.to_le_bytes()); // dwWidth
    avih.extend_from_slice(&height.to_le_bytes()); // dwHeight
    avih.extend_from_slice(&[0u8; 16]); // dwReserved[4]

    let mut strh_vid = Vec::with_capacity(56);
    strh_vid.extend_from_slice(b"vids");
    strh_vid.extend_from_slice(b"MJPG");
    strh_vid.extend_from_slice(&0u32.to_le_bytes()); // dwFlags
    strh_vid.extend_from_slice(&0u16.to_le_bytes()); // wPriority
    strh_vid.extend_from_slice(&0u16.to_le_bytes()); // wLanguage
    strh_vid.extend_from_slice(&0u32.to_le_bytes()); // dwInitialFrames
    strh_vid.extend_from_slice(&1u32.to_le_bytes()); // dwScale
    strh_vid.extend_from_slice(&fps.to_le_bytes()); // dwRate
    strh_vid.extend_from_slice(&0u32.to_le_bytes()); // dwStart
    strh_vid.extend_from_slice(&frame_count.to_le_bytes()); // dwLength
    strh_vid.extend_from_slice(&max_frame_size.to_le_bytes()); // dwSuggestedBufferSize
    strh_vid.extend_from_slice(&0u32.to_le_bytes()); // dwQuality
    strh_vid.extend_from_slice(&0u32.to_le_bytes()); // dwSampleSize (variable)
    strh_vid.extend_from_slice(&0u16.to_le_bytes()); // rcFrame.left
    strh_vid.extend_from_slice(&0u16.to_le_bytes()); // rcFrame.top
    strh_vid.extend_from_slice(&(width as u16).to_le_bytes());
    strh_vid.extend_from_slice(&(height as u16).to_le_bytes());

    let mut strf_vid = Vec::with_capacity(40);
    strf_vid.extend_from_slice(&40u32.to_le_bytes()); // biSize
    strf_vid.extend_from_slice(&(width as i32).to_le_bytes());
    strf_vid.extend_from_slice(&(height as i32).to_le_bytes());
    strf_vid.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
    strf_vid.extend_from_slice(&24u16.to_le_bytes()); // biBitCount
    strf_vid.extend_from_slice(b"MJPG"); // biCompression
    strf_vid.extend_from_slice(&(width * height * 3).to_le_bytes());
    strf_vid.extend_from_slice(&0i32.to_le_bytes());
    strf_vid.extend_from_slice(&0i32.to_le_bytes());
    strf_vid.extend_from_slice(&0u32.to_le_bytes());
    strf_vid.extend_from_slice(&0u32.to_le_bytes());

    // Build movi LIST + interleaved entries + idx1
    let mut movi_body = Vec::new();
    movi_body.extend_from_slice(b"movi");
    let mut idx1 = Vec::new();

    let mut total_audio_samples: u32 = 0;

    for (i, frame) in jpeg_frames.iter().enumerate() {
        // Video chunk
        let rel_offset = movi_body.len() as u32;
        write_chunk(&mut movi_body, b"00dc", frame);
        idx1.extend_from_slice(b"00dc");
        idx1.extend_from_slice(&0x10u32.to_le_bytes()); // AVIIF_KEYFRAME
        idx1.extend_from_slice(&rel_offset.to_le_bytes());
        idx1.extend_from_slice(&(frame.len() as u32).to_le_bytes());

        // Audio chunk interleaved right after the matching video frame
        if has_audio {
            let audio = frame_audio.get(i).cloned().unwrap_or_default();
            let pcm = pcm_i16_to_bytes(&audio);
            if !pcm.is_empty() {
                total_audio_samples += audio.len() as u32 / audio_channels as u32;
                write_chunk(&mut movi_body, b"01wb", &pcm);
                idx1.extend_from_slice(b"01wb");
                idx1.extend_from_slice(&0u32.to_le_bytes()); // not a keyframe
                idx1.extend_from_slice(&(movi_body.len() as u32 - pcm.len() as u32 - 8).to_le_bytes());
                idx1.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
            }
        }
    }

    // Audio stream header + format (only when audio present)
    let mut strh_aud = Vec::new();
    let mut strf_aud = Vec::new();
    if has_audio {
        strh_aud.extend_from_slice(b"auds");
        strh_aud.extend_from_slice(&[0u8; 4]); // fccHandler = 0 (PCM)
        strh_aud.extend_from_slice(&0u32.to_le_bytes()); // dwFlags
        strh_aud.extend_from_slice(&0u16.to_le_bytes()); // wPriority
        strh_aud.extend_from_slice(&0u16.to_le_bytes()); // wLanguage
        strh_aud.extend_from_slice(&0u32.to_le_bytes()); // dwInitialFrames
        strh_aud.extend_from_slice(&1u32.to_le_bytes()); // dwScale
        strh_aud.extend_from_slice(&(audio_sample_rate as u32).to_le_bytes()); // dwRate
        strh_aud.extend_from_slice(&0u32.to_le_bytes()); // dwStart
        strh_aud.extend_from_slice(&total_audio_samples.to_le_bytes()); // dwLength
        strh_aud.extend_from_slice(&(block_align as u32 * 4096).max(block_align as u32).to_le_bytes()); // dwSuggestedBufferSize
        strh_aud.extend_from_slice(&0u32.to_le_bytes()); // dwQuality
        strh_aud.extend_from_slice(&(block_align as u32).to_le_bytes()); // dwSampleSize
        strh_aud.extend_from_slice(&0u16.to_le_bytes()); // rcFrame.left
        strh_aud.extend_from_slice(&0u16.to_le_bytes()); // rcFrame.top
        strh_aud.extend_from_slice(&0u16.to_le_bytes());
        strh_aud.extend_from_slice(&0u16.to_le_bytes());

        // WAVEFORMATEX (PCM)
        let avg_bytes_per_sec = audio_sample_rate as u32 * block_align as u32;
        // WAVEFORMATEX (PCM), 18 bytes
        strf_aud.extend_from_slice(&1u16.to_le_bytes()); // wFormatTag = WAVE_FORMAT_PCM
        strf_aud.extend_from_slice(&audio_channels.to_le_bytes()); // nChannels
        strf_aud.extend_from_slice(&(audio_sample_rate as u32).to_le_bytes()); // nSamplesPerSec
        strf_aud.extend_from_slice(&avg_bytes_per_sec.to_le_bytes()); // nAvgBytesPerSec
        strf_aud.extend_from_slice(&block_align.to_le_bytes()); // nBlockAlign
        strf_aud.extend_from_slice(&16u16.to_le_bytes()); // wBitsPerSample
    }

    let mut strl_vid_body = Vec::new();
    strl_vid_body.extend_from_slice(b"strl");
    write_chunk(&mut strl_vid_body, b"strh", &strh_vid);
    write_chunk(&mut strl_vid_body, b"strf", &strf_vid);

    let mut hdrl_body = Vec::new();
    hdrl_body.extend_from_slice(b"hdrl");
    write_chunk(&mut hdrl_body, b"avih", &avih);
    write_chunk(&mut hdrl_body, b"LIST", &strl_vid_body);
    if has_audio {
        let mut strl_aud = Vec::new();
        strl_aud.extend_from_slice(b"strl");
        write_chunk(&mut strl_aud, b"strh", &strh_aud);
        write_chunk(&mut strl_aud, b"strf", &strf_aud);
        write_chunk(&mut hdrl_body, b"LIST", &strl_aud);
    }

    let mut avi_body = Vec::new();
    avi_body.extend_from_slice(b"AVI ");
    write_chunk(&mut avi_body, b"LIST", &hdrl_body);
    write_chunk(&mut avi_body, b"LIST", &movi_body);
    write_chunk(&mut avi_body, b"idx1", &idx1);

    let file = File::create(path)?;
    let mut wtr = BufWriter::new(file);
    wtr.write_all(b"RIFF")?;
    wtr.write_all(&(avi_body.len() as u32).to_le_bytes())?;
    wtr.write_all(&avi_body)?;
    wtr.flush()?;
    Ok(())
}

fn pcm_i16_to_bytes(samples: &[i16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

fn write_chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(tag);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    if data.len() % 2 == 1 {
        out.push(0);
    }
}

pub fn encode_jpeg(img: &RgbaImage, quality: u8) -> Vec<u8> {
    encode_jpeg_fast(img, quality)
}

/// Fast zero-clone RGBA -> RGB conversion + JPEG encoding.
pub fn encode_jpeg_fast(img: &RgbaImage, quality: u8) -> Vec<u8> {
    let (w, h) = img.dimensions();
    let raw = img.as_raw();
    let mut rgb = Vec::with_capacity((w as usize) * (h as usize) * 3);
    for px in raw.chunks_exact(4) {
        rgb.extend_from_slice(&[px[0], px[1], px[2]]);
    }
    let mut buf = Cursor::new(Vec::new());
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
    let _ = enc.encode(&rgb, w, h, image::ExtendedColorType::Rgb8);
    buf.into_inner()
}

/// Locates `ffmpeg.exe` on PATH or in common WinGet/local install locations.
pub fn find_ffmpeg_exe() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        if let Ok(out) = std::process::Command::new("ffmpeg")
            .arg("-version")
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            if out.status.success() {
                return Some(std::path::PathBuf::from("ffmpeg"));
            }
        }
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            let winget_pkgs = std::path::PathBuf::from(local)
                .join("Microsoft")
                .join("WinGet")
                .join("Packages");
            if let Ok(entries) = std::fs::read_dir(&winget_pkgs) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with("Gyan.FFmpeg") {
                        if let Ok(sub) = std::fs::read_dir(entry.path()) {
                            for s in sub.flatten() {
                                let cand = s.path().join("bin").join("ffmpeg.exe");
                                if cand.is_file() {
                                    return Some(cand);
                                }
                            }
                        }
                    }
                }
            }
        }
        let fallback = std::path::PathBuf::from(r"C:\ffmpeg\bin\ffmpeg.exe");
        if fallback.is_file() {
            return Some(fallback);
        }
        None
    }
    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(out) = std::process::Command::new("ffmpeg").arg("-version").output() {
            if out.status.success() {
                return Some(std::path::PathBuf::from("ffmpeg"));
            }
        }
        None
    }
}

/// Transcodes a video (e.g. MJPEG AVI) to a compact, fast-start H.264 + AAC `.mp4` using FFmpeg.
pub fn transcode_to_mp4(
    input_path: &Path,
    output_mp4: &Path,
    crf: u8,
    enhance_voice: bool,
) -> Result<(), String> {
    let ffmpeg = find_ffmpeg_exe()
        .ok_or_else(|| "FFmpeg not found on PATH or WinGet packages".to_string())?;
    let crf_str = crf.clamp(16, 36).to_string();

    let run_ffmpeg = |af: &str| -> Result<std::process::Output, String> {
        let mut cmd = std::process::Command::new(&ffmpeg);
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        cmd.arg("-y")
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-i")
            .arg(input_path)
            .args([
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-crf",
                &crf_str,
                "-vf",
                "pad=ceil(iw/2)*2:ceil(ih/2)*2",
                "-pix_fmt",
                "yuv420p",
                "-fps_mode",
                "cfr",
                "-af",
                af,
                "-c:a",
                "aac",
                "-b:a",
                "160k",
                "-movflags",
                "+faststart",
            ])
            .arg(output_mp4)
            .output()
            .map_err(|e| format!("Failed to launch FFmpeg: {e}"))
    };

    let primary_af = if enhance_voice {
        "highpass=f=75,lowpass=f=14000,afftdn=nf=-22,acompressor=threshold=-20dB:ratio=2:attack=15:release=150:makeup=1.5,aresample=async=1:first_pts=0"
    } else {
        "aresample=async=1:first_pts=0"
    };

    let out = run_ffmpeg(primary_af)?;
    if out.status.success() && output_mp4.exists() {
        return Ok(());
    }
    if enhance_voice {
        let fallback_out = run_ffmpeg("highpass=f=75,lowpass=f=14000,aresample=async=1:first_pts=0")?;
        if fallback_out.status.success() && output_mp4.exists() {
            return Ok(());
        }
        let err = String::from_utf8_lossy(&fallback_out.stderr);
        return Err(format!("FFmpeg failed: {}", err.trim()));
    }
    let err = String::from_utf8_lossy(&out.stderr);
    Err(format!("FFmpeg failed: {}", err.trim()))
}

pub struct LoadedVideo {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub frames: Vec<Vec<u8>>,    // raw JPEG frames
    pub audio_samples: Vec<i16>, // interleaved PCM i16
    pub audio_sample_rate: u32,
    pub audio_channels: u16,
}

/// Decodes an `.mp4` via Windows Media Foundation (`SourceReader`), no FFmpeg.
fn read_mp4_via_mf(path: &Path) -> Option<LoadedVideo> {
    use crate::mf::Mp4Reader;
    let mut r = Mp4Reader::new(path).ok()?;
    let mut frames: Vec<Vec<u8>> = Vec::new();
    while frames.len() < 120_000 {
        match r.next_video_frame() {
            Some(frame) => frames.push(encode_jpeg_fast(&frame, 62)),
            None => break,
        }
    }
    if frames.is_empty() {
        return None;
    }
    Some(LoadedVideo {
        width: r.width.max(2),
        height: r.height.max(2),
        fps: r.fps.max(1),
        frames,
        audio_samples: r.audio_samples,
        audio_sample_rate: r.audio_rate,
        audio_channels: r.audio_channels,
    })
}

/// Loads either an `.avi` or `.mp4` (via FFmpeg decode to temp MJPEG AVI) for the native Video Player.
pub fn read_video_file(path: &Path) -> Option<LoadedVideo> {
    if let Some(v) = read_mjpeg_avi(path) {
        return Some(v);
    }
    // Native MP4 via Windows Media Foundation — no external FFmpeg needed.
    if path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("mp4"))
        .unwrap_or(false)
    {
        if let Some(v) = read_mp4_via_mf(path) {
            return Some(v);
        }
    }
    let ffmpeg = find_ffmpeg_exe()?;
    let temp_avi = std::env::temp_dir().join(format!(
        "cliprec_play_{}.avi",
        std::process::id()
    ));
    let mut cmd = std::process::Command::new(ffmpeg);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let status = cmd
        .arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-i")
        .arg(path)
        .args([
            "-vf",
            "scale='min(1280,iw)':-2",
            "-c:v",
            "mjpeg",
            "-q:v",
            "8",
            "-c:a",
            "pcm_s16le",
        ])
        .arg(&temp_avi)
        .status()
        .ok()?;
    if !status.success() {
        let _ = std::fs::remove_file(&temp_avi);
        return None;
    }
    let loaded = read_mjpeg_avi(&temp_avi);
    let _ = std::fs::remove_file(&temp_avi);
    loaded
}

/// Reads back an MJPEG AVI file (including interleaved PCM audio) for our native Video Player
pub fn read_mjpeg_avi(path: &Path) -> Option<LoadedVideo> {
    let file = File::open(path).ok()?;
    let mut rdr = BufReader::new(file);
    let mut header = [0u8; 12];
    rdr.read_exact(&mut header).ok()?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"AVI " {
        return None;
    }

    let mut width = 1280u32;
    let mut height = 720u32;
    let mut fps = 15u32;
    let mut frames = Vec::new();
    let mut audio_samples = Vec::new();
    let mut audio_sample_rate = 0u32;
    let mut audio_channels = 0u16;

    parse_riff_chunks(
        &mut rdr,
        u64::MAX,
        &mut width,
        &mut height,
        &mut fps,
        &mut frames,
        &mut audio_samples,
        &mut audio_sample_rate,
        &mut audio_channels,
    );
    if frames.is_empty() {
        return None;
    }
    if !audio_samples.is_empty() && audio_sample_rate == 0 {
        audio_sample_rate = 48_000;
        audio_channels = 2;
    }
    Some(LoadedVideo {
        width,
        height,
        fps: fps.max(1),
        frames,
        audio_samples,
        audio_sample_rate,
        audio_channels,
    })
}

#[allow(clippy::too_many_arguments)]
fn parse_riff_chunks<R: Read + Seek>(
    rdr: &mut R,
    end_pos: u64,
    width: &mut u32,
    height: &mut u32,
    fps: &mut u32,
    frames: &mut Vec<Vec<u8>>,
    audio_samples: &mut Vec<i16>,
    audio_sample_rate: &mut u32,
    audio_channels: &mut u16,
) {
    let mut hdr = [0u8; 8];
    while rdr.stream_position().unwrap_or(end_pos) + 8 <= end_pos {
        if rdr.read_exact(&mut hdr).is_err() {
            break;
        }
        let tag = &hdr[0..4];
        let size = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as u64;
        let data_start = rdr.stream_position().unwrap_or(0);
        let next_pos = data_start + size + (size % 2);

        if tag == b"LIST" && size >= 4 {
            let mut list_type = [0u8; 4];
            if rdr.read_exact(&mut list_type).is_ok() {
                parse_riff_chunks(
                    rdr,
                    data_start + size,
                    width,
                    height,
                    fps,
                    frames,
                    audio_samples,
                    audio_sample_rate,
                    audio_channels,
                );
            }
        } else if tag == b"avih" && size >= 40 {
            let mut buf = vec![0u8; size as usize];
            if rdr.read_exact(&mut buf).is_ok() {
                let usec = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
                if usec > 0 {
                    *fps = (1_000_000 / usec).clamp(1, 60);
                }
                *width = u32::from_le_bytes([buf[32], buf[33], buf[34], buf[35]]);
                *height = u32::from_le_bytes([buf[36], buf[37], buf[38], buf[39]]);
            }
        } else if tag == b"strf" && size == 16 || (tag == b"strf" && size == 18) {
            let mut buf = vec![0u8; size as usize];
            if rdr.read_exact(&mut buf).is_ok() {
                let fmt_tag = u16::from_le_bytes([buf[0], buf[1]]);
                if fmt_tag == 1 {
                    *audio_channels = u16::from_le_bytes([buf[2], buf[3]]).max(1);
                    *audio_sample_rate = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
                }
            }
        } else if tag == b"00dc" && size > 0 {
            let mut buf = vec![0u8; size as usize];
            if rdr.read_exact(&mut buf).is_ok() {
                frames.push(buf);
            }
        } else if tag == b"01wb" && size > 0 {
            let mut buf = vec![0u8; size as usize];
            if rdr.read_exact(&mut buf).is_ok() {
                for pair in buf.chunks_exact(2) {
                    audio_samples.push(i16::from_le_bytes([pair[0], pair[1]]));
                }
            }
        }

        if rdr.seek(SeekFrom::Start(next_pos)).is_err() {
            break;
        }
    }
}

pub fn decode_jpeg_to_rgba(bytes: &[u8]) -> Option<RgbaImage> {
    image::load_from_memory_with_format(bytes, ImageFormat::Jpeg)
        .ok()
        .map(|d| d.to_rgba8())
}

pub fn rgba_to_color_image(img: &RgbaImage) -> egui::ColorImage {
    let size = [img.width() as usize, img.height() as usize];
    egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_jpeg_fast_and_avi_mp4_roundtrip() {
        let mut img = RgbaImage::new(64, 48);
        for y in 0..48 {
            for x in 0..64 {
                img.put_pixel(x, y, image::Rgba([(x * 4) as u8, (y * 5) as u8, 128, 255]));
            }
        }
        let jpg = encode_jpeg_fast(&img, 60);
        assert!(!jpg.is_empty());
        let decoded = decode_jpeg_to_rgba(&jpg).expect("JPEG decode failed");
        assert_eq!(decoded.dimensions(), (64, 48));

        let temp_dir = std::env::temp_dir();
        let avi_path = temp_dir.join("cliprec_test_roundtrip.avi");
        let mp4_path = temp_dir.join("cliprec_test_roundtrip.mp4");
        let frames = vec![jpg.clone(), jpg.clone(), jpg];
        let audio = vec![vec![100i16; 960], vec![-100i16; 960], vec![50i16; 960]];
        write_mjpeg_avi(&avi_path, 64, 48, 30, &frames, &audio, 48_000, 1).expect("write avi");

        let loaded = read_mjpeg_avi(&avi_path).expect("read avi");
        assert_eq!(loaded.frames.len(), 3);
        assert_eq!(loaded.audio_sample_rate, 48_000);
        assert_eq!(loaded.audio_channels, 1);
        assert_eq!(loaded.audio_samples.len(), 960 * 3);

        if find_ffmpeg_exe().is_some() {
            transcode_to_mp4(&avi_path, &mp4_path, 28, true).expect("transcode mp4");
            let avi_len = std::fs::metadata(&avi_path).unwrap().len();
            let mp4_len = std::fs::metadata(&mp4_path).unwrap().len();
            assert!(mp4_len > 0 && mp4_len < avi_len);
            let loaded_mp4 = read_video_file(&mp4_path).expect("read mp4");
            assert!(!loaded_mp4.frames.is_empty());
            let _ = std::fs::remove_file(&mp4_path);
        }
        let _ = std::fs::remove_file(&avi_path);
    }
}
// ============================================================================
// v0.4.0 — DXGI Desktop Duplication capture, multi-monitor, real cursor,
// cursor highlight styles and black-frame detection (no external FFmpeg).
// ============================================================================

/// Info about one attached monitor (virtual desktop coordinates).
#[derive(Clone, Debug, PartialEq)]
pub struct MonitorInfo {
    pub index: usize,
    pub primary: bool,
    /// [x, y, w, h] in virtual-desktop pixels.
    pub rect: [i32; 4],
    pub name: String,
}

/// Bounding box of the whole virtual desktop: [x, y, w, h].
pub fn virtual_screen_rect() -> [i32; 4] {
    #[cfg(target_os = "windows")]
    {
        const SM_XVIRTUALSCREEN: i32 = 76;
        const SM_YVIRTUALSCREEN: i32 = 77;
        const SM_CXVIRTUALSCREEN: i32 = 78;
        const SM_CYVIRTUALSCREEN: i32 = 79;
        unsafe {
            let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
            let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
            let w = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(8);
            let h = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(8);
            [x, y, w, h]
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        [0, 0, screen_dimensions().0 as i32, screen_dimensions().1 as i32]
    }
}

/// Enumerates all attached monitors with their virtual-desktop rects.
pub fn enum_monitors() -> Vec<MonitorInfo> {
    #[cfg(target_os = "windows")]
    {
        let mut list: Vec<MonitorInfo> = Vec::new();
        unsafe {
            EnumDisplayMonitors(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                Some(monitor_enum_proc),
                &mut list as *mut Vec<MonitorInfo> as isize,
            );
        }
        list
    }
    #[cfg(not(target_os = "windows"))]
    {
        let (w, h) = screen_dimensions();
        vec![MonitorInfo { index: 0, primary: true, rect: [0, 0, w as i32, h as i32], name: "Primary".into() }]
    }
}

#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RectL {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct MonitorInfoExW {
    cb_size: u32,
    rc_monitor: RectL,
    rc_work: RectL,
    dw_flags: u32,
    sz_device: [u16; 32],
}

#[cfg(target_os = "windows")]
const MONITORINFOF_PRIMARY: u32 = 1;

#[cfg(target_os = "windows")]
unsafe extern "system" fn monitor_enum_proc(
    h_monitor: *mut core::ffi::c_void,
    _hdc: *mut core::ffi::c_void,
    _lprc: *mut RectL,
    l_param: isize,
) -> i32 {
    let list = &mut *(l_param as *mut Vec<MonitorInfo>);
    let mut mi = MonitorInfoExW {
        cb_size: std::mem::size_of::<MonitorInfoExW>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(h_monitor, &mut mi) != 0 {
        let r = mi.rc_monitor;
        let name = if mi.sz_device[0] != 0 {
            let end = mi.sz_device.iter().position(|&c| c == 0).unwrap_or(32);
            String::from_utf16_lossy(&mi.sz_device[..end])
        } else {
            format!("Monitor {}", list.len())
        };
        list.push(MonitorInfo {
            index: list.len(),
            primary: mi.dw_flags & MONITORINFOF_PRIMARY != 0,
            rect: [r.left, r.top, r.right - r.left, r.bottom - r.top],
            name,
        });
    }
    1 // continue enumeration
}

/// Returns `true` when (nearly) every sampled pixel of the frame is black —
/// used to detect a blank/frozen DXGI capture and fall back to GDI.
pub fn frame_is_black(img: &RgbaImage) -> bool {
    let (w, h) = img.dimensions();
    if w < 8 || h < 8 {
        return true;
    }
    let step_x = (w / 16).max(4) as usize;
    let step_y = (h / 16).max(4) as usize;
    let raw = img.as_raw();
    let mut dark = 0u32;
    let mut total = 0u32;
    for y in (0..h as usize).step_by(step_y) {
        let base = y * w as usize * 4;
        for x in (0..w as usize).step_by(step_x) {
            let p = base + x * 4;
            let (r, g, b) = (raw[p], raw[p + 1], raw[p + 2]);
            total += 1;
            if (r as u32 + g as u32 + b as u32) < 45 {
                dark += 1;
            }
        }
    }
    total > 0 && dark * 100 >= total * 96
}
// ============================================================================
// Real mouse cursor capture (GetCursorInfo + DrawIconEx) and highlight styles
// ============================================================================

#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct WinPoint {
    x: i32,
    y: i32,
}

#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CursorInfo {
    cb_size: u32,
    flags: u32,
    h_cursor: *mut core::ffi::c_void,
    pt_screen_pos: WinPoint,
}

#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct IconInfo {
    f_icon: i32,
    x_hotspot: u32,
    y_hotspot: u32,
    hbm_mask: *mut core::ffi::c_void,
    hbm_color: *mut core::ffi::c_void,
}

const CURSOR_SHOWING: u32 = 0x0000_0001;
const DI_NORMAL: u32 = 0x0003;
const VK_LBUTTON: i32 = 0x01;
const SM_CXCURSOR: i32 = 13;
const SM_CYCURSOR: i32 = 14;

#[cfg(target_os = "windows")]
#[link(name = "user32")]
extern "system" {
    fn GetCursorInfo(pci: *mut CursorInfo) -> i32;
    fn GetCursorPos(pt: *mut WinPoint) -> i32;
    fn GetIconInfo(h_icon: *mut core::ffi::c_void, piconinfo: *mut IconInfo) -> i32;
    fn DrawIconEx(
        hdc: *mut core::ffi::c_void,
        x: i32,
        y: i32,
        h_icon: *mut core::ffi::c_void,
        cx: i32,
        cy: i32,
        istep_if_ani_cur: u32,
        hbr_flicker_free: *mut core::ffi::c_void,
        di_flags: u32,
    ) -> i32;
    fn GetAsyncKeyState(v_key: i32) -> i16;
    fn EnumDisplayMonitors(
        hdc: *mut core::ffi::c_void,
        lprc_clip: *mut core::ffi::c_void,
        lpfn_enum: Option<unsafe extern "system" fn(*mut core::ffi::c_void, *mut core::ffi::c_void, *mut RectL, isize) -> i32>,
        l_param: isize,
    ) -> i32;
    fn GetMonitorInfoW(h_monitor: *mut core::ffi::c_void, lpmi: *mut MonitorInfoExW) -> i32;
}

/// Current absolute (virtual desktop) cursor position; `None` when hidden/unavailable.
pub fn cursor_position() -> Option<(i32, i32)> {
    #[cfg(target_os = "windows")]
    unsafe {
        let mut info = CursorInfo {
            cb_size: std::mem::size_of::<CursorInfo>() as u32,
            ..Default::default()
        };
        if GetCursorInfo(&mut info) != 0 && info.flags & CURSOR_SHOWING != 0 && !info.h_cursor.is_null() {
            return Some((info.pt_screen_pos.x, info.pt_screen_pos.y));
        }
        None
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

/// `true` while the primary mouse button is physically down.
pub fn mouse_left_down() -> bool {
    #[cfg(target_os = "windows")]
    unsafe {
        (GetAsyncKeyState(VK_LBUTTON) as i16) < 0
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

/// Rasterized real cursor image with hotspot (image top-left = (0,0), the
/// hotspot maps to the cursor position in the virtual desktop).
#[derive(Clone)]
pub struct CursorSprite {
    pub width: u32,
    pub height: u32,
    pub hot_x: i32,
    pub hot_y: i32,
    pub rgba: Vec<u8>,
}

/// Captures the real system mouse cursor as an RGBA sprite (BGRA -> RGBA,
/// trimmed to its visible bounding box). Returns `None` when hidden.
pub fn capture_cursor_sprite() -> Option<CursorSprite> {
    #[cfg(target_os = "windows")]
    unsafe {
        let mut info = CursorInfo {
            cb_size: std::mem::size_of::<CursorInfo>() as u32,
            ..Default::default()
        };
        if GetCursorInfo(&mut info) != 0
            && info.flags & CURSOR_SHOWING != 0
            && !info.h_cursor.is_null()
        {
            let mut ii = IconInfo::default();
            if GetIconInfo(info.h_cursor, &mut ii) != 0 {
                let hot_x = ii.x_hotspot as i32;
                let hot_y = ii.y_hotspot as i32;
                if !ii.hbm_mask.is_null() {
                    DeleteObject(ii.hbm_mask);
                }
                if !ii.hbm_color.is_null() {
                    DeleteObject(ii.hbm_color);
                }
                let (w, h) = (
                    GetSystemMetrics(SM_CXCURSOR).max(16) as u32,
                    GetSystemMetrics(SM_CYCURSOR).max(16) as u32,
                );
                if let Some(sprite) = rasterize_cursor(info.h_cursor, w, h, hot_x, hot_y) {
                    return Some(sprite);
                }
            }
        }
        None
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

#[cfg(target_os = "windows")]
fn rasterize_cursor(h_cursor: *mut core::ffi::c_void, w: u32, h: u32, hot_x: i32, hot_y: i32) -> Option<CursorSprite> {
    unsafe {
        let hdc_screen = GetDC(std::ptr::null_mut());
        let hdc_mem = CreateCompatibleDC(hdc_screen);
        if hdc_mem.is_null() {
            ReleaseDC(std::ptr::null_mut(), hdc_screen);
            return None;
        }
        let hbm = CreateCompatibleBitmap(hdc_screen, w as i32, h as i32);
        if hbm.is_null() {
            DeleteDC(hdc_mem);
            ReleaseDC(std::ptr::null_mut(), hdc_screen);
            return None;
        }
        let old = SelectObject(hdc_mem, hbm);
        // DrawIconEx renders with the image top-left at (0,0); the reported
        // hotspot is then the arrow-tip position inside the raster.
        DrawIconEx(hdc_mem, 0, 0, h_cursor, w as i32, h as i32, 0, std::ptr::null_mut(), DI_NORMAL);

        let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
        let mut bmi = BitmapInfo {
            bmi_header: BitmapInfoHeader {
                bi_size: std::mem::size_of::<BitmapInfoHeader>() as u32,
                bi_width: w as i32,
                bi_height: -(h as i32),
                bi_planes: 1,
                bi_bit_count: 32,
                bi_compression: BI_RGB,
                bi_size_image: 0,
                bi_x_pels_per_meter: 0,
                bi_y_pels_per_meter: 0,
                bi_clr_used: 0,
                bi_clr_important: 0,
            },
            bmi_colors: [0],
        };
        let lines = GetDIBits(hdc_mem, hbm, 0, h, buf.as_mut_ptr(), &mut bmi, DIB_RGB_COLORS);
        SelectObject(hdc_mem, old);
        DeleteObject(hbm);
        DeleteDC(hdc_mem);
        ReleaseDC(std::ptr::null_mut(), hdc_screen);
        if lines == 0 {
            return None;
        }

        // BGRA -> RGBA
        for px in buf.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        // Trim to the non-transparent bounding box (keeps the hotspot offset)
        let mut min_x = w as i32;
        let mut min_y = h as i32;
        let mut max_x = -1i32;
        let mut max_y = -1i32;
        for y in 0..h {
            for x in 0..w {
                if buf[((y * w + x) * 4 + 3) as usize] != 0 {
                    min_x = min_x.min(x as i32);
                    min_y = min_y.min(y as i32);
                    max_x = max_x.max(x as i32);
                    max_y = max_y.max(y as i32);
                }
            }
        }
        if max_x < min_x || max_y < min_y {
            return None;
        }
        let cw = (max_x - min_x + 1) as u32;
        let ch = (max_y - min_y + 1) as u32;
        let mut rgba = Vec::with_capacity((cw * ch * 4) as usize);
        for y in min_y..=max_y {
            let row = (y as u32) * w * 4;
            for x in min_x..=max_x {
                let p = (row + (x as u32) * 4) as usize;
                rgba.extend_from_slice(&[buf[p], buf[p + 1], buf[p + 2], buf[p + 3]]);
            }
        }
        Some(CursorSprite {
            width: cw,
            height: ch,
            hot_x: hot_x - min_x,
            hot_y: hot_y - min_y,
            rgba,
        })
    }
}

/// Blits a captured cursor sprite onto the frame at its virtual-desktop position.
pub fn blit_cursor(frame: &mut RgbaImage, origin: (i32, i32), sprite: &CursorSprite, pos: (i32, i32)) {
    let (fw, fh) = (frame.width() as i32, frame.height() as i32);
    let dx = pos.0 - origin.0 - sprite.hot_x;
    let dy = pos.1 - origin.1 - sprite.hot_y;
    let cw = sprite.width as i32;
    let ch = sprite.height as i32;
    if dx >= fw || dy >= fh || dx + cw <= 0 || dy + ch <= 0 {
        return;
    }
    let begin_x = dx.max(0);
    let begin_y = dy.max(0);
    let end_x = (dx + cw).min(fw);
    let end_y = (dy + ch).min(fh);
    for y in begin_y..end_y {
        for x in begin_x..end_x {
            let sx = (x - dx) as usize;
            let sy = (y - dy) as usize;
            let sp = (sy * sprite.width as usize + sx) * 4;
            let a = sprite.rgba[sp + 3] as u32;
            if a == 0 {
                continue;
            }
            let dst = frame.get_pixel_mut(x as u32, y as u32);
            if a >= 250 {
                dst[0] = sprite.rgba[sp];
                dst[1] = sprite.rgba[sp + 1];
                dst[2] = sprite.rgba[sp + 2];
                dst[3] = 255;
            } else {
                let inv = 255 - a;
                dst[0] = ((sprite.rgba[sp] as u32 * a + dst[0] as u32 * inv) / 255) as u8;
                dst[1] = ((sprite.rgba[sp + 1] as u32 * a + dst[1] as u32 * inv) / 255) as u8;
                dst[2] = ((sprite.rgba[sp + 2] as u32 * a + dst[2] as u32 * inv) / 255) as u8;
            }
        }
    }
}

/// Expanding click-ripple state (position in virtual desktop coords).
#[derive(Clone, Copy)]
pub struct ClickRipple {
    pub x: i32,
    pub y: i32,
    /// 0.0 just pressed … 1.0 faded out.
    pub phase: f32,
}

const RIPPLE_RADIUS_MAX: f32 = 130.0;
const HALO_RADIUS: f32 = 34.0;
const SPOTLIGHT_RADIUS: f32 = 190.0;

/// Applies a cursor highlight style (plus an optional click ripple) to the frame.
pub fn apply_cursor_overlay(
    frame: &mut RgbaImage,
    origin: (i32, i32),
    pos: (i32, i32),
    style: CursorStyle,
    ripple: Option<ClickRipple>,
) {
    let (w, h) = frame.dimensions();
    if w == 0 || h == 0 {
        return;
    }
    let cx = (pos.0 - origin.0) as f32;
    let cy = (pos.1 - origin.1) as f32;
    let scale = (w.min(h) as f32 / 720.0).clamp(0.6, 2.2);

    match style {
        CursorStyle::Spotlight => {
            let r = SPOTLIGHT_RADIUS * scale;
            let dim = 0.45f32;
            let y0 = (cy - r).floor().max(0.0) as u32;
            let y1 = (cy + r).ceil().min(h as f32) as u32;
            let x0 = (cx - r).floor().max(0.0) as u32;
            let x1 = (cx + r).ceil().min(w as f32) as u32;
            for y in 0..y0 {
                dim_row(frame, y, dim);
            }
            for y in y1..h {
                dim_row(frame, y, dim);
            }
            for y in y0..y1 {
                for x in 0..x0 {
                    dim_px(frame, x, y, dim);
                }
                for x in x1..w {
                    dim_px(frame, x, y, dim);
                }
            }
            for y in y0..y1 {
                for x in x0..x1 {
                    let dxp = x as f32 + 0.5 - cx;
                    let dyp = y as f32 + 0.5 - cy;
                    if dxp * dxp + dyp * dyp > r * r {
                        dim_px(frame, x, y, dim);
                    }
                }
            }
        }
        CursorStyle::Halo => {
            let r = HALO_RADIUS * scale;
            draw_ring(frame, cx, cy, r, 3.5 * scale, 255, 59, 107, 220);
            draw_filled_circle(frame, cx, cy, 2.5 * scale, 255, 59, 107, 255);
        }
        CursorStyle::ClickRipple => {}
        CursorStyle::ColoredArrow => {}
    }

    if let Some(rip) = ripple {
        let r = 14.0 + RIPPLE_RADIUS_MAX * rip.phase;
        let alpha = ((1.0 - rip.phase) * 255.0).clamp(0.0, 255.0) as u8;
        let lx = rip.x as f32 - origin.0 as f32;
        let ly = rip.y as f32 - origin.1 as f32;
        draw_ring(frame, lx, ly, r, 5.0, 66, 165, 245, alpha);
        if rip.phase < 0.45 {
            draw_ring(frame, lx, ly, r * 0.55, 3.5, 255, 59, 107, alpha);
        }
    }

    if style == CursorStyle::ColoredArrow {
        draw_colored_arrow(frame, pos.0 - origin.0, pos.1 - origin.1);
    }
}

fn dim_row(frame: &mut RgbaImage, y: u32, dim: f32) {
    let (w, _) = frame.dimensions();
    let w = w as usize;
    let row = frame.as_mut();
    let base = ((y as usize) * w * 4) as usize;
    for i in 0..(w as usize * 4) {
        if i % 4 != 3 && row[base + i] != 0 {
            row[base + i] = (row[base + i] as f32 * dim) as u8;
        }
    }
}

fn dim_px(frame: &mut RgbaImage, x: u32, y: u32, dim: f32) {
    let px = frame.get_pixel_mut(x, y);
    for c in 0..3 {
        px[c] = (px[c] as f32 * dim) as u8;
    }
}

fn draw_filled_circle(frame: &mut RgbaImage, cx: f32, cy: f32, r: f32, r8: u8, g8: u8, b8: u8, a8: u8) {
    let (w, h) = frame.dimensions();
    let y0 = (cy - r).floor().max(0.0) as u32;
    let y1 = (cy + r).ceil().min(h as f32) as u32;
    let x0 = (cx - r).floor().max(0.0) as u32;
    let x1 = (cx + r).ceil().min(w as f32) as u32;
    let r2 = r * r;
    for y in y0..y1 {
        for x in x0..x1 {
            let dxp = x as f32 + 0.5 - cx;
            let dyp = y as f32 + 0.5 - cy;
            if dxp * dxp + dyp * dyp <= r2 {
                let px = frame.get_pixel_mut(x, y);
                px[0] = blend(px[0], r8, a8);
                px[1] = blend(px[1], g8, a8);
                px[2] = blend(px[2], b8, a8);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_ring(
    frame: &mut RgbaImage,
    cx: f32,
    cy: f32,
    radius: f32,
    width: f32,
    r8: u8,
    g8: u8,
    b8: u8,
    a8: u8,
) {
    let (w, h) = frame.dimensions();
    let outer = radius + width / 2.0;
    let inner_max = (radius - width / 2.0).max(0.0);
    let y0 = (cy - outer).floor().max(0.0) as u32;
    let y1 = (cy + outer).ceil().min(h as f32) as u32;
    let x0 = (cx - outer).floor().max(0.0) as u32;
    let x1 = (cx + outer).ceil().min(w as f32) as u32;
    let o2 = outer * outer;
    let i2 = inner_max * inner_max;
    for y in y0..y1 {
        for x in x0..x1 {
            let dxp = x as f32 + 0.5 - cx;
            let dyp = y as f32 + 0.5 - cy;
            let d2 = dxp * dxp + dyp * dyp;
            if d2 <= o2 && d2 >= i2 {
                let px = frame.get_pixel_mut(x, y);
                px[0] = blend(px[0], r8, a8);
                px[1] = blend(px[1], g8, a8);
                px[2] = blend(px[2], b8, a8);
            }
        }
    }
}

fn blend(dst: u8, src: u8, a: u8) -> u8 {
    if a >= 250 {
        return src;
    }
    ((src as u32 * a as u32 + dst as u32 * (255 - a as u32)) / 255) as u8
}

/// Bold colored arrow replacing the real cursor (0,0 = hotspot/tip).
const ARROW_MASK: [&str; 16] = [
    "X................",
    "XX...............",
    "X.X..............",
    "X..X.............",
    "X...X............",
    "X....X...........",
    "X.....X..........",
    "X......X.........",
    "X.......X........",
    "X........X.......",
    "X.........X......",
    "X..........X.....",
    "X...........X....",
    "X............X...",
    "X.............X..",
    "X..............X.",
];

fn draw_colored_arrow(frame: &mut RgbaImage, hx: i32, hy: i32) {
    let size = 16u32;
    let ox = hx;
    let oy = hy - 2;
    for y in 0..size {
        for x in 0..size {
            let row = ARROW_MASK[y as usize].as_bytes();
            let fill = row[x as usize] == b'X';
            let outline = !fill
                && [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)]
                    .iter()
                    .any(|&(nx, ny)| {
                        nx < size
                            && ny < size
                            && ARROW_MASK[ny as usize].as_bytes()[nx as usize] == b'X'
                    });
            if !fill && !outline {
                continue;
            }
            let px = ox + x as i32;
            let py = oy + y as i32;
            if px < 0 || py < 0 || px >= frame.width() as i32 || py >= frame.height() as i32 {
                continue;
            }
            let (r8, g8, b8) = if fill { (255, 180, 60) } else { (20, 20, 20) };
            let dst = frame.get_pixel_mut(px as u32, py as u32);
            if fill {
                dst[0] = r8;
                dst[1] = g8;
                dst[2] = b8;
            } else {
                dst[0] = blend(dst[0], r8, 220);
                dst[1] = blend(dst[1], g8, 220);
                dst[2] = blend(dst[2], b8, 220);
            }
        }
    }
}

// ============================================================================
// DXGI Desktop Duplication capture (hardware desktop: games, GPU-accelerated
// browsers, layered windows — no black screen) with automatic GDI fallback.
// ============================================================================

#[cfg(target_os = "windows")]
pub struct DxgiCapturer {
    device: windows::Win32::Graphics::Direct3D11::ID3D11Device,
    ctx: windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
    dup: windows::Win32::Graphics::Dxgi::IDXGIOutputDuplication,
    out_rect: [i32; 4],
    format: DXGI_FORMAT,
    staging: Option<windows::Win32::Graphics::Direct3D11::ID3D11Texture2D>,
    lost: bool,
    last_err: Option<String>,
}

#[cfg(not(target_os = "windows"))]
pub struct DxgiCapturer;

#[cfg(target_os = "windows")]
impl DxgiCapturer {
    pub fn new(monitor_index: usize) -> Result<Self, String> {
        use windows::core::Interface;
        use windows::Win32::Graphics::Direct3D::{
            D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_11_0,
            D3D_FEATURE_LEVEL_9_1,
        };
        use windows::Win32::Graphics::Direct3D11::{
            D3D11CreateDevice, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CPU_ACCESS_READ,
            D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, ID3D11Device,
            ID3D11DeviceContext, ID3D11Texture2D,
        };
        use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
        use windows::Win32::Graphics::Dxgi::{
            CreateDXGIFactory1, IDXGIAdapter, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput,
            IDXGIOutput1, IDXGIOutputDuplication,
        };

        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()
                .map_err(|e| format!("CreateDXGIFactory1 failed: {e}"))?;

            // Walk adapters/outputs until the requested global output index.
            let mut global_output = 0usize;
            let mut adapter1: Option<IDXGIAdapter1> = None;
            let mut output: Option<IDXGIOutput> = None;
            let mut out_desc = None;
            let mut adapter_idx = 0u32;
            while let Ok(a) = factory.EnumAdapters1(adapter_idx) {
                let mut out_idx = 0u32;
                while let Ok(o) = a.EnumOutputs(out_idx) {
                    if global_output == monitor_index {
                        adapter1 = Some(a.clone());
                        let desc = o.GetDesc().map_err(|e| format!("GetDesc: {e}"))?;
                        out_desc = Some(desc);
                        output = Some(o);
                        break;
                    }
                    global_output += 1;
                    out_idx += 1;
                }
                if output.is_some() {
                    break;
                }
                adapter_idx += 1;
            }
            let adapter1 = adapter1.ok_or_else(|| format!("No monitor with index {monitor_index}"))?;
            let output = output.ok_or_else(|| format!("No output for monitor {monitor_index}"))?;
            let out_desc = out_desc.ok_or_else(|| "monitor desc missing".to_string())?;

            let adapter: IDXGIAdapter = adapter1.cast().map_err(|e| format!("adapter cast: {e}"))?;
            let output1: IDXGIOutput1 = output.cast().map_err(|e| format!("output cast: {e}"))?;

            let levels = [
                D3D_FEATURE_LEVEL_11_0,
                D3D_FEATURE_LEVEL_10_0,
                D3D_FEATURE_LEVEL_9_1,
            ];
            let mut device: Option<ID3D11Device> = None;
            let mut ctx: Option<ID3D11DeviceContext> = None;
            D3D11CreateDevice(
                Some(&adapter),
                D3D_DRIVER_TYPE_HARDWARE,
                Default::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            )
            .map_err(|e| format!("D3D11CreateDevice failed: {e}"))?;
            let device = device.ok_or_else(|| "no d3d device".to_string())?;
            let ctx = ctx.ok_or_else(|| "no d3d context".to_string())?;

            let dup: IDXGIOutputDuplication = output1
                .DuplicateOutput(&device)
                .map_err(|e| format!("DuplicateOutput failed (secure desktop or driver): {e}"))?;

            let dup_desc = dup.GetDesc();
            let format = dup_desc.ModeDesc.Format;

            let r = out_desc.DesktopCoordinates;
            let w = (r.right - r.left).max(2) as u32;
            let h = (r.bottom - r.top).max(2) as u32;

            let tex_desc = D3D11_TEXTURE2D_DESC {
                Width: w,
                Height: h,
                MipLevels: 1,
                ArraySize: 1,
                Format: format,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut staging: Option<ID3D11Texture2D> = None;
            device
                .CreateTexture2D(&tex_desc, None, Some(&mut staging))
                .map_err(|e| format!("CreateTexture2D staging failed: {e}"))?;

            Ok(Self {
                device,
                ctx,
                dup,
                out_rect: [r.left, r.top, w as i32, h as i32],
                format,
                staging,
                lost: false,
                last_err: None,
            })
        }
    }

    /// Bounding box (virtual desktop coords) of the duplicated monitor.
    pub fn rect(&self) -> [i32; 4] {
        self.out_rect
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_err.as_deref()
    }

    /// Captures `region` (virtual desktop coords [x, y, w, h]) — clipped to the
    /// duplicated monitor — as an RGBA frame. `None` = no new desktop frame yet.
    pub fn capture_region(&mut self, region: [i32; 4]) -> Option<RgbaImage> {
        use windows::core::Interface;
        use windows::Win32::Graphics::Direct3D11::{D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, ID3D11Texture2D};
        use windows::Win32::Graphics::Dxgi::{
            DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, IDXGIResource,
        };

        if self.lost {
            if let Err(e) = self.recreate() {
                self.last_err = Some(e);
                return None;
            }
        }

        unsafe {
            let mut frame_info: DXGI_OUTDUPL_FRAME_INFO = Default::default();
            let mut resource: Option<IDXGIResource> = None;
            if let Err(e) = self
                .dup
                .AcquireNextFrame(0, &mut frame_info, &mut resource)
            {
                if e.code() == DXGI_ERROR_WAIT_TIMEOUT {
                    return None;
                }
                if e.code() == DXGI_ERROR_ACCESS_LOST {
                    self.lost = true;
                    return None;
                }
                self.last_err = Some(format!("AcquireNextFrame: {e}"));
                return None;
            }
            let Some(res) = resource else {
                let _ = self.dup.ReleaseFrame();
                return None;
            };
            let tex: ID3D11Texture2D = match res.cast() {
                Ok(t) => t,
                Err(e) => {
                    let _ = self.dup.ReleaseFrame();
                    self.last_err = Some(format!("resource cast: {e}"));
                    return None;
                }
            };
            let staging = self.staging.clone();
            if let Some(st) = &staging {
                self.ctx.CopyResource(st, &tex);
            }
            let _ = self.dup.ReleaseFrame();

            let st = staging?;
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            if let Err(e) = self.ctx.Map(&st, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) {
                self.last_err = Some(format!("Map staging: {e}"));
                return None;
            }
            let img = read_mapped_rgba(&mapped, self.out_rect, self.format, region);
            self.ctx.Unmap(&st, 0);
            if img.is_none() {
                self.last_err = Some("staging row read failed".to_string());
            }
            img
        }
    }

    /// Re-creates the duplication object (after DXGI_ERROR_ACCESS_LOST).
    pub fn recreate(&mut self) -> Result<(), String> {
        use windows::core::Interface;
        use windows::Win32::Graphics::Dxgi::{IDXGIOutput, IDXGIOutput1, IDXGIOutputDuplication};

        let output: IDXGIOutput = unsafe { self.dup.GetParent() }.map_err(|e| format!("GetParent: {e}"))?;
        let output1: IDXGIOutput1 = output.cast().map_err(|e| format!("output cast: {e}"))?;
        let dup: IDXGIOutputDuplication = unsafe { output1.DuplicateOutput(&self.device) }
            .map_err(|e| format!("re-DuplicateOutput: {e}"))?;
        self.dup = dup;
        self.lost = false;
        self.last_err = None;
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
impl DxgiCapturer {
    pub fn new(_monitor_index: usize) -> Result<Self, String> {
        Err("DXGI not supported on this platform".into())
    }
    pub fn rect(&self) -> [i32; 4] {
        [0, 0, 0, 0]
    }
    pub fn last_error(&self) -> Option<&str> {
        None
    }
    pub fn capture_region(&mut self, _region: [i32; 4]) -> Option<RgbaImage> {
        None
    }
    pub fn recreate(&mut self) -> Result<(), String> {
        Err("DXGI not supported on this platform".into())
    }
}

#[cfg(target_os = "windows")]
fn read_mapped_rgba(
    mapped: &D3D11_MAPPED_SUBRESOURCE,
    out_rect: [i32; 4],
    format: DXGI_FORMAT,
    region: [i32; 4],
) -> Option<RgbaImage> {
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R10G10B10A2_UNORM};
    let [ox, oy, ow, oh] = out_rect;
    let [rx, ry, rw, rh] = region;
    let x0 = rx.max(ox);
    let y0 = ry.max(oy);
    let x1 = (rx + rw).min(ox + ow);
    let y1 = (ry + rh).min(oy + oh);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let cw = (x1 - x0) as u32;
    let ch = (y1 - y0) as u32;
    let pitch = mapped.RowPitch as usize;
    let src_x = (x0 - ox) as usize;
    let src_y = (y0 - oy) as usize;
    let base = unsafe { mapped.pData as *const u8 };
    let mut rgba = vec![0u8; cw as usize * ch as usize * 4];

    match format {
        DXGI_FORMAT_R10G10B10A2_UNORM => {
            for y in 0..ch as usize {
                let row = unsafe { base.add((src_y + y) * pitch) };
                for x in 0..cw as usize {
                    let v = u32::from_le_bytes(unsafe {
                        std::ptr::read_unaligned(row.add((src_x + x) * 4) as *const [u8; 4])
                    });
                    let r = ((v >> 22) & 0x3FF) as u8;
                    let g = ((v >> 12) & 0x3FF) as u8;
                    let b = ((v >> 2) & 0x3FF) as u8;
                    let p = (y * cw as usize + x) * 4;
                    rgba[p] = r;
                    rgba[p + 1] = g;
                    rgba[p + 2] = b;
                    rgba[p + 3] = 255;
                }
            }
        }
        _ => {
            // B8G8R8A8 (or equivalent 4-byte layout)
            let _ = DXGI_FORMAT_B8G8R8A8_UNORM;
            for y in 0..ch as usize {
                let row = unsafe { base.add((src_y + y) * pitch) };
                let out_start = y * cw as usize * 4;
                for x in 0..cw as usize {
                    let p = (src_x + x) * 4;
                    let b = unsafe { *row.add(p) };
                    let g = unsafe { *row.add(p + 1) };
                    let r = unsafe { *row.add(p + 2) };
                    let a = unsafe { *row.add(p + 3) };
                    let o = out_start + x * 4;
                    rgba[o] = r;
                    rgba[o + 1] = g;
                    rgba[o + 2] = b;
                    rgba[o + 3] = if a == 0 { 255 } else { a };
                }
            }
        }
    }
    RgbaImage::from_raw(cw, ch, rgba)
}

/// Convenience: try DXGI first, fall back to GDI when it yields nothing.
pub fn capture_region_smart(
    dxgi: Option<&mut DxgiCapturer>,
    region: [u32; 4],
    prefer_dxgi: bool,
) -> RgbaImage {
    let region_i = [region[0] as i32, region[1] as i32, region[2] as i32, region[3] as i32];
    if prefer_dxgi {
        if let Some(cap) = dxgi {
            if let Some(img) = cap.capture_region(region_i) {
                return img;
            }
        }
    }
    capture_screen_region(region[0] as i32, region[1] as i32, region[2], region[3])
}


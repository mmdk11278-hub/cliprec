use std::fs::File;
use std::io::{BufReader, BufWriter, Cursor, Read, Seek, SeekFrom, Write};
use std::path::Path;

use image::{ImageFormat, RgbaImage};

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
fn capture_gdi_windows(x: i32, y: i32, w: u32, h: u32) -> Option<RgbaImage> {
    unsafe {
        let hdc_screen = GetDC(std::ptr::null_mut());
        if hdc_screen.is_null() {
            return None;
        }
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
        let ok = BitBlt(hdc_mem, 0, 0, w as i32, h as i32, hdc_screen, x, y, SRCCOPY | CAPTUREBLT);

        let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
        let mut bmi = BitmapInfo {
            bmi_header: BitmapInfoHeader {
                bi_size: std::mem::size_of::<BitmapInfoHeader>() as u32,
                bi_width: w as i32,
                bi_height: -(h as i32), // top-down DIB
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

        let lines = if ok != 0 {
            GetDIBits(hdc_mem, hbm, 0, h, buf.as_mut_ptr(), &mut bmi, DIB_RGB_COLORS)
        } else {
            0
        };

        SelectObject(hdc_mem, old);
        DeleteObject(hbm);
        DeleteDC(hdc_mem);
        ReleaseDC(std::ptr::null_mut(), hdc_screen);

        if lines == 0 {
            return None;
        }

        // Convert BGRA -> RGBA with full alpha
        for px in buf.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255;
        }

        RgbaImage::from_raw(w, h, buf)
    }
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

/// Loads either an `.avi` or `.mp4` (via FFmpeg decode to temp MJPEG AVI) for the native Video Player.
pub fn read_video_file(path: &Path) -> Option<LoadedVideo> {
    if let Some(v) = read_mjpeg_avi(path) {
        return Some(v);
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

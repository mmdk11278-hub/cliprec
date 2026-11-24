use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaKind {
    Image,
    Video,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MediaItem {
    pub id: u64,
    pub name: String,
    pub kind: MediaKind,
    pub file_path: String,
    pub preview_path: String,
    pub created_unix: u64,
    pub size_bytes: u64,
    pub duration_secs: f32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecordingQuality {
    Sd720p,
    Hd1080p,
    Qhd1440p,
    Uhd4K,
}

impl RecordingQuality {
    pub fn label(self) -> &'static str {
        match self {
            Self::Sd720p => "SD (720p)",
            Self::Hd1080p => "HD (1080p)",
            Self::Qhd1440p => "2K QHD (1440p)",
            Self::Uhd4K => "4K UHD (2160p)",
        }
    }

    pub fn max_height(self) -> u32 {
        match self {
            Self::Sd720p => 720,
            Self::Hd1080p => 1080,
            Self::Qhd1440p => 1440,
            Self::Uhd4K => 2160,
        }
    }

    pub fn crf(self, compact: bool) -> u8 {
        if compact {
            match self {
                Self::Sd720p => 29,
                Self::Hd1080p => 28,
                Self::Qhd1440p => 27,
                Self::Uhd4K => 26,
            }
        } else {
            match self {
                Self::Sd720p => 24,
                Self::Hd1080p => 23,
                Self::Qhd1440p => 22,
                Self::Uhd4K => 21,
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptureMethod {
    DesktopDuplication,
    GdiCompatible,
}

impl CaptureMethod {
    pub fn label(self) -> &'static str {
        match self {
            Self::DesktopDuplication => "GPU Desktop Duplication (DXGI)",
            Self::GdiCompatible => "Fast CPU + RAM (GDI Persistent — Recommended)",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum EncoderKind {
    /// Native Windows Media Foundation MP4 (H.264 + AAC) — no external FFmpeg needed.
    #[default]
    NativeMp4,
    /// Legacy Motion-JPEG AVI fallback (used when the H.264 encoder is unavailable).
    LegacyAvi,
}

impl EncoderKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::NativeMp4 => "Native MP4 (H.264 + AAC) — no FFmpeg needed",
            Self::LegacyAvi => "Legacy AVI (MJPEG, large files)",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::NativeMp4 => Self::LegacyAvi,
            Self::LegacyAvi => Self::NativeMp4,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CursorStyle {
    /// Colored ring drawn around the real mouse cursor.
    #[default]
    Halo,
    /// Dims everything but a bright circle around the cursor.
    Spotlight,
    /// Ripple ring burst on every left click.
    ClickRipple,
    /// Bold colored arrow replacing the real cursor.
    ColoredArrow,
}

impl CursorStyle {
    pub fn label(self) -> &'static str {
        match self {
            Self::Halo => "Halo ring",
            Self::Spotlight => "Spotlight",
            Self::ClickRipple => "Click ripple",
            Self::ColoredArrow => "Colored arrow",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Halo => Self::Spotlight,
            Self::Spotlight => Self::ClickRipple,
            Self::ClickRipple => Self::ColoredArrow,
            Self::ColoredArrow => Self::Halo,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FloatingUiMode {
    FullMenuAndControls,
    MinimalMenuAndControls,
    MinimalMenuOnly,
    Hidden,
}

impl FloatingUiMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::FullMenuAndControls => "Full Floating Dock & Controls",
            Self::MinimalMenuAndControls => "Minimal Dock & Recording Bar",
            Self::MinimalMenuOnly => "Minimal Dock Only",
            Self::Hidden => "Hidden While Recording",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebcamBgMode {
    Normal,
    Blur,
    Transparent,
}

impl WebcamBgMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Blur => "Blur",
            Self::Transparent => "Transparent",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Normal => Self::Blur,
            Self::Blur => Self::Transparent,
            Self::Transparent => Self::Normal,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebcamSizePreset {
    Small,
    Large,
    Full,
}

impl WebcamSizePreset {
    pub fn label(self) -> &'static str {
        match self {
            Self::Small => "Small Bubble",
            Self::Large => "Large Bubble",
            Self::Full => "Full Frame",
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppSettings {
    pub quality: RecordingQuality,
    pub fps: u32,
    pub capture_method: CaptureMethod,
    pub record_mic: bool,
    pub mic_gain: f32,
    pub ai_mic_enhance: bool,
    pub record_system_audio: bool,
    pub enable_webcam: bool,
    pub webcam_bg: WebcamBgMode,
    pub webcam_size: WebcamSizePreset,
    pub floating_ui_mode: FloatingUiMode,
    pub countdown_enabled: bool,
    pub auto_open_editor: bool,
    pub mouse_highlight: bool,
    pub capture_hotkey: String,
    pub pause_hotkey: String,
    pub dictation_hotkey: String,
    #[serde(default = "default_true")]
    pub prefer_mp4: bool,
    #[serde(default = "default_true")]
    pub compact_video: bool,
    #[serde(default)]
    pub encoder: EncoderKind,
    #[serde(default)]
    pub cursor_style: CursorStyle,
    #[serde(default)]
    pub admin_capture_enabled: bool,
}

impl Default for AppSettings {
    /// Defaults verified from decompiled ScreenRec functions:
    /// - fps = 25 (enforceFpsConstraints 0x140086a80 & fallback 0x14005f500)
    /// - capture_hotkey = Alt+S (0x140087b10), pause_hotkey = Alt+Q (0x140087a80), dictation = Alt+D (0x1400879f0)
    /// - mouse_highlight (SettingsManager::initialize 0x1400b6070 at 0x1400b8105)
    fn default() -> Self {
        Self {
            quality: RecordingQuality::Hd1080p,
            fps: 25,
            capture_method: CaptureMethod::GdiCompatible,
            record_mic: true,
            mic_gain: 0.85,
            ai_mic_enhance: true,
            record_system_audio: true,
            enable_webcam: false,
            webcam_bg: WebcamBgMode::Normal,
            webcam_size: WebcamSizePreset::Small,
            floating_ui_mode: FloatingUiMode::FullMenuAndControls,
            countdown_enabled: true,
            auto_open_editor: true,
            mouse_highlight: true,
            capture_hotkey: "Alt+S".to_string(),
            pause_hotkey: "Alt+Q".to_string(),
            dictation_hotkey: "Alt+D".to_string(),
            prefer_mp4: true,
            compact_video: true,
            encoder: EncoderKind::NativeMp4,
            cursor_style: CursorStyle::Halo,
            admin_capture_enabled: false,
        }
    }
}

pub struct StorageManager {
    pub root_dir: PathBuf,
    pub records_dir: PathBuf,
    pub index_path: PathBuf,
    pub settings_path: PathBuf,
}

impl StorageManager {
    pub fn new() -> Self {
        let root_dir = default_data_dir();
        let records_dir = root_dir.join("Records");
        let _ = fs::create_dir_all(&records_dir);
        let index_path = root_dir.join("gallery_index.json");
        let settings_path = root_dir.join("settings.json");
        Self {
            root_dir,
            records_dir,
            index_path,
            settings_path,
        }
    }

    pub fn load_settings(&self) -> AppSettings {
        fs::read_to_string(&self.settings_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save_settings(&self, settings: &AppSettings) {
        if let Ok(json) = serde_json::to_string_pretty(settings) {
            let _ = fs::write(&self.settings_path, json);
        }
    }

    pub fn load_items(&self) -> Vec<MediaItem> {
        let mut items: Vec<MediaItem> = fs::read_to_string(&self.index_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        items.retain(|it| Path::new(&it.file_path).exists());
        items
    }

    pub fn save_items(&self, items: &[MediaItem]) {
        if let Ok(json) = serde_json::to_string_pretty(items) {
            let _ = fs::write(&self.index_path, json);
        }
    }

    pub fn now_unix() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    pub fn next_file_paths(&self, kind: MediaKind) -> (u64, String, PathBuf, PathBuf) {
        let ext = match kind {
            MediaKind::Image => "png",
            MediaKind::Video => "avi",
        };
        self.next_file_paths_with_ext(kind, ext)
    }

    pub fn next_file_paths_with_ext(
        &self,
        kind: MediaKind,
        ext: &str,
    ) -> (u64, String, PathBuf, PathBuf) {
        let ts = Self::now_unix();
        let prefix = match kind {
            MediaKind::Image => "Screenshot",
            MediaKind::Video => "Recording",
        };
        let mut suffix = 1u32;
        loop {
            let stem = if suffix == 1 {
                format!("{prefix}_{ts}")
            } else {
                // Matches rewriteNameLineIfNeeded (0x140087cd0): " (%1)"
                format!("{prefix}_{ts} ({suffix})")
            };
            let main_path = self.records_dir.join(format!("{stem}.{ext}"));
            if !main_path.exists() {
                let preview_path = match kind {
                    MediaKind::Image => main_path.clone(),
                    MediaKind::Video => self.records_dir.join(format!("{stem}_thumb.png")),
                };
                return (ts * 100 + suffix as u64, stem, main_path, preview_path);
            }
            suffix += 1;
        }
    }

    pub fn open_in_explorer(path: &Path) {
        #[cfg(target_os = "windows")]
        {
            if path.is_file() {
                let arg = format!("/select,{}", path.display());
                let _ = std::process::Command::new("explorer").arg(arg).spawn();
            } else {
                let _ = std::process::Command::new("explorer").arg(path).spawn();
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = path;
        }
    }
}

fn default_data_dir() -> PathBuf {
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        PathBuf::from(local).join("ClipRec")
    } else if let Ok(exe) = std::env::current_exe() {
        exe.parent()
            .unwrap_or_else(|| Path::new("."))
            .join("ClipRecData")
    } else {
        PathBuf::from("ClipRecData")
    }
}

pub fn format_duration(secs: f32) -> String {
    let total = secs.max(0.0).floor() as u32;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

pub fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

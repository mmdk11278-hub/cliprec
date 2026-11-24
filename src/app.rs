use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::{
    Align, Color32, ColorImage, CornerRadius, FontId, Layout, Pos2, Rect, RichText, Sense, Stroke,
    TextureHandle, TextureOptions, Vec2,
};
use image::RgbaImage;

use crate::annotations::{
    flatten_annotations, to_norm, AnnotationCanvas, AnnotationTool,
};
use crate::capture::{
    capture_full_screen, capture_screen_region, decode_jpeg_to_rgba, encode_jpeg, even_dim,
    enum_monitors, exclude_window_from_capture, find_ffmpeg_exe, read_mjpeg_avi, read_video_file,
    rgba_to_color_image, save_png_fast, screen_dimensions, transcode_to_mp4, write_mjpeg_avi,
    LoadedVideo,
};
use crate::mf::h264_encoder_available;
use crate::recorder::{
    grab_frame, probe_default_mic_name, RecorderConfig, RecorderHandle, VideoAudioPlayer,
    WebcamFeed,
};
use crate::storage::{
    format_duration, format_size, AppSettings, CaptureMethod, CursorStyle, EncoderKind,
    FloatingUiMode, MediaItem, MediaKind, RecordingQuality, StorageManager, WebcamBgMode,
    WebcamSizePreset,
};
use crate::theme::{
    apply_light_theme, draw_icon, DockIcon, ANNOTATION_PALETTE, BORDER_LIGHT, CARD_BG, DANGER_RED,
    DOCK_ACTIVE, DOCK_BG, DOCK_HOVER, GALLERY_BLUE, LEVEL_GREEN, PAUSE_ORANGE, PLAYER_BASE,
    PURPLE_ACTION, PURPLE_HOVER, PURPLE_PRIMARY, PURPLE_SOFT, PURPLE_TINT, REC_RED, SURFACE_BG,
    TEXT_PRIMARY, TEXT_SECONDARY,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActiveView {
    CaptureStudio,
    Editor,
    VideoEditor,
    Gallery,
    Player,
    Settings,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GalleryFilter {
    All,
    Images,
    Videos,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GallerySort {
    Date,
    Name,
    Size,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RecordingState {
    Idle,
    Countdown { started: Instant },
    Recording,
    Paused,
}

struct ToastMessage {
    text: String,
    is_error: bool,
    expires: Instant,
}

struct ActiveRecording {
    region: [u32; 4], // x, y, w, h in screen pixels
    fps: u32,
    handle: RecorderHandle,
    synced_webcam: (bool, WebcamSizePreset),
    elapsed: Duration,
    last_tick: Instant,
    /// Output path used when the recorder writes a native MP4 during recording.
    mp4_output: Option<(u64, String, PathBuf, PathBuf)>,
    /// Recording was started from the one-click Lecture mode preset.
    lecture: bool,
}

struct ActivePlayback {
    item_id: u64,
    video: LoadedVideo,
    audio_player: Option<VideoAudioPlayer>,
    playing: bool,
    position_sec: f32,
    duration_sec: f32,
    speed: f32,
    volume: f32,
    muted: bool,
    last_frame_idx: usize,
    last_tick: Instant,
    texture: Option<TextureHandle>,
}

/// In-app video editor state (trim / cut / merge / export).
struct VideoEditorState {
    clips: Vec<crate::editor::ClipDef>,
    width: u32,
    height: u32,
    fps: u32,
    selected: usize,
    split_sec: f32,
    /// Current clip preview first-frame (rebuilt on demand).
    thumb: Option<TextureHandle>,
    thumb_frame: Option<RgbaImage>,
    exporting: bool,
    export_progress: Arc<Mutex<f32>>,
    export_rx: Option<Receiver<Result<(u64, String, PathBuf), String>>>,
    export_msg: String,
    add_pending: Option<u64>,
}

pub struct ClipRecApp {
    storage: StorageManager,
    settings: AppSettings,
    items: Vec<MediaItem>,

    view: ActiveView,
    dock_collapsed: bool,

    // Capture Studio state
    desktop_snapshot: Option<RgbaImage>,
    desktop_texture: Option<TextureHandle>,
    selection_norm: [f32; 4], // x0, y0, x1, y1 in [0..1]
    selecting_drag: bool,
    selected_monitor: usize,
    lecture_armed: bool,
    rec_state: RecordingState,
    active_rec: Option<ActiveRecording>,
    rec_preview_texture: Option<TextureHandle>,
    webcam: Option<WebcamFeed>,
    webcam_texture: Option<TextureHandle>,
    webcam_error_shown: bool,
    sim_mic_level: f32,
    sim_sys_level: f32,
    dictating: bool,
    mic_device: Option<String>,

    // Sticky vertical recording sidebar state
    sidebar_on_right: bool,
    sidebar_collapsed: bool,
    sidebar_y_norm: f32,
    sidebar_capture_excluded: bool,
    sidebar_reposition: bool,
    auto_minimized_for_rec: bool,

    // Annotation Editor state
    editor_image: Option<RgbaImage>,
    editor_texture: Option<TextureHandle>,
    editor_item_id: Option<u64>,
    editor_title: String,
    annotations: AnnotationCanvas,
    show_color_picker: Option<ColorTarget>,

    // Gallery state
    gallery_filter: GalleryFilter,
    gallery_sort: GallerySort,
    gallery_desc: bool,
    gallery_search: String,
    gallery_selected: HashSet<u64>,
    gallery_renaming: Option<(u64, String)>,
    gallery_confirm_delete: bool,
    thumbnails: HashMap<u64, TextureHandle>,

    // Video Player state
    playback: Option<ActivePlayback>,

    // Video Editor state
    video_editor: Option<VideoEditorState>,

    // Toast notification (5000ms auto-hide per BottomMessageDialog.qml)
    toast: Option<ToastMessage>,

    // Headless / automated verification screenshot mode
    screenshot_target: Option<PathBuf>,
    screenshot_requested: bool,
    screenshot_saved: bool,
    frame_count: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ColorTarget {
    Stroke,
    Fill,
    Font,
}

/// Decodes the first frame of a video file (MP4 via Media Foundation, AVI via MJPEG reader).
fn first_video_frame(path: &Path) -> Option<RgbaImage> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    if ext == "mp4" {
        let mut r = crate::mf::Mp4Reader::new(path).ok()?;
        r.next_video_frame()
    } else {
        let v = read_mjpeg_avi(path)?;
        let jpeg = v.frames.first()?;
        decode_jpeg_to_rgba(jpeg)
    }
}

impl ClipRecApp {
    pub fn new(cc: &eframe::CreationContext<'_>, screenshot_target: Option<PathBuf>) -> Self {
        apply_light_theme(&cc.egui_ctx);

        let storage = StorageManager::new();
        let settings = storage.load_settings();
        let mut items = storage.load_items();

        // Capture a real live desktop preview immediately so the Studio stage looks crisp on launch
        let desktop_img = capture_full_screen();
        let desktop_tex = cc.egui_ctx.load_texture(
            "desktop_preview",
            rgba_to_color_image(&desktop_img),
            TextureOptions::LINEAR,
        );

        // Seed one initial screenshot into the editor buffer so switching to Editor immediately works
        let editor_img = desktop_img.clone();
        let editor_tex = cc.egui_ctx.load_texture(
            "editor_preview",
            rgba_to_color_image(&editor_img),
            TextureOptions::LINEAR,
        );

        // Sort newest first by default
        items.sort_by(|a, b| b.created_unix.cmp(&a.created_unix));

        Self {
            storage,
            settings,
            items,
            view: ActiveView::CaptureStudio,
            dock_collapsed: false,
            desktop_snapshot: Some(desktop_img),
            desktop_texture: Some(desktop_tex),
            selection_norm: [0.12, 0.12, 0.88, 0.88],
            selecting_drag: false,
            selected_monitor: 0,
            lecture_armed: false,
            rec_state: RecordingState::Idle,
            active_rec: None,
            rec_preview_texture: None,
            webcam: None,
            webcam_texture: None,
            webcam_error_shown: false,
            sim_mic_level: 0.35,
            sim_sys_level: 0.2,
            dictating: false,
            mic_device: probe_default_mic_name(),
            sidebar_on_right: true,
            sidebar_collapsed: false,
            sidebar_y_norm: 0.5,
            sidebar_capture_excluded: false,
            sidebar_reposition: false,
            auto_minimized_for_rec: false,
            editor_image: Some(editor_img),
            editor_texture: Some(editor_tex),
            editor_item_id: None,
            editor_title: "Desktop Preview".to_string(),
            annotations: AnnotationCanvas::default(),
            show_color_picker: None,
            gallery_filter: GalleryFilter::All,
            gallery_sort: GallerySort::Date,
            gallery_desc: true,
            gallery_search: String::new(),
            gallery_selected: HashSet::new(),
            gallery_renaming: None,
            gallery_confirm_delete: false,
            thumbnails: HashMap::new(),
            playback: None,
            video_editor: None,
            toast: None,
            screenshot_target,
            screenshot_requested: false,
            screenshot_saved: false,
            frame_count: 0,
        }
    }

    fn show_toast(&mut self, msg: impl Into<String>, is_error: bool) {
        self.toast = Some(ToastMessage {
            text: msg.into(),
            is_error,
            expires: Instant::now() + Duration::from_millis(5000),
        });
    }

    fn refresh_desktop_snapshot(&mut self, ctx: &egui::Context) {
        let img = self.selection_monitor_preview();
        let tex = ctx.load_texture(
            "desktop_preview",
            rgba_to_color_image(&img),
            TextureOptions::LINEAR,
        );
        self.desktop_snapshot = Some(img);
        self.desktop_texture = Some(tex);
    }

    /// Returns the [x, y, w, h] virtual-desktop rect of the currently selected monitor.
    fn selected_monitor_rect(&self) -> [i32; 4] {
        let monitors = enum_monitors();
        if monitors.is_empty() {
            return [0, 0, screen_dimensions().0 as i32, screen_dimensions().1 as i32];
        }
        let idx = self.selected_monitor.min(monitors.len() - 1);
        monitors[idx].rect
    }

    /// Full capture of the selected monitor (used for the Studio preview + lecture full-screen).
    fn selection_monitor_preview(&self) -> RgbaImage {
        let [x, y, w, h] = self.selected_monitor_rect();
        capture_screen_region(x, y, w as u32, h as u32)
    }

    fn selected_screen_region(&self) -> [u32; 4] {
        let [mx, my, mw, mh] = self.selected_monitor_rect();
        let (sw, sh) = (mw.max(64) as u32, mh.max(64) as u32);
        let x0 = (self.selection_norm[0].min(self.selection_norm[2]) * sw as f32).round() as u32;
        let y0 = (self.selection_norm[1].min(self.selection_norm[3]) * sh as f32).round() as u32;
        let x1 = (self.selection_norm[0].max(self.selection_norm[2]) * sw as f32).round() as u32;
        let y1 = (self.selection_norm[1].max(self.selection_norm[3]) * sh as f32).round() as u32;
        let w = even_dim(x1.saturating_sub(x0).max(64));
        let h = even_dim(y1.saturating_sub(y0).max(64));
        [
            mx as u32 + x0.min(sw.saturating_sub(w)),
            my as u32 + y0.min(sh.saturating_sub(h)),
            w,
            h,
        ]
    }

    fn take_screenshot(&mut self, ctx: &egui::Context, full_screen: bool) {
        let mid_recording = self.rec_state != RecordingState::Idle;
        if mid_recording {
            let img_arc = self
                .active_rec
                .as_ref()
                .and_then(|r| r.handle.latest_frame())
                .unwrap_or_else(|| {
                    let [rx, ry, rw, rh] = self.selected_screen_region();
                    std::sync::Arc::new(capture_screen_region(rx as i32, ry as i32, rw, rh))
                });
            let (id, name, file_path, preview_path) = self.storage.next_file_paths(MediaKind::Image);
            let item = MediaItem {
                id,
                name,
                kind: MediaKind::Image,
                file_path: file_path.to_string_lossy().to_string(),
                preview_path: preview_path.to_string_lossy().to_string(),
                created_unix: StorageManager::now_unix(),
                size_bytes: 0,
                duration_secs: 0.0,
                width: img_arc.width(),
                height: img_arc.height(),
            };
            self.items.insert(0, item);
            self.storage.save_items(&self.items);
            std::thread::spawn(move || {
                let _ = save_png_fast(&img_arc, &file_path);
            });
            self.show_toast("Screenshot saved to Gallery!", false);
            return;
        }

        let [rx, ry, rw, rh] = if full_screen {
            let (sw, sh) = screen_dimensions();
            [0, 0, sw, sh]
        } else {
            self.selected_screen_region()
        };

        let img = capture_screen_region(rx as i32, ry as i32, rw, rh);
        let (id, name, file_path, preview_path) = self.storage.next_file_paths(MediaKind::Image);
        if let Err(e) = save_png_fast(&img, &file_path) {
            self.show_toast(format!("Could not save screenshot: {e}"), true);
            return;
        }
        let size_bytes = std::fs::metadata(&file_path).map(|m| m.len()).unwrap_or(0);
        let item = MediaItem {
            id,
            name: name.clone(),
            kind: MediaKind::Image,
            file_path: file_path.to_string_lossy().to_string(),
            preview_path: preview_path.to_string_lossy().to_string(),
            created_unix: StorageManager::now_unix(),
            size_bytes,
            duration_secs: 0.0,
            width: img.width(),
            height: img.height(),
        };
        self.items.insert(0, item);
        self.storage.save_items(&self.items);

        let tex = ctx.load_texture(
            format!("editor_{id}"),
            rgba_to_color_image(&img),
            TextureOptions::LINEAR,
        );
        self.editor_image = Some(img);
        self.editor_texture = Some(tex);
        self.editor_item_id = Some(id);
        self.editor_title = name;
        self.annotations = AnnotationCanvas::default();
        self.show_color_picker = None;

        if self.settings.auto_open_editor {
            self.view = ActiveView::Editor;
        }
        self.show_toast("Screenshot captured and saved to Gallery!", false);
    }

    fn start_video_recording(&mut self, ctx: &egui::Context) {
        if self.rec_state != RecordingState::Idle {
            return;
        }
        if self.settings.countdown_enabled {
            self.rec_state = RecordingState::Countdown {
                started: Instant::now(),
            };
        } else {
            self.begin_recording_now(ctx);
        }
    }

    fn begin_recording_now(&mut self, ctx: &egui::Context) {
        // Lecture mode: one-click preset = full screen + webcam + mic + system audio + cursor halo.
        let lecture = self.lecture_armed;
        self.lecture_armed = false;
        let enable_webcam = lecture || self.settings.enable_webcam;
        let record_mic = lecture || self.settings.record_mic;
        let record_system_audio = lecture || self.settings.record_system_audio;
        let region = if lecture {
            let [mx, my, mw, mh] = self.selected_monitor_rect();
            [mx as u32, my as u32, even_dim(mw.max(2) as u32), even_dim(mh.max(2) as u32)]
        } else {
            self.selected_screen_region()
        };
        let fps = self.settings.fps.clamp(5, 30);
        if enable_webcam && self.webcam.is_none() {
            self.webcam = Some(WebcamFeed::start());
        }
        let webcam = if enable_webcam {
            self.webcam.as_ref().map(|w| (w.latest.clone(), self.settings.webcam_size))
        } else {
            None
        };

        // Always use Fast CPU + RAM (Persistent GDI) and Native MP4 (compact, high quality).
        let use_dxgi = false;
        let encoder: EncoderKind = if h264_encoder_available() {
            EncoderKind::NativeMp4
        } else {
            EncoderKind::LegacyAvi
        };
        let monitor_index = self.selected_monitor;
        let mp4_output = if encoder == EncoderKind::NativeMp4 {
            let (id, name, fp, prev) =
                self.storage.next_file_paths_with_ext(MediaKind::Video, "mp4");
            Some((id, name, fp, prev))
        } else {
            None
        };
        let mp4_path = mp4_output.clone().map(|(_, _, fp, _)| fp);
        let cursor_style = if lecture || self.settings.mouse_highlight {
            Some(if lecture {
                CursorStyle::Halo
            } else {
                self.settings.cursor_style
            })
        } else {
            None
        };

        let handle = RecorderHandle::start(RecorderConfig {
            region,
            fps,
            max_height: self.settings.quality.max_height().min(1080),
            webcam,
            record_mic,
            record_system_audio,
            mic_gain: self.settings.mic_gain,
            ai_mic_enhance: self.settings.ai_mic_enhance,
            use_dxgi,
            monitor_index,
            cursor_style,
            encoder,
            mp4_path,
        });

        // Surface audio-capture status clearly so the user knows if voice will be recorded.
        if record_mic {
            if let Some(device) = handle.mic_device_name() {
                self.mic_device = Some(device.to_string());
                self.show_toast(
                    format!("Mic recording ON — device: {device}"),
                    false,
                );
            } else {
                self.mic_device = None;
                self.show_toast(
                    "Microphone not available — recording video only (no voice).",
                    true,
                );
            }
        } else {
            self.mic_device = None;
        }
        if lecture && !handle.system_audio_active() {
            self.show_toast(
                "Lecture mode: system audio could not be captured on this machine — the video will still be recorded.",
                true,
            );
        }

        self.active_rec = Some(ActiveRecording {
            region,
            fps,
            handle,
            synced_webcam: (enable_webcam, self.settings.webcam_size),
            elapsed: Duration::ZERO,
            last_tick: Instant::now(),
            mp4_output,
            lecture,
        });
        self.rec_state = RecordingState::Recording;
        self.sidebar_capture_excluded = false;

        if self.screenshot_target.is_none() && self.settings.floating_ui_mode != FloatingUiMode::Hidden {
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Minimized(true));
            self.auto_minimized_for_rec = true;
        }

        self.show_toast(
            format!(
                "Recording started ({}x{} @ {} FPS)",
                region[2], region[3], fps
            ),
            false,
        );
    }

    fn toggle_pause_recording(&mut self) {
        match self.rec_state {
            RecordingState::Recording => {
                if let Some(ref mut rec) = self.active_rec {
                    rec.elapsed += rec.last_tick.elapsed();
                    rec.handle.set_paused(true);
                }
                self.rec_state = RecordingState::Paused;
                self.show_toast("Recording paused", false);
            }
            RecordingState::Paused => {
                if let Some(ref mut rec) = self.active_rec {
                    rec.last_tick = Instant::now();
                    rec.handle.set_paused(false);
                }
                self.rec_state = RecordingState::Recording;
                self.show_toast("Recording resumed", false);
            }
            _ => {}
        }
    }

    fn stop_video_recording(&mut self, ctx: &egui::Context) {
        let Some(mut rec) = self.active_rec.take() else {
            self.rec_state = RecordingState::Idle;
            return;
        };
        if self.rec_state == RecordingState::Recording {
            rec.elapsed += rec.last_tick.elapsed();
        }
        self.rec_state = RecordingState::Idle;
        self.rec_preview_texture = None;
        self.sidebar_capture_excluded = false;

        let was_minimized = ctx.input(|i| i.viewport().minimized).unwrap_or(false);
        if self.auto_minimized_for_rec || was_minimized {
            self.auto_minimized_for_rec = false;
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
        }

        let webcam_src = if self.settings.enable_webcam {
            self.webcam.as_ref().map(|w| (w.latest.clone(), self.settings.webcam_size))
        } else {
            None
        };
        let max_h = self.settings.quality.max_height().min(1080);
        let mut out = rec.handle.finish();

        // Ensure at least 1 frame was captured
        if out.frames.is_empty() && out.total_frames == 0 {
            let frame = grab_frame(rec.region, max_h, webcam_src.as_ref());
            out.frames.push(encode_jpeg(&frame, 78));
            out.first_frame = Some(frame);
        }

        let first = out
            .first_frame
            .take()
            .unwrap_or_else(|| capture_screen_region(rec.region[0] as i32, rec.region[1] as i32, rec.region[2], rec.region[3]));
        let frame_audio = out.frame_audio.clone();
        let audio_sample_rate = out.audio_sample_rate;
        let audio_channels = out.audio_channels;
        let frames = out.frames;
        let recorded_frames = out.total_frames.max(frames.len());
        let out_w = even_dim(first.width());
        let out_h = even_dim(first.height());
        let total_audio_frames: usize = if audio_sample_rate > 0 && audio_channels > 0 {
            if out.total_audio_samples > 0 {
                out.total_audio_samples / (audio_channels as usize)
            } else {
                frame_audio.iter().map(|c| c.len()).sum::<usize>() / (audio_channels as usize)
            }
        } else {
            0
        };
        let exact_duration = if total_audio_frames > 0 {
            total_audio_frames as f32 / audio_sample_rate as f32
        } else {
            rec.elapsed.as_secs_f32().max(0.1)
        };
        let actual_fps = if exact_duration > 0.25 {
            ((recorded_frames as f32 / exact_duration).round() as u32).clamp(1, rec.fps)
        } else {
            rec.fps.max(1)
        };

        // Save a compact 640px-wide thumbnail instead of a full 1080p PNG
        let thumb_img = if first.width() > 640 {
            let th = (first.height() as f32 * 640.0 / first.width() as f32).round().max(1.0) as u32;
            image::imageops::thumbnail(&first, 640, th)
        } else {
            first.clone()
        };

        // --- Native MP4 final file (written during recording via Media Foundation) ---
        let native_ok = matches!(out.mp4_result, Some(Ok(())))
            && out
                .mp4_path
                .as_ref()
                .map(|p| {
                    std::fs::metadata(p)
                        .map(|m| m.len() > 0)
                        .unwrap_or(false)
                })
                .unwrap_or(false);

        let (item, _size_bytes) = if native_ok {
            match rec.mp4_output.clone() {
                Some((id, name, file_path, preview_path)) => {
                    let _ = thumb_img.save(&preview_path);
                    let size = std::fs::metadata(&file_path).map(|m| m.len()).unwrap_or(0);
                    let dur = if audio_sample_rate > 0 && audio_channels > 0 && total_audio_frames > 0 {
                        (total_audio_frames as f32 / audio_sample_rate as f32).max(0.2)
                    } else {
                        (recorded_frames as f32 / actual_fps as f32)
                            .max(rec.elapsed.as_secs_f32())
                    };
                    (
                        MediaItem {
                            id,
                            name,
                            kind: MediaKind::Video,
                            file_path: file_path.to_string_lossy().to_string(),
                            preview_path: preview_path.to_string_lossy().to_string(),
                            created_unix: StorageManager::now_unix(),
                            size_bytes: size,
                            duration_secs: dur,
                            width: out_w,
                            height: out_h,
                        },
                        size,
                    )
                }
                None => {
                    // MP4 written but the plan was dropped — fall back to the AVI path.
                    if let Some(p) = &out.mp4_path {
                        let _ = std::fs::remove_file(p);
                    }
                    let (id, name, file_path, preview_path) =
                        self.storage.next_file_paths_with_ext(MediaKind::Video, "avi");
                    if let Err(e) = write_mjpeg_avi(
                        &file_path,
                        out_w,
                        out_h,
                        actual_fps,
                        &frames,
                        &frame_audio,
                        audio_sample_rate,
                        audio_channels,
                    ) {
                        self.show_toast(format!("Failed to write recording: {e}"), true);
                        return;
                    }
                    let _ = thumb_img.save(&preview_path);
                    let size = std::fs::metadata(&file_path).map(|m| m.len()).unwrap_or(0);
                    let dur = (frames.len() as f32 / actual_fps as f32)
                        .max(rec.elapsed.as_secs_f32());
                    (
                        MediaItem {
                            id,
                            name,
                            kind: MediaKind::Video,
                            file_path: file_path.to_string_lossy().to_string(),
                            preview_path: preview_path.to_string_lossy().to_string(),
                            created_unix: StorageManager::now_unix(),
                            size_bytes: size,
                            duration_secs: dur,
                            width: out_w,
                            height: out_h,
                        },
                        size,
                    )
                }
            }
        } else {
            // Partial/native MP4 failed or recorder used the legacy encoder → AVI fallback.
            if let Some(path) = &out.mp4_path {
                let _ = std::fs::remove_file(path);
            }
            let (id, name, file_path, preview_path) =
                self.storage.next_file_paths_with_ext(MediaKind::Video, "avi");
            if let Err(e) = write_mjpeg_avi(
                &file_path,
                out_w,
                out_h,
                actual_fps,
                &frames,
                &frame_audio,
                audio_sample_rate,
                audio_channels,
            ) {
                self.show_toast(format!("Failed to write recording: {e}"), true);
                return;
            }
            let _ = thumb_img.save(&preview_path);
            let size = std::fs::metadata(&file_path).map(|m| m.len()).unwrap_or(0);
            let dur = (frames.len() as f32 / actual_fps as f32)
                .max(rec.elapsed.as_secs_f32());
            (
                MediaItem {
                    id,
                    name,
                    kind: MediaKind::Video,
                    file_path: file_path.to_string_lossy().to_string(),
                    preview_path: preview_path.to_string_lossy().to_string(),
                    created_unix: StorageManager::now_unix(),
                    size_bytes: size,
                    duration_secs: dur,
                    width: out_w,
                    height: out_h,
                },
                size,
            )
        };
        let new_id = item.id;
        let new_dur = item.duration_secs;
        let frame_count = recorded_frames;
        self.items.insert(0, item);
        self.storage.save_items(&self.items);

        self.open_video_in_player(ctx, new_id);
        self.show_toast(
            format!(
                "Saved {} video ({} frames) to Gallery!",
                format_duration(new_dur),
                frame_count
            ),
            false,
        );
    }


    fn tick_recording(&mut self, ctx: &egui::Context) {
        // Keep the webcam feed alive while the overlay is enabled, and sync changes live to recorder
        let webcam_on = self.settings.enable_webcam
            || self
                .active_rec
                .as_ref()
                .map(|r| r.lecture)
                .unwrap_or(false);
        if webcam_on {
            if self.webcam.is_none() {
                self.webcam = Some(WebcamFeed::start());
                self.webcam_error_shown = false;
            }
            if let Some(ref cam) = self.webcam {
                if let Some(ref mut rec) = self.active_rec {
                    let target = (true, self.settings.webcam_size);
                    if rec.synced_webcam != target {
                        rec.handle.set_webcam(Some((cam.latest.clone(), self.settings.webcam_size)));
                        rec.synced_webcam = target;
                    }
                }
                if let Some(img) = cam.frame() {
                    let ci = rgba_to_color_image(&img);
                    match self.webcam_texture {
                        Some(ref mut t) => t.set(ci, TextureOptions::LINEAR),
                        None => self.webcam_texture = Some(ctx.load_texture("webcam_live", ci, TextureOptions::LINEAR)),
                    }
                }
                if !self.webcam_error_shown {
                    if let Some(err) = cam.error() {
                        self.webcam_error_shown = true;
                        self.show_toast(err, true);
                    }
                }
            }
            if self.view == ActiveView::CaptureStudio && self.rec_state == RecordingState::Idle {
                ctx.request_repaint_after(Duration::from_millis(40));
            }
        } else {
            if let Some(ref mut rec) = self.active_rec {
                if rec.synced_webcam.0 {
                    rec.handle.set_webcam(None);
                    rec.synced_webcam.0 = false;
                }
            }
            if self.webcam.is_some() {
                self.webcam = None;
                self.webcam_texture = None;
            }
        }

        match self.rec_state {
            RecordingState::Idle => {}
            RecordingState::Countdown { started } => {
                ctx.request_repaint_after(Duration::from_millis(50));
                if started.elapsed() >= Duration::from_secs(3) {
                    self.begin_recording_now(ctx);
                }
            }
            RecordingState::Paused => {
                ctx.request_repaint_after(Duration::from_millis(120));
            }
            RecordingState::Recording => {
                // Surface recorder events (capture-source fallbacks, warnings) to the user.
                if let Some(ref rec) = self.active_rec {
                    for ev in rec.handle.drain_events() {
                        if ev.contains("switched") || ev.contains("DXGI") {
                            self.show_toast(format!("{ev}"), true);
                        }
                    }
                }
                let root_minimized = ctx.input(|i| i.viewport().minimized).unwrap_or(false);
                if root_minimized {
                    // Drain any queued preview without spending CPU/GPU uploading to a hidden window
                    if let Some(ref rec) = self.active_rec {
                        let _ = rec.handle.take_preview();
                    }
                    ctx.request_repaint_after(Duration::from_millis(120));
                } else {
                    // Capture/encode happens on the recorder thread; UI only refreshes ~30x/s
                    ctx.request_repaint_after(Duration::from_millis(33));
                    if let Some(img) = self.active_rec.as_ref().and_then(|r| r.handle.take_preview()) {
                        let ci = rgba_to_color_image(&img);
                        match self.rec_preview_texture {
                            Some(ref mut t) => t.set(ci, TextureOptions::LINEAR),
                            None => {
                                self.rec_preview_texture =
                                    Some(ctx.load_texture("rec_preview", ci, TextureOptions::LINEAR))
                            }
                        }
                    }
                }

                if let Some(ref rec) = self.active_rec {
                    let mic = rec.handle.mic_level();
                    self.sim_mic_level = if rec.lecture || self.settings.record_mic {
                        (self.sim_mic_level * 0.75 + mic * 0.25).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    // WASAPI loopback system-audio level (real, not simulated anymore).
                    let sys = rec.handle.system_audio_level();
                    self.sim_sys_level = if (rec.lecture || self.settings.record_system_audio) {
                        (self.sim_sys_level * 0.75 + sys * 0.25).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                }
            }
        }
    }

    fn current_rec_elapsed(&self) -> f32 {
        match (&self.rec_state, &self.active_rec) {
            (RecordingState::Recording, Some(rec)) => {
                (rec.elapsed + rec.last_tick.elapsed()).as_secs_f32()
            }
            (RecordingState::Paused, Some(rec)) => rec.elapsed.as_secs_f32(),
            _ => 0.0,
        }
    }

    fn open_image_in_editor(&mut self, ctx: &egui::Context, item_id: u64) {
        let Some(item) = self.items.iter().find(|i| i.id == item_id).cloned() else {
            return;
        };
        match image::open(&item.file_path) {
            Ok(dyn_img) => {
                let rgba = dyn_img.to_rgba8();
                let tex = ctx.load_texture(
                    format!("editor_{item_id}"),
                    rgba_to_color_image(&rgba),
                    TextureOptions::LINEAR,
                );
                self.editor_image = Some(rgba);
                self.editor_texture = Some(tex);
                self.editor_item_id = Some(item_id);
                self.editor_title = item.name;
                self.annotations = AnnotationCanvas::default();
                self.view = ActiveView::Editor;
            }
            Err(e) => {
                self.show_toast(format!("Failed to load image: {e}"), true);
            }
        }
    }

    fn open_video_in_player(&mut self, ctx: &egui::Context, item_id: u64) {
        let Some(item) = self.items.iter().find(|i| i.id == item_id).cloned() else {
            return;
        };
        if let Some(video) = read_video_file(std::path::Path::new(&item.file_path)) {
            let duration_sec = (video.frames.len() as f32 / video.fps as f32).max(0.1);
            let audio_player = if video.audio_sample_rate > 0 && !video.audio_samples.is_empty() {
                VideoAudioPlayer::new(
                    video.audio_samples.clone(),
                    video.audio_sample_rate,
                    video.audio_channels,
                )
            } else {
                None
            };
            let first_tex = video
                .frames
                .first()
                .and_then(|b| decode_jpeg_to_rgba(b))
                .map(|img| {
                    ctx.load_texture(
                        format!("player_{item_id}"),
                        rgba_to_color_image(&img),
                        TextureOptions::LINEAR,
                    )
                });
            self.playback = Some(ActivePlayback {
                item_id,
                video,
                audio_player,
                playing: true,
                position_sec: 0.0,
                duration_sec,
                speed: 1.0,
                volume: 1.0,
                muted: false,
                last_frame_idx: 0,
                last_tick: Instant::now(),
                texture: first_tex,
            });
            self.view = ActiveView::Player;
        } else {
            self.show_toast("Could not decode video file", true);
        }
    }

    /// Opens the selected video in the in-app Video Editor (timeline trim/cut/merge).
    fn open_video_editor(&mut self, ctx: &egui::Context, item_id: u64) {
        // Editor exports via Media Foundation; if H.264 is missing we can't write MP4.
        if !h264_encoder_available() {
            self.show_toast(
                "Video Editor needs the Windows H.264 encoder (Media Feature Pack) to export.",
                true,
            );
            return;
        }
        let Some(item) = self.items.iter().find(|i| i.id == item_id).cloned() else {
            return;
        };
        if item.kind != MediaKind::Video {
            return;
        }
        let source = PathBuf::from(&item.file_path);
        let mut total = item.duration_secs as f64;
        let mut w = item.width.max(2);
        let mut h = item.height.max(2);
        let mut fps = 25u32;
        if let Ok(p) = crate::editor::probe(&source) {
            w = p.0.max(2);
            h = p.1.max(2);
            fps = p.2.clamp(5, 60);
            if total <= 0.0 {
                total = p.5;
            }
        }
        let clip = crate::editor::ClipDef::new(
            source,
            item.name.clone(),
            if total > 0.0 { total } else { 10.0 },
        );
        self.view = ActiveView::VideoEditor;
        self.video_editor = Some(VideoEditorState {
            clips: vec![clip],
            width: even_dim(w),
            height: even_dim(h),
            fps,
            selected: 0,
            split_sec: 0.5,
            thumb: None,
            thumb_frame: None,
            exporting: false,
            export_progress: Arc::new(Mutex::new(0.0)),
            export_rx: None,
            export_msg: String::new(),
            add_pending: None,
        });
        self.show_toast("Video Editor — trim / cut / merge, then Export MP4", false);
        ctx.request_repaint();
    }

    fn export_item_as_mp4(&mut self, item_id: u64) {
        let Some(idx) = self.items.iter().position(|i| i.id == item_id) else {
            return;
        };
        let src_path = PathBuf::from(&self.items[idx].file_path);
        if src_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("mp4"))
            .unwrap_or(false)
        {
            StorageManager::open_in_explorer(&src_path);
            self.copy_text_to_clipboard(&src_path.to_string_lossy(), "MP4 path");
            return;
        }
        let mp4_path = src_path.with_extension("mp4");
        let old_size = self.items[idx].size_bytes;
        // Native Media Foundation conversion first (no FFmpeg), FFmpeg as fallback.
        let native = if h264_encoder_available() {
            let res = crate::editor::probe(&src_path).and_then(|p| {
                let clips = vec![crate::editor::ClipDef::new(
                    src_path.clone(),
                    "export".to_string(),
                    p.5,
                )];
                crate::editor::build_mp4(&clips, &mp4_path, p.0, p.1, p.2, |_| {})
            });
            match res {
                Ok(()) => Some(()),
                Err(_) => {
                    let _ = std::fs::remove_file(&mp4_path);
                    None
                }
            }
        } else {
            None
        };
        let transcode = |app: &mut ClipRecApp| {
            let crf = app.settings.quality.crf(app.settings.compact_video);
            transcode_to_mp4(&src_path, &mp4_path, crf, app.settings.ai_mic_enhance)
        };
        let result = if native.is_some() {
            Ok(())
        } else {
            transcode(self)
        };
        match result {
            Ok(()) => {
                let new_size = std::fs::metadata(&mp4_path).map(|m| m.len()).unwrap_or(0);
                let _ = std::fs::remove_file(&src_path);
                self.items[idx].file_path = mp4_path.to_string_lossy().to_string();
                self.items[idx].size_bytes = new_size;
                self.storage.save_items(&self.items);
                StorageManager::open_in_explorer(&mp4_path);
                self.show_toast(
                    format!(
                        "Exported MP4 ({} -> {})!",
                        format_size(old_size),
                        format_size(new_size)
                    ),
                    false,
                );
            }
            Err(e) => {
                self.show_toast(format!("MP4 export failed: {e}"), true);
            }
        }
    }

    fn convert_all_avi_to_mp4(&mut self) {
        let avi_ids: Vec<u64> = self
            .items
            .iter()
            .filter(|i| {
                i.kind == MediaKind::Video
                    && i.file_path.to_ascii_lowercase().ends_with(".avi")
            })
            .map(|i| i.id)
            .collect();
        if avi_ids.is_empty() {
            self.show_toast("All videos are already MP4!", false);
            return;
        }
        let crf = self.settings.quality.crf(self.settings.compact_video);
        let mut converted = 0usize;
        let mut saved_bytes = 0u64;
        for id in avi_ids {
            if let Some(idx) = self.items.iter().position(|i| i.id == id) {
                let src_path = PathBuf::from(&self.items[idx].file_path);
                let mp4_path = src_path.with_extension("mp4");
                let old_size = self.items[idx].size_bytes;
                // Native Media Foundation conversion first (no FFmpeg), FFmpeg as fallback.
                let native = if h264_encoder_available() {
                    crate::editor::probe(&src_path)
                        .and_then(|p| {
                            let clips = vec![crate::editor::ClipDef::new(
                                src_path.clone(),
                                "convert".to_string(),
                                p.5,
                            )];
                            crate::editor::build_mp4(&clips, &mp4_path, p.0, p.1, p.2, |_| {})
                        })
                        .is_ok()
                } else {
                    false
                };
                let ok = if native {
                    true
                } else {
                    let _ = std::fs::remove_file(&mp4_path);
                    transcode_to_mp4(&src_path, &mp4_path, crf, self.settings.ai_mic_enhance).is_ok()
                };
                if ok {
                    let new_size = std::fs::metadata(&mp4_path).map(|m| m.len()).unwrap_or(0);
                    let _ = std::fs::remove_file(&src_path);
                    self.items[idx].file_path = mp4_path.to_string_lossy().to_string();
                    self.items[idx].size_bytes = new_size;
                    saved_bytes += old_size.saturating_sub(new_size);
                    converted += 1;
                }
            }
        }
        self.storage.save_items(&self.items);
        self.show_toast(
            format!(
                "Converted {converted} video(s) to MP4 — saved {}!",
                format_size(saved_bytes)
            ),
            false,
        );
    }

    fn save_annotated_image(&mut self, ctx: &egui::Context, overwrite_current: bool) {
        let Some(ref base) = self.editor_image else {
            return;
        };
        let flattened = flatten_annotations(base, &self.annotations.items);
        if overwrite_current {
            if let Some(id) = self.editor_item_id {
                if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
                    if let Err(e) = flattened.save(&item.file_path) {
                        self.show_toast(format!("Save failed: {e}"), true);
                        return;
                    }
                    item.size_bytes = std::fs::metadata(&item.file_path).map(|m| m.len()).unwrap_or(0);
                    self.thumbnails.remove(&id);
                    self.storage.save_items(&self.items);
                    let tex = ctx.load_texture(
                        format!("editor_{id}_saved"),
                        rgba_to_color_image(&flattened),
                        TextureOptions::LINEAR,
                    );
                    self.editor_image = Some(flattened);
                    self.editor_texture = Some(tex);
                    self.annotations.clear();
                    self.show_toast("Annotations saved to image!", false);
                    return;
                }
            }
        }
        // Save as new copy in gallery
        let (id, name, file_path, preview_path) = self.storage.next_file_paths(MediaKind::Image);
        if let Err(e) = flattened.save(&file_path) {
            self.show_toast(format!("Export failed: {e}"), true);
            return;
        }
        let size_bytes = std::fs::metadata(&file_path).map(|m| m.len()).unwrap_or(0);
        self.items.insert(
            0,
            MediaItem {
                id,
                name: format!("{name}_annotated"),
                kind: MediaKind::Image,
                file_path: file_path.to_string_lossy().to_string(),
                preview_path: preview_path.to_string_lossy().to_string(),
                created_unix: StorageManager::now_unix(),
                size_bytes,
                duration_secs: 0.0,
                width: flattened.width(),
                height: flattened.height(),
            },
        );
        self.storage.save_items(&self.items);
        self.show_toast(format!("Saved PNG to {}", file_path.display()), false);
    }

    fn copy_editor_to_clipboard(&mut self) {
        let Some(ref base) = self.editor_image else {
            return;
        };
        let flattened = flatten_annotations(base, &self.annotations.items);
        match arboard::Clipboard::new() {
            Ok(mut cb) => {
                let img_data = arboard::ImageData {
                    width: flattened.width() as usize,
                    height: flattened.height() as usize,
                    bytes: std::borrow::Cow::Owned(flattened.into_raw()),
                };
                if let Err(e) = cb.set_image(img_data) {
                    self.show_toast(format!("Clipboard copy failed: {e}"), true);
                } else {
                    self.show_toast("Annotated image copied to clipboard!", false);
                }
            }
            Err(e) => {
                self.show_toast(format!("Clipboard unavailable: {e}"), true);
            }
        }
    }

    fn copy_text_to_clipboard(&mut self, text: &str, label: &str) {
        if let Ok(mut cb) = arboard::Clipboard::new() {
            if cb.set_text(text.to_string()).is_ok() {
                self.show_toast(format!("{label} copied to clipboard!"), false);
                return;
            }
        }
        self.show_toast("Could not copy to clipboard", true);
    }

    /// Whether the current process runs elevated (Administrator).
    fn is_elevated(&self) -> bool {
        #[cfg(target_os = "windows")]
        {
            unsafe { windows::Win32::UI::Shell::IsUserAnAdmin().as_bool() }
        }
        #[cfg(not(target_os = "windows"))]
        {
            false
        }
    }

    /// Relaunches ClipRec as Administrator (triggers the UAC prompt).
    /// Elevated processes can capture other elevated (Admin) windows.
    fn relaunch_as_admin(&self) {
        let exe = match std::env::current_exe() {
            Ok(e) => e,
            Err(_) => return,
        };
        let params = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
        #[cfg(target_os = "windows")]
        {
            use windows::core::PCWSTR;
            use windows::Win32::UI::Shell::ShellExecuteW;
            use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
            fn wide(s: &str) -> Vec<u16> {
                s.encode_utf16().chain(std::iter::once(0)).collect()
            }
            let runas = wide("runas");
            let file = wide(&exe.to_string_lossy());
            let args = wide(&params);
            unsafe {
                ShellExecuteW(
                    None,
                    PCWSTR(runas.as_ptr()),
                    PCWSTR(file.as_ptr()),
                    PCWSTR(args.as_ptr()),
                    PCWSTR(std::ptr::null()),
                    SW_SHOWNORMAL,
                );
            }
        }
    }

    fn delete_items(&mut self, ids: &[u64]) {
        let set: HashSet<u64> = ids.iter().copied().collect();
        for item in &self.items {
            if set.contains(&item.id) {
                let _ = std::fs::remove_file(&item.file_path);
                if item.preview_path != item.file_path {
                    let _ = std::fs::remove_file(&item.preview_path);
                }
                self.thumbnails.remove(&item.id);
            }
        }
        self.items.retain(|i| !set.contains(&i.id));
        self.gallery_selected.retain(|id| !set.contains(id));
        if let Some(ref pb) = self.playback {
            if set.contains(&pb.item_id) {
                self.playback = None;
                if self.view == ActiveView::Player {
                    self.view = ActiveView::Gallery;
                }
            }
        }
        self.storage.save_items(&self.items);
        self.show_toast(format!("Deleted {} item(s)", set.len()), false);
    }

    fn ensure_thumbnail(&mut self, ctx: &egui::Context, item: &MediaItem) -> Option<TextureHandle> {
        if let Some(tex) = self.thumbnails.get(&item.id) {
            return Some(tex.clone());
        }
        let img = image::open(&item.preview_path).ok()?.to_rgba8();
        let thumb = image::imageops::thumbnail(&img, 320, 200);
        let tex = ctx.load_texture(
            format!("thumb_{}", item.id),
            rgba_to_color_image(&thumb),
            TextureOptions::LINEAR,
        );
        self.thumbnails.insert(item.id, tex.clone());
        Some(tex)
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() {
            return;
        }
        let mut alt_s = false;
        let mut alt_q = false;
        let mut alt_d = false;
        let mut ctrl_z = false;
        let mut ctrl_y = false;
        let mut space = false;
        let mut left = false;
        let mut right = false;
        let mut del = false;

        ctx.input(|i| {
            alt_s = i.modifiers.alt && i.key_pressed(egui::Key::S);
            alt_q = i.modifiers.alt && i.key_pressed(egui::Key::Q);
            alt_d = i.modifiers.alt && i.key_pressed(egui::Key::D);
            ctrl_z = i.modifiers.ctrl && !i.modifiers.shift && i.key_pressed(egui::Key::Z);
            ctrl_y = (i.modifiers.ctrl && i.key_pressed(egui::Key::Y))
                || (i.modifiers.ctrl && i.modifiers.shift && i.key_pressed(egui::Key::Z));
            space = i.key_pressed(egui::Key::Space);
            left = i.key_pressed(egui::Key::ArrowLeft);
            right = i.key_pressed(egui::Key::ArrowRight);
            del = i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace);
        });

        if alt_s {
            if self.rec_state == RecordingState::Idle {
                self.take_screenshot(ctx, false);
            } else {
                self.stop_video_recording(ctx);
            }
        }
        if alt_q && matches!(self.rec_state, RecordingState::Recording | RecordingState::Paused) {
            self.toggle_pause_recording();
        }
        if alt_d {
            self.dictating = !self.dictating;
            self.show_toast(
                if self.dictating {
                    "Voice indicator active (Alt+D)"
                } else {
                    "Voice indicator stopped"
                },
                false,
            );
        }
        if self.view == ActiveView::Editor {
            if ctrl_z {
                self.annotations.undo();
            } else if ctrl_y {
                self.annotations.redo();
            } else if del {
                self.annotations.delete_selected();
            }
        }
        if self.view == ActiveView::Player {
            if let Some(ref mut pb) = self.playback {
                if space {
                    pb.playing = !pb.playing;
                    pb.last_tick = Instant::now();
                } else if left {
                    pb.position_sec = (pb.position_sec - 10.0).max(0.0);
                } else if right {
                    pb.position_sec = (pb.position_sec + 10.0).min(pb.duration_sec);
                }
            }
        }
    }

    fn handle_screenshot_mode(&mut self, ctx: &egui::Context) {
        let Some(ref target) = self.screenshot_target else {
            return;
        };
        ctx.request_repaint();
        self.frame_count += 1;

        if !self.screenshot_requested && self.frame_count >= 115 {
            self.screenshot_requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }

        let mut captured: Option<std::sync::Arc<ColorImage>> = None;
        ctx.input(|i| {
            for ev in &i.events {
                if let egui::Event::Screenshot { image, .. } = ev {
                    captured = Some(image.clone());
                }
            }
        });

        if let Some(img) = captured {
            if let Some(parent) = target.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let w = img.width() as u32;
            let h = img.height() as u32;
            let mut raw = Vec::with_capacity((w * h * 4) as usize);
            for px in &img.pixels {
                raw.extend_from_slice(&px.to_array());
            }
            if let Some(rgba) = RgbaImage::from_raw(w, h, raw) {
                let _ = rgba.save(target);
                self.screenshot_saved = true;
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if self.frame_count > 180 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    // =========================================================================
    // TOP BRAND HEADER BAR (#5100A4 — matching GalleryDialog / VideoPlayer QML)
    // =========================================================================
    fn render_top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("brand_header")
            .exact_height(46.0)
            .frame(egui::Frame::new().fill(PURPLE_PRIMARY).inner_margin(egui::Margin::symmetric(14, 0)))
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    // Neutral brand mark: crisp concentric capture circle
                    let (mark_rect, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
                    let p = ui.painter();
                    p.circle_stroke(mark_rect.center(), 10.0, Stroke::new(2.2, Color32::WHITE));
                    p.circle_filled(
                        mark_rect.center(),
                        5.2,
                        if self.rec_state == RecordingState::Recording { REC_RED } else { Color32::WHITE },
                    );
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("ClipRec")
                            .size(18.0)
                            .strong()
                            .color(Color32::WHITE),
                    );
                    ui.add_space(18.0);

                    let tabs = [
                        (ActiveView::CaptureStudio, "Capture"),
                        (ActiveView::Editor, "Annotate"),
                        (ActiveView::Gallery, "Gallery"),
                        (ActiveView::Player, "Player"),
                        (ActiveView::Settings, "Settings"),
                    ];
                    for (tab, label) in tabs {
                        let active = self.view == tab;
                        let bg = if active {
                            PURPLE_HOVER
                        } else {
                            Color32::from_rgba_unmultiplied(255, 255, 255, 18)
                        };
                        let btn = egui::Button::new(
                            RichText::new(label)
                                .size(13.5)
                                .strong()
                                .color(Color32::WHITE),
                        )
                        .fill(bg)
                        .stroke(Stroke::NONE)
                        .corner_radius(CornerRadius::same(6));
                        if ui.add_sized(Vec2::new(86.0, 30.0), btn).clicked() {
                            self.view = tab;
                        }
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        // Quick status pill
                        let status_text = match self.rec_state {
                            RecordingState::Idle => format!(
                                "{}  |  {} FPS  |  {}",
                                self.settings.quality.label(),
                                self.settings.fps,
                                self.settings.capture_hotkey
                            ),
                            RecordingState::Countdown { started } => {
                                let rem = 3u64.saturating_sub(started.elapsed().as_secs()).max(1);
                                format!("Starting in {rem}...")
                            }
                            RecordingState::Recording => {
                                format!("REC  {}", format_duration(self.current_rec_elapsed()))
                            }
                            RecordingState::Paused => {
                                format!("PAUSED  {}", format_duration(self.current_rec_elapsed()))
                            }
                        };
                        let pill_fill = match self.rec_state {
                            RecordingState::Recording => REC_RED,
                            RecordingState::Paused => PAUSE_ORANGE,
                            RecordingState::Countdown { .. } => PURPLE_ACTION,
                            RecordingState::Idle => Color32::from_rgba_unmultiplied(0, 0, 0, 55),
                        };
                        egui::Frame::new()
                            .fill(pill_fill)
                            .corner_radius(CornerRadius::same(12))
                            .inner_margin(egui::Margin::symmetric(12, 4))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(status_text)
                                        .size(12.0)
                                        .strong()
                                        .color(Color32::WHITE),
                                );
                            });
                    });
                });
            });
    }

    // =========================================================================
    // ICONIC DARK FLOATING DOCK (#1C1C1C — matching FlyingMenu.qml)
    // =========================================================================
    fn render_floating_dock(&mut self, ctx: &egui::Context) {
        let dock_w = if self.dock_collapsed { 52.0 } else { 76.0 };
        egui::SidePanel::right("floating_dock_panel")
            .resizable(false)
            .exact_width(dock_w)
            .frame(egui::Frame::new().fill(SURFACE_BG).inner_margin(egui::Margin::symmetric(6, 12)))
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    egui::Frame::new()
                        .fill(DOCK_BG)
                        .corner_radius(CornerRadius::same(14))
                        .inner_margin(egui::Margin::symmetric(6, 10))
                        .show(ui, |ui| {
                            ui.set_width(dock_w - 24.0);
                            ui.vertical_centered(|ui| {
                                // Collapse / Pin toggle at top
                                let pin_lbl = if self.dock_collapsed { "<<" } else { ">>" };
                                if ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new(pin_lbl).size(11.0).color(Color32::from_gray(180)),
                                        )
                                        .fill(Color32::TRANSPARENT)
                                        .stroke(Stroke::NONE),
                                    )
                                    .on_hover_text(if self.dock_collapsed {
                                        "Expand floating dock"
                                    } else {
                                        "Collapse floating dock"
                                    })
                                    .clicked()
                                {
                                    self.dock_collapsed = !self.dock_collapsed;
                                }

                                ui.add_space(6.0);

                                // 1. Screenshot button
                                if self.dock_button(
                                    ui,
                                    DockIcon::Camera,
                                    "Shot",
                                    "Take a screenshot (Alt+S)",
                                    false,
                                ) {
                                    self.take_screenshot(ctx, false);
                                }

                                ui.add_space(6.0);

                                // 2. Record / Stop button
                                let is_rec = self.rec_state != RecordingState::Idle;
                                if self.dock_button(
                                    ui,
                                    if is_rec { DockIcon::StopSquare } else { DockIcon::Video },
                                    if is_rec { "Stop" } else { "Record" },
                                    if is_rec {
                                        "Stop recording (Alt+S)"
                                    } else {
                                        "Start video recording"
                                    },
                                    is_rec,
                                ) {
                                    if is_rec {
                                        self.stop_video_recording(ctx);
                                    } else {
                                        self.start_video_recording(ctx);
                                    }
                                }

                                // Live timer & recording controls (exactly like FlyingMenu.qml)
                                if matches!(self.rec_state, RecordingState::Recording | RecordingState::Paused) {
                                    ui.add_space(4.0);
                                    let elapsed = format_duration(self.current_rec_elapsed());
                                    let is_paused = self.rec_state == RecordingState::Paused;
                                    let blink_alpha = if is_paused {
                                        let t = ctx.input(|i| i.time);
                                        if ((t / 0.75) as i64) % 2 == 0 { 255 } else { 90 }
                                    } else {
                                        255
                                    };
                                    let timer_col = if is_paused {
                                        Color32::from_rgba_unmultiplied(
                                            PAUSE_ORANGE.r(),
                                            PAUSE_ORANGE.g(),
                                            PAUSE_ORANGE.b(),
                                            blink_alpha,
                                        )
                                    } else {
                                        Color32::WHITE
                                    };
                                    ui.label(RichText::new(elapsed).size(11.5).strong().color(timer_col));

                                    ui.add_space(4.0);
                                    if self.dock_button(
                                        ui,
                                        if is_paused { DockIcon::Play } else { DockIcon::Pause },
                                        if is_paused { "Resume" } else { "Pause" },
                                        "Pause / Resume recording (Alt+Q)",
                                        is_paused,
                                    ) {
                                        self.toggle_pause_recording();
                                    }

                                    // Dashed separator
                                    ui.add_space(6.0);
                                    let (sep_r, _) = ui.allocate_exact_size(Vec2::new(34.0, 4.0), Sense::hover());
                                    for i in 0..5 {
                                        let sx = sep_r.left() + i as f32 * 7.5;
                                        ui.painter().line_segment(
                                            [Pos2::new(sx, sep_r.center().y), Pos2::new(sx + 4.0, sep_r.center().y)],
                                            Stroke::new(1.2, Color32::from_gray(140)),
                                        );
                                    }
                                    ui.add_space(6.0);

                                    // Mic toggle + live level
                                    if self.dock_button(
                                        ui,
                                        DockIcon::Mic {
                                            muted: !self.settings.record_mic,
                                            level: self.sim_mic_level,
                                        },
                                        "Mic",
                                        if self.settings.record_mic {
                                            "Mute microphone"
                                        } else {
                                            "Unmute microphone"
                                        },
                                        false,
                                    ) {
                                        self.settings.record_mic = !self.settings.record_mic;
                                        self.storage.save_settings(&self.settings);
                                    }

                                    ui.add_space(6.0);

                                    // System Audio toggle + live level
                                    if self.dock_button(
                                        ui,
                                        DockIcon::Speaker {
                                            muted: !self.settings.record_system_audio,
                                            level: self.sim_sys_level,
                                        },
                                        "Audio",
                                        if self.settings.record_system_audio {
                                            "Mute system audio"
                                        } else {
                                            "Unmute system audio"
                                        },
                                        false,
                                    ) {
                                        self.settings.record_system_audio = !self.settings.record_system_audio;
                                        self.storage.save_settings(&self.settings);
                                    }

                                    ui.add_space(6.0);

                                    // Webcam toggle
                                    if self.dock_button(
                                        ui,
                                        DockIcon::Webcam {
                                            active: self.settings.enable_webcam,
                                        },
                                        "Cam",
                                        "Toggle webcam bubble overlay",
                                        self.settings.enable_webcam,
                                    ) {
                                        self.settings.enable_webcam = !self.settings.enable_webcam;
                                        self.storage.save_settings(&self.settings);
                                    }
                                }

                                ui.add_space(8.0);
                                let (sep2, _) = ui.allocate_exact_size(Vec2::new(34.0, 2.0), Sense::hover());
                                ui.painter().line_segment(
                                    [sep2.left_center(), sep2.right_center()],
                                    Stroke::new(1.0, Color32::from_gray(70)),
                                );
                                ui.add_space(8.0);

                                // Dictation quick toggle
                                if self.dock_button(
                                    ui,
                                    DockIcon::Mic {
                                        muted: !self.dictating,
                                        level: if self.dictating { 0.72 } else { 0.0 },
                                    },
                                    "Voice",
                                    if self.dictating {
                                        "Stop voice dictation (Alt+D)"
                                    } else {
                                        "Voice notes indicator (Alt+D)"
                                    },
                                    self.dictating,
                                ) {
                                    self.dictating = !self.dictating;
                                    self.show_toast(
                                        if self.dictating {
                                            "Voice indicator active"
                                        } else {
                                            "Voice indicator stopped"
                                        },
                                        false,
                                    );
                                }

                                ui.add_space(6.0);

                                // Gallery button
                                if self.dock_button(
                                    ui,
                                    DockIcon::Gallery,
                                    "Gallery",
                                    "View all screen captures",
                                    self.view == ActiveView::Gallery,
                                ) {
                                    self.view = ActiveView::Gallery;
                                }

                                ui.add_space(6.0);

                                // Settings button
                                if self.dock_button(
                                    ui,
                                    DockIcon::Settings,
                                    "Settings",
                                    "Preferences & hotkeys",
                                    self.view == ActiveView::Settings,
                                ) {
                                    self.view = ActiveView::Settings;
                                }
                            });
                        });
                });
            });
    }

    fn dock_button(
        &self,
        ui: &mut egui::Ui,
        icon: DockIcon,
        label: &str,
        tooltip: &str,
        active: bool,
    ) -> bool {
        let size = if self.dock_collapsed {
            Vec2::new(36.0, 36.0)
        } else {
            Vec2::new(50.0, 50.0)
        };
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        let bg = if active {
            DOCK_ACTIVE
        } else if resp.hovered() {
            DOCK_HOVER
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, CornerRadius::same(8), bg);

        let icon_rect = if self.dock_collapsed {
            rect.shrink(4.0)
        } else {
            Rect::from_min_max(
                rect.min + Vec2::new(8.0, 4.0),
                Pos2::new(rect.max.x - 8.0, rect.max.y - 16.0),
            )
        };
        draw_icon(ui.painter(), icon_rect, icon, Color32::WHITE);

        if !self.dock_collapsed {
            ui.painter().text(
                Pos2::new(rect.center().x, rect.max.y - 8.0),
                egui::Align2::CENTER_CENTER,
                label,
                FontId::proportional(10.0),
                Color32::from_gray(220),
            );
        }
        resp.on_hover_text(tooltip).clicked()
    }

    // =========================================================================
    // VIEW 1: CAPTURE STUDIO (Region selector + ActionMenuWindow + Live Recording)
    // =========================================================================
    fn render_capture_studio(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let [rx, ry, rw, rh] = self.selected_screen_region();
        let (sw, sh) = screen_dimensions();

        // Top quick-action ribbon
        egui::Frame::new()
            .fill(CARD_BG)
            .stroke(Stroke::new(1.0, BORDER_LIGHT))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Capture Area")
                            .size(14.5)
                            .strong()
                            .color(PURPLE_PRIMARY),
                    );
                    ui.add_space(6.0);
                    if ui.button("Full Screen").clicked() {
                        self.selection_norm = [0.0, 0.0, 1.0, 1.0];
                    }
                    if ui.button("16:9 Center").clicked() {
                        self.selection_norm = [0.12, 0.12, 0.88, 0.88];
                    }
                    if ui.button("Left Half").clicked() {
                        self.selection_norm = [0.0, 0.0, 0.5, 1.0];
                    }
                    if ui.button("Refresh Preview").clicked() {
                        self.refresh_desktop_snapshot(ctx);
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        // One-click Lecture mode preset.
                        let lec_btn = egui::Button::new(
                            RichText::new("🎓  Lecture Mode")
                                .size(12.0)
                                .strong()
                                .color(Color32::WHITE),
                        )
                        .fill(PURPLE_ACTION);
                        if ui
                            .add(lec_btn)
                            .on_hover_text(
                                "Full screen + camera + mic + system audio + highlighted cursor, all in one click",
                            )
                            .clicked()
                        {
                            if self.rec_state == RecordingState::Idle {
                                self.lecture_armed = true;
                                if self.settings.countdown_enabled {
                                    self.rec_state = RecordingState::Countdown {
                                        started: Instant::now(),
                                    };
                                    self.show_toast(
                                        "Lecture mode armed — full screen, camera, mic, system audio and cursor highlight",
                                        false,
                                    );
                                } else {
                                    self.begin_recording_now(ctx);
                                }
                            } else {
                                self.show_toast("Stop the current recording first", true);
                            }
                        }

                        let monitors = enum_monitors();
                        if monitors.len() > 1 {
                            let cur = self.selected_monitor.min(monitors.len() - 1);
                            egui::ComboBox::from_id_salt("monitor_sel")
                                .width(150.0)
                                .selected_text(
                                    monitors
                                        .get(cur)
                                        .map(|m| {
                                            if m.primary {
                                                format!("{}, {} (primary)", m.name, m.index + 1)
                                            } else {
                                                format!("{}, {}", m.name, m.index + 1)
                                            }
                                        })
                                        .unwrap_or_else(|| "Screen".to_string()),
                                )
                                .show_ui(ui, |ui| {
                                    for m in &monitors {
                                        let label = if m.primary {
                                            format!("Monitor {} — {} (primary)", m.index + 1, m.name)
                                        } else {
                                            format!("Monitor {} — {}", m.index + 1, m.name)
                                        };
                                        if ui
                                            .selectable_label(self.selected_monitor == m.index, label)
                                            .clicked()
                                        {
                                            self.selected_monitor = m.index;
                                            self.refresh_desktop_snapshot(ctx);
                                        }
                                    }
                                });
                        }

                        // Audio & Webcam quick toggles
                        let cam_btn = egui::Button::new(
                            RichText::new(if self.settings.enable_webcam {
                                format!("Cam: {}", self.settings.webcam_bg.label())
                            } else {
                                "Cam: Off".to_string()
                            })
                            .size(12.0),
                        )
                        .fill(if self.settings.enable_webcam { PURPLE_SOFT } else { Color32::WHITE });
                        if ui.add(cam_btn).clicked() {
                            if !self.settings.enable_webcam {
                                self.settings.enable_webcam = true;
                            } else {
                                self.settings.webcam_bg = self.settings.webcam_bg.next();
                            }
                            self.storage.save_settings(&self.settings);
                        }

                        let sys_btn = egui::Button::new(
                            RichText::new(if self.settings.record_system_audio {
                                "System Audio: On"
                            } else {
                                "System Audio: Off"
                            })
                            .size(12.0),
                        )
                        .fill(if self.settings.record_system_audio { PURPLE_SOFT } else { Color32::WHITE });
                        if ui.add(sys_btn).clicked() {
                            self.settings.record_system_audio = !self.settings.record_system_audio;
                            self.storage.save_settings(&self.settings);
                        }

                        let mic_btn = egui::Button::new(
                            RichText::new(if self.settings.record_mic { "Mic: On" } else { "Mic: Off" }).size(12.0),
                        )
                        .fill(if self.settings.record_mic { PURPLE_SOFT } else { Color32::WHITE });
                        if ui.add(mic_btn).clicked() {
                            self.settings.record_mic = !self.settings.record_mic;
                            self.storage.save_settings(&self.settings);
                        }
                    });
                });
            });

        ui.add_space(10.0);

        // Main interactive desktop preview stage
        let avail = ui.available_size();
        let stage_h = (avail.y - 96.0).max(240.0);
        let stage_size = Vec2::new(avail.x, stage_h);

        egui::Frame::new()
            .fill(Color32::from_rgb(0x16, 0x16, 0x1D))
            .corner_radius(CornerRadius::same(12))
            .stroke(Stroke::new(1.0, Color32::from_rgb(0x30, 0x26, 0x45)))
            .show(ui, |ui| {
                let (stage_rect, _) = ui.allocate_exact_size(stage_size, Sense::hover());

                // Fit desktop preview with aspect ratio preserved
                let aspect = sw as f32 / sh.max(1) as f32;
                let inner = stage_rect.shrink(14.0);
                let fit_w = inner.width().min(inner.height() * aspect);
                let fit_h = fit_w / aspect;
                let preview_rect = Rect::from_center_size(inner.center(), Vec2::new(fit_w, fit_h));

                if let Some(ref tex) = self.desktop_texture {
                    ui.painter().image(
                        tex.id(),
                        preview_rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }

                // Region selection drag interaction (when idle)
                let drag_resp = ui.interact(preview_rect, ui.id().with("region_select"), Sense::drag());
                if self.rec_state == RecordingState::Idle {
                    if drag_resp.drag_started() {
                        if let Some(pos) = drag_resp.interact_pointer_pos() {
                            let n = to_norm(preview_rect, pos);
                            self.selection_norm = [n[0], n[1], n[0], n[1]];
                            self.selecting_drag = true;
                        }
                    } else if drag_resp.dragged() && self.selecting_drag {
                        if let Some(pos) = drag_resp.interact_pointer_pos() {
                            let n = to_norm(preview_rect, pos);
                            self.selection_norm[2] = n[0];
                            self.selection_norm[3] = n[1];
                        }
                    } else if drag_resp.drag_stopped() {
                        self.selecting_drag = false;
                        // Prevent tiny accidental clicks from collapsing region
                        if (self.selection_norm[2] - self.selection_norm[0]).abs() < 0.04
                            || (self.selection_norm[3] - self.selection_norm[1]).abs() < 0.04
                        {
                            self.selection_norm = [0.12, 0.12, 0.88, 0.88];
                        }
                    }
                }

                // Dim outside selected capture region
                let sx0 = preview_rect.left() + self.selection_norm[0].min(self.selection_norm[2]) * preview_rect.width();
                let sy0 = preview_rect.top() + self.selection_norm[1].min(self.selection_norm[3]) * preview_rect.height();
                let sx1 = preview_rect.left() + self.selection_norm[0].max(self.selection_norm[2]) * preview_rect.width();
                let sy1 = preview_rect.top() + self.selection_norm[1].max(self.selection_norm[3]) * preview_rect.height();
                let sel_rect = Rect::from_min_max(Pos2::new(sx0, sy0), Pos2::new(sx1, sy1));

                let dim = Color32::from_rgba_unmultiplied(12, 6, 28, 130);
                let p = ui.painter();
                p.rect_filled(Rect::from_min_max(preview_rect.min, Pos2::new(preview_rect.max.x, sel_rect.min.y)), CornerRadius::ZERO, dim);
                p.rect_filled(Rect::from_min_max(Pos2::new(preview_rect.min.x, sel_rect.max.y), preview_rect.max), CornerRadius::ZERO, dim);
                p.rect_filled(Rect::from_min_max(Pos2::new(preview_rect.min.x, sel_rect.min.y), Pos2::new(sel_rect.min.x, sel_rect.max.y)), CornerRadius::ZERO, dim);
                p.rect_filled(Rect::from_min_max(Pos2::new(sel_rect.max.x, sel_rect.min.y), Pos2::new(preview_rect.max.x, sel_rect.max.y)), CornerRadius::ZERO, dim);

                // Live recorded frame (already includes the webcam) inside the selection
                let recording_live = matches!(self.rec_state, RecordingState::Recording | RecordingState::Paused);
                if recording_live {
                    if let Some(ref tex) = self.rec_preview_texture {
                        p.image(tex.id(), sel_rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                    }
                }

                // Selection border (purple when idle, pulsing red when recording)
                let border_col = match self.rec_state {
                    RecordingState::Recording => REC_RED,
                    RecordingState::Paused => PAUSE_ORANGE,
                    _ => PURPLE_HOVER,
                };
                p.rect_stroke(sel_rect, CornerRadius::same(2), Stroke::new(2.5, border_col), egui::StrokeKind::Outside);

                // Corner handles
                for pt in [sel_rect.left_top(), sel_rect.right_top(), sel_rect.left_bottom(), sel_rect.right_bottom()] {
                    p.circle_filled(pt, 5.0, Color32::WHITE);
                    p.circle_stroke(pt, 5.0, Stroke::new(2.0, border_col));
                }

                // Resolution badge inside top-left of selection
                let dim_label = format!("{rw} x {rh} px  (drag on preview to adjust)");
                let badge_rect = Rect::from_min_size(sel_rect.left_top() + Vec2::new(8.0, 8.0), Vec2::new(240.0, 24.0));
                p.rect_filled(badge_rect, CornerRadius::same(6), Color32::from_rgba_unmultiplied(28, 28, 28, 215));
                p.text(
                    badge_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    dim_label,
                    FontId::proportional(11.5),
                    Color32::WHITE,
                );

                // Circular Webcam PiP Bubble preview (matching WebCamDialog.qml)
                if self.settings.enable_webcam && !recording_live {
                    let short = sel_rect.width().min(sel_rect.height());
                    let diameter = match self.settings.webcam_size {
                        WebcamSizePreset::Small => short * 0.24,
                        WebcamSizePreset::Large => short * 0.38,
                        WebcamSizePreset::Full => short * 0.38,
                    }
                    .max(40.0);
                    let bubble_center = sel_rect.right_bottom() - Vec2::splat(diameter * 0.5 + short * 0.03);
                    let bg_col = match self.settings.webcam_bg {
                        WebcamBgMode::Normal => Color32::from_rgb(0x2D, 0x26, 0x44),
                        WebcamBgMode::Blur => Color32::from_rgb(0x4A, 0x3A, 0x7A),
                        WebcamBgMode::Transparent => Color32::from_rgba_unmultiplied(81, 0, 164, 90),
                    };
                    if let Some(ref tex) = self.webcam_texture {
                        // Center-cropped square UV, drawn as a circle via a textured mesh
                        let [tw, th] = tex.size();
                        let (tw, th) = (tw as f32, th as f32);
                        let (uw, vh) = if tw > th { (th / tw, 1.0) } else { (1.0, tw / th) };
                        let (u0, v0) = ((1.0 - uw) / 2.0, (1.0 - vh) / 2.0);
                        let r = diameter * 0.5;
                        let mut mesh = egui::Mesh::with_texture(tex.id());
                        mesh.colored_vertex(bubble_center, Color32::WHITE);
                        mesh.vertices[0].uv = Pos2::new(u0 + uw / 2.0, v0 + vh / 2.0);
                        let segs = 48;
                        for i in 0..=segs {
                            let a = i as f32 / segs as f32 * std::f32::consts::TAU;
                            let (s, c) = a.sin_cos();
                            mesh.vertices.push(egui::epaint::Vertex {
                                pos: bubble_center + Vec2::new(c, s) * r,
                                uv: Pos2::new(u0 + uw * (0.5 + 0.5 * c), v0 + vh * (0.5 + 0.5 * s)),
                                color: Color32::WHITE,
                            });
                            if i > 0 {
                                mesh.add_triangle(0, i as u32, i as u32 + 1);
                            }
                        }
                        p.add(egui::Shape::mesh(mesh));
                        p.circle_stroke(bubble_center, r, Stroke::new(2.5, Color32::WHITE));
                    } else {
                        p.circle_filled(bubble_center, diameter * 0.5, bg_col);
                        p.circle_stroke(bubble_center, diameter * 0.5, Stroke::new(2.5, PURPLE_PRIMARY));
                        p.circle_filled(bubble_center + Vec2::new(0.0, -diameter * 0.14), diameter * 0.16, Color32::from_gray(230));
                        p.circle_filled(bubble_center + Vec2::new(0.0, diameter * 0.22), diameter * 0.24, Color32::from_gray(210));
                    }
                    p.text(
                        bubble_center + Vec2::new(0.0, diameter * 0.5 + 10.0),
                        egui::Align2::CENTER_CENTER,
                        if self.webcam_texture.is_some() { "Webcam (live)".to_string() } else { "Starting webcam…".to_string() },
                        FontId::proportional(10.5),
                        Color32::WHITE,
                    );
                }

                // 3-2-1 Countdown Overlay (matching VideoRecordingDialog 1.svg..3.svg)
                if let RecordingState::Countdown { started } = self.rec_state {
                    let rem = 3u64.saturating_sub(started.elapsed().as_secs()).max(1);
                    p.circle_filled(preview_rect.center(), 64.0, Color32::from_rgba_unmultiplied(81, 0, 164, 220));
                    p.circle_stroke(preview_rect.center(), 64.0, Stroke::new(4.0, Color32::WHITE));
                    p.text(
                        preview_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        format!("{rem}"),
                        FontId::proportional(58.0),
                        Color32::WHITE,
                    );
                }
            });

        ui.add_space(10.0);

        // Bottom Capture Action Bar (faithful to ActionMenuWindow.qml: white rounded bar, buttons + WxH)
        ui.horizontal_centered(|ui| {
            egui::Frame::new()
                .fill(CARD_BG)
                .stroke(Stroke::new(1.0, BORDER_LIGHT))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(egui::Margin::symmetric(16, 10))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let shot_btn = egui::Button::new(
                            RichText::new("Take Screenshot (Alt+S)")
                                .size(13.5)
                                .strong()
                                .color(Color32::WHITE),
                        )
                        .fill(PURPLE_ACTION)
                        .corner_radius(CornerRadius::same(7));
                        if ui.add_sized(Vec2::new(190.0, 38.0), shot_btn).clicked() {
                            self.take_screenshot(ctx, false);
                        }

                        ui.add_space(6.0);

                        let is_rec = self.rec_state != RecordingState::Idle;
                        let rec_btn = egui::Button::new(
                            RichText::new(if is_rec { "Stop Recording" } else { "Start Video Recording" })
                                .size(13.5)
                                .strong()
                                .color(Color32::WHITE),
                        )
                        .fill(if is_rec { REC_RED } else { PURPLE_PRIMARY })
                        .corner_radius(CornerRadius::same(7));
                        if ui.add_sized(Vec2::new(190.0, 38.0), rec_btn).clicked() {
                            if is_rec {
                                self.stop_video_recording(ctx);
                            } else {
                                self.start_video_recording(ctx);
                            }
                        }

                        if matches!(self.rec_state, RecordingState::Recording | RecordingState::Paused) {
                            ui.add_space(6.0);
                            let paused = self.rec_state == RecordingState::Paused;
                            let pause_btn = egui::Button::new(
                                RichText::new(if paused { "Resume (Alt+Q)" } else { "Pause (Alt+Q)" })
                                    .size(13.0)
                                    .strong()
                                    .color(Color32::WHITE),
                            )
                            .fill(PAUSE_ORANGE)
                            .corner_radius(CornerRadius::same(7));
                            if ui.add_sized(Vec2::new(132.0, 38.0), pause_btn).clicked() {
                                self.toggle_pause_recording();
                            }
                        } else {
                            ui.add_space(6.0);
                            if ui
                                .add_sized(Vec2::new(116.0, 38.0), egui::Button::new("Reset Region"))
                                .clicked()
                            {
                                self.selection_norm = [0.12, 0.12, 0.88, 0.88];
                            }
                            ui.add_space(4.0);
                            if ui
                                .add_sized(Vec2::new(82.0, 38.0), egui::Button::new("Cancel"))
                                .on_hover_text("Clear selection back to full screen")
                                .clicked()
                            {
                                self.selection_norm = [0.0, 0.0, 1.0, 1.0];
                            }
                        }

                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!("{rw}x{rh}"))
                                    .size(18.0)
                                    .strong()
                                    .color(TEXT_PRIMARY),
                            );
                        });
                    });
                });
        });
    }

    // =========================================================================
    // VIEW 2: SCREENSHOT ANNOTATION EDITOR (Arrow, Frame, Note, Text + 80-color Palette)
    // =========================================================================
    fn render_editor(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        // Editor toolbar
        egui::Frame::new()
            .fill(CARD_BG)
            .stroke(Stroke::new(1.0, BORDER_LIGHT))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let tools = [
                        (AnnotationTool::Select, None),
                        (AnnotationTool::Arrow, Some(DockIcon::ArrowTool)),
                        (AnnotationTool::Frame, Some(DockIcon::FrameTool)),
                        (AnnotationTool::Note, Some(DockIcon::NoteTool)),
                        (AnnotationTool::Text, Some(DockIcon::TextTool)),
                    ];
                    for (tool, _icon) in tools {
                        let active = self.annotations.active_tool == tool;
                        let btn = egui::Button::new(
                            RichText::new(tool.label())
                                .size(13.0)
                                .strong()
                                .color(if active { Color32::WHITE } else { TEXT_PRIMARY }),
                        )
                        .fill(if active { PURPLE_ACTION } else { Color32::WHITE })
                        .corner_radius(CornerRadius::same(6));
                        if ui.add_sized(Vec2::new(72.0, 30.0), btn).clicked() {
                            self.annotations.active_tool = tool;
                        }
                    }

                    ui.separator();

                    // Style controls (color swatches, opacity, stroke, font size)
                    let active_stroke = self
                        .annotations
                        .selected_mut()
                        .map(|a| a.stroke_color)
                        .unwrap_or(match self.annotations.active_tool {
                            AnnotationTool::Arrow => self.annotations.defaults.arrow_color,
                            AnnotationTool::Frame => self.annotations.defaults.frame_stroke,
                            AnnotationTool::Note => self.annotations.defaults.note_stroke,
                            _ => self.annotations.defaults.text_font_color,
                        });
                    if self.color_swatch_btn(ui, "Color", active_stroke) {
                        self.show_color_picker = if self.show_color_picker == Some(ColorTarget::Stroke) {
                            None
                        } else {
                            Some(ColorTarget::Stroke)
                        };
                    }

                    let active_fill = self
                        .annotations
                        .selected_mut()
                        .map(|a| a.fill_color)
                        .unwrap_or(self.annotations.defaults.frame_fill);
                    if self.color_swatch_btn(ui, "Fill", active_fill) {
                        self.show_color_picker = if self.show_color_picker == Some(ColorTarget::Fill) {
                            None
                        } else {
                            Some(ColorTarget::Fill)
                        };
                    }

                    // Opacity slider for Frame / Note
                    let mut op = self
                        .annotations
                        .selected_mut()
                        .map(|a| a.opacity_pct)
                        .unwrap_or(if self.annotations.active_tool == AnnotationTool::Note {
                            self.annotations.defaults.note_opacity
                        } else {
                            self.annotations.defaults.frame_opacity
                        });
                    ui.label(RichText::new("Opacity").size(12.0).color(TEXT_SECONDARY));
                    if ui.add(egui::Slider::new(&mut op, 0..=100).suffix("%").show_value(true)).changed() {
                        if let Some(sel) = self.annotations.selected_mut() {
                            sel.opacity_pct = op;
                        } else if self.annotations.active_tool == AnnotationTool::Note {
                            self.annotations.defaults.note_opacity = op;
                        } else {
                            self.annotations.defaults.frame_opacity = op;
                        }
                    }

                    ui.separator();

                    if ui
                        .add_enabled(!self.annotations.undo_stack.is_empty(), egui::Button::new("Undo"))
                        .on_hover_text("Ctrl+Z")
                        .clicked()
                    {
                        self.annotations.undo();
                    }
                    if ui
                        .add_enabled(!self.annotations.redo_stack.is_empty(), egui::Button::new("Redo"))
                        .on_hover_text("Ctrl+Y")
                        .clicked()
                    {
                        self.annotations.redo();
                    }
                    if ui.button("Clear").clicked() {
                        self.annotations.clear();
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let save_btn = egui::Button::new(
                            RichText::new("Save PNG").strong().color(Color32::WHITE),
                        )
                        .fill(PURPLE_ACTION);
                        if ui.add(save_btn).clicked() {
                            self.save_annotated_image(ctx, self.editor_item_id.is_some());
                        }
                        if ui.button("Copy Image").clicked() {
                            self.copy_editor_to_clipboard();
                        }
                    });
                });

                // Inline text editor when a Note or Text item is selected
                let mut text_edit_val: Option<(String, f32)> = self.annotations.selected_mut().and_then(|sel| {
                    if matches!(sel.tool, AnnotationTool::Note | AnnotationTool::Text) {
                        Some((sel.text.clone(), sel.font_size))
                    } else {
                        None
                    }
                });
                if let Some((ref mut txt, ref mut fsize)) = text_edit_val {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Annotation Text:").size(12.0).color(PURPLE_PRIMARY));
                        let changed_t = ui
                            .add(egui::TextEdit::singleline(txt).desired_width(260.0))
                            .changed();
                        ui.label(RichText::new("Font Size:").size(12.0).color(TEXT_SECONDARY));
                        let changed_s = ui.add(egui::Slider::new(fsize, 12.0..=48.0)).changed();
                        if ui.button("Delete Selected").clicked() {
                            self.annotations.delete_selected();
                        } else if changed_t || changed_s {
                            if let Some(sel) = self.annotations.selected_mut() {
                                sel.text = txt.clone();
                                sel.font_size = *fsize;
                            }
                        }
                    });
                }

                // 80-color palette popup (10x8 from ColorPalette.qml)
                if let Some(target) = self.show_color_picker {
                    ui.add_space(6.0);
                    egui::Frame::new()
                        .fill(SURFACE_BG)
                        .stroke(Stroke::new(1.0, BORDER_LIGHT))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(match target {
                                        ColorTarget::Stroke => "Choose Stroke / Arrow / Text Color:",
                                        ColorTarget::Fill => "Choose Fill Color:",
                                        ColorTarget::Font => "Choose Font Color:",
                                    })
                                    .size(12.0)
                                    .strong()
                                    .color(PURPLE_PRIMARY),
                                );
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui.small_button("Close").clicked() {
                                        self.show_color_picker = None;
                                    }
                                });
                            });
                            ui.add_space(4.0);
                            egui::Grid::new("palette_80_grid")
                                .spacing(Vec2::splat(4.0))
                                .show(ui, |ui| {
                                    for (idx, &col) in ANNOTATION_PALETTE.iter().enumerate() {
                                        let (r, resp) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::click());
                                        ui.painter().rect_filled(r, CornerRadius::same(3), col);
                                        ui.painter().rect_stroke(
                                            r,
                                            CornerRadius::same(3),
                                            Stroke::new(1.0, Color32::from_gray(190)),
                                            egui::StrokeKind::Inside,
                                        );
                                        if resp.clicked() {
                                            self.apply_chosen_color(target, col);
                                            self.show_color_picker = None;
                                        }
                                        if (idx + 1) % 10 == 0 {
                                            ui.end_row();
                                        }
                                    }
                                });
                        });
                }
            });

        ui.add_space(10.0);

        // Canvas stage
        let avail = ui.available_size();
        egui::Frame::new()
            .fill(Color32::from_rgb(0x22, 0x22, 0x2A))
            .corner_radius(CornerRadius::same(10))
            .show(ui, |ui| {
                let (canvas_rect, _) = ui.allocate_exact_size(avail, Sense::hover());
                if let (Some(ref tex), Some(ref img)) = (&self.editor_texture, &self.editor_image) {
                    let aspect = img.width() as f32 / img.height().max(1) as f32;
                    let inner = canvas_rect.shrink(14.0);
                    let fit_w = inner.width().min(inner.height() * aspect);
                    let fit_h = fit_w / aspect;
                    let img_rect = Rect::from_center_size(inner.center(), Vec2::new(fit_w, fit_h));

                    ui.painter().image(
                        tex.id(),
                        img_rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );

                    let resp = ui.interact(img_rect, ui.id().with("ann_canvas"), Sense::click_and_drag());
                    if self.annotations.active_tool == AnnotationTool::Select {
                        if resp.clicked() {
                            if let Some(pos) = resp.interact_pointer_pos() {
                                self.annotations.selected_id = self.annotations.hit_test(img_rect, pos);
                            }
                        }
                        if resp.drag_started() {
                            if let Some(pos) = resp.interact_pointer_pos() {
                                if let Some(id) = self.annotations.hit_test(img_rect, pos) {
                                    self.annotations.push_undo();
                                    self.annotations.selected_id = Some(id);
                                    self.annotations.dragging_id = Some((id, pos));
                                }
                            }
                        } else if resp.dragged() {
                            if let (Some((id, prev)), Some(pos)) = (self.annotations.dragging_id, resp.interact_pointer_pos()) {
                                let dn_x = (pos.x - prev.x) / img_rect.width().max(1.0);
                                let dn_y = (pos.y - prev.y) / img_rect.height().max(1.0);
                                if let Some(ann) = self.annotations.items.iter_mut().find(|a| a.id == id) {
                                    ann.start[0] = (ann.start[0] + dn_x).clamp(0.0, 1.0);
                                    ann.start[1] = (ann.start[1] + dn_y).clamp(0.0, 1.0);
                                    ann.end[0] = (ann.end[0] + dn_x).clamp(0.0, 1.0);
                                    ann.end[1] = (ann.end[1] + dn_y).clamp(0.0, 1.0);
                                }
                                self.annotations.dragging_id = Some((id, pos));
                            }
                        } else if resp.drag_stopped() {
                            self.annotations.dragging_id = None;
                        }
                    } else if self.annotations.active_tool == AnnotationTool::Text {
                        if resp.clicked() {
                            if let Some(pos) = resp.interact_pointer_pos() {
                                let n = to_norm(img_rect, pos);
                                self.annotations.push_undo();
                                let ann = self.annotations.new_annotation(AnnotationTool::Text, n, [n[0] + 0.12, n[1] + 0.05]);
                                self.annotations.selected_id = Some(ann.id);
                                self.annotations.items.push(ann);
                            }
                        }
                    } else {
                        // Drag-to-draw Arrow, Frame, or Note
                        if resp.drag_started() {
                            if let Some(pos) = resp.interact_pointer_pos() {
                                let n = to_norm(img_rect, pos);
                                let draft = self.annotations.new_annotation(self.annotations.active_tool, n, n);
                                self.annotations.drawing = Some(draft);
                            }
                        } else if resp.dragged() {
                            if let (Some(ref mut draft), Some(pos)) = (&mut self.annotations.drawing, resp.interact_pointer_pos()) {
                                draft.end = to_norm(img_rect, pos);
                            }
                        } else if resp.drag_stopped() {
                            if let Some(draft) = self.annotations.drawing.take() {
                                self.annotations.push_undo();
                                self.annotations.selected_id = Some(draft.id);
                                self.annotations.items.push(draft);
                            }
                        }
                    }

                    self.annotations.paint(ui.painter(), img_rect);
                } else {
                    ui.painter().text(
                        canvas_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "Take a screenshot to start annotating",
                        FontId::proportional(16.0),
                        Color32::from_gray(200),
                    );
                }
            });
    }

    // =========================================================================
    // VIDEO EDITOR (v1): timeline trim / cut / reorder / merge / export MP4
    // =========================================================================
    fn render_video_editor(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let Some(mut ed) = self.video_editor.take() else {
            return;
        };
        let mut close_editor = false;
        let mut toast: Option<(String, bool)> = None;
        let mut export_done: Option<Result<(u64, String, PathBuf), String>> = None;
        let mut start_export = false;
        let mut do_split = false;
        let mut do_remove: Option<usize> = None;
        let mut do_move: Option<(usize, i32)> = None;
        let mut do_add: Option<u64> = None;

        // Poll background export progress/result.
        if ed.exporting {
            let progress = ed.export_progress.lock().map(|g| *g).unwrap_or(0.0);
            if let Some(rx) = ed.export_rx.take() {
                match rx.try_recv() {
                    Ok(r) => {
                        ed.exporting = false;
                        export_done = Some(r);
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        ed.export_rx = Some(rx);
                        ed.export_msg = format!("Exporting… {:.0}%", progress * 100.0);
                        ctx.request_repaint_after(Duration::from_millis(80));
                    }
                    Err(_) => {
                        ed.exporting = false;
                        toast = Some(("Export thread stopped unexpectedly.".into(), true));
                    }
                }
            }
        }

        egui::Frame::new()
            .fill(CARD_BG)
            .stroke(Stroke::new(1.0, BORDER_LIGHT))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("← Back to Gallery").clicked() {
                        close_editor = true;
                    }
                    ui.label(
                        RichText::new("Video Editor")
                            .size(16.0)
                            .strong()
                            .color(PURPLE_PRIMARY),
                    );
                    ui.label(
                        RichText::new(format!(
                            "{} x {} @ {} FPS — {} clip(s) • Export writes a native MP4 (no FFmpeg)",
                            ed.width,
                            ed.height,
                            ed.fps,
                            ed.clips.len()
                        ))
                        .size(11.5)
                        .color(TEXT_SECONDARY),
                    );
                });

                ui.add_space(8.0);

                // Add another video from the Gallery into the timeline.
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Merge:").strong());
                    let mut cur = ed.add_pending.unwrap_or_default();
                    let clicked = egui::ComboBox::from_id_salt("editor_add_src")
                        .width(260.0)
                        .selected_text(
                            self.items
                                .iter()
                                .find(|i| Some(i.id) == ed.add_pending)
                                .map(|i| i.name.as_str())
                                .unwrap_or("Choose video…"),
                        )
                        .show_ui(ui, |ui| {
                            let mut picked = None;
                            for item in self.items.iter().filter(|i| i.kind == MediaKind::Video) {
                                if ui.selectable_label(cur == item.id, &item.name).clicked() {
                                    picked = Some(item.id);
                                }
                            }
                            picked
                        })
                        .inner;
                    if let Some(p) = clicked {
                        ed.add_pending = p;
                    }
                    if ui.button("+ Add to Timeline").clicked() {
                        if let Some(p) = ed.add_pending {
                            do_add = Some(p);
                        } else {
                            toast = Some(("Pick a video from the list first.".into(), true));
                        }
                    }
                });

                ui.add_space(10.0);
                // Timeline rows
                if ed.clips.is_empty() {
                    ui.label(RichText::new("Timeline is empty — add clips above.").size(12.0));
                } else {
                    let mut drop: Option<(usize, usize)> = None;
                    egui::ScrollArea::vertical()
                        .max_height(240.0)
                        .show(ui, |ui| {
                            for i in 0..ed.clips.len() {
                                let sel = i == ed.selected;
                                let src_name = ed.clips[i]
                                    .source
                                    .file_name()
                                    .map(|s| s.to_string_lossy().into_owned())
                                    .unwrap_or_default();
                                egui::Frame::new()
                                    .fill(if sel {
                                        PURPLE_SOFT
                                    } else {
                                        Color32::WHITE
                                    })
                                    .stroke(Stroke::new(
                                        if sel { 1.5 } else { 1.0 },
                                        if sel { PURPLE_PRIMARY } else { BORDER_LIGHT },
                                    ))
                                    .corner_radius(CornerRadius::same(8))
                                    .inner_margin(egui::Margin::same(8))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            if ui.selectable_label(sel, format!("#{}", i + 1)).clicked() {
                                                ed.selected = i;
                                                ed.thumb = None;
                                                ed.thumb_frame = None;
                                            }
                                            ui.label(RichText::new(&ed.clips[i].label).strong().size(12.0));
                                            ui.label(
                                                RichText::new(src_name)
                                                    .size(10.5)
                                                    .color(TEXT_SECONDARY),
                                            );
                                            ui.with_layout(
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    if ui.small_button("✕").on_hover_text("Remove clip").clicked()
                                                    {
                                                        do_remove = Some(i);
                                                    }
                                                    if ui.small_button("↓").clicked() {
                                                        do_move = Some((i, 1));
                                                    }
                                                    if ui.small_button("↑").clicked() {
                                                        do_move = Some((i, -1));
                                                    }
                                                },
                                            );
                                        });
                                        ui.add_space(4.0);
                                        let total = ed.clips[i].total_sec.max(0.1);
                                        ui.horizontal(|ui| {
                                            ui.label("Start");
                                            let mut s = ed.clips[i].start_sec as f32;
                                            if ui
                                                .add(
                                                    egui::Slider::new(&mut s, 0.0..=total as f32)
                                                        .suffix(" s"),
                                                )
                                                .changed()
                                            {
                                                ed.clips[i].start_sec = s as f64;
                                                if ed.clips[i].start_sec >= ed.clips[i].end_sec {
                                                    ed.clips[i].end_sec =
                                                        (ed.clips[i].start_sec + 0.1).min(total);
                                                }
                                            }
                                            ui.label("End");
                                            let mut e = ed.clips[i].end_sec as f32;
                                            if ui
                                                .add(
                                                    egui::Slider::new(&mut e, 0.0..=total as f32)
                                                        .suffix(" s"),
                                                )
                                                .changed()
                                            {
                                                ed.clips[i].end_sec = e as f64;
                                                if ed.clips[i].end_sec <= ed.clips[i].start_sec {
                                                    ed.clips[i].start_sec =
                                                        (ed.clips[i].end_sec - 0.1).max(0.0);
                                                }
                                            }
                                            ui.label(
                                                RichText::new(format!(
                                                    "{:.1}s → {:.1}s  = {:.1}s",
                                                    ed.clips[i].start_sec,
                                                    ed.clips[i].end_sec,
                                                    ed.clips[i].duration_sec()
                                                ))
                                                .size(11.0)
                                                .color(TEXT_SECONDARY),
                                            );
                                        });
                                    });
                            }
                        });

                    // Split the selected clip.
                    let idx = ed.selected.min(ed.clips.len().saturating_sub(1));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let c = &ed.clips[idx];
                        let total = c.total_sec.max(0.1);
                        ui.label(format!(
                            "Split #{} at:",
                            idx + 1
                        ));
                        ui.add(
                            egui::Slider::new(&mut ed.split_sec, 0.0..=total as f32).suffix(" s"),
                        );
                        if ui
                            .button("Cut Here")
                            .on_hover_text("Splits this clip into two — then remove the part you do not want")
                            .clicked()
                        {
                            do_split = true;
                        }
                    });
                }

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(8.0);

                // Preview of the selected clip's first frame.
                if !ed.clips.is_empty() {
                    let idx = ed.selected.min(ed.clips.len() - 1);
                    if ed.thumb_frame.is_none() && ed.thumb.is_none() {
                        if let Some(c) = ed.clips.get(idx) {
                            if let Some(frame) = first_video_frame(&c.source) {
                                let tex = ctx.load_texture(
                                    "video_editor_thumb",
                                    rgba_to_color_image(&frame),
                                    TextureOptions::LINEAR,
                                );
                                ed.thumb = Some(tex);
                                ed.thumb_frame = Some(frame);
                            }
                        }
                    }
                    if let Some(ref tex) = ed.thumb {
                        let [tw, th] = tex.size();
                        let max_w = 240.0;
                        let scale = (max_w / tw.max(1) as f32).min(1.0);
                        let size = Vec2::new(tw as f32 * scale, th as f32 * scale);
                        let (r, _) = ui.allocate_exact_size(size, Sense::hover());
                        ui.painter().image(
                            tex.id(),
                            r,
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                }

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let total_out: f64 = ed
                        .clips
                        .iter()
                        .map(|c| c.duration_sec())
                        .sum();
                    if ed.exporting {
                        ui.vertical(|ui| {
                            ui.add(
                                egui::ProgressBar::new(
                                    ed.export_progress.lock().map(|g| *g).unwrap_or(0.0),
                                )
                                    .desired_width(320.0)
                                    .text(&ed.export_msg),
                            );
                        });
                    } else {
                        let btn = egui::Button::new(
                            RichText::new(format!(
                                "Export MP4  ({:.1}s total)",
                                total_out
                            ))
                            .strong()
                            .color(Color32::WHITE),
                        )
                        .fill(PURPLE_ACTION);
                        if ui.add(btn).clicked() {
                            start_export = true;
                        }
                    }
                });
            });

        // ---- Apply editor actions ------------------------------------------
        if do_split {
            let idx = ed.selected.min(ed.clips.len().saturating_sub(1));
            let c = &ed.clips[idx];
            let total = c.total_sec;
            let pos = (ed.split_sec as f64)
                .clamp((c.start_sec + 0.05).min(c.end_sec - 0.05), (c.end_sec - 0.05).max(c.start_sec + 0.05));
            if c.duration_sec() > 0.3 {
                let mut a = c.clone();
                a.end_sec = pos;
                let mut b = c.clone();
                b.start_sec = pos;
                ed.clips.splice(idx..=idx, [a, b]);
                ed.selected = idx;
            } else {
                toast = Some(("Clip too short to split.".into(), true));
            }
            ed.thumb = None;
            ed.thumb_frame = None;
        }
        if let Some(i) = do_remove {
            if ed.clips.len() > 1 {
                ed.clips.remove(i);
                ed.selected = ed.selected.min(ed.clips.len() - 1);
                ed.thumb = None;
                ed.thumb_frame = None;
            } else {
                toast = Some(("A timeline needs at least one clip.".into(), true));
            }
        }
        if let Some((i, d)) = do_move {
            let j = (i as i32 + d).clamp(0, ed.clips.len() as i32 - 1) as usize;
            ed.clips.swap(i, j);
            ed.selected = j;
        }
        if let Some(id) = do_add {
            if let Some(item) = self.items.iter().find(|it| it.id == id).cloned() {
                let src = PathBuf::from(&item.file_path);
                let mut total = item.duration_secs as f64;
                let mut fps = ed.fps;
                if let Ok(p) = crate::editor::probe(&src) {
                    if total <= 0.0 {
                        total = p.5;
                    }
                    fps = p.2.clamp(5, 60);
                }
                ed.clips.push(crate::editor::ClipDef::new(
                    src,
                    item.name.clone(),
                    if total > 0.0 { total } else { 10.0 },
                ));
                ed.fps = fps;
                ed.selected = ed.clips.len() - 1;
                ed.add_pending = None;
                ed.thumb = None;
                ed.thumb_frame = None;
            }
        }
        if start_export {
            let clips: Vec<crate::editor::ClipDef> = ed
                .clips
                .iter()
                .filter(|c| c.duration_sec() > 0.05)
                .cloned()
                .collect();
            if clips.is_empty() {
                toast = Some(("No valid clips to export — set the start/end ranges.".into(), true));
            } else {
                let (id, name, out_path, _prev) =
                    self.storage.next_file_paths_with_ext(MediaKind::Video, "mp4");
                let w = ed.width.max(2);
                let h = ed.height.max(2);
                let fps = ed.fps.clamp(5, 60);
                let progress = ed.export_progress.clone();
                *ed.export_progress.lock().unwrap() = 0.0;
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::Builder::new()
                    .name("editor_export".into())
                    .spawn(move || {
                        let res = crate::editor::build_mp4(
                            &clips,
                            &out_path,
                            w,
                            h,
                            fps,
                            move |p| {
                                if let Ok(mut g) = progress.lock() {
                                    *g = p;
                                }
                            },
                        );
                        let _ = tx.send(res.map(|_| (id, name, out_path)));
                    })
                    .ok();
                ed.export_rx = Some(rx);
                ed.exporting = true;
                ed.export_msg = "Starting export…".into();
            }
        }

        self.video_editor = Some(ed);

        if close_editor {
            self.video_editor = None;
            self.view = ActiveView::Gallery;
        }
        if let Some((msg, is_err)) = toast {
            self.show_toast(msg, is_err);
        }
        if let Some(res) = export_done {
            match res {
                Ok((id, name, path)) => {
                    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    let dur: f32 = self
                        .video_editor
                        .as_ref()
                        .map(|e| e.clips.iter().map(|c| c.duration_sec() as f32).sum())
                        .unwrap_or(0.0);
                    let item = MediaItem {
                        id,
                        name,
                        kind: MediaKind::Video,
                        file_path: path.to_string_lossy().to_string(),
                        preview_path: path.to_string_lossy().to_string(),
                        created_unix: StorageManager::now_unix(),
                        size_bytes: size,
                        duration_secs: dur.max(0.1),
                        width: 0,
                        height: 0,
                    };
                    self.items.insert(0, item);
                    self.storage.save_items(&self.items);
                    self.open_video_in_player(ctx, id);
                    self.show_toast(
                        format!("Video exported — MP4 saved ({}).", format_size(size)),
                        false,
                    );
                }
                Err(e) => {
                    self.show_toast(format!("Export failed: {e}"), true);
                }
            }
        }
    }

    fn color_swatch_btn(&self, ui: &mut egui::Ui, label: &str, col: Color32) -> bool {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(66.0, 28.0), Sense::click());
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(6),
            if resp.hovered() { PURPLE_SOFT } else { Color32::WHITE },
        );
        ui.painter().rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, BORDER_LIGHT), egui::StrokeKind::Inside);
        let sw = Rect::from_min_size(rect.min + Vec2::new(6.0, 6.0), Vec2::splat(16.0));
        ui.painter().rect_filled(sw, CornerRadius::same(3), col);
        ui.painter().rect_stroke(sw, CornerRadius::same(3), Stroke::new(1.0, Color32::from_gray(150)), egui::StrokeKind::Inside);
        ui.painter().text(
            Pos2::new(sw.right() + 6.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            FontId::proportional(12.0),
            TEXT_PRIMARY,
        );
        resp.clicked()
    }

    fn apply_chosen_color(&mut self, target: ColorTarget, col: Color32) {
        if let Some(sel) = self.annotations.selected_mut() {
            match target {
                ColorTarget::Stroke => {
                    sel.stroke_color = col;
                    if matches!(sel.tool, AnnotationTool::Arrow | AnnotationTool::Text) {
                        sel.font_color = col;
                        sel.fill_color = col;
                    }
                }
                ColorTarget::Fill => sel.fill_color = col,
                ColorTarget::Font => sel.font_color = col,
            }
        } else {
            match target {
                ColorTarget::Stroke => {
                    self.annotations.defaults.arrow_color = col;
                    self.annotations.defaults.frame_stroke = col;
                    self.annotations.defaults.note_stroke = col;
                    self.annotations.defaults.note_font_color = col;
                    self.annotations.defaults.text_font_color = col;
                }
                ColorTarget::Fill => {
                    self.annotations.defaults.frame_fill = col;
                    self.annotations.defaults.note_fill = col;
                }
                ColorTarget::Font => {
                    self.annotations.defaults.note_font_color = col;
                    self.annotations.defaults.text_font_color = col;
                }
            }
        }
    }

    // =========================================================================
    // VIEW 3: GALLERY (All/Images/Videos counts, Search, Sort, Multi-select, Cards)
    // =========================================================================
    fn render_gallery(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let all_count = self.items.len();
        let img_count = self.items.iter().filter(|i| i.kind == MediaKind::Image).count();
        let vid_count = self.items.iter().filter(|i| i.kind == MediaKind::Video).count();

        // Filter & Sort bar (matching GalleryMediaTypes.qml & GallerySortDropdown.qml)
        egui::Frame::new()
            .fill(CARD_BG)
            .stroke(Stroke::new(1.0, BORDER_LIGHT))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let filters = [
                        (GalleryFilter::All, format!("All: {all_count}")),
                        (GalleryFilter::Images, format!("Images: {img_count}")),
                        (GalleryFilter::Videos, format!("Videos: {vid_count}")),
                    ];
                    for (f, lbl) in filters {
                        let sel = self.gallery_filter == f;
                        let btn = egui::Button::new(
                            RichText::new(lbl)
                                .size(13.0)
                                .strong()
                                .color(if sel { GALLERY_BLUE } else { TEXT_SECONDARY }),
                        )
                        .fill(if sel { Color32::from_rgb(0xE8, 0xF3, 0xFC) } else { Color32::TRANSPARENT })
                        .stroke(if sel { Stroke::new(1.0, GALLERY_BLUE) } else { Stroke::NONE });
                        if ui.add(btn).clicked() {
                            self.gallery_filter = f;
                        }
                    }

                    ui.separator();

                    ui.add(
                        egui::TextEdit::singleline(&mut self.gallery_search)
                            .hint_text("Search captures...")
                            .desired_width(210.0),
                    );

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let order_lbl = if self.gallery_desc { "Desc v" } else { "Asc ^" };
                        if ui.button(order_lbl).clicked() {
                            self.gallery_desc = !self.gallery_desc;
                        }
                        egui::ComboBox::from_id_salt("gallery_sort_combo")
                            .selected_text(match self.gallery_sort {
                                GallerySort::Date => "Sort by: Date",
                                GallerySort::Name => "Sort by: Name",
                                GallerySort::Size => "Sort by: Size",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.gallery_sort, GallerySort::Date, "Date");
                                ui.selectable_value(&mut self.gallery_sort, GallerySort::Name, "Name");
                                ui.selectable_value(&mut self.gallery_sort, GallerySort::Size, "Size");
                            });

                        if ui.button("Open Folder").clicked() {
                            StorageManager::open_in_explorer(&self.storage.records_dir);
                        }
                        let has_avi = self.items.iter().any(|i| {
                            i.kind == MediaKind::Video
                                && i.file_path.to_ascii_lowercase().ends_with(".avi")
                        });
                        if has_avi
                            && ui
                                .button("Convert AVI -> MP4")
                                .on_hover_text("Convert all AVI recordings to compact MP4 to save disk space")
                                .clicked()
                        {
                            self.convert_all_avi_to_mp4();
                        }
                    });
                });

                // Multi-select toolbar (matching GallerySelectionToolbar.qml)
                if !self.gallery_selected.is_empty() {
                    ui.add_space(8.0);
                    egui::Frame::new()
                        .fill(Color32::from_rgb(0xE8, 0xF3, 0xFC))
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(egui::Margin::symmetric(10, 6))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!("{} items selected", self.gallery_selected.len()))
                                        .strong()
                                        .color(GALLERY_BLUE),
                                );
                                if ui.button("Select All").clicked() {
                                    for item in &self.items {
                                        self.gallery_selected.insert(item.id);
                                    }
                                }
                                if ui.button("Clear Selection").clicked() {
                                    self.gallery_selected.clear();
                                    self.gallery_confirm_delete = false;
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if self.gallery_confirm_delete {
                                        let confirm = egui::Button::new(
                                            RichText::new("Confirm Delete").strong().color(Color32::WHITE),
                                        )
                                        .fill(DANGER_RED);
                                        if ui.add(confirm).clicked() {
                                            let ids: Vec<u64> = self.gallery_selected.iter().copied().collect();
                                            self.delete_items(&ids);
                                            self.gallery_confirm_delete = false;
                                        }
                                        if ui.button("Cancel").clicked() {
                                            self.gallery_confirm_delete = false;
                                        }
                                    } else {
                                        let del_btn = egui::Button::new(
                                            RichText::new("Delete Selected").strong().color(Color32::WHITE),
                                        )
                                        .fill(DANGER_RED);
                                        if ui.add(del_btn).clicked() {
                                            self.gallery_confirm_delete = true;
                                        }
                                    }
                                });
                            });
                        });
                }
            });

        ui.add_space(10.0);

        // Filter & sort items
        let q = self.gallery_search.trim().to_lowercase();
        let mut filtered: Vec<MediaItem> = self
            .items
            .iter()
            .filter(|i| match self.gallery_filter {
                GalleryFilter::All => true,
                GalleryFilter::Images => i.kind == MediaKind::Image,
                GalleryFilter::Videos => i.kind == MediaKind::Video,
            })
            .filter(|i| q.is_empty() || i.name.to_lowercase().contains(&q))
            .cloned()
            .collect();

        filtered.sort_by(|a, b| {
            let ord = match self.gallery_sort {
                GallerySort::Date => a.created_unix.cmp(&b.created_unix),
                GallerySort::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                GallerySort::Size => a.size_bytes.cmp(&b.size_bytes),
            };
            if self.gallery_desc { ord.reverse() } else { ord }
        });

        if filtered.is_empty() {
            egui::Frame::new()
                .fill(CARD_BG)
                .stroke(Stroke::new(1.0, BORDER_LIGHT))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(egui::Margin::same(40))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new("No captures yet")
                                .size(18.0)
                                .strong()
                                .color(TEXT_PRIMARY),
                        );
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new("Take a screenshot (Alt+S) or record a video from Capture Studio to see it here.")
                                .size(13.5)
                                .color(TEXT_SECONDARY),
                        );
                        ui.add_space(14.0);
                        let cta = egui::Button::new(
                            RichText::new("Take First Screenshot").strong().color(Color32::WHITE),
                        )
                        .fill(PURPLE_ACTION);
                        if ui.add_sized(Vec2::new(190.0, 36.0), cta).clicked() {
                            self.take_screenshot(ctx, false);
                        }
                    });
                });
            return;
        }

        let mut action_open: Option<(u64, MediaKind)> = None;
        let mut action_delete: Option<u64> = None;
        let mut action_copy_path: Option<String> = None;
        let mut action_export_mp4: Option<u64> = None;
        let mut action_edit_video: Option<u64> = None;
        let mut action_commit_rename: Option<(u64, String)> = None;

        egui::ScrollArea::vertical().show(ui, |ui| {
            let cols = ((ui.available_width() / 285.0).floor() as usize).clamp(1, 4);
            let card_w = (ui.available_width() - (cols as f32 - 1.0) * 12.0) / cols as f32;

            egui::Grid::new("gallery_cards_grid")
                .spacing(Vec2::splat(12.0))
                .show(ui, |ui| {
                    for (idx, item) in filtered.iter().enumerate() {
                        let thumb = self.ensure_thumbnail(ctx, item);
                        let selected = self.gallery_selected.contains(&item.id);

                        egui::Frame::new()
                            .fill(CARD_BG)
                            .stroke(Stroke::new(
                                if selected { 2.0 } else { 1.0 },
                                if selected { GALLERY_BLUE } else { BORDER_LIGHT },
                            ))
                            .corner_radius(CornerRadius::same(10))
                            .inner_margin(egui::Margin::same(8))
                            .show(ui, |ui| {
                                ui.set_width((card_w - 18.0).max(200.0));
                                ui.vertical(|ui| {
                                    // Preview thumbnail area
                                    let thumb_h = 142.0;
                                    let (thumb_rect, thumb_resp) = ui.allocate_exact_size(
                                        Vec2::new(ui.available_width(), thumb_h),
                                        Sense::click(),
                                    );
                                    ui.painter().rect_filled(
                                        thumb_rect,
                                        CornerRadius::same(6),
                                        Color32::from_rgb(0x1E, 0x1E, 0x26),
                                    );
                                    if let Some(ref tex) = thumb {
                                        ui.painter().image(
                                            tex.id(),
                                            thumb_rect,
                                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                            Color32::WHITE,
                                        );
                                    }
                                    // Type / Duration badge
                                    let badge = match item.kind {
                                        MediaKind::Image => format!("PNG  {}x{}", item.width, item.height),
                                        MediaKind::Video => format!("VIDEO  {}", format_duration(item.duration_secs)),
                                    };
                                    let b_rect = Rect::from_min_size(
                                        Pos2::new(thumb_rect.left() + 6.0, thumb_rect.bottom() - 24.0),
                                        Vec2::new(112.0, 18.0),
                                    );
                                    ui.painter().rect_filled(
                                        b_rect,
                                        CornerRadius::same(4),
                                        Color32::from_rgba_unmultiplied(0, 0, 0, 180),
                                    );
                                    ui.painter().text(
                                        b_rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        badge,
                                        FontId::proportional(10.5),
                                        Color32::WHITE,
                                    );

                                    if thumb_resp.clicked() {
                                        action_open = Some((item.id, item.kind));
                                    }

                                    ui.add_space(6.0);

                                    // Title row + checkbox
                                    ui.horizontal(|ui| {
                                        let mut is_sel = selected;
                                        if ui.checkbox(&mut is_sel, "").changed() {
                                            if is_sel {
                                                self.gallery_selected.insert(item.id);
                                            } else {
                                                self.gallery_selected.remove(&item.id);
                                            }
                                        }
                                        if let Some((ren_id, ref mut buf)) = self.gallery_renaming {
                                            if ren_id == item.id {
                                                let r = ui.add(egui::TextEdit::singleline(buf).desired_width(120.0));
                                                if ui.small_button("OK").clicked() || r.lost_focus() {
                                                    action_commit_rename = Some((item.id, buf.clone()));
                                                }
                                            } else {
                                                ui.label(RichText::new(&item.name).strong().size(12.5));
                                            }
                                        } else {
                                            ui.label(RichText::new(&item.name).strong().size(12.5));
                                        }
                                    });

                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(format_size(item.size_bytes))
                                                .size(11.0)
                                                .color(TEXT_SECONDARY),
                                        );
                                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                            if ui.small_button("Del").clicked() {
                                                action_delete = Some(item.id);
                                            }
                                            if item.kind == MediaKind::Video
                                                && ui
                                                    .small_button("MP4")
                                                    .on_hover_text("Export / reveal as MP4")
                                                    .clicked()
                                            {
                                                action_export_mp4 = Some(item.id);
                                            }
                                            if item.kind == MediaKind::Video
                                                && ui
                                                    .small_button("Edit")
                                                    .on_hover_text("Trim / cut / merge in Video Editor")
                                                    .clicked()
                                            {
                                                action_edit_video = Some(item.id);
                                            }
                                            if ui.small_button("Path").on_hover_text("Copy file path").clicked() {
                                                action_copy_path = Some(item.file_path.clone());
                                            }
                                            if ui.small_button("Rename").clicked() {
                                                self.gallery_renaming = Some((item.id, item.name.clone()));
                                            }
                                            if ui.small_button("Open").clicked() {
                                                action_open = Some((item.id, item.kind));
                                            }
                                        });
                                    });
                                });
                            });

                        if (idx + 1) % cols == 0 {
                            ui.end_row();
                        }
                    }
                });
        });

        if let Some((id, kind)) = action_open {
            match kind {
                MediaKind::Image => self.open_image_in_editor(ctx, id),
                MediaKind::Video => self.open_video_in_player(ctx, id),
            }
        }
        if let Some(id) = action_delete {
            self.delete_items(&[id]);
        }
        if let Some(p) = action_copy_path {
            self.copy_text_to_clipboard(&p, "File path");
        }
        if let Some(id) = action_export_mp4 {
            self.export_item_as_mp4(id);
        }
        if let Some(id) = action_edit_video {
            self.open_video_editor(ctx, id);
        }
        if let Some((id, new_name)) = action_commit_rename {
            let trimmed = new_name.trim().to_string();
            if !trimmed.is_empty() {
                if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
                    item.name = trimmed;
                }
                self.storage.save_items(&self.items);
            }
            self.gallery_renaming = None;
        }
    }

    // =========================================================================
    // VIEW 4: NATIVE VIDEO PLAYER (#250864 bar + #7F54F8 progress per PlayerStyle.qml)
    // =========================================================================
    fn render_player(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        // Advance playback timer (locked to master hardware audio clock when playing at 1x)
        if let Some(ref mut pb) = self.playback {
            if pb.playing {
                ctx.request_repaint();
                let dt = pb.last_tick.elapsed().as_secs_f32() * pb.speed;
                pb.last_tick = Instant::now();
                if let Some(ref ap) = pb.audio_player {
                    if (pb.speed - 1.0).abs() < 0.05 {
                        pb.position_sec = ap.position_seconds();
                        if ap.is_finished() || pb.position_sec >= pb.duration_sec {
                            pb.position_sec = 0.0;
                            ap.seek_seconds(0.0);
                        }
                    } else {
                        pb.position_sec += dt;
                        if pb.position_sec >= pb.duration_sec {
                            pb.position_sec = 0.0;
                            ap.seek_seconds(0.0);
                        } else if (pb.position_sec - ap.position_seconds()).abs() > 0.12 {
                            ap.seek_seconds(pb.position_sec);
                        }
                    }
                } else {
                    pb.position_sec += dt;
                    if pb.position_sec >= pb.duration_sec {
                        pb.position_sec = 0.0;
                    }
                }
            } else {
                pb.last_tick = Instant::now();
            }
            if let Some(ref ap) = pb.audio_player {
                ap.set_playing(pb.playing);
                ap.set_muted(pb.muted);
                ap.set_volume(pb.volume);
            }

            let idx = ((pb.position_sec * pb.video.fps as f32).floor() as usize)
                .min(pb.video.frames.len().saturating_sub(1));
            if idx != pb.last_frame_idx || pb.texture.is_none() {
                pb.last_frame_idx = idx;
                if let Some(rgba) = decode_jpeg_to_rgba(&pb.video.frames[idx]) {
                    let ci = rgba_to_color_image(&rgba);
                    match pb.texture {
                        Some(ref mut tex) => tex.set(ci, TextureOptions::LINEAR),
                        None => {
                            pb.texture = Some(ctx.load_texture(
                                format!("player_frame_{}", pb.item_id),
                                ci,
                                TextureOptions::LINEAR,
                            ));
                        }
                    }
                }
            }
        }

        if self.playback.is_none() {
            // Auto-open newest video if available
            if let Some(vid) = self.items.iter().find(|i| i.kind == MediaKind::Video).cloned() {
                self.open_video_in_player(ctx, vid.id);
            }
        }

        let Some(ref mut pb) = self.playback else {
            egui::Frame::new()
                .fill(CARD_BG)
                .stroke(Stroke::new(1.0, BORDER_LIGHT))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(egui::Margin::same(40))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("No recorded video loaded").size(18.0).strong());
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new("Record a video from Capture Studio to play it back here with scrubbing and speed controls.")
                                .color(TEXT_SECONDARY),
                        );
                        ui.add_space(12.0);
                        if ui.button("Go to Capture Studio").clicked() {
                            self.view = ActiveView::CaptureStudio;
                        }
                    });
                });
            return;
        };

        let video_ids: Vec<u64> = self
            .items
            .iter()
            .filter(|i| i.kind == MediaKind::Video)
            .map(|i| i.id)
            .collect();
        let current_vid_pos = video_ids.iter().position(|&id| id == pb.item_id);
        let prev_vid = current_vid_pos.and_then(|idx| idx.checked_sub(1).and_then(|p| video_ids.get(p).copied()));
        let next_vid = current_vid_pos.and_then(|idx| video_ids.get(idx + 1).copied());
        let mut switch_video: Option<u64> = None;
        let mut export_mp4_id: Option<u64> = None;
        let mut edit_video_id: Option<u64> = None;

        let item_meta = self.items.iter().find(|i| i.id == pb.item_id).cloned();
        let avail = ui.available_size();
        let stage_h = (avail.y - 76.0).max(260.0);

        // Video stage
        egui::Frame::new()
            .fill(Color32::BLACK)
            .corner_radius(CornerRadius::same(10))
            .show(ui, |ui| {
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(avail.x, stage_h), Sense::click());
                if resp.clicked() {
                    pb.playing = !pb.playing;
                    pb.last_tick = Instant::now();
                }

                if let Some(ref tex) = pb.texture {
                    let aspect = pb.video.width as f32 / pb.video.height.max(1) as f32;
                    let fit_w = rect.width().min(rect.height() * aspect);
                    let fit_h = fit_w / aspect;
                    let vid_rect = Rect::from_center_size(rect.center(), Vec2::new(fit_w, fit_h));
                    ui.painter().image(
                        tex.id(),
                        vid_rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }

                // Big centered play button overlay when paused (66px diameter from PlayerStyle.qml)
                if !pb.playing {
                    let c = rect.center();
                    ui.painter().circle_filled(
                        c,
                        33.0,
                        Color32::from_rgba_unmultiplied(PLAYER_BASE.r(), PLAYER_BASE.g(), PLAYER_BASE.b(), 216),
                    );
                    draw_icon(
                        ui.painter(),
                        Rect::from_center_size(c + Vec2::new(2.0, 0.0), Vec2::splat(36.0)),
                        DockIcon::Play,
                        Color32::WHITE,
                    );
                }
            });

        ui.add_space(6.0);

        // Deep purple #250864 Player Control Bar (from PlayerStyle.qml)
        egui::Frame::new()
            .fill(PLAYER_BASE)
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(14, 8))
            .show(ui, |ui| {
                // Scrub progress track (#7F54F8 play fill)
                let (track_rect, track_resp) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 12.0), Sense::click_and_drag());
                let bar = Rect::from_center_size(
                    track_rect.center(),
                    Vec2::new(track_rect.width(), if track_resp.hovered() { 8.0 } else { 5.0 }),
                );
                ui.painter().rect_filled(
                    bar,
                    CornerRadius::same(3),
                    Color32::from_rgba_unmultiplied(255, 255, 255, 76),
                );
                let frac = (pb.position_sec / pb.duration_sec.max(0.01)).clamp(0.0, 1.0);
                let fill_bar = Rect::from_min_max(
                    bar.min,
                    Pos2::new(bar.left() + bar.width() * frac, bar.max.y),
                );
                ui.painter().rect_filled(fill_bar, CornerRadius::same(3), PURPLE_HOVER);

                if (track_resp.clicked() || track_resp.dragged()) && track_rect.width() > 1.0 {
                    if let Some(pos) = track_resp.interact_pointer_pos() {
                        let r = ((pos.x - track_rect.left()) / track_rect.width()).clamp(0.0, 1.0);
                        pb.position_sec = r * pb.duration_sec;
                        if let Some(ref ap) = pb.audio_player {
                            ap.seek_seconds(pb.position_sec);
                        }
                    }
                }

                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    let play_btn = egui::Button::new(
                        RichText::new(if pb.playing { "Pause" } else { "Play" })
                            .strong()
                            .color(Color32::WHITE),
                    )
                    .fill(PURPLE_HOVER);
                    if ui.add_sized(Vec2::new(68.0, 28.0), play_btn).clicked() {
                        pb.playing = !pb.playing;
                        pb.last_tick = Instant::now();
                    }
                    if ui
                        .add(
                            egui::Button::new(RichText::new("-10s").color(Color32::WHITE))
                                .fill(Color32::from_rgba_unmultiplied(255, 255, 255, 24)),
                        )
                        .clicked()
                    {
                        pb.position_sec = (pb.position_sec - 10.0).max(0.0);
                        if let Some(ref ap) = pb.audio_player {
                            ap.seek_seconds(pb.position_sec);
                        }
                    }
                    if ui
                        .add(
                            egui::Button::new(RichText::new("+10s").color(Color32::WHITE))
                                .fill(Color32::from_rgba_unmultiplied(255, 255, 255, 24)),
                        )
                        .clicked()
                    {
                        pb.position_sec = (pb.position_sec + 10.0).min(pb.duration_sec);
                        if let Some(ref ap) = pb.audio_player {
                            ap.seek_seconds(pb.position_sec);
                        }
                    }

                    if ui
                        .add_enabled(
                            prev_vid.is_some(),
                            egui::Button::new(RichText::new("< Back").color(Color32::WHITE))
                                .fill(Color32::from_rgba_unmultiplied(255, 255, 255, 24)),
                        )
                        .on_hover_text("Previous video")
                        .clicked()
                    {
                        switch_video = prev_vid;
                    }
                    if ui
                        .add_enabled(
                            next_vid.is_some(),
                            egui::Button::new(RichText::new("Next >").color(Color32::WHITE))
                                .fill(Color32::from_rgba_unmultiplied(255, 255, 255, 24)),
                        )
                        .on_hover_text("Next video")
                        .clicked()
                    {
                        switch_video = next_vid;
                    }
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(format!(
                            "{} / {}",
                            format_duration(pb.position_sec),
                            format_duration(pb.duration_sec)
                        ))
                        .size(12.5)
                        .color(Color32::WHITE),
                    );

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if let Some(ref meta) = item_meta {
                            if ui
                                .add(
                                    egui::Button::new(RichText::new("Show File").color(Color32::WHITE))
                                        .fill(Color32::from_rgba_unmultiplied(255, 255, 255, 24)),
                                )
                                .clicked()
                            {
                                StorageManager::open_in_explorer(std::path::Path::new(&meta.file_path));
                            }
                            if ui
                                .add(
                                    egui::Button::new(RichText::new("Export MP4").strong().color(Color32::WHITE))
                                        .fill(PURPLE_ACTION),
                                )
                                .clicked()
                            {
                                export_mp4_id = Some(meta.id);
                            }
                            if ui
                                .add(
                                    egui::Button::new(RichText::new("Open Video Editor").color(Color32::WHITE))
                                        .fill(Color32::from_rgba_unmultiplied(255, 255, 255, 24)),
                                )
                                .on_hover_text("Trim / cut / merge this video and export MP4")
                                .clicked()
                            {
                                edit_video_id = Some(meta.id);
                            }
                        }
                        if pb.audio_player.is_some() {
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(if pb.muted { "Muted" } else { "Sound" })
                                            .size(11.5)
                                            .color(Color32::WHITE),
                                    )
                                    .fill(if pb.muted {
                                        DANGER_RED
                                    } else {
                                        Color32::from_rgba_unmultiplied(255, 255, 255, 24)
                                    }),
                                )
                                .clicked()
                            {
                                pb.muted = !pb.muted;
                            }
                        }
                        for spd in [2.0f32, 1.5, 1.0, 0.5] {
                            let active = (pb.speed - spd).abs() < 0.05;
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(format!("{spd}x")).size(11.5).color(Color32::WHITE),
                                    )
                                    .fill(if active {
                                        PURPLE_HOVER
                                    } else {
                                        Color32::from_rgba_unmultiplied(255, 255, 255, 20)
                                    }),
                                )
                                .clicked()
                            {
                                pb.speed = spd;
                            }
                        }
                        ui.label(RichText::new("Speed:").size(11.5).color(Color32::from_gray(200)));
                    });
                });
            });

        if let Some(target_id) = switch_video {
            self.open_video_in_player(ctx, target_id);
        }
        if let Some(id) = export_mp4_id {
            self.export_item_as_mp4(id);
        }
        if let Some(id) = edit_video_id {
            self.open_video_editor(ctx, id);
        }
    }

    // =========================================================================
    // VIEW 5: SETTINGS (Quality, FPS, Capture Method, Audio, Camera, UI, Hotkeys)
    // =========================================================================
    fn render_settings(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.columns(2, |cols| {
                // Column 1: Video & Capture
                egui::Frame::new()
                    .fill(CARD_BG)
                    .stroke(Stroke::new(1.0, BORDER_LIGHT))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(egui::Margin::same(16))
                    .show(&mut cols[0], |ui| {
                        ui.label(
                            RichText::new("Recording Quality & Capture")
                                .size(15.5)
                                .strong()
                                .color(PURPLE_PRIMARY),
                        );
                        ui.add_space(10.0);

                        ui.label(RichText::new("Recording Quality").strong());
                        egui::ComboBox::from_id_salt("settings_quality")
                            .width(240.0)
                            .selected_text(self.settings.quality.label())
                            .show_ui(ui, |ui| {
                                for q in [
                                    RecordingQuality::Sd720p,
                                    RecordingQuality::Hd1080p,
                                    RecordingQuality::Qhd1440p,
                                    RecordingQuality::Uhd4K,
                                ] {
                                    if ui.selectable_value(&mut self.settings.quality, q, q.label()).changed() {
                                        changed = true;
                                    }
                                }
                            });
                        ui.label(
                            RichText::new("SD records up to 720p, HD up to 1080p, and 2K/4K up to source display resolution.")
                                .size(11.5)
                                .color(TEXT_SECONDARY),
                        );

                        ui.add_space(12.0);
                        ui.label(RichText::new("Frame Rate (FPS)").strong());
                        ui.horizontal(|ui| {
                            for fps in [25u32, 30, 60] {
                                let sel = self.settings.fps == fps;
                                let btn = egui::Button::new(
                                    RichText::new(format!("{fps} FPS"))
                                        .color(if sel { Color32::WHITE } else { TEXT_PRIMARY }),
                                )
                                .fill(if sel { PURPLE_ACTION } else { Color32::WHITE });
                                if ui.add_sized(Vec2::new(76.0, 30.0), btn).clicked() {
                                    self.settings.fps = fps;
                                    changed = true;
                                }
                            }
                        });
                        if self.settings.fps == 60 {
                            ui.label(
                                RichText::new("60 FPS creates much larger files; use it when extra-smooth motion is needed.")
                                    .size(11.5)
                                    .color(PAUSE_ORANGE),
                            );
                        } else if self.settings.fps == 30 {
                            ui.label(
                                RichText::new("30 FPS and 60 FPS benefit from hardware acceleration; 25 FPS is the default fallback.")
                                    .size(11.5)
                                    .color(TEXT_SECONDARY),
                            );
                        }

                        ui.add_space(12.0);
                        ui.label(RichText::new("Capture Method").strong());
                        egui::ComboBox::from_id_salt("settings_cap_method")
                            .width(260.0)
                            .selected_text(self.settings.capture_method.label())
                            .show_ui(ui, |ui| {
                                for m in [CaptureMethod::DesktopDuplication, CaptureMethod::GdiCompatible] {
                                    if ui.selectable_value(&mut self.settings.capture_method, m, m.label()).changed() {
                                        changed = true;
                                    }
                                }
                            });

                        ui.add_space(12.0);
                        ui.label(RichText::new("Video Encoder").strong());
                        egui::ComboBox::from_id_salt("settings_encoder")
                            .width(320.0)
                            .selected_text(self.settings.encoder.label())
                            .show_ui(ui, |ui| {
                                for e in [EncoderKind::NativeMp4, EncoderKind::LegacyAvi] {
                                    if ui.selectable_value(&mut self.settings.encoder, e, e.label()).changed() {
                                        changed = true;
                                    }
                                }
                            });
                        ui.label(
                            RichText::new(
                                "Native MP4 records straight to H.264+AAC using Windows Media Foundation — \
                                 small files and no FFmpeg needed. AVI is the compatibility fallback.",
                            )
                            .size(11.5)
                            .color(TEXT_SECONDARY),
                        );
                        if ui
                            .checkbox(&mut self.settings.prefer_mp4, "Prefer MP4 output (kept for older settings)")
                            .changed()
                        {
                            changed = true;
                        }
                        if ui
                            .checkbox(
                                &mut self.settings.compact_video,
                                "Compact Video Mode (Lower disk space per video shot)",
                            )
                            .changed()
                        {
                            changed = true;
                        }
                        if ui
                            .checkbox(&mut self.settings.countdown_enabled, "Show 3-second countdown before recording")
                            .changed()
                        {
                            changed = true;
                        }
                        if ui
                            .checkbox(&mut self.settings.mouse_highlight, "Highlight mouse cursor during recording")
                            .changed()
                        {
                            changed = true;
                        }
                        if self.settings.mouse_highlight {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("Cursor Style").strong());
                                egui::ComboBox::from_id_salt("settings_cursor_style")
                                    .width(170.0)
                                    .selected_text(self.settings.cursor_style.label())
                                    .show_ui(ui, |ui| {
                                        for c in [
                                            CursorStyle::Halo,
                                            CursorStyle::Spotlight,
                                            CursorStyle::ClickRipple,
                                            CursorStyle::ColoredArrow,
                                        ] {
                                            if ui
                                                .selectable_value(&mut self.settings.cursor_style, c, c.label())
                                                .changed()
                                            {
                                                changed = true;
                                            }
                                        }
                                    });
                            });
                        }
                        if ui
                            .checkbox(&mut self.settings.auto_open_editor, "Open Annotation Editor after taking a screenshot")
                            .changed()
                        {
                            changed = true;
                        }

                        ui.add_space(12.0);
                        ui.label(RichText::new("Floating Dock Visibility").strong());
                        egui::ComboBox::from_id_salt("settings_floating_ui")
                            .width(260.0)
                            .selected_text(self.settings.floating_ui_mode.label())
                            .show_ui(ui, |ui| {
                                for mode in [
                                    FloatingUiMode::FullMenuAndControls,
                                    FloatingUiMode::MinimalMenuAndControls,
                                    FloatingUiMode::MinimalMenuOnly,
                                    FloatingUiMode::Hidden,
                                ] {
                                    if ui
                                        .selectable_value(&mut self.settings.floating_ui_mode, mode, mode.label())
                                        .changed()
                                    {
                                        changed = true;
                                    }
                                }
                            });
                    });

                // Column 2: Audio, Webcam, Hotkeys & Storage
                egui::Frame::new()
                    .fill(CARD_BG)
                    .stroke(Stroke::new(1.0, BORDER_LIGHT))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(egui::Margin::same(16))
                    .show(&mut cols[1], |ui| {
                        ui.label(
                            RichText::new("Audio & Webcam")
                                .size(15.5)
                                .strong()
                                .color(PURPLE_PRIMARY),
                        );
                        ui.add_space(10.0);

                        if ui.checkbox(&mut self.settings.record_mic, "Record Microphone").changed() {
                            changed = true;
                        }
                        if self.settings.record_mic {
                            if let Some(ref device) = self.mic_device {
                                ui.label(
                                    RichText::new(format!("Input device: {device}"))
                                        .size(11.0)
                                        .color(TEXT_SECONDARY),
                                );
                            } else {
                                ui.label(
                                    RichText::new("Input device: not detected")
                                        .size(11.0)
                                        .color(Color32::from_rgb(255, 100, 100)),
                                );
                            }
                            ui.horizontal(|ui| {
                                ui.label("Mic Gain:");
                                if ui.add(egui::Slider::new(&mut self.settings.mic_gain, 0.0..=1.5)).changed() {
                                    changed = true;
                                }
                            });
                            if ui.checkbox(&mut self.settings.ai_mic_enhance, "AI Mic Noise Reduction").changed() {
                                changed = true;
                            }
                        }
                        if ui.checkbox(&mut self.settings.record_system_audio, "Record System Audio").changed() {
                            changed = true;
                        }

                        ui.add_space(10.0);
                        if ui.checkbox(&mut self.settings.enable_webcam, "Show Webcam Bubble Overlay").changed() {
                            changed = true;
                        }
                        if self.settings.enable_webcam {
                            ui.horizontal(|ui| {
                                ui.label("Background:");
                                for mode in [WebcamBgMode::Normal, WebcamBgMode::Blur, WebcamBgMode::Transparent] {
                                    if ui.selectable_value(&mut self.settings.webcam_bg, mode, mode.label()).changed() {
                                        changed = true;
                                    }
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label("Size:");
                                for sz in [WebcamSizePreset::Small, WebcamSizePreset::Large, WebcamSizePreset::Full] {
                                    if ui.selectable_value(&mut self.settings.webcam_size, sz, sz.label()).changed() {
                                        changed = true;
                                    }
                                }
                            });
                        }

                        ui.add_space(14.0);
                        ui.separator();
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new("Hotkeys & Local Storage")
                                .size(15.5)
                                .strong()
                                .color(PURPLE_PRIMARY),
                        );
                        ui.add_space(8.0);

                        egui::Grid::new("hotkeys_grid").num_columns(2).spacing(Vec2::new(14.0, 8.0)).show(ui, |ui| {
                            ui.label("Capture / Stop Recording:");
                            if ui.add(egui::TextEdit::singleline(&mut self.settings.capture_hotkey).desired_width(110.0)).changed() {
                                changed = true;
                            }
                            ui.end_row();

                            ui.label("Pause / Resume Recording:");
                            if ui.add(egui::TextEdit::singleline(&mut self.settings.pause_hotkey).desired_width(110.0)).changed() {
                                changed = true;
                            }
                            ui.end_row();

                            ui.label("Voice Indicator:");
                            if ui.add(egui::TextEdit::singleline(&mut self.settings.dictation_hotkey).desired_width(110.0)).changed() {
                                changed = true;
                            }
                            ui.end_row();
                        });

                        ui.add_space(8.0);
                        if ui.button("Reset Shortcuts to Default").clicked() {
                            self.settings.capture_hotkey = "Alt+S".to_string();
                            self.settings.pause_hotkey = "Alt+Q".to_string();
                            self.settings.dictation_hotkey = "Alt+D".to_string();
                            changed = true;
                        }

                        ui.add_space(12.0);
                        ui.label(RichText::new("Local Recordings Folder").strong());
                        ui.label(
                            RichText::new(self.storage.records_dir.to_string_lossy().to_string())
                                .size(11.5)
                                .color(TEXT_SECONDARY),
                        );
                        ui.add_space(4.0);
                        if ui.button("Open Recordings Folder").clicked() {
                            StorageManager::open_in_explorer(&self.storage.records_dir);
                        }

                        ui.add_space(14.0);
                        ui.separator();
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new("Elevated / Admin Windows")
                                .size(15.5)
                                .strong()
                                .color(PURPLE_PRIMARY),
                        );
                        ui.add_space(6.0);
                        if ui
                            .checkbox(
                                &mut self.settings.admin_capture_enabled,
                                "Enable Admin capture (restart ClipRec elevated)",
                            )
                            .changed()
                        {
                            changed = true;
                        }
                        ui.label(
                            RichText::new(
                                "Restarting as Administrator lets ClipRec record windows that run elevated. \
                                 Note: DRM content (Netflix app) and the UAC secure desktop still cannot be \
                                 captured by any tool.",
                            )
                            .size(11.0)
                            .color(TEXT_SECONDARY),
                        );
                        ui.add_space(4.0);
                        if self.is_elevated() {
                            ui.label(
                                RichText::new("Status: running as Administrator")
                                    .size(12.0)
                                    .color(LEVEL_GREEN),
                            );
                        } else {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("Status: normal user")
                                        .size(12.0)
                                        .color(TEXT_SECONDARY),
                                );
                                if ui
                                    .button("Restart as Administrator now (UAC)")
                                    .on_hover_text("A UAC consent dialog will appear once")
                                    .clicked()
                                {
                                    self.storage.save_settings(&self.settings);
                                    self.relaunch_as_admin();
                                    self.show_toast(
                                        "Restarting as Administrator — confirm the UAC prompt…",
                                        false,
                                    );
                                }
                            });
                        }
                    });
            });
        });

        if changed {
            self.storage.save_settings(&self.settings);
        }
    }

    fn render_toast(&mut self, ctx: &egui::Context) {
        let expired = self
            .toast
            .as_ref()
            .map_or(false, |t| Instant::now() >= t.expires);
        if expired {
            self.toast = None;
        }
        if let Some(ref toast) = self.toast {
            egui::Area::new(egui::Id::new("bottom_toast_area"))
                .anchor(egui::Align2::RIGHT_BOTTOM, Vec2::new(-94.0, -18.0))
                .show(ctx, |ui| {
                    egui::Frame::new()
                        .fill(if toast.is_error { DANGER_RED } else { PURPLE_PRIMARY })
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(egui::Margin::symmetric(16, 10))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(&toast.text)
                                    .size(13.0)
                                    .strong()
                                    .color(Color32::WHITE),
                            );
                        });
                });
        }
    }
}

impl eframe::App for ClipRecApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_shortcuts(ctx);
        self.tick_recording(ctx);
        if self.view != ActiveView::Player {
            if let Some(ref mut pb) = self.playback {
                if pb.playing {
                    pb.playing = false;
                    if let Some(ref ap) = pb.audio_player {
                        ap.set_playing(false);
                    }
                }
            }
        }

        self.render_top_bar(ctx);
        self.render_floating_dock(ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(SURFACE_BG).inner_margin(egui::Margin::same(14)))
            .show(ctx, |ui| match self.view {
                ActiveView::CaptureStudio => self.render_capture_studio(ctx, ui),
                ActiveView::Editor => self.render_editor(ctx, ui),
            ActiveView::VideoEditor => self.render_video_editor(ctx, ui),
                ActiveView::Gallery => self.render_gallery(ctx, ui),
                ActiveView::Player => self.render_player(ctx, ui),
                ActiveView::Settings => self.render_settings(ui),
            });

        self.render_toast(ctx);
        self.render_recording_sidebar(ctx);
        self.handle_screenshot_mode(ctx);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SidebarAction {
    None,
    TogglePause,
    Stop,
    ToggleWebcam,
    ToggleMic,
    Screenshot,
    ToggleMainWindow,
    ToggleCollapse,
    FlipSide,
}

impl ClipRecApp {
    /// Small always-on-top sticky vertical control bar shown while recording.
    fn render_recording_sidebar(&mut self, ctx: &egui::Context) {
        let active = matches!(
            self.rec_state,
            RecordingState::Recording | RecordingState::Paused | RecordingState::Countdown { .. }
        ) && self.settings.floating_ui_mode != FloatingUiMode::Hidden
            && self.screenshot_target.is_none();

        if !active {
            self.sidebar_capture_excluded = false;
            return;
        }

        let root_minimized = ctx.input(|i| i.viewport().minimized).unwrap_or(false);
        let ppp = ctx.pixels_per_point().max(0.5);
        let (sw, sh) = screen_dimensions();
        let screen_w = sw as f32 / ppp;
        let screen_h = sh as f32 / ppp;

        let collapsed = self.sidebar_collapsed;
        let on_right = self.sidebar_on_right;
        let size = if collapsed {
            Vec2::new(26.0, 98.0)
        } else {
            Vec2::new(58.0, 366.0)
        };
        let x = if on_right { (screen_w - size.x).max(0.0) } else { 0.0 };
        let max_y = (screen_h - size.y - 8.0).max(8.0);
        let y = ((screen_h - size.y) * self.sidebar_y_norm).clamp(8.0, max_y);
        let pos = Pos2::new(x, y);

        let elapsed = self.current_rec_elapsed();
        let state = self.rec_state;
        let webcam_on = self.settings.enable_webcam;
        let mic_on = self.settings.record_mic;
        let mic_level = self.sim_mic_level;
        let reposition = self.sidebar_reposition;

        let (action, cur_outer) = ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("rec_sidebar"),
            egui::ViewportBuilder::default()
                .with_title("ClipRec Controls")
                .with_icon(std::sync::Arc::new(crate::icon::app_icon_data()))
                .with_inner_size(size)
                .with_position(pos)
                .with_decorations(false)
                .with_resizable(false)
                .with_always_on_top()
                .with_taskbar(false),
            |vctx, _class| {
                let mut act = SidebarAction::None;
                let cur_outer = vctx.input(|i| i.viewport().outer_rect);

                if reposition {
                    vctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
                    vctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
                }
                if vctx.input(|i| i.viewport().close_requested()) {
                    vctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    act = SidebarAction::ToggleCollapse;
                }

                let margin = if collapsed {
                    egui::Margin::symmetric(3, 8)
                } else {
                    egui::Margin::symmetric(6, 8)
                };
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::new()
                            .fill(DOCK_BG)
                            .stroke(Stroke::new(1.0_f32, Color32::from_rgb(0x48, 0x35, 0x6E)))
                            .corner_radius(CornerRadius::ZERO)
                            .inner_margin(margin),
                    )
                    .show(vctx, |ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(4.0, 5.0);

                        let dot_col = match state {
                            RecordingState::Recording => REC_RED,
                            RecordingState::Paused => PAUSE_ORANGE,
                            _ => PURPLE_HOVER,
                        };
                        let blink = state != RecordingState::Recording
                            || ((vctx.input(|i| i.time) * 2.0) as i64) % 2 == 0;

                        if collapsed {
                            ui.vertical_centered(|ui| {
                                let (d, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                                if blink {
                                    ui.painter().circle_filled(d.center(), 4.5, dot_col);
                                }
                                let (exp_rect, exp_resp) =
                                    ui.allocate_exact_size(Vec2::new(18.0, 38.0), Sense::click());
                                if exp_resp.hovered() {
                                    ui.painter().rect_filled(exp_rect, CornerRadius::same(4), DOCK_HOVER);
                                }
                                let c = exp_rect.center();
                                let dir = if on_right { -1.0_f32 } else { 1.0_f32 };
                                let ch = Stroke::new(1.8_f32, Color32::WHITE);
                                ui.painter().line_segment(
                                    [c + Vec2::new(-dir * 3.0, -5.0), c + Vec2::new(dir * 3.0, 0.0)],
                                    ch,
                                );
                                ui.painter().line_segment(
                                    [c + Vec2::new(dir * 3.0, 0.0), c + Vec2::new(-dir * 3.0, 5.0)],
                                    ch,
                                );
                                if exp_resp.on_hover_text("Expand recording bar").clicked() {
                                    act = SidebarAction::ToggleCollapse;
                                }

                                let (stop_rect, stop_resp) =
                                    ui.allocate_exact_size(Vec2::new(18.0, 20.0), Sense::click());
                                let sfill = if stop_resp.hovered() { REC_RED } else { DANGER_RED };
                                ui.painter().rect_filled(stop_rect, CornerRadius::same(4), sfill);
                                ui.painter().rect_filled(
                                    Rect::from_center_size(stop_rect.center(), Vec2::splat(7.0)),
                                    CornerRadius::same(1),
                                    Color32::WHITE,
                                );
                                if stop_resp.on_hover_text("Stop & Save (Alt+S)").clicked() {
                                    act = SidebarAction::Stop;
                                }
                            });
                        } else {
                            ui.vertical_centered(|ui| {
                                // Drag grip handle
                                let (h, h_resp) =
                                    ui.allocate_exact_size(Vec2::new(44.0, 12.0), Sense::click_and_drag());
                                for i in 0..3 {
                                    let ly = h.top() + 2.0 + i as f32 * 3.5;
                                    ui.painter().line_segment(
                                        [Pos2::new(h.center().x - 10.0, ly), Pos2::new(h.center().x + 10.0, ly)],
                                        Stroke::new(1.4_f32, Color32::from_gray(145)),
                                    );
                                }
                                if h_resp.drag_started() {
                                    vctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                                }
                                h_resp.on_hover_text("Drag to move vertically");

                                // Edge utility row: Flip side + Collapse
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing = Vec2::new(4.0, 0.0);
                                    let mini_btn = |ui: &mut egui::Ui, txt: &str, tip: &str| -> bool {
                                        let (r, resp) = ui.allocate_exact_size(Vec2::new(20.0, 18.0), Sense::click());
                                        let bg = if resp.hovered() {
                                            DOCK_HOVER
                                        } else {
                                            Color32::from_rgb(0x28, 0x28, 0x30)
                                        };
                                        ui.painter().rect_filled(r, CornerRadius::same(4), bg);
                                        ui.painter().text(
                                            r.center(),
                                            egui::Align2::CENTER_CENTER,
                                            txt,
                                            FontId::proportional(10.0),
                                            Color32::from_gray(205),
                                        );
                                        resp.on_hover_text(tip).clicked()
                                    };
                                    if mini_btn(
                                        ui,
                                        if on_right { "<" } else { ">" },
                                        "Stick to opposite screen edge",
                                    ) {
                                        act = SidebarAction::FlipSide;
                                    }
                                    if mini_btn(
                                        ui,
                                        if on_right { ">>" } else { "<<" },
                                        "Collapse to edge tab",
                                    ) {
                                        act = SidebarAction::ToggleCollapse;
                                    }
                                });

                                // Status dot + label + live timer
                                let (d, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                                if blink {
                                    ui.painter().circle_filled(d.center(), 4.5, dot_col);
                                }
                                let status_lbl = match state {
                                    RecordingState::Recording => "REC".to_string(),
                                    RecordingState::Paused => "PAUSED".to_string(),
                                    RecordingState::Countdown { started } => {
                                        let rem = 3u64.saturating_sub(started.elapsed().as_secs()).max(1);
                                        format!("{rem}s")
                                    }
                                    RecordingState::Idle => String::new(),
                                };
                                ui.label(RichText::new(status_lbl).size(9.5).strong().color(dot_col));
                                ui.label(
                                    RichText::new(format_duration(elapsed))
                                        .size(10.5)
                                        .monospace()
                                        .color(Color32::WHITE),
                                );

                                let (sep, _) = ui.allocate_exact_size(Vec2::new(36.0, 2.0), Sense::hover());
                                ui.painter().line_segment(
                                    [sep.left_center(), sep.right_center()],
                                    Stroke::new(1.0_f32, Color32::from_gray(65)),
                                );

                                let icon_btn = |ui: &mut egui::Ui, icon: DockIcon, fill: Color32, tip: &str| -> bool {
                                    let (rect, resp) =
                                        ui.allocate_exact_size(Vec2::new(42.0, 36.0), Sense::click());
                                    let bg = if resp.hovered() {
                                        Color32::from_rgb(
                                            fill.r().saturating_add(18),
                                            fill.g().saturating_add(18),
                                            fill.b().saturating_add(18),
                                        )
                                    } else {
                                        fill
                                    };
                                    ui.painter().rect_filled(rect, CornerRadius::same(9), bg);
                                    draw_icon(ui.painter(), rect.shrink(6.0), icon, Color32::WHITE);
                                    resp.on_hover_text(tip).clicked()
                                };

                                // 1. Pause / Resume (or Countdown badge)
                                if let RecordingState::Countdown { started } = state {
                                    let rem = 3u64.saturating_sub(started.elapsed().as_secs()).max(1);
                                    let (r, _) = ui.allocate_exact_size(Vec2::new(42.0, 36.0), Sense::hover());
                                    ui.painter().rect_filled(r, CornerRadius::same(9), PURPLE_PRIMARY);
                                    ui.painter().text(
                                        r.center(),
                                        egui::Align2::CENTER_CENTER,
                                        format!("{rem}"),
                                        FontId::proportional(16.0),
                                        Color32::WHITE,
                                    );
                                } else {
                                    let paused = state == RecordingState::Paused;
                                    if icon_btn(
                                        ui,
                                        if paused { DockIcon::Play } else { DockIcon::Pause },
                                        if paused { PAUSE_ORANGE } else { DOCK_HOVER },
                                        if paused { "Resume (Alt+Q)" } else { "Pause (Alt+Q)" },
                                    ) {
                                        act = SidebarAction::TogglePause;
                                    }
                                }

                                // 2. Stop & Save (vivid red button with white square)
                                {
                                    let (rect, resp) =
                                        ui.allocate_exact_size(Vec2::new(42.0, 36.0), Sense::click());
                                    let bg = if resp.hovered() { REC_RED } else { DANGER_RED };
                                    ui.painter().rect_filled(rect, CornerRadius::same(9), bg);
                                    ui.painter().rect_filled(
                                        Rect::from_center_size(rect.center(), Vec2::splat(13.0)),
                                        CornerRadius::same(2),
                                        Color32::WHITE,
                                    );
                                    let tip = if matches!(state, RecordingState::Countdown { .. }) {
                                        "Cancel countdown"
                                    } else {
                                        "Stop & save (Alt+S)"
                                    };
                                    if resp.on_hover_text(tip).clicked() {
                                        act = SidebarAction::Stop;
                                    }
                                }

                                // 3. Webcam toggle
                                if icon_btn(
                                    ui,
                                    DockIcon::Webcam { active: webcam_on },
                                    if webcam_on { PURPLE_PRIMARY } else { DOCK_HOVER },
                                    if webcam_on { "Hide webcam" } else { "Show webcam" },
                                ) {
                                    act = SidebarAction::ToggleWebcam;
                                }

                                // 4. Mic toggle
                                if icon_btn(
                                    ui,
                                    DockIcon::Mic {
                                        muted: !mic_on,
                                        level: mic_level,
                                    },
                                    if mic_on { Color32::from_rgb(0x2E, 0x2B, 0x3D) } else { DOCK_HOVER },
                                    if mic_on { "Mute mic" } else { "Unmute mic" },
                                ) {
                                    act = SidebarAction::ToggleMic;
                                }

                                // 5. Screenshot
                                if icon_btn(ui, DockIcon::Camera, DOCK_HOVER, "Take screenshot") {
                                    act = SidebarAction::Screenshot;
                                }

                                // 6. Show / Minimize main ClipRec window
                                if icon_btn(
                                    ui,
                                    DockIcon::AppWindow,
                                    if root_minimized {
                                        DOCK_HOVER
                                    } else {
                                        Color32::from_rgb(0x32, 0x28, 0x4A)
                                    },
                                    if root_minimized {
                                        "Show ClipRec window"
                                    } else {
                                        "Minimize ClipRec window"
                                    },
                                ) {
                                    act = SidebarAction::ToggleMainWindow;
                                }
                            });
                        }
                    });

                if matches!(state, RecordingState::Recording | RecordingState::Countdown { .. }) {
                    vctx.request_repaint_after(Duration::from_millis(200));
                }
                (act, cur_outer)
            },
        );

        self.sidebar_reposition = false;
        if !self.sidebar_capture_excluded && exclude_window_from_capture("ClipRec Controls") {
            self.sidebar_capture_excluded = true;
        }

        match action {
            SidebarAction::None => {}
            SidebarAction::TogglePause => self.toggle_pause_recording(),
            SidebarAction::Stop => {
                if matches!(self.rec_state, RecordingState::Countdown { .. }) {
                    self.rec_state = RecordingState::Idle;
                    self.sidebar_capture_excluded = false;
                    if self.auto_minimized_for_rec {
                        self.auto_minimized_for_rec = false;
                        ctx.send_viewport_cmd_to(
                            egui::ViewportId::ROOT,
                            egui::ViewportCommand::Minimized(false),
                        );
                        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
                    }
                    self.show_toast("Recording cancelled", false);
                } else {
                    self.stop_video_recording(ctx);
                }
            }
            SidebarAction::ToggleWebcam => {
                self.settings.enable_webcam = !self.settings.enable_webcam;
                self.storage.save_settings(&self.settings);
            }
            SidebarAction::ToggleMic => {
                self.settings.record_mic = !self.settings.record_mic;
                self.storage.save_settings(&self.settings);
            }
            SidebarAction::Screenshot => {
                let prev = self.view;
                self.take_screenshot(ctx, false);
                self.view = prev; // don't jump to the editor mid-recording
            }
            SidebarAction::ToggleMainWindow => {
                if root_minimized {
                    self.auto_minimized_for_rec = false;
                    ctx.send_viewport_cmd_to(
                        egui::ViewportId::ROOT,
                        egui::ViewportCommand::Minimized(false),
                    );
                    ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
                } else {
                    ctx.send_viewport_cmd_to(
                        egui::ViewportId::ROOT,
                        egui::ViewportCommand::Minimized(true),
                    );
                }
            }
            SidebarAction::ToggleCollapse => {
                if let Some(r) = cur_outer {
                    self.sidebar_y_norm = (r.min.y / (screen_h - size.y).max(1.0)).clamp(0.0, 1.0);
                }
                self.sidebar_collapsed = !self.sidebar_collapsed;
                self.sidebar_reposition = true;
            }
            SidebarAction::FlipSide => {
                if let Some(r) = cur_outer {
                    self.sidebar_y_norm = (r.min.y / (screen_h - size.y).max(1.0)).clamp(0.0, 1.0);
                }
                self.sidebar_on_right = !self.sidebar_on_right;
                self.sidebar_reposition = true;
            }
        }
    }
}

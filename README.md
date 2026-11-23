# ClipRec — Screen Capture, Recording & Annotation Studio

A fast, lightweight, native Windows screen recorder, screenshot annotator, and video player built in Rust with `egui` / `eframe`, `cpal`, `nokhwa`, and FFmpeg.

## Features

- **Screen & Region Recording**: Capture full screen or any custom drag-selected region at 25 / 30 / 60 FPS with hardware audio-clock synchronization (zero A/V drift).
- **Compact MP4 Export (H.264 + AAC)**: Automatically saves recordings as compact, fast-start `.mp4` files via FFmpeg, plus one-click **Export MP4** and batch **Convert AVI -> MP4** in the Gallery.
- **Studio Voice Capture & Enhancement**: Clean `cpal` microphone capture with optional FFmpeg voice enhancement (`highpass`, `lowpass`, `afftdn` noise reduction, and dynamic compression).
- **Synchronized In-App Video Player**: Built-in player locked to the hardware audio clock with scrubbing, `-10s`/`+10s` seek, speed control (`0.5x`–`2.0x`), and mute toggle.
- **Live Webcam PiP Bubble**: Circular webcam overlay (Small / Large / Full Frame) with live preview.
- **Screenshot & Annotation Editor**: Capture screenshots (`Alt+S`), annotate with Arrows, Frames, Callouts, and Text using an 80-color palette, and copy to clipboard or save as PNG.
- **Sticky Recording Sidebar**: Always-on-top, capture-excluded (`WDA_EXCLUDEFROMCAPTURE`) floating controls while recording.

## Requirements

- Windows 10 / 11
- [Rust](https://rustup.rs/) (for building from source)
- [FFmpeg](https://ffmpeg.org/) on `PATH` (or installed via `winget install Gyan.FFmpeg`) for MP4 recording and export

## Build & Run

```powershell
cargo build --release
.\target\release\cliprec.exe
```

## Shortcuts

- `Alt+S` — Take screenshot (idle) / Stop recording (active)
- `Alt+Q` — Pause / Resume recording
- `Alt+D` — Toggle voice indicator

<div align="center">

<img src="assets/banner.svg" alt="ClipRec — Native Rust Screen Recorder & Studio" width="100%"/>

<br/><br/>

[![Latest Release](https://img.shields.io/github/v/release/mmdk11278-hub/cliprec?style=for-the-badge&color=683DE3&label=Release)](https://github.com/mmdk11278-hub/cliprec/releases/latest)
[![Platform](https://img.shields.io/badge/Platform-Windows%2010%20%2F%2011-0078D6?style=for-the-badge&logo=windows)](https://github.com/mmdk11278-hub/cliprec/releases/latest/download/ClipRec-Setup.exe)
[![Built With Rust](https://img.shields.io/badge/Built%20With-Rust-orange?style=for-the-badge&logo=rust)](https://www.rust-lang.org/)
[![H.264 MP4](https://img.shields.io/badge/Export-MP4%20(H.264%20%2B%20AAC)-38C172?style=for-the-badge)](https://ffmpeg.org/)

<br/>

### **[Download ClipRec-Setup.exe (Windows Installer v0.3.0)](https://github.com/mmdk11278-hub/cliprec/releases/latest/download/ClipRec-Setup.exe)**
*One-click Windows setup wizard with Desktop & Start Menu shortcuts and custom brand icon.*

</div>

---

## Preview

<p align="center">
  <img src="assets/screenshot.png" alt="ClipRec Capture Studio" width="92%"/>
</p>

---

## Key Features

| Feature | Description |
|---|---|
| **Screen & Region Capture** | Capture full screen or any drag-selected region at **25 / 30 / 60 FPS** with GDI capture and background JPEG encoding. |
| **Compact MP4 (H.264 + AAC)** | Automatically saves recordings as fast-start `.mp4` files, plus one-click **Export MP4** and batch **Convert AVI -> MP4** in the Gallery. |
| **Audio-Clock Locked A/V Sync** | Frame pacing is locked to the hardware audio clock so video never drifts behind or ahead of voiceover. |
| **Studio Voice Enhancement** | Clean `cpal` microphone capture with optional FFmpeg voice chain (`highpass`, `lowpass`, `afftdn` noise reduction, and dynamic compression). |
| **Synchronized Video Player** | Built-in player driven by the master audio clock with scrubbing, `-10s`/`+10s` seek, `0.5x`–`2.0x` speed, and mute toggle. |
| **Live Webcam PiP Bubble** | Circular webcam overlay (Small / Large / Full Frame) composited live onto recordings. |
| **80-Color Annotation Editor** | Annotate screenshots with Arrows, Frames, Callouts, and Text, then save as PNG or copy directly to clipboard. |
| **Sticky Recording Sidebar** | Always-on-top, capture-excluded (`WDA_EXCLUDEFROMCAPTURE`) floating controls while recording. |

---

## Keyboard Shortcuts

| Shortcut | Action |
|---|---|
| `Alt + S` | Take screenshot (when idle) / Stop & save recording (when recording) |
| `Alt + Q` | Pause / Resume active recording |
| `Alt + D` | Toggle voice indicator |
| `Ctrl + Z` / `Ctrl + Y` | Undo / Redo in Annotation Editor |
| `Space` / `Left` / `Right` | Play/Pause & `-10s`/`+10s` seek in Video Player |

---

## Build From Source

### Requirements
- Windows 10 / 11
- [Rust](https://rustup.rs/) (`stable-x86_64-pc-windows-gnu`)
- [FFmpeg](https://ffmpeg.org/) on `PATH` (or `winget install Gyan.FFmpeg`) for MP4 recording & export

### Commands

```powershell
# 1. Run tests
cargo test --bin cliprec

# 2. Build the main recorder
cargo build --release --bin cliprec

# 3. Build the self-contained Windows Setup Installer (embeds cliprec.exe + icon)
cargo build --release --bin ClipRec-Setup
```

The compiled binaries will be at:
- `target\release\cliprec.exe`
- `target\release\ClipRec-Setup.exe`

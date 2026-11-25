#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[path = "../icon.rs"]
mod icon;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{Color32, CornerRadius, RichText, Stroke, Vec2};

const PAYLOAD_EXE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/cliprec_payload.bin"));

const PURPLE_PRIMARY: Color32 = Color32::from_rgb(0x51, 0x00, 0xA4);
const PURPLE_ACTION: Color32 = Color32::from_rgb(0x68, 0x3D, 0xE3);
const SURFACE_BG: Color32 = Color32::from_rgb(0xF4, 0xF6, 0xF8);
const TEXT_PRIMARY: Color32 = Color32::from_rgb(0x1C, 0x1C, 0x1C);
const TEXT_SECONDARY: Color32 = Color32::from_rgb(0x5F, 0x60, 0x60);

fn default_install_dir() -> String {
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        PathBuf::from(local)
            .join("Programs")
            .join("ClipRec")
            .to_string_lossy()
            .to_string()
    } else {
        r"C:\Program Files\ClipRec".to_string()
    }
}

fn perform_install(
    install_dir: &Path,
    create_desktop: bool,
    create_start_menu: bool,
    update_existing_shortcut: bool,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(install_dir)
        .map_err(|e| format!("Could not create install folder: {e}"))?;

    let exe_dst = install_dir.join("cliprec.exe");
    let ico_dst = install_dir.join("cliprec.ico");
    let uninst_dst = install_dir.join("uninstall.cmd");

    if !PAYLOAD_EXE.is_empty() {
        std::fs::write(&exe_dst, PAYLOAD_EXE)
            .map_err(|e| format!("Failed to write cliprec.exe: {e}"))?;
    } else if let Ok(cur) = std::env::current_exe() {
        let adj = cur.with_file_name("cliprec.exe");
        if adj.is_file() {
            std::fs::copy(&adj, &exe_dst)
                .map_err(|e| format!("Failed to copy cliprec.exe: {e}"))?;
        } else {
            return Err("Embedded cliprec.exe payload is missing".to_string());
        }
    }

    let ico_bytes = icon::generate_ico_bytes();
    std::fs::write(&ico_dst, &ico_bytes)
        .map_err(|e| format!("Failed to write cliprec.ico: {e}"))?;

    let uninstall_script = format!(
        "@echo off\r\n\
         taskkill /IM cliprec.exe /F >nul 2>&1\r\n\
         del /Q \"%USERPROFILE%\\Desktop\\ClipRec.lnk\" >nul 2>&1\r\n\
         del /Q \"%APPDATA%\\Microsoft\\Windows\\Start Menu\\Programs\\ClipRec.lnk\" >nul 2>&1\r\n\
         reg delete \"HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\ClipRec\" /f >nul 2>&1\r\n\
         rmdir /S /Q \"{}\"\r\n",
        install_dir.display()
    );
    let _ = std::fs::write(&uninst_dst, uninstall_script);

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let exe_s = exe_dst.to_string_lossy().replace('\'', "''");
        let ico_s = ico_dst.to_string_lossy().replace('\'', "''");
        let dir_s = install_dir.to_string_lossy().replace('\'', "''");
        let uni_s = uninst_dst.to_string_lossy().replace('\'', "''");

        let mut ps = String::new();
        ps.push_str("$sh = New-Object -ComObject WScript.Shell; ");
        if create_desktop {
            ps.push_str(&format!(
                "$d = [Environment]::GetFolderPath('Desktop'); \
                 $s = $sh.CreateShortcut((Join-Path $d 'ClipRec.lnk')); \
                 $s.TargetPath = '{exe_s}'; \
                 $s.WorkingDirectory = '{dir_s}'; \
                 $s.IconLocation = '{ico_s},0'; \
                 $s.Description = 'ClipRec — Screen Capture & Recording Studio'; \
                 $s.Save(); "
            ));
        }
        if update_existing_shortcut {
            ps.push_str(&format!(
                "$d = [Environment]::GetFolderPath('Desktop'); \
                 $old = Join-Path $d 'cliprec - Shortcut.lnk'; \
                 if (Test-Path $old) {{ \
                     $s2 = $sh.CreateShortcut($old); \
                     $s2.TargetPath = '{exe_s}'; \
                     $s2.WorkingDirectory = '{dir_s}'; \
                     $s2.IconLocation = '{ico_s},0'; \
                     $s2.Save(); \
                 }} "
            ));
        }
        if create_start_menu {
            ps.push_str(&format!(
                "$sm = Join-Path $env:APPDATA 'Microsoft\\Windows\\Start Menu\\Programs\\ClipRec.lnk'; \
                 $s3 = $sh.CreateShortcut($sm); \
                 $s3.TargetPath = '{exe_s}'; \
                 $s3.WorkingDirectory = '{dir_s}'; \
                 $s3.IconLocation = '{ico_s},0'; \
                 $s3.Description = 'ClipRec — Screen Capture & Recording Studio'; \
                 $s3.Save(); "
            ));
        }
        ps.push_str(&format!(
            "$reg = 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\ClipRec'; \
             New-Item -Path $reg -Force | Out-Null; \
             Set-ItemProperty -Path $reg -Name 'DisplayName' -Value 'ClipRec — Screen Capture & Recording Studio'; \
             Set-ItemProperty -Path $reg -Name 'DisplayVersion' -Value '0.4.1'; \
             Set-ItemProperty -Path $reg -Name 'Publisher' -Value 'ClipRec Studio'; \
             Set-ItemProperty -Path $reg -Name 'DisplayIcon' -Value '{ico_s}'; \
             Set-ItemProperty -Path $reg -Name 'InstallLocation' -Value '{dir_s}'; \
             Set-ItemProperty -Path $reg -Name 'UninstallString' -Value '{uni_s}'; \
             Set-ItemProperty -Path $reg -Name 'NoModify' -Type DWord -Value 1; \
             Set-ItemProperty -Path $reg -Name 'NoRepair' -Type DWord -Value 1;"
        ));

        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &ps])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| format!("Shortcut/registry setup failed: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!("Shortcut setup error: {}", err.trim()));
        }
    }

    Ok(exe_dst)
}

struct SetupApp {
    install_dir: String,
    create_desktop: bool,
    create_start_menu: bool,
    update_existing_shortcut: bool,
    installed_exe: Option<PathBuf>,
    error_msg: Option<String>,
    logo_tex: Option<egui::TextureHandle>,
}

impl SetupApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut visuals = egui::Visuals::light();
        visuals.panel_fill = SURFACE_BG;
        visuals.window_fill = Color32::WHITE;
        cc.egui_ctx.set_visuals(visuals);

        let rgba = icon::render_brand_icon_rgba(64);
        let ci = egui::ColorImage::from_rgba_unmultiplied([64, 64], &rgba);
        let logo_tex = Some(cc.egui_ctx.load_texture("setup_logo", ci, egui::TextureOptions::LINEAR));

        Self {
            install_dir: default_install_dir(),
            create_desktop: true,
            create_start_menu: true,
            update_existing_shortcut: true,
            installed_exe: None,
            error_msg: None,
            logo_tex,
        }
    }
}

impl eframe::App for SetupApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("setup_header")
            .exact_height(68.0)
            .frame(egui::Frame::new().fill(PURPLE_PRIMARY).inner_margin(egui::Margin::symmetric(18, 12)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if let Some(ref tex) = self.logo_tex {
                        ui.image((tex.id(), Vec2::splat(42.0)));
                    }
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("ClipRec Setup Wizard")
                                .size(20.0)
                                .strong()
                                .color(Color32::WHITE),
                        );
                        ui.label(
                            RichText::new("v0.4.1 • Screen Recorder, MP4 Studio & Annotator")
                                .size(12.0)
                                .color(Color32::from_rgb(0xE9, 0xD5, 0xFF)),
                        );
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(SURFACE_BG).inner_margin(egui::Margin::same(18)))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::WHITE)
                    .stroke(Stroke::new(1.0, Color32::from_rgb(0xE0, 0xE0, 0xE0)))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(egui::Margin::same(16))
                    .show(ui, |ui| {
                        if let Some(ref exe_path) = self.installed_exe {
                            ui.label(
                                RichText::new("Installation Complete!")
                                    .size(18.0)
                                    .strong()
                                    .color(PURPLE_PRIMARY),
                            );
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(format!("Installed to: {}", exe_path.display()))
                                    .color(TEXT_SECONDARY),
                            );
                            ui.add_space(6.0);
                            ui.label("Desktop and Start Menu shortcuts with the ClipRec icon are ready.");
                            ui.add_space(18.0);
                            ui.horizontal(|ui| {
                                let launch_btn = egui::Button::new(
                                    RichText::new("Launch ClipRec Now").strong().color(Color32::WHITE),
                                )
                                .fill(PURPLE_ACTION);
                                if ui.add_sized(Vec2::new(170.0, 36.0), launch_btn).clicked() {
                                    let _ = std::process::Command::new(exe_path).spawn();
                                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                                }
                                if ui.add_sized(Vec2::new(90.0, 36.0), egui::Button::new("Finish")).clicked() {
                                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                                }
                            });
                            return;
                        }

                        ui.label(RichText::new("Installation Folder").strong().color(TEXT_PRIMARY));
                        ui.add_space(4.0);
                        ui.add(
                            egui::TextEdit::singleline(&mut self.install_dir)
                                .desired_width(ui.available_width()),
                        );
                        ui.add_space(12.0);
                        ui.checkbox(&mut self.create_desktop, "Create Desktop shortcut (ClipRec)");
                        ui.checkbox(&mut self.create_start_menu, "Create Start Menu shortcut");
                        ui.checkbox(
                            &mut self.update_existing_shortcut,
                            "Update existing Desktop shortcut icon",
                        );

                        if let Some(ref err) = self.error_msg {
                            ui.add_space(8.0);
                            ui.label(RichText::new(err).color(Color32::RED));
                        }

                        ui.add_space(16.0);
                        ui.horizontal(|ui| {
                            let install_btn = egui::Button::new(
                                RichText::new("Install ClipRec").strong().color(Color32::WHITE),
                            )
                            .fill(PURPLE_PRIMARY);
                            if ui.add_sized(Vec2::new(150.0, 36.0), install_btn).clicked() {
                                match perform_install(
                                    Path::new(self.install_dir.trim()),
                                    self.create_desktop,
                                    self.create_start_menu,
                                    self.update_existing_shortcut,
                                ) {
                                    Ok(p) => {
                                        self.installed_exe = Some(p);
                                        self.error_msg = None;
                                    }
                                    Err(e) => self.error_msg = Some(e),
                                }
                            }
                            if ui.add_sized(Vec2::new(90.0, 36.0), egui::Button::new("Cancel")).clicked() {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        });
                    });
            });
    }
}

fn main() -> eframe::Result<()> {
    if std::env::args().any(|a| a == "--silent") {
        let dir = default_install_dir();
        if let Err(e) = perform_install(Path::new(&dir), true, true, true) {
            eprintln!("Silent install failed: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ClipRec Setup v0.4.1")
            .with_icon(Arc::new(icon::app_icon_data()))
            .with_inner_size([520.0, 370.0])
            .with_resizable(false),
        ..Default::default()
    };

    eframe::run_native(
        "ClipRec Setup",
        options,
        Box::new(|cc| Ok(Box::new(SetupApp::new(cc)))),
    )
}

use egui::{Color32, CornerRadius, Painter, Pos2, Rect, Stroke, Vec2};

// Brand & surface palette derived directly from ScreenRec QML resources
pub const PURPLE_PRIMARY: Color32 = Color32::from_rgb(0x51, 0x00, 0xA4);
pub const PURPLE_ACTION: Color32 = Color32::from_rgb(0x68, 0x3D, 0xE3);
pub const PURPLE_HOVER: Color32 = Color32::from_rgb(0x7F, 0x54, 0xF8);
pub const PURPLE_SOFT: Color32 = Color32::from_rgb(0xF3, 0xE8, 0xFF);
pub const PURPLE_TINT: Color32 = Color32::from_rgb(0xE8, 0xE0, 0xF0);
pub const PLAYER_BASE: Color32 = Color32::from_rgb(0x25, 0x08, 0x64);

pub const DOCK_BG: Color32 = Color32::from_rgb(0x1C, 0x1C, 0x1C);
pub const DOCK_HOVER: Color32 = Color32::from_rgb(0x3F, 0x3F, 0x3F);
pub const DOCK_ACTIVE: Color32 = Color32::from_rgb(0x51, 0x00, 0xA4);

pub const SURFACE_BG: Color32 = Color32::from_rgb(0xF4, 0xF6, 0xF8);
pub const CARD_BG: Color32 = Color32::WHITE;
pub const BORDER_LIGHT: Color32 = Color32::from_rgb(0xE0, 0xE0, 0xE0);
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(0x1C, 0x1C, 0x1C);
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(0x5F, 0x60, 0x60);

pub const REC_RED: Color32 = Color32::from_rgb(0xFF, 0x3B, 0x30);
pub const PAUSE_ORANGE: Color32 = Color32::from_rgb(0xFF, 0xA5, 0x00);
pub const LEVEL_GREEN: Color32 = Color32::from_rgb(0x38, 0xC1, 0x72);
pub const GALLERY_BLUE: Color32 = Color32::from_rgb(0x36, 0x91, 0xE1);
pub const DANGER_RED: Color32 = Color32::from_rgb(0xE5, 0x39, 0x35);

/// 80-color annotation palette (10 columns x 8 rows) from ColorPalette.qml
pub const ANNOTATION_PALETTE: &[Color32] = &[
    // Row 1: Grayscale
    Color32::from_rgb(0x00, 0x00, 0x00),
    Color32::from_rgb(0x43, 0x43, 0x43),
    Color32::from_rgb(0x66, 0x66, 0x66),
    Color32::from_rgb(0x99, 0x99, 0x99),
    Color32::from_rgb(0xB7, 0xB7, 0xB7),
    Color32::from_rgb(0xCC, 0xCC, 0xCC),
    Color32::from_rgb(0xD9, 0xD9, 0xD9),
    Color32::from_rgb(0xEF, 0xEF, 0xEF),
    Color32::from_rgb(0xF3, 0xF3, 0xF3),
    Color32::from_rgb(0xFF, 0xFF, 0xFF),
    // Row 2: Vivid
    Color32::from_rgb(0x98, 0x00, 0x00),
    Color32::from_rgb(0xFF, 0x00, 0x00),
    Color32::from_rgb(0xFF, 0x99, 0x00),
    Color32::from_rgb(0xFF, 0xFF, 0x00),
    Color32::from_rgb(0x00, 0xFF, 0x00),
    Color32::from_rgb(0x00, 0xFF, 0xFF),
    Color32::from_rgb(0x4A, 0x86, 0xE8),
    Color32::from_rgb(0x00, 0x00, 0xFF),
    Color32::from_rgb(0x99, 0x00, 0xFF),
    Color32::from_rgb(0xFF, 0x00, 0xFF),
    // Row 3: Pastels
    Color32::from_rgb(0xE6, 0xB8, 0xAF),
    Color32::from_rgb(0xF4, 0xCC, 0xCC),
    Color32::from_rgb(0xFC, 0xE5, 0xCD),
    Color32::from_rgb(0xFF, 0xF2, 0xCC),
    Color32::from_rgb(0xD9, 0xEA, 0xD3),
    Color32::from_rgb(0xD0, 0xE0, 0xE3),
    Color32::from_rgb(0xC9, 0xDA, 0xF8),
    Color32::from_rgb(0xCF, 0xE2, 0xF3),
    Color32::from_rgb(0xD9, 0xD2, 0xE9),
    Color32::from_rgb(0xEA, 0xD1, 0xDC),
    // Row 4: Soft
    Color32::from_rgb(0xDD, 0x7E, 0x6B),
    Color32::from_rgb(0xEA, 0x99, 0x99),
    Color32::from_rgb(0xF9, 0xCB, 0x9C),
    Color32::from_rgb(0xFF, 0xE5, 0x99),
    Color32::from_rgb(0xB6, 0xD7, 0xA8),
    Color32::from_rgb(0xA2, 0xC4, 0xC9),
    Color32::from_rgb(0xA4, 0xC2, 0xF4),
    Color32::from_rgb(0x9F, 0xC5, 0xE8),
    Color32::from_rgb(0xB4, 0xA7, 0xD6),
    Color32::from_rgb(0xD5, 0xA6, 0xBD),
    // Row 5: Medium
    Color32::from_rgb(0xCC, 0x41, 0x25),
    Color32::from_rgb(0xE0, 0x66, 0x66),
    Color32::from_rgb(0xF6, 0xB2, 0x6B),
    Color32::from_rgb(0xFF, 0xD9, 0x66),
    Color32::from_rgb(0x93, 0xC4, 0x7D),
    Color32::from_rgb(0x76, 0xA5, 0xAF),
    Color32::from_rgb(0x6D, 0x9E, 0xEB),
    Color32::from_rgb(0x6F, 0xA8, 0xDC),
    Color32::from_rgb(0x8E, 0x7C, 0xC3),
    Color32::from_rgb(0xC2, 0x7B, 0xA0),
    // Row 6: Rich
    Color32::from_rgb(0xA6, 0x1C, 0x00),
    Color32::from_rgb(0xCC, 0x00, 0x00),
    Color32::from_rgb(0xE6, 0x91, 0x38),
    Color32::from_rgb(0xF1, 0xC2, 0x32),
    Color32::from_rgb(0x6A, 0xA8, 0x4F),
    Color32::from_rgb(0x45, 0x81, 0x8E),
    Color32::from_rgb(0x3C, 0x78, 0xD8),
    Color32::from_rgb(0x3D, 0x85, 0xC6),
    Color32::from_rgb(0x67, 0x4E, 0xA7),
    Color32::from_rgb(0xA6, 0x4D, 0x79),
    // Row 7: Deep
    Color32::from_rgb(0x85, 0x20, 0x0C),
    Color32::from_rgb(0x99, 0x00, 0x00),
    Color32::from_rgb(0xB4, 0x5F, 0x06),
    Color32::from_rgb(0xBF, 0x90, 0x00),
    Color32::from_rgb(0x38, 0x76, 0x1D),
    Color32::from_rgb(0x13, 0x4F, 0x5C),
    Color32::from_rgb(0x11, 0x55, 0xCC),
    Color32::from_rgb(0x0B, 0x53, 0x94),
    Color32::from_rgb(0x35, 0x1C, 0x75),
    Color32::from_rgb(0x74, 0x1B, 0x47),
    // Row 8: Darkest
    Color32::from_rgb(0x5B, 0x0F, 0x00),
    Color32::from_rgb(0x66, 0x00, 0x00),
    Color32::from_rgb(0x78, 0x3F, 0x04),
    Color32::from_rgb(0x7F, 0x60, 0x00),
    Color32::from_rgb(0x27, 0x4E, 0x13),
    Color32::from_rgb(0x0C, 0x34, 0x3D),
    Color32::from_rgb(0x1C, 0x45, 0x87),
    Color32::from_rgb(0x07, 0x37, 0x63),
    Color32::from_rgb(0x20, 0x12, 0x4D),
    Color32::from_rgb(0x4C, 0x11, 0x30),
];

pub fn apply_light_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::light();
    visuals.window_fill = CARD_BG;
    visuals.panel_fill = SURFACE_BG;
    visuals.faint_bg_color = Color32::from_rgb(0xEE, 0xF1, 0xF5);
    visuals.extreme_bg_color = Color32::WHITE;
    visuals.selection.bg_fill = PURPLE_ACTION;
    visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.hyperlink_color = PURPLE_ACTION;
    visuals.widgets.noninteractive.bg_fill = CARD_BG;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER_LIGHT);
    visuals.widgets.inactive.bg_fill = Color32::WHITE;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER_LIGHT);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(6);
    visuals.widgets.hovered.bg_fill = PURPLE_SOFT;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.2, PURPLE_ACTION);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, PURPLE_PRIMARY);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(6);
    visuals.widgets.active.bg_fill = PURPLE_TINT;
    visuals.widgets.active.bg_stroke = Stroke::new(1.5, PURPLE_PRIMARY);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, PURPLE_PRIMARY);
    visuals.widgets.active.corner_radius = CornerRadius::same(6);
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(12.0, 6.0);
    ctx.set_style(style);
}

#[derive(Clone, Copy)]
pub enum DockIcon {
    Camera,
    Video,
    StopSquare,
    Pause,
    Play,
    Mic { muted: bool, level: f32 },
    Speaker { muted: bool, level: f32 },
    Webcam { active: bool },
    Gallery,
    Settings,
    ArrowTool,
    FrameTool,
    NoteTool,
    TextTool,
    AppWindow,
}

/// Crisp vector icons drawn directly onto egui's Painter (zero external font/image deps)
pub fn draw_icon(painter: &Painter, rect: Rect, icon: DockIcon, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height()) * 0.42;
    let stroke = Stroke::new((s * 0.16).clamp(1.5, 2.4), color);

    match icon {
        DockIcon::Camera => {
            let body = Rect::from_center_size(c + Vec2::new(0.0, s * 0.12), Vec2::new(s * 1.9, s * 1.3));
            painter.rect_stroke(body, CornerRadius::same(3), stroke, egui::StrokeKind::Middle);
            let top = Rect::from_center_size(c + Vec2::new(0.0, -s * 0.65), Vec2::new(s * 0.72, s * 0.3));
            painter.rect_filled(top, CornerRadius::same(2), color);
            painter.circle_stroke(body.center(), s * 0.36, stroke);
            painter.circle_filled(body.center(), s * 0.12, color);
        }
        DockIcon::Video => {
            let body = Rect::from_center_size(c + Vec2::new(-s * 0.25, 0.0), Vec2::new(s * 1.35, s * 1.1));
            painter.rect_stroke(body, CornerRadius::same(3), stroke, egui::StrokeKind::Middle);
            let lens = vec![
                Pos2::new(body.right() + s * 0.08, c.y - s * 0.18),
                Pos2::new(c.x + s * 0.95, c.y - s * 0.46),
                Pos2::new(c.x + s * 0.95, c.y + s * 0.46),
                Pos2::new(body.right() + s * 0.08, c.y + s * 0.18),
            ];
            painter.add(egui::Shape::convex_polygon(lens, Color32::TRANSPARENT, stroke));
            painter.circle_filled(body.left_top() + Vec2::new(s * 0.35, s * 0.35), s * 0.14, REC_RED);
        }
        DockIcon::StopSquare => {
            let sq = Rect::from_center_size(c, Vec2::splat(s * 1.1));
            painter.rect_filled(sq, CornerRadius::same(3), REC_RED);
        }
        DockIcon::Pause => {
            let bar_w = s * 0.36;
            let bar_h = s * 1.25;
            painter.rect_filled(
                Rect::from_center_size(c + Vec2::new(-s * 0.32, 0.0), Vec2::new(bar_w, bar_h)),
                CornerRadius::same(2),
                color,
            );
            painter.rect_filled(
                Rect::from_center_size(c + Vec2::new(s * 0.32, 0.0), Vec2::new(bar_w, bar_h)),
                CornerRadius::same(2),
                color,
            );
        }
        DockIcon::Play => {
            let pts = vec![
                Pos2::new(c.x - s * 0.45, c.y - s * 0.65),
                Pos2::new(c.x + s * 0.7, c.y),
                Pos2::new(c.x - s * 0.45, c.y + s * 0.65),
            ];
            painter.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
        }
        DockIcon::Mic { muted, level } => {
            let capsule = Rect::from_center_size(c + Vec2::new(0.0, -s * 0.25), Vec2::new(s * 0.72, s * 1.15));
            let active_col = if muted { Color32::from_gray(135) } else { color };
            let mic_stroke = Stroke::new(stroke.width, active_col);
            if !muted && level > 0.02 {
                let fill_h = capsule.height() * level.clamp(0.0, 1.0);
                let fill_rect = Rect::from_min_max(
                    Pos2::new(capsule.left() + 1.5, capsule.bottom() - fill_h),
                    Pos2::new(capsule.right() - 1.5, capsule.bottom() - 1.0),
                );
                painter.rect_filled(fill_rect, CornerRadius::same(4), LEVEL_GREEN);
            }
            painter.rect_stroke(capsule, CornerRadius::same(8), mic_stroke, egui::StrokeKind::Middle);
            // U-stand
            let u_left = Pos2::new(c.x - s * 0.62, c.y + s * 0.05);
            let u_bot_l = Pos2::new(c.x - s * 0.52, c.y + s * 0.55);
            let u_bot_r = Pos2::new(c.x + s * 0.52, c.y + s * 0.55);
            let u_right = Pos2::new(c.x + s * 0.62, c.y + s * 0.05);
            painter.line_segment([u_left, u_bot_l], mic_stroke);
            painter.line_segment([u_bot_l, u_bot_r], mic_stroke);
            painter.line_segment([u_bot_r, u_right], mic_stroke);
            painter.line_segment([Pos2::new(c.x, c.y + s * 0.55), Pos2::new(c.x, c.y + s * 0.92)], mic_stroke);
            painter.line_segment([Pos2::new(c.x - s * 0.35, c.y + s * 0.92), Pos2::new(c.x + s * 0.35, c.y + s * 0.92)], mic_stroke);
            if muted {
                painter.line_segment(
                    [Pos2::new(c.x - s * 0.8, c.y - s * 0.75), Pos2::new(c.x + s * 0.8, c.y + s * 0.85)],
                    Stroke::new(2.0, REC_RED),
                );
            }
        }
        DockIcon::Speaker { muted, level } => {
            let col = if muted { Color32::from_gray(135) } else { color };
            let st = Stroke::new(stroke.width, col);
            let cone = vec![
                Pos2::new(c.x - s * 0.75, c.y - s * 0.28),
                Pos2::new(c.x - s * 0.35, c.y - s * 0.28),
                Pos2::new(c.x + s * 0.1, c.y - s * 0.68),
                Pos2::new(c.x + s * 0.1, c.y + s * 0.68),
                Pos2::new(c.x - s * 0.35, c.y + s * 0.28),
                Pos2::new(c.x - s * 0.75, c.y + s * 0.28),
            ];
            painter.add(egui::Shape::convex_polygon(cone, col, Stroke::NONE));
            if muted {
                painter.line_segment(
                    [Pos2::new(c.x + s * 0.32, c.y - s * 0.32), Pos2::new(c.x + s * 0.82, c.y + s * 0.32)],
                    Stroke::new(2.0, REC_RED),
                );
                painter.line_segment(
                    [Pos2::new(c.x + s * 0.82, c.y - s * 0.32), Pos2::new(c.x + s * 0.32, c.y + s * 0.32)],
                    Stroke::new(2.0, REC_RED),
                );
            } else {
                let wave_col = if level > 0.15 { LEVEL_GREEN } else { col };
                let wst = Stroke::new(st.width, wave_col);
                painter.line_segment([Pos2::new(c.x + s * 0.42, c.y - s * 0.28), Pos2::new(c.x + s * 0.42, c.y + s * 0.28)], wst);
                painter.line_segment([Pos2::new(c.x + s * 0.72, c.y - s * 0.52), Pos2::new(c.x + s * 0.72, c.y + s * 0.52)], wst);
            }
        }
        DockIcon::Webcam { active } => {
            let col = if active { color } else { Color32::from_gray(135) };
            let st = Stroke::new(stroke.width, col);
            painter.circle_stroke(c + Vec2::new(0.0, -s * 0.14), s * 0.66, st);
            painter.circle_filled(c + Vec2::new(0.0, -s * 0.14), s * 0.22, if active { LEVEL_GREEN } else { col });
            painter.line_segment([Pos2::new(c.x - s * 0.5, c.y + s * 0.78), Pos2::new(c.x + s * 0.5, c.y + s * 0.78)], st);
            if !active {
                painter.line_segment(
                    [Pos2::new(c.x - s * 0.75, c.y - s * 0.75), Pos2::new(c.x + s * 0.75, c.y + s * 0.75)],
                    Stroke::new(2.0, REC_RED),
                );
            }
        }
        DockIcon::Gallery => {
            let gap = s * 0.18;
            let cell = s * 0.72;
            for gx in 0..2 {
                for gy in 0..2 {
                    let cx = c.x + (gx as f32 - 0.5) * (cell + gap);
                    let cy = c.y + (gy as f32 - 0.5) * (cell + gap);
                    let r = Rect::from_center_size(Pos2::new(cx, cy), Vec2::splat(cell));
                    painter.rect_stroke(r, CornerRadius::same(2), stroke, egui::StrokeKind::Middle);
                }
            }
        }
        DockIcon::Settings => {
            painter.circle_stroke(c, s * 0.42, stroke);
            for i in 0..8 {
                let ang = (i as f32) * std::f32::consts::FRAC_PI_4;
                let p0 = c + Vec2::new(ang.cos(), ang.sin()) * (s * 0.54);
                let p1 = c + Vec2::new(ang.cos(), ang.sin()) * (s * 0.86);
                painter.line_segment([p0, p1], Stroke::new(stroke.width * 1.15, color));
            }
        }
        DockIcon::ArrowTool => {
            let p0 = c + Vec2::new(-s * 0.7, s * 0.65);
            let p1 = c + Vec2::new(s * 0.7, -s * 0.65);
            painter.line_segment([p0, p1], stroke);
            painter.line_segment([p1, p1 + Vec2::new(-s * 0.55, s * 0.08)], stroke);
            painter.line_segment([p1, p1 + Vec2::new(-s * 0.08, s * 0.55)], stroke);
        }
        DockIcon::FrameTool => {
            let r = Rect::from_center_size(c, Vec2::new(s * 1.55, s * 1.15));
            painter.rect_stroke(r, CornerRadius::same(2), stroke, egui::StrokeKind::Middle);
        }
        DockIcon::NoteTool => {
            let r = Rect::from_center_size(c + Vec2::new(0.0, -s * 0.15), Vec2::new(s * 1.55, s * 1.05));
            painter.rect_stroke(r, CornerRadius::same(4), stroke, egui::StrokeKind::Middle);
            painter.line_segment([Pos2::new(c.x - s * 0.25, r.bottom()), Pos2::new(c.x - s * 0.55, c.y + s * 0.82)], stroke);
            painter.line_segment([Pos2::new(c.x - s * 0.55, c.y + s * 0.82), Pos2::new(c.x + s * 0.15, r.bottom())], stroke);
        }
        DockIcon::TextTool => {
            painter.line_segment([Pos2::new(c.x - s * 0.62, c.y - s * 0.68), Pos2::new(c.x + s * 0.62, c.y - s * 0.68)], stroke);
            painter.line_segment([Pos2::new(c.x, c.y - s * 0.68), Pos2::new(c.x, c.y + s * 0.72)], stroke);
            painter.line_segment([Pos2::new(c.x - s * 0.3, c.y + s * 0.72), Pos2::new(c.x + s * 0.3, c.y + s * 0.72)], stroke);
        }
        DockIcon::AppWindow => {
            let outer = Rect::from_center_size(c, Vec2::new(s * 1.75, s * 1.35));
            painter.rect_stroke(outer, CornerRadius::same(2), stroke, egui::StrokeKind::Middle);
            let titlebar = Rect::from_min_max(
                outer.min,
                Pos2::new(outer.max.x, outer.min.y + outer.height() * 0.30),
            );
            painter.rect_filled(titlebar, CornerRadius::same(2), color);
        }
    }
}

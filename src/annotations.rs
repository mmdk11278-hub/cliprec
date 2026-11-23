use egui::{Color32, CornerRadius, FontId, Painter, Pos2, Rect, Stroke, Vec2};
use image::{Rgba, RgbaImage};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnnotationTool {
    Select,
    Arrow,
    Frame,
    Note,
    Text,
}

impl AnnotationTool {
    pub fn label(self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::Arrow => "Arrow",
            Self::Frame => "Frame",
            Self::Note => "Callout",
            Self::Text => "Text",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Annotation {
    pub id: u64,
    pub tool: AnnotationTool,
    /// Normalized coordinates in image space [0.0 .. 1.0]
    pub start: [f32; 2],
    pub end: [f32; 2],
    pub fill_color: Color32,
    pub stroke_color: Color32,
    pub font_color: Color32,
    pub opacity_pct: u8, // 0..100
    pub stroke_width: f32,
    pub font_size: f32,
    pub font_bold: bool,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct ToolStyleDefaults {
    pub arrow_color: Color32,
    pub frame_fill: Color32,
    pub frame_stroke: Color32,
    pub frame_opacity: u8,
    pub note_fill: Color32,
    pub note_stroke: Color32,
    pub note_opacity: u8,
    pub note_font_color: Color32,
    pub note_font_size: f32,
    pub text_font_color: Color32,
    pub text_font_size: f32,
    pub font_bold: bool,
    pub stroke_width: f32,
}

impl Default for ToolStyleDefaults {
    /// Exact defaults from ScreenRec EditorSettingsManager.qml (zlib 0x7d427b)
    fn default() -> Self {
        Self {
            arrow_color: Color32::from_rgb(0xFF, 0x00, 0x00),
            frame_fill: Color32::WHITE,
            frame_stroke: Color32::from_rgb(0xFF, 0x00, 0x00),
            frame_opacity: 0,
            note_fill: Color32::WHITE,
            note_stroke: Color32::from_rgb(0xFF, 0x00, 0x00),
            note_opacity: 100,
            note_font_color: Color32::from_rgb(0xFF, 0x00, 0x00),
            note_font_size: 16.0,
            text_font_color: Color32::from_rgb(0xFF, 0x00, 0x00),
            text_font_size: 18.0,
            font_bold: false,
            stroke_width: 3.0,
        }
    }
}

pub struct AnnotationCanvas {
    pub items: Vec<Annotation>,
    pub undo_stack: Vec<Vec<Annotation>>,
    pub redo_stack: Vec<Vec<Annotation>>,
    pub active_tool: AnnotationTool,
    pub selected_id: Option<u64>,
    pub defaults: ToolStyleDefaults,
    pub next_id: u64,
    pub drawing: Option<Annotation>,
    pub dragging_id: Option<(u64, Pos2)>,
}

impl Default for AnnotationCanvas {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            active_tool: AnnotationTool::Arrow,
            selected_id: None,
            defaults: ToolStyleDefaults::default(),
            next_id: 1,
            drawing: None,
            dragging_id: None,
        }
    }
}

impl AnnotationCanvas {
    pub fn push_undo(&mut self) {
        self.undo_stack.push(self.items.clone());
        if self.undo_stack.len() > 50 {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    pub fn undo(&mut self) -> bool {
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(self.items.clone());
            self.items = prev;
            self.selected_id = None;
            true
        } else {
            false
        }
    }

    pub fn redo(&mut self) -> bool {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(self.items.clone());
            self.items = next;
            self.selected_id = None;
            true
        } else {
            false
        }
    }

    pub fn clear(&mut self) {
        if !self.items.is_empty() {
            self.push_undo();
            self.items.clear();
            self.selected_id = None;
        }
    }

    pub fn delete_selected(&mut self) {
        if let Some(id) = self.selected_id.take() {
            self.push_undo();
            self.items.retain(|a| a.id != id);
        }
    }

    pub fn selected_mut(&mut self) -> Option<&mut Annotation> {
        let id = self.selected_id?;
        self.items.iter_mut().find(|a| a.id == id)
    }

    pub fn new_annotation(&mut self, tool: AnnotationTool, start: [f32; 2], end: [f32; 2]) -> Annotation {
        let id = self.next_id;
        self.next_id += 1;
        match tool {
            AnnotationTool::Arrow => Annotation {
                id,
                tool,
                start,
                end,
                fill_color: self.defaults.arrow_color,
                stroke_color: self.defaults.arrow_color,
                font_color: self.defaults.arrow_color,
                opacity_pct: 100,
                stroke_width: self.defaults.stroke_width,
                font_size: 16.0,
                font_bold: false,
                text: String::new(),
            },
            AnnotationTool::Frame => Annotation {
                id,
                tool,
                start,
                end,
                fill_color: self.defaults.frame_fill,
                stroke_color: self.defaults.frame_stroke,
                font_color: self.defaults.frame_stroke,
                opacity_pct: self.defaults.frame_opacity,
                stroke_width: self.defaults.stroke_width,
                font_size: 16.0,
                font_bold: false,
                text: String::new(),
            },
            AnnotationTool::Note => Annotation {
                id,
                tool,
                start,
                end,
                fill_color: self.defaults.note_fill,
                stroke_color: self.defaults.note_stroke,
                font_color: self.defaults.note_font_color,
                opacity_pct: self.defaults.note_opacity,
                stroke_width: self.defaults.stroke_width,
                font_size: self.defaults.note_font_size,
                font_bold: self.defaults.font_bold,
                text: "Note".to_string(),
            },
            AnnotationTool::Text => Annotation {
                id,
                tool,
                start,
                end,
                fill_color: Color32::TRANSPARENT,
                stroke_color: self.defaults.text_font_color,
                font_color: self.defaults.text_font_color,
                opacity_pct: 100,
                stroke_width: 2.0,
                font_size: self.defaults.text_font_size,
                font_bold: self.defaults.font_bold,
                text: "Label".to_string(),
            },
            AnnotationTool::Select => Annotation {
                id,
                tool: AnnotationTool::Frame,
                start,
                end,
                fill_color: Color32::WHITE,
                stroke_color: Color32::RED,
                font_color: Color32::RED,
                opacity_pct: 0,
                stroke_width: 3.0,
                font_size: 16.0,
                font_bold: false,
                text: String::new(),
            },
        }
    }

    pub fn hit_test(&self, image_rect: Rect, pos: Pos2) -> Option<u64> {
        for ann in self.items.iter().rev() {
            let p0 = to_screen(image_rect, ann.start);
            let p1 = to_screen(image_rect, ann.end);
            let bounds = Rect::from_two_pos(p0, p1).expand(12.0);
            if bounds.contains(pos) {
                return Some(ann.id);
            }
        }
        None
    }

    pub fn paint(&self, painter: &Painter, image_rect: Rect) {
        for ann in &self.items {
            paint_annotation(painter, image_rect, ann, self.selected_id == Some(ann.id));
        }
        if let Some(ref draft) = self.drawing {
            paint_annotation(painter, image_rect, draft, true);
        }
    }
}

pub fn to_screen(rect: Rect, norm: [f32; 2]) -> Pos2 {
    Pos2::new(
        rect.left() + norm[0] * rect.width(),
        rect.top() + norm[1] * rect.height(),
    )
}

pub fn to_norm(rect: Rect, pos: Pos2) -> [f32; 2] {
    [
        ((pos.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0),
        ((pos.y - rect.top()) / rect.height().max(1.0)).clamp(0.0, 1.0),
    ]
}

fn with_opacity(col: Color32, pct: u8) -> Color32 {
    let a = ((pct.min(100) as f32 / 100.0) * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), a)
}

fn paint_annotation(painter: &Painter, image_rect: Rect, ann: &Annotation, selected: bool) {
    let p0 = to_screen(image_rect, ann.start);
    let p1 = to_screen(image_rect, ann.end);

    match ann.tool {
        AnnotationTool::Arrow => {
            let stroke = Stroke::new(ann.stroke_width.max(2.0), ann.stroke_color);
            painter.line_segment([p0, p1], stroke);
            let dir = p1 - p0;
            let len = dir.length();
            if len > 4.0 {
                let u = dir / len;
                let perp = Vec2::new(-u.y, u.x);
                let head_len = (ann.stroke_width * 4.5).clamp(12.0, 26.0);
                let head_w = head_len * 0.55;
                let tip = p1;
                let left = p1 - u * head_len + perp * head_w;
                let right = p1 - u * head_len - perp * head_w;
                painter.add(egui::Shape::convex_polygon(
                    vec![tip, left, right],
                    ann.stroke_color,
                    Stroke::NONE,
                ));
            }
            if selected {
                painter.circle_filled(p0, 4.5, Color32::WHITE);
                painter.circle_stroke(p0, 4.5, Stroke::new(1.5, crate::theme::PURPLE_PRIMARY));
                painter.circle_filled(p1, 4.5, Color32::WHITE);
                painter.circle_stroke(p1, 4.5, Stroke::new(1.5, crate::theme::PURPLE_PRIMARY));
            }
        }
        AnnotationTool::Frame => {
            let r = Rect::from_two_pos(p0, p1);
            if ann.opacity_pct > 0 {
                painter.rect_filled(r, CornerRadius::same(4), with_opacity(ann.fill_color, ann.opacity_pct));
            }
            painter.rect_stroke(
                r,
                CornerRadius::same(4),
                Stroke::new(ann.stroke_width.max(1.5), ann.stroke_color),
                egui::StrokeKind::Middle,
            );
            if selected {
                draw_selection_handles(painter, r);
            }
        }
        AnnotationTool::Note => {
            let mut r = Rect::from_two_pos(p0, p1);
            if r.width() < 80.0 {
                r.max.x = r.min.x + 140.0;
            }
            if r.height() < 40.0 {
                r.max.y = r.min.y + 52.0;
            }
            let fill = with_opacity(ann.fill_color, ann.opacity_pct);
            painter.rect_filled(r, CornerRadius::same(12), fill);
            painter.rect_stroke(
                r,
                CornerRadius::same(12),
                Stroke::new(ann.stroke_width.max(2.0), ann.stroke_color),
                egui::StrokeKind::Middle,
            );
            // Speech callout tail at bottom-left
            let tail = vec![
                Pos2::new(r.left() + 18.0, r.bottom() - 1.0),
                Pos2::new(r.left() + 6.0, r.bottom() + 16.0),
                Pos2::new(r.left() + 34.0, r.bottom() - 1.0),
            ];
            painter.add(egui::Shape::convex_polygon(
                tail,
                fill,
                Stroke::new(ann.stroke_width.max(1.5), ann.stroke_color),
            ));
            let font = FontId::proportional(ann.font_size.clamp(11.0, 44.0));
            painter.text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                &ann.text,
                font,
                ann.font_color,
            );
            if selected {
                draw_selection_handles(painter, r);
            }
        }
        AnnotationTool::Text => {
            let font = FontId::proportional(ann.font_size.clamp(12.0, 56.0));
            let galley = painter.layout_no_wrap(ann.text.clone(), font, ann.font_color);
            let r = Rect::from_min_size(p0, galley.size()).expand(4.0);
            if selected {
                painter.rect_stroke(
                    r,
                    CornerRadius::same(3),
                    Stroke::new(1.2, crate::theme::PURPLE_ACTION),
                    egui::StrokeKind::Outside,
                );
            }
            painter.galley(p0, galley, ann.font_color);
        }
        AnnotationTool::Select => {}
    }
}

fn draw_selection_handles(painter: &Painter, r: Rect) {
    let corners = [r.left_top(), r.right_top(), r.left_bottom(), r.right_bottom()];
    for pt in corners {
        painter.circle_filled(pt, 4.5, Color32::WHITE);
        painter.circle_stroke(pt, 4.5, Stroke::new(1.5, crate::theme::PURPLE_PRIMARY));
    }
}

/// Rasterizes annotations onto a copy of the screenshot for PNG export / clipboard copy
pub fn flatten_annotations(base: &RgbaImage, annotations: &[Annotation]) -> RgbaImage {
    let mut out = base.clone();
    let w = out.width() as f32;
    let h = out.height() as f32;

    for ann in annotations {
        let x0 = (ann.start[0] * w).round() as i32;
        let y0 = (ann.start[1] * h).round() as i32;
        let x1 = (ann.end[0] * w).round() as i32;
        let y1 = (ann.end[1] * h).round() as i32;
        let stroke_col = Rgba([ann.stroke_color.r(), ann.stroke_color.g(), ann.stroke_color.b(), 255]);
        let fill_a = ((ann.opacity_pct as f32 / 100.0) * 255.0).round() as u8;
        let fill_col = Rgba([ann.fill_color.r(), ann.fill_color.g(), ann.fill_color.b(), fill_a]);
        let thick = ann.stroke_width.round().max(2.0) as i32;

        match ann.tool {
            AnnotationTool::Arrow => {
                draw_thick_line(&mut out, x0, y0, x1, y1, thick, stroke_col);
                let dx = (x1 - x0) as f32;
                let dy = (y1 - y0) as f32;
                let len = (dx * dx + dy * dy).sqrt();
                if len > 4.0 {
                    let ux = dx / len;
                    let uy = dy / len;
                    let px = -uy;
                    let py = ux;
                    let hl = (thick as f32 * 5.0).clamp(14.0, 32.0);
                    let hw = hl * 0.55;
                    let lx = (x1 as f32 - ux * hl + px * hw) as i32;
                    let ly = (y1 as f32 - uy * hl + py * hw) as i32;
                    let rx = (x1 as f32 - ux * hl - px * hw) as i32;
                    let ry = (y1 as f32 - uy * hl - py * hw) as i32;
                    draw_thick_line(&mut out, x1, y1, lx, ly, thick, stroke_col);
                    draw_thick_line(&mut out, x1, y1, rx, ry, thick, stroke_col);
                    draw_thick_line(&mut out, lx, ly, rx, ry, thick, stroke_col);
                }
            }
            AnnotationTool::Frame | AnnotationTool::Note => {
                let rx0 = x0.min(x1);
                let ry0 = y0.min(y1);
                let mut rx1 = x0.max(x1);
                let mut ry1 = y0.max(y1);
                if ann.tool == AnnotationTool::Note {
                    if rx1 - rx0 < 100 {
                        rx1 = rx0 + 160;
                    }
                    if ry1 - ry0 < 48 {
                        ry1 = ry0 + 60;
                    }
                }
                if fill_a > 0 {
                    blend_rect(&mut out, rx0, ry0, rx1, ry1, fill_col);
                }
                draw_rect_border(&mut out, rx0, ry0, rx1, ry1, thick, stroke_col);
            }
            AnnotationTool::Text => {
                // Highlight underline indicator on exported image for text anchor
                draw_thick_line(&mut out, x0, y0 + 20, x0 + (ann.text.len() as i32 * 10).max(40), y0 + 20, 2, stroke_col);
            }
            AnnotationTool::Select => {}
        }
    }
    out
}

fn blend_pixel(img: &mut RgbaImage, x: i32, y: i32, col: Rgba<u8>) {
    if x < 0 || y < 0 || x >= img.width() as i32 || y >= img.height() as i32 {
        return;
    }
    let dst = img.get_pixel_mut(x as u32, y as u32);
    let a = col[3] as f32 / 255.0;
    let inv = 1.0 - a;
    dst[0] = (col[0] as f32 * a + dst[0] as f32 * inv) as u8;
    dst[1] = (col[1] as f32 * a + dst[1] as f32 * inv) as u8;
    dst[2] = (col[2] as f32 * a + dst[2] as f32 * inv) as u8;
    dst[3] = 255;
}

fn blend_rect(img: &mut RgbaImage, x0: i32, y0: i32, x1: i32, y1: i32, col: Rgba<u8>) {
    for y in y0.max(0)..=y1.min(img.height() as i32 - 1) {
        for x in x0.max(0)..=x1.min(img.width() as i32 - 1) {
            blend_pixel(img, x, y, col);
        }
    }
}

fn draw_rect_border(img: &mut RgbaImage, x0: i32, y0: i32, x1: i32, y1: i32, thick: i32, col: Rgba<u8>) {
    draw_thick_line(img, x0, y0, x1, y0, thick, col);
    draw_thick_line(img, x1, y0, x1, y1, thick, col);
    draw_thick_line(img, x1, y1, x0, y1, thick, col);
    draw_thick_line(img, x0, y1, x0, y0, thick, col);
}

fn draw_thick_line(img: &mut RgbaImage, x0: i32, y0: i32, x1: i32, y1: i32, thick: i32, col: Rgba<u8>) {
    let steps = (x1 - x0).abs().max((y1 - y0).abs()).max(1);
    let r = (thick / 2).max(1);
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let cx = (x0 as f32 + (x1 - x0) as f32 * t).round() as i32;
        let cy = (y0 as f32 + (y1 - y0) as f32 * t).round() as i32;
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy <= r * r + 1 {
                    blend_pixel(img, cx + dx, cy + dy, col);
                }
            }
        }
    }
}

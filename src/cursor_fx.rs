//! Cursor visual-effects overlay for recorded/screenshot frames.
//!
//! Owns no state beyond what it draws: it paints a crisp arrow cursor and an
//! optional effect (highlight ring, spotlight, click ripples, laser pointer,
//! or magnifier) into an [`image::RgbaImage`]. On Windows the live cursor
//! position and mouse buttons are queried through Win32 FFI and mapped into
//! the frame by [`CursorFxTracker`], which also keeps click ripples alive for
//! ~450 ms after each press.

use std::time::{Duration, Instant};

use image::{Rgba, RgbaImage};

/// How long a click ripple stays visible after the press that created it.
const RIPPLE_LIFETIME: Duration = Duration::from_millis(450);

/// Left-button ripple colour (red/orange).
const RIPPLE_LEFT_RGBA: [u8; 4] = [255, 59, 48, 255];
/// Right-button ripple colour (blue).
const RIPPLE_RIGHT_RGBA: [u8; 4] = [54, 145, 225, 255];
/// Laser-pointer dot colour.
const LASER_RGBA: [u8; 4] = [255, 30, 30, 255];
/// Magnifier border accent.
const MAGNIFIER_PURPLE: [u8; 4] = [168, 85, 247, 255];

/// Available cursor overlay styles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CursorStyle {
    None,
    HighlightRing,
    Spotlight,
    ClickRipple,
    RingAndRipple,
    LaserPointer,
    Magnifier,
}

impl CursorStyle {
    /// Human-readable label for UI pickers.
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::HighlightRing => "Highlight Ring",
            Self::Spotlight => "Spotlight",
            Self::ClickRipple => "Click Ripple",
            Self::RingAndRipple => "Ring + Ripple",
            Self::LaserPointer => "Laser Pointer",
            Self::Magnifier => "Magnifier",
        }
    }

    /// Every style in display order.
    pub fn all() -> &'static [CursorStyle] {
        use CursorStyle::*;
        &[
            None,
            HighlightRing,
            Spotlight,
            ClickRipple,
            RingAndRipple,
            LaserPointer,
            Magnifier,
        ]
    }
}

/// Win32 cursor/button access, or a harmless stub on other platforms.
#[cfg(target_os = "windows")]
mod win32 {
    use std::os::raw::c_int;

    #[link(name = "user32")]
    extern "system" {
        fn GetCursorPos(point: *mut Point) -> c_int;
        fn GetAsyncKeyState(virtual_key: c_int) -> i16;
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    struct Point {
        x: c_int,
        y: c_int,
    }

    /// VK_LBUTTON
    pub(super) const VK_LBUTTON: u32 = 0x01;
    /// VK_RBUTTON
    pub(super) const VK_RBUTTON: u32 = 0x02;

    /// Current cursor position in screen coordinates, if obtainable.
    pub(super) fn cursor_position() -> Option<(i32, i32)> {
        let mut point = Point::default();
        // SAFETY: `point` is a valid, writable object of the exact repr C
        // shape that `GetCursorPos` expects.
        let ok = unsafe { GetCursorPos(&mut point) };
        (ok != 0).then_some((point.x, point.y))
    }

    /// Whether the given virtual key is currently held down.
    pub(super) fn key_down(virtual_key: u32) -> bool {
        // SAFETY: stateless Win32 query with a well-defined virtual-key code.
        let state = unsafe { GetAsyncKeyState(virtual_key as c_int) };
        (state as u16) & 0x8000 != 0
    }
}

#[cfg(not(target_os = "windows"))]
mod win32 {
    /// VK_LBUTTON
    pub(super) const VK_LBUTTON: u32 = 0x01;
    /// VK_RBUTTON
    pub(super) const VK_RBUTTON: u32 = 0x02;

    /// No live cursor on non-Windows targets.
    pub(super) fn cursor_position() -> Option<(i32, i32)> {
        None
    }

    /// No button state available off Windows.
    pub(super) fn key_down(_virtual_key: u32) -> bool {
        false
    }
}

/// A click that is still animating.
struct Ripple {
    /// Frame-space centre x.
    x: f32,
    /// Frame-space centre y.
    y: f32,
    /// Whether this was a left-button (`true`) or right-button (`false`) press.
    left: bool,
    /// When the press was observed.
    started: Instant,
}

/// Tracks the live Windows cursor and animates click ripples across frames.
pub struct CursorFxTracker {
    ripples: Vec<Ripple>,
    prev_left: bool,
    prev_right: bool,
}

impl CursorFxTracker {
    /// Creates a tracker with no pending ripples.
    pub fn new() -> Self {
        Self {
            ripples: Vec::new(),
            prev_left: false,
            prev_right: false,
        }
    }

    /// Paints `style` onto `frame`.
    ///
    /// `region` is `[x, y, width, height]` in screen coordinates of the area
    /// shown in the frame; the live cursor is mapped through it so it lands on
    /// the correct frame pixel, scaled to `frame.dimensions()`. Click ripples
    /// are recorded for ~450 ms and drawn as expanding, fading rings whenever
    /// the style includes them.
    pub fn apply_to_frame(
        &mut self,
        frame: &mut RgbaImage,
        region: [u32; 4],
        style: CursorStyle,
        radius_px: f32,
        color_rgba: [u8; 4],
    ) {
        if style == CursorStyle::None {
            self.ripples.retain(|r| r.started.elapsed() < RIPPLE_LIFETIME);
            self.prev_left = false;
            self.prev_right = false;
            return;
        }

        let (width, height) = frame.dimensions();
        if width == 0 || height == 0 {
            return;
        }

        let (cursor_x, cursor_y) = match win32::cursor_position() {
            Some(position) => position,
            None => return,
        };
        let [region_x, region_y, region_w, region_h] = region;
        let fx = (cursor_x as f32 - region_x as f32) * width as f32 / region_w.max(1) as f32;
        let fy = (cursor_y as f32 - region_y as f32) * height as f32 / region_h.max(1) as f32;

        let left_down = win32::key_down(win32::VK_LBUTTON);
        let right_down = win32::key_down(win32::VK_RBUTTON);
        if left_down && !self.prev_left {
            self.ripples.push(Ripple {
                x: fx,
                y: fy,
                left: true,
                started: Instant::now(),
            });
        }
        if right_down && !self.prev_right {
            self.ripples.push(Ripple {
                x: fx,
                y: fy,
                left: false,
                started: Instant::now(),
            });
        }
        self.prev_left = left_down;
        self.prev_right = right_down;
        self.ripples.retain(|r| r.started.elapsed() < RIPPLE_LIFETIME);

        if matches!(
            style,
            CursorStyle::ClickRipple | CursorStyle::RingAndRipple
        ) {
            for ripple in &self.ripples {
                draw_expanding_ripple(
                    frame,
                    ripple.x,
                    ripple.y,
                    ripple.left,
                    radius_px,
                    ripple.started.elapsed(),
                );
            }
        }

        draw_cursor_at(
            frame,
            fx.round() as i32,
            fy.round() as i32,
            style,
            radius_px,
            color_rgba,
        );
    }
}

impl Default for CursorFxTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Paints the whole cursor effect at `(cx, cy)` in a single call.
///
/// `left_down`/`right_down` control the ripple rings drawn by
/// [`CursorStyle::ClickRipple`] and [`CursorStyle::RingAndRipple`].
pub fn draw_cursor_fx_at(
    frame: &mut RgbaImage,
    cx: i32,
    cy: i32,
    left_down: bool,
    right_down: bool,
    style: CursorStyle,
    radius_px: f32,
    color_rgba: [u8; 4],
) {
    if style == CursorStyle::None {
        return;
    }
    if matches!(
        style,
        CursorStyle::ClickRipple | CursorStyle::RingAndRipple
    ) {
        if left_down {
            draw_static_ripple(frame, cx, cy, true, radius_px);
        }
        if right_down {
            draw_static_ripple(frame, cx, cy, false, radius_px);
        }
    }
    draw_cursor_at(frame, cx, cy, style, radius_px, color_rgba);
}

/// Draws everything except the animated/static ripples.
fn draw_cursor_at(
    frame: &mut RgbaImage,
    cx: i32,
    cy: i32,
    style: CursorStyle,
    radius_px: f32,
    color_rgba: [u8; 4],
) {
    match style {
        CursorStyle::None => {}
        CursorStyle::HighlightRing | CursorStyle::RingAndRipple => {
            draw_highlight_ring(frame, cx, cy, radius_px, color_rgba);
        }
        CursorStyle::Spotlight => draw_spotlight(frame, cx, cy, radius_px),
        CursorStyle::ClickRipple => {}
        CursorStyle::LaserPointer => draw_laser_pointer(frame, cx, cy, radius_px),
        CursorStyle::Magnifier => draw_magnifier(frame, cx, cy, radius_px),
    }
    draw_arrow(frame, cx, cy);
}

/// Anti-aliased glowing ring with a soft translucent fill.
fn draw_highlight_ring(frame: &mut RgbaImage, cx: i32, cy: i32, radius_px: f32, color_rgba: [u8; 4]) {
    let radius = radius_px.max(1.0);
    let glow_half_width = (radius * 0.24).clamp(2.0, 24.0);
    let crisp_half_width = (radius * 0.07).clamp(1.0, 9.0);

    // Soft translucent fill.
    fill_circle(
        frame,
        cx as f32,
        cy as f32,
        radius * 0.88,
        color_rgba,
        0.133,
        (radius * 0.22).clamp(2.0, 24.0),
    );
    // Wide, faint halo pass, then a crisp ring on top.
    draw_ring(
        frame,
        cx as f32,
        cy as f32,
        radius,
        glow_half_width,
        color_rgba,
        0.251,
    );
    draw_ring(
        frame,
        cx as f32,
        cy as f32,
        radius,
        crisp_half_width,
        color_rgba,
        0.882,
    );
}

/// Dims the frame outside the spotlight while feathering the transition.
fn draw_spotlight(frame: &mut RgbaImage, cx: i32, cy: i32, radius_px: f32) {
    let radius = radius_px.max(1.0);
    let fade_end = radius * 1.8;
    let (width, height) = frame.dimensions();
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 + 0.5 - cx as f32;
            let dy = y as f32 + 0.5 - cy as f32;
            let distance = dx.hypot(dy);
            // 0 inside `radius`, 1 at/after `fade_end`, smoothly feathered in between.
            let t = smoothstep(radius, fade_end, distance);
            let factor = 1.0 - 0.55 * t;
            if factor < 1.0 {
                dim_pixel(frame.get_pixel_mut(x, y), factor);
            }
        }
    }
}

/// Glowing red laser dot with a bright white core.
fn draw_laser_pointer(frame: &mut RgbaImage, cx: i32, cy: i32, radius_px: f32) {
    let glow = radius_px.max(2.0);
    fill_circle(
        frame,
        cx as f32,
        cy as f32,
        glow * 0.75,
        LASER_RGBA,
        0.086,
        glow * 0.22,
    );
    fill_circle(
        frame,
        cx as f32,
        cy as f32,
        glow * 0.45,
        LASER_RGBA,
        0.235,
        glow * 0.16,
    );
    fill_circle(
        frame,
        cx as f32,
        cy as f32,
        (glow * 0.22).max(1.5),
        LASER_RGBA,
        0.627,
        1.2,
    );
    fill_circle(
        frame,
        cx as f32,
        cy as f32,
        1.8,
        [255, 255, 255, 255],
        0.922,
        1.0,
    );
}

/// 2x zoom lens ringed by a dark edge, purple band, and white inner band.
fn draw_magnifier(frame: &mut RgbaImage, cx: i32, cy: i32, radius_px: f32) {
    let (width, height) = frame.dimensions();
    if width == 0 || height == 0 {
        return;
    }
    let radius = radius_px.max(1.0);
    // Work in pixel-centre space so rings hit integer pixels crisply.
    let fcx = cx as f32 + 0.5;
    let fcy = cy as f32 + 0.5;
    let pad = 3.0;
    let min_x = ((fcx - radius - pad).floor().max(0.0) as u32).min(width - 1);
    let min_y = ((fcy - radius - pad).floor().max(0.0) as u32).min(height - 1);
    let max_x = ((fcx + radius + pad).ceil().max(0.0) as u32).min(width - 1);
    let max_y = ((fcy + radius + pad).ceil().max(0.0) as u32).min(height - 1);

    // Snapshot the lens neighbourhood before remapping any of it.
    let buffer_width = max_x - min_x + 1;
    let mut source: Vec<[u8; 4]> = Vec::with_capacity((buffer_width * (max_y - min_y + 1)) as usize);
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let pixel = frame.get_pixel(x, y);
            source.push([pixel[0], pixel[1], pixel[2], pixel[3]]);
        }
    }

    let radius_squared = radius * radius;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let dx = x as f32 + 0.5 - fcx;
            let dy = y as f32 + 0.5 - fcy;
            if dx * dx + dy * dy > radius_squared {
                continue;
            }
            // Discrete 2x zoom: sample the pixel half-way toward the centre.
            let offset_x = (x as i64 - cx as i64).div_euclid(2);
            let offset_y = (y as i64 - cy as i64).div_euclid(2);
            let sample_x = (cx as i64 + offset_x).clamp(min_x as i64, max_x as i64) as u32;
            let sample_y = (cy as i64 + offset_y).clamp(min_y as i64, max_y as i64) as u32;
            let index = ((sample_y - min_y) * buffer_width + (sample_x - min_x)) as usize;
            *frame.get_pixel_mut(x, y) = Rgba(source[index]);
        }
    }

    // Crisp white + purple border ring: dark outline, purple band, white lip.
    draw_ring(frame, fcx, fcy, radius, 1.2, [15, 10, 25, 255], 1.0);
    draw_ring(
        frame,
        fcx,
        fcy,
        radius - 2.0,
        1.4,
        MAGNIFIER_PURPLE,
        1.0,
    );
    draw_ring(
        frame,
        fcx,
        fcy,
        radius - 4.0,
        1.0,
        [255, 255, 255, 255],
        1.0,
    );
}

/// Classic arrow pointer mask, one `u32` row per line (LSB = column 0).
const ARROW_WIDTH: usize = 11;
const ARROW_ROWS: [u32; 19] = [
    0b00000000001,
    0b00000000011,
    0b00000000111,
    0b00000001111,
    0b00000011111,
    0b00000111111,
    0b00001111111,
    0b00011111111,
    0b00111111111,
    0b01111111111,
    0b11111111111,
    0b11111111011,
    0b11111110111,
    0b11111101111,
    0b11111011111,
    0b11110111111,
    0b11101111111,
    0b11011111111,
    0b10111111111,
];

/// Crisp white arrow with a 1px dark outline; tip is the hot spot `(cx, cy)`.
fn draw_arrow(frame: &mut RgbaImage, cx: i32, cy: i32) {
    // Offsets first so the white core on top leaves only a clean dark border.
    for dy in -1i32..=1 {
        for dx in -1i32..=1 {
            if dx == 0 && dy == 0 {
                continue;
            }
            paint_arrow_mask(frame, cx + dx, cy + dy, [0, 0, 0, 255]);
        }
    }
    paint_arrow_mask(frame, cx, cy, [255, 255, 255, 255]);
}

fn paint_arrow_mask(frame: &mut RgbaImage, origin_x: i32, origin_y: i32, rgba: [u8; 4]) {
    let (width, height) = frame.dimensions();
    for (row, bits) in ARROW_ROWS.iter().enumerate() {
        let y = origin_y + row as i32;
        if y < 0 || y >= height as i32 {
            continue;
        }
        for col in 0..ARROW_WIDTH {
            if bits & (1 << col) == 0 {
                continue;
            }
            let x = origin_x + col as i32;
            if x >= 0 && x < width as i32 {
                *frame.get_pixel_mut(x as u32, y as u32) = Rgba(rgba);
            }
        }
    }
}

/// A ripple that expands and fades out over [`RIPPLE_LIFETIME`].
fn draw_expanding_ripple(
    frame: &mut RgbaImage,
    x: f32,
    y: f32,
    left: bool,
    radius_px: f32,
    elapsed: Duration,
) {
    let progress = (elapsed.as_secs_f32() / RIPPLE_LIFETIME.as_secs_f32()).clamp(0.0, 1.0);
    let fade = 1.0 - progress;
    let rgba = if left { RIPPLE_LEFT_RGBA } else { RIPPLE_RIGHT_RGBA };
    for ring_index in 0..2u32 {
        let radius = radius_px.max(1.0) * (0.55 + 0.45 * progress + 0.5 * ring_index as f32);
        let half_width = (radius_px * 0.12).clamp(1.0, 6.0);
        draw_ring(frame, x, y, radius, half_width, rgba, 0.667 * fade);
    }
}

/// Two concentric rings drawn while the button is held down (static variant).
fn draw_static_ripple(
    frame: &mut RgbaImage,
    cx: i32,
    cy: i32,
    left: bool,
    radius_px: f32,
) {
    let rgba = if left { RIPPLE_LEFT_RGBA } else { RIPPLE_RIGHT_RGBA };
    let radius = radius_px.max(1.0);
    let half_width = (radius * 0.12).clamp(1.0, 6.0);
    draw_ring(frame, cx as f32, cy as f32, radius * 0.70, half_width, rgba, 0.706);
    draw_ring(frame, cx as f32, cy as f32, radius * 1.15, half_width, rgba, 0.51);
}

/// Anti-aliased ring of `half_width` at distance `radius` from `(cx, cy)`.
fn draw_ring(
    frame: &mut RgbaImage,
    cx: f32,
    cy: f32,
    radius: f32,
    half_width: f32,
    rgba: [u8; 4],
    alpha: f32,
) {
    if alpha <= 0.0 || half_width <= 0.0 {
        return;
    }
    let (width, height) = frame.dimensions();
    let reach = radius.max(0.0) + half_width + 2.0;
    let x0 = ((cx - reach).floor().max(0.0) as u32).min(width);
    let y0 = ((cy - reach).floor().max(0.0) as u32).min(height);
    let x1 = ((cx + reach).ceil().max(0.0) as u32).min(width);
    let y1 = ((cy + reach).ceil().max(0.0) as u32).min(height);
    for y in y0..y1 {
        for x in x0..x1 {
            let distance = ((x as f32 + 0.5) - cx).hypot((y as f32 + 0.5) - cy);
            let coverage = 1.0 - smoothstep(0.0, half_width, (distance - radius).abs());
            if coverage > 0.0 {
                blend_pixel(frame.get_pixel_mut(x, y), rgba, alpha * coverage);
            }
        }
    }
}

/// Anti-aliased filled circle fading out past `radius`.
fn fill_circle(
    frame: &mut RgbaImage,
    cx: f32,
    cy: f32,
    radius: f32,
    rgba: [u8; 4],
    alpha: f32,
    feather: f32,
) {
    if alpha <= 0.0 || radius <= 0.0 {
        return;
    }
    let (width, height) = frame.dimensions();
    let reach = radius + feather + 1.0;
    let x0 = ((cx - reach).floor().max(0.0) as u32).min(width);
    let y0 = ((cy - reach).floor().max(0.0) as u32).min(height);
    let x1 = ((cx + reach).ceil().max(0.0) as u32).min(width);
    let y1 = ((cy + reach).ceil().max(0.0) as u32).min(height);
    for y in y0..y1 {
        for x in x0..x1 {
            let distance = ((x as f32 + 0.5) - cx).hypot((y as f32 + 0.5) - cy);
            let coverage = 1.0 - smoothstep(radius - feather, radius + feather, distance);
            if coverage > 0.0 {
                blend_pixel(frame.get_pixel_mut(x, y), rgba, alpha * coverage);
            }
        }
    }
}

/// Alpha-composites `rgba` (scaled by `alpha`) over `pixel`.
fn blend_pixel(pixel: &mut Rgba<u8>, rgba: [u8; 4], alpha: f32) {
    let source_alpha = (rgba[3] as f32 / 255.0) * alpha.clamp(0.0, 1.0);
    if source_alpha <= 0.0 {
        return;
    }
    let dest_alpha = pixel[3] as f32 / 255.0;
    let out_alpha = source_alpha + dest_alpha * (1.0 - source_alpha);
    if out_alpha <= 0.0 {
        return;
    }
    for channel in 0..3 {
        let source = rgba[channel] as f32;
        let dest = pixel[channel] as f32;
        let blended = (source * source_alpha + dest * dest_alpha * (1.0 - source_alpha)) / out_alpha;
        pixel[channel] = blended.round().clamp(0.0, 255.0) as u8;
    }
    pixel[3] = (out_alpha * 255.0).round() as u8;
}

/// Scales RGB by `factor` (alpha left alone: the frame is screen content).
fn dim_pixel(pixel: &mut Rgba<u8>, factor: f32) {
    pixel[0] = (pixel[0] as f32 * factor).round() as u8;
    pixel[1] = (pixel[1] as f32 * factor).round() as u8;
    pixel[2] = (pixel[2] as f32 * factor).round() as u8;
}

/// Smooth Hermite interpolation between `edge0` and `edge1`.
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::{CursorStyle, draw_cursor_fx_at};
    use image::{Rgba, RgbaImage};

    fn assert_any_pixel(image: &RgbaImage, predicate: impl Fn(&Rgba<u8>) -> bool) {
        for pixel in image.pixels() {
            if predicate(pixel) {
                return;
            }
        }
        panic!("expected at least one pixel to satisfy the predicate");
    }

    #[test]
    fn highlight_ring_paints_glowing_ring_and_fill() {
        let mut image = RgbaImage::from_pixel(64, 64, Rgba([20, 20, 20, 255]));
        draw_cursor_fx_at(
            &mut image,
            32,
            32,
            false,
            false,
            CursorStyle::HighlightRing,
            10.0,
            [255, 180, 60, 255],
        );
        // The ring band should carry the warm highlight colour, not just the
        // white arrow (which would saturate all channels).
        assert_any_pixel(&image, |p| p[0] > 150 && p[1] > 100 && p[2] < 140);
    }

    #[test]
    fn spotlight_dims_outside_and_keeps_centre_bright() {
        let mut image = RgbaImage::from_pixel(64, 64, Rgba([255, 200, 100, 255]));
        draw_cursor_fx_at(
            &mut image,
            32,
            32,
            false,
            false,
            CursorStyle::Spotlight,
            8.0,
            [255, 255, 255, 255],
        );
        // Past radius * 1.8 the pixels are dimmed by ~55%.
        let corner = image.get_pixel(0, 0);
        assert!(corner[0] < 160 && corner[1] < 130 && corner[2] < 80);
        // Inside the full-brightness circle nothing should fade (spot is
        // above the arrow, which hangs below the hot spot).
        let inside = image.get_pixel(32, 25);
        assert!(inside[0] > 250 && inside[1] > 195 && inside[2] > 95);
    }

    #[test]
    fn click_ripple_uses_red_for_left_and_blue_for_right() {
        let mut left_image = RgbaImage::from_pixel(64, 64, Rgba([10, 10, 10, 255]));
        draw_cursor_fx_at(
            &mut left_image,
            32,
            32,
            true,
            false,
            CursorStyle::ClickRipple,
            10.0,
            [255, 255, 255, 255],
        );
        assert_any_pixel(&left_image, |p| p[0] > 150 && p[1] < 120 && p[2] < 120);

        let mut right_image = RgbaImage::from_pixel(64, 64, Rgba([10, 10, 10, 255]));
        draw_cursor_fx_at(
            &mut right_image,
            32,
            32,
            false,
            true,
            CursorStyle::ClickRipple,
            10.0,
            [255, 255, 255, 255],
        );
        assert_any_pixel(&right_image, |p| p[2] > 150 && p[0] < 120);
    }

    #[test]
    fn magnifier_zooms_and_draws_border() {
        let mut image = RgbaImage::from_pixel(64, 64, Rgba([255, 0, 0, 255]));
        // Marker sits left of the hot spot, clear of the arrow and its outline.
        image.put_pixel(30, 32, Rgba([255, 255, 255, 255]));
        draw_cursor_fx_at(
            &mut image,
            32,
            32,
            false,
            false,
            CursorStyle::Magnifier,
            14.0,
            [255, 255, 255, 255],
        );
        // 2x zoom: the marker at (30, 32) reappears at (28, 32).
        let zoomed = image.get_pixel(28, 32);
        assert!(zoomed[0] > 240 && zoomed[1] > 240 && zoomed[2] > 240);
        // Nearby lens pixels not under the marker keep the red background.
        let unchanged = image.get_pixel(27, 32);
        assert!(unchanged[0] > 200 && unchanged[1] < 80 && unchanged[2] < 80);
        // The purple band of the border ring is visible.
        let border = image.get_pixel(20, 32);
        assert!(border[2] > 200 && border[0] > 120 && border[1] < 160);
    }
}

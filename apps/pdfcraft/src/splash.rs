//! The opening animation, as approved in the Uniphar adjustment board: on white, the app's icon
//! builds itself (the rounded base spins in, the sheet drops, its lines draw, the letter tile flies
//! in), slides left as the app's name opens beside it, and "Uniphar Indústria Farmacêutica" comes
//! in under the name, aligned with it. Then it fades out over the app, which loaded underneath.
//!
//! A click or a key ends it at once (the key is not passed on). With the system's animation
//! effects off it shows the finished picture, still, before the fade.

use egui::{Color32, ColorImage, Context, Id, Order, Rect, Sense, TextureHandle, TextureOptions, Ui, Vec2, pos2, vec2};

/// The pictures: the icon's four parts (each on the icon's whole square), the app's name and the
/// company line (each cut to its ink, so their left edges line up).
const PICTURES: [&[u8]; 6] = [
    include_bytes!("../../../assets/splash/base.png"),
    include_bytes!("../../../assets/splash/sheet.png"),
    include_bytes!("../../../assets/splash/motif.png"),
    include_bytes!("../../../assets/splash/tile.png"),
    include_bytes!("../../../assets/splash/nome.png"),
    include_bytes!("../../../assets/splash/empresa.png"),
];

/// Seconds from the first frame: the fade starts, and the splash is gone.
const FADE: f32 = 3.3;
const FADE_LEN: f32 = 0.35;
/// With animation effects off, how long the finished picture stays before the fade.
const STILL: f32 = 1.2;

pub struct Splash {
    pictures: Option<Vec<TextureHandle>>,
    start: Option<f64>,
    /// The previous frame's time: a frame that took long (the window still opening, shaders being
    /// built) doesn't count, so the animation is seen from its start.
    last: Option<f64>,
    /// When the fade starts: `FADE`, sooner once skipped, `STILL` without animations.
    fade_at: f32,
    still: bool,
    done: bool,
}

impl Splash {
    /// `still`: the system asks for no animations. `skip`: no splash at all (an agent drives the app).
    pub fn new(still: bool, skip: bool) -> Self {
        Self { pictures: None, start: None, last: None, fade_at: if still { STILL } else { FADE }, still, done: skip }
    }

    /// Draws the splash over everything; call before the app's own UI so a key that ends it
    /// doesn't reach the document.
    pub fn show(&mut self, ui: &mut Ui) {
        if self.done {
            return;
        }
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let start = self.start.get_or_insert(now);
        if let Some(last) = self.last
            && now - last > 0.1
        {
            *start += now - last - 1.0 / 60.0;
        }
        self.last = Some(now);
        let t = (now - *start) as f32;
        // A click or a key: straight to the fade.
        let pressed = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Key { pressed: true, .. } | egui::Event::Text(_))));
        if pressed {
            ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Key { .. } | egui::Event::Text(_))));
            self.fade_at = self.fade_at.min(t);
        }
        let fade = ((t - self.fade_at) / FADE_LEN).clamp(0.0, 1.0);
        if fade >= 1.0 {
            self.done = true;
            self.pictures = None;
            return;
        }
        let Some(pictures) = self.pictures(&ctx) else {
            self.done = true;
            return;
        };
        // Without animations, everything is where it ends up.
        let a = if self.still { FADE } else { t };
        let screen = ctx.viewport_rect();
        egui::Area::new(Id::new("uniphar-splash")).order(Order::Tooltip).fixed_pos(screen.min).show(&ctx, |ui| {
            let clicked = ui.allocate_rect(screen, Sense::click()).clicked();
            if clicked {
                self.fade_at = self.fade_at.min(t);
            }
            let alpha = 1.0 - fade;
            ui.painter().rect_filled(screen, 0.0, Color32::WHITE.gamma_multiply(alpha));
            draw(ui, &pictures, screen, a, alpha);
        });
        ctx.request_repaint();
    }

    fn pictures(&mut self, ctx: &Context) -> Option<Vec<TextureHandle>> {
        if self.pictures.is_none() {
            let options = TextureOptions { mipmap_mode: Some(egui::TextureFilter::Linear), ..TextureOptions::LINEAR };
            let loaded: Option<Vec<_>> = PICTURES
                .iter()
                .enumerate()
                .map(|(i, png)| {
                    let icon = eframe::icon_data::from_png_bytes(png).map_err(|e| log::warn!("splash picture {i}: {e}")).ok()?;
                    let image = ColorImage::from_rgba_unmultiplied([icon.width as usize, icon.height as usize], &icon.rgba);
                    Some(ctx.load_texture(format!("uniphar-splash-{i}"), image, options))
                })
                .collect();
            self.pictures = loaded;
        }
        self.pictures.clone()
    }
}

/// How far `t` is through the step that starts at `start` and lasts `len`, from 0 to 1.
fn step(t: f32, start: f32, len: f32) -> f32 {
    ((t - start) / len).clamp(0.0, 1.0)
}

/// Overshoots a little and settles, like a thing that lands.
fn back_out(x: f32) -> f32 {
    let (c1, c3) = (1.70158, 2.70158);
    1.0 + c3 * (x - 1.0).powi(3) + c1 * (x - 1.0).powi(2)
}

fn cubic_out(x: f32) -> f32 {
    1.0 - (1.0 - x).powi(3)
}

fn cubic_in_out(x: f32) -> f32 {
    if x < 0.5 { 4.0 * x * x * x } else { 1.0 - (-2.0 * x + 2.0).powi(3) / 2.0 }
}

/// `rect` scaled by `k` about the point at `pivot` (as fractions of its size).
fn scaled(rect: Rect, k: f32, pivot: Vec2) -> Rect {
    let p = rect.min + rect.size() * pivot;
    Rect::from_min_max(p + (rect.min - p) * k, p + (rect.max - p) * k)
}

fn picture(ui: &Ui, texture: &TextureHandle, rect: Rect, angle: f32, pivot: Vec2, alpha: f32) {
    egui::Image::new((texture.id(), rect.size()))
        .rotate(angle, pivot)
        .tint(Color32::WHITE.gamma_multiply(alpha.clamp(0.0, 1.0)))
        .paint_at(ui, rect);
}

fn draw(ui: &mut Ui, p: &[TextureHandle], screen: Rect, t: f32, alpha: f32) {
    let aspect = |i: usize| {
        let [w, h] = p[i].size();
        w as f32 / h.max(1) as f32
    };
    // Sizes as fractions of the icon, measured on the approved preview.
    let icon = (screen.width().min(screen.height()) * 0.2).clamp(72.0, 150.0);
    let name_h = icon * 0.307;
    let line_h = icon * 0.100;
    let (name_w, line_w) = (name_h * aspect(4), line_h * aspect(5));
    let gap = icon * 0.193;
    let text_w = name_w.max(line_w);
    let text_h = name_h + icon * 0.09 + line_h;

    // 2. The text opens beside the icon, which moves left to keep the pair centred.
    let open = cubic_in_out(step(t, 1.45, 0.6));
    let row = icon + open * (gap + text_w);
    let left = screen.center().x - row / 2.0;
    let icon_rect = Rect::from_min_size(pos2(left, screen.center().y - icon / 2.0), Vec2::splat(icon));
    let centre = Vec2::splat(0.5);

    // 1. The icon builds itself.
    let base = back_out(step(t, 0.0, 0.7));
    picture(ui, &p[0], scaled(icon_rect, 0.2 + 0.8 * base, centre), -200f32.to_radians() * (1.0 - base), centre, alpha * base);
    let sheet = back_out(step(t, 0.35, 0.5));
    let drop = vec2(0.0, -0.18 * icon * (1.0 - sheet));
    picture(ui, &p[1], scaled(icon_rect.translate(drop), 0.9 + 0.1 * sheet, centre), 0.0, centre, alpha * sheet.min(1.0));
    let lines = cubic_out(step(t, 0.6, 0.55));
    if lines > 0.0 {
        // The sheet's lines and charts sit between 42% and 76% of the icon's width: they draw left to right.
        let clip = Rect::from_min_max(icon_rect.min, pos2(icon_rect.min.x + icon * (0.42 + 0.34 * lines), icon_rect.max.y));
        let mut clipped = ui.new_child(egui::UiBuilder::new().max_rect(icon_rect));
        clipped.set_clip_rect(clip.intersect(ui.clip_rect()));
        picture(&clipped, &p[2], icon_rect, 0.0, centre, alpha * lines);
    }
    let tile = back_out(step(t, 0.8, 0.55));
    // The letter tile's centre on the icon (92, 140 of 256).
    let pivot = vec2(92.0 / 256.0, 140.0 / 256.0);
    let fly = vec2(-0.15 * icon, 0.15 * icon) * (1.0 - tile);
    picture(ui, &p[3], scaled(icon_rect.translate(fly), 0.4 + 0.6 * tile, pivot), -35f32.to_radians() * (1.0 - tile), pivot, alpha * tile.min(1.0));

    if open > 0.0 {
        let x = icon_rect.max.x + open * gap;
        let top = screen.center().y - text_h / 2.0;
        let mut text = ui.new_child(egui::UiBuilder::new().max_rect(screen));
        text.set_clip_rect(Rect::from_min_max(pos2(x, screen.min.y), pos2(x + open * text_w, screen.max.y)).intersect(ui.clip_rect()));
        picture(&text, &p[4], Rect::from_min_size(pos2(x, top), vec2(name_w, name_h)), 0.0, centre, alpha);
        // 3. The company line comes up under the name, starting where the name starts.
        let line = cubic_out(step(t, 2.05, 0.5));
        let rise = 8.0 * (1.0 - line);
        let line_rect = Rect::from_min_size(pos2(x + icon * 0.01, top + name_h + icon * 0.09 + rise), vec2(line_w, line_h));
        picture(&text, &p[5], line_rect, 0.0, centre, alpha * line);
    }
}

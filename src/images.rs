use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Sender;

use crate::api::Api;
use crate::events::Event;
use egui::{pos2, Color32, Context, Rect, TextureHandle, Ui, Vec2};

const MAX_IN_FLIGHT: usize = 6;
static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

// Cards render ~340 logical points wide; on a 150% display that is ~510 physical
// pixels, so anything below ~760 looked soft.
/// Covers are drawn ~490 physical pixels wide on this display and are cropped from the
/// centre, so they need the headroom.
const MAX_EDGE: u32 = 760;
/// Avatars are 34pt circles, so they never needed the cover resolution.
const AVATAR_EDGE: u32 = 160;
const PLACEHOLDER: Color32 = Color32::from_rgb(0x1E, 0x1E, 0x1E);

#[derive(Clone)]
enum Slot {
    Loading,
    Failed,
    Ready(TextureHandle),
}

#[derive(Default)]
pub struct ImageCache {
    slots: HashMap<String, Slot>,
}

impl ImageCache {
    pub fn apply_loaded(&mut self, url: &str, rgba: Option<(usize, usize, Vec<u8>)>, ctx: &Context) {
        match rgba {
            Some((w, h, bytes)) => {
                let img = egui::ColorImage::from_rgba_unmultiplied([w, h], &bytes);
                let tex = ctx.load_texture(url, img, egui::TextureOptions::LINEAR);
                self.slots.insert(url.to_owned(), Slot::Ready(tex));
            }
            None => {
                self.slots.insert(url.to_owned(), Slot::Failed);
            }
        }
    }

    /// A screenful is 30 covers + 30 avatars; letting all of them open a socket and a
    /// thread at once is what made the first second stutter.
    fn request(&mut self, url: &str, ctx: &Context, tx: &Sender<Event>, cap: u32) {
        if IN_FLIGHT.fetch_add(1, Ordering::Relaxed) >= MAX_IN_FLIGHT {
            IN_FLIGHT.fetch_sub(1, Ordering::Relaxed);
            return; // no slot recorded, so the next pass asks again
        }
        self.slots.insert(url.to_owned(), Slot::Loading);
        let key = url.to_owned();
        let tx2 = tx.clone();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let decoded = Api::fetch_image(&key).ok().and_then(|bytes| {
                let img = image::load_from_memory(&bytes).ok()?;
                let longest = img.width().max(img.height());
                let img = if longest > cap {
                    let scale = cap as f32 / longest as f32;
                    img.thumbnail(
                        (img.width() as f32 * scale) as u32,
                        (img.height() as f32 * scale) as u32,
                    )
                } else {
                    img
                };
                let rgba = img.to_rgba8();
                Some((
                    rgba.width() as usize,
                    rgba.height() as usize,
                    rgba.into_raw(),
                ))
            });
            IN_FLIGHT.fetch_sub(1, Ordering::Relaxed);
            let _ = tx2.send(Event::Image {
                url: key,
                result: decoded,
            });
            ctx2.request_repaint();
        });
    }

    fn known(&mut self, url: &str, ctx: &Context, tx: &Sender<Event>, cap: u32) -> Option<TextureHandle> {
        if url.is_empty() {
            return None;
        }
        if !self.slots.contains_key(url) {
            self.request(url, ctx, tx, cap);
        }
        match self.slots.get(url).cloned() {
            Some(Slot::Ready(t)) => Some(t),
            _ => None,
        }
    }

    /// Draws the image into `rect` filling it completely, cropping the overflow
    /// equally from both sides of the axis that is too long (i.e. centre crop).
    pub fn cover(
        &mut self,
        ui: &mut Ui,
        url: &str,
        rect: Rect,
        radius: u8,
        tx: &Sender<Event>,
    ) {
        self.draw_cover(ui, url, rect, radius, tx, MAX_EDGE);
    }

    fn draw_cover(
        &mut self,
        ui: &mut Ui,
        url: &str,
        rect: Rect,
        radius: u8,
        tx: &Sender<Event>,
        cap: u32,
    ) {
        ui.painter().rect_filled(rect, radius, PLACEHOLDER);
        let tex = self.known(url, ui.ctx(), tx, cap);
        if let Some(tex) = tex {
            let [tw, th] = tex.size();
            if tw == 0 || th == 0 {
                return;
            }
            let img_ar = tw as f32 / th as f32;
            let box_ar = (rect.width() / rect.height()).max(0.01);
            let uv = if img_ar > box_ar {
                let inset = (1.0 - box_ar / img_ar) * 0.5;
                Rect::from_min_max(pos2(inset, 0.0), pos2(1.0 - inset, 1.0))
            } else {
                let inset = (1.0 - img_ar / box_ar) * 0.5;
                Rect::from_min_max(pos2(0.0, inset), pos2(1.0, 1.0 - inset))
            };
            paint(ui, &tex, rect, radius, uv);
        }
    }

    /// Circle-cropped avatar.
    pub fn avatar(&mut self, ui: &mut Ui, url: &str, size: f32, tx: &Sender<Event>) {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
        ui.painter()
            .circle_filled(rect.center(), size * 0.5, PLACEHOLDER);
        let radius = (size as u8 / 2).max(1);
        self.draw_cover(ui, url, rect, radius, tx, AVATAR_EDGE);
    }
}

/// `egui::Image::corner_radius` silently drops the draw for some sizes, so rounded
/// textures are painted as a textured rect shape instead.
fn paint(ui: &Ui, tex: &TextureHandle, rect: Rect, radius: u8, uv: Rect) {
    let shape = egui::epaint::RectShape::filled(rect, radius, Color32::WHITE).with_texture(tex.id(), uv);
    ui.painter().add(shape);
}

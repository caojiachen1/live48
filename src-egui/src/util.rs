use egui::{Align2, Color32, Pos2, Rect, RichText, Ui};

use crate::theme as t;

/// Rough advance width: CJK glyphs are full-em, Latin/punctuation about 0.56em.
/// egui 0.35 does not expose a cheap text-measure API outside a layout pass, and
/// these are for overlay pills where pixel-perfection does not matter.
fn char_width(ch: char, size: f32) -> f32 {
    if (ch as u32) > 0x2E80 {
        size
    } else {
        size * 0.56
    }
}

pub fn glyph_width(s: &str, size: f32) -> f32 {
    s.chars().map(|c| char_width(c, size)).sum()
}

pub fn pill_width(text: &str, size: f32) -> f32 {
    glyph_width(text, size) + 12.0
}

/// Draws a rounded label whose top-left corner is `at`. Returns its width.
pub fn pill(ui: &Ui, at: Pos2, text: &str, size: f32, fg: Color32, bg: Color32) -> f32 {
    let w = pill_width(text, size);
    let rect = Rect::from_min_size(at, egui::Vec2::new(w, size + 8.0));
    ui.painter().rect_filled(rect, 4.0, bg);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        egui::FontId::new(size, t::bold()),
        fg,
    );
    w
}

pub fn truncate(text: &str, max_width: f32, size: f32) -> String {
    if glyph_width(text, size) <= max_width {
        return text.to_owned();
    }
    let mut used = 0.0;
    let mut out = String::new();
    for ch in text.chars() {
        let w = char_width(ch, size);
        if used + w > max_width {
            out.push('…');
            return out;
        }
        used += w;
        out.push(ch);
    }
    out
}

pub fn section(ui: &mut Ui, title: &str) {
    ui.add_space(10.0);
    ui.label(
        RichText::new(title)
            .size(11.5)
            .color(t::TEXT_3)
            .font(egui::FontId::new(11.5, t::bold())),
    );
    ui.add_space(5.0);
}

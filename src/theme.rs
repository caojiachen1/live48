use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Stroke,
    TextStyle, Vec2, Visuals,
};
use std::sync::Arc;

// Monochrome Fluent dark ramp.
pub const BG: Color32 = Color32::from_rgb(0x1B, 0x1B, 0x1B);
pub const SURFACE: Color32 = Color32::from_rgb(0x22, 0x22, 0x22);
pub const CARD: Color32 = Color32::from_rgb(0x26, 0x26, 0x26);
pub const HOVER: Color32 = Color32::from_rgb(0x2E, 0x2E, 0x2E);
pub const ACTIVE: Color32 = Color32::from_rgb(0x36, 0x36, 0x36);
pub const FIELD: Color32 = Color32::from_rgb(0x2A, 0x2A, 0x2A);
pub const LINE: Color32 = Color32::from_rgb(0x3F, 0x3F, 0x3F);
pub const LINE_SOFT: Color32 = Color32::from_rgb(0x33, 0x33, 0x33);
pub const TEXT: Color32 = Color32::from_rgb(0xF2, 0xF2, 0xF2);
pub const TEXT_2: Color32 = Color32::from_rgb(0xBE, 0xBE, 0xBE);
pub const TEXT_3: Color32 = Color32::from_rgb(0x8A, 0x8A, 0x8A);
pub const WHITE: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
pub const ON_ACCENT: Color32 = Color32::from_rgb(0x1B, 0x1B, 0x1B);
pub const OK: Color32 = Color32::from_rgb(0x6C, 0xD9, 0x8A);
pub const WARN: Color32 = Color32::from_rgb(0xF2, 0xC5, 0x5B);
pub const ERR: Color32 = Color32::from_rgb(0xF3, 0x7B, 0x7B);

pub const BOLD_NAME: &str = "semibold";

/// `FontFamily::Name` owns an `Arc<str>`, and `heading()` runs for every label in
/// every pass, so the name is interned once instead of re-allocated per call.
fn bold_name() -> &'static Arc<str> {
    static NAME: std::sync::OnceLock<Arc<str>> = std::sync::OnceLock::new();
    NAME.get_or_init(|| Arc::from(BOLD_NAME))
}

pub fn bold() -> FontFamily {
    FontFamily::Name(bold_name().clone())
}

pub fn alpha(c: Color32, a: u8) -> Color32 {
    let [r, g, b, _] = c.to_array();
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    apply_fonts(ctx);
    apply_style(ctx);
    // winit reports one notch as 1.0 "line"; egui's multiplier assumes ~3, so the
    // default feels like crawling on Windows.
    ctx.options_mut(|o| o.input_options.line_scroll_speed = 120.0);
}

fn read_font(paths: &[&str]) -> Option<Vec<u8>> {
    paths.iter().find_map(|p| std::fs::read(p).ok())
}

fn apply_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    let regular = read_font(&[
        "C:/Windows/Fonts/segoeui.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ]);
    let cjk = read_font(&[
        "C:/Windows/Fonts/msyh.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
    ]);
    let bold_regular = read_font(&["C:/Windows/Fonts/segoeuib.ttf"]);
    let cjk_bold = read_font(&["C:/Windows/Fonts/msyhbd.ttc", "C:/Windows/Fonts/simhei.ttf"]);

    let mut insert = |name: &str, data: Option<Vec<u8>>| {
        if let Some(bytes) = data {
            let mut fd = FontData::from_owned(bytes);
            fd.index = 0; // a .ttc collection resolves to its first face
            fonts.font_data.insert(name.to_owned(), Arc::new(fd));
        }
    };
    insert("ui", regular);
    insert("cjk", cjk);
    insert("ui-bold", bold_regular);
    insert("cjk-bold", cjk_bold.clone());

    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        let list = fonts.families.entry(family).or_default();
        for name in ["ui", "cjk"] {
            if fonts.font_data.contains_key(name) && !list.iter().any(|x| x == name) {
                list.push(name.to_owned());
            }
        }
    }

    let mut heavy: Vec<String> = Vec::new();
    if fonts.font_data.contains_key("ui-bold") {
        heavy.push("ui-bold".to_owned());
    }
    if fonts.font_data.contains_key("cjk-bold") {
        heavy.push("cjk-bold".to_owned());
    }
    if heavy.is_empty() {
        if fonts.font_data.contains_key("ui") {
            heavy.push("ui".to_owned());
        }
        if fonts.font_data.contains_key("cjk") {
            heavy.push("cjk".to_owned());
        }
    }
    if !heavy.is_empty() {
        fonts.families.insert(bold(), heavy);
    }

    ctx.set_fonts(fonts);
}

fn apply_style(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.spacing.interact_size = Vec2::new(32.0, 32.0);
        style.spacing.button_padding = Vec2::new(11.0, 5.0);
        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.spacing.window_margin = Margin::same(16);
        style.spacing.menu_margin = Margin::same(10);
        style.spacing.icon_spacing = 6.0;
        style.spacing.icon_width = 16.0;
        style.spacing.indent = 14.0;
        style.spacing.slider_width = 180.0;
        style.spacing.combo_width = 44.0;
        // Six 32pt rows plus the menu padding already overflow egui's 200pt default,
        // which puts a scrollbar in the group/type dropdowns.
        style.spacing.combo_height = 400.0;
        style.animation_time = 0.12;
        // Labels are read-only here; leaving selection on turns the pointer into an
        // I-beam everywhere and costs a hit-test entry per label.
        style.interaction.selectable_labels = false;
        style.visuals = visuals();
        style.visuals.interact_cursor = Some(egui::CursorIcon::PointingHand);

        style
            .text_styles
            .insert(TextStyle::Small, FontId::proportional(12.0));
        style.text_styles.insert(TextStyle::Body, FontId::proportional(14.0));
        style
            .text_styles
            .insert(TextStyle::Button, FontId::new(13.0, bold()));
        style
            .text_styles
            .insert(TextStyle::Heading, FontId::new(20.0, bold()));
        style
            .text_styles
            .insert(TextStyle::Monospace, FontId::monospace(13.0));
    });
}

pub fn heading(size: f32) -> FontId {
    FontId::new(size, bold())
}

pub fn visuals() -> Visuals {
    let mut v = Visuals::dark();
    v.dark_mode = true;
    v.override_text_color = Some(TEXT);
    v.weak_text_color = Some(TEXT_3);
    v.faint_bg_color = SURFACE;
    v.extreme_bg_color = Color32::from_rgb(0x12, 0x12, 0x12);
    v.panel_fill = BG;
    v.window_fill = SURFACE;
    v.window_stroke = Stroke::new(1.0, LINE);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.hyperlink_color = TEXT_2;
    v.selection.bg_fill = Color32::from_rgb(0x4A, 0x4A, 0x4A);
    v.selection.stroke = Stroke::new(1.0, WHITE);

    v.widgets.noninteractive.bg_fill = BG;
    v.widgets.noninteractive.weak_bg_fill = BG;
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_2);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE_SOFT);
    v.widgets.noninteractive.corner_radius = CornerRadius::same(6);

    v.widgets.inactive.bg_fill = FIELD;
    v.widgets.inactive.weak_bg_fill = FIELD;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, LINE);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(0xDD, 0xDD, 0xDD));
    v.widgets.inactive.corner_radius = CornerRadius::same(6);

    v.widgets.hovered.bg_fill = HOVER;
    v.widgets.hovered.weak_bg_fill = HOVER;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x55, 0x55, 0x55));
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, WHITE);
    v.widgets.hovered.corner_radius = CornerRadius::same(6);
    v.widgets.hovered.expansion = 0.5;

    v.widgets.active.bg_fill = ACTIVE;
    v.widgets.active.weak_bg_fill = ACTIVE;
    v.widgets.active.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x66, 0x66, 0x66));
    v.widgets.active.fg_stroke = Stroke::new(1.0, WHITE);
    v.widgets.active.corner_radius = CornerRadius::same(6);
    v.widgets.active.expansion = 1.0;

    v.widgets.open.bg_fill = HOVER;
    v.widgets.open.weak_bg_fill = HOVER;
    v.widgets.open.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x55, 0x55, 0x55));
    v.widgets.open.fg_stroke = Stroke::new(1.0, WHITE);
    v.widgets.open.corner_radius = CornerRadius::same(6);

    v.image_loading_spinners = true;
    v
}

/// Fluent accent button: white fill, black label.
pub fn accent_button(label: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(
        egui::RichText::new(label)
            .color(ON_ACCENT)
            .font(FontId::new(13.0, bold())),
    )
    .fill(WHITE)
    .stroke(Stroke::new(1.0, WHITE))
    .corner_radius(CornerRadius::same(6))
}

/// Outlined secondary button.
pub fn ghost_button(label: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(
        egui::RichText::new(label)
            .color(TEXT_2)
            .font(FontId::new(13.0, bold())),
    )
    .stroke(Stroke::new(1.0, LINE))
    .corner_radius(CornerRadius::same(6))
}

//! The speaker button shared by the main player and the mini window. Hovering
//! opens a draggable volume slider in a foreground layer, so the popup is not
//! clipped by the control bar it belongs to.

use egui::{Align2, Area, Color32, Id, Order, Pos2, Rect, Sense, Ui, Vec2};

use crate::icons::{self, Icon};
use crate::theme as t;

pub struct VolumeButton {
    pub clicked: bool,
    pub volume: Option<f32>,
}

pub fn button(ui: &mut Ui, id: &'static str, volume: f32, size: f32) -> VolumeButton {
    let mut volume = volume.clamp(0.0, 1.0);
    let icon = if volume <= 0.001 {
        Icon::SpeakerMute
    } else {
        Icon::Speaker
    };
    let response = icons::icon_button(ui, icon, size);

    let open_id = Id::new(("volume_open", id));
    let mut open = ui
        .ctx()
        .data_mut(|d| d.get_temp::<bool>(open_id).unwrap_or(false));
    if response.hovered() {
        open = true;
    }

    let mut changed = None;
    if open {
        let screen = ui.ctx().content_rect();
        let panel_w = 146.0;
        let panel_h = 30.0;
        let gap = 6.0;
        let x = (response.rect.max.x - panel_w).clamp(
            screen.min.x + 6.0,
            (screen.max.x - panel_w - 6.0).max(screen.min.x + 6.0),
        );
        let y = (response.rect.min.y - gap - panel_h).max(screen.min.y + 6.0);
        let panel = Rect::from_min_size(Pos2::new(x, y), Vec2::new(panel_w, panel_h));
        let hot_h = (response.rect.min.y - y).max(panel_h);
        let mut drawn = volume;
        let mut dragging = false;
        let mut hovering = false;

        let area = Area::new(Id::new(("volume_popup", id)))
            .order(Order::Foreground)
            .fixed_pos(panel.min)
            .interactable(true)
            .show(ui.ctx(), |ui| {
                let (hot, _) = ui.allocate_exact_size(Vec2::new(panel_w, hot_h), Sense::hover());
                let panel = Rect::from_min_size(hot.min, Vec2::new(panel_w, panel_h));
                let track = Rect::from_min_size(
                    Pos2::new(panel.min.x + 10.0, panel.center().y - 1.5),
                    Vec2::new((panel_w - 54.0).max(40.0), 3.0),
                );
                let slider = ui.interact(
                    panel,
                    Id::new(("volume_slider", id)),
                    Sense::click_and_drag(),
                );
                dragging = slider.dragged();
                hovering = slider.hovered();
                if dragging || slider.clicked() {
                    if let Some(pos) = slider.interact_pointer_pos() {
                        drawn = ((pos.x - track.min.x) / track.width()).clamp(0.0, 1.0);
                        drawn = (drawn * 100.0).round() / 100.0;
                    }
                }

                ui.painter()
                    .rect_filled(panel, 8.0, t::alpha(Color32::from_rgb(8, 8, 8), 235));
                ui.painter().rect_filled(track, 1.5, t::alpha(t::WHITE, 38));
                ui.painter().rect_filled(
                    Rect::from_min_size(
                        track.min,
                        Vec2::new(track.width() * drawn, track.height()),
                    ),
                    1.5,
                    t::WHITE,
                );
                ui.painter().circle_filled(
                    Pos2::new(track.min.x + track.width() * drawn, track.center().y),
                    4.5,
                    t::WHITE,
                );
                ui.painter().text(
                    Pos2::new(panel.max.x - 10.0, panel.center().y),
                    Align2::RIGHT_CENTER,
                    format!("{}%", (drawn * 100.0).round() as i32),
                    egui::FontId::proportional(10.5),
                    t::TEXT_3,
                );
            });

        if area.response.hovered() || hovering || dragging {
            open = true;
        } else if !response.hovered() {
            open = false;
        }
        if (drawn - volume).abs() > 0.001 {
            volume = drawn;
            changed = Some(volume);
        }
    }

    ui.ctx().data_mut(|d| d.insert_temp(open_id, open));
    VolumeButton {
        clicked: response.clicked(),
        volume: changed,
    }
}

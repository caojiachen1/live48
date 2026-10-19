//! Frameless always-on-top mini player, drawn in its own egui viewport window.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use egui::{
    Align, Color32, Context, Frame, Id, Layout, Margin, Pos2, Rect, ResizeDirection, RichText,
    Sense, Stroke, StrokeKind, Ui, UiBuilder, Vec2, ViewportBuilder, ViewportCommand, ViewportId,
};

use crate::icons::{self, Icon};
use crate::player::Player;
use crate::theme as t;

pub const ACTION_NONE: usize = 0;
pub const ACTION_RESTORE: usize = 1;
pub const ACTION_CLOSE: usize = 2;

const BAR_H: f32 = 36.0;
const GRIP: f32 = 26.0;
const MIN_W: f32 = 200.0;
const MIN_H: f32 = 150.0;

pub struct MiniState {
    pub player: Arc<Mutex<Player>>,
    pub action: AtomicUsize,
}

impl MiniState {
    pub fn new(player: Arc<Mutex<Player>>) -> Self {
        MiniState {
            player,
            action: AtomicUsize::new(ACTION_NONE),
        }
    }
}

fn viewport_id() -> ViewportId {
    ViewportId::from_hash_of("mini-player")
}

pub fn show(ctx: &Context, state: &Arc<MiniState>, anchor: Rect) {
    let id = viewport_id();
    let pos = Pos2::new((anchor.max.x - 348.0).max(0.0), (anchor.max.y - 258.0).max(0.0));
    let builder = ViewportBuilder::default()
        .with_title("live48 - 小窗播放")
        .with_inner_size([320.0, 220.0])
        .with_min_inner_size([MIN_W, MIN_H])
        .with_position(pos)
        .with_decorations(false)
        // Keeps WS_SIZEBOX, which is what lets the grip hand the drag to the OS.
        .with_resizable(true)
        .with_always_on_top();

    let shared = state.clone();
    // The window is hidden by `act` when it dismisses itself, and eframe keeps the
    // viewport entry around until the main window prunes it, so a re-opened mini has
    // to be pulled back on screen explicitly.
    ctx.send_viewport_cmd_to(id, ViewportCommand::Visible(true));
    ctx.show_viewport_deferred(id, builder, move |ui, _class| {
        frame(&shared, ui);
    });
}

fn lock(player: &Arc<Mutex<Player>>) -> std::sync::MutexGuard<'_, Player> {
    player.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn frame(state: &Arc<MiniState>, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let viewport = ctx.viewport_id();
    let rect = ui.max_rect();

    // This callback runs both embedded in the main window's pass and standalone. Pin
    // the id path, or every auto-id widget flips identity between the two and a click
    // that starts in one pass never finishes in the other.
    let layout = *ui.layout();
    let mut pinned = ui.new_child(
        UiBuilder::new()
            .id_salt("mini-frame")
            .max_rect(rect)
            .layout(layout),
    );
    let ui = &mut pinned;

    lock(&state.player).pump(&ctx, viewport);

    ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
    ui.painter().rect_stroke(
        rect.shrink(0.5),
        0.0,
        Stroke::new(1.0, t::alpha(t::WHITE, 56)),
        StrokeKind::Inside,
    );

    let bar_top = rect.max.y - BAR_H;
    let video = Rect::from_min_max(rect.min, Pos2::new(rect.max.x, bar_top));

    // Everything but the top-left grip drags the window; double-click restores it.
    let drag = ui
        .interact(
            Rect::from_min_max(Pos2::new(rect.min.x + GRIP, video.min.y), video.max),
            Id::new("mini-drag"),
            Sense::click_and_drag(),
        )
        .on_hover_and_drag_cursor(egui::CursorIcon::Grab);
    if drag.drag_started() {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }
    if drag.double_clicked() {
        act(state, &ctx, ACTION_RESTORE);
    }

    let (aspect, started, tex) = {
        let p = lock(&state.player);
        (p.aspect(), p.is_started(), p.texture_id())
    };
    if started {
        if let Some(tex) = tex {
            let size = crate::player::fit(video.size(), aspect);
            let dest = Rect::from_center_size(video.center(), size);
            let shape =
                egui::epaint::RectShape::filled(dest, 0.0, Color32::WHITE).with_texture(
                    tex.0,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                );
            ui.painter().add(shape);
        }
    } else {
        ui.put(
            Rect::from_center_size(video.center(), Vec2::splat(34.0)),
            egui::Spinner::new().size(24.0),
        );
    }

    // Top-left grip. Hand the drag over to the OS: it tracks the pointer exactly and
    // repaints as it goes, which per-frame `InnerSize`/`OuterPosition` commands never
    // did (they lag a frame, drift, and rebuild the surface every move).
    let grip_rect = Rect::from_min_size(rect.min, Vec2::splat(GRIP));
    let grip = ui
        .interact(grip_rect, Id::new("mini-grip"), Sense::drag())
        .on_hover_and_drag_cursor(egui::CursorIcon::ResizeNwSe);
    if grip.drag_started() {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(ResizeDirection::NorthWest));
    }
    draw_grip(
        ui,
        grip_rect,
        if grip.hovered() || grip.dragged() {
            t::alpha(t::WHITE, 220)
        } else {
            t::alpha(t::WHITE, 90)
        },
    );

    control_bar(state, &ctx, ui, Rect::from_min_max(Pos2::new(rect.min.x, bar_top), rect.max));
}

fn control_bar(state: &Arc<MiniState>, ctx: &Context, ui: &mut Ui, bar: Rect) {
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(bar)
            .id_salt("mini-bar")
            .layout(Layout::top_down(Align::Min)),
    );
    Frame::new()
        .fill(t::alpha(Color32::from_rgb(14, 14, 14), 220))
        .corner_radius(0.0)
        .inner_margin(Margin {
            left: 10,
            right: 6,
            top: 3,
            bottom: 3,
        })
        .show(&mut child, |ui| {
            let (title, nickname, paused, muted, err) = {
                let p = lock(&state.player);
                (
                    p.title.clone(),
                    p.nickname.clone(),
                    p.paused,
                    p.muted,
                    p.error.clone(),
                )
            };
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let text_w = (bar.width() - 140.0).max(40.0);
                    ui.set_width(text_w);
                    ui.label(
                        RichText::new(crate::util::truncate(
                            &err.unwrap_or(title),
                            text_w,
                            11.5,
                        ))
                        .color(t::TEXT)
                        .size(11.5),
                    );
                    if !nickname.is_empty() {
                        ui.label(RichText::new(nickname).size(10.0).color(t::TEXT_3));
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let close = icons::icon_button(ui, Icon::Close, 26.0).on_hover_text("关闭小窗");
                    let restore = icons::icon_button(ui, Icon::PipExit, 26.0)
                        .on_hover_text("回到主窗口播放");
                    let sound = if muted {
                        Icon::SpeakerMute
                    } else {
                        Icon::Speaker
                    };
                    let mute = icons::icon_button(ui, sound, 26.0)
                        .on_hover_text(if muted { "取消静音" } else { "静音" });
                    let play = icons::icon_button(
                        ui,
                        if paused { Icon::Play } else { Icon::Pause },
                        26.0,
                    )
                    .on_hover_text(if paused { "播放" } else { "暂停" });

                    if close.clicked() {
                        act(state, ctx, ACTION_CLOSE);
                    }
                    if restore.clicked() {
                        act(state, ctx, ACTION_RESTORE);
                    }
                    if mute.clicked() {
                        lock(&state.player).toggle_mute();
                        ctx.request_repaint();
                    }
                    if play.clicked() {
                        lock(&state.player).toggle_pause();
                        ctx.request_repaint();
                    }
                });
            });
        });
}

/// The little window tears itself down without waiting for the main one: a hidden
/// window never gets a paint from Windows, so any command left for the main window's
/// next pass could sit there forever. It hides its own window (which *is* applied by
/// its own pass) and un-hides the main one; the main window then drops the leftover
/// state - and destroys this viewport - the first time it runs again.
fn act(state: &Arc<MiniState>, ctx: &Context, action: usize) {
    state.action.store(action, Ordering::Release);
    if action == ACTION_CLOSE {
        // silence it now: the player is only released once the main window prunes us
        let mut p = lock(&state.player);
        if !p.paused {
            p.toggle_pause();
        }
        if !p.muted {
            p.toggle_mute();
        }
    }
    let root = ViewportId::ROOT;
    ctx.send_viewport_cmd_to(root, ViewportCommand::Visible(true));
    ctx.send_viewport_cmd_to(root, ViewportCommand::Focus);
    ctx.send_viewport_cmd(ViewportCommand::Visible(false));
}

fn draw_grip(ui: &Ui, zone: Rect, color: Color32) {
    let origin = zone.min + Vec2::new(5.0, 5.0);
    for i in 0..3 {
        let off = i as f32 * 5.0;
        ui.painter().line_segment(
            [
                Pos2::new(origin.x + off, origin.y),
                Pos2::new(origin.x, origin.y + off),
            ],
            Stroke::new(1.4, color),
        );
    }
}

pub fn take_action(state: &Arc<MiniState>) -> usize {
    state.action.swap(ACTION_NONE, Ordering::AcqRel)
}

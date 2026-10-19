//! The in-window player: libVLC frames uploaded as an egui texture, plus a
//! monochrome Fluent control chrome.

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use egui::{
    Align, Align2, Color32, ColorImage, Context, Direction, Frame, Id, Margin, Pos2, Rect, RichText,
    Sense, Stroke, TextureId, Ui, UiBuilder, Vec2, ViewportId,
};

use crate::events::Event;
use crate::icons::{self, Icon};
use crate::images::ImageCache;
use crate::theme as t;
use crate::vlc::{Engine, Session};

/// Ceiling for what libVLC converts on the CPU; egui's sampler scales from here.
pub const MAX_W: u32 = 1280;
pub const MAX_H: u32 = 720;

#[derive(Clone)]
pub struct Entry {
    pub live_id: String,
    pub title: String,
    pub nickname: String,
    pub cover: String,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Cmd {
    None,
    Close,
    Mini,
    Refresh,
    Prev,
    Next,
    Goto(usize),
}

pub struct Player {
    pub session: Session,
    pub entries: Vec<Entry>,
    pub index: usize,
    pub is_live: bool,
    pub title: String,
    pub nickname: String,

    texture: Option<TextureHandle>,
    staging: Vec<u8>,
    last_seq: u64,
    frame_size: (usize, usize),

    pub paused: bool,
    pub muted: bool,
    started: bool,
    buffering: bool,
    pub error: Option<String>,
    time_ms: i64,
    len_ms: i64,

    controls: bool,
    hide_at: Instant,
    pub playlist_open: bool,
    /// Slide progress of the playlist drawer, and the window it occupies so the wheel
    /// can scroll the list instead of jumping tracks.
    pub fly_t: f32,
    pub fly_prev: f64,
    pub playlist_rect: Rect,
    pub cmd: Cmd,
    seek_preview: Option<f32>,

    /// Counters for the headless `--apitest` harness.
    pub uploads: u64,
    pub upload_micros: u128,
}

use egui::TextureHandle;

impl Player {
    pub fn open(
        engine: &Engine,
        url: &str,
        entries: Vec<Entry>,
        index: usize,
        is_live: bool,
        ctx: &Context,
    ) -> Result<Player, String> {
        let session = engine.open(url, MAX_W, MAX_H)?;
        let title = entries
            .get(index)
            .map(|e| e.title.clone())
            .unwrap_or_default();
        let nickname = entries.get(index).map(|e| e.nickname.clone()).unwrap_or_default();
        Ok(Player {
            session,
            entries,
            index,
            is_live,
            title: if title.is_empty() { "无标题".into() } else { title },
            nickname,
            texture: Some(ctx.load_texture(
                "video-frame",
                ColorImage::from_rgb([2, 2], &[0u8; 12]),
                LINEAR,
            )),
            staging: Vec::new(),
            last_seq: 0,
            frame_size: (16, 9),
            paused: false,
            muted: false,
            started: false,
            buffering: true,
            error: None,
            time_ms: 0,
            len_ms: 0,
            controls: true,
            hide_at: Instant::now() + Duration::from_secs(3),
            playlist_open: false,
            fly_t: 0.0,
            fly_prev: 0.0,
            playlist_rect: Rect::NOTHING,
            cmd: Cmd::None,
            seek_preview: None,
            uploads: 0,
            upload_micros: 0,
        })
    }

    /// Swaps in a new stream while keeping the playlist and chrome state.
    pub fn reattach(&mut self, engine: &Engine, url: &str, index: usize) -> Result<(), String> {
        let session = engine.open(url, MAX_W, MAX_H)?;
        self.session = session;
        self.index = index;
        if let Some(e) = self.entries.get(index) {
            self.title = if e.title.is_empty() {
                "无标题".into()
            } else {
                e.title.clone()
            };
            self.nickname = e.nickname.clone();
        }
        self.started = false;
        self.buffering = true;
        self.error = None;
        self.last_seq = 0;
        self.paused = false;
        Ok(())
    }

    /// Uploads the newest decoded frame and keeps repaints flowing at video rate.
    pub fn pump(&mut self, ctx: &Context, viewport: ViewportId) {
        let frames = self.session.frames();
        let seq = frames.sequence();
        if seq != self.last_seq {
            self.last_seq = seq;
            let t0 = Instant::now();
            if frames.take_rgba(&mut self.staging) {
                self.uploads += 1;
                self.upload_micros += t0.elapsed().as_micros();
                if let Some((w, h)) = frames.size() {
                    self.frame_size = (w, h);
                    let img = ColorImage::from_rgba_unmultiplied([w, h], &self.staging);
                    match self.texture.as_mut() {
                        Some(tex) => tex.set(img, LINEAR),
                        None => {
                            self.texture = Some(ctx.load_texture("video-frame", img, LINEAR))
                        }
                    }
                    if !self.started {
                        self.started = true;
                        self.buffering = false;
                    }
                }
            }
        }

        match self.session.vlc_state() {
            1 | 2 | 8 => self.buffering = !self.started,
            3 => {
                self.buffering = false;
                self.error = None;
            }
            4 => {
                self.buffering = false;
                self.paused = true;
            }
            6 => self.error = Some("流已结束".to_owned()),
            7 => self.error = Some("VLC 无法播放该地址（解码或网络错误）".to_owned()),
            _ => {}
        }

        self.time_ms = self.session.time_ms().max(0);
        self.len_ms = self.session.length_ms().max(0);

        let every = if self.started { 16 } else { 80 };
        ctx.request_repaint_after_for(Duration::from_millis(every), viewport);
    }

    pub fn controls_visible(&self) -> bool {
        self.controls
    }

    pub fn aspect(&self) -> f32 {
        (self.frame_size.0 as f32 / self.frame_size.1 as f32).max(0.2)
    }

    pub fn is_started(&self) -> bool {
        self.started
    }

    pub fn texture_id(&self) -> Option<(TextureId, Vec2)> {
        self.texture.as_ref().map(|t| (t.id(), t.size_vec2()))
    }

    pub fn toggle_pause(&mut self) {
        self.paused = !self.paused;
        self.session.set_paused(self.paused);
    }

    pub fn toggle_mute(&mut self) {
        self.muted = !self.muted;
        self.session.set_muted(self.muted);
    }

    fn wake(&mut self) {
        self.controls = true;
        self.hide_at = Instant::now() + Duration::from_secs(3);
    }

    pub fn settle(&mut self) {
        if self.controls && Instant::now() >= self.hide_after_now() && !self.playlist_open {
            self.controls = false;
        }
    }

    fn hide_after_now(&self) -> Instant {
        self.hide_at
    }

    // ------------------------------------------------------------------ bars

    pub fn top_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if icons::icon_button(ui, Icon::Close, 32.0)
                .on_hover_text("关闭播放器")
                .clicked()
            {
                self.cmd = Cmd::Close;
            }
            if icons::icon_button(ui, Icon::Pip, 32.0)
                .on_hover_text("小窗播放")
                .clicked()
            {
                self.cmd = Cmd::Mini;
            }
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(
                    RichText::new(crate::util::truncate(&self.title, 430.0, 14.5))
                        .color(t::TEXT)
                        .font(t::heading(14.5)),
                );
                ui.label(RichText::new(&self.nickname).size(11.5).color(t::TEXT_3));
            });
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                if self.entries.len() > 1
                    && icons::icon_button(ui, Icon::List, 32.0)
                        .on_hover_text("播放列表")
                        .clicked()
                {
                    self.playlist_open = !self.playlist_open;
                    self.wake();
                }
                if self.is_live
                    && icons::icon_button(ui, Icon::Refresh, 32.0)
                        .on_hover_text("刷新直播流")
                        .clicked()
                {
                    self.cmd = Cmd::Refresh;
                }
            });
        });
    }

    pub fn bottom_bar(&mut self, ui: &mut Ui) {
        let seekable = !self.is_live && self.len_ms > 0;
        if seekable {
            self.seek_row(ui);
        } else {
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::centered_and_justified(Direction::TopDown), |ui| {
                    let pulse = (ui.ctx().input(|i| i.time).fract() * 2.0).sin();
                    let a = (150.0 + pulse * 105.0).clamp(0.0, 255.0) as u8;
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(64.0, 18.0), Sense::hover());
                    ui.painter().circle_filled(
                        Pos2::new(rect.min.x + 6.0, rect.center().y),
                        4.0,
                        Color32::from_rgba_unmultiplied(255, 255, 255, a),
                    );
                    ui.painter().text(
                        Pos2::new(rect.min.x + 17.0, rect.center().y),
                        Align2::LEFT_CENTER,
                        "LIVE",
                        t::heading(11.5),
                        t::TEXT_2,
                    );
                });
            });
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let icon = if self.paused { Icon::Play } else { Icon::Pause };
            if icons::icon_button(ui, icon, 32.0)
                .on_hover_text(if self.paused { "播放" } else { "暂停" })
                .clicked()
            {
                self.toggle_pause();
            }
            let sound = if self.muted {
                Icon::SpeakerMute
            } else {
                Icon::Speaker
            };
            if icons::icon_button(ui, sound, 32.0)
                .on_hover_text(if self.muted { "取消静音" } else { "静音" })
                .clicked()
            {
                self.toggle_mute();
            }
            ui.label(
                RichText::new(fmt_time(self.time_ms as f64 / 1000.0))
                    .size(12.0)
                    .color(t::TEXT_2),
            );
            if seekable {
                ui.label(RichText::new("/").size(12.0).color(t::TEXT_3));
                ui.label(
                    RichText::new(fmt_time(self.len_ms as f64 / 1000.0))
                        .size(12.0)
                        .color(t::TEXT_3),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{} × {}", self.frame_size.0, self.frame_size.1))
                        .size(11.0)
                        .color(t::TEXT_3),
                );
            });
        });
    }

    fn seek_row(&mut self, ui: &mut Ui) {
        let width = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 18.0), Sense::click_and_drag());
        let held = (self.time_ms as f32 / self.len_ms as f32).clamp(0.0, 1.0);
        let frac = self.seek_preview.unwrap_or(held);

        if resp.dragged() {
            if let Some(p) = resp.interact_pointer_pos() {
                self.seek_preview = Some(((p.x - rect.min.x) / rect.width()).clamp(0.0, 1.0));
            }
        }
        // A plain click never goes through `dragged()`, so the preview would stay
        // empty and the seek would be dropped.
        let jump = if resp.drag_stopped() {
            self.seek_preview.take()
        } else if resp.clicked() {
            resp.interact_pointer_pos()
                .map(|p| ((p.x - rect.min.x) / rect.width()).clamp(0.0, 1.0))
        } else {
            None
        };
        if let Some(f) = jump {
            self.session.set_time_ms((f as f64 * self.len_ms as f64) as i64);
        }
        if resp.hovered() {
            self.wake();
        }

        let rail = Rect::from_min_size(Pos2::new(rect.min.x, rect.center().y - 2.0), Vec2::new(rect.width(), 4.0));
        ui.painter().rect_filled(rail, 2.0, t::LINE_SOFT);
        ui.painter().rect_filled(
            Rect::from_min_size(rail.min, Vec2::new(rail.width() * frac, 4.0)),
            2.0,
            t::WHITE,
        );
        let big = resp.dragged() || resp.hovered();
        ui.painter().circle_filled(
            Pos2::new(rail.min.x + rail.width() * frac, rail.center().y),
            if big { 6.5 } else { 4.5 },
            t::WHITE,
        );
    }

    // ----------------------------------------------------------------- video

    pub fn video(&mut self, ui: &mut Ui) {
        let rect = ui.available_rect_before_wrap();
        let resp = ui
            .interact(rect, Id::new("video-surface"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        ui.painter().rect_filled(rect, 0.0, Color32::BLACK);

        let ar = self.aspect();
        let dest = Rect::from_center_size(rect.center(), fit(rect.size(), ar));
        if self.started {
            if let Some(tex) = &self.texture {
                egui::Image::from_texture((tex.id(), tex.size_vec2())).paint_at(ui, dest);
            }
        }

        if resp.clicked() {
            if self.controls {
                self.controls = false;
            } else {
                self.wake();
            }
        }
        if resp.hovered() {
            self.wake();
        }

        if self.controls && self.started && self.error.is_none() {
            let hot = Rect::from_center_size(rect.center(), Vec2::splat(72.0));
            // Only under the pointer - a permanent disc in the middle of the picture
            // hides what is actually happening on screen.
            let hovered = ui
                .ctx()
                .input(|i| i.pointer.hover_pos())
                .is_some_and(|p| hot.contains(p));
            if hovered {
                let mut child = ui.new_child(
                    UiBuilder::new()
                        .max_rect(hot)
                        .layout(egui::Layout::centered_and_justified(Direction::TopDown)),
                );
                let icon = if self.paused { Icon::Play } else { Icon::Pause };
                if icons::overlay_icon(&mut child, icon, 72.0).clicked() {
                    self.toggle_pause();
                }
            }
        }

        if self.buffering && !self.started && self.error.is_none() {
            ui.put(
                Rect::from_center_size(rect.center(), Vec2::splat(40.0)),
                egui::Spinner::new().size(30.0),
            );
        }

        if let Some(err) = self.error.clone() {
            let mut child = ui.new_child(
                UiBuilder::new()
                    .max_rect(Rect::from_center_size(rect.center(), Vec2::new(420.0, 140.0))),
            );
            Frame::new()
                .fill(t::alpha(Color32::from_rgb(24, 24, 24), 235))
                .corner_radius(10.0)
                .inner_margin(Margin::same(16))
                .stroke(Stroke::new(1.0, t::LINE))
                .show(&mut child, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("无法播放").font(t::heading(15.0)).color(t::ERR));
                        ui.add_space(6.0);
                        ui.label(RichText::new(err).size(12.5).color(t::TEXT_2));
                        ui.add_space(12.0);
                        if ui
                            .add_sized(Vec2::new(100.0, 28.0), t::ghost_button("关闭"))
                            .clicked()
                        {
                            self.cmd = Cmd::Close;
                        }
                    });
                });
        }
    }

    // -------------------------------------------------------------- playlist

    /// Body of the playlist drawer; `ui::flyout` supplies the backdrop, the frame and
    /// the slide, so this only paints the rows inside the given width.
    pub fn playlist_body(&mut self, ui: &mut Ui, thumbs: &mut ImageCache, tx: &Sender<Event>) {
        ui.spacing_mut().item_spacing.y = 0.0;
        egui::ScrollArea::vertical()
            .id_salt("pl")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let current = self.index;
                let rows: Vec<(usize, Entry)> = self.entries.iter().cloned().enumerate().collect();
                for (idx, e) in rows {
                    let active = idx == current;
                    // A Frame shrinks to its content, which would make the highlight
                    // stop short of the panel edge.
                    let row_w = ui.available_width();
                    let row = Frame::new()
                        .fill(if active {
                            t::alpha(t::WHITE, 28)
                        } else {
                            Color32::TRANSPARENT
                        })
                        .inner_margin(Margin {
                            left: 12,
                            right: 12,
                            top: 8,
                            bottom: 8,
                        })
                        .corner_radius(0.0)
                        .show(ui, |ui| {
                            ui.set_width(row_w - 24.0);
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 10.0;
                                let crect = Rect::from_min_size(ui.cursor().min, Vec2::splat(40.0));
                                ui.allocate_at_least(Vec2::splat(40.0), Sense::hover());
                                thumbs.cover(ui, &e.cover, crect, 4, tx);
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    ui.label(
                                        RichText::new(crate::util::truncate(
                                            if e.title.is_empty() {
                                                "无标题"
                                            } else {
                                                &e.title
                                            },
                                            row_w - 100.0,
                                            12.5,
                                        ))
                                        .color(if active { t::WHITE } else { t::TEXT })
                                        .size(12.5),
                                    );
                                    ui.label(
                                        RichText::new(crate::util::truncate(
                                            &e.nickname,
                                            row_w - 100.0,
                                            11.0,
                                        ))
                                        .color(t::TEXT_3)
                                        .size(11.0),
                                    );
                                });
                            });
                        });
                    let resp = ui
                        .interact(row.response.rect, Id::new(("pl", idx)), Sense::click())
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    if resp.hovered() && !active {
                        ui.painter().rect_filled(
                            row.response.rect,
                            0.0,
                            t::alpha(t::WHITE, 12),
                        );
                    }
                    if resp.clicked() && !active {
                        self.cmd = Cmd::Goto(idx);
                    }
                }
            });
    }
}

const LINEAR: egui::TextureOptions = egui::TextureOptions::LINEAR;

pub fn fit(avail: Vec2, ar: f32) -> Vec2 {
    if avail.x <= 0.0 || avail.y <= 0.0 {
        return Vec2::ZERO;
    }
    let by_w = Vec2::new(avail.x, avail.x / ar);
    if by_w.y <= avail.y {
        by_w
    } else {
        Vec2::new(avail.y * ar, avail.y)
    }
}

pub fn fmt_time(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

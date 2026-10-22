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
use crate::models::{Danmu, Source, Stream};
use crate::theme as t;
use crate::vlc::{Engine, Session};

/// Ceiling for what libVLC converts on the CPU; egui's sampler scales from here.
pub const MAX_W: u32 = 1280;
pub const MAX_H: u32 = 720;

/// A record's danmaku stays on screen for this long, matching the original client.
pub const DANMU_WINDOW: f64 = 3.0;

#[derive(Clone)]
pub struct Entry {
    pub live_id: String,
    pub title: String,
    pub nickname: String,
    pub cover: String,
    pub source: Source,
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
    /// Switch清晰度: the media (and therefore its sout) is rebuilt from scratch.
    Quality(i64),
    /// Start or stop capturing the stream that is on screen.
    Record,
    SaveDanmu,
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

    /// Clearness menu for this item; empty until the room info comes back.
    pub streams: Vec<Stream>,
    /// The `streamType` in effect; `0` means the automatic choice.
    pub quality: i64,
    pub danmu_url: Option<String>,
    pub danmu: Vec<Danmu>,
    /// The `.lrc` is fetched once per item, not once per frame.
    pub danmu_requested: bool,
    /// The drawer shows the playlist (false) or the danmaku (true).
    pub show_danmu: bool,
    /// Mirrored from the app every pass so the button can show the truth.
    pub recording: bool,
    danmu_cur: usize,

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
            streams: Vec::new(),
            quality: 0,
            danmu_url: None,
            danmu: Vec::new(),
            danmu_requested: false,
            show_danmu: false,
            recording: false,
            danmu_cur: 0,
            uploads: 0,
            upload_micros: 0,
        })
    }

    /// Swaps in a new stream while keeping the playlist and chrome state.
    pub fn reattach(&mut self, engine: &Engine, url: &str, index: usize) -> Result<(), String> {
        let session = engine.open(url, MAX_W, MAX_H)?;
        self.session = session;
        self.index = index;
        self.streams = Vec::new();
        self.danmu_url = None;
        self.danmu = Vec::new();
        self.danmu_requested = false;
        self.danmu_cur = 0;
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

    /// `want` is the清晰度 this item already uses, so a reattach does not silently
    /// drop back to automatic.
    pub fn set_streams(&mut self, streams: Vec<Stream>, want: Option<i64>) {
        self.quality = want.unwrap_or_else(|| {
            for t in [3i64, 1, 2] {
                if streams.iter().any(|s| s.stream_type == t) {
                    return t;
                }
            }
            streams.first().map(|s| s.stream_type).unwrap_or(0)
        });
        self.streams = streams;
    }

    pub fn set_danmu(&mut self, items: Vec<Danmu>) {
        self.danmu_cur = 0;
        self.danmu = items;
    }

    /// The last danmaku at or before the play head - what the panel highlights.
    fn danmu_cursor(&self, now: f64) -> usize {
        match self.danmu.binary_search_by(|d| d.secs.total_cmp(&now)) {
            Ok(i) => i,
            Err(0) => usize::MAX,
            Err(i) => i - 1,
        }
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
                        .on_hover_text("播放列表 / 弹幕")
                        .clicked()
                {
                    self.playlist_open = !self.playlist_open;
                    self.wake();
                }
                let rec_label = if self.recording { "停止录制" } else { "录制" };
                let rec_btn = if self.recording {
                    t::accent_button(rec_label)
                } else {
                    t::ghost_button(rec_label)
                };
                if ui
                    .add_sized(
                        Vec2::new(if self.recording { 76.0 } else { 52.0 }, 32.0),
                        rec_btn,
                    )
                    .on_hover_text(if self.recording {
                        "停止录制并保留已录内容"
                    } else {
                        "边看边录成 TS 文件，断流后会自动续录"
                    })
                    .clicked()
                {
                    self.cmd = Cmd::Record;
                }
                if self.streams.len() > 1 {
                    let current = self
                        .streams
                        .iter()
                        .find(|s| s.stream_type == self.quality)
                        .map(|s| s.name.clone())
                        .unwrap_or_else(|| "清晰度".to_owned());
                    let options = self.streams.clone();
                    egui::ComboBox::from_id_salt("quality_combo")
                        .width(104.0)
                        .selected_text(RichText::new(current).size(12.5).color(t::TEXT_2))
                        .show_ui(ui, |ui| {
                            for s in options {
                                if ui
                                    .selectable_value(
                                        &mut self.quality,
                                        s.stream_type,
                                        if s.vip {
                                            format!("{}（VIP）", s.name)
                                        } else {
                                            s.name.clone()
                                        },
                                    )
                                    .changed()
                                {
                                    self.cmd = Cmd::Quality(s.stream_type);
                                }
                            }
                        });
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
        let floor = ui.spacing().interact_size.y;
        ui.spacing_mut().interact_size.y = 26.0;
        ui.spacing_mut().button_padding.y = 3.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for (value, label) in [(false, "列表"), (true, "弹幕")] {
                let selected = self.show_danmu == value;
                let btn = egui::Button::new(
                    RichText::new(label)
                        .size(12.5)
                        .color(if selected { t::ON_ACCENT } else { t::TEXT_2 })
                        .font(t::heading(12.5)),
                )
                .fill(if selected { t::WHITE } else { Color32::TRANSPARENT })
                .corner_radius(7.0);
                if ui.add_sized(Vec2::new(52.0, 26.0), btn).clicked() {
                    self.show_danmu = value;
                    self.wake();
                }
            }
            if self.show_danmu && !self.danmu.is_empty() {
                if ui
                    .add_sized(Vec2::new(70.0, 26.0), t::ghost_button("保存弹幕"))
                    .on_hover_text("把这条录播的 .lrc 存到下载目录")
                    .clicked()
                {
                    self.cmd = Cmd::SaveDanmu;
                }
            }
            if self.show_danmu {
                ui.label(
                    RichText::new(format!("{} 条", self.danmu.len())).size(11.0).color(t::TEXT_3),
                );
            }
        });
        ui.spacing_mut().interact_size.y = floor;
        ui.add_space(6.0);

        if self.show_danmu {
            self.danmu_body(ui);
            return;
        }

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

    /// The record's danmaku as a scrolling list: the line the play head is on stays
    /// highlighted and the view follows it, and clicking any line jumps the video there.
    fn danmu_body(&mut self, ui: &mut Ui) {
        if self.danmu.is_empty() {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(if self.is_live {
                        "直播没有弹幕文件，只有录播有"
                    } else {
                        "这条录播没有弹幕文件"
                    })
                    .size(12.5)
                    .color(t::TEXT_3),
                );
            });
            return;
        }
        let row_h = 24.0;
        let now = self.time_ms as f64 / 1000.0;
        let cur = self.danmu_cursor(now);
        let follow = cur != usize::MAX && cur != self.danmu_cur;
        if cur != usize::MAX {
            self.danmu_cur = cur;
        }
        let mut seek: Option<f64> = None;
        let jump_to = if follow { Some(self.danmu_cur) } else { None };
        egui::ScrollArea::vertical()
            .id_salt("danmu")
            .auto_shrink([false, false])
            .show_rows(ui, row_h, self.danmu.len(), |ui, range| {
                if let Some(row) = jump_to {
                    // Virtualised rows still occupy exact-height spacers, so the
                    // target rect is simply `row * row_height`.
                    ui.scroll_to_rect(
                        Rect::from_min_size(
                            Pos2::new(0.0, row as f32 * row_h),
                            Vec2::new(1.0, row_h),
                        ),
                        Some(Align::Center),
                    );
                }
                for (n, d) in self.danmu[range.clone()].iter().enumerate() {
                    let row = range.start + n;
                    let live = d.secs <= now && now - d.secs < DANMU_WINDOW;
                    let w = ui.available_width();
                    let frame = Frame::new()
                        .fill(if live {
                            t::alpha(t::WHITE, 26)
                        } else {
                            Color32::TRANSPARENT
                        })
                        .inner_margin(Margin {
                            left: 10,
                            right: 10,
                            top: 3,
                            bottom: 3,
                        })
                        .show(ui, |ui| {
                            ui.set_width(w - 20.0);
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                ui.label(
                                    RichText::new(&d.time)
                                        .size(11.0)
                                        .color(if live { t::WHITE } else { t::TEXT_3 }),
                                );
                                ui.label(
                                    RichText::new(crate::util::truncate(
                                        &d.nickname,
                                        96.0,
                                        12.0,
                                    ))
                                    .size(12.0)
                                    .color(if live { t::WHITE } else { t::TEXT_2 }),
                                );
                                ui.label(
                                    RichText::new(crate::util::truncate(&d.message, 120.0, 12.0))
                                        .size(12.0)
                                        .color(t::TEXT),
                                );
                            });
                        });
                    let resp = ui
                        .interact(
                            frame.response.rect,
                            Id::new(("danmu", row)),
                            Sense::click(),
                        )
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    if resp.clicked() {
                        seek = Some(d.secs);
                    }
                }
            });
        if let Some(secs) = seek {
            self.session.set_time_ms((secs * 1000.0) as i64);
            self.wake();
        }
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

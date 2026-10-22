use std::cell::Cell;

use chrono::{Datelike, NaiveDate};
use egui::{
    Align, Align2, Area, Color32, Context, Frame, Id, Key, Layout, Margin, Panel, Pos2, Rect,
    RichText, Sense, Stroke, StrokeKind, Ui, UiBuilder, Vec2,
};

use crate::api;
use crate::app::{App, Fly, PlayerMode, Tab};
use crate::config;
use crate::events::{Action, ToastKind};
use crate::icons::{self, Icon};
use crate::player::Cmd;
use crate::models::{badge_text, LiveItem};
use crate::theme as t;
use crate::util;

const GROUPS_LIST: [&str; 5] = ["SNH48", "GNZ48", "BEJ48", "CKG48", "CGT48"];
const TYPE_LIST: [&str; 5] = ["视频", "电台", "连麦", "游戏", "AI"];

impl eframe::App for App {
    fn logic(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        let _perf = crate::perf::Guard::new("logic");
        self.drain_events();
        self.tick_toasts();
        self.tick_timers();

        if self.player.is_some() {
            // Mirrors the web player's gestures: wheel and arrow keys switch
            // between neighbours, Esc tears the overlay down.
            let mut cmd = Cmd::None;
            ctx.input(|i| {
                // Count wheel notches straight from the events: egui smoothes the
                // scroll delta over several frames, which would turn one notch into
                // several track switches.
                let mut notches = 0.0;
                let mut over_list = false;
                for ev in &i.raw.events {
                    if let egui::Event::MouseWheel { unit, delta, .. } = ev {
                        notches += match unit {
                            egui::MouseWheelUnit::Line => delta.y,
                            egui::MouseWheelUnit::Point => delta.y / 40.0,
                            egui::MouseWheelUnit::Page => delta.y * 3.0,
                        };
                        // ...except while the pointer is inside the playlist, where the
                        // wheel belongs to that scroll area.
                        over_list = i
                            .pointer
                            .hover_pos()
                            .is_some_and(|p| self.playlist_hit().contains(p));
                    }
                }
                if i.key_pressed(Key::Escape) {
                    cmd = Cmd::Close;
                } else if i.key_pressed(Key::ArrowDown) {
                    cmd = Cmd::Next;
                } else if i.key_pressed(Key::ArrowUp) {
                    cmd = Cmd::Prev;
                } else if notches.abs() >= 1.0 && !over_list {
                    cmd = if notches < 0.0 { Cmd::Next } else { Cmd::Prev };
                }
            });
            if cmd != Cmd::None {
                self.handle_cmd(cmd);
            }
            return;
        }

        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.filter_open = false;
            self.settings_open = false;
        }

    }

    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let _perf = crate::perf::Guard::new("ui");

        if !self.chrome_done {
            self.chrome_done = true;
            // with_maximized on the builder is ignored once an inner_size is set,
            // so ask for it as a command on the first real pass.
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
            #[cfg(windows)]
            crate::win::tune(frame);
        }

        self.mini_tick(&ctx);

        if self.player.is_some() && self.mini.is_none() {
            self.player_view(ui);
            self.paint_toasts(&ctx);
            return;
        }

        self.header(ui);
        self.toolbar(ui);
        self.status_bar(ui);

        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(t::BG)
                    .inner_margin(Margin::symmetric(18, 10)),
            )
            .show(ui, |ui| self.list(ui));

        self.drawer(ui);

        self.paint_toasts(ui.ctx());
    }
}

impl App {
    /// The settings/filter drawer, painted last so it floats over the list (which
    /// keeps its full width underneath) and slides in and out.
    fn drawer(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let want = match (self.settings_open, self.filter_open) {
            (true, _) => Some(Fly::Settings),
            (false, true) => Some(Fly::Filter),
            _ => None,
        };
        if let Some(f) = want {
            self.fly = Some(f);
        }
        let target = if want.is_some() { 1.0 } else { 0.0 };
        if (self.fly_t - target).abs() > 0.001 {
            let now = ctx.input(|i| i.time);
            let dt = (now - self.fly_prev).clamp(0.0, 0.05) as f32;
            let step = dt / 0.25;
            self.fly_t = if self.fly_t < target {
                (self.fly_t + step).min(1.0)
            } else {
                (self.fly_t - step).max(0.0)
            };
            ctx.request_repaint();
        }
        self.fly_prev = ctx.input(|i| i.time);
        if self.fly_t <= 0.001 {
            if want.is_none() {
                self.fly = None;
            }
            return;
        }
        let close = match self.fly.unwrap_or(Fly::Settings) {
            Fly::Settings => self.settings_panel(ui, self.fly_t),
            Fly::Filter => self.filter_panel(ui, self.fly_t),
        };
        if close {
            self.settings_open = false;
            self.filter_open = false;
        }
    }

    fn player_view(&mut self, ui: &mut Ui) {
        let tx = self.req.tx();
        let Some(arc) = self.player.clone() else { return };
        let mut want_danmu = None;
        {
            let mut guard = arc.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.recording = guard
                .entries
                .get(guard.index)
                .is_some_and(|e| self.recordings.contains_key(&e.live_id));
            if guard.danmu_url.is_some() && !guard.danmu_requested {
                guard.danmu_requested = true;
                if let Some(e) = guard.entries.get(guard.index) {
                    want_danmu = Some((e.live_id.clone(), guard.danmu_url.clone().unwrap()));
                }
            }
        }
        if let Some((live_id, url)) = want_danmu {
            self.fetch_danmu(live_id, url);
        }
        let mut guard = arc.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let pl = &mut *guard;
        pl.pump(ui.ctx(), egui::ViewportId::ROOT);

        Panel::top("player_top")
            .frame(
                Frame::new()
                    .fill(t::BG)
                    .inner_margin(Margin { left: 10, right: 12, top: 8, bottom: 6 }),
            )
            .show(ui, |ui| pl.top_bar(ui));

        let visible = pl.controls_visible();
        Panel::bottom("player_bottom")
            .frame(
                Frame::new()
                    .fill(t::BG)
                    .inner_margin(if visible {
                        Margin {
                            left: 14,
                            right: 14,
                            top: 6,
                            bottom: 10,
                        }
                    } else {
                        Margin::ZERO
                    }),
            )
            .show(ui, |ui| {
                if visible {
                    pl.bottom_bar(ui);
                }
            });

        egui::CentralPanel::default()
            .frame(Frame::new().fill(Color32::BLACK).inner_margin(0))
            .show(ui, |ui| pl.video(ui));

        // Same drawer as the list page's filters: it floats over the video instead of
        // stealing width from it.
        let target = if pl.playlist_open { 1.0 } else { 0.0 };
        if (pl.fly_t - target).abs() > 0.001 {
            let now = ui.ctx().input(|i| i.time);
            let step = (now - pl.fly_prev).clamp(0.0, 0.05) as f32 / 0.25;
            pl.fly_t = if pl.fly_t < target {
                (pl.fly_t + step).min(1.0)
            } else {
                (pl.fly_t - step).max(0.0)
            };
            ui.ctx().request_repaint();
        }
        pl.fly_prev = ui.ctx().input(|i| i.time);
        if pl.fly_t > 0.001 {
            let title = if pl.show_danmu {
                format!("弹幕 {}", pl.danmu.len())
            } else {
                format!("播放列表 {}", pl.entries.len())
            };
            let (close, rect) = flyout(
                ui,
                "playlist_flyout",
                300.0,
                &title,
                Icon::List,
                pl.fly_t,
                |ui| pl.playlist_body(ui, &mut self.images, &tx),
            );
            pl.playlist_rect = rect;
            if close {
                pl.playlist_open = false;
            }
        } else {
            pl.playlist_rect = Rect::NOTHING;
        }

        pl.settle();
        let cmd = pl.cmd;
        pl.cmd = Cmd::None;
        drop(guard);
        if cmd != Cmd::None {
            self.handle_cmd(cmd);
        }
    }

    /// Drives the always-on-top mini window and picks up its button presses.
    fn mini_tick(&mut self, ctx: &Context) {
        let Some(state) = self.mini.clone() else {
            return;
        };
        // The little window closes itself (a hidden main window cannot be relied on to
        // run a pass), so read that first - calling `show` again would resurrect it.
        match crate::mini::take_action(&state) {
            crate::mini::ACTION_RESTORE => {
                self.mini = None;
            }
            crate::mini::ACTION_CLOSE => {
                self.mini = None;
                self.player = None;
            }
            _ => {}
        }
        if self.mini.is_some() {
            crate::mini::show(ctx, &state, ctx.viewport_rect());
        }
    }

    fn header(&mut self, ui: &mut Ui) {
        Panel::top("header")
            .frame(
                Frame::new()
                    .fill(t::BG)
                    .inner_margin(Margin {
                        left: 18,
                        right: 14,
                        top: 15,
                        bottom: 4,
                    }),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("口袋48 直播")
                            .size(21.0)
                            .color(t::TEXT)
                            .font(t::heading(21.0)),
                    );
                    ui.label(RichText::new("EGUI · NATIVE").size(10.0).color(t::TEXT_3));

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icons::icon_button(ui, Icon::Settings, 32.0)
                            .on_hover_text("播放设置")
                            .clicked()
                        {
                            self.settings_open = !self.settings_open;
                            self.filter_open = false;
                        }
                        if tool_button(ui, Icon::Filter, "筛选", self.has_filters())
                            .on_hover_text("按团体、类型、成员与日期筛选")
                            .clicked()
                        {
                            self.filter_open = !self.filter_open;
                            self.settings_open = false;
                        }
                    });
                });
            });
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        Panel::top("toolbar")
            .frame(
                Frame::new()
                    .fill(t::BG)
                    .inner_margin(Margin::symmetric(18, 8)),
            )
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    self.tabs(ui);

                    let group_text = if self.groups.is_empty() {
                        "全部团体"
                    } else if self.groups.len() == 1 {
                        self.groups[0].as_str()
                    } else {
                        "已选多个团体"
                    };                    egui::ComboBox::from_id_salt("group_combo")
                        .width(112.0)
                        .selected_text(RichText::new(group_text).size(13.0).color(t::TEXT_2))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.groups, vec![], "全部团体");
                            for g in GROUPS_LIST {
                                ui.selectable_value(&mut self.groups, vec![g.to_owned()], g);
                            }
                        });

                    let type_text = if self.types.is_empty() {
                        "全部类型"
                    } else if self.types.len() == 1 {
                        self.types[0].as_str()
                    } else {
                        "已选多个类型"
                    };
                    egui::ComboBox::from_id_salt("type_combo")
                        .width(100.0)
                        .selected_text(RichText::new(type_text).size(13.0).color(t::TEXT_2))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.types, vec![], "全部类型");
                            for ty in TYPE_LIST {
                                ui.selectable_value(&mut self.types, vec![ty.to_owned()], ty);
                            }
                        });

                    ui.add_space(4.0);
                    self.search_box(ui);

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add(t::ghost_button("刷新").min_size(Vec2::new(66.0, 32.0)))
                            .on_hover_text("重新拉取当前列表")
                            .clicked()
                        {
                            match self.tab {
                                Tab::Live => self.refresh_live(),
                                Tab::Record => self.refresh_records(false),
                                Tab::Open | Tab::OpenRecord => self.refresh_open(false),
                            }
                        }

                        if !self.recordings.is_empty() {
                            ui.label(
                                RichText::new(format!("● 录制中 {}", self.recordings.len()))
                                    .size(11.5)
                                    .color(t::OK),
                            );
                        }

                        let mode_label = match self.mode {
                            PlayerMode::System => "系统播放器",
                            PlayerMode::Vlc => "VLC 播放器",
                            PlayerMode::Builtin => "内置 libVLC",
                        };
                        let mode_resp = ui.add(
                            egui::Button::new(
                                RichText::new(mode_label).size(11.5).color(t::TEXT_2),
                            )
                            .fill(t::alpha(t::WHITE, 14))
                            .stroke(Stroke::new(1.0, t::LINE_SOFT))
                            .corner_radius(16.0)
                            .min_size(Vec2::new(58.0, 32.0)),
                        );
                        if mode_resp
                            .on_hover_text("当前播放器，点击可切换")
                            .clicked()
                        {
                            self.settings_open = true;
                            self.filter_open = false;
                        }

                        if !self.downloads.is_empty() {
                            ui.label(
                                RichText::new(format!("下载中 {}", self.downloads.len()))
                                    .size(11.5)
                                    .color(t::TEXT_3),
                            );
                        }
                    });
                });
            });
    }

    fn tabs(&mut self, ui: &mut Ui) {
        // Buttons floor their height at `interact_size.y`, so the segmented control
        // has to lower that locally or it overflows its own 32pt container.
        let floor = ui.spacing().interact_size.y;
        ui.spacing_mut().interact_size.y = 26.0;
        // The label's own padding also feeds the button's desired height, so it has to
        // come down with the floor or the container ends up 2pt taller than its peers.
        ui.spacing_mut().button_padding.y = 3.0;
        Frame::new()
            .fill(t::alpha(t::WHITE, 10))
            .corner_radius(9.0)
            .inner_margin(2)
            .stroke(Stroke::new(1.0, t::LINE_SOFT))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for value in [Tab::Live, Tab::Record, Tab::Open, Tab::OpenRecord] {
                        let selected = self.tab == value;
                        let label = value.label();
                        let btn = egui::Button::new(
                            RichText::new(label)
                                .size(13.0)
                                .color(if selected { t::ON_ACCENT } else { t::TEXT_2 })
                                .font(t::heading(13.0)),
                        )
                        .fill(if selected {
                            t::WHITE
                        } else {
                            Color32::TRANSPARENT
                        })
                        .corner_radius(7.0);
                        let wide = label.chars().count() > 2;
                        if ui
                            .add_sized(Vec2::new(if wide { 78.0 } else { 64.0 }, 26.0), btn)
                            .on_hover_text(label)
                            .clicked()
                            && !selected
                        {
                            self.tab = value;
                            let empty = match value {
                                Tab::Live => self.lives.is_empty(),
                                Tab::Record => self.records.is_empty(),
                                Tab::Open => self.open_lives.is_empty(),
                                Tab::OpenRecord => self.open_records.is_empty(),
                            };
                            if value.is_open() {
                                if empty {
                                    self.refresh_open(false);
                                }
                            } else if empty {
                                self.refresh_records(false);
                            }
                        }
                    }
                });
            });
        ui.spacing_mut().interact_size.y = floor;
    }

    fn search_box(&mut self, ui: &mut Ui) {
        // TextEdit floors its height at `interact_size.y` and only paints its frame
        // around the text, so the box has to be drawn at an explicit 32pt rect or it
        // ends up shorter than the buttons next to it.
        let frame = Frame::new()
            .fill(t::FIELD)
            .corner_radius(7.0)
            .inner_margin(Margin {
                left: 28,
                right: 10,
                top: 0,
                bottom: 0,
            })
            .stroke(Stroke::new(1.0, t::LINE_SOFT));
        let (rect, _) = ui.allocate_exact_size(Vec2::new(300.0, 32.0), Sense::hover());
        let inset = frame.total_margin();
        let content = rect - inset;
        ui.painter().add(frame.paint(content));
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(content)
                .layout(Layout::left_to_right(Align::Center)),
        );
        child.spacing_mut().interact_size.y = 20.0;
        let text_w = (child.available_width() - 2.0).max(40.0);
        child.add(
            egui::TextEdit::singleline(&mut self.search)
                .hint_text("搜索成员或标题")
                .vertical_align(Align::Center)
                .desired_width(text_w)
                .frame(Frame::new()),
        );
        icons::draw(
            ui,
            Icon::Search,
            Rect::from_center_size(Pos2::new(rect.min.x + 16.0, rect.center().y), Vec2::splat(14.0)),
            t::TEXT_3,
        );
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        Panel::bottom("status")
            .frame(
                Frame::new()
                    .fill(t::BG)
                    .inner_margin(Margin::symmetric(18, 7)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if self.loading_live || self.loading_records || self.loading_open {
                        ui.add(egui::Spinner::new().size(13.0));
                        ui.label(RichText::new("加载中…").size(12.0).color(t::TEXT_2));
                    } else {
                        let tab = self.tab;
                        let shown = self.filtered(tab).len();
                        let total = match tab {
                            Tab::Live => self.lives.len(),
                            Tab::Record => self.records.len(),
                            Tab::Open => self.open_lives.len(),
                            Tab::OpenRecord => self.open_records.len(),
                        };
                        let what = match tab {
                            Tab::Live => "个直播",
                            Tab::Record => "个录播",
                            Tab::Open => "场公演",
                            Tab::OpenRecord => "个公演录播",
                        };
                        ui.label(
                            RichText::new(format!("{shown} / {total} {what}"))
                                .size(12.0)
                                .color(t::TEXT_3),
                        );
                    }
                    if self.has_filters() {
                        ui.label(
                            RichText::new("筛选中")
                                .size(11.0)
                                .color(t::WHITE)
                                .background_color(t::alpha(t::WHITE, 34)),
                        );
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if self.cfg.grab.enabled {
                            let left = self
                                .grab_at
                                .map(|at| at.saturating_duration_since(std::time::Instant::now()).as_secs())
                                .unwrap_or(0);
                            ui.label(
                                RichText::new(format!("自动抓取 {} 秒后", left))
                                    .size(11.0)
                                    .color(t::TEXT_3),
                            );
                        }
                        let (txt, col) = if !self.vlc.checked {
                            ("libVLC 未检测", t::TEXT_3)
                        } else if self.vlc.installed {
                            ("libVLC 就绪", t::OK)
                        } else if self.vlc.integrated {
                            ("libVLC 无法加载", t::WARN)
                        } else {
                            ("libVLC 缺失", t::ERR)
                        };
                        ui.label(RichText::new(txt).size(11.0).color(col));
                    });
                });
            });
    }

    // ---------------------------------------------------------------- list

    fn list(&mut self, ui: &mut Ui) {
        let tab = self.tab;
        let items = self.filtered(tab);
        let next = match tab {
            Tab::Live => "0".to_owned(),
            Tab::Record => self.record_next.clone(),
            Tab::Open => self.open_next.clone(),
            Tab::OpenRecord => self.open_record_next.clone(),
        };
        let loading = self.loading_records || self.loading_live || self.loading_open;
        let is_record = tab.is_record();
        let more_open = tab == Tab::OpenRecord;

        if items.is_empty() {
            egui::ScrollArea::vertical()
                .id_salt("list")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(70.0);
                        if loading {
                            ui.add(egui::Spinner::new().size(26.0));
                            ui.label(
                                RichText::new("正在加载列表…").color(t::TEXT_2).size(13.0),
                            );
                        } else {
                            ui.label(RichText::new("无匹配内容").color(t::TEXT_3).size(14.0));
                        }
                    });
                });
            return;
        }

        let gap = 14.0;
        let min_card = 280.0;
        let avail = ui.available_width();
        let cols = (((avail + gap) / (min_card + gap)).floor() as usize).clamp(1, 6);
        let w = (avail - gap * (cols - 1) as f32) / cols as f32;
        let rows = items.len().div_ceil(cols);
        // Cards are all the same size, so the rows can be virtualised. The height is
        // measured from the first real row and re-derived whenever the width changes.
        let est = match self.row_height {
            Some((cw, h)) if (cw - w).abs() < 0.5 => h,
            _ => w + 86.0,
        };
        let measured = Cell::new(0.0f32);
        let last = rows;

        ui.spacing_mut().item_spacing.y = gap;
        egui::ScrollArea::vertical()
            .id_salt("list")
            .auto_shrink([false, false])
            .show_rows(ui, est, rows, |ui, range| {
                let first = range.start;
                let end = range.end;
                for row in range {
                    let lo = row * cols;
                    let hi = (lo + cols).min(items.len());
                    let resp = ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        for item in &items[lo..hi] {
                            self.card(ui, item, w, is_record);
                        }
                    });
                    if row == first {
                        measured.set(resp.response.rect.height());
                    }
                }
                if is_record && next != "0" && end >= last {
                    ui.add_space(6.0);
                    ui.vertical_centered(|ui| {
                        let label = if more_open { "加载更多公演录播" } else { "加载更多录播" };
                        if ui
                            .add_sized(Vec2::new(180.0, 32.0), t::ghost_button(label))
                            .clicked()
                        {
                            if more_open {
                                self.refresh_open(true);
                            } else {
                                self.load_more();
                            }
                        }
                    });
                }
            });

        let h = measured.get();
        if h > 0.0 && (h - est).abs() > 1.0 {
            self.row_height = Some((w, h));
            ui.ctx().request_repaint();
        }
    }

    /// Every card is exactly the same size: a square cover (centre-cropped), one
    /// meta row and one action row. Download state is folded into those two rows so
    /// a card never grows taller than its neighbours.
    fn card(&mut self, ui: &mut Ui, item: &LiveItem, w: f32, is_record: bool) {
        let tx = self.req.tx();
        let badge = badge_text(item);
        let linked = item.in_microphone_connection;
        let cover_url = api::resolve_image(&item.cover_path);
        let avatar_url = api::resolve_image(&item.avatar);
        let live_id = item.live_id.clone();
        let title = if item.title.is_empty() {
            "暂无标题".to_owned()
        } else {
            item.title.clone()
        };
        let nickname = if item.nickname.is_empty() {
            item.sub_title.clone()
        } else {
            item.nickname.clone()
        };
        let ctime = item.ctime;
        let download = self.downloads.get(&live_id).copied();
        let recording = self.recordings.contains_key(&live_id);
        let retryable = !recording && self.jobs.contains_key(&live_id) && download.is_none();
        // A theatre show that has not started has no stream to ask for yet.
        let unavailable = item.source == crate::models::Source::Open && item.status == 1;
        let builtin = self.mode == PlayerMode::Builtin;
        let inner = (w - 24.0).max(120.0);
        // Where the button row starts: the card-wide "open" hit area has to stop
        // there, or it would swallow the clicks on 播放/下载/地址.
        let actions_at = Cell::new(f32::INFINITY);

        let frame = Frame::new()
            .fill(t::CARD)
            .corner_radius(11.0)
            .inner_margin(11)
            .stroke(Stroke::new(1.0, t::LINE_SOFT))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.set_width(inner);
                    ui.spacing_mut().item_spacing.y = 0.0;

                    let (crect, _) = ui.allocate_exact_size(Vec2::splat(inner), Sense::hover());
                    self.images.cover(ui, &cover_url, crect, 6, &tx);

                    // A stepped gradient reads far softer than the hard black band.
                    let grad = 48.0f32;
                    let steps = 16;
                    for i in 0..steps {
                        let f = (i + 1) as f32 / steps as f32;
                        let y0 = crect.max.y - grad * (1.0 - f);
                        ui.painter().rect_filled(
                            Rect::from_min_max(
                                Pos2::new(crect.min.x, y0),
                                Pos2::new(crect.max.x, y0 + grad / steps as f32 + 1.0),
                            ),
                            0.0,
                            t::alpha(Color32::BLACK, (196.0 * f * f) as u8),
                        );
                    }

                    let badge_w = util::pill_width(badge, 11.0);
                    util::pill(
                        ui,
                        Pos2::new(crect.max.x - 8.0 - badge_w, crect.max.y - 26.0),
                        badge,
                        11.0,
                        t::WHITE,
                        t::alpha(t::WHITE, 46),
                    );
                    ui.painter().text(
                        Pos2::new(crect.min.x + 9.0, crect.max.y - 16.5),
                        Align2::LEFT_CENTER,
                        util::truncate(&title, inner - badge_w - 34.0, 12.5),
                        t::heading(12.5),
                        Color32::from_rgb(0xF6, 0xF6, 0xF6),
                    );

                    let tab_label = match (is_record, item.source) {
                        (true, crate::models::Source::Open) => "公演录播",
                        (false, crate::models::Source::Open) => "公演",
                        (true, _) => "录播",
                        (false, _) => "● 直播",
                    };
                    util::pill(
                        ui,
                        Pos2::new(crect.min.x + 8.0, crect.min.y + 8.0),
                        tab_label,
                        11.0,
                        t::WHITE,
                        t::alpha(Color32::BLACK, 178),
                    );
                    let mut pill_x = crect.max.x - 8.0;
                    if recording {
                        pill_x -= util::pill_width("● 录制", 11.0) + 6.0;
                        util::pill(
                            ui,
                            Pos2::new(pill_x, crect.min.y + 8.0),
                            "● 录制",
                            11.0,
                            t::WHITE,
                            t::alpha(t::ERR, 220),
                        );
                    }
                    if linked {
                        util::pill(
                            ui,
                            Pos2::new(
                                pill_x - util::pill_width("连麦", 11.0) - 6.0,
                                crect.min.y + 8.0,
                            ),
                            "连麦",
                            11.0,
                            t::WHITE,
                            t::alpha(Color32::BLACK, 178),
                        );
                    }

                    if let Some(p) = download {
                        let rail = Rect::from_min_max(
                            Pos2::new(crect.min.x, crect.max.y - 3.0),
                            Pos2::new(crect.max.x, crect.max.y),
                        );
                        ui.painter().rect_filled(rail, 0.0, t::alpha(Color32::BLACK, 160));
                        ui.painter().rect_filled(
                            Rect::from_min_size(
                                rail.min,
                                Vec2::new(
                                    rail.width() * (p as f32 / 100.0).clamp(0.0, 1.0),
                                    3.0,
                                ),
                            ),
                            0.0,
                            t::WHITE,
                        );
                    }

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        if item.avatar.is_empty() {
                            let (rect, _) =
                                ui.allocate_exact_size(Vec2::splat(34.0), Sense::hover());
                            ui.painter()
                                .circle_filled(rect.center(), 17.0, t::alpha(t::WHITE, 22));
                        } else {
                            self.images.avatar(ui, &avatar_url, 34.0, &tx);
                        }
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.label(
                                RichText::new(util::truncate(&nickname, inner - 60.0, 13.0))
                                    .color(t::TEXT)
                                    .font(t::heading(13.0)),
                            );
                            let second = if let Some(p) = download {
                                format!("下载中 {p:.0}%")
                            } else if retryable {
                                "分片有缺失，可重试续传".to_owned()
                            } else if recording {
                                let secs = self
                                    .recordings
                                    .get(&live_id)
                                    .map(|r| r.started_at.elapsed().as_secs())
                                    .unwrap_or(0);
                                format!("录制中 {}:{}", secs / 60, secs % 60)
                            } else if let Some(ms) = ctime {
                                let when = crate::app::fmt_time(ms);
                                if item.duration.is_empty() {
                                    when
                                } else {
                                    format!("{when} · {}", item.duration)
                                }
                            } else {
                                String::new()
                            };
                            if !second.is_empty() {
                                ui.label(RichText::new(second).color(t::TEXT_3).size(11.0));
                            }
                        });
                    });

                    ui.add_space(10.0);
                    actions_at.set(ui.cursor().min.y);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        // The primary button absorbs the leftover width so the row
                        // is flush with the cover instead of trailing off into slack.
                        // 播放 + 录制/下载 + 地址, plus 重试 and 取消 when they apply.
                        let mut tail = 62.0 + 32.0 + 6.0 * 2.0 + 62.0 + 6.0;
                        if retryable {
                            tail += 62.0 + 6.0;
                        }
                        if download.is_some() {
                            tail += 32.0 + 6.0;
                        }
                        let play = ui.add_sized(
                            Vec2::new((inner - tail).max(72.0), 32.0),
                            t::accent_button("播放"),
                        );
                        if play
                            .on_hover_text(if unavailable {
                                "公演还没有开始"
                            } else if builtin {
                                "用内置 libVLC 播放"
                            } else {
                                "用外部播放器打开"
                            })
                            .clicked()
                            && !unavailable
                        {
                            self.play_by_id(&live_id, !is_record);
                        }

                        if is_record && !unavailable {
                            let dl = ui.add_sized(Vec2::new(62.0, 32.0), t::ghost_button("下载"));
                            if dl.on_hover_text("保存为 TS 文件").clicked() {
                                self.act(
                                    item,
                                    Action::Download { item: item.clone() },
                                );
                            }
                        }

                        if !is_record {
                            let label = if recording { "停止" } else { "录制" };
                            let btn = if recording {
                                t::accent_button(label)
                            } else {
                                t::ghost_button(label)
                            };
                            let rec = ui.add_sized(Vec2::new(62.0, 32.0), btn);
                            if rec
                                .on_hover_text(if recording {
                                    "停止录制，已录内容保留在文件里"
                                } else {
                                    "后台录制这条直播，断流会自动续录"
                                })
                                .clicked()
                                && !unavailable
                            {
                                self.toggle_recording(&live_id);
                            }
                        }

                        let cp = icons::icon_button(ui, Icon::Copy, 32.0);
                        if cp.on_hover_text("复制推流/录播地址").clicked() && !unavailable {
                            self.act(item, Action::Copy);
                        }

                        if retryable && download.is_none() {
                            let rt = ui.add_sized(Vec2::new(62.0, 32.0), t::ghost_button("重试"));
                            if rt
                                .on_hover_text("只补下缺失的分片，已下载的会跳过")
                                .clicked()
                            {
                                self.retry_download(&live_id);
                            }
                        }

                        if download.is_some()
                            && icons::icon_button(ui, Icon::Close, 32.0)
                                .on_hover_text("取消下载")
                                .clicked()
                        {
                            self.cancel_download(&live_id);
                        }
                    });
                });
            });

        let rect = frame.response.rect;
        let hot = Rect::from_min_max(rect.min, Pos2::new(rect.max.x, actions_at.get()));
        let resp = ui
            .interact(hot, ui.id().with(&live_id), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.hovered() {
            ui.painter().rect_stroke(
                rect,
                11.0,
                Stroke::new(1.0, t::alpha(t::WHITE, 90)),
                StrokeKind::Inside,
            );
        }
        if resp.clicked() && !unavailable {
            self.play_by_id(&live_id, !is_record);
        }
    }

    fn play_by_id(&mut self, live_id: &str, live_tab: bool) {
        let source: &Vec<LiveItem> = match (live_tab, self.tab.is_open()) {
            (true, false) => &self.lives,
            (false, false) => &self.records,
            (true, true) => &self.open_lives,
            (false, true) => &self.open_records,
        };
        let found = source.iter().find(|i| i.live_id == live_id).cloned();

        if let Some(item) = found {
            match self.mode {
                PlayerMode::System => self.act(&item, Action::PlayExternal),
                PlayerMode::Vlc => self.act(&item, Action::PlayVlcExternal),
                PlayerMode::Builtin => self.start_builtin(live_id),
            }
        }
    }

    // ------------------------------------------------------------- flyouts

    fn filter_panel(&mut self, ui: &mut Ui, t: f32) -> bool {
        flyout(ui, "filter_flyout", 380.0, "高级筛选", Icon::Filter, t, |ui| {
            let all_nick = self.nicknames();

            util::section(ui, "团体");
            let mut groups = self.groups.clone();
            ui.horizontal_wrapped(|ui| {
                for g in GROUPS_LIST {
                    let on = groups.iter().any(|x| x == g);
                    if chip(ui, g, on).clicked() {
                        if on {
                            groups.retain(|x| x != g);
                        } else {
                            groups.push(g.to_owned());
                        }
                    }
                }
            });
            self.groups = groups;

            util::section(ui, "类型");
            let mut types = self.types.clone();
            ui.horizontal_wrapped(|ui| {
                for ty in TYPE_LIST {
                    let on = types.iter().any(|x| x == ty);
                    if chip(ui, ty, on).clicked() {
                        if on {
                            types.retain(|x| x != ty);
                        } else {
                            types.push(ty.to_owned());
                        }
                    }
                }
            });
            self.types = types;

            util::section(ui, "成员");
            ui.add(
                egui::TextEdit::singleline(&mut self.member_query)
                    .hint_text("搜索成员")
                    .desired_width((ui.available_width() - 20.0).max(80.0))
                    .frame(field_frame()),
            );
            if !self.members.is_empty() {
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(RichText::new("清空").size(12.0).color(t::TEXT_3))
                                .min_size(Vec2::new(52.0, 26.0)),
                        )
                        .clicked()
                    {
                        self.members.clear();
                    }
                    ui.label(
                        RichText::new(format!("已选 {}", self.members.len()))
                            .size(12.0)
                            .color(t::TEXT_3),
                    );
                });
            }

            let query = self.member_query.to_lowercase();
            Frame::new()
                .fill(t::BG)
                .corner_radius(8.0)
                .inner_margin(8)
                .stroke(Stroke::new(1.0, t::LINE_SOFT))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("members")
                        .max_height(190.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let picked: Vec<String> = self.members.clone();
                            let mut added: Option<String> = None;
                            let mut removed: Option<String> = None;
                            ui.columns(2, |cols| {
                                for (i, col) in cols.iter_mut().enumerate() {
                                    col.spacing_mut().item_spacing.y = 2.0;
                                    for (idx, name) in all_nick.iter().enumerate() {
                                        if idx % 2 != i || !name.to_lowercase().contains(&query) {
                                            continue;
                                        }
                                        let mut on = picked.iter().any(|x| x == name);
                                        if col.checkbox(&mut on, name).changed() {
                                            if on {
                                                added = Some(name.clone());
                                            } else {
                                                removed = Some(name.clone());
                                            }
                                        }
                                    }
                                }
                            });
                            if let Some(n) = added {
                                self.members.push(n);
                            }
                            if let Some(n) = removed {
                                self.members.retain(|x| *x != n);
                            }
                        });
                });

            let only = self.member_only.clone();
            util::section(ui, "按成员查询录播（服务端）");
            if let Some((_, name)) = only {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("当前只看 {name}")).size(12.5).color(t::TEXT));
                    if ui
                        .add(egui::Button::new(RichText::new("取消").size(12.0).color(t::TEXT_3)))
                        .clicked()
                    {
                        self.clear_member_only();
                    }
                });
            } else if self.members.len() == 1 {
                let name = self.members[0].clone();
                if ui
                    .add_sized(
                        Vec2::new(ui.available_width(), 30.0),
                        t::ghost_button(&format!("只看 {name} 的历史录播")),
                    )
                    .on_hover_text("带 userId 向服务器查询，比本地筛选翻得深")
                    .clicked()
                {
                    self.records_of(&name);
                }
            } else {
                ui.label(
                    RichText::new("在上面的成员列表里勾一个人，再点这里查询")
                        .size(11.5)
                        .color(t::TEXT_3),
                );
            }

            util::section(ui, "日期范围（录播）");
            let mut on = self.date_on;
            if ui.checkbox(&mut on, "启用日期筛选").changed() {
                self.date_on = on;
            }
            if self.date_on {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("从").size(12.0).color(t::TEXT_3));
                    date_edit(ui, &mut self.date_from);
                    ui.label(RichText::new("到").size(12.0).color(t::TEXT_3));
                    date_edit(ui, &mut self.date_to);
                });
            }

            util::section(ui, "筛选预设");
            ui.horizontal(|ui| {
                let name_w = (ui.available_width() - 64.0).max(80.0);
                ui.add(
                    egui::TextEdit::singleline(&mut self.preset_name)
                        .hint_text("预设名称")
                        .desired_width(name_w)
                        .frame(field_frame()),
                );
                if ui
                    .add_sized(
                        Vec2::new(58.0, 32.0),
                        egui::Button::new(RichText::new("保存").size(12.0)),
                    )
                    .clicked()
                {
                    self.save_preset();
                }
            });
            let names: Vec<String> = self.cfg.presets.iter().map(|p| p.name.clone()).collect();
            for (idx, name) in names.iter().enumerate() {
                ui.horizontal(|ui| {
                    let row_w = (ui.available_width() - 38.0).max(60.0);
                    if ui
                        .add_sized(
                            Vec2::new(row_w, 32.0),
                            egui::Button::new(
                                RichText::new(name).size(12.5).color(t::TEXT_2),
                            )
                            .stroke(Stroke::new(1.0, t::LINE_SOFT)),
                        )
                        .on_hover_text("载入该预设")
                        .clicked()
                    {
                        self.load_preset(idx);
                    }
                    if icons::icon_button(ui, Icon::Close, 32.0)
                        .on_hover_text("删除预设")
                        .clicked()
                    {
                        self.cfg.presets.remove(idx);
                        config::save(&self.cfg);
                    }
                });
            }

            ui.add_space(12.0);
            let width = ui.available_width();
            if ui
                .add_sized(Vec2::new(width, 32.0), t::ghost_button("重置全部筛选"))
                .clicked()
            {
                self.reset_filters();
            }
        }).0
    }

    fn settings_panel(&mut self, ui: &mut Ui, t: f32) -> bool {
        if !self.vlc.checked && !self.vlc_check_pending {
            self.vlc_check_pending = true;
            self.check_vlc();
        }
        flyout(ui, "settings_flyout", 360.0, "播放设置", Icon::Settings, t, |ui| {
            util::section(ui, "播放器选择");
            let options = [
                (
                    PlayerMode::System,
                    "系统默认播放器",
                    "用 PotPlayer 或系统关联播放器打开",
                    true,
                ),
                (PlayerMode::Vlc, "VLC 播放器", "用已安装的 VLC 打开", true),
                (
                    PlayerMode::Builtin,
                    "内置播放器 (libVLC)",
                    "程序内直接播放，支持 RTMP / HLS / HTTP-TS",
                    self.vlc.installed || self.vlc.integrated || self.engine.is_some(),
                ),
            ];
            for (mode, title, desc, enabled) in options {
                let selected = self.mode == mode;
                // A Frame sizes to its content, so pin the width or the options come
                // out as three different shapes.
                let opt_w = ui.available_width();
                let frame = Frame::new()
                    .fill(t::alpha(t::WHITE, if selected { 26 } else { 10 }))
                    .corner_radius(9.0)
                    .inner_margin(10)
                    .stroke(Stroke::new(
                        if selected { 1.5 } else { 1.0 },
                        if selected { t::WHITE } else { t::LINE_SOFT },
                    ))
                    .show(ui, |ui| {
                        ui.set_width(opt_w - 20.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            let (rect, _) =
                                ui.allocate_exact_size(Vec2::new(16.0, 22.0), Sense::hover());
                            ui.painter().circle_stroke(
                                rect.center(),
                                7.0,
                                Stroke::new(1.4, if enabled { t::TEXT_2 } else { t::TEXT_3 }),
                            );
                            if selected {
                                ui.painter().circle_filled(rect.center(), 4.0, t::WHITE);
                            }
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.label(
                                    RichText::new(title)
                                        .size(13.5)
                                        .color(if enabled { t::TEXT } else { t::TEXT_3 })
                                        .font(t::heading(13.5)),
                                );
                                ui.label(RichText::new(desc).size(11.5).color(t::TEXT_3));
                                if !enabled {
                                    ui.label(
                                        RichText::new("需要先准备好 libVLC 引擎")
                                            .size(11.0)
                                            .color(t::WARN),
                                    );
                                }
                            });
                        });
                    });
                let resp = ui.interact(frame.response.rect, Id::new(("mode", title)), Sense::click());
                if resp.clicked() && enabled {
                    let kind = mode.as_str().to_owned();
                    self.mode = mode;
                    self.cfg.player_mode = Some(kind);
                    config::save(&self.cfg);
                    if self.mode == PlayerMode::Builtin {
                        self.ensure_engine();
                    }
                }
            }

            util::section(ui, "libVLC 引擎");
            let (dot, txt) = if !self.vlc.checked {
                (t::TEXT_3, "尚未检测")
            } else if self.vlc.installed {
                (
                    t::OK,
                    if self.vlc.integrated {
                        "可用（已内置）"
                    } else {
                        "可用（来自本机 VLC）"
                    },
                )
            } else if self.vlc.integrated {
                (t::WARN, "已内置但加载失败，可重新下载")
            } else {
                (t::ERR, "不可用，请下载引擎")
            };
            let status_w = ui.available_width();
            Frame::new()
                .fill(t::alpha(t::WHITE, 10))
                .corner_radius(9.0)
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.set_width(status_w - 20.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let (rect, _) = ui.allocate_exact_size(Vec2::new(10.0, 18.0), Sense::hover());
                        ui.painter().circle_filled(rect.center(), 4.0, dot);
                        ui.label(RichText::new(txt).size(12.5).color(t::TEXT_2));
                    });
                    if let Some((p, msg)) = self.vlc.progress.clone() {
                        ui.add_space(6.0);
                        if p >= 0.0 {
                            ui.add(egui::ProgressBar::new(p as f32 / 100.0).animate(true));
                        }
                        ui.label(
                            RichText::new(msg)
                                .size(11.5)
                                .color(if p < 0.0 { t::ERR } else { t::TEXT_3 }),
                        );
                    }
                });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let half = ((ui.available_width() - 6.0) / 2.0).max(60.0);
                if ui
                    .add_sized(Vec2::new(half, 32.0), t::ghost_button("重新检测"))
                    .clicked()
                {
                    self.vlc.progress = None;
                    self.check_vlc();
                }
                if self.vlc.progress.is_none() {
                    if ui
                        .add_sized(Vec2::new(half, 32.0), t::accent_button("下载 VLC 引擎"))
                        .on_hover_text("约 101 MB，解压到应用数据目录")
                        .clicked()
                    {
                        self.download_engine();
                    }
                }
            });
            if !self.vlc.detail.is_empty() {
                ui.label(
                    RichText::new(crate::util::truncate(&self.vlc.detail, 290.0, 10.5))
                        .size(10.5)
                        .color(t::TEXT_3),
                );
            }

            util::section(ui, "下载与录制目录");
            let dir = self.download_dir();
            ui.label(
                RichText::new(crate::util::truncate(&dir, 240.0, 11.5))
                    .size(11.5)
                    .color(t::TEXT_3),
            );
            ui.horizontal(|ui| {
                let half = ((ui.available_width() - 6.0) / 2.0).max(60.0);
                if ui
                    .add_sized(Vec2::new(half, 30.0), t::ghost_button("选择目录"))
                    .on_hover_text("录播下载与直播录制都写到这里")
                    .clicked()
                {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        self.cfg.download_dir = Some(p.to_string_lossy().to_string());
                        config::save(&self.cfg);
                    }
                }
                if !self.recordings.is_empty() {
                    ui.label(
                        RichText::new(format!("● 正在录制 {}", self.recordings.len()))
                            .size(11.5)
                            .color(t::OK),
                    );
                }
            });

            util::section(ui, "自动抓取（开播即录）");
            let mut g = self.cfg.grab.clone();
            if ui.checkbox(&mut g.enabled, "启用自动抓取").changed() {
                self.cfg.grab = g.clone();
                config::save(&self.cfg);
            }
            ui.label(
                RichText::new("按名单轮询直播列表，命中且没在录的就开一条后台录制")
                    .size(11.0)
                    .color(t::TEXT_3),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("间隔（分）").size(12.0).color(t::TEXT_2));
                ui.add(egui::DragValue::new(&mut g.minutes).range(1..=720).speed(1));
                ui.label(RichText::new("同时最多").size(12.0).color(t::TEXT_2));
                ui.add(egui::DragValue::new(&mut g.max).range(1..=5).speed(1));
            });
            ui.label(RichText::new("成员姓名或 ID（逗号分隔）").size(12.0).color(t::TEXT_2));
            ui.add(
                egui::TextEdit::multiline(&mut g.users)
                    .hint_text("SNH48-张三, 123456789")
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .frame(field_frame()),
            );
            ui.label(RichText::new("保存目录（留空用上面的目录）").size(12.0).color(t::TEXT_2));
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(crate::util::truncate(
                        if g.dir.is_empty() { "（同上）" } else { &g.dir },
                        220.0,
                        11.0,
                    ))
                    .size(11.0)
                    .color(t::TEXT_3),
                );
                if ui
                    .add(egui::Button::new(RichText::new("选择").size(11.5).color(t::TEXT_2)))
                    .clicked()
                {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        g.dir = p.to_string_lossy().to_string();
                    }
                }
            });
            if g != self.cfg.grab {
                self.cfg.grab = g;
                config::save(&self.cfg);
            }
            if self.cfg.grab.enabled && self.engine.is_none() {
                ui.label(
                    RichText::new("自动抓取需要 libVLC 引擎，先在上方下载")
                        .size(11.0)
                        .color(t::WARN),
                );
            }
        }).0
    }

    fn save_preset(&mut self) {
        let name = self.preset_name.trim().to_owned();
        if name.is_empty() {
            return;
        }
        let fmt = |d: NaiveDate| d.format("%Y-%m-%d").to_string();
        let p = config::Preset {
            name: name.clone(),
            groups: self.groups.clone(),
            types: self.types.clone(),
            members: self.members.clone(),
            date_start: if self.date_on {
                fmt(self.date_from)
            } else {
                String::new()
            },
            date_end: if self.date_on {
                fmt(self.date_to)
            } else {
                String::new()
            },
        };
        match self.cfg.presets.iter_mut().find(|x| x.name == name) {
            Some(slot) => *slot = p,
            None => self.cfg.presets.push(p),
        }
        config::save(&self.cfg);
        self.preset_name.clear();
    }

    fn load_preset(&mut self, idx: usize) {
        let Some(p) = self.cfg.presets.get(idx).cloned() else {
            return;
        };
        self.groups = p.groups;
        self.types = p.types;
        self.members = p.members;
        self.date_on = !p.date_start.is_empty();
        if let Ok(d) = NaiveDate::parse_from_str(&p.date_start, "%Y-%m-%d") {
            self.date_from = d;
        }
        if let Ok(d) = NaiveDate::parse_from_str(&p.date_end, "%Y-%m-%d") {
            self.date_to = d;
        }
    }

    fn reset_filters(&mut self) {
        self.groups.clear();
        self.types.clear();
        self.members.clear();
        self.date_on = false;
        self.search.clear();
    }

    // ---------------------------------------------------------------- toasts

    fn paint_toasts(&mut self, ctx: &Context) {
        if self.toasts.is_empty() {
            return;
        }
        let now = std::time::Instant::now();
        egui::Area::new(Id::new("toasts"))
            .anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -30.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.spacing_mut().item_spacing.y = 6.0;
                    for (kind, text, at) in self.toasts.iter() {
                        let left = 4.0 - now.duration_since(*at).as_secs_f32();
                        let a = (left / 0.4).clamp(0.0, 1.0);
                        let accent = match kind {
                            ToastKind::Info => t::TEXT_2,
                            ToastKind::Success => t::OK,
                            ToastKind::Warning => t::WARN,
                            ToastKind::Error => t::ERR,
                        };
                        Frame::new()
                            .fill(t::alpha(Color32::from_rgb(30, 30, 30), (a * 235.0) as u8))
                            .corner_radius(9.0)
                            .inner_margin(Margin::symmetric(12, 7))
                            .stroke(Stroke::new(1.0, t::alpha(t::WHITE, (a * 42.0) as u8)))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 8.0;
                                    let (rect, _) =
                                        ui.allocate_exact_size(Vec2::new(3.0, 15.0), Sense::hover());
                                    ui.painter().rect_filled(rect, 1.5, accent.gamma_multiply(a));
                                    ui.label(
                                        RichText::new(text).size(12.5).color(t::TEXT.gamma_multiply(a)),
                                    );
                                });
                            });
                    }
                });
            });
    }
}

// ------------------------------------------------------------ widgets

fn date_edit(ui: &mut Ui, date: &mut NaiveDate) {
    let mut y = date.year();
    let mut m = date.month() as i32;
    let mut d = date.day() as i32;
    let changed = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let a = ui.add(egui::DragValue::new(&mut y).suffix(" 年"));
            let b = ui.add(egui::DragValue::new(&mut m).range(1..=12).suffix(" 月"));
            let c = ui.add(egui::DragValue::new(&mut d).range(1..=31).suffix(" 日"));
            a.changed() || b.changed() || c.changed()
        })
        .inner;
    if changed {
        let days = days_in_month(y, m);
        let dd = d.clamp(1, days) as u32;
        let mm = m.clamp(1, 12) as u32;
        if let Some(nd) = NaiveDate::from_ymd_opt(y, mm, dd) {
            *date = nd;
        }
    }
}

fn days_in_month(y: i32, m: i32) -> i32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        _ => 30,
    }
}

/// Fluent drawer: a dim backdrop over the whole window with the panel sliding in on
/// top of it, so the list behind keeps its width instead of being squeezed aside.
/// Returns whether the user asked to close it, plus the panel rect (which may sit
/// partly off screen while it slides).
fn flyout(
    ui: &mut Ui,
    id: &str,
    width: f32,
    title: &str,
    icon: Icon,
    t: f32,
    content: impl FnOnce(&mut Ui),
) -> (bool, Rect) {
    let close = Cell::new(false);
    let vp = ui.ctx().viewport_rect();
    let eased = 1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3);
    let w = width.min(vp.width() * 0.85);
    let panel = Rect::from_min_size(Pos2::new(vp.max.x - w * eased, vp.min.y), Vec2::new(w, vp.height()));

    ui.painter().rect_filled(
        vp,
        0.0,
        Color32::from_rgba_unmultiplied(0, 0, 0, (128.0 * eased) as u8),
    );
    if ui
        .interact(vp, Id::new(format!("{id}-backdrop")), Sense::click())
        .clicked()
    {
        close.set(true);
    }

    Area::new(Id::new(id))
        .fixed_pos(panel.min)
        .movable(false)
        .constrain(false)
        .show(ui, |ui| {
            Frame::new()
                .fill(t::SURFACE)
                .inner_margin(Margin {
                    left: 20,
                    right: 20,
                    top: 18,
                    bottom: 18,
                })
                .stroke(Stroke::new(1.0, t::LINE))
                .shadow(egui::Shadow {
                    offset: [-6, 0],
                    blur: 24,
                    spread: 0,
                    color: Color32::from_rgba_unmultiplied(0, 0, 0, 150),
                })
                .show(ui, |ui| {
                    ui.set_width(w - 40.0);
                    ui.set_height(vp.height() - 36.0);
                    ui.horizontal(|ui| {
                        let (rect, _) = ui.allocate_exact_size(Vec2::new(18.0, 22.0), Sense::hover());
                        icons::draw(ui, icon, rect.shrink(2.0), t::TEXT);
                        ui.label(
                            RichText::new(title)
                                .size(16.0)
                                .color(t::TEXT)
                                .font(t::heading(16.0)),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if icons::icon_button(ui, Icon::Close, 26.0).clicked() {
                                close.set(true);
                            }
                        });
                    });
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical()
                        .id_salt(format!("{id}-scroll"))
                        .auto_shrink([false, false])
                        .show(ui, |ui| content(ui));
                });
        });
    (close.into_inner(), panel)
}

/// Flyout text fields, matching the toolbar search box.
fn field_frame() -> Frame {
    Frame::new()
        .fill(t::FIELD)
        .corner_radius(7.0)
        .inner_margin(Margin {
            left: 9,
            right: 9,
            top: 6,
            bottom: 6,
        })
        .stroke(Stroke::new(1.0, t::LINE_SOFT))
}

fn chip(ui: &mut Ui, label: &str, selected: bool) -> egui::Response {
    let w = util::pill_width(label, 12.5) + 2.0;
    ui.add_sized(
        Vec2::new(w, 26.0),
        egui::Button::new(
            RichText::new(label)
                .size(12.5)
                .color(if selected { t::ON_ACCENT } else { t::TEXT_2 }),
        )
        .fill(if selected {
            t::WHITE
        } else {
            t::alpha(t::WHITE, 14)
        })
        .stroke(Stroke::new(
            1.0,
            if selected { t::WHITE } else { t::LINE_SOFT },
        ))
        .corner_radius(13.0),
    )
}

fn tool_button(ui: &mut Ui, icon: Icon, label: &str, badge: bool) -> egui::Response {
    let tw = util::pill_width(label, 13.0) - 12.0;
    let size = Vec2::new(tw + 42.0, 32.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let v = ui.style().interact(&resp);
        ui.painter().rect_filled(rect, 8.0, v.bg_fill);
        ui.painter().rect_stroke(
            rect,
            8.0,
            Stroke::new(1.0, if badge { t::WHITE } else { v.bg_stroke.color }),
            StrokeKind::Inside,
        );
        let icon_rect =
            Rect::from_center_size(Pos2::new(rect.min.x + 16.0, rect.center().y), Vec2::splat(15.0));
        icons::draw(ui, icon, icon_rect, v.fg_stroke.color);
        ui.painter().text(
            Pos2::new(rect.min.x + 28.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            t::heading(13.0),
            v.fg_stroke.color,
        );
        if badge {
            ui.painter()
                .circle_filled(Pos2::new(rect.max.x - 6.0, rect.min.y + 6.0), 3.5, t::WHITE);
        }
    }
    resp
}

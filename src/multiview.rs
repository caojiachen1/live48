//! A single-window wall of live rooms: one libVLC session per tile, a chosen
//! grid, draggable tiles and draggable gutters. There is deliberately no second
//! window here - every tile is painted inside the app's own viewport.

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::{
    Align, Align2, Color32, Context, CursorIcon, Frame, Id, Key, Layout as UiLayout, Margin, Panel,
    Pos2, Rect, Response, RichText, Sense, Stroke, StrokeKind, TextureId, Ui, Vec2, ViewportId,
};

use crate::config::{Config, MultiMember};
use crate::events::Event;
use crate::icons::{self, Icon};
use crate::images::ImageCache;
use crate::models::{LiveItem, Source};
use crate::player::Player;
use crate::theme as t;
use crate::util;

pub const MAX_SLOTS: usize = 12;
const GAP: f32 = 5.0;
const MIN_SHARE: f32 = 0.08;
const RETRY_DELAY: Duration = Duration::from_secs(6);
const MAX_AUTO_RETRIES: u8 = 3;

/// Main groups shown in the picker. Keep the labels short enough for the narrow
/// flyout; the id is what the API actually carries on each member row.
const PICKER_GROUPS: [(i64, &str); 5] = [
    (10, "SNH48"),
    (12, "GNZ48"),
    (11, "BEJ48"),
    (14, "CKG48"),
    (21, "CGT48"),
];
const PICKER_TYPES: [(i64, &str); 5] = [
    (1, "视频"),
    (2, "电台"),
    (3, "连麦"),
    (5, "游戏"),
    (6, "AI"),
];

fn picker_group_id(item: &LiveItem) -> i64 {
    if item.group_id != 0 {
        return item.group_id;
    }
    let nick = item.nickname.to_uppercase();
    crate::api::GROUP_IDS
        .iter()
        .find(|(name, _)| {
            nick.starts_with(&name.to_uppercase())
                || item.team_name.to_uppercase().starts_with(&name.to_uppercase())
        })
        .map(|(_, id)| *id)
        .unwrap_or(0)
}

fn picker_group_name(id: i64) -> &'static str {
    PICKER_GROUPS
        .iter()
        .find(|(group, _)| *group == id)
        .map(|(_, name)| *name)
        .unwrap_or("其他团体")
}

fn picker_type_id(item: &LiveItem) -> i64 {
    if item.in_microphone_connection {
        3
    } else {
        item.live_type
    }
}

fn picker_type_name(id: i64) -> &'static str {
    PICKER_TYPES
        .iter()
        .find(|(kind, _)| *kind == id)
        .map(|(_, name)| *name)
        .unwrap_or("其他类型")
}

fn matches_picker_group(item: &LiveItem, group: Option<i64>) -> bool {
    group.map_or(true, |id| picker_group_id(item) == id)
}

fn matches_picker_type(item: &LiveItem, kind: Option<i64>) -> bool {
    kind.map_or(true, |id| picker_type_id(item) == id)
}

/// What the view asks the app to do outside its own state: network work is the
/// app's job because it owns the worker pool and the VLC engine.
pub enum MultiAction {
    Close,
    Add(LiveItem),
    RefreshCandidates,
    Retry(usize),
}

pub enum AddOutcome {
    Added(usize),
    Duplicate,
    Full,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Layout {
    Auto,
    Two,
    Three,
    Focus,
}

impl Layout {
    pub const ALL: [Layout; 4] = [Layout::Auto, Layout::Two, Layout::Three, Layout::Focus];

    pub fn label(self) -> &'static str {
        match self {
            Layout::Auto => "自动",
            Layout::Two => "双列",
            Layout::Three => "三列",
            Layout::Focus => "主画面",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Layout::Auto => "auto",
            Layout::Two => "two",
            Layout::Three => "three",
            Layout::Focus => "focus",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "two" => Layout::Two,
            "three" => Layout::Three,
            "focus" => Layout::Focus,
            _ => Layout::Auto,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SlotLive {
    pub live_id: String,
    pub title: String,
    pub nickname: String,
    pub source: Source,
}

impl SlotLive {
    fn from_item(item: &LiveItem) -> Self {
        SlotLive {
            live_id: item.live_id.clone(),
            title: item.title.clone(),
            nickname: if item.nickname.is_empty() {
                item.sub_title.clone()
            } else {
                item.nickname.clone()
            },
            source: item.source,
        }
    }

    fn from_config(m: &MultiMember) -> Self {
        SlotLive {
            live_id: m.live_id.clone(),
            title: m.title.clone(),
            nickname: m.nickname.clone(),
            source: if m.open { Source::Open } else { Source::Member },
        }
    }

    fn to_config(&self) -> MultiMember {
        MultiMember {
            live_id: self.live_id.clone(),
            title: self.title.clone(),
            nickname: self.nickname.clone(),
            open: self.source == Source::Open,
            volume: 1.0,
        }
    }

    fn display_name(&self) -> &str {
        if self.nickname.is_empty() {
            &self.live_id
        } else {
            &self.nickname
        }
    }
}

pub struct MultiSlot {
    pub info: SlotLive,
    pub player: Option<Arc<Mutex<Player>>>,
    /// Per-tile level, independent of every other room.
    pub volume: f32,
    /// The level to restore when the speaker is clicked while muted.
    last_volume: f32,
    texture: Option<(TextureId, Vec2)>,
    aspect: f32,
    started: bool,
    pub error: Option<String>,
    pub loading: bool,
    retries: u8,
    token: u64,
    retry_at: Option<Instant>,
    volume_applied: bool,
}

impl MultiSlot {
    fn new(info: SlotLive, volume: f32) -> Self {
        let volume = if volume.is_finite() {
            volume.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let last_volume = if volume > 0.001 { volume } else { 1.0 };
        MultiSlot {
            info,
            player: None,
            volume,
            last_volume,
            texture: None,
            aspect: 9.0 / 16.0,
            started: false,
            error: None,
            loading: false,
            retries: 0,
            token: 0,
            retry_at: None,
            volume_applied: false,
        }
    }

    fn set_volume(&mut self, volume: f32) {
        let volume = volume.clamp(0.0, 1.0);
        if volume > 0.001 {
            self.last_volume = volume;
        }
        self.volume = volume;
    }

    /// One click on the speaker flips between the current level and silence.
    /// The previous level survives both a click and a dragged slider down to zero.
    fn toggle_mute(&mut self) {
        if self.volume > 0.001 {
            self.set_volume(0.0);
        } else {
            self.set_volume(self.last_volume.max(0.01));
        }
    }
}

impl Drop for MultiSlot {
    fn drop(&mut self) {
        if let Some(player) = self.player.take() {
            crate::player::retire_player(player);
        }
    }
}

pub struct MultiView {
    pub slots: Vec<MultiSlot>,
    pub layout: Layout,
    pub picker_open: bool,
    pub picker_query: String,
    /// Picker-only filters; they do not change the main list's persisted filters.
    pub picker_group: Option<i64>,
    pub picker_type: Option<i64>,
    pub dirty: bool,

    drag_from: Option<usize>,
    drop_target: Option<usize>,
    col_w: Vec<f32>,
    row_w: Vec<f32>,
    weights_for: Option<(usize, usize, bool)>,
    next_token: u64,
}

impl MultiView {
    pub fn new(cfg: &Config) -> Self {
        let mut slots: Vec<MultiSlot> = Vec::new();
        for m in &cfg.multi_members {
            if m.live_id.is_empty() || slots.iter().any(|s| s.info.live_id == m.live_id) {
                continue;
            }
            slots.push(MultiSlot::new(SlotLive::from_config(m), m.volume));
            if slots.len() == MAX_SLOTS {
                break;
            }
        }
        let picker_open = slots.is_empty();
        let layout = Layout::from_str(&cfg.multi_layout);
        let saved_weights = cfg.multi_weights_layout == layout.as_str();
        MultiView {
            slots,
            layout,
            picker_open,
            picker_query: String::new(),
            picker_group: None,
            picker_type: None,
            dirty: false,
            drag_from: None,
            drop_target: None,
            col_w: if saved_weights {
                cfg.multi_col_w.clone()
            } else {
                Vec::new()
            },
            row_w: if saved_weights {
                cfg.multi_row_w.clone()
            } else {
                Vec::new()
            },
            weights_for: None,
            next_token: 0,
        }
    }

    pub fn sync_config(&self, cfg: &mut Config) {
        cfg.multi_members = self
            .slots
            .iter()
            .map(|s| {
                let mut m = s.info.to_config();
                m.volume = s.volume;
                m
            })
            .collect();
        cfg.multi_layout = self.layout.as_str().to_owned();
        cfg.multi_col_w = self.col_w.clone();
        cfg.multi_row_w = self.row_w.clone();
        cfg.multi_weights_layout = self.layout.as_str().to_owned();
    }

    pub fn frame_limits(&self) -> (u32, u32) {
        match self.slots.len() {
            0..=2 => (960, 540),
            3..=4 => (800, 450),
            _ => (640, 360),
        }
    }

    pub fn slots_to_start(&self) -> Vec<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.player.is_none() && !s.loading)
            .map(|(i, _)| i)
            .collect()
    }

    pub fn add_live(&mut self, item: &LiveItem) -> AddOutcome {
        if self.slots.iter().any(|s| s.info.live_id == item.live_id) {
            return AddOutcome::Duplicate;
        }
        if self.slots.len() >= MAX_SLOTS {
            return AddOutcome::Full;
        }
        let info = SlotLive::from_item(item);
        self.slots.push(MultiSlot::new(info, 1.0));
        self.dirty = true;
        AddOutcome::Added(self.slots.len() - 1)
    }

    pub fn remove(&mut self, index: usize) {
        if index >= self.slots.len() {
            return;
        }
        self.slots.remove(index);
        self.dirty = true;
    }

    /// Drops every tile in one go. Each dropped slot retires its libVLC session;
    /// the layout stays as chosen, but the per-layout gutters no longer apply.
    pub fn clear(&mut self) {
        if self.slots.is_empty() {
            return;
        }
        self.slots.clear();
        self.drag_from = None;
        self.drop_target = None;
        self.col_w.clear();
        self.row_w.clear();
        self.weights_for = None;
        self.dirty = true;
    }

    pub fn set_layout(&mut self, layout: Layout) {
        if self.layout == layout {
            return;
        }
        self.layout = layout;
        self.col_w.clear();
        self.row_w.clear();
        self.weights_for = None;
        self.dirty = true;
    }

    /// Marks a tile as pending and reserves a fresh request token. The app uses
    /// the returned info to resolve the address on a worker thread.
    pub fn begin_open(&mut self, index: usize) -> Option<(u64, SlotLive)> {
        let slot = self.slots.get_mut(index)?;
        if let Some(old) = slot.player.take() {
            crate::player::retire_player(old);
        }
        self.next_token = self.next_token.wrapping_add(1);
        let token = self.next_token;
        slot.token = token;
        slot.loading = true;
        slot.error = None;
        slot.started = false;
        slot.texture = None;
        slot.retry_at = None;
        slot.volume_applied = false;
        slot.retries = slot.retries.saturating_add(1);
        Some((token, slot.info.clone()))
    }

    pub fn fail_open(&mut self, token: u64, error: String) {
        if let Some(slot) = self
            .slots
            .iter_mut()
            .find(|s| s.token == token && s.loading)
        {
            slot.loading = false;
            slot.error = Some(error);
            schedule_retry(slot);
        }
    }

    pub fn accept_player(&mut self, token: u64, player: Arc<Mutex<Player>>) -> bool {
        let Some(index) = self
            .slots
            .iter()
            .position(|s| s.token == token && s.loading)
        else {
            crate::player::retire_player(player);
            return false;
        };
        let slot = &mut self.slots[index];
        if let Some(old) = slot.player.replace(player) {
            crate::player::retire_player(old);
        }
        slot.loading = false;
        slot.error = None;
        slot.retry_at = None;
        slot.volume_applied = false;
        true
    }

    pub fn due_retries(&mut self) -> Vec<usize> {
        let now = Instant::now();
        let mut due = Vec::new();
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot.loading || slot.retries >= MAX_AUTO_RETRIES {
                continue;
            }
            let Some(at) = slot.retry_at else {
                continue;
            };
            if now >= at && slot.error.is_some() {
                slot.retry_at = None;
                due.push(index);
            }
        }
        due
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        candidates: &[LiveItem],
        candidates_loading: bool,
        images: &mut ImageCache,
        tx: &Sender<Event>,
    ) -> Vec<MultiAction> {
        let mut actions = Vec::new();

        if ui.ctx().input(|i| i.key_pressed(Key::Escape)) {
            // Escape closes the member drawer only. Leaving the wall itself needs
            // the explicit 返回 button, so a stray Esc cannot tear down eight
            // running streams.
            self.picker_open = false;
        }

        self.pump(ui.ctx());

        Panel::top("multi_top")
            .frame(Frame::new().fill(t::BG).inner_margin(Margin {
                left: 16,
                right: 12,
                top: 9,
                bottom: 8,
            }))
            .show(ui, |ui| self.top_bar(ui, &mut actions));

        if self.picker_open {
            Panel::right("multi_members")
                .resizable(false)
                .exact_size(330.0)
                .frame(
                    Frame::new()
                        .fill(t::SURFACE)
                        .inner_margin(Margin::symmetric(14, 14)),
                )
                .show(ui, |ui| {
                    self.picker_body(ui, candidates, candidates_loading, images, tx, &mut actions)
                });
        }

        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(Color32::BLACK)
                    .inner_margin(Margin::same(6)),
            )
            .show(ui, |ui| self.grid(ui, &mut actions));

        actions
    }

    fn top_bar(&mut self, ui: &mut Ui, actions: &mut Vec<MultiAction>) {
        ui.horizontal(|ui| {
            if ui
                .add_sized(Vec2::new(64.0, 30.0), t::ghost_button("返回"))
                .on_hover_text("退出多画面，回到列表")
                .clicked()
            {
                actions.push(MultiAction::Close);
            }
            ui.add_space(8.0);
            ui.label(
                RichText::new("多画面")
                    .size(18.0)
                    .color(t::TEXT)
                    .font(t::heading(18.0)),
            );
            ui.label(
                RichText::new(format!("{} / {} 路", self.slots.len(), MAX_SLOTS))
                    .size(11.5)
                    .color(t::TEXT_3),
            )
            .on_hover_text(format!("最多 {MAX_SLOTS} 路，满时上下各六路"));

            ui.add_space(12.0);
            ui.label(RichText::new("声音：全部播放").size(11.5).color(t::TEXT_3))
                .on_hover_text(
                    "每格右下角喇叭悬停后拖动滑块，或按住喇叭上下拖动，单独调整这一路音量",
                );

            ui.with_layout(UiLayout::right_to_left(Align::Center), |ui| {
                self.picker_open = if ui
                    .add_sized(
                        Vec2::new(92.0, 30.0),
                        t::ghost_button(if self.picker_open {
                            "收起成员"
                        } else {
                            "成员"
                        }),
                    )
                    .on_hover_text("选择正在直播的成员")
                    .clicked()
                {
                    !self.picker_open
                } else {
                    self.picker_open
                };

                let mut chosen = self.layout;
                egui::ComboBox::from_id_salt("multi_layout")
                    .width(94.0)
                    .selected_text(
                        RichText::new(format!("布局 · {}", self.layout.label()))
                            .size(12.5)
                            .color(t::TEXT_2),
                    )
                    .show_ui(ui, |ui| {
                        for layout in Layout::ALL {
                            ui.selectable_value(&mut chosen, layout, layout.label());
                        }
                    });
                if chosen != self.layout {
                    self.set_layout(chosen);
                }

                let clear = ui.add_enabled(
                    !self.slots.is_empty(),
                    t::ghost_button("清空").min_size(Vec2::new(64.0, 30.0)),
                );
                if clear
                    .on_hover_text("关闭全部画面，回到空的多画面")
                    .clicked()
                {
                    self.clear();
                }
            });
        });
    }

    fn picker_body(
        &mut self,
        ui: &mut Ui,
        candidates: &[LiveItem],
        loading: bool,
        images: &mut ImageCache,
        tx: &Sender<Event>,
        actions: &mut Vec<MultiAction>,
    ) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("添加成员")
                    .size(15.0)
                    .color(t::TEXT)
                    .font(t::heading(15.0)),
            );
            ui.with_layout(UiLayout::right_to_left(Align::Center), |ui| {
                if icons::icon_button(ui, Icon::Close, 28.0)
                    .on_hover_text("收起成员列表")
                    .clicked()
                {
                    self.picker_open = false;
                }
            });
        });
        ui.add_space(4.0);
        ui.label(
            RichText::new("只列出当前正在直播的成员；点加入后会立刻开一路画面。")
                .size(11.5)
                .color(t::TEXT_3),
        );
        ui.add_space(6.0);

        let frame = Frame::new()
            .fill(t::FIELD)
            .corner_radius(7.0)
            .inner_margin(Margin::symmetric(8, 4))
            .stroke(Stroke::new(1.0, t::LINE));
        ui.add(
            egui::TextEdit::singleline(&mut self.picker_query)
                .hint_text("搜索昵称、标题或队伍")
                .desired_width(f32::INFINITY)
                .frame(frame),
        );
        ui.add_space(5.0);

        ui.horizontal(|ui| {
            ui.label(RichText::new("团体").size(11.5).color(t::TEXT_3));
            let group_text = self
                .picker_group
                .map(picker_group_name)
                .unwrap_or("全部团体");
            egui::ComboBox::from_id_salt("multi_group_combo")
                .width(92.0)
                .selected_text(RichText::new(group_text).size(12.0).color(t::TEXT_2))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.picker_group, None, "全部团体");
                    for (id, name) in PICKER_GROUPS {
                        ui.selectable_value(&mut self.picker_group, Some(id), name);
                    }
                });
            ui.add_space(6.0);
            ui.label(RichText::new("类型").size(11.5).color(t::TEXT_3));
            let type_text = self
                .picker_type
                .map(picker_type_name)
                .unwrap_or("全部类型");
            egui::ComboBox::from_id_salt("multi_type_combo")
                .width(78.0)
                .selected_text(RichText::new(type_text).size(12.0).color(t::TEXT_2))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.picker_type, None, "全部类型");
                    for (id, name) in PICKER_TYPES {
                        ui.selectable_value(&mut self.picker_type, Some(id), name);
                    }
                });
        });
        ui.add_space(4.0);

        let group = self.picker_group;
        let kind = self.picker_type;
        let filtering = group.is_some() || kind.is_some();

        ui.horizontal(|ui| {
            if loading {
                ui.add(egui::Spinner::new().size(13.0));
                ui.label(RichText::new("正在刷新…").size(11.5).color(t::TEXT_3));
            } else {
                let available = candidates
                    .iter()
                    .filter(|i| {
                        i.source == Source::Member
                            && i.live_mode == 0
                            && matches_picker_group(i, group)
                            && matches_picker_type(i, kind)
                    })
                    .count();
                ui.label(
                    RichText::new(if filtering {
                        format!("筛出 {available} 个")
                    } else {
                        format!("{available} 个在播")
                    })
                    .size(11.5)
                    .color(t::TEXT_3),
                );
            }
            ui.with_layout(UiLayout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_sized(Vec2::new(58.0, 26.0), t::ghost_button("刷新"))
                    .clicked()
                {
                    actions.push(MultiAction::RefreshCandidates);
                }
            });
        });
        ui.add_space(4.0);

        let query = self.picker_query.trim().to_lowercase();
        let mut shown = 0usize;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for item in candidates {
                    if item.source != Source::Member
                        || item.live_mode != 0
                        || !matches_picker_group(item, group)
                        || !matches_picker_type(item, kind)
                    {
                        continue;
                    }
                    let name = if item.nickname.is_empty() {
                        &item.sub_title
                    } else {
                        &item.nickname
                    };
                    if !query.is_empty()
                        && !format!(
                            "{} {} {}",
                            name.to_lowercase(),
                            item.title.to_lowercase(),
                            item.team_name.to_lowercase()
                        )
                        .contains(&query)
                    {
                        continue;
                    }
                    shown += 1;
                    let added = self.slots.iter().any(|s| s.info.live_id == item.live_id);
                    let mut add = false;
                    Frame::new()
                        .fill(t::CARD)
                        .corner_radius(9.0)
                        .inner_margin(Margin::same(9))
                        .stroke(Stroke::new(1.0, t::LINE_SOFT))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let (avatar, _) =
                                    ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
                                let avatar_url = if item.avatar.is_empty() {
                                    String::new()
                                } else {
                                    crate::api::resolve_image(&item.avatar)
                                };
                                if !images.avatar_at(ui, &avatar_url, avatar, tx) {
                                    let initial = name.chars().next().unwrap_or('?').to_string();
                                    ui.painter().text(
                                        avatar.center(),
                                        Align2::CENTER_CENTER,
                                        initial,
                                        t::heading(13.0),
                                        t::TEXT,
                                    );
                                }
                                ui.add_space(2.0);
                                ui.vertical(|ui| {
                                    ui.label(
                                        RichText::new(util::truncate(
                                            name,
                                            ui.available_width().max(60.0) - 18.0,
                                            12.5,
                                        ))
                                        .size(12.5)
                                        .color(t::TEXT)
                                        .font(t::heading(12.5)),
                                    );
                                    let sub = if item.title.is_empty() {
                                        item.team_name.clone()
                                    } else {
                                        format!("{} · {}", item.title, item.team_name)
                                    };
                                    ui.label(
                                        RichText::new(util::truncate(
                                            &sub,
                                            ui.available_width().max(60.0) - 8.0,
                                            10.5,
                                        ))
                                        .size(10.5)
                                        .color(t::TEXT_3),
                                    );
                                });
                                ui.with_layout(UiLayout::right_to_left(Align::Center), |ui| {
                                    if added {
                                        ui.label(
                                            RichText::new("已加入").size(11.0).color(t::TEXT_3),
                                        );
                                    } else if self.slots.len() >= MAX_SLOTS {
                                        ui.label(
                                            RichText::new("已满").size(11.0).color(t::TEXT_3),
                                        );
                                    } else if ui
                                        .add_sized(Vec2::new(52.0, 26.0), t::ghost_button("加入"))
                                        .clicked()
                                    {
                                        add = true;
                                    }
                                });
                            });
                        });
                    if add {
                        actions.push(MultiAction::Add(item.clone()));
                    }
                    ui.add_space(6.0);
                }
                if shown == 0 {
                    ui.add_space(20.0);
                    ui.vertical_centered(|ui| {
                        let plain_empty = query.is_empty() && !filtering;
                        ui.label(
                            RichText::new(if plain_empty {
                                "还没有正在直播的成员"
                            } else {
                                "没有匹配的成员"
                            })
                            .size(12.0)
                            .color(t::TEXT_3),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(if plain_empty {
                                "点右上角刷新再试"
                            } else {
                                "换个团体、类型或搜索词"
                            })
                            .size(11.0)
                            .color(t::TEXT_3),
                        );
                    });
                }
            });
    }

    fn grid(&mut self, ui: &mut Ui, actions: &mut Vec<MultiAction>) {
        let rect = ui.available_rect_before_wrap();
        ui.allocate_rect(rect, Sense::hover());
        ui.painter().rect_filled(rect, 0.0, t::BG);

        if self.slots.is_empty() {
            let hit = ui.interact(rect, Id::new("multi_empty_first"), Sense::click());
            let color = if hit.hovered() { t::TEXT_2 } else { t::TEXT_3 };
            ui.painter().rect_stroke(
                rect.shrink(1.0),
                10.0,
                Stroke::new(1.0, t::LINE),
                StrokeKind::Inside,
            );
            let plus =
                Rect::from_center_size(rect.center() - Vec2::new(0.0, 18.0), Vec2::splat(44.0));
            icons::draw(ui, Icon::Plus, plus, color);
            ui.painter().text(
                rect.center() + Vec2::new(0.0, 24.0),
                Align2::CENTER_CENTER,
                "添加成员，同屏看多路直播",
                t::heading(15.0),
                color,
            );
            ui.painter().text(
                rect.center() + Vec2::new(0.0, 48.0),
                Align2::CENTER_CENTER,
                "拖动画面的位置可以交换，拖动画面之间的缝隙可以改变宽高",
                egui::FontId::proportional(11.5),
                t::TEXT_3,
            );
            if hit.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                self.picker_open = true;
            }
            return;
        }

        let geo = self.geometry(rect);

        // Keep the drop highlight one frame current while a drag is live.
        self.update_drop_target(&geo, ui.ctx());

        let mut remove_req = None;
        let mut retry_req = None;
        let mut drag_started = false;
        let mut drag_stopped = false;
        let mut volume_active = false;

        for cell in &geo.cells {
            let index = cell.index;
            let hit = ui
                .interact(
                    cell.rect,
                    Id::new(("multi_tile", index)),
                    Sense::click_and_drag(),
                )
                .on_hover_cursor(CursorIcon::PointingHand);
            let drop_target = self.drop_target == Some(index);
            let result = paint_tile(
                ui,
                &self.slots[index],
                index,
                cell.rect,
                &hit,
                drop_target,
                self.drag_from == Some(index),
            );
            if result.remove {
                remove_req = Some(index);
            } else if result.retry {
                retry_req = Some(index);
            }
            if result.toggle_mute {
                self.slots[index].toggle_mute();
                self.apply_volume(index);
                self.dirty = true;
            }
            if let Some(volume) = result.volume {
                let changed = (self.slots[index].volume - volume).abs() > 0.002;
                if changed {
                    self.slots[index].set_volume(volume);
                    self.apply_volume(index);
                }
            }
            if result.volume_commit {
                self.dirty = true;
            }
            volume_active |= result.volume_active;
            drag_started |= hit.drag_started();
            drag_stopped |= hit.drag_stopped();
        }

        for empty in &geo.empty {
            let hit = ui
                .interact(
                    *empty,
                    Id::new(("multi_empty", empty.min.x as i32, empty.min.y as i32)),
                    Sense::click(),
                )
                .on_hover_cursor(CursorIcon::PointingHand);
            paint_empty_tile(ui, *empty, &hit);
            if hit.clicked() {
                self.picker_open = true;
            }
        }

        self.draw_splits(ui, &geo.splits);

        if drag_started && !volume_active {
            if let Some(pos) = ui.ctx().input(|i| i.pointer.interact_pos()) {
                if let Some(cell) = geo.cells.iter().find(|c| c.rect.contains(pos)) {
                    self.drag_from = Some(cell.index);
                }
            }
        }
        if drag_stopped {
            if let (Some(from), Some(to)) = (self.drag_from, self.drop_target) {
                if from != to {
                    self.slots.swap(from, to);
                    self.dirty = true;
                }
            }
            self.drag_from = None;
            self.drop_target = None;
        }
        if self.drag_from.is_some() {
            ui.ctx().request_repaint();
            if let Some(pos) = ui.ctx().input(|i| i.pointer.interact_pos()) {
                let text = "拖到另一格交换位置";
                let painter = ui.painter();
                let pos = pos + Vec2::new(14.0, 16.0);
                let galley = painter.layout_no_wrap(
                    text.to_owned(),
                    egui::FontId::proportional(12.0),
                    t::TEXT,
                );
                let bg = Rect::from_min_size(
                    pos - Vec2::new(6.0, 4.0),
                    galley.size() + Vec2::new(12.0, 8.0),
                );
                painter.rect_filled(bg, 5.0, t::alpha(Color32::from_rgb(12, 12, 12), 225));
                painter.galley(pos, galley, t::TEXT);
            }
        }

        if let Some(index) = remove_req {
            self.remove(index);
            self.drag_from = None;
            self.drop_target = None;
        } else if let Some(index) = retry_req {
            if let Some(slot) = self.slots.get_mut(index) {
                slot.retries = 0;
                slot.retry_at = None;
            }
            actions.push(MultiAction::Retry(index));
        }
    }

    fn update_drop_target(&mut self, geo: &Geometry, ctx: &Context) {
        if self.drag_from.is_none() {
            self.drop_target = None;
            return;
        }
        let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) else {
            return;
        };
        self.drop_target = geo
            .cells
            .iter()
            .find(|c| c.rect.contains(pos))
            .map(|c| c.index);
    }

    fn pump(&mut self, ctx: &Context) {
        let now = Instant::now();
        for slot in &mut self.slots {
            let Some(player) = slot.player.clone() else {
                continue;
            };
            let mut p = player
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            p.pump(ctx, ViewportId::ROOT);
            let started = p.is_started();
            if started && !slot.volume_applied {
                p.set_volume(slot.volume);
                slot.volume_applied = true;
            }
            slot.texture = p.texture_id();
            slot.aspect = p.aspect();
            slot.started = started;
            let error = p.error.clone();
            drop(p);
            slot.error = error;
            if let Some(err) = &slot.error {
                if err.contains("结束") {
                    slot.retry_at = None;
                } else if slot.retry_at.is_none() && slot.retries < MAX_AUTO_RETRIES {
                    slot.retry_at = Some(now + RETRY_DELAY);
                }
            } else {
                slot.retry_at = None;
            }
        }
        if !self.slots.is_empty() {
            ctx.request_repaint_after_for(Duration::from_millis(33), ViewportId::ROOT);
        }
    }

    fn apply_volume(&self, index: usize) {
        let Some(slot) = self.slots.get(index) else {
            return;
        };
        if !slot.started {
            return;
        }
        let Some(player) = &slot.player else {
            return;
        };
        let mut p = player
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        p.set_volume(slot.volume);
    }

    /// The aspect a member wall should use. Member rooms are almost always
    /// portrait, so the auto layout keeps the tiles portrait too instead of
    /// leaving wide letterbox bars around every face.
    fn auto_aspect(&self) -> f32 {
        let mut aspects: Vec<f32> = self
            .slots
            .iter()
            .filter(|slot| {
                slot.started && slot.aspect.is_finite() && slot.aspect > 0.1 && slot.aspect < 10.0
            })
            .map(|slot| slot.aspect)
            .collect();
        if aspects.is_empty() {
            return 9.0 / 16.0;
        }
        aspects.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        aspects[aspects.len() / 2]
    }

    /// Auto keeps the live aspect. Up to four rooms share one row; from five
    /// rooms on it always uses a two-row wall with at most six columns, so
    /// twelve rooms land as six on top and six below.
    fn auto_geometry(&self, rect: Rect) -> Geometry {
        let count = self.slots.len().max(1);
        let ar = self.auto_aspect().max(0.1);
        let (rows, cols) = if count <= 4 {
            (1, count)
        } else {
            (2, count.div_ceil(2).min(MAX_SLOTS))
        };
        let cell_w = ((rect.width() - GAP * (cols as f32 - 1.0)) / cols as f32).max(1.0);
        let cell_h = ((rect.height() - GAP * (rows as f32 - 1.0)) / rows as f32).max(1.0);
        let tile_w = cell_w.min(cell_h * ar).max(1.0);
        let tile_h = tile_w / ar;

        let group_h = tile_h * rows as f32 + GAP * (rows as f32 - 1.0);
        let y0 = rect.center().y - group_h * 0.5;

        let mut cells = Vec::with_capacity(self.slots.len());
        for i in 0..self.slots.len() {
            let row = i / cols;
            let col = i % cols;
            let count_in_row = (self.slots.len() - row * cols).min(cols);
            let row_w = tile_w * count_in_row as f32 + GAP * (count_in_row as f32 - 1.0);
            let row_x0 = rect.center().x - row_w * 0.5;
            cells.push(SlotCell {
                index: i,
                rect: Rect::from_min_size(
                    Pos2::new(
                        row_x0 + (tile_w + GAP) * col as f32,
                        y0 + (tile_h + GAP) * row as f32,
                    ),
                    Vec2::new(tile_w, tile_h),
                ),
            });
        }

        Geometry {
            cells,
            empty: Vec::new(),
            splits: Vec::new(),
        }
    }

    fn geometry(&mut self, rect: Rect) -> Geometry {
        if self.layout == Layout::Auto {
            return self.auto_geometry(rect);
        }
        let (cols, rows, focus_layout) = self.shape();
        self.ensure_weights(cols, rows, focus_layout);

        let col_spans = spans(&self.col_w, rect.min.x, rect.width(), GAP);
        let row_spans = spans(&self.row_w, rect.min.y, rect.height(), GAP);
        let mut cells = Vec::new();
        let mut empty = Vec::new();
        let mut splits = Vec::new();

        if focus_layout {
            let left = Rect::from_min_max(
                Pos2::new(col_spans[0].0, rect.min.y),
                Pos2::new(col_spans[0].1, rect.max.y),
            );
            if !self.slots.is_empty() {
                cells.push(SlotCell {
                    index: 0,
                    rect: left,
                });
            }
            let right_x0 = col_spans[1].0;
            let right_x1 = col_spans[1].1;
            for (i, span) in row_spans.iter().enumerate() {
                if let Some(slot_index) = i.checked_add(1) {
                    if slot_index < self.slots.len() {
                        cells.push(SlotCell {
                            index: slot_index,
                            rect: Rect::from_min_max(
                                Pos2::new(right_x0, span.0),
                                Pos2::new(right_x1, span.1),
                            ),
                        });
                    }
                }
            }
            splits.push(Split {
                axis: SplitAxis::Col,
                index: 0,
                rect: Rect::from_min_max(
                    Pos2::new(col_spans[0].1 + GAP * 0.5 - 3.5, rect.min.y),
                    Pos2::new(col_spans[0].1 + GAP * 0.5 + 3.5, rect.max.y),
                ),
                usable: rect.width() - GAP,
            });
            for i in 0..row_spans.len().saturating_sub(1) {
                let y = row_spans[i].1 + GAP * 0.5;
                splits.push(Split {
                    axis: SplitAxis::Row,
                    index: i,
                    rect: Rect::from_min_max(
                        Pos2::new(right_x0, y - 3.5),
                        Pos2::new(right_x1, y + 3.5),
                    ),
                    usable: rect.height() - GAP * (row_spans.len() as f32 - 1.0),
                });
            }
            return Geometry {
                cells,
                empty,
                splits,
            };
        }

        for row in 0..rows {
            for col in 0..cols {
                let index = row * cols + col;
                let cell = Rect::from_min_max(
                    Pos2::new(col_spans[col].0, row_spans[row].0),
                    Pos2::new(col_spans[col].1, row_spans[row].1),
                );
                if index < self.slots.len() {
                    cells.push(SlotCell { index, rect: cell });
                } else {
                    empty.push(cell);
                }
            }
        }

        for col in 0..cols.saturating_sub(1) {
            let x = col_spans[col].1 + GAP * 0.5;
            splits.push(Split {
                axis: SplitAxis::Col,
                index: col,
                rect: Rect::from_min_max(
                    Pos2::new(x - 3.5, rect.min.y),
                    Pos2::new(x + 3.5, rect.max.y),
                ),
                usable: rect.width() - GAP * (cols as f32 - 1.0),
            });
        }
        for row in 0..rows.saturating_sub(1) {
            let y = row_spans[row].1 + GAP * 0.5;
            splits.push(Split {
                axis: SplitAxis::Row,
                index: row,
                rect: Rect::from_min_max(
                    Pos2::new(rect.min.x, y - 3.5),
                    Pos2::new(rect.max.x, y + 3.5),
                ),
                usable: rect.height() - GAP * (rows as f32 - 1.0),
            });
        }

        Geometry {
            cells,
            empty,
            splits,
        }
    }

    fn shape(&self) -> (usize, usize, bool) {
        let n = self.slots.len();
        if n == 0 {
            return (1, 1, false);
        }
        match self.layout {
            Layout::Auto if n == 1 => (1, 1, false),
            Layout::Auto if n == 2 => (2, 1, false),
            Layout::Auto if n == 3 => (3, 1, false),
            Layout::Auto if n <= 4 => (2, 2, false),
            Layout::Auto => (3, n.div_ceil(3), false),
            Layout::Two => (2, n.div_ceil(2), false),
            Layout::Three => (3, n.div_ceil(3), false),
            Layout::Focus if n == 1 => (1, 1, false),
            Layout::Focus => (2, n - 1, true),
        }
    }

    fn ensure_weights(&mut self, cols: usize, rows: usize, focus: bool) {
        if self.weights_for == Some((cols, rows, focus))
            && self.col_w.len() == cols
            && self.row_w.len() == rows
        {
            return;
        }
        // On the first frame after a launch, accept shares stored in config if
        // they match this shape. They are normalized again in case an older
        // file was written by hand.
        let saved = self.weights_for.is_none()
            && self.col_w.len() == cols
            && self.row_w.len() == rows
            && self.col_w.iter().all(|w| w.is_finite() && *w >= MIN_SHARE)
            && self.row_w.iter().all(|w| w.is_finite() && *w >= MIN_SHARE);
        if saved {
            let cs: f32 = self.col_w.iter().sum::<f32>().max(0.001);
            let rs: f32 = self.row_w.iter().sum::<f32>().max(0.001);
            for w in &mut self.col_w {
                *w /= cs;
            }
            for w in &mut self.row_w {
                *w /= rs;
            }
            self.weights_for = Some((cols, rows, focus));
            return;
        }
        self.col_w = if focus {
            vec![0.62, 0.38]
        } else {
            vec![1.0 / cols as f32; cols]
        };
        self.row_w = vec![1.0 / rows.max(1) as f32; rows.max(1)];
        self.weights_for = Some((cols, rows, focus));
    }

    fn draw_splits(&mut self, ui: &mut Ui, splits: &[Split]) {
        for split in splits {
            let response = ui
                .interact(
                    split.rect,
                    Id::new(("multi_split", split.axis, split.index)),
                    Sense::drag(),
                )
                .on_hover_cursor(match split.axis {
                    SplitAxis::Col => CursorIcon::ResizeHorizontal,
                    SplitAxis::Row => CursorIcon::ResizeVertical,
                });
            let hot = response.hovered() || response.dragged();
            let width = if hot { 2.0 } else { 1.0 };
            let color = if hot { t::TEXT_2 } else { t::LINE };
            match split.axis {
                SplitAxis::Col => {
                    let x = split.rect.center().x;
                    ui.painter().rect_filled(
                        Rect::from_min_max(
                            Pos2::new(x - width * 0.5, split.rect.min.y),
                            Pos2::new(x + width * 0.5, split.rect.max.y),
                        ),
                        0.0,
                        color,
                    );
                }
                SplitAxis::Row => {
                    let y = split.rect.center().y;
                    ui.painter().rect_filled(
                        Rect::from_min_max(
                            Pos2::new(split.rect.min.x, y - width * 0.5),
                            Pos2::new(split.rect.max.x, y + width * 0.5),
                        ),
                        0.0,
                        color,
                    );
                }
            }

            if response.dragged() {
                let delta = response.drag_delta();
                let usable = split.usable.max(1.0);
                let d = match split.axis {
                    SplitAxis::Col => delta.x / usable,
                    SplitAxis::Row => delta.y / usable,
                };
                let weights = match split.axis {
                    SplitAxis::Col => &mut self.col_w,
                    SplitAxis::Row => &mut self.row_w,
                };
                if split.index + 1 < weights.len() {
                    let a = weights[split.index];
                    let b = weights[split.index + 1];
                    let d = d.clamp(-(a - MIN_SHARE), b - MIN_SHARE);
                    weights[split.index] = a + d;
                    weights[split.index + 1] = b - d;
                    self.dirty = true;
                    ui.ctx().request_repaint();
                }
            }
        }
    }
}

struct SlotCell {
    index: usize,
    rect: Rect,
}

struct Geometry {
    cells: Vec<SlotCell>,
    empty: Vec<Rect>,
    splits: Vec<Split>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum SplitAxis {
    Col,
    Row,
}

struct Split {
    axis: SplitAxis,
    index: usize,
    rect: Rect,
    usable: f32,
}

struct TileHit {
    remove: bool,
    retry: bool,
    volume: Option<f32>,
    toggle_mute: bool,
    volume_active: bool,
    volume_commit: bool,
}

fn paint_tile(
    ui: &mut Ui,
    slot: &MultiSlot,
    index: usize,
    rect: Rect,
    hit: &Response,
    drop_target: bool,
    dragging: bool,
) -> TileHit {
    let painter = ui.painter().clone();
    painter.rect_filled(rect, 6.0, Color32::BLACK);
    let mut volume = slot.volume;
    let mut volume_change = None;
    let mut volume_active = false;
    let mut volume_commit = false;
    let mut toggle_mute = false;

    if slot.started {
        if let Some((texture, _)) = slot.texture {
            ui.painter().image(
                texture,
                rect,
                cover_uv(slot.aspect, rect.size()),
                Color32::WHITE,
            );
        }
    }

    if dragging {
        painter.rect_filled(rect, 6.0, t::alpha(Color32::from_rgb(8, 8, 8), 150));
    }

    let close_rect = Rect::from_center_size(
        Pos2::new(rect.max.x - 18.0, rect.min.y + 18.0),
        Vec2::splat(26.0),
    );
    let close = ui
        .interact(close_rect, Id::new(("multi_close", index)), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);

    if hit.hovered() || close.hovered() {
        ui.painter().circle_filled(
            close_rect.center(),
            11.0,
            t::alpha(Color32::from_rgb(10, 10, 10), 190),
        );
        icons::draw(
            ui,
            Icon::Close,
            close_rect.shrink(7.0),
            if close.hovered() { t::WHITE } else { t::TEXT_2 },
        );
    }

    let status = if let Some(err) = &slot.error {
        Some(err.clone())
    } else if slot.player.is_none() || !slot.started {
        Some(if slot.loading {
            "正在打开直播…".to_owned()
        } else if slot.player.is_some() {
            "正在缓冲…".to_owned()
        } else {
            "等待连接…".to_owned()
        })
    } else {
        None
    };
    if let Some(status) = status {
        let pill = Rect::from_center_size(
            rect.center(),
            Vec2::new(
                (rect.width() - 24.0).clamp(70.0, 360.0),
                26.0f32.min(rect.height() * 0.4),
            ),
        );
        ui.painter()
            .rect_filled(pill, 7.0, t::alpha(Color32::from_rgb(8, 8, 8), 185));
        ui.painter().text(
            pill.center(),
            Align2::CENTER_CENTER,
            util::truncate(&status, pill.width() - 18.0, 11.5),
            egui::FontId::proportional(11.5),
            if slot.error.is_some() {
                t::ERR
            } else {
                t::TEXT_2
            },
        );
    }

    if slot.error.is_some() {
        let hint = Rect::from_center_size(
            rect.center() + Vec2::new(0.0, 34.0),
            Vec2::new((rect.width() - 24.0).min(320.0), 24.0),
        );
        ui.painter().text(
            hint.center(),
            Align2::CENTER_CENTER,
            "点击画面重试",
            egui::FontId::proportional(11.0),
            t::TEXT_3,
        );
    }

    let show_meta = rect.height() > 74.0;
    if show_meta {
        let strip_h = 38.0f32.min(rect.height() * 0.5);
        let strip = Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - strip_h), rect.max);
        ui.painter()
            .rect_filled(strip, 0.0, t::alpha(Color32::from_rgb(6, 6, 6), 185));
        let name = slot.info.display_name();
        let name_size = 12.5;
        let name_w = (rect.width() - 42.0).max(40.0);
        ui.painter().text(
            Pos2::new(rect.min.x + 10.0, strip.min.y + 12.0),
            Align2::LEFT_CENTER,
            util::truncate(name, name_w, name_size),
            t::heading(name_size),
            t::TEXT,
        );
        let subtitle = if slot.info.title.is_empty() {
            "直播中".to_owned()
        } else {
            slot.info.title.clone()
        };
        ui.painter().text(
            Pos2::new(rect.min.x + 10.0, strip.min.y + 27.0),
            Align2::LEFT_CENTER,
            util::truncate(&subtitle, name_w, 10.5),
            egui::FontId::proportional(10.5),
            t::TEXT_3,
        );

        let vol_rect = Rect::from_center_size(
            Pos2::new(rect.max.x - 18.0, strip.center().y),
            Vec2::splat(24.0),
        );
        let vol_hit = ui
            .interact(
                vol_rect,
                Id::new(("multi_vol", index)),
                Sense::click_and_drag(),
            )
            .on_hover_cursor(CursorIcon::PointingHand);

        let pop_w = 146.0f32.min(rect.width() - 16.0).max(96.0);
        let pop_h = 28.0;
        let pop = Rect::from_min_size(
            Pos2::new(
                (rect.max.x - 8.0 - pop_w).max(rect.min.x + 8.0),
                (strip.min.y - 8.0 - pop_h).max(rect.min.y + 8.0),
            ),
            Vec2::new(pop_w, pop_h),
        );
        // The gap between the popover and the speaker is part of its hover area,
        // so moving the pointer up from the icon keeps the slider open.
        let pop_hot =
            Rect::from_min_max(pop.min, Pos2::new(pop.max.x, vol_rect.min.y.max(pop.max.y)));
        let pop_hit = ui.interact(
            pop_hot,
            Id::new(("multi_vol_pop", index)),
            Sense::click_and_drag(),
        );
        let open = vol_hit.hovered() || vol_hit.dragged() || pop_hit.hovered() || pop_hit.dragged();
        volume_active = open;
        if open {
            let track = Rect::from_min_size(
                Pos2::new(pop.min.x + 10.0, pop.center().y - 1.5),
                Vec2::new((pop_w - 54.0).max(40.0), 3.0),
            );
            if vol_hit.dragged() {
                let dy = ui.ctx().input(|i| i.pointer.delta().y);
                volume = ((volume - dy * 0.006).clamp(0.0, 1.0) * 100.0).round() / 100.0;
                volume_change = Some(volume);
                volume_commit = vol_hit.drag_stopped();
            } else if pop_hit.dragged() || pop_hit.clicked() {
                if let Some(pos) = pop_hit.interact_pointer_pos() {
                    volume = ((pos.x - track.min.x) / track.width()).clamp(0.0, 1.0);
                    volume = (volume * 100.0).round() / 100.0;
                    volume_change = Some(volume);
                    volume_commit = pop_hit.drag_stopped() || pop_hit.clicked();
                }
            }
            ui.painter()
                .rect_filled(pop, 8.0, t::alpha(Color32::from_rgb(8, 8, 8), 225));
            ui.painter().rect_filled(track, 1.5, t::alpha(t::WHITE, 38));
            ui.painter().rect_filled(
                Rect::from_min_size(track.min, Vec2::new(track.width() * volume, track.height())),
                1.5,
                t::WHITE,
            );
            ui.painter().circle_filled(
                Pos2::new(track.min.x + track.width() * volume, track.center().y),
                4.5,
                t::WHITE,
            );
            ui.painter().text(
                Pos2::new(pop.max.x - 10.0, pop.center().y),
                Align2::RIGHT_CENTER,
                format!("{}%", (volume * 100.0).round() as i32),
                egui::FontId::proportional(10.5),
                t::TEXT_3,
            );
        }

        let muted = volume <= 0.001;
        icons::draw(
            ui,
            if muted {
                Icon::SpeakerMute
            } else {
                Icon::Speaker
            },
            vol_rect.shrink(4.0),
            if open || vol_hit.hovered() {
                t::WHITE
            } else {
                t::TEXT_3
            },
        );
        if vol_hit.drag_stopped() || pop_hit.drag_stopped() {
            volume_commit = true;
        }
        toggle_mute = vol_hit.clicked();
    }

    if drop_target {
        painter.rect_stroke(
            rect.shrink(1.0),
            6.0,
            Stroke::new(2.0, t::OK),
            StrokeKind::Inside,
        );
    } else if hit.hovered() {
        painter.rect_stroke(
            rect.shrink(1.0),
            6.0,
            Stroke::new(1.0, t::alpha(t::WHITE, 80)),
            StrokeKind::Inside,
        );
    }

    let remove = close.clicked();
    let clicked = hit.clicked() && !remove && !volume_active;
    TileHit {
        remove,
        retry: clicked && slot.error.is_some(),
        volume: volume_change,
        toggle_mute,
        volume_active,
        volume_commit,
    }
}

fn paint_empty_tile(ui: &mut Ui, rect: Rect, hit: &Response) {
    ui.painter().rect_filled(rect, 6.0, t::SURFACE);
    ui.painter().rect_stroke(
        rect.shrink(1.0),
        6.0,
        Stroke::new(1.0, if hit.hovered() { t::LINE } else { t::LINE_SOFT }),
        StrokeKind::Inside,
    );
    let color = if hit.hovered() { t::TEXT_2 } else { t::TEXT_3 };
    icons::draw(
        ui,
        Icon::Plus,
        Rect::from_center_size(rect.center() - Vec2::new(0.0, 10.0), Vec2::splat(28.0)),
        color,
    );
    ui.painter().text(
        rect.center() + Vec2::new(0.0, 20.0),
        Align2::CENTER_CENTER,
        "添加成员",
        t::heading(11.5),
        color,
    );
}

fn schedule_retry(slot: &mut MultiSlot) {
    if slot.retries < MAX_AUTO_RETRIES {
        slot.retry_at = Some(Instant::now() + RETRY_DELAY);
    }
}

fn cover_uv(ar: f32, dest: Vec2) -> Rect {
    let full = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
    if !ar.is_finite() || ar <= 0.0 || dest.x <= 0.0 || dest.y <= 0.0 {
        return full;
    }
    let dest_ar = dest.x / dest.y;
    if ar > dest_ar {
        let keep = dest_ar / ar;
        let side = (1.0 - keep) * 0.5;
        Rect::from_min_max(Pos2::new(side, 0.0), Pos2::new(1.0 - side, 1.0))
    } else {
        let keep = ar / dest_ar;
        let top = (1.0 - keep) * 0.5;
        Rect::from_min_max(Pos2::new(0.0, top), Pos2::new(1.0, 1.0 - top))
    }
}

fn spans(weights: &[f32], start: f32, total: f32, gap: f32) -> Vec<(f32, f32)> {
    let n = weights.len().max(1);
    let usable = (total - gap * (n as f32 - 1.0)).max(1.0);
    let sum: f32 = weights.iter().copied().sum::<f32>().max(0.001);
    let mut out = Vec::with_capacity(weights.len());
    let mut cursor = start;
    for w in weights {
        let len = (usable * (*w / sum)).max(1.0);
        out.push((cursor, cursor + len));
        cursor += len + gap;
    }
    if out.is_empty() {
        out.push((start, start + total));
    }
    out
}

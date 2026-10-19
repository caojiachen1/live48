use egui::{Color32, Pos2, Rect, Response, Shape, Stroke, Ui, Vec2};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Play,
    Pause,
    Stop,
    Close,
    Refresh,
    Filter,
    Settings,
    Download,
    Copy,
    List,
    Pip,
    PipExit,
    Search,
    Check,
    ChevronDown,
    ChevronUp,
    Dot,
    Speaker,
    SpeakerMute,
    Sliders,
}

/// A capsule between two points, with round terminals.
fn bar(p0: Pos2, p1: Pos2, w: f32, color: Color32) -> Vec<Shape> {
    let d = p1 - p0;
    let len = d.length().max(0.0001);
    let half = d * (w * 0.5 / len);
    let n = Vec2::new(-half.y, half.x);
    vec![
        Shape::convex_polygon(
            vec![p0 + n, p1 + n, p1 - n, p0 - n],
            color,
            egui::Stroke::NONE,
        ),
        Shape::circle_filled(p0, w * 0.5, color),
        Shape::circle_filled(p1, w * 0.5, color),
    ]
}

fn polyline(pts: &[Pos2], w: f32, color: Color32, closed: bool) -> Vec<Shape> {
    let mut out = Vec::new();
    let n = pts.len();
    for i in 0..n.saturating_sub(1) {
        out.extend(bar(pts[i], pts[i + 1], w, color));
    }
    if closed && n > 2 {
        out.extend(bar(pts[n - 1], pts[0], w, color));
    }
    out
}

fn speaker_body(rect: Rect, color: Color32) -> Shape {
    let c = rect.center();
    Shape::convex_polygon(
        vec![
            Pos2::new(rect.min.x, c.y - rect.height() * 0.16),
            Pos2::new(rect.min.x + rect.width() * 0.26, c.y - rect.height() * 0.16),
            Pos2::new(rect.min.x + rect.width() * 0.5, rect.min.y),
            Pos2::new(rect.min.x + rect.width() * 0.5, rect.max.y),
            Pos2::new(rect.min.x + rect.width() * 0.26, c.y + rect.height() * 0.16),
            Pos2::new(rect.min.x, c.y + rect.height() * 0.16),
        ],
        color,
        egui::Stroke::NONE,
    )
}

pub fn draw(ui: &Ui, icon: Icon, rect: Rect, color: Color32) {
    let w = (rect.width().min(rect.height())) * 0.11 + 0.9;
    let c = rect.center();
    let (x0, x1) = (rect.min.x, rect.max.x);
    let (y0, y1) = (rect.min.y, rect.max.y);
    let shapes: Vec<Shape> = match icon {
        Icon::Play => vec![Shape::convex_polygon(
            vec![
                Pos2::new(x0 + rect.width() * 0.12, y0),
                Pos2::new(x1, c.y),
                Pos2::new(x0 + rect.width() * 0.12, y1),
            ],
            color,
            egui::Stroke::NONE,
        )],
        Icon::Pause => vec![
            Shape::rect_filled(
                Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x0 + rect.width() * 0.3, y1)),
                1.0,
                color,
            ),
            Shape::rect_filled(
                Rect::from_min_max(Pos2::new(x1 - rect.width() * 0.3, y0), Pos2::new(x1, y1)),
                1.0,
                color,
            ),
        ],
        Icon::Stop => vec![Shape::rect_filled(rect, 2.0, color)],
        Icon::Close => [
            [Pos2::new(x0, y0), Pos2::new(x1, y1)],
            [Pos2::new(x1, y0), Pos2::new(x0, y1)],
        ]
        .into_iter()
        .flat_map(|p| bar(p[0], p[1], w, color))
        .collect(),
        Icon::Refresh => {
            let r = rect.width() * 0.42;
            let mut pts: Vec<Pos2> = (0..22)
                .map(|i| {
                    let a = std::f32::consts::PI * 2.0 * (i as f32 / 21.0) - 0.55;
                    c + Vec2::new(a.cos(), a.sin()) * r
                })
                .collect();
            let mut out = polyline(&pts, w, color, false);
            let head = pts.pop().unwrap_or(c);
            let dir = (head - pts[pts.len() - 2]).normalized();
            let n = Vec2::new(-dir.y, dir.x);
            out.push(Shape::convex_polygon(
                vec![
                    head + dir * (w * 2.6),
                    head - dir * (w * 0.6) + n * (w * 1.9),
                    head - dir * (w * 0.6) - n * (w * 1.9),
                ],
                color,
                egui::Stroke::NONE,
            ));
            out
        }
        Icon::Filter | Icon::Sliders => [0.0, 0.5, 1.0]
            .into_iter()
            .flat_map(|t| {
                let y = y0 + rect.height() * t;
                let inset = rect.width() * 0.13 * t;
                bar(
                    Pos2::new(x0 + inset, y),
                    Pos2::new(x1 - inset, y),
                    w,
                    color,
                )
            })
            .collect(),
        Icon::Settings => {
            let mut out = vec![Shape::circle_stroke(c, rect.width() * 0.22, Stroke::new(w, color))];
            for i in 0..8 {
                let a = std::f32::consts::PI / 4.0 * i as f32;
                let v = Vec2::new(a.cos(), a.sin());
                out.extend(bar(
                    c + v * rect.width() * 0.34,
                    c + v * rect.width() * 0.48,
                    w,
                    color,
                ));
            }
            out
        }
        Icon::Download => {
            let mut out = bar(c, Pos2::new(c.y * 0.0 + c.x, y1 - w * 1.6), w, color);
            out.extend(polyline(
                &[
                    Pos2::new(c.x - w * 1.7, y1 - w * 3.4),
                    Pos2::new(c.x, y1 - w * 1.4),
                    Pos2::new(c.x + w * 1.7, y1 - w * 3.4),
                ],
                w,
                color,
                false,
            ));
            out.extend(bar(
                Pos2::new(x0 + rect.width() * 0.05, y1),
                Pos2::new(x1 - rect.width() * 0.05, y1),
                w,
                color,
            ));
            out
        }
        Icon::Copy => vec![
            Shape::rect_stroke(
                Rect::from_min_max(Pos2::new(x0, y0 + rect.height() * 0.18), Pos2::new(x1 - w, y1)),
                2.0,
                Stroke::new(w, color),
                egui::StrokeKind::Inside,
            ),
            Shape::rect_filled(
                Rect::from_min_max(Pos2::new(x0 + w * 1.6, y0), Pos2::new(x1, y1 - w)),
                2.0,
                color,
            ),
        ],
        Icon::List => {
            let mut out = Vec::new();
            for i in 0..3 {
                let y = y0 + rect.height() * (0.15 + 0.35 * i as f32);
                out.extend(bar(
                    Pos2::new(x0 + w * 1.6, y),
                    Pos2::new(x1, y),
                    w,
                    color,
                ));
                out.push(Shape::circle_filled(
                    Pos2::new(x0 + w * 0.4, y),
                    w * 0.75,
                    color,
                ));
            }
            out
        }
        Icon::Pip | Icon::PipExit => {
            let outer = Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1)).shrink(w * 0.5);
            let mut out = vec![Shape::rect_stroke(
                outer,
                2.0,
                Stroke::new(w, color),
                egui::StrokeKind::Inside,
            )];
            // anchored to the bottom-right so it reads as a sub-window, not a stop box
            let small = Rect::from_min_max(
                Pos2::new(
                    outer.max.x - outer.width() * 0.55,
                    outer.max.y - outer.height() * 0.5,
                ),
                Pos2::new(outer.max.x - w * 1.2, outer.max.y - w * 1.2),
            );
            if icon == Icon::Pip {
                out.push(Shape::rect_filled(small, 1.0, color));
            } else {
                out.push(Shape::rect_stroke(
                    small,
                    1.0,
                    Stroke::new(w * 0.85, color),
                    egui::StrokeKind::Middle,
                ));
            }
            out
        }
        Icon::Search => {
            let cc = Pos2::new(c.x - rect.width() * 0.1, c.y - rect.height() * 0.1);
            let mut out = vec![Shape::circle_stroke(
                cc,
                rect.width() * 0.32,
                Stroke::new(w, color),
            )];
            out.extend(bar(
                cc + Vec2::new(0.7, 0.7).normalized() * rect.width() * 0.32,
                Pos2::new(x1, y1),
                w,
                color,
            ));
            out
        }
        Icon::Check => polyline(
            &[
                Pos2::new(x0, c.y),
                Pos2::new(c.x - w, y1),
                Pos2::new(x1, y0),
            ],
            w,
            color,
            false,
        ),
        Icon::ChevronDown | Icon::ChevronUp => {
            let pts = if icon == Icon::ChevronDown {
                vec![
                    Pos2::new(x0 + rect.width() * 0.08, c.y - rect.height() * 0.18),
                    c,
                    Pos2::new(x1 - rect.width() * 0.08, c.y - rect.height() * 0.18),
                ]
            } else {
                vec![
                    Pos2::new(x0 + rect.width() * 0.08, c.y + rect.height() * 0.18),
                    c,
                    Pos2::new(x1 - rect.width() * 0.08, c.y + rect.height() * 0.18),
                ]
            };
            polyline(&pts, w, color, false)
        }
        Icon::Dot => vec![Shape::circle_filled(c, rect.width() * 0.28, color)],
        Icon::Speaker => {
            let mut out = vec![speaker_body(rect, color)];
            let cx = x0 + rect.width() * 0.54;
            for (rad, span) in [
                (rect.width() * 0.19, 0.60f32),
                (rect.width() * 0.34, 0.72f32),
            ] {
                let pts: Vec<Pos2> = (0..=4)
                    .map(|k| {
                        let a = -span + 2.0 * span * (k as f32 / 4.0);
                        Pos2::new(cx + rad * a.cos(), c.y + rad * a.sin())
                    })
                    .collect();
                out.extend(polyline(&pts, w, color, false));
            }
            out
        }
        Icon::SpeakerMute => {
            let mut out = vec![speaker_body(rect, color)];
            out.extend(bar(
                Pos2::new(x0 + rect.width() * 0.66, y0 + rect.height() * 0.2),
                Pos2::new(x1, y1 - rect.height() * 0.2),
                w,
                color,
            ));
            out.extend(bar(
                Pos2::new(x1, y0 + rect.height() * 0.2),
                Pos2::new(x0 + rect.width() * 0.66, y1 - rect.height() * 0.2),
                w,
                color,
            ));
            out
        }
    };
    for s in shapes {
        ui.painter().add(s);
    }
}

/// Icon-only button sized `size`, using the active widget visuals for bg/stroke.
pub fn icon_button(ui: &mut Ui, icon: Icon, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let v = ui.style().interact(&resp);
        if v.bg_fill != Color32::TRANSPARENT {
            ui.painter()
                .rect_filled(rect.shrink(0.5), v.corner_radius, v.bg_fill);
        }
        if v.bg_stroke.width > 0.0 {
            ui.painter().rect_stroke(
                rect.shrink(0.5),
                v.corner_radius,
                v.bg_stroke,
                egui::StrokeKind::Inside,
            );
        }
        let inner = Rect::from_center_size(rect.center(), Vec2::splat(size * 0.5));
        draw(ui, icon, inner, v.fg_stroke.color);
    }
    // A raw `Sense::click` area does not pick up `visuals.interact_cursor`.
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Icon-only button with a translucent circular backdrop, for overlaying on video.
pub fn overlay_icon(ui: &mut Ui, icon: Icon, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if resp.hovered() {
            Color32::from_rgba_unmultiplied(0, 0, 0, 190)
        } else {
            Color32::from_rgba_unmultiplied(0, 0, 0, 130)
        };
        ui.painter().circle_filled(rect.center(), size * 0.5, fill);
        let inner = Rect::from_center_size(rect.center(), Vec2::splat(size * 0.42));
        let color = if resp.hovered() {
            Color32::WHITE
        } else {
            Color32::from_rgba_unmultiplied(255, 255, 255, 225)
        };
        draw(ui, icon, inner, color);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

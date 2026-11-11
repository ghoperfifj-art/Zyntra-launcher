//! Theme and custom widgets: dark plum palette, glowing cards, pills, charts.

use eframe::egui::{
    self, epaint::Shadow, Align2, Color32, CornerRadius, CursorIcon, FontData, FontDefinitions, FontFamily, FontId,
    Mesh, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
};
use std::sync::Arc;

pub const BG: Color32 = Color32::from_rgb(11, 9, 14);
pub const PANEL: Color32 = Color32::from_rgb(16, 13, 20);
pub const CARD: Color32 = Color32::from_rgb(23, 19, 28);
pub const CARD_HI: Color32 = Color32::from_rgb(34, 28, 41);
pub const SUNKEN: Color32 = Color32::from_rgb(15, 12, 19);
pub const BORDER: Color32 = Color32::from_rgb(46, 38, 54);
pub const ACCENT: Color32 = Color32::from_rgb(222, 168, 238);
pub const ACCENT_DIM: Color32 = Color32::from_rgb(112, 66, 132);
pub const TEXT: Color32 = Color32::from_rgb(242, 236, 247);
pub const MUTED: Color32 = Color32::from_rgb(148, 138, 160);
pub const GREEN: Color32 = Color32::from_rgb(96, 218, 146);
pub const RED: Color32 = Color32::from_rgb(244, 114, 128);

pub fn glow_color(alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(214, 120, 236, alpha)
}

pub fn setup(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let mut proportional = fonts.families[&FontFamily::Proportional].clone();
    if let Ok(bytes) = std::fs::read("C:\\Windows\\Fonts\\segoeui.ttf") {
        fonts.font_data.insert("segoe".into(), Arc::new(FontData::from_owned(bytes)));
        proportional.insert(0, "segoe".into());
    }
    let mut bold = proportional.clone();
    if let Ok(bytes) = std::fs::read("C:\\Windows\\Fonts\\seguisb.ttf").or_else(|_| std::fs::read("C:\\Windows\\Fonts\\segoeuib.ttf")) {
        fonts.font_data.insert("segoe-bold".into(), Arc::new(FontData::from_owned(bytes)));
        bold.insert(0, "segoe-bold".into());
    }
    fonts.families.insert(FontFamily::Proportional, proportional);
    fonts.families.insert(FontFamily::Name("bold".into()), bold);
    ctx.set_fonts(fonts);

    // Always dark: with a light Windows theme egui would otherwise use its light style
    // (white scroll bars, other spacing).
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut(|s| {
        let v = &mut s.visuals;
        *v = egui::Visuals::dark();
        v.panel_fill = BG;
        v.window_fill = PANEL;
        v.extreme_bg_color = SUNKEN;
        v.faint_bg_color = CARD_HI;
        v.override_text_color = Some(TEXT);
        v.selection.bg_fill = ACCENT_DIM;
        v.selection.stroke = Stroke::new(1.0_f32, ACCENT);
        v.hyperlink_color = ACCENT;
        let radius = CornerRadius::same(10);
        for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            w.corner_radius = radius;
        }
        v.widgets.inactive.bg_fill = CARD_HI;
        v.widgets.inactive.weak_bg_fill = CARD_HI;
        v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);
        v.widgets.hovered.bg_fill = Color32::from_rgb(48, 38, 58);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(48, 38, 58);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT_DIM);
        v.widgets.active.bg_fill = ACCENT_DIM;
        v.widgets.active.weak_bg_fill = ACCENT_DIM;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
        s.spacing.item_spacing = egui::vec2(10.0, 10.0);
        s.spacing.button_padding = egui::vec2(14.0, 7.0);
        s.spacing.slider_width = 260.0;
        s.spacing.scroll = egui::style::ScrollStyle::thin();
        s.text_styles.insert(egui::TextStyle::Body, FontId::proportional(14.5));
        s.text_styles.insert(egui::TextStyle::Button, FontId::proportional(14.0));
    });
}

pub fn font(size: f32) -> FontId {
    FontId::proportional(size)
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

pub fn text(p: &Painter, pos: Pos2, anchor: Align2, s: impl ToString, f: FontId, c: Color32) -> Rect {
    p.text(pos, anchor, s.to_string(), f, c)
}

/// Text truncated with "…" to fit `max_width`.
pub fn text_fit(p: &Painter, pos: Pos2, anchor: Align2, s: &str, f: FontId, c: Color32, max_width: f32) -> Rect {
    let mut s = s.to_owned();
    let width = |s: &str| p.layout_no_wrap(s.to_owned(), f.clone(), c).size().x;
    if width(&s) > max_width {
        while !s.is_empty() && width(&format!("{s}…")) > max_width {
            s.pop();
        }
        s.push('…');
    }
    p.text(pos, anchor, s, f, c)
}

pub fn card(p: &Painter, rect: Rect) {
    p.add(Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(90) }.as_shape(rect, 20));
    p.rect(rect, 20, CARD, Stroke::new(1.0_f32, BORDER), StrokeKind::Inside);
}

/// Soft radial glow clipped to `clip`.
pub fn glow(p: &Painter, clip: Rect, center: Pos2, radius: f32, color: Color32) {
    let p = p.with_clip_rect(clip.intersect(p.clip_rect()));
    let rings = [(0.0, 1.0), (0.3, 0.7), (0.6, 0.3), (1.0, 0.0)];
    let segments = 64;
    let mut mesh = Mesh::default();
    for &(r, a) in &rings {
        for i in 0..segments {
            let t = i as f32 / segments as f32 * std::f32::consts::TAU;
            let pos = center + Vec2::angled(t) * radius * r;
            mesh.colored_vertex(pos, color.gamma_multiply(a));
        }
    }
    let seg = segments as u32;
    for ring in 0..(rings.len() as u32 - 1) {
        for i in 0..seg {
            let a = ring * seg + i;
            let b = ring * seg + (i + 1) % seg;
            let c = (ring + 1) * seg + i;
            let d = (ring + 1) * seg + (i + 1) % seg;
            mesh.add_triangle(a, b, c);
            mesh.add_triangle(b, d, c);
        }
    }
    p.add(Shape::mesh(mesh));
}

/// Outline of a rounded rectangle, clockwise from the top-left arc.
fn rounded_outline(rect: Rect, r: f32) -> Vec<Pos2> {
    use std::f32::consts::{FRAC_PI_2, PI};
    let r = r.min(rect.width() * 0.5).min(rect.height() * 0.5);
    // (arc centre, angle the arc starts at)
    let arcs = [
        (rect.left_top() + Vec2::new(r, r), PI),
        (rect.right_top() + Vec2::new(-r, r), PI * 1.5),
        (rect.right_bottom() + Vec2::new(-r, -r), 0.0),
        (rect.left_bottom() + Vec2::new(r, -r), FRAC_PI_2),
    ];
    let steps = 16;
    let mut arc_points = Vec::with_capacity(arcs.len() * (steps + 1));
    for (c, start) in arcs {
        for i in 0..=steps {
            arc_points.push(c + Vec2::angled(start + FRAC_PI_2 * i as f32 / steps as f32) * r);
        }
    }
    // Points every few pixels along the straight sides too: long thin triangles would show
    // as stripes in the glow.
    const STEP: f32 = 8.0;
    let mut out = Vec::new();
    for (i, &a) in arc_points.iter().enumerate() {
        let b = arc_points[(i + 1) % arc_points.len()];
        let n = (a.distance(b) / STEP).ceil().max(1.0) as u32;
        for k in 0..n {
            out.push(a + (b - a) * (k as f32 / n as f32));
        }
    }
    out
}

/// Glow inside a rounded card, drawn only within its rounded shape (nothing is painted
/// over the corners, so they look the same whatever lies under the card); re-strokes the
/// card border.
pub fn card_glow(p: &Painter, rect: Rect, rounding: f32, center: Pos2, radius: f32, color: Color32, border: Color32) {
    // Same falloff as `glow`: full at the centre, gone at `radius`.
    let falloff = |d: f32| -> f32 {
        let x = d / radius;
        let stops = [(0.0, 1.0), (0.3, 0.7), (0.6, 0.3), (1.0, 0.0)];
        stops.windows(2).find(|w| x <= w[1].0).map_or(0.0, |w| w[0].1 + (w[1].1 - w[0].1) * (x - w[0].0) / (w[1].0 - w[0].0))
    };
    // Rays from a point inside the card to its outline; a centre outside the card still
    // sets the colours.
    // A corner's radius inside: rays from the very corner would fill the corner square.
    let inset = Vec2::splat(rounding.max(1.0).min(rect.width().min(rect.height()) / 2.0));
    let origin = center.clamp(rect.min + inset, rect.max - inset);
    let outline = rounded_outline(rect, rounding);
    let samples = 16u32;
    let mut mesh = Mesh::default();
    for &edge in &outline {
        for k in 0..=samples {
            let pos = origin + (edge - origin) * (k as f32 / samples as f32);
            mesh.colored_vertex(pos, color.gamma_multiply(falloff(pos.distance(center))));
        }
    }
    let n = outline.len() as u32;
    let per = samples + 1;
    for ray in 0..n {
        let (a, b) = (ray * per, (ray + 1) % n * per);
        for k in 0..samples {
            mesh.add_triangle(a + k, a + k + 1, b + k);
            mesh.add_triangle(a + k + 1, b + k + 1, b + k);
        }
    }
    p.add(Shape::mesh(mesh));
    p.rect_stroke(rect, rounding, Stroke::new(1.0_f32, border), StrokeKind::Inside);
}

pub fn pill(ui: &mut Ui, label: &str, selected: bool) -> Response {
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font(13.5), TEXT);
    let size = egui::vec2(galley.size().x + 30.0, 34.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let p = ui.painter();
    if selected {
        p.add(Shadow { offset: [0, 0], blur: 16, spread: 0, color: glow_color(70) }.as_shape(rect, 17));
        p.rect_filled(rect, 17, ACCENT_DIM);
        p.rect_stroke(rect, 17, Stroke::new(1.0_f32, glow_color(160)), StrokeKind::Inside);
    } else {
        let fill = if resp.hovered() { CARD_HI } else { SUNKEN };
        let stroke = if resp.hovered() { ACCENT_DIM } else { BORDER };
        p.rect(rect, 17, fill, Stroke::new(1.2_f32, stroke), StrokeKind::Inside);
    }
    let color = if selected || resp.hovered() { TEXT } else { Color32::from_rgb(208, 200, 216) };
    p.galley_with_override_text_color(rect.center() - galley.size() / 2.0, galley, color);
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// Big glowing accent button.
pub fn primary(ui: &mut Ui, label: &str, size: Vec2, enabled: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let p = ui.painter();
    let radius = (size.y / 2.0).min(255.0) as u8;
    if enabled {
        let hover = resp.hovered();
        p.add(Shadow { offset: [0, 4], blur: if hover { 34 } else { 24 }, spread: 0, color: glow_color(if hover { 150 } else { 100 }) }.as_shape(rect, radius));
        let base = if hover { Color32::from_rgb(232, 186, 245) } else { Color32::from_rgb(214, 152, 232) };
        p.rect_filled(rect, radius, base);
        p.text(rect.center(), Align2::CENTER_CENTER, label, bold(size.y.clamp(14.0, 40.0) * 0.36 + 4.0), Color32::from_rgb(38, 18, 46));
    } else {
        p.rect(rect, radius, CARD_HI, Stroke::new(1.0_f32, BORDER), StrokeKind::Inside);
        p.text(rect.center(), Align2::CENTER_CENTER, label, bold(size.y.clamp(14.0, 40.0) * 0.36 + 4.0), MUTED);
    }
    if enabled { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp }
}

/// Vector folder glyph (the emoji fonts draw folders poorly).
pub fn folder_icon(p: &Painter, center: Pos2, color: Color32) {
    let body = Rect::from_center_size(center + egui::vec2(0.0, 1.5), egui::vec2(16.0, 11.0));
    let tab = Rect::from_min_size(body.left_top() - egui::vec2(0.0, 3.0), egui::vec2(7.0, 4.0));
    p.rect_filled(tab, 1, color);
    p.rect_stroke(body, 2, Stroke::new(1.6_f32, color), StrokeKind::Middle);
}

/// Draws `icon` centred at `center`; the name "folder" gives the vector folder.
pub fn icon(p: &Painter, center: Pos2, icon: &str, size: f32, color: Color32) {
    if icon == "folder" {
        folder_icon(p, center, color);
    } else if icon == "sliders" {
        // Three settings sliders with their knobs
        let stroke = Stroke::new(1.6_f32, color);
        for (dy, knob) in [(-5.0, 3.0), (0.0, -3.0), (5.0, 1.5)] {
            let y = center.y + dy;
            p.line_segment([Pos2::new(center.x - 8.0, y), Pos2::new(center.x + 8.0, y)], stroke);
            p.circle_filled(Pos2::new(center.x + knob, y), 2.6, color);
        }
    } else if icon == "star" {
        // Four-point sparkle, for shaders
        let (r, k) = (8.5, 2.2);
        let pts: Vec<Pos2> = (0..8)
            .map(|i| {
                let a = i as f32 * std::f32::consts::FRAC_PI_4 - std::f32::consts::FRAC_PI_2;
                center + egui::Vec2::angled(a) * if i % 2 == 0 { r } else { k }
            })
            .collect();
        p.add(Shape::convex_polygon(pts.clone(), Color32::TRANSPARENT, Stroke::NONE));
        p.add(Shape::closed_line(pts, Stroke::new(1.6_f32, color)));
        p.circle_filled(center + egui::vec2(6.0, -6.0), 1.6, color);
    } else if icon == "cube" {
        // Isometric block outline
        let s = 7.5;
        let top = [center + egui::vec2(0.0, -s), center + egui::vec2(s, -s * 0.5), center, center + egui::vec2(-s, -s * 0.5)];
        let stroke = Stroke::new(1.6_f32, color);
        p.add(Shape::closed_line(top.to_vec(), stroke));
        for c in [top[1], top[2], top[3]] {
            p.line_segment([c, c + egui::vec2(0.0, s)], stroke);
        }
        p.line_segment([top[3] + egui::vec2(0.0, s), center + egui::vec2(0.0, s * 1.5)], stroke);
        p.line_segment([top[1] + egui::vec2(0.0, s), center + egui::vec2(0.0, s * 1.5)], stroke);
        p.line_segment([center + egui::vec2(0.0, s), center + egui::vec2(0.0, s * 1.5)], stroke);
    } else {
        p.text(center, Align2::CENTER_CENTER, icon, font(size), color);
    }
}

pub fn circle_button(ui: &mut Ui, glyph: &str, hint: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(38.0, 38.0), Sense::click());
    let p = ui.painter();
    let fill = if resp.hovered() { CARD_HI } else { SUNKEN };
    p.circle(rect.center(), 19.0, fill, Stroke::new(1.0_f32, if resp.hovered() { ACCENT_DIM } else { BORDER }));
    icon(p, rect.center(), glyph, 16.0, TEXT);
    resp.on_hover_text(hint).on_hover_cursor(CursorIcon::PointingHand)
}

/// Custom RAM slider: rounded track, glowing fill, a tick at `recommended`.
/// Returns true when the user dragged or clicked it.
pub fn ram_slider(ui: &mut Ui, rect: Rect, value: &mut u32, min: u32, max: u32, recommended: u32) -> bool {
    let resp = ui.interact(rect.expand2(egui::vec2(8.0, 6.0)), ui.id().with(("ram_slider", rect.min.x as i32)), Sense::click_and_drag());
    let track = Rect::from_center_size(rect.center(), egui::vec2(rect.width() - 16.0, 8.0));
    let span = (max.saturating_sub(min)).max(1) as f32;
    let mut changed = false;
    if let Some(pos) = resp.interact_pointer_pos().filter(|_| resp.dragged() || resp.clicked()) {
        let t = ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0);
        let new = ((min as f32 + t * span) / 512.0).round() as u32 * 512;
        let new = new.clamp(min, max);
        if new != *value {
            *value = new;
        }
        changed = true;
    }
    let t = ((*value).clamp(min, max) - min) as f32 / span;
    let p = ui.painter();
    p.rect_filled(track, 4, SUNKEN);
    p.rect_stroke(track, 4, Stroke::new(1.0_f32, BORDER), StrokeKind::Outside);
    let fill = Rect::from_min_max(track.min, Pos2::new(track.left() + track.width() * t, track.bottom()));
    if fill.width() > 1.0 {
        p.add(Shadow { offset: [0, 0], blur: 10, spread: 0, color: glow_color(100) }.as_shape(fill, 4));
        p.rect_filled(fill, 4, ACCENT);
    }
    let rec_x = track.left() + track.width() * ((recommended.clamp(min, max) - min) as f32 / span);
    p.line_segment([Pos2::new(rec_x, track.top() - 5.0), Pos2::new(rec_x, track.bottom() + 5.0)], Stroke::new(2.0_f32, GREEN));
    let knob = Pos2::new(track.left() + track.width() * t, track.center().y);
    let hot = resp.hovered() || resp.dragged();
    p.circle_filled(knob, if hot { 11.0 } else { 9.0 }, glow_color(70));
    p.circle(knob, 7.0, Color32::from_rgb(250, 228, 255), Stroke::new(2.0_f32, ACCENT));
    if hot {
        ui.ctx().set_cursor_icon(CursorIcon::Grab);
    }
    changed
}

pub fn progress(p: &Painter, rect: Rect, frac: Option<f32>, time: f64) {
    p.rect_filled(rect, 2, SUNKEN);
    let fill_rect = match frac {
        Some(f) => Rect::from_min_size(rect.min, egui::vec2(rect.width() * f.clamp(0.0, 1.0), rect.height())),
        None => {
            // Indeterminate: a sliding segment.
            let w = rect.width() * 0.3;
            let x = rect.left() + ((time * 0.6).fract() as f32) * (rect.width() + w) - w;
            Rect::from_min_max(Pos2::new(x.max(rect.left()), rect.top()), Pos2::new((x + w).min(rect.right()), rect.bottom()))
        }
    };
    if fill_rect.width() > 0.5 {
        p.add(Shadow { offset: [0, 0], blur: 12, spread: 0, color: glow_color(120) }.as_shape(fill_rect, 2));
        p.rect_filled(fill_rect, 2, ACCENT);
        // Lighter top row of pixels, like the in-game experience bar
        let shine = Rect::from_min_max(fill_rect.min, Pos2::new(fill_rect.right(), fill_rect.top() + fill_rect.height() * 0.35));
        p.rect_filled(shine, 0, Color32::from_rgba_unmultiplied(255, 240, 255, 70));
    }
    // Segment notches across the whole bar (XP-bar look)
    let segments = (rect.width() / 18.0).round().max(4.0) as i32;
    for i in 1..segments {
        let x = rect.left() + rect.width() * i as f32 / segments as f32;
        p.rect_filled(Rect::from_center_size(Pos2::new(x, rect.center().y), egui::vec2(2.0, rect.height())), 0, SUNKEN);
    }
    p.rect_stroke(rect, 2, Stroke::new(1.0_f32, Color32::from_black_alpha(160)), StrokeKind::Outside);
}

/// Stepped pixel squares (as in the logo), fading out from `origin` along `dir`.
pub fn pixel_dust(p: &Painter, origin: Pos2, dir: egui::Vec2, cell: f32, color: Color32) {
    let steps = [(0.0, 0.0, 1.0), (1.0, -1.0, 0.75), (2.0, 0.0, 0.55), (2.0, -2.0, 0.4), (3.0, -1.0, 0.3), (4.0, -3.0, 0.2)];
    for (dx, dy, a) in steps {
        let pos = origin + egui::vec2(dx * dir.x, dy * dir.y) * cell;
        p.rect_filled(Rect::from_min_size(pos, egui::vec2(cell, cell)), 0, color.gamma_multiply(a));
    }
}

/// Version picture (the block from assets/version_block.png), drawn as is — no glow behind it.
pub fn block_icon(p: &Painter, rect: Rect, texture: egui::TextureId) {
    p.image(texture, rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
}

/// Area chart of `values` (newest last) with hover tooltip.
pub fn area_chart(ui: &mut Ui, rect: Rect, values: &[f32], unit: &str) {
    let p = ui.painter_at(rect);
    let max = values.iter().cloned().fold(1.0f32, f32::max) * 1.2;
    let plot = Rect::from_min_max(rect.min + egui::vec2(46.0, 8.0), rect.max - egui::vec2(6.0, 22.0));
    for i in 0..=4 {
        let y = plot.bottom() - plot.height() * i as f32 / 4.0;
        p.line_segment([Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)], Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 10)));
        let v = max * i as f32 / 4.0;
        let label = if max < 10.0 { format!("{v:.1}") } else { format!("{v:.0}") };
        text(&p, Pos2::new(plot.left() - 10.0, y), Align2::RIGHT_CENTER, label, font(11.0), MUTED);
    }
    for (i, label) in ["−60 с", "−45 с", "−30 с", "−15 с", "сейчас"].iter().enumerate() {
        let x = plot.left() + plot.width() * i as f32 / 4.0;
        let anchor = match i {
            0 => Align2::LEFT_CENTER,
            4 => Align2::RIGHT_CENTER,
            _ => Align2::CENTER_CENTER,
        };
        text(&p, Pos2::new(x, plot.bottom() + 12.0), anchor, label, font(11.0), MUTED);
    }
    if values.len() < 2 {
        return;
    }
    let n = values.len();
    let point = |i: usize| {
        Pos2::new(
            plot.left() + plot.width() * i as f32 / (n - 1) as f32,
            plot.bottom() - plot.height() * (values[i] / max).clamp(0.0, 1.0),
        )
    };
    let mut mesh = Mesh::default();
    for i in 0..n {
        let pt = point(i);
        mesh.colored_vertex(pt, glow_color(70));
        mesh.colored_vertex(Pos2::new(pt.x, plot.bottom()), glow_color(0));
    }
    for i in 0..(n as u32 - 1) {
        let a = i * 2;
        mesh.add_triangle(a, a + 1, a + 2);
        mesh.add_triangle(a + 1, a + 3, a + 2);
    }
    p.add(Shape::mesh(mesh));
    let line: Vec<Pos2> = (0..n).map(point).collect();
    p.add(Shape::line(line, Stroke::new(2.0_f32, Color32::from_rgb(232, 200, 240))));

    if let Some(hover) = ui.ctx().pointer_hover_pos().filter(|h| plot.contains(*h)) {
        let i = (((hover.x - plot.left()) / plot.width()) * (n - 1) as f32).round() as usize;
        let pt = point(i.min(n - 1));
        let dashes = Shape::dashed_line(&[pt, Pos2::new(pt.x, plot.bottom())], Stroke::new(1.0_f32, MUTED), 4.0, 4.0);
        p.extend(dashes);
        p.circle_filled(pt, 9.0, glow_color(90));
        p.circle(pt, 5.0, Color32::from_rgb(250, 220, 255), Stroke::new(2.0_f32, ACCENT));
        let label = format!("{:.1} {unit}", values[i.min(n - 1)]);
        let tip = Rect::from_center_size(pt - egui::vec2(0.0, 34.0), egui::vec2(110.0, 34.0));
        let tip = tip.translate(egui::vec2((rect.left() + 4.0 - tip.left()).max(0.0) + (rect.right() - 4.0 - tip.right()).min(0.0), 0.0));
        p.rect(tip, 10, CARD_HI, Stroke::new(1.0_f32, BORDER), StrokeKind::Inside);
        text(&p, tip.center(), Align2::CENTER_CENTER, label, bold(13.0), TEXT);
    }
}

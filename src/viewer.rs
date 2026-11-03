//! Small software 3D renderer for a player skin: textured boxes, drag to rotate.

use crate::ui::glow_color;
use eframe::egui::{self, epaint::Vertex, Color32, Mesh, Pos2, Rect, Sense, Shape, TextureId, Ui, Vec2};

pub struct ViewerState {
    pub yaw: f32,
    pub pitch: f32,
    last_drag: f64,
}

impl Default for ViewerState {
    fn default() -> Self {
        Self { yaw: -0.5, pitch: 0.12, last_drag: -10.0 }
    }
}

type V3 = [f32; 3];

/// One box of the model: texture offset, size and min corner (model units = skin pixels).
struct Part {
    uv: (f32, f32),
    size: V3,
    min: V3,
    inflate: f32,
}

fn parts(slim: bool) -> Vec<Part> {
    let aw = if slim { 3.0 } else { 4.0 };
    let part = |uv: (f32, f32), size: V3, min: V3, inflate: f32| Part { uv, size, min, inflate };
    let mut list = Vec::new();
    for (base, layer, size, min, inflate) in [
        ((0.0, 0.0), (32.0, 0.0), [8.0, 8.0, 8.0], [-4.0, 24.0, -4.0], 0.5),
        ((16.0, 16.0), (16.0, 32.0), [8.0, 12.0, 4.0], [-4.0, 12.0, -2.0], 0.25),
        ((40.0, 16.0), (40.0, 32.0), [aw, 12.0, 4.0], [-4.0 - aw, 12.0, -2.0], 0.25),
        ((32.0, 48.0), (48.0, 48.0), [aw, 12.0, 4.0], [4.0, 12.0, -2.0], 0.25),
        ((0.0, 16.0), (0.0, 32.0), [4.0, 12.0, 4.0], [-4.0, 0.0, -2.0], 0.25),
        ((16.0, 48.0), (0.0, 48.0), [4.0, 12.0, 4.0], [0.0, 0.0, -2.0], 0.25),
    ] {
        list.push(part(base, size, min, 0.0));
        list.push(part(layer, size, min, inflate));
    }
    list
}

struct Quad {
    corners: [V3; 4],
    uv: [Pos2; 4],
    normal: V3,
}

/// The six faces of a part using the standard skin layout.
fn quads(p: &Part) -> Vec<Quad> {
    let [w, h, d] = p.size;
    let (u, v) = p.uv;
    let i = p.inflate;
    let (x0, y0, z0) = (p.min[0] - i, p.min[1] - i, p.min[2] - i);
    let (x1, y1, z1) = (p.min[0] + w + i, p.min[1] + h + i, p.min[2] + d + i);
    let tex = |tu: f32, tv: f32, tw: f32, th: f32| {
        let e = 0.02; // stay inside the texel to avoid bleeding
        [
            Pos2::new((tu + e) / 64.0, (tv + e) / 64.0),
            Pos2::new((tu + tw - e) / 64.0, (tv + e) / 64.0),
            Pos2::new((tu + tw - e) / 64.0, (tv + th - e) / 64.0),
            Pos2::new((tu + e) / 64.0, (tv + th - e) / 64.0),
        ]
    };
    vec![
        Quad { corners: [[x0, y1, z1], [x1, y1, z1], [x1, y0, z1], [x0, y0, z1]], uv: tex(u + d, v + d, w, h), normal: [0.0, 0.0, 1.0] },
        Quad { corners: [[x1, y1, z0], [x0, y1, z0], [x0, y0, z0], [x1, y0, z0]], uv: tex(u + 2.0 * d + w, v + d, w, h), normal: [0.0, 0.0, -1.0] },
        Quad { corners: [[x0, y1, z0], [x0, y1, z1], [x0, y0, z1], [x0, y0, z0]], uv: tex(u, v + d, d, h), normal: [-1.0, 0.0, 0.0] },
        Quad { corners: [[x1, y1, z1], [x1, y1, z0], [x1, y0, z0], [x1, y0, z1]], uv: tex(u + d + w, v + d, d, h), normal: [1.0, 0.0, 0.0] },
        Quad { corners: [[x0, y1, z0], [x1, y1, z0], [x1, y1, z1], [x0, y1, z1]], uv: tex(u + d, v, w, d), normal: [0.0, 1.0, 0.0] },
        Quad { corners: [[x0, y0, z1], [x1, y0, z1], [x1, y0, z0], [x0, y0, z0]], uv: tex(u + d + w, v, w, d), normal: [0.0, -1.0, 0.0] },
    ]
}

fn rotate(p: V3, yaw: f32, pitch: f32) -> V3 {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let x = p[0] * cy + p[2] * sy;
    let z = -p[0] * sy + p[2] * cy;
    let y = p[1] * cp - z * sp;
    let z = p[1] * sp + z * cp;
    [x, y, z]
}

/// Draws the rotatable model into `rect`. Drag to rotate, double-click to reset;
/// it slowly turns by itself when left alone.
pub fn show(ui: &mut Ui, rect: Rect, texture: TextureId, slim: bool, state: &mut ViewerState) {
    let resp = ui.interact(rect, ui.id().with("skin_viewer"), Sense::click_and_drag());
    let now = ui.input(|i| i.time);
    if resp.dragged() {
        let delta = resp.drag_delta();
        state.yaw += delta.x * 0.012;
        state.pitch = (state.pitch + delta.y * 0.008).clamp(-0.7, 0.7);
        state.last_drag = now;
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    if resp.double_clicked() {
        *state = ViewerState { last_drag: now, ..Default::default() };
    }
    if now - state.last_drag > 2.5 {
        state.yaw += ui.input(|i| i.stable_dt).min(0.1) * 0.5;
        ui.ctx().request_repaint();
    }

    let p = ui.painter_at(rect);
    let scale = (rect.height() / 40.0).min(rect.width() / 22.0);
    let center = rect.center() + Vec2::new(0.0, scale * 2.0);
    // Floor shadow and glow
    crate::ui::glow(&p, rect, center, rect.height() * 0.45, glow_color(55));
    p.add(Shape::ellipse_filled(center + Vec2::new(0.0, scale * 16.5), Vec2::new(scale * 9.0, scale * 2.2), Color32::from_black_alpha(140)));

    let light = {
        let l = [0.35f32, 0.55, 0.75];
        let n = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
        [l[0] / n, l[1] / n, l[2] / n]
    };
    let focal = 90.0;
    let project = |v: V3| {
        let k = focal / (focal - v[2]);
        center + Vec2::new(v[0] * k * scale, -v[1] * k * scale)
    };

    let mut faces: Vec<(f32, [Pos2; 4], [Pos2; 4], Color32)> = Vec::new();
    for part in parts(slim) {
        for q in quads(&part) {
            let n = rotate(q.normal, state.yaw, state.pitch);
            if n[2] <= 0.001 {
                continue;
            }
            let pts = q.corners.map(|c| rotate([c[0], c[1] - 16.0, c[2]], state.yaw, state.pitch));
            let depth = pts.iter().map(|c| c[2]).sum::<f32>() / 4.0;
            let shade = 0.55 + 0.45 * (n[0] * light[0] + n[1] * light[1] + n[2] * light[2]).max(0.0);
            let c = (shade * 255.0) as u8;
            faces.push((depth, pts.map(project), q.uv, Color32::from_rgb(c, c, c)));
        }
    }
    faces.sort_by(|a, b| a.0.total_cmp(&b.0));

    let mut mesh = Mesh::with_texture(texture);
    for (_, pts, uv, color) in faces {
        let base = mesh.vertices.len() as u32;
        for k in 0..4 {
            mesh.vertices.push(Vertex { pos: pts[k], uv: uv[k], color });
        }
        mesh.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    p.add(Shape::mesh(mesh));
}

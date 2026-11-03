//! The launcher window: sidebar, header and the five pages.

use crate::config::{loader_of, Config, Fine, Target};
use crate::minecraft::{self, Dirs, LaunchOptions, Manifest};
use crate::task::Task;
use crate::ui::*;
use crate::{fabric, horizon, hz_mods, mods, native, net, skin, system, viewer};
use anyhow::Result;
use eframe::egui::{self, vec2, Align, Align2, Color32, CursorIcon, Layout, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const GAP: f32 = 16.0;
/// The «Сборка Horizon» entry of the Horizon tab (native mods build).
const NATIVE_BUILD_ID: &str = "zyntra_native_build";
const SPEED_SAMPLES: usize = 240; // 60 s at 4 samples/s

#[derive(Default)]
struct Remote {
    manifest: Option<Manifest>,
    fabric: Vec<(String, bool)>,
    errors: Vec<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Home,
    Versions,
    Skins,
    Mods,
    Shaders,
    Fine,
    Console,
    Settings,
}

#[derive(Clone, Copy, PartialEq)]
enum Filter {
    Release,
    Snapshot,
    Fabric,
    Old,
    Installed,
}

impl Filter {
    const ALL: [Filter; 5] = [Filter::Release, Filter::Snapshot, Filter::Fabric, Filter::Old, Filter::Installed];
    fn label(self) -> &'static str {
        match self {
            Filter::Release => "Релизы",
            Filter::Snapshot => "Снапшоты",
            Filter::Fabric => "Fabric",
            Filter::Old => "Beta / Alpha",
            Filter::Installed => "Установленные",
        }
    }
}

/// Results shared with background threads of the mods page.
#[derive(Default)]
struct ModsState {
    searching: bool,
    results: Vec<mods::Hit>,
    error: Option<String>,
    /// project → None while loading, then the (loader, game version) list or an error.
    targets: HashMap<String, Option<Result<Vec<(String, String)>, String>>>,
    icons: HashMap<String, egui::ColorImage>,
    icon_requested: HashSet<String>,
    /// Horizon's registry: None until fetched.
    hz_index: Option<Result<Vec<hz_mods::Project>, String>>,
    hz_loading: bool,
}

/// Results shared with background threads of the shaders page.
#[derive(Default)]
struct ShaderState {
    searching: bool,
    results: Vec<mods::Hit>,
    error: Option<String>,
    /// project → None while loading, then its versions or an error.
    versions: HashMap<String, Option<Result<Vec<mods::Version>, String>>>,
}

/// 1234567 → «1,2 млн».
fn fmt_count(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.0} тыс", n as f64 / 1e3),
        _ => format!("{:.1} млн", n as f64 / 1e6).replace('.', ","),
    }
}

struct Entry {
    target: Target,
    title: String,
    sub: String,
    installed: bool,
}

pub struct App {
    cfg: Config,
    saved: Config,
    page: Page,
    filter: Filter,
    search: String,
    remote: Arc<Mutex<Remote>>,
    task: Arc<Task>,
    speed: VecDeque<f32>,
    last_bytes: u64,
    last_sample: Instant,
    installed: Vec<(String, bool)>,
    horizon_local: Option<horizon::Build>,
    was_running: bool,
    logo: egui::TextureHandle,
    /// Block picture shown next to every version (assets/version_block.png).
    version_icon: egui::TextureHandle,
    total_ram: u32,
    skin_tex: egui::TextureHandle,
    /// Skin path the texture was loaded from, to notice changes.
    skin_loaded: Option<PathBuf>,
    skin_error: Option<String>,
    viewer: viewer::ViewerState,
    mods_state: Arc<Mutex<ModsState>>,
    mod_query: String,
    mod_loader: Option<&'static str>,
    /// When the search text last changed (searched after a short pause).
    mod_dirty: Option<Instant>,
    mod_searched_once: bool,
    mod_selected: Option<mods::Hit>,
    mod_icons: HashMap<String, egui::TextureHandle>,
    /// Selected entry in the Horizon tab of the mods page.
    mod_hz_selected: Option<String>,
    shaders_state: Arc<Mutex<ShaderState>>,
    shader_query: String,
    /// When the search text last changed (searched after a short pause).
    shader_dirty: Option<Instant>,
    shader_searched_once: bool,
    shader_selected: Option<mods::Hit>,
    /// Install shaders for «Horizon OptiFine» (true) or for the selected Java version.
    shader_for_horizon: bool,
}

fn version_icon_image() -> egui::ColorImage {
    let img = image::load_from_memory(include_bytes!("../assets/version_block.png")).expect("version_block.png").to_rgba8();
    skin::to_color_image(&img)
}

fn child_ui(ui: &mut Ui, rect: Rect, salt: &str, layout: Layout) -> Ui {
    ui.new_child(UiBuilder::new().max_rect(rect).layout(layout).id_salt(salt))
}

fn open_folder(path: &Path) {
    let _ = std::fs::create_dir_all(path);
    let _ = std::process::Command::new(if cfg!(windows) { "explorer" } else { "open" }).arg(path).spawn();
}

fn kind_ru(kind: &str) -> &'static str {
    match kind {
        "release" => "Релиз",
        "snapshot" => "Снапшот",
        "old_beta" => "Beta",
        "old_alpha" => "Alpha",
        _ => "Версия",
    }
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup(&cc.egui_ctx);
        let cfg = Config::load();
        let horizon_first = cfg.target.as_ref().is_some_and(|t| t.is_horizon());
        let mut app = Self {
            saved: cfg.clone(),
            cfg,
            page: Page::Home,
            filter: Filter::Release,
            search: String::new(),
            remote: Arc::default(),
            task: Arc::default(),
            speed: VecDeque::from(vec![0.0; SPEED_SAMPLES]),
            last_bytes: 0,
            last_sample: Instant::now(),
            installed: vec![],
            horizon_local: None,
            was_running: false,
            total_ram: system::total_ram_mb(),
            skin_tex: cc.egui_ctx.load_texture("skin", skin::to_color_image(&skin::default_skin()), egui::TextureOptions::NEAREST),
            skin_loaded: None,
            version_icon: cc.egui_ctx.load_texture("version_block", version_icon_image(), egui::TextureOptions::LINEAR),
            skin_error: None,
            viewer: viewer::ViewerState::default(),
            mods_state: Arc::default(),
            mod_query: String::new(),
            // Playing Horizon: the mods page opens on its Horizon tab.
            mod_loader: horizon_first.then_some("horizon"),
            mod_dirty: None,
            mod_searched_once: false,
            mod_selected: None,
            mod_icons: HashMap::new(),
            mod_hz_selected: None,
            shaders_state: Arc::default(),
            shader_query: String::new(),
            shader_dirty: None,
            shader_searched_once: false,
            shader_selected: None,
            shader_for_horizon: true,
            logo: cc.egui_ctx.load_texture(
                "logo",
                egui::ColorImage::from_rgba_unmultiplied([crate::LOGO_MARK_SIZE as usize; 2], crate::LOGO_MARK),
                egui::TextureOptions::LINEAR,
            ),
        };
        app.refresh_installed();
        app.reload_skin();
        app.fetch_remote(&cc.egui_ctx);
        app
    }

    fn fetch_remote(&self, ctx: &egui::Context) {
        let remote = self.remote.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let manifest = Manifest::fetch();
            let fabric = fabric::game_versions();
            let mut r = remote.lock().unwrap();
            match manifest {
                Ok(m) => r.manifest = Some(m),
                Err(e) => r.errors.push(format!("Mojang: {e:#}")),
            }
            match fabric {
                Ok(f) => r.fabric = f,
                Err(e) => r.errors.push(format!("Fabric: {e:#}")),
            }
            ctx.request_repaint();
        });
    }

    fn refresh_installed(&mut self) {
        // Folders whose assets/libraries can be reused instead of downloaded again.
        let official = Some(crate::config::official_minecraft_dir());
        let mut mirrors: Vec<PathBuf> = [Some(self.cfg.tlauncher_dir.clone()), official, Some(self.cfg.game_dir.clone())].into_iter().flatten().collect();
        mirrors.dedup();
        crate::net::set_mirrors(mirrors);
        let mut list: Vec<(String, bool)> = minecraft::list_installed(&self.cfg.game_dir).into_iter().map(|id| (id, false)).collect();
        if self.cfg.show_tlauncher && self.cfg.tlauncher_dir != self.cfg.game_dir {
            list.extend(minecraft::list_installed(&self.cfg.tlauncher_dir).into_iter().map(|id| (id, true)));
        }
        self.installed = list;
        self.horizon_local = horizon::installed(&self.cfg.game_dir);
    }

    fn entries(&self) -> Vec<Entry> {
        let remote = self.remote.lock().unwrap();
        let local: HashSet<&str> = self.installed.iter().filter(|(_, tl)| !tl).map(|(id, _)| id.as_str()).collect();
        let mut out = Vec::new();
        if matches!(self.filter, Filter::Release | Filter::Installed) && (self.filter == Filter::Release || self.horizon_local.is_some()) {
            out.push(Entry {
                target: Target::Horizon,
                title: Target::Horizon.title(),
                sub: "Minecraft 26.3, переписанный на Rust".into(),
                installed: self.horizon_local.is_some(),
            });
            out.push(Entry {
                target: Target::HorizonFine,
                title: Target::HorizonFine.title(),
                sub: "Тот же Horizon + Zyntra Fine и зум".into(),
                installed: self.horizon_local.is_some(),
            });
        }
        match self.filter {
            Filter::Release | Filter::Snapshot | Filter::Old => {
                for v in remote.manifest.iter().flat_map(|m| &m.versions) {
                    let ok = match self.filter {
                        Filter::Release => v.kind == "release",
                        Filter::Snapshot => v.kind == "snapshot",
                        _ => v.kind.starts_with("old_"),
                    };
                    if ok {
                        out.push(Entry {
                            target: Target::Vanilla(v.id.clone()),
                            title: v.id.clone(),
                            sub: format!("{} · {}", kind_ru(&v.kind), v.release_time.get(..10).unwrap_or_default()),
                            installed: local.contains(v.id.as_str()),
                        });
                    }
                }
            }
            Filter::Fabric => {
                for (game, _) in remote.fabric.iter().filter(|(_, stable)| *stable) {
                    let suffix = format!("-{game}");
                    out.push(Entry {
                        target: Target::Fabric(game.clone()),
                        title: format!("Fabric {game}"),
                        sub: "Последний стабильный загрузчик Fabric".into(),
                        installed: local.iter().any(|id| id.starts_with("fabric-loader-") && id.ends_with(&suffix)),
                    });
                }
            }
            Filter::Installed => {
                for (id, tl) in &self.installed {
                    let loader = loader_of(id);
                    out.push(Entry {
                        target: Target::Installed { id: id.clone(), tl: *tl },
                        title: id.clone(),
                        sub: format!("{loader} · {}", if *tl { "папка TLauncher" } else { "Zyntra" }),
                        installed: true,
                    });
                }
            }
        }
        let q = self.search.trim().to_lowercase();
        if !q.is_empty() {
            out.retain(|e| e.title.to_lowercase().contains(&q));
        }
        out
    }

    // ---------------------------------------------------------------- jobs

    fn start(&mut self, ctx: &egui::Context, target: Target, play: bool) {
        if play && !target.is_horizon() && !self.cfg.nickname_valid() {
            self.page = Page::Settings;
            return;
        }
        let cfg = self.cfg.clone();
        let manifest = self.remote.lock().unwrap().manifest.clone();
        let label = format!("Подготовка {}", target.title());
        let (close, hide) = (cfg.close_on_launch, cfg.hide_on_launch);
        self.spawn_job(ctx, &label, close, hide, move |task| run_target(&cfg, manifest.as_ref(), &target, play, task));
    }

    /// Runs `job` in the background. If it starts the game, the launcher closes or hides
    /// itself (as configured) and, when hidden, comes back once the game exits.
    fn spawn_job(
        &mut self,
        ctx: &egui::Context,
        label: &str,
        close_on_launch: bool,
        hide_on_launch: bool,
        job: impl FnOnce(&Task) -> Result<Option<Child>> + Send + 'static,
    ) {
        let task = self.task.clone();
        if !task.begin(label) {
            return;
        }
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            match job(&task) {
                Ok(Some(mut child)) => {
                    task.finish("Игра запущена — приятной игры!");
                    ctx.request_repaint();
                    if close_on_launch {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    } else if hide_on_launch {
                        crate::window::hide();
                    }
                    let status = child.wait();
                    if hide_on_launch && !close_on_launch {
                        crate::window::show();
                    }
                    match status {
                        Ok(s) if s.success() => task.log("Игра закрыта"),
                        Ok(s) => task.log(format!("Игра завершилась с кодом {} — смотри лог в папке logs", s.code().unwrap_or(-1))),
                        Err(e) => task.log(format!("Ошибка ожидания процесса: {e}")),
                    }
                }
                Ok(None) => task.finish("Готово"),
                Err(e) => task.fail(format!("{e:#}")),
            }
            ctx.request_repaint();
        });
    }

    // ---------------------------------------------------------------- frame

    fn sample_speed(&mut self) {
        let dt = self.last_sample.elapsed();
        if dt < Duration::from_millis(250) {
            return;
        }
        let bytes = self.task.bytes.load(std::sync::atomic::Ordering::Relaxed);
        let mbps = bytes.saturating_sub(self.last_bytes) as f32 / dt.as_secs_f32() / 1_048_576.0;
        self.speed.push_back(mbps);
        while self.speed.len() > SPEED_SAMPLES {
            self.speed.pop_front();
        }
        self.last_bytes = bytes;
        self.last_sample = Instant::now();
    }

    fn sidebar(&mut self, ui: &mut Ui, rect: Rect) {
        let p = ui.painter().clone();
        p.rect(rect, 22, PANEL, Stroke::new(1.0_f32, BORDER), StrokeKind::Inside);
        // Glow sits well inside the panel so the rounded corners stay clean.
        card_glow(&p, rect, 22.0, BG, rect.left_top() + vec2(rect.width() * 0.45, 70.0), 220.0, glow_color(26), BORDER);

        // Same artwork as the window / .exe icon.
        let logo = Rect::from_min_size(rect.min + vec2(18.0, 22.0), vec2(44.0, 44.0));
        p.add(egui::epaint::Shadow { offset: [0, 0], blur: 24, spread: 0, color: glow_color(120) }.as_shape(logo.shrink(4.0), 10));
        p.image(self.logo.id(), logo, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        // Wordmark styled like the logo: ZYNTRA over a letter-spaced LAUNCHER
        text(&p, logo.right_center() + vec2(12.0, -8.0), Align2::LEFT_CENTER, "ZYNTRA", bold(22.0), TEXT);
        text(&p, logo.right_center() + vec2(13.0, 13.0), Align2::LEFT_CENTER, "L A U N C H E R", font(10.5), ACCENT);

        let items = [(Page::Home, "🏠", "Главная"), (Page::Versions, "📦", "Версии"), (Page::Skins, "👤", "Скины"), (Page::Mods, "cube", "Моды"), (Page::Shaders, "star", "Шейдеры"), (Page::Fine, "sliders", "Zyntra Fine"), (Page::Console, "🖥", "Консоль")];
        let mut y = rect.top() + 100.0;
        for (page, icon, label) in items {
            let r = Rect::from_min_size(Pos2::new(rect.left() + 14.0, y), vec2(rect.width() - 28.0, 46.0));
            if nav_item(ui, r, icon, label, self.page == page) {
                self.page = page;
            }
            y += 52.0;
        }

        // Decoration in the empty middle: a small isometric pile of the version block,
        // with two smaller blocks floating above it.
        let area = Rect::from_min_max(Pos2::new(rect.left() + 14.0, y + 10.0), Pos2::new(rect.right() - 14.0, rect.bottom() - 132.0));
        if area.height() > 150.0 {
            let w = (area.width() * 0.2).min(44.0);
            let base = area.center_bottom() - vec2(w * 0.5, w * 1.6);
            // Step sizes match the cube inside version_block.png (it has a small transparent margin).
            let iso = |i: f32, j: f32, k: f32| base + vec2((i - j) * w * 0.45, (i + j) * w * 0.235 - k * w * 0.49);
            let mut pile: [(f32, f32, f32); 8] = [(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0), (1.0, 1.0, 0.0), (2.0, 0.0, 0.0), (0.0, 0.0, 1.0), (1.0, 0.0, 1.0), (0.0, 0.0, 2.0)];
            pile.sort_by(|a, b| (a.0 + a.1 + a.2).total_cmp(&(b.0 + b.1 + b.2)).then(a.2.total_cmp(&b.2)));
            glow(&p, rect, iso(1.0, 1.0, 0.0) + vec2(w * 0.5, w * 0.6), w * 2.6, glow_color(45));
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            for (i, j, k) in pile {
                let r = Rect::from_min_size(iso(i, j, k), vec2(w, w));
                p.image(self.version_icon.id(), r, uv, Color32::from_gray(225));
            }
            for (offset, size, alpha) in [(vec2(w * 1.5, -w * 1.3), w * 0.55, 150u8), (vec2(-w * 0.8, -w * 1.7), w * 0.4, 110u8)] {
                let pos = iso(0.0, 0.0, 2.0) + offset;
                let r = Rect::from_min_size(Pos2::new(pos.x, pos.y.max(area.top())), vec2(size, size));
                p.image(self.version_icon.id(), r, uv, Color32::from_rgba_unmultiplied(255, 255, 255, alpha));
            }
        }

        let r = Rect::from_min_size(Pos2::new(rect.left() + 14.0, rect.bottom() - 120.0), vec2(rect.width() - 28.0, 46.0));
        if nav_item(ui, r, "⚙", "Настройки", self.page == Page::Settings) {
            self.page = Page::Settings;
        }
        let r = r.translate(vec2(0.0, 52.0));
        if nav_item(ui, r, "folder", "Папка игры", false) {
            open_folder(&self.cfg.game_dir);
        }
        text(&p, Pos2::new(rect.center().x, rect.bottom() - 12.0), Align2::CENTER_BOTTOM, concat!("v", env!("CARGO_PKG_VERSION"), " · без рекламы"), font(11.0), MUTED);
    }

    fn header(&mut self, ui: &mut Ui, rect: Rect) {
        let p = ui.painter().clone();
        let nick = if self.cfg.nickname.is_empty() { "игрок" } else { &self.cfg.nickname };
        let r1 = text(&p, rect.left_top() + vec2(2.0, 2.0), Align2::LEFT_TOP, "Добро пожаловать, ", bold(28.0), TEXT);
        text(&p, r1.right_top(), Align2::LEFT_TOP, nick, bold(28.0), ACCENT);
        text(&p, Pos2::new(rect.left() + 2.0, r1.bottom() + 4.0), Align2::LEFT_TOP, "Все версии Minecraft и запуск в один клик — без рекламы", font(13.5), MUTED);

        // Profile block
        // Profile block: the skin's face as a pixel avatar (click → skins page)
        let avatar = Pos2::new(rect.right() - 180.0, rect.center().y);
        let head = Rect::from_center_size(avatar, vec2(40.0, 40.0));
        p.add(egui::epaint::Shadow { offset: [0, 0], blur: 18, spread: 0, color: glow_color(110) }.as_shape(head, 6));
        p.rect_filled(head.expand(2.0), 7, ACCENT);
        let face = |(u, v): (f32, f32)| Rect::from_min_size(Pos2::new(u, v), vec2(8.0 / 64.0, 8.0 / 64.0));
        p.image(self.skin_tex.id(), head, face(skin::HEAD_UV), Color32::WHITE);
        p.image(self.skin_tex.id(), head, face(skin::HAT_UV), Color32::WHITE);
        if ui.interact(head.expand(4.0), ui.id().with("avatar"), Sense::click()).on_hover_text("Сменить скин").on_hover_cursor(CursorIcon::PointingHand).clicked() {
            self.page = Page::Skins;
        }
        text_fit(&p, avatar + vec2(30.0, -9.0), Align2::LEFT_CENTER, nick, bold(15.0), TEXT, 145.0);
        text(&p, avatar + vec2(30.0, 10.0), Align2::LEFT_CENTER, "Офлайн-профиль", font(12.0), MUTED);

        let buttons = Rect::from_min_max(Pos2::new(avatar.x - 130.0, rect.top()), Pos2::new(avatar.x - 34.0, rect.bottom()));
        let mut c = child_ui(ui, buttons, "header_buttons", Layout::right_to_left(Align::Center));
        if circle_button(&mut c, "⚙", "Настройки").clicked() {
            self.page = Page::Settings;
        }
        if circle_button(&mut c, "folder", "Открыть папку игры").clicked() {
            open_folder(&self.cfg.game_dir);
        }
    }

    fn filter_bar(&mut self, ui: &mut Ui, rect: Rect) {
        let mut c = child_ui(ui, rect, "filters", Layout::left_to_right(Align::Center));
        c.spacing_mut().item_spacing.x = 8.0;
        for f in Filter::ALL {
            if pill(&mut c, f.label(), self.filter == f).clicked() {
                self.filter = f;
            }
        }
        // Search box on the right
        let search = Rect::from_min_max(Pos2::new(rect.right() - 300.0, rect.top()), rect.max);
        let p = ui.painter().clone();
        p.rect(search, 20, SUNKEN, Stroke::new(1.2_f32, BORDER), StrokeKind::Inside);
        text(&p, search.left_center() + vec2(18.0, 0.0), Align2::LEFT_CENTER, "🔍", font(14.0), MUTED);
        let edit = Rect::from_min_max(search.min + vec2(44.0, 9.0), search.max - vec2(16.0, 7.0));
        let mut s = child_ui(ui, edit, "search", Layout::left_to_right(Align::Center));
        s.add(egui::TextEdit::singleline(&mut self.search).frame(false).hint_text("Найти версию…").desired_width(edit.width()).font(font(14.5)));
    }

    // ---------------------------------------------------------------- pages

    fn home(&mut self, ctx: &egui::Context, ui: &mut Ui, body: Rect) {
        let bar = Rect::from_min_size(body.min, vec2(body.width(), 40.0));
        self.filter_bar(ui, bar);

        let mid_top = bar.bottom() + GAP;
        let mid_h = ((body.bottom() - mid_top - GAP) * 0.6).max(280.0);
        let w = body.width();
        let c1 = (w - 2.0 * GAP) * 0.30;
        let c2 = (w - 2.0 * GAP) * 0.37;
        let c3 = w - 2.0 * GAP - c1 - c2;
        let col1 = Rect::from_min_size(Pos2::new(body.left(), mid_top), vec2(c1, mid_h));
        let col2 = Rect::from_min_size(Pos2::new(col1.right() + GAP, mid_top), vec2(c2, mid_h));
        let col3 = Rect::from_min_size(Pos2::new(col2.right() + GAP, mid_top), vec2(c3, mid_h));
        let bottom = Rect::from_min_max(Pos2::new(body.left(), col1.bottom() + GAP), body.max);

        // Column 1: selected version + quick profile
        let sel_h = (mid_h - GAP - 160.0).min(mid_h * 0.45).max(118.0);
        let sel = Rect::from_min_size(col1.min, vec2(c1, sel_h));
        let profile = Rect::from_min_max(Pos2::new(col1.left(), sel.bottom() + GAP), col1.max);
        self.selected_card(ui, sel);
        self.profile_card(ui, profile);

        // Column 2: version list
        let p = ui.painter().clone();
        card(&p, col2);
        let entries = self.entries();
        text(&p, col2.left_top() + vec2(20.0, 20.0), Align2::LEFT_TOP, "Версии", bold(18.0), TEXT);
        text(&p, col2.right_top() + vec2(-20.0, 24.0), Align2::RIGHT_TOP, format!("{} · {}", self.filter.label(), entries.len()), font(12.5), MUTED);
        let list = Rect::from_min_max(col2.min + vec2(10.0, 56.0), col2.max - vec2(10.0, 12.0));
        let mut c = child_ui(ui, list, "home_list", Layout::top_down(Align::Min));
        let mut play = None;
        egui::ScrollArea::vertical().id_salt("home_scroll").auto_shrink([false; 2]).show(&mut c, |ui| {
            if entries.is_empty() {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| ui.label(egui::RichText::new(self.empty_hint()).color(MUTED)));
            }
            for e in &entries {
                let selected = self.cfg.target.as_ref() == Some(&e.target);
                let resp = list_row(ui, e, selected, false, self.version_icon.id());
                if resp.row.clicked() {
                    self.cfg.target = Some(e.target.clone());
                }
                if resp.row.double_clicked() {
                    play = Some(e.target.clone());
                }
            }
        });

        // Column 3: installed tiles
        card(&p, col3);
        text(&p, col3.left_top() + vec2(20.0, 20.0), Align2::LEFT_TOP, "Мои версии", bold(18.0), TEXT);
        let all_btn = Rect::from_min_size(col3.right_top() + vec2(-128.0, 14.0), vec2(112.0, 34.0));
        let mut b = child_ui(ui, all_btn, "all_btn", Layout::right_to_left(Align::Center));
        if pill(&mut b, "Все", false).on_hover_text("Все установленные версии").clicked() {
            self.filter = Filter::Installed;
            self.page = Page::Versions;
        }
        let grid = Rect::from_min_max(col3.min + vec2(16.0, 62.0), col3.max - vec2(16.0, 16.0));
        if self.installed.is_empty() {
            text(&p, grid.center(), Align2::CENTER_CENTER, "Пока ничего не установлено", font(13.5), MUTED);
        }
        let tile_w = (grid.width() - 12.0) / 2.0;
        let tile_h = (grid.height() - 12.0) / 2.0;
        let mut tiles: Vec<(Target, String, &str, String)> = Vec::new();
        if self.horizon_local.is_some() {
            tiles.push((Target::HorizonFine, Target::HorizonFine.title(), "Fine + зум", "Horizon".into()));
        }
        for (id, tl) in &self.installed {
            let source = if *tl { "из TLauncher" } else { "Zyntra" };
            tiles.push((Target::Installed { id: id.clone(), tl: *tl }, id.clone(), source, loader_of(id).into()));
        }
        for (i, (target, id, source, loader)) in tiles.into_iter().take(4).enumerate() {
            let pos = grid.min + vec2((i % 2) as f32 * (tile_w + 12.0), (i / 2) as f32 * (tile_h + 12.0));
            let r = Rect::from_min_size(pos, vec2(tile_w, tile_h));
            let resp = ui.interact(r, ui.id().with(("tile", i)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            let selected = self.cfg.target.as_ref() == Some(&target);
            p.rect(r, 16, if resp.hovered() || selected { CARD_HI } else { SUNKEN }, Stroke::new(1.0_f32, if selected { ACCENT_DIM } else { BORDER }), StrokeKind::Inside);
            if selected {
                card_glow(&p, r, 16.0, CARD, r.right_top(), r.width() * 0.8, glow_color(40), ACCENT_DIM);
            }
            text_fit(&p, r.left_top() + vec2(14.0, 14.0), Align2::LEFT_TOP, &id, bold(15.0), TEXT, r.width() - 24.0);
            text(&p, r.left_top() + vec2(14.0, 38.0), Align2::LEFT_TOP, source, font(12.0), GREEN);
            block_icon(&p, Rect::from_min_size(r.left_bottom() + vec2(12.0, -42.0), vec2(32.0, 32.0)), self.version_icon.id());
            text_fit(&p, r.left_bottom() + vec2(50.0, -26.0), Align2::LEFT_CENTER, &loader, font(12.0), MUTED, r.width() - 100.0);
            let play_c = r.right_bottom() + vec2(-28.0, -26.0);
            p.circle(play_c, 14.0, if resp.hovered() { ACCENT } else { CARD_HI }, Stroke::new(1.0_f32, BORDER));
            text(&p, play_c + vec2(1.0, 0.0), Align2::CENTER_CENTER, "▶", font(12.0), if resp.hovered() { Color32::from_rgb(40, 18, 48) } else { TEXT });
            if resp.clicked() {
                self.cfg.target = Some(target.clone());
            }
            if resp.double_clicked() {
                play = Some(target);
            }
        }

        // Bottom: launch panel + speed chart
        self.launch_card(ctx, ui, bottom);
        if let Some(t) = play {
            self.cfg.target = Some(t.clone());
            self.start(ctx, t, true);
        }
    }

    fn empty_hint(&self) -> String {
        let r = self.remote.lock().unwrap();
        if self.filter != Filter::Installed && r.manifest.is_none() {
            if r.errors.is_empty() { "Загрузка списка версий…".into() } else { "Нет связи с серверами Mojang".into() }
        } else {
            "Ничего не найдено".into()
        }
    }

    fn selected_card(&mut self, ui: &mut Ui, rect: Rect) {
        let p = ui.painter().clone();
        card(&p, rect);
        card_glow(&p, rect, 20.0, BG, rect.right_bottom(), rect.width() * 0.7, glow_color(35), BORDER);
        text(&p, rect.left_top() + vec2(20.0, 18.0), Align2::LEFT_TOP, "Версия", bold(16.0), TEXT);
        let btn = Rect::from_min_size(rect.right_top() + vec2(-122.0, 14.0), vec2(106.0, 34.0));
        let mut c = child_ui(ui, btn, "change_btn", Layout::right_to_left(Align::Center));
        if pill(&mut c, "Сменить", false).clicked() {
            self.page = Page::Versions;
        }
        let (title, kind) = match &self.cfg.target {
            Some(t) => (t.title(), t.kind()),
            None => ("—".into(), "Выбери версию в списке".into()),
        };
        // A long name gets a smaller font before it would be cut.
        let max_w = rect.width() - 40.0;
        let size = [28.0, 24.0, 21.0].into_iter().find(|s| p.layout_no_wrap(title.clone(), bold(*s), TEXT).size().x <= max_w).unwrap_or(21.0);
        let big = text_fit(&p, rect.left_top() + vec2(20.0, 50.0), Align2::LEFT_TOP, &title, bold(size), TEXT, max_w);
        text_fit(&p, Pos2::new(rect.left() + 20.0, big.bottom() + 6.0), Align2::LEFT_TOP, &kind, font(13.0), ACCENT, rect.width() - 40.0);
    }

    /// Quick profile: nickname and RAM right on the home page.
    fn profile_card(&mut self, ui: &mut Ui, rect: Rect) {
        let p = ui.painter().clone();
        card(&p, rect);
        card_glow(&p, rect, 20.0, BG, rect.center_bottom() + vec2(0.0, 30.0), rect.width() * 0.75, glow_color(100), BORDER);
        text(&p, rect.left_top() + vec2(20.0, 18.0), Align2::LEFT_TOP, "Профиль", bold(16.0), TEXT);
        let left = rect.left() + 20.0;
        let right = rect.right() - 20.0;

        // Nickname field
        let field = Rect::from_min_max(Pos2::new(left, rect.top() + 48.0), Pos2::new(right, rect.top() + 78.0));
        p.rect(field, 10, SUNKEN, Stroke::new(1.0_f32, BORDER), StrokeKind::Inside);
        // Exact rect with no inner margin, text centred vertically in the field
        let edit = Rect::from_min_max(Pos2::new(field.left() + 12.0, field.top() + 2.0), Pos2::new(field.right() - 30.0, field.bottom() - 2.0));
        ui.put(
            edit,
            egui::TextEdit::singleline(&mut self.cfg.nickname)
                .frame(false)
                .margin(egui::Margin::ZERO)
                .vertical_align(Align::Center)
                .min_size(edit.size())
                .desired_width(edit.width())
                .font(font(14.5))
                .char_limit(16)
                .hint_text("Ник в игре"),
        );
        let (mark, color) = if self.cfg.nickname_valid() { ("✔", GREEN) } else { ("!", RED) };
        text(&p, field.right_center() - vec2(14.0, 0.0), Align2::CENTER_CENTER, mark, bold(14.0), color);

        // Memory
        let total = self.total_ram;
        let max = system::max_game_ram_mb(total);
        let recommended = system::recommended_ram_mb(total);
        let y = field.bottom() + 12.0;
        text(&p, Pos2::new(left, y), Align2::LEFT_TOP, "Память", font(13.0), MUTED);
        let total_label = text(&p, Pos2::new(right, y), Align2::RIGHT_TOP, format!(" / {} ГБ", (total + 512) / 1024), font(13.0), MUTED);
        text(&p, total_label.left_top(), Align2::RIGHT_TOP, format!("{:.1} ГБ", self.cfg.ram_mb as f32 / 1024.0), bold(13.5), ACCENT);
        let slider = Rect::from_min_max(Pos2::new(left - 6.0, y + 20.0), Pos2::new(right + 6.0, y + 38.0));
        if ram_slider(ui, slider, &mut self.cfg.ram_mb, 1024, max, recommended) {
            self.cfg.ram_auto = false;
        }
        let hint_y = slider.bottom() + 6.0;
        if hint_y + 14.0 < rect.bottom() - 6.0 {
            let hint = if self.cfg.ram_auto {
                format!("Авто: {:.1} ГБ — подобрано под этот ПК", recommended as f32 / 1024.0)
            } else {
                format!("Рекомендуется {:.1} ГБ (зелёная метка)", recommended as f32 / 1024.0)
            };
            text_fit(&p, Pos2::new(left, hint_y), Align2::LEFT_TOP, &hint, font(11.5), MUTED, right - left);
        }

        // Auto toggle
        let auto = Rect::from_min_size(Pos2::new(right - 74.0, rect.top() + 9.0), vec2(74.0, 34.0));
        let mut b = child_ui(ui, auto, "ram_auto", Layout::right_to_left(Align::Center));
        if pill(&mut b, "Авто", self.cfg.ram_auto).on_hover_text("Подбирать память автоматически").clicked() {
            self.cfg.ram_auto = !self.cfg.ram_auto;
        }
    }

    fn launch_card(&mut self, ctx: &egui::Context, ui: &mut Ui, rect: Rect) {
        let p = ui.painter().clone();
        card(&p, rect);
        let left = Rect::from_min_size(rect.min + vec2(22.0, 20.0), vec2((rect.width() * 0.32).max(280.0), rect.height() - 40.0));
        let chart = Rect::from_min_max(Pos2::new(left.right() + 30.0, rect.top() + 20.0), rect.max - vec2(20.0, 16.0));

        text(&p, left.left_top(), Align2::LEFT_TOP, "Запуск игры", bold(18.0), TEXT);
        let running = self.task.running();
        let state = self.task.snapshot();
        let title = self.cfg.target.as_ref().map(Target::title).unwrap_or_else(|| "Версия не выбрана".into());
        text_fit(&p, left.left_top() + vec2(0.0, 32.0), Align2::LEFT_TOP, &title, font(14.0), MUTED, left.width());

        let btn = Rect::from_min_size(left.left_top() + vec2(0.0, 64.0), vec2(left.width(), 56.0));
        let mut c = child_ui(ui, btn, "play_btn", Layout::top_down(Align::Min));
        let can_play = !running && self.cfg.target.is_some();
        let label = if running { "Подготовка…" } else { "▶   ИГРАТЬ" };
        if primary(&mut c, label, btn.size(), can_play).clicked() {
            if let Some(t) = self.cfg.target.clone() {
                self.start(ctx, t, true);
            }
        }

        let bar = Rect::from_min_size(btn.left_bottom() + vec2(0.0, 22.0), vec2(left.width(), 8.0));
        let frac = self.task.fraction();
        if running {
            progress(&p, bar, frac, ctx.input(|i| i.time));
        } else {
            progress(&p, bar, Some(if state.error.is_some() { 0.0 } else { 1.0 }), 0.0);
        }
        let (done, total) = self.task.counts();
        let status = match (&state.error, running) {
            (Some(e), false) => e.lines().next().unwrap_or_default().to_owned(),
            _ if !self.cfg.nickname_valid() => "Укажи ник (3–16 символов: A–Z, 0–9, _) в настройках".into(),
            _ if state.stage.is_empty() => "Готов к запуску".into(),
            _ => state.stage.clone(),
        };
        let color = if state.error.is_some() && !running { RED } else { MUTED };
        text_fit(&p, bar.left_bottom() + vec2(0.0, 12.0), Align2::LEFT_TOP, &status, font(12.5), color, left.width() - 60.0);
        if running && total > 0 {
            text(&p, bar.right_bottom() + vec2(0.0, 12.0), Align2::RIGHT_TOP, format!("{done}/{total}"), font(12.5), ACCENT);
        }

        // Chart
        text(&p, chart.left_top(), Align2::LEFT_TOP, "Скорость загрузки", bold(18.0), TEXT);
        let now = self.speed.back().copied().unwrap_or_default();
        text(&p, chart.left_top() + vec2(0.0, 30.0), Align2::LEFT_TOP, format!("{now:.1} МБ/с"), bold(15.0), ACCENT);
        let btns = Rect::from_min_size(Pos2::new(chart.right() - 260.0, chart.top() - 4.0), vec2(260.0, 34.0));
        let mut b = child_ui(ui, btns, "chart_btns", Layout::right_to_left(Align::Center));
        b.spacing_mut().item_spacing.x = 8.0;
        if pill(&mut b, "Консоль", false).clicked() {
            self.page = Page::Console;
        }
        if pill(&mut b, "Логи игры", false).clicked() {
            open_folder(&self.cfg.game_dir.join("logs"));
        }
        let values: Vec<f32> = self.speed.iter().copied().collect();
        area_chart(ui, Rect::from_min_max(chart.left_top() + vec2(0.0, 60.0), chart.max), &values, "МБ/с");
    }

    fn versions(&mut self, ctx: &egui::Context, ui: &mut Ui, body: Rect) {
        let bar = Rect::from_min_size(body.min, vec2(body.width(), 40.0));
        self.filter_bar(ui, bar);
        let rest = Rect::from_min_max(Pos2::new(body.left(), bar.bottom() + GAP), body.max);
        let rect = rest;
        let p = ui.painter().clone();
        card(&p, rect);
        let entries = self.entries();
        text(&p, rect.left_top() + vec2(22.0, 20.0), Align2::LEFT_TOP, self.filter.label(), bold(20.0), TEXT);
        text(&p, rect.left_top() + vec2(22.0, 48.0), Align2::LEFT_TOP, versions_hint(self.filter), font(12.5), MUTED);
        let list = Rect::from_min_max(rect.min + vec2(12.0, 80.0), rect.max - vec2(12.0, 12.0));
        let mut c = child_ui(ui, list, "versions_list", Layout::top_down(Align::Min));
        let running = self.task.running();
        let mut action = None;
        egui::ScrollArea::vertical().id_salt("versions_scroll").auto_shrink([false; 2]).show(&mut c, |ui| {
            if entries.is_empty() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| ui.label(egui::RichText::new(self.empty_hint()).color(MUTED)));
            }
            for e in &entries {
                let selected = self.cfg.target.as_ref() == Some(&e.target);
                let r = list_row(ui, e, selected, !running, self.version_icon.id());
                if r.row.clicked() {
                    self.cfg.target = Some(e.target.clone());
                }
                if r.install {
                    action = Some((e.target.clone(), false));
                }
                if r.play || r.row.double_clicked() {
                    action = Some((e.target.clone(), true));
                }
            }
        });
        if let Some((t, play)) = action {
            self.cfg.target = Some(t.clone());
            self.start(ctx, t, play);
            if play {
                self.page = Page::Home;
            }
        }
    }

    /// Loads the chosen skin (or the default character) into the shared texture.
    fn reload_skin(&mut self) {
        self.skin_loaded = self.cfg.skin_path.clone();
        self.skin_error = None;
        let image = match &self.cfg.skin_path {
            Some(path) => match skin::load(path) {
                Ok(img) => img,
                Err(e) => {
                    self.skin_error = Some(format!("{e:#}"));
                    self.cfg.skin_path = None;
                    self.skin_loaded = None;
                    skin::default_skin()
                }
            },
            None => skin::default_skin(),
        };
        self.skin_tex.set(skin::to_color_image(&image), egui::TextureOptions::NEAREST);
    }

    fn skins(&mut self, ui: &mut Ui, body: Rect) {
        let p = ui.painter().clone();
        let side_w = (body.width() * 0.36).clamp(300.0, 420.0);
        let stage = Rect::from_min_max(body.min, Pos2::new(body.right() - side_w - GAP, body.bottom()));
        let side = Rect::from_min_max(Pos2::new(stage.right() + GAP, body.top()), body.max);

        // 3D model
        card(&p, stage);
        card_glow(&p, stage, 20.0, BG, stage.center_bottom(), stage.width() * 0.6, glow_color(60), BORDER);
        pixel_dust(&p, stage.right_top() + vec2(-60.0, 70.0), vec2(1.0, 1.0), 10.0, glow_color(120));
        pixel_dust(&p, stage.left_bottom() + vec2(30.0, -60.0), vec2(1.0, 1.0), 8.0, glow_color(90));
        text(&p, stage.left_top() + vec2(22.0, 20.0), Align2::LEFT_TOP, "Персонаж", bold(20.0), TEXT);
        text(&p, stage.left_top() + vec2(22.0, 48.0), Align2::LEFT_TOP, "Зажми мышь и крути · двойной клик — сбросить", font(12.5), MUTED);
        let nick = if self.cfg.nickname.is_empty() { "Player" } else { &self.cfg.nickname };
        let model = Rect::from_min_max(stage.min + vec2(20.0, 110.0), stage.max - vec2(20.0, 30.0));
        let galley = p.layout_no_wrap(nick.to_owned(), bold(16.0), TEXT);
        let tag = Rect::from_center_size(Pos2::new(model.center().x, model.top() - 14.0), galley.size() + vec2(22.0, 10.0));
        p.rect_filled(tag, 4, Color32::from_black_alpha(150));
        p.galley(tag.center() - galley.size() / 2.0, galley, TEXT);
        viewer::show(ui, model, self.skin_tex.id(), self.cfg.skin_slim, &mut self.viewer);

        // Controls
        card(&p, side);
        text(&p, side.left_top() + vec2(22.0, 20.0), Align2::LEFT_TOP, "Твой скин", bold(20.0), TEXT);
        let face = Rect::from_min_size(side.left_top() + vec2(22.0, 62.0), vec2(72.0, 72.0));
        p.add(egui::epaint::Shadow { offset: [0, 0], blur: 20, spread: 0, color: glow_color(100) }.as_shape(face, 6));
        let uv = |(u, v): (f32, f32)| Rect::from_min_size(Pos2::new(u, v), vec2(8.0 / 64.0, 8.0 / 64.0));
        p.image(self.skin_tex.id(), face, uv(skin::HEAD_UV), Color32::WHITE);
        p.image(self.skin_tex.id(), face, uv(skin::HAT_UV), Color32::WHITE);
        let name = match &self.cfg.skin_path {
            Some(path) => path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            None => "Скин по умолчанию".into(),
        };
        text_fit(&p, face.right_top() + vec2(16.0, 14.0), Align2::LEFT_TOP, &name, bold(15.0), TEXT, side.right() - face.right() - 40.0);
        let kind = if self.cfg.skin_slim { "Тонкие руки" } else { "Классические руки" };
        text(&p, face.right_top() + vec2(16.0, 40.0), Align2::LEFT_TOP, kind, font(12.5), ACCENT);

        let inner = Rect::from_min_max(Pos2::new(side.left() + 22.0, face.bottom() + 22.0), side.max - vec2(22.0, 18.0));
        let mut c = child_ui(ui, inner, "skin_controls", Layout::top_down(Align::Min));
        c.spacing_mut().item_spacing = vec2(8.0, 12.0);
        if primary(&mut c, "Выбрать файл скина…", vec2(inner.width(), 48.0), true).clicked() {
            if let Some(file) = rfd::FileDialog::new().add_filter("Скин PNG", &["png"]).pick_file() {
                self.cfg.skin_path = Some(file);
            }
        }
        c.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if pill(ui, "Классические", !self.cfg.skin_slim).clicked() {
                self.cfg.skin_slim = false;
            }
            if pill(ui, "Тонкие руки", self.cfg.skin_slim).clicked() {
                self.cfg.skin_slim = true;
            }
        });
        if self.cfg.skin_path.is_some() && pill(&mut c, "Вернуть стандартный", false).clicked() {
            self.cfg.skin_path = None;
        }
        if let Some(err) = &self.skin_error {
            c.label(egui::RichText::new(err).color(RED).size(12.5));
        }
        c.add_space(6.0);
        c.label(
            egui::RichText::new(
                "Подойдёт любой скин PNG 64×64 или 64×32. В игре скин виден тебе: лаунчер подключает \
                 ресурс-пак ZyntraSkin к обычным версиям. Другие игроки на серверах его не увидят — \
                 для этого нужен сервер со своей системой скинов.",
            )
            .color(MUTED)
            .size(12.5),
        );
    }

    // ---------------------------------------------------------------- mods

    fn mods_search(&mut self, ctx: &egui::Context) {
        self.mod_dirty = None;
        let (query, loader) = (self.mod_query.clone(), self.mod_loader);
        let state = self.mods_state.clone();
        state.lock().unwrap().searching = true;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = mods::search(&query, loader);
            let mut st = state.lock().unwrap();
            st.searching = false;
            match result {
                Ok(hits) => {
                    st.results = hits;
                    st.error = None;
                }
                Err(e) => st.error = Some(format!("Каталог модов недоступен: {e:#}")),
            }
            ctx.request_repaint();
        });
    }

    fn mods_fetch_targets(&self, ctx: &egui::Context, project: &str) {
        let state = self.mods_state.clone();
        if state.lock().unwrap().targets.contains_key(project) {
            return;
        }
        state.lock().unwrap().targets.insert(project.to_owned(), None);
        let (project, ctx) = (project.to_owned(), ctx.clone());
        std::thread::spawn(move || {
            let result = mods::versions(&project).map(|v| mods::targets(&v)).map_err(|e| format!("{e:#}"));
            state.lock().unwrap().targets.insert(project, Some(result));
            ctx.request_repaint();
        });
    }

    /// Icon texture for a mod; starts a background download the first time.
    fn mod_icon(&mut self, ctx: &egui::Context, hit: &mods::Hit) -> Option<egui::TextureId> {
        self.icon_tex(ctx, &hit.project_id, hit.icon_url.as_deref())
    }

    /// Icon texture by id and image URL, downloaded in the background the first time.
    fn icon_tex(&mut self, ctx: &egui::Context, project_id: &str, icon_url: Option<&str>) -> Option<egui::TextureId> {
        if let Some(tex) = self.mod_icons.get(project_id) {
            return Some(tex.id());
        }
        let mut st = self.mods_state.lock().unwrap();
        if let Some(img) = st.icons.remove(project_id) {
            drop(st);
            let tex = ctx.load_texture(format!("mod-{project_id}"), img, egui::TextureOptions::LINEAR);
            let id = tex.id();
            self.mod_icons.insert(project_id.to_owned(), tex);
            return Some(id);
        }
        if let Some(url) = icon_url.map(str::to_owned).filter(|_| st.icon_requested.insert(project_id.to_owned())) {
            let (state, project, ctx) = (self.mods_state.clone(), project_id.to_owned(), ctx.clone());
            std::thread::spawn(move || {
                let image = net::client()
                    .get(&url)
                    .send()
                    .and_then(|r| r.bytes())
                    .ok()
                    .and_then(|b| image::load_from_memory(&b).ok())
                    .map(|i| skin::to_color_image(&i.resize(64, 64, image::imageops::FilterType::Triangle).to_rgba8()));
                if let Some(img) = image {
                    state.lock().unwrap().icons.insert(project, img);
                    ctx.request_repaint();
                }
            });
        }
        None
    }

    /// Which launcher version runs mods for (loader, game version), if any.
    fn mod_target(&self, loader: &str, game: &str) -> Option<Target> {
        if loader == "fabric" {
            return Some(Target::Fabric(game.to_owned()));
        }
        let wanted = if loader == "neoforge" { "NeoForge" } else { "Forge" };
        self.installed.iter().find_map(|(id, tl)| {
            let tokens: Vec<&str> = id.split(|c: char| !(c.is_ascii_digit() || c == '.')).collect();
            (loader_of(id) == wanted && tokens.contains(&game)).then(|| Target::Installed { id: id.clone(), tl: *tl })
        })
    }

    fn mods_install(&mut self, ctx: &egui::Context, project: String, loader: String, game: String, target: Target, play: bool) {
        let cfg = self.cfg.clone();
        let manifest = self.remote.lock().unwrap().manifest.clone();
        let mods_dir = cfg.game_dir_for(&target).join("mods");
        self.cfg.target = Some(target.clone());
        let (close, hide) = (cfg.close_on_launch && play, cfg.hide_on_launch && play);
        self.spawn_job(ctx, "Установка мода", close, hide, move |task| {
            let added = mods::install(&project, &game, &loader, &mods_dir, task)?;
            task.log(format!("Готово: {} файл(ов) в {}", added.len(), mods_dir.display()));
            if play {
                run_target(&cfg, manifest.as_ref(), &target, true, task)
            } else {
                Ok(None)
            }
        });
    }

    fn mods_page(&mut self, ctx: &egui::Context, ui: &mut Ui, body: Rect) {
        if self.mods_state.lock().unwrap().results.is_empty() && !self.mod_searched_once {
            self.mod_searched_once = true;
            self.mods_search(ctx);
        }
        if self.mod_dirty.is_some_and(|t| t.elapsed() > Duration::from_millis(350)) {
            self.mods_search(ctx);
        }

        // Top bar: search, loader filter, mods folder
        let bar = Rect::from_min_size(body.min, vec2(body.width(), 40.0));
        let p = ui.painter().clone();
        let search = Rect::from_min_size(bar.min, vec2((bar.width() * 0.38).max(280.0), 40.0));
        p.rect(search, 20, SUNKEN, Stroke::new(1.2_f32, BORDER), StrokeKind::Inside);
        text(&p, search.left_center() + vec2(18.0, 0.0), Align2::LEFT_CENTER, "🔍", font(14.0), MUTED);
        let edit = Rect::from_min_max(search.min + vec2(44.0, 2.0), search.max - vec2(16.0, 2.0));
        let resp = ui.put(
            edit,
            egui::TextEdit::singleline(&mut self.mod_query)
                .frame(false)
                .margin(egui::Margin::ZERO)
                .vertical_align(Align::Center)
                .min_size(edit.size())
                .font(font(14.5))
                .hint_text("Найти мод: Sodium, JEI, Create…"),
        );
        if resp.changed() {
            self.mod_dirty = Some(Instant::now());
        }
        let pills = Rect::from_min_max(Pos2::new(search.right() + 14.0, bar.top()), bar.max);
        let mut c = child_ui(ui, pills, "mod_pills", Layout::left_to_right(Align::Center));
        c.spacing_mut().item_spacing.x = 8.0;
        for (label, value) in [("Все", None), ("Fabric", Some("fabric")), ("Forge", Some("forge")), ("NeoForge", Some("neoforge")), ("Horizon", Some("horizon"))] {
            if pill(&mut c, label, self.mod_loader == value).clicked() && self.mod_loader != value {
                self.mod_loader = value;
                if value != Some("horizon") {
                    self.mods_search(ctx);
                }
            }
        }
        let folder_target = self.cfg.target.clone().filter(|t| !t.is_horizon());
        c.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if let Some(t) = &folder_target {
                if pill(ui, "📁 Папка mods", false).on_hover_text(self.cfg.game_dir_for(t).join("mods").display().to_string()).clicked() {
                    open_folder(&self.cfg.game_dir_for(t).join("mods"));
                }
            }
        });

        let rest = Rect::from_min_max(Pos2::new(body.left(), bar.bottom() + GAP), body.max);
        if self.mod_loader == Some("horizon") {
            self.horizon_mods(ctx, ui, rest);
            return;
        }
        let list_w = (rest.width() - GAP) * 0.56;
        let list = Rect::from_min_size(rest.min, vec2(list_w, rest.height()));
        let side = Rect::from_min_max(Pos2::new(list.right() + GAP, rest.top()), rest.max);

        // Search results
        card(&p, list);
        let (searching, error, hits) = {
            let st = self.mods_state.lock().unwrap();
            (st.searching, st.error.clone(), st.results.clone())
        };
        let heading = if self.mod_query.trim().is_empty() { "Популярные моды" } else { "Результаты поиска" };
        text(&p, list.left_top() + vec2(22.0, 20.0), Align2::LEFT_TOP, heading, bold(19.0), TEXT);
        text(&p, list.right_top() + vec2(-22.0, 24.0), Align2::RIGHT_TOP, if searching { "поиск…".to_owned() } else { "каталог Modrinth".to_owned() }, font(12.5), MUTED);
        let area = Rect::from_min_max(list.min + vec2(10.0, 58.0), list.max - vec2(10.0, 12.0));
        let mut lc = child_ui(ui, area, "mod_list", Layout::top_down(Align::Min));
        let mut clicked = None;
        egui::ScrollArea::vertical().id_salt("mod_scroll").auto_shrink([false; 2]).show(&mut lc, |ui| {
            if let Some(e) = &error {
                ui.label(egui::RichText::new(e).color(RED));
            } else if hits.is_empty() && !searching {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| ui.label(egui::RichText::new("Ничего не найдено").color(MUTED)));
            }
            for hit in &hits {
                let width = ui.available_width();
                let (r, resp) = ui.allocate_exact_size(vec2(width, 68.0), Sense::click());
                let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
                let selected = self.mod_selected.as_ref().is_some_and(|s| s.project_id == hit.project_id);
                let rp = ui.painter().clone();
                if selected {
                    rp.rect(r, 14, CARD_HI, Stroke::new(1.0_f32, ACCENT_DIM), StrokeKind::Inside);
                } else if resp.hovered() {
                    rp.rect_filled(r, 14, Color32::from_rgb(29, 24, 35));
                }
                let icon = Rect::from_min_size(r.left_center() + vec2(10.0, -22.0), vec2(44.0, 44.0));
                match self.mod_icon(ctx, hit) {
                    Some(tex) => {
                        rp.image(tex, icon, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                    }
                    None => block_icon(&rp, icon, self.version_icon.id()),
                }
                let tx = icon.right() + 12.0;
                let max_w = r.right() - tx - 100.0;
                text_fit(&rp, Pos2::new(tx, r.top() + 14.0), Align2::LEFT_TOP, &hit.title, bold(15.0), TEXT, max_w);
                text_fit(&rp, Pos2::new(tx, r.top() + 37.0), Align2::LEFT_TOP, &hit.description, font(12.0), MUTED, max_w);
                text(&rp, Pos2::new(r.right() - 14.0, r.top() + 16.0), Align2::RIGHT_TOP, format!("⬇ {}", fmt_count(hit.downloads)), font(12.0), ACCENT);
                let loaders: Vec<&str> = hit.categories.iter().filter_map(|c| mods::LOADERS.iter().find(|l| *l == c).copied()).collect();
                text_fit(&rp, Pos2::new(r.right() - 14.0, r.top() + 38.0), Align2::RIGHT_TOP, &loaders.join(" · "), font(11.5), MUTED, 90.0);
                if resp.clicked() {
                    clicked = Some(hit.clone());
                }
            }
        });
        if let Some(hit) = clicked {
            self.mods_fetch_targets(ctx, &hit.project_id);
            self.mod_selected = Some(hit);
        }

        // Details + installed mods
        card(&p, side);
        let installed_h = (side.height() * 0.36).clamp(150.0, 260.0);
        let detail = Rect::from_min_max(side.min, Pos2::new(side.right(), side.bottom() - installed_h));
        let installed = Rect::from_min_max(Pos2::new(side.left(), detail.bottom()), side.max);
        self.mod_details(ctx, ui, detail);
        p.line_segment([installed.left_top() + vec2(20.0, 0.0), installed.right_top() - vec2(20.0, 0.0)], Stroke::new(1.0_f32, BORDER));
        self.installed_mods(ui, installed);
    }

    fn mod_details(&mut self, ctx: &egui::Context, ui: &mut Ui, rect: Rect) {
        let p = ui.painter().clone();
        let Some(hit) = self.mod_selected.clone() else {
            text(&p, rect.center(), Align2::CENTER_CENTER, "Выбери мод слева —\nздесь будут версии для запуска", font(14.0), MUTED);
            return;
        };
        let icon = Rect::from_min_size(rect.left_top() + vec2(22.0, 22.0), vec2(56.0, 56.0));
        match self.mod_icon(ctx, &hit) {
            Some(tex) => {
                p.image(tex, icon, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            }
            None => block_icon(&p, icon, self.version_icon.id()),
        }
        text_fit(&p, icon.right_top() + vec2(14.0, 4.0), Align2::LEFT_TOP, &hit.title, bold(19.0), TEXT, rect.right() - icon.right() - 36.0);
        text(&p, icon.right_top() + vec2(14.0, 32.0), Align2::LEFT_TOP, format!("⬇ {} скачиваний", fmt_count(hit.downloads)), font(12.5), ACCENT);
        let desc = p.layout(hit.description.clone(), font(12.5), MUTED, rect.width() - 44.0);
        let desc_h = desc.size().y.min(52.0);
        p.with_clip_rect(Rect::from_min_size(Pos2::new(rect.left() + 22.0, icon.bottom() + 12.0), vec2(rect.width() - 44.0, desc_h)))
            .galley(Pos2::new(rect.left() + 22.0, icon.bottom() + 12.0), desc, MUTED);
        let list_top = icon.bottom() + 22.0 + desc_h;
        text(&p, Pos2::new(rect.left() + 22.0, list_top), Align2::LEFT_TOP, "Доступно для запуска", bold(14.0), TEXT);

        let targets = self.mods_state.lock().unwrap().targets.get(&hit.project_id).cloned().flatten();
        let area = Rect::from_min_max(Pos2::new(rect.left() + 12.0, list_top + 26.0), rect.max - vec2(12.0, 10.0));
        let mut c = child_ui(ui, area, "mod_targets", Layout::top_down(Align::Min));
        let running = self.task.running();
        let mut action = None;
        egui::ScrollArea::vertical().id_salt("mod_targets_scroll").auto_shrink([false; 2]).show(&mut c, |ui| {
            match &targets {
                None => {
                    ui.label(egui::RichText::new("Загрузка версий…").color(MUTED));
                }
                Some(Err(e)) => {
                    ui.label(egui::RichText::new(e).color(RED));
                }
                Some(Ok(list)) if list.is_empty() => {
                    ui.label(egui::RichText::new("Нет версий для Fabric, Forge или NeoForge").color(MUTED));
                }
                Some(Ok(list)) => {
                    for (loader, game) in list.iter().take(60) {
                        let target = self.mod_target(loader, game);
                        let width = ui.available_width();
                        let (r, _) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
                        let rp = ui.painter().clone();
                        let name = match loader.as_str() {
                            "fabric" => "Fabric",
                            "neoforge" => "NeoForge",
                            _ => "Forge",
                        };
                        text_fit(&rp, r.left_center() + vec2(10.0, 0.0), Align2::LEFT_CENTER, &format!("{name} {game}"), bold(14.0), TEXT, r.width() - 230.0);
                        match target {
                            Some(t) if !running => {
                                let play_r = Rect::from_min_size(r.right_center() + vec2(-96.0, -14.0), vec2(88.0, 28.0));
                                let inst_r = play_r.translate(vec2(-104.0, 0.0)).expand2(vec2(8.0, 0.0));
                                if small_button(ui, play_r, ui.id().with(("mplay", loader, game)), "▶ Играть", true) {
                                    action = Some((loader.clone(), game.clone(), t.clone(), true));
                                }
                                if small_button(ui, inst_r, ui.id().with(("minst", loader, game)), "⬇ Установить", false) {
                                    action = Some((loader.clone(), game.clone(), t, false));
                                }
                            }
                            Some(_) => {
                                text(&rp, r.right_center() - vec2(10.0, 0.0), Align2::RIGHT_CENTER, "подожди…", font(12.0), MUTED);
                            }
                            None => {
                                text(&rp, r.right_center() - vec2(10.0, 0.0), Align2::RIGHT_CENTER, format!("нужен {name} {game} в TLauncher"), font(12.0), MUTED);
                            }
                        }
                    }
                }
            }
        });
        if let Some((loader, game, target, play)) = action {
            self.mods_install(ctx, hit.project_id.clone(), loader, game, target, play);
        }
    }

    /// Mods in the selected version's folder, with delete buttons.
    fn installed_mods(&mut self, ui: &mut Ui, rect: Rect) {
        let p = ui.painter().clone();
        // Versions that can load mods: installed Fabric/Forge/NeoForge/… plus the selected one.
        let is_modded = |t: &Target| match t {
            Target::Fabric(_) => true,
            Target::Installed { id, .. } => loader_of(id) != "Vanilla",
            _ => false,
        };
        let mut choices: Vec<Target> = self.installed.iter().map(|(id, tl)| Target::Installed { id: id.clone(), tl: *tl }).filter(|t| is_modded(t)).collect();
        if let Some(t) = self.cfg.target.clone().filter(|t| is_modded(t) && !choices.contains(t)) {
            choices.insert(0, t);
        }
        let current = self.cfg.target.clone().filter(|t| is_modded(t)).or_else(|| choices.first().cloned());
        let Some(target) = current else {
            text(&p, rect.center(), Align2::CENTER_CENTER, "Пока нет версий с модами —\nнажми «Установить» у любого мода", font(13.0), MUTED);
            return;
        };
        let dir = self.cfg.game_dir_for(&target).join("mods");

        // Header: version picker + folder for your own mods
        // Row 1: version picker and the folder button on one line; row 2: the caption; then the list.
        let header = Rect::from_min_max(rect.min + vec2(18.0, 12.0), Pos2::new(rect.right() - 18.0, rect.top() + 50.0));
        let mut h = child_ui(ui, header, "mods_header", Layout::left_to_right(Align::Center));
        egui::ComboBox::from_id_salt("mods_version").selected_text(target.title()).width((header.width() - 190.0).max(120.0)).show_ui(&mut h, |ui| {
            for choice in &choices {
                if ui.selectable_label(*choice == target, choice.title()).clicked() {
                    self.cfg.target = Some(choice.clone());
                }
            }
        });
        h.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if pill(ui, "📁 Папка mods", false).on_hover_text(format!("{}\nСюда можно класть свои моды (.jar)", dir.display())).clicked() {
                open_folder(&dir);
            }
        });
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "jar"))
            .collect();
        files.sort();
        let p = ui.painter().clone();
        text_fit(&p, Pos2::new(rect.left() + 22.0, header.bottom() + 10.0), Align2::LEFT_TOP, &format!("Моды в «{}»: {}", target.title(), files.len()), font(12.5), MUTED, rect.width() - 44.0);
        let area = Rect::from_min_max(Pos2::new(rect.left() + 12.0, header.bottom() + 34.0), rect.max - vec2(12.0, 10.0));
        let mut c = child_ui(ui, area, "installed_mods", Layout::top_down(Align::Min));
        let mut remove = None;
        egui::ScrollArea::vertical().id_salt("installed_mods_scroll").auto_shrink([false; 2]).show(&mut c, |ui| {
            if files.is_empty() {
                ui.label(egui::RichText::new("Пока пусто — установи мод из списка").color(MUTED).size(12.5));
            }
            for path in &files {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let width = ui.available_width();
                let (r, _) = ui.allocate_exact_size(vec2(width, 30.0), Sense::hover());
                let rp = ui.painter().clone();
                text_fit(&rp, r.left_center() + vec2(10.0, 0.0), Align2::LEFT_CENTER, &name, font(12.5), TEXT, r.width() - 60.0);
                let del = Rect::from_center_size(r.right_center() - vec2(20.0, 0.0), vec2(28.0, 24.0));
                if small_button(ui, del, ui.id().with(("mdel", &name)), "✖", false) {
                    remove = Some(path.clone());
                }
            }
        });
        if let Some(path) = remove {
            let _ = std::fs::remove_file(path);
        }
    }

    // ---------------------------------------------------------------- Horizon add-ons

    /// The Horizon tab of the mods page: Zyntra's own add-ons first, then Horizon's registry.
    fn horizon_mods(&mut self, ctx: &egui::Context, ui: &mut Ui, rest: Rect) {
        {
            let mut st = self.mods_state.lock().unwrap();
            if st.hz_index.is_none() && !st.hz_loading {
                st.hz_loading = true;
                let (state, ctx) = (self.mods_state.clone(), ctx.clone());
                std::thread::spawn(move || {
                    let result = hz_mods::fetch_index().map_err(|e| format!("Каталог Horizon недоступен: {e:#}"));
                    let mut st = state.lock().unwrap();
                    st.hz_index = Some(result);
                    st.hz_loading = false;
                    ctx.request_repaint();
                });
            }
        }
        let game = horizon::game_folder(&self.cfg.game_dir);
        let installed = hz_mods::installed(&game);
        let projects = match &self.mods_state.lock().unwrap().hz_index {
            Some(Ok(list)) => list.clone(),
            _ => Vec::new(),
        };
        let index_error = match &self.mods_state.lock().unwrap().hz_index {
            Some(Err(e)) => Some(e.clone()),
            _ => None,
        };

        let p = ui.painter().clone();
        let list_w = (rest.width() - GAP) * 0.56;
        let list = Rect::from_min_size(rest.min, vec2(list_w, rest.height()));
        let side = Rect::from_min_max(Pos2::new(list.right() + GAP, rest.top()), rest.max);

        // Entries: (id, title, subtitle, right label, icon url)
        let native_selected = native::selected(&self.cfg.game_dir);
        let query = self.mod_query.trim().to_lowercase();
        let mut entries: Vec<(String, String, String, String, Option<String>)> = vec![
            (NATIVE_BUILD_ID.into(), "Сборка Horizon".into(), "Нативные моды: Freecam, Minimap и другие".into(), "Zyntra".into(), None),
            
            (hz_mods::CRYSTALS_ID.into(), "Zyntra Crystals".into(), "Кристальный блок, осколки и кристальный меч".into(), "Zyntra".into(), None),
            (hz_mods::XRAY_ID.into(), "Zyntra Xray".into(), "Ресурс-пак: руды видно сквозь камень".into(), "Zyntra".into(), None),
        ];
        for pr in &projects {
            let in_build = native_selected.iter().any(|f| f.starts_with(&format!("{}-", pr.id)));
            let right = if in_build { "✔ в сборке".into() } else if pr.installable() { format!("⬇ {}", fmt_count(pr.downloads)) } else { "нативный".into() };
            entries.push((pr.id.clone(), pr.name.clone(), format!("{} · {}", pr.kind_ru(), pr.summary), right, pr.icon.clone()));
        }
        entries.retain(|e| query.is_empty() || e.1.to_lowercase().contains(&query) || e.2.to_lowercase().contains(&query));
        let selected_id = self.mod_hz_selected.clone().unwrap_or_else(|| NATIVE_BUILD_ID.into());

        card(&p, list);
        text(&p, list.left_top() + vec2(22.0, 20.0), Align2::LEFT_TOP, "Моды для Horizon", bold(19.0), TEXT);
        let loading = self.mods_state.lock().unwrap().hz_loading;
        text(&p, list.right_top() + vec2(-22.0, 24.0), Align2::RIGHT_TOP, if loading { "загрузка каталога…" } else { "Zyntra + каталог Horizon" }, font(12.5), MUTED);
        let area = Rect::from_min_max(list.min + vec2(10.0, 58.0), list.max - vec2(10.0, 12.0));
        let mut lc = child_ui(ui, area, "hz_list", Layout::top_down(Align::Min));
        let mut clicked = None;
        egui::ScrollArea::vertical().id_salt("hz_scroll").auto_shrink([false; 2]).show(&mut lc, |ui| {
            if let Some(e) = &index_error {
                ui.label(egui::RichText::new(e).color(RED).size(12.5));
            }
            for (id, title, sub, right, icon_url) in &entries {
                let width = ui.available_width();
                let (r, resp) = ui.allocate_exact_size(vec2(width, 64.0), Sense::click());
                let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
                let rp = ui.painter().clone();
                if *id == selected_id {
                    rp.rect(r, 14, CARD_HI, Stroke::new(1.0_f32, ACCENT_DIM), StrokeKind::Inside);
                } else if resp.hovered() {
                    rp.rect_filled(r, 14, Color32::from_rgb(29, 24, 35));
                }
                let icon = Rect::from_min_size(r.left_center() + vec2(10.0, -21.0), vec2(42.0, 42.0));
                let full = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                if id == hz_mods::OPTIMIZER_ID {
                    rp.image(self.logo.id(), icon, full, Color32::WHITE);
                } else if let Some(tex) = self.icon_tex(ctx, id, icon_url.as_deref()) {
                    rp.image(tex, icon, full, Color32::WHITE);
                } else {
                    block_icon(&rp, icon, self.version_icon.id());
                }
                let tx = icon.right() + 12.0;
                let max_w = r.right() - tx - 110.0;
                text_fit(&rp, Pos2::new(tx, r.top() + 13.0), Align2::LEFT_TOP, title, bold(15.0), TEXT, max_w);
                text_fit(&rp, Pos2::new(tx, r.top() + 36.0), Align2::LEFT_TOP, sub, font(12.0), MUTED, max_w);
                let color = if right == "Zyntra" { ACCENT } else if right == "нужна сборка" { MUTED } else { ACCENT };
                text(&rp, Pos2::new(r.right() - 14.0, r.center().y), Align2::RIGHT_CENTER, right, font(12.0), color);
                if resp.clicked() {
                    clicked = Some(id.clone());
                }
            }
        });
        if let Some(id) = clicked {
            self.mod_hz_selected = Some(id);
        }

        // Details + installed add-ons
        card(&p, side);
        let project = projects.iter().find(|pr| pr.id == selected_id).cloned();
        // Zyntra Fine is a long settings menu: it gets the whole column.
        if selected_id == hz_mods::OPTIMIZER_ID {
            self.horizon_detail(ctx, ui, side, &selected_id, project, &game, &installed);
            return;
        }
        let installed_h = (side.height() * 0.38).clamp(160.0, 280.0);
        let detail = Rect::from_min_max(side.min, Pos2::new(side.right(), side.bottom() - installed_h));
        let bottom = Rect::from_min_max(Pos2::new(side.left(), detail.bottom()), side.max);
        self.horizon_detail(ctx, ui, detail, &selected_id, project, &game, &installed);
        p.line_segment([bottom.left_top() + vec2(20.0, 0.0), bottom.right_top() - vec2(20.0, 0.0)], Stroke::new(1.0_f32, BORDER));
        self.horizon_installed(ui, bottom, &game, installed);
    }

    fn horizon_detail(
        &mut self,
        ctx: &egui::Context,
        ui: &mut Ui,
        rect: Rect,
        id: &str,
        project: Option<hz_mods::Project>,
        game: &Path,
        installed: &[hz_mods::Installed],
    ) {
        let inner = Rect::from_min_max(rect.min + vec2(22.0, 20.0), rect.max - vec2(22.0, 12.0));
        let mut c = child_ui(ui, inner, "hz_detail", Layout::top_down(Align::Min));
        c.set_clip_rect(rect);
        c.spacing_mut().item_spacing.y = 8.0;
        let running = self.task.running();

        if id == hz_mods::OPTIMIZER_ID {
            c.label(egui::RichText::new("Zyntra Fine — OptiFine для Horizon").font(bold(20.0)).color(TEXT));
            c.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(egui::RichText::new("Пресет:").color(MUTED));
                let current = self.cfg.fine.clone();
                for (label, preset) in [("Слабый ПК", Fine::fast()), ("Баланс", Fine::balanced()), ("Качество", Fine::quality())] {
                    // A preset changes the video settings, not the shader pack or the zoom.
                    let keep = |p: Fine| Fine { shader_pack: current.shader_pack.clone(), zoom: current.zoom, zoom_key: current.zoom_key.clone(), ..p };
                    let same = keep(preset.clone()) == current;
                    if pill(ui, label, same).clicked() {
                        self.cfg.fine = keep(preset);
                    }
                }
            });
            let shader_packs = horizon::shader_packs(&self.cfg.game_dir);
            let fine = &mut self.cfg.fine;
            let extras = &mut self.cfg.hz_extras;
            let bright = &mut self.cfg.max_gamma;
            egui::ScrollArea::vertical().id_salt("hz_fine_scroll").auto_shrink([false; 2]).show(&mut c, |ui| {
                ui.label(egui::RichText::new("Применяется в версии «Horizon OptiFine». «Horizon 26.3» запускается с обычными настройками игры.").color(ACCENT).size(12.5));
                let section = |ui: &mut Ui, title: &str| {
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(title).font(bold(14.5)).color(ACCENT));
                };
                let choice = |ui: &mut Ui, label: &str, value: &mut u32, options: &[(&str, u32)]| {
                    ui.label(egui::RichText::new(label).size(13.0));
                    ui.horizontal_wrapped(|ui| {
                        for (name, v) in options {
                            if ui.selectable_label(*value == *v, *name).clicked() {
                                *value = *v;
                            }
                        }
                    });
                };

                section(ui, "Производительность");
                labeled_slider(ui, "Дальность прорисовки (чанки)", egui::Slider::new(&mut fine.render_distance, 2..=32));
                labeled_slider(ui, "Дальность симуляции (чанки)", egui::Slider::new(&mut fine.simulation_distance, 5..=16));
                let fps_label = if fine.max_fps >= 260 { "Лимит FPS: без лимита" } else { "Лимит FPS" };
                labeled_slider(ui, fps_label, egui::Slider::new(&mut fine.max_fps, 30..=260).step_by(10.0));
                ui.checkbox(&mut fine.vsync, "Вертикальная синхронизация (VSync)");
                choice(ui, "Подгрузка чанков:", &mut fine.chunk_updates, &[("все сразу", 0), ("плавно", 8), ("очень плавно", 4), ("минимум", 2)]);

                section(ui, "Качество");
                ui.checkbox(&mut fine.smooth_lighting, "Мягкое освещение");
                labeled_slider(ui, "Мипмапы (сглаживание текстур вдали)", egui::Slider::new(&mut fine.mipmaps, 0..=4));
                ui.checkbox(&mut fine.fxaa, "Сглаживание (FXAA)");
                labeled_slider(ui, "Масштаб рендера (меньше — быстрее)", egui::Slider::new(&mut fine.resolution_scale, 50..=100).step_by(5.0).suffix("%"));

                section(ui, "Детали");
                choice(ui, "Облака:", &mut fine.clouds, &[("выкл", 0), ("быстрые", 1), ("красивые", 2)]);
                ui.checkbox(&mut fine.sky, "Небо");
                ui.checkbox(&mut fine.fog, "Туман");
                ui.checkbox(&mut fine.weather, "Дождь и снег");
                ui.checkbox(&mut fine.entity_shadows, "Тени сущностей");
                ui.checkbox(&mut fine.fancy_leaves, "Красивая листва");
                labeled_slider(ui, "Смешивание цветов биомов", egui::Slider::new(&mut fine.biome_blend, 0..=7));

                section(ui, "Анимации");
                ui.checkbox(&mut fine.animated_textures, "Анимированные текстуры (вода, лава, огонь)");
                choice(ui, "Частицы:", &mut fine.particles, &[("все", 0), ("меньше", 1), ("минимум", 2)]);

                section(ui, "Отсечение невидимого");
                ui.checkbox(&mut fine.entity_culling, "Не рисовать скрытых мобов");
                ui.checkbox(&mut fine.cave_culling, "Не рисовать скрытые пещеры");
                ui.checkbox(&mut fine.leaf_culling, "Упрощать кроны деревьев");

                section(ui, "Освещение и удобства");
                ui.checkbox(bright, "Яркость на максимум (гамма 100%) — во всех версиях");
                for (key, title, desc, default) in hz_mods::EXTRAS {
                    let mut on = extras.get(key).copied().unwrap_or(default);
                    if ui.checkbox(&mut on, title).on_hover_text(desc).changed() {
                        extras.insert(key.to_owned(), on);
                    }
                }

                section(ui, "Шейдеры");
                egui::ComboBox::from_id_salt("fine_shader")
                    .selected_text(if fine.shader_pack.is_empty() { "Выкл" } else { fine.shader_pack.as_str() })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut fine.shader_pack, String::new(), "Выкл");
                        for pack in &shader_packs {
                            ui.selectable_value(&mut fine.shader_pack, pack.clone(), pack);
                        }
                    });
                ui.label(egui::RichText::new("Подходят паки OptiFine/Iris (.zip, например BSL) — Horizon переводит их сам — и паки Horizon (.hshader). Свои кладутся в «📁 Шейдеры».").color(MUTED).size(12.0));
                ui.label(egui::RichText::new("Скачать шейдеры (BSL, Complementary и другие) — раздел «Шейдеры» в меню.").color(ACCENT).size(12.0));

                section(ui, "Ресурс-паки OptiFine");
                ui.label(egui::RichText::new("Соединённые текстуры, свои текстуры предметов, случайные и светящиеся мобы, своё небо — Horizon читает их сам. Положи пак в «📁 Паки».").color(MUTED).size(12.0));
                section(ui, "Зум");
                ui.checkbox(&mut fine.zoom, "Зум, как в OptiFine (зажми клавишу в игре)");
                ui.horizontal_wrapped(|ui| {
                    ui.label("Клавиша:");
                    for (label, _) in crate::zoom::KEYS {
                        if ui.selectable_label(fine.zoom_key == label, label).clicked() {
                            fine.zoom_key = label.to_owned();
                        }
                    }
                });
                ui.label(egui::RichText::new("Пока клавиша зажата, обзор сужается до 30°, отпустил — вернулся твой. Работает без пересборки Horizon.").color(MUTED).size(12.0));
            });
            return;
        }

        if id == NATIVE_BUILD_ID {
            let selected = native::selected(&self.cfg.game_dir);
            let ready = native::ready(&self.cfg.game_dir).is_some();
            c.label(egui::RichText::new("Сборка Horizon с нативными модами").font(bold(20.0)).color(TEXT));
            c.label(
                egui::RichText::new(
                    "Как в официальном лаунчере Horizon: моды применяются к исходникам Horizon (horizon-mixins), и игра \
                     компилируется заново. Собранную версию запускает «Horizon OptiFine»; «Horizon 26.3» остаётся обычным.",
                )
                .color(MUTED)
                .size(12.5),
            );
            c.add_space(4.0);
            c.label(egui::RichText::new(format!("Моды в сборке: {}", selected.len())).strong());
            let mut remove = None;
            for file in &selected {
                c.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("• {file}")).size(13.0));
                    if ui.small_button("✖").on_hover_text("Убрать из сборки").clicked() {
                        remove = Some(file.clone());
                    }
                });
            }
            if let Some(file) = remove {
                native::remove_mod(&self.cfg.game_dir, &file);
            }
            if selected.iter().any(|f| f.starts_with("minimap-")) {
                c.add_space(4.0);
                let current = native::minimap_corner(&self.cfg.game_dir);
                c.horizontal_wrapped(|ui| {
                    ui.label("Мини-карта:");
                    for (value, label) in [("top_left", "слева сверху"), ("top_right", "справа сверху"), ("bottom_left", "слева снизу"), ("bottom_right", "справа снизу")] {
                        if ui.selectable_label(current == value, label).clicked() {
                            native::set_minimap_corner(&self.cfg.game_dir, value);
                        }
                    }
                });
            }
            if selected.is_empty() {
                c.label(egui::RichText::new("Выбери нативные моды в списке слева (Freecam, Minimap…) и нажми «Добавить в сборку».").color(MUTED).size(12.5));
            }
            c.add_space(6.0);
            let (status, color) = if ready { ("✔ Собрано — «Horizon OptiFine» запустит версию с этими модами", GREEN) } else if selected.is_empty() { ("Нечего собирать", MUTED) } else { ("Нужно собрать", ACCENT) };
            c.label(egui::RichText::new(status).color(color));
            let label = if ready { "🔨 Пересобрать" } else { "🔨 Собрать Horizon" };
            if primary(&mut c, label, vec2(inner.width().min(280.0), 44.0), !running && !selected.is_empty()).clicked() {
                let game_dir = self.cfg.game_dir.clone();
                self.spawn_job(ctx, "Сборка Horizon", false, false, move |task| {
                    native::build(&game_dir, task)?;
                    Ok(None)
                });
            }
            c.label(egui::RichText::new("Первая сборка: 30–60 минут и ~5–10 ГБ на диске (компилируется вся игра, ход — внизу на главной и в «Консоли»). Следующие — быстрее. После обновления Horizon нужно собрать заново.").color(MUTED).size(11.5));
            return;
        }

        if id == hz_mods::XRAY_ID {
            let is_installed = installed.iter().any(|i| i.id == hz_mods::XRAY_FILE);
            c.label(egui::RichText::new("Zyntra Xray").font(bold(20.0)).color(TEXT));
            c.label(
                egui::RichText::new(
                    "Наш ресурс-пак: камень, сланец, земля, песок, незерак и прочая порода становятся невидимыми, \
                     а руды (уголь, железо, медь, золото, редстоун, лазурит, изумруды, алмазы, кварц, древние обломки) \
                     видно сквозь них. Переписывать его под Horizon не нужно — Horizon читает обычные ресурс-паки.",
                )
                .color(MUTED)
                .size(12.5),
            );
            c.add_space(4.0);
            let fullbright = self.cfg.hz_extras.get("fullbright").copied().unwrap_or(false);
            let mut fb = fullbright;
            c.checkbox(&mut fb, "Включить полную яркость (под землёй иначе темно)");
            if fb != fullbright {
                self.cfg.hz_extras.insert("fullbright".into(), fb);
            }
            c.add_space(4.0);
            let label = if is_installed { "Переустановить" } else { "⬇ Установить и включить" };
            if primary(&mut c, label, vec2(inner.width().min(280.0), 44.0), !running).clicked() {
                // Cave culling would hide ore sections enclosed by (now invisible) rock.
                self.cfg.fine.cave_culling = false;
                let game = game.to_path_buf();
                self.spawn_job(ctx, "Установка Zyntra Xray", false, false, move |task| {
                    hz_mods::install_xray(&game)?;
                    task.log("Zyntra Xray установлен и включён в Horizon");
                    Ok(None)
                });
            }
            c.label(
                egui::RichText::new("Работает в «Horizon OptiFine» (там же выключается отсечение пещер, иначе руды в толще камня пропадают). Выключить пак — ✖ в списке «Установлено» или экран «Ресурс-паки» в игре. На серверах Xray обычно запрещён.")
                    .color(MUTED)
                    .size(12.0),
            );
            if is_installed {
                c.label(egui::RichText::new("✔ установлен").color(GREEN));
            }
            return;
        }

        if id == hz_mods::CRYSTALS_ID {
            let is_installed = installed.iter().any(|i| i.id == hz_mods::CRYSTALS_ID);
            c.label(egui::RichText::new("Zyntra Crystals").font(bold(20.0)).color(TEXT));
            c.label(egui::RichText::new("Наш контентный мод: светящийся кристальный блок, осколки и кристальный меч (урон 9, прочность 2400). Своя вкладка в творческом режиме, рецепты из аметиста и светопыли.").color(MUTED).size(12.5));
            c.add_space(6.0);
            let label = if is_installed { "Переустановить" } else { "⬇ Установить" };
            if primary(&mut c, label, vec2(inner.width().min(260.0), 44.0), !running).clicked() {
                let game = game.to_path_buf();
                self.spawn_job(ctx, "Установка Zyntra Crystals", false, false, move |task| {
                    hz_mods::install_crystals(&game)?;
                    task.log("Zyntra Crystals установлен и включён");
                    Ok(None)
                });
            }
            if is_installed {
                c.label(egui::RichText::new("✔ установлен — включается и выключается ниже").color(GREEN).size(12.5));
            }
            return;
        }

        let Some(pr) = project else {
            c.label(egui::RichText::new("Выбери дополнение слева").color(MUTED));
            return;
        };
        let version = &pr.versions[0];
        c.horizontal(|ui| {
            let (icon_rect, _) = ui.allocate_exact_size(vec2(52.0, 52.0), Sense::hover());
            match self.icon_tex(ctx, &pr.id, pr.icon.as_deref()) {
                Some(tex) => {
                    ui.painter().image(tex, icon_rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                }
                None => block_icon(ui.painter(), icon_rect, self.version_icon.id()),
            }
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(&pr.name).font(bold(19.0)).color(TEXT));
                ui.label(egui::RichText::new(format!("{} · v{} · {:.1} МБ", pr.kind_ru(), version.version, version.file.size as f64 / 1_048_576.0)).color(ACCENT).size(12.5));
            });
        });
        c.label(egui::RichText::new(&pr.summary).color(MUTED).size(12.5));
        c.add_space(6.0);
        let file_there = installed.iter().any(|i| i.id == pr.id || i.path.file_name().is_some_and(|n| n.to_string_lossy() == version.file.name));
        if pr.kind == "native-mod" {
            let selected = native::selected(&self.cfg.game_dir);
            let chosen = selected.iter().find(|f| f.starts_with(&format!("{}-", pr.id))).cloned();
            c.label(egui::RichText::new("Нативный мод: код на Rust, который встраивается в Horizon при сборке игры. Добавь его в сборку и собери в пункте «Сборка Horizon» — затем он будет в «Horizon OptiFine».").color(MUTED).size(12.5));
            c.label(egui::RichText::new("Нативные моды не изолированы: это обычный код с правами твоего пользователя. Ставь только те, которым доверяешь (каталог Horizon проверяет их при добавлении).").color(MUTED).size(11.5));
            match chosen {
                Some(file) => {
                    c.label(egui::RichText::new("✔ в сборке").color(GREEN));
                    if pill(&mut c, "Убрать из сборки", false).clicked() {
                        native::remove_mod(&self.cfg.game_dir, &file);
                    }
                }
                None => {
                    if primary(&mut c, "⬇ Добавить в сборку", vec2(inner.width().min(260.0), 44.0), !running).clicked() {
                        let game_dir = self.cfg.game_dir.clone();
                        self.spawn_job(ctx, "Нативный мод", false, false, move |task| {
                            native::add_mod(&game_dir, &pr, task)?;
                            task.log(format!("{} добавлен в сборку — собери Horizon в пункте «Сборка Horizon»", pr.name));
                            Ok(None)
                        });
                    }
                }
            }
        } else if !pr.installable() {
            c.label(egui::RichText::new("Этот тип дополнения Zyntra пока не ставит.").color(MUTED).size(12.5));
        } else if file_there {
            c.label(egui::RichText::new("✔ установлено").color(GREEN));
        } else if primary(&mut c, "⬇ Установить", vec2(inner.width().min(260.0), 44.0), !running).clicked() {
            let game = game.to_path_buf();
            self.spawn_job(ctx, "Установка дополнения Horizon", false, false, move |task| {
                hz_mods::install(&pr, &game, task)?;
                task.log(format!("{} установлен", pr.name));
                Ok(None)
            });
        }
    }

    fn horizon_installed(&mut self, ui: &mut Ui, rect: Rect, game: &Path, installed: Vec<hz_mods::Installed>) {
        // Row 1: folder buttons; row 2: the caption; then the list.
        let header = Rect::from_min_max(rect.min + vec2(18.0, 12.0), Pos2::new(rect.right() - 18.0, rect.top() + 50.0));
        let mut h = child_ui(ui, header, "hz_installed_header", Layout::left_to_right(Align::Center));
        h.spacing_mut().item_spacing.x = 6.0;
        for (label, folder) in [("📁 Моды", "horizon-mods"), ("📁 Паки", "resourcepacks"), ("📁 Шейдеры", "shaderpacks")] {
            if pill(&mut h, label, false).on_hover_text(game.join(folder).display().to_string()).clicked() {
                open_folder(&game.join(folder));
            }
        }
        let p = ui.painter().clone();
        text(&p, Pos2::new(rect.left() + 22.0, header.bottom() + 10.0), Align2::LEFT_TOP, format!("Установлено: {}", installed.len()), bold(14.0), TEXT);
        let area = Rect::from_min_max(Pos2::new(rect.left() + 12.0, header.bottom() + 36.0), rect.max - vec2(12.0, 10.0));
        let mut c = child_ui(ui, area, "hz_installed", Layout::top_down(Align::Min));
        let (mut toggle, mut remove) = (None, None);
        egui::ScrollArea::vertical().id_salt("hz_installed_scroll").auto_shrink([false; 2]).show(&mut c, |ui| {
            if installed.is_empty() {
                ui.label(egui::RichText::new("Пока пусто").color(MUTED).size(12.5));
            }
            for (i, item) in installed.iter().enumerate() {
                let width = ui.available_width();
                let (r, _) = ui.allocate_exact_size(vec2(width, 30.0), Sense::hover());
                let rp = ui.painter().clone();
                text(&rp, r.left_center() + vec2(8.0, 0.0), Align2::LEFT_CENTER, item.kind, font(11.5), MUTED);
                text_fit(&rp, r.left_center() + vec2(84.0, 0.0), Align2::LEFT_CENTER, &item.name, font(13.0), TEXT, r.width() - 190.0);
                if let Some(on) = item.enabled {
                    let tr = Rect::from_center_size(r.right_center() - vec2(80.0, 0.0), vec2(64.0, 24.0));
                    if small_button(ui, tr, ui.id().with(("hztog", i)), if on { "вкл" } else { "выкл" }, on) {
                        toggle = Some((item.id.clone(), !on));
                    }
                }
                let del = Rect::from_center_size(r.right_center() - vec2(20.0, 0.0), vec2(28.0, 24.0));
                if small_button(ui, del, ui.id().with(("hzdel", i)), "✖", false) {
                    remove = Some(i);
                }
            }
        });
        if let Some((id, on)) = toggle {
            let _ = hz_mods::set_enabled(game, &id, on);
        }
        if let Some(i) = remove {
            let _ = hz_mods::remove(&installed[i]);
        }
    }

    // ---------------------------------------------------------------- shaders

    fn shaders_search(&mut self, ctx: &egui::Context) {
        self.shader_dirty = None;
        let query = self.shader_query.clone();
        let state = self.shaders_state.clone();
        state.lock().unwrap().searching = true;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = mods::search_shaders(&query);
            let mut st = state.lock().unwrap();
            st.searching = false;
            match result {
                Ok(hits) => {
                    st.results = hits;
                    st.error = None;
                }
                Err(e) => st.error = Some(format!("Каталог шейдеров недоступен: {e:#}")),
            }
            ctx.request_repaint();
        });
    }

    fn shader_versions(&self, ctx: &egui::Context, project: &str) -> Option<Result<Vec<mods::Version>, String>> {
        let mut st = self.shaders_state.lock().unwrap();
        if let Some(entry) = st.versions.get(project) {
            return entry.clone();
        }
        st.versions.insert(project.to_owned(), None);
        let (state, project, ctx) = (self.shaders_state.clone(), project.to_owned(), ctx.clone());
        std::thread::spawn(move || {
            let result = mods::versions(&project).map_err(|e| format!("{e:#}"));
            state.lock().unwrap().versions.insert(project, Some(result));
            ctx.request_repaint();
        });
        None
    }

    /// Where shader packs go: Horizon's shaderpacks/ (picked for «Horizon OptiFine»), or the
    /// selected Java version's game folder, where Iris or OptiFine reads them.
    fn shader_dest(&self) -> Result<PathBuf, &'static str> {
        if self.shader_for_horizon {
            return Ok(horizon::game_folder(&self.cfg.game_dir).join("shaderpacks"));
        }
        match &self.cfg.target {
            Some(t) if !t.is_horizon() => Ok(self.cfg.game_dir_for(t).join("shaderpacks")),
            _ => Err("Выбери Java-версию на главной или в «Версиях»"),
        }
    }

    fn shaders_page(&mut self, ctx: &egui::Context, ui: &mut Ui, body: Rect) {
        if self.shaders_state.lock().unwrap().results.is_empty() && !self.shader_searched_once {
            self.shader_searched_once = true;
            self.shaders_search(ctx);
        }
        if self.shader_dirty.is_some_and(|t| t.elapsed() > Duration::from_millis(350)) {
            self.shaders_search(ctx);
        }

        // Top bar: search and where to install
        let bar = Rect::from_min_size(body.min, vec2(body.width(), 40.0));
        let p = ui.painter().clone();
        let search = Rect::from_min_size(bar.min, vec2((bar.width() * 0.38).max(280.0), 40.0));
        p.rect(search, 20, SUNKEN, Stroke::new(1.2_f32, BORDER), StrokeKind::Inside);
        text(&p, search.left_center() + vec2(18.0, 0.0), Align2::LEFT_CENTER, "🔍", font(14.0), MUTED);
        let edit = Rect::from_min_max(search.min + vec2(44.0, 2.0), search.max - vec2(16.0, 2.0));
        let resp = ui.put(
            edit,
            egui::TextEdit::singleline(&mut self.shader_query)
                .frame(false)
                .margin(egui::Margin::ZERO)
                .vertical_align(Align::Center)
                .min_size(edit.size())
                .font(font(14.5))
                .hint_text("Найти шейдеры: BSL, Complementary…"),
        );
        if resp.changed() {
            self.shader_dirty = Some(Instant::now());
        }
        let pills = Rect::from_min_max(Pos2::new(search.right() + 14.0, bar.top()), bar.max);
        let mut c = child_ui(ui, pills, "shader_dest", Layout::left_to_right(Align::Center));
        c.spacing_mut().item_spacing.x = 8.0;
        c.label(egui::RichText::new("Ставить для:").color(MUTED));
        if pill(&mut c, "Horizon OptiFine", self.shader_for_horizon).clicked() {
            self.shader_for_horizon = true;
        }
        let java_label = match &self.cfg.target {
            Some(t) if !t.is_horizon() => t.title(),
            _ => "Java-версии".into(),
        };
        if pill(&mut c, &java_label, !self.shader_for_horizon).clicked() {
            self.shader_for_horizon = false;
        }

        let rest = Rect::from_min_max(Pos2::new(body.left(), bar.bottom() + GAP), body.max);
        let list_w = (rest.width() - GAP) * 0.56;
        let list = Rect::from_min_size(rest.min, vec2(list_w, rest.height()));
        let side = Rect::from_min_max(Pos2::new(list.right() + GAP, rest.top()), rest.max);

        // Catalog
        card(&p, list);
        let (searching, error, hits) = {
            let st = self.shaders_state.lock().unwrap();
            (st.searching, st.error.clone(), st.results.clone())
        };
        let heading = if self.shader_query.trim().is_empty() { "Популярные шейдеры" } else { "Результаты поиска" };
        text(&p, list.left_top() + vec2(22.0, 20.0), Align2::LEFT_TOP, heading, bold(19.0), TEXT);
        text(&p, list.right_top() + vec2(-22.0, 24.0), Align2::RIGHT_TOP, if searching { "поиск…" } else { "каталог Modrinth" }, font(12.5), MUTED);
        let area = Rect::from_min_max(list.min + vec2(10.0, 58.0), list.max - vec2(10.0, 12.0));
        let mut lc = child_ui(ui, area, "shader_list", Layout::top_down(Align::Min));
        let mut clicked = None;
        egui::ScrollArea::vertical().id_salt("shader_scroll").auto_shrink([false; 2]).show(&mut lc, |ui| {
            if let Some(e) = &error {
                ui.label(egui::RichText::new(e).color(RED));
            } else if hits.is_empty() && !searching {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| ui.label(egui::RichText::new("Ничего не найдено").color(MUTED)));
            }
            for hit in &hits {
                let width = ui.available_width();
                let (r, resp) = ui.allocate_exact_size(vec2(width, 68.0), Sense::click());
                let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
                let selected = self.shader_selected.as_ref().is_some_and(|s| s.project_id == hit.project_id);
                let rp = ui.painter().clone();
                if selected {
                    rp.rect(r, 14, CARD_HI, Stroke::new(1.0_f32, ACCENT_DIM), StrokeKind::Inside);
                } else if resp.hovered() {
                    rp.rect_filled(r, 14, Color32::from_rgb(29, 24, 35));
                }
                let icon = Rect::from_min_size(r.left_center() + vec2(10.0, -22.0), vec2(44.0, 44.0));
                match self.mod_icon(ctx, hit) {
                    Some(tex) => {
                        rp.image(tex, icon, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                    }
                    None => block_icon(&rp, icon, self.version_icon.id()),
                }
                let tx = icon.right() + 12.0;
                let max_w = r.right() - tx - 100.0;
                text_fit(&rp, Pos2::new(tx, r.top() + 14.0), Align2::LEFT_TOP, &hit.title, bold(15.0), TEXT, max_w);
                text_fit(&rp, Pos2::new(tx, r.top() + 37.0), Align2::LEFT_TOP, &hit.description, font(12.0), MUTED, max_w);
                text(&rp, Pos2::new(r.right() - 14.0, r.center().y), Align2::RIGHT_CENTER, format!("⬇ {}", fmt_count(hit.downloads)), font(12.0), ACCENT);
                if resp.clicked() {
                    clicked = Some(hit.clone());
                }
            }
        });
        if let Some(hit) = clicked {
            self.shader_selected = Some(hit);
        }

        // Details + installed
        card(&p, side);
        let installed_h = (side.height() * 0.4).clamp(170.0, 300.0);
        let detail = Rect::from_min_max(side.min, Pos2::new(side.right(), side.bottom() - installed_h));
        let bottom = Rect::from_min_max(Pos2::new(side.left(), detail.bottom()), side.max);
        self.shader_details(ctx, ui, detail);
        p.line_segment([bottom.left_top() + vec2(20.0, 0.0), bottom.right_top() - vec2(20.0, 0.0)], Stroke::new(1.0_f32, BORDER));
        self.installed_shaders(ui, bottom);
    }

    fn shader_details(&mut self, ctx: &egui::Context, ui: &mut Ui, rect: Rect) {
        let p = ui.painter().clone();
        let Some(hit) = self.shader_selected.clone() else {
            text(&p, rect.center(), Align2::CENTER_CENTER, "Выбери шейдер слева —\nздесь будут его версии", font(14.0), MUTED);
            return;
        };
        let icon = Rect::from_min_size(rect.left_top() + vec2(22.0, 20.0), vec2(52.0, 52.0));
        match self.mod_icon(ctx, &hit) {
            Some(tex) => {
                p.image(tex, icon, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            }
            None => block_icon(&p, icon, self.version_icon.id()),
        }
        text_fit(&p, icon.right_top() + vec2(14.0, 2.0), Align2::LEFT_TOP, &hit.title, bold(19.0), TEXT, rect.right() - icon.right() - 36.0);
        text(&p, icon.right_top() + vec2(14.0, 30.0), Align2::LEFT_TOP, format!("⬇ {} скачиваний", fmt_count(hit.downloads)), font(12.5), ACCENT);
        let desc = p.layout(hit.description.clone(), font(12.5), MUTED, rect.width() - 44.0);
        let desc_h = desc.size().y.min(36.0);
        p.with_clip_rect(Rect::from_min_size(Pos2::new(rect.left() + 22.0, icon.bottom() + 10.0), vec2(rect.width() - 44.0, desc_h)))
            .galley(Pos2::new(rect.left() + 22.0, icon.bottom() + 10.0), desc, MUTED);
        let list_top = icon.bottom() + 18.0 + desc_h;
        text(&p, Pos2::new(rect.left() + 22.0, list_top), Align2::LEFT_TOP, "Версии", bold(14.0), TEXT);

        let dest = self.shader_dest();
        let present: Vec<String> = match &dest {
            Ok(dir) => std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect(),
            Err(_) => Vec::new(),
        };
        let versions = self.shader_versions(ctx, &hit.project_id);
        let area = Rect::from_min_max(Pos2::new(rect.left() + 12.0, list_top + 24.0), rect.max - vec2(12.0, 8.0));
        let mut c = child_ui(ui, area, "shader_versions", Layout::top_down(Align::Min));
        let running = self.task.running();
        let mut action = None;
        egui::ScrollArea::vertical().id_salt("shader_versions_scroll").auto_shrink([false; 2]).show(&mut c, |ui| {
            if let Err(why) = &dest {
                ui.label(egui::RichText::new(*why).color(MUTED).size(12.5));
                return;
            }
            match &versions {
                None => {
                    ui.label(egui::RichText::new("Загрузка версий…").color(MUTED));
                }
                Some(Err(e)) => {
                    ui.label(egui::RichText::new(e).color(RED));
                }
                Some(Ok(list)) => {
                    for (i, v) in list.iter().take(40).enumerate() {
                        let Some(file) = v.primary_file() else { continue };
                        let width = ui.available_width();
                        let (r, _) = ui.allocate_exact_size(vec2(width, 36.0), Sense::hover());
                        let rp = ui.painter().clone();
                        text_fit(&rp, r.left_center() + vec2(10.0, 0.0), Align2::LEFT_CENTER, &format!("v{}", v.version_number), bold(14.0), TEXT, 150.0);
                        text_fit(&rp, r.left_center() + vec2(140.0, 0.0), Align2::LEFT_CENTER, &v.loaders.join(" · "), font(11.5), MUTED, (r.width() - 290.0).max(40.0));
                        let btn = Rect::from_min_size(r.right_center() + vec2(-128.0, -14.0), vec2(120.0, 28.0));
                        if present.contains(&file.filename) {
                            text(&rp, btn.center(), Align2::CENTER_CENTER, "✔ установлен", font(12.5), GREEN);
                        } else if !running && small_button(ui, btn, ui.id().with(("shinst", i)), "⬇ Установить", true) {
                            action = Some(v.clone());
                        }
                    }
                }
            }
        });
        if let (Some(version), Ok(dir)) = (action, dest) {
            let Some(file) = version.primary_file().cloned() else { return };
            if self.shader_for_horizon {
                self.cfg.fine.shader_pack = file.filename.clone();
            }
            let title = hit.title.clone();
            self.spawn_job(ctx, "Загрузка шейдеров", false, false, move |task| {
                task.stage(format!("{title} v{}", version.version_number));
                let job = net::Job { url: file.url.clone(), path: dir.join(&file.filename), sha1: file.hashes.sha1.clone(), size: Some(file.size) };
                net::download(&job, task)?;
                task.log(format!("{} установлен в {}", file.filename, dir.display()));
                Ok(None)
            });
        }
    }

    /// Zyntra Fine on its own page: the settings on the left, a summary of what applies on the right.
    fn fine_page(&mut self, ctx: &egui::Context, ui: &mut Ui, body: Rect) {
        let p = ui.painter().clone();
        let settings_w = if body.width() > 760.0 { (body.width() - GAP) * 0.6 } else { body.width() };
        let settings = Rect::from_min_size(body.min, vec2(settings_w, body.height()));
        card(&p, settings);
        let game = horizon::game_folder(&self.cfg.game_dir);
        let installed = hz_mods::installed(&game);
        self.horizon_detail(ctx, ui, settings, hz_mods::OPTIMIZER_ID, None, &game, &installed);
        if settings_w >= body.width() {
            return;
        }

        // Summary
        let side = Rect::from_min_max(Pos2::new(settings.right() + GAP, body.top()), body.max);
        card(&p, side);
        card_glow(&p, side, 20.0, BG, side.center_bottom(), side.width() * 0.8, glow_color(70), BORDER);
        let f = &self.cfg.fine;
        let on = |v: bool| if v { "вкл" } else { "выкл" };
        let shader = if f.shader_pack.is_empty() { "выкл".to_owned() } else { f.shader_pack.clone() };
        let rows = [
            ("Версия", Target::HorizonFine.title()),
            (
                "Сборка Horizon",
                horizon::installed(&self.cfg.game_dir)
                    .map(|b| format!("nightly {}", b.updated_at.get(..10).unwrap_or_default()))
                    .unwrap_or_else(|| "не установлена".into()),
            ),
            (
                "Нативные моды",
                if native::ready(&self.cfg.game_dir).is_some() {
                    format!("{} в сборке", native::selected(&self.cfg.game_dir).len())
                } else if native::selected(&self.cfg.game_dir).is_empty() {
                    "нет".into()
                } else {
                    "нужно собрать".into()
                },
            ),
            ("Дальность / симуляция", format!("{} / {}", f.render_distance, f.simulation_distance)),
            ("Подгрузка чанков", if f.chunk_updates == 0 { "все сразу".into() } else { format!("{} за кадр", f.chunk_updates) }),
            ("Лимит FPS", if f.max_fps >= 260 { "без лимита".into() } else { f.max_fps.to_string() }),
            ("Зум", if f.zoom { format!("клавиша {}", f.zoom_key) } else { "выкл".into() }),
            ("Шейдер", shader),
            ("Динамический свет", on(self.cfg.hz_extras.get("dynamicLights").copied().unwrap_or(true)).into()),
            ("Яркость на максимум", on(self.cfg.max_gamma).into()),
        ];
        text(&p, side.left_top() + vec2(24.0, 22.0), Align2::LEFT_TOP, "Что включится", bold(19.0), TEXT);
        let mut y = side.top() + 64.0;
        for (k, v) in rows {
            text(&p, Pos2::new(side.left() + 24.0, y), Align2::LEFT_TOP, k, font(13.0), MUTED);
            text_fit(&p, Pos2::new(side.right() - 24.0, y), Align2::RIGHT_TOP, &v, bold(13.5), TEXT, side.width() * 0.5);
            y += 30.0;
        }
        let note = p.layout(
            "Всё это применяется при каждом запуске версии «Horizon OptiFine». «Horizon 26.3» запускается с обычными настройками игры.".into(),
            font(12.5),
            MUTED,
            side.width() - 48.0,
        );
        p.galley(Pos2::new(side.left() + 24.0, y + 10.0), note, MUTED);
        // Version above the button, so the button's own text stays short enough to fit.
        let btn = Rect::from_center_size(Pos2::new(side.center().x, side.bottom() - 50.0), vec2((side.width() - 48.0).min(340.0), 50.0));
        text(&p, btn.center_top() - vec2(0.0, 12.0), Align2::CENTER_BOTTOM, Target::HorizonFine.title(), bold(15.0), ACCENT);
        let mut c = child_ui(ui, btn, "fine_play", Layout::top_down(Align::Min));
        if primary(&mut c, "▶   ИГРАТЬ", btn.size(), !self.task.running()).clicked() {
            self.cfg.target = Some(Target::HorizonFine);
            self.start(ctx, Target::HorizonFine, true);
        }
    }

    /// Packs in the destination folder; for Horizon, the one «Horizon OptiFine» uses is picked here.
    fn installed_shaders(&mut self, ui: &mut Ui, rect: Rect) {
        let p = ui.painter().clone();
        let dest = self.shader_dest();
        let header = Rect::from_min_max(rect.min + vec2(18.0, 12.0), Pos2::new(rect.right() - 18.0, rect.top() + 50.0));
        let mut h = child_ui(ui, header, "shader_installed_header", Layout::left_to_right(Align::Center));
        if let Ok(dir) = &dest {
            if pill(&mut h, "📁 Папка шейдеров", false).on_hover_text(dir.display().to_string()).clicked() {
                open_folder(dir);
            }
        }
        let caption = if self.shader_for_horizon {
            "Установлено для Horizon (выбранный включается в «Horizon OptiFine»)".to_owned()
        } else {
            "Установлено для Java-версии (нужен Iris или OptiFine)".to_owned()
        };
        text_fit(&p, Pos2::new(rect.left() + 22.0, header.bottom() + 10.0), Align2::LEFT_TOP, &caption, font(12.5), MUTED, rect.width() - 44.0);
        let Ok(dir) = dest else { return };
        let packs: Vec<String> = if self.shader_for_horizon {
            horizon::shader_packs(&self.cfg.game_dir)
        } else {
            let mut list: Vec<String> = std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.path().is_dir() || e.path().extension().is_some_and(|x| x == "zip"))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            list.sort();
            list
        };
        let area = Rect::from_min_max(Pos2::new(rect.left() + 12.0, header.bottom() + 34.0), rect.max - vec2(12.0, 10.0));
        let mut c = child_ui(ui, area, "installed_shaders", Layout::top_down(Align::Min));
        let (mut pick, mut remove) = (None, None);
        egui::ScrollArea::vertical().id_salt("installed_shaders_scroll").auto_shrink([false; 2]).show(&mut c, |ui| {
            if packs.is_empty() {
                ui.label(egui::RichText::new("Пока пусто — установи шейдер из списка").color(MUTED).size(12.5));
            }
            for (i, name) in packs.iter().enumerate() {
                let width = ui.available_width();
                let (r, _) = ui.allocate_exact_size(vec2(width, 32.0), Sense::hover());
                let rp = ui.painter().clone();
                text_fit(&rp, r.left_center() + vec2(10.0, 0.0), Align2::LEFT_CENTER, name, font(13.0), TEXT, r.width() - 190.0);
                if self.shader_for_horizon {
                    let on = self.cfg.fine.shader_pack == *name;
                    let br = Rect::from_center_size(r.right_center() - vec2(96.0, 0.0), vec2(96.0, 26.0));
                    if small_button(ui, br, ui.id().with(("shpick", i)), if on { "✔ выбран" } else { "выбрать" }, on) {
                        pick = Some(if on { String::new() } else { name.clone() });
                    }
                }
                let del = Rect::from_center_size(r.right_center() - vec2(20.0, 0.0), vec2(28.0, 24.0));
                if small_button(ui, del, ui.id().with(("shdel", i)), "✖", false) {
                    remove = Some(dir.join(name));
                }
            }
        });
        if let Some(name) = pick {
            self.cfg.fine.shader_pack = name;
        }
        if let Some(path) = remove {
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(&path);
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    fn console(&mut self, ctx: &egui::Context, ui: &mut Ui, body: Rect) {
        let p = ui.painter().clone();
        card(&p, body);
        text(&p, body.left_top() + vec2(22.0, 20.0), Align2::LEFT_TOP, "Консоль", bold(20.0), TEXT);
        let state = self.task.snapshot();
        let btns = Rect::from_min_size(body.right_top() + vec2(-330.0, 16.0), vec2(310.0, 34.0));
        let mut b = child_ui(ui, btns, "console_btns", Layout::right_to_left(Align::Center));
        b.spacing_mut().item_spacing.x = 8.0;
        if pill(&mut b, "Копировать", false).clicked() {
            ctx.copy_text(state.log.join("\n"));
        }
        if pill(&mut b, "Папка логов", false).clicked() {
            open_folder(&self.cfg.game_dir.join("logs"));
        }
        let errors = self.remote.lock().unwrap().errors.clone();
        let area = Rect::from_min_max(body.min + vec2(16.0, 66.0), body.max - vec2(16.0, 16.0));
        p.rect_filled(area, 14, SUNKEN);
        let mut c = child_ui(ui, area.shrink(12.0), "console_area", Layout::top_down(Align::Min));
        egui::ScrollArea::vertical().id_salt("console_scroll").stick_to_bottom(true).auto_shrink([false; 2]).show(&mut c, |ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            for e in &errors {
                ui.label(egui::RichText::new(format!("сеть: {e}")).monospace().size(12.5).color(RED));
            }
            if state.log.is_empty() {
                ui.label(egui::RichText::new("Здесь появится ход установки и запуска.").color(MUTED));
            }
            for line in &state.log {
                let color = if line.starts_with("ОШИБКА") { RED } else if line.starts_with("  ") { MUTED } else { TEXT };
                ui.label(egui::RichText::new(line).monospace().size(12.5).color(color));
            }
        });
    }

    fn settings(&mut self, ui: &mut Ui, body: Rect) {
        let p = ui.painter().clone();
        card(&p, body);
        card_glow(&p, body, 20.0, BG, body.right_top(), 320.0, glow_color(30), BORDER);
        text(&p, body.left_top() + vec2(26.0, 22.0), Align2::LEFT_TOP, "Настройки", bold(22.0), TEXT);
        let inner = Rect::from_min_max(body.min + vec2(26.0, 72.0), body.max - vec2(26.0, 20.0));
        let mut c = child_ui(ui, inner, "settings", Layout::top_down(Align::Min));
        let total_ram = self.total_ram;
        let cfg = &mut self.cfg;
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(&mut c, |ui| {
            egui::Grid::new("settings_grid").num_columns(2).spacing([28.0, 18.0]).min_col_width(200.0).show(ui, |ui| {
                label(ui, "Ник в игре");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut cfg.nickname).desired_width(260.0).char_limit(16));
                    if cfg.nickname_valid() {
                        ui.label(egui::RichText::new("✔").color(GREEN));
                    } else {
                        ui.label(egui::RichText::new("3–16 символов: A–Z, 0–9, _").color(RED).size(12.5));
                    }
                });
                ui.end_row();

                label(ui, "Оперативная память");
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(300.0, 24.0), Sense::hover());
                    if ram_slider(ui, rect, &mut cfg.ram_mb, 1024, system::max_game_ram_mb(total_ram), system::recommended_ram_mb(total_ram)) {
                        cfg.ram_auto = false;
                    }
                    ui.label(egui::RichText::new(format!("{:.1} ГБ из {} ГБ", cfg.ram_mb as f32 / 1024.0, (total_ram + 512) / 1024)).color(ACCENT).strong());
                    ui.checkbox(&mut cfg.ram_auto, "Авто");
                });
                ui.end_row();

                label(ui, "Папка игры");
                path_row(ui, &mut cfg.game_dir, "game_dir");
                ui.end_row();

                label(ui, "Java");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut cfg.java_path).desired_width(420.0).hint_text("пусто = Java от Mojang скачается сама"));
                    if ui.button("Обзор…").clicked() {
                        if let Some(f) = rfd::FileDialog::new().add_filter("Java", &["exe"]).pick_file() {
                            cfg.java_path = f.display().to_string();
                        }
                    }
                });
                ui.end_row();

                label(ui, "Размер окна игры");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut cfg.width).range(640..=7680));
                    ui.label("×");
                    ui.add(egui::DragValue::new(&mut cfg.height).range(480..=4320));
                });
                ui.end_row();

                label(ui, "После запуска игры");
                ui.vertical(|ui| {
                    ui.checkbox(&mut cfg.hide_on_launch, "Скрывать Zyntra во время игры и открывать после выхода");
                    ui.checkbox(&mut cfg.close_on_launch, "Полностью закрывать Zyntra");
                });
                ui.end_row();

                label(ui, "Яркость");
                ui.checkbox(&mut cfg.max_gamma, "На максимум (гамма 100%) — лучше видно в темноте, во всех версиях");
                ui.end_row();

                label(ui, "Графика Horizon");
                ui.vertical(|ui| {
                    ui.label("Версия «Horizon OptiFine» = Horizon + Zyntra Fine и зум.");
                    ui.label(
                        egui::RichText::new("Пресеты и все настройки — в разделе «Моды» → Horizon → Zyntra Fine.")
                            .color(MUTED)
                            .size(12.0),
                    );
                });
                ui.end_row();

                label(ui, "Версии из TLauncher");
                ui.vertical(|ui| {
                    ui.checkbox(&mut cfg.show_tlauncher, "Показывать уже установленные версии TLauncher");
                    path_row(ui, &mut cfg.tlauncher_dir, "tl_dir");
                });
                ui.end_row();
            });
            ui.add_space(18.0);
            ui.label(egui::RichText::new("Настройки сохраняются автоматически в zyntra.json рядом с программой.").color(MUTED).size(12.5));
        });
    }
}

/// A caption on its own line, then a slider as wide as the panel.
fn labeled_slider(ui: &mut Ui, caption: &str, slider: egui::Slider) {
    ui.label(egui::RichText::new(caption).size(13.0));
    ui.spacing_mut().slider_width = (ui.available_width() - 70.0).max(80.0);
    ui.add(slider);
}

fn label(ui: &mut Ui, s: &str) {
    ui.label(egui::RichText::new(s).color(MUTED));
}

fn path_row(ui: &mut Ui, path: &mut PathBuf, id: &str) {
    ui.horizontal(|ui| {
        let mut s = path.display().to_string();
        if ui.add(egui::TextEdit::singleline(&mut s).desired_width(420.0).id_salt(id)).changed() {
            *path = PathBuf::from(s);
        }
        if ui.button("Обзор…").clicked() {
            if let Some(dir) = rfd::FileDialog::new().set_directory(&*path).pick_folder() {
                *path = dir;
            }
        }
    });
}

fn versions_hint(f: Filter) -> &'static str {
    match f {
        Filter::Release => "Все официальные релизы — тот же список, что и в TLauncher. Двойной клик — играть.",
        Filter::Snapshot => "Тестовые версии Mojang с новыми фичами.",
        Filter::Fabric => "Fabric ставится автоматически поверх выбранной версии — кидай моды в папку mods.",
        Filter::Old => "Классика: Beta и Alpha версии.",
        Filter::Installed => "Версии в папке Zyntra и уже скачанные в TLauncher (Forge, OptiFine, Fabric…).",
    }
}

fn nav_item(ui: &mut Ui, rect: Rect, icon: &str, label: &str, selected: bool) -> bool {
    let resp = ui.interact(rect, ui.id().with(("nav", label)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    let p = ui.painter();
    if selected {
        p.add(egui::epaint::Shadow { offset: [0, 0], blur: 18, spread: 0, color: glow_color(60) }.as_shape(rect, 14));
        p.rect(rect, 14, Color32::from_rgb(92, 56, 110), Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(222, 168, 238, 90)), StrokeKind::Inside);

    } else if resp.hovered() {
        p.rect_filled(rect, 14, CARD);
    }
    let color = if selected || resp.hovered() { TEXT } else { Color32::from_rgb(196, 188, 206) };
    crate::ui::icon(p, rect.left_center() + vec2(22.0, 0.0), icon, 16.0, color);
    text(p, rect.left_center() + vec2(46.0, 0.0), Align2::LEFT_CENTER, label, font(15.0), color);
    resp.clicked()
}

struct RowResponse {
    row: egui::Response,
    install: bool,
    play: bool,
}

fn small_button(ui: &mut Ui, rect: Rect, id: egui::Id, label: &str, accent: bool) -> bool {
    let resp = ui.interact(rect, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    let p = ui.painter();
    if accent {
        p.rect_filled(rect, 15, if resp.hovered() { Color32::from_rgb(232, 186, 245) } else { ACCENT });
        text(p, rect.center(), Align2::CENTER_CENTER, label, bold(13.0), Color32::from_rgb(40, 18, 48));
    } else {
        p.rect(rect, 15, if resp.hovered() { CARD_HI } else { SUNKEN }, Stroke::new(1.0_f32, if resp.hovered() { ACCENT_DIM } else { BORDER }), StrokeKind::Inside);
        text(p, rect.center(), Align2::CENTER_CENTER, label, font(13.0), TEXT);
    }
    resp.clicked()
}

/// One version row. With `buttons`, shows Install/Play on the right.
fn list_row(ui: &mut Ui, e: &Entry, selected: bool, buttons: bool, icon: egui::TextureId) -> RowResponse {
    let width = ui.available_width();
    let (rect, row) = ui.allocate_exact_size(vec2(width, 54.0), Sense::click());
    let row = row.on_hover_cursor(CursorIcon::PointingHand);
    let p = ui.painter().clone();
    if selected {
        p.rect(rect, 14, CARD_HI, Stroke::new(1.0_f32, ACCENT_DIM), StrokeKind::Inside);
    } else if row.hovered() {
        p.rect_filled(rect, 14, Color32::from_rgb(29, 24, 35));
    }
    block_icon(&p, Rect::from_min_size(rect.left_center() + vec2(8.0, -18.0), vec2(36.0, 36.0)), icon);
    let right_reserved = if buttons { 230.0 } else { 120.0 };
    text_fit(&p, rect.left_center() + vec2(54.0, -9.0), Align2::LEFT_CENTER, &e.title, bold(15.0), TEXT, width - 54.0 - right_reserved);
    text_fit(&p, rect.left_center() + vec2(54.0, 11.0), Align2::LEFT_CENTER, &e.sub, font(12.0), MUTED, width - 54.0 - right_reserved);

    let mut install = false;
    let mut play = false;
    if buttons {
        let play_r = Rect::from_min_size(rect.right_center() + vec2(-100.0, -15.0), vec2(88.0, 30.0));
        play = small_button(ui, play_r, ui.id().with(("play", &e.title)), "▶ Играть", true);
        if e.installed {
            text(&p, play_r.left_center() - vec2(14.0, 0.0), Align2::RIGHT_CENTER, "✔ установлена", font(12.5), GREEN);
        } else {
            let inst_r = play_r.translate(vec2(-112.0, 0.0)).expand2(vec2(10.0, 0.0)).translate(vec2(4.0, 0.0));
            install = small_button(ui, inst_r, ui.id().with(("inst", &e.title)), "⬇ Скачать", false);
        }
    } else if e.installed {
        text(&p, rect.right_center() - vec2(14.0, 0.0), Align2::RIGHT_CENTER, "✔ установлена", font(12.0), GREEN);
    } else {
        text(&p, rect.right_center() - vec2(14.0, 0.0), Align2::RIGHT_CENTER, "скачать", font(12.0), MUTED);
    }
    RowResponse { row, install, play }
}

/// Brightness slider at its top ("Яркая") in a version's options.txt.
fn max_gamma(options: &Path, task: &Task) {
    match horizon::set_keys(options, &[("gamma", "1.0")]) {
        Ok(()) => task.log("Яркость: на максимум"),
        Err(e) => task.log(format!("Не удалось поставить яркость: {e:#}")),
    }
}

fn run_target(cfg: &Config, manifest: Option<&Manifest>, target: &Target, play: bool, task: &Task) -> Result<Option<Child>> {
    let game = &cfg.game_dir;
    if target.is_horizon() {
        // Only the game build is downloaded (no Horizon launcher); refreshed when a newer nightly exists.
        // GitHub is asked at most every 6 hours, so a normal Play starts right away.
        let local = horizon::installed(game);
        let outdated = match &local {
            None => true,
            Some(_) if horizon::checked_recently(game) => false,
            Some(l) => {
                let remote = horizon::remote();
                horizon::mark_checked(game);
                remote.is_ok_and(|r| r.asset_id != l.asset_id)
            }
        };
        if outdated {
            horizon::install(game, task)?;
        }
        horizon::prepare(game, manifest, &cfg.java_path, task)?;
        if !play {
            return Ok(None);
        }
        let skin_file = skin::effective_path(cfg.skin_path.as_deref(), game);
        let player = horizon::Player { name: &cfg.nickname, skin: skin_file.as_deref() };
        // «Horizon OptiFine» is the same install with Zyntra Fine, its extras and zoom.
        let fine = (*target == Target::HorizonFine).then_some((&cfg.fine, &cfg.hz_extras));
        if cfg.max_gamma {
            max_gamma(&horizon::game_folder(game).join("options.txt"), task);
        }
        return Ok(Some(horizon::launch(game, &player, fine, task)?));
    }
    let (dirs, id) = match target {
        Target::Vanilla(id) => (Dirs::new(game.clone(), game), id.clone()),
        Target::Fabric(version) => {
            let dirs = Dirs::new(game.clone(), game);
            let id = fabric::install(&dirs, version, task)?;
            (dirs, id)
        }
        Target::Installed { id, tl } => {
            let root = if *tl { cfg.tlauncher_dir.clone() } else { game.clone() };
            (Dirs::new(root, game), id.clone())
        }
        Target::Horizon | Target::HorizonFine => unreachable!(),
    };
    let prepared = minecraft::install(&dirs, &id, manifest, &cfg.java_path, task)?;
    // The chosen skin goes in as a resource pack (offline games show the default skins).
    let modern = prepared.v["arguments"].is_object();
    let main_class = prepared.v["mainClass"].as_str().unwrap_or_default();
    let forge = ["cpw.mods", "minecraftforge", "neoforged"].iter().any(|m| main_class.contains(m));
    let game_dir = cfg.game_dir_for(target);
    if let Err(e) = skin::apply_pack(&game_dir, skin::effective_path(cfg.skin_path.as_deref(), game).as_deref(), modern, forge) {
        task.log(format!("Скин не подключён: {e:#}"));
    }
    if cfg.max_gamma {
        max_gamma(&game_dir.join("options.txt"), task);
    }
    if !play {
        return Ok(None);
    }
    let opts = LaunchOptions { game_dir, nickname: cfg.nickname.clone(), ram_mb: cfg.ram_mb, width: cfg.width, height: cfg.height };
    Ok(Some(minecraft::launch(&prepared, &opts, task)?))
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.sample_speed();
        let max_ram = system::max_game_ram_mb(self.total_ram);
        if self.cfg.ram_auto {
            self.cfg.ram_mb = system::recommended_ram_mb(self.total_ram);
        }
        self.cfg.ram_mb = self.cfg.ram_mb.clamp(1024, max_ram);
        let running = self.task.running();
        if self.was_running && !running {
            self.refresh_installed();
        }
        self.was_running = running;
        ctx.request_repaint_after(Duration::from_millis(if running { 60 } else { 250 }));

        if self.cfg.target.is_none() {
            if let Some(m) = &self.remote.lock().unwrap().manifest {
                self.cfg.target = Some(Target::Vanilla(m.latest.release.clone()));
            }
        }

        egui::CentralPanel::default().frame(egui::Frame::default().fill(BG)).show(ctx, |ui| {
            let full = ui.max_rect();
            let side = Rect::from_min_size(full.min + vec2(16.0, 16.0), vec2(238.0, full.height() - 32.0));
            self.sidebar(ui, side);
            let main = Rect::from_min_max(Pos2::new(side.right() + 20.0, full.top() + 20.0), full.max - vec2(20.0, 16.0));
            self.header(ui, Rect::from_min_size(main.min, vec2(main.width(), 62.0)));
            let body = Rect::from_min_max(main.min + vec2(0.0, 82.0), main.max);
            match self.page {
                Page::Home => self.home(ctx, ui, body),
                Page::Versions => self.versions(ctx, ui, body),
                Page::Skins => self.skins(ui, body),
                Page::Mods => self.mods_page(ctx, ui, body),
                Page::Shaders => self.shaders_page(ctx, ui, body),
                Page::Fine => self.fine_page(ctx, ui, body),
                Page::Console => self.console(ctx, ui, body),
                Page::Settings => self.settings(ui, body),
            }
        });

        if self.cfg.skin_path != self.skin_loaded {
            self.reload_skin();
        }
        if self.cfg != self.saved {
            let dirs_changed = self.cfg.game_dir != self.saved.game_dir
                || self.cfg.tlauncher_dir != self.saved.tlauncher_dir
                || self.cfg.show_tlauncher != self.saved.show_tlauncher;
            self.cfg.save();
            self.saved = self.cfg.clone();
            if dirs_changed {
                self.refresh_installed();
            }
        }
    }
}

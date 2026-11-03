//! Add-ons for Horizon: its own registry (horizon-rust.github.io/horizon-mods), the mods
//! bundled with Zyntra, and Zyntra Optimizer — Horizon's built-in OptiFine-style extras.

use crate::net::{self, Job};
use crate::task::Task;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const INDEX_URL: &str = "https://horizon-rust.github.io/horizon-mods/index.json";
const SWITCHES: &str = "horizon-mods.txt";
pub const CRYSTALS_ID: &str = "zyntra_crystals";
pub const OPTIMIZER_ID: &str = "zyntra_optimizer";

/// Zyntra Crystals (mods-src/zyntra_crystals), packed by build.rs.
const CRYSTALS_ZIP: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/zyntra_crystals.zip"));
/// Zyntra Xray resource pack (mods-src/zyntra_xray), packed by build.rs.
const XRAY_ZIP: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/zyntra_xray.zip"));
pub const XRAY_ID: &str = "zyntra_xray";
pub const XRAY_FILE: &str = "ZyntraXray.zip";

/// Horizon's built-in extras Zyntra Fine switches (key in horizon-extras.txt, title, what it does,
/// default: dynamic lights are on, as in OptiFine).
pub const EXTRAS: [(&str, &str, &str, bool); 7] = [
    ("fullbright", "Полная яркость", "Всё видно даже в тёмных пещерах", false),
    ("dynamicLights", "Динамический свет", "Факел в любой руке, брошенный факел и горящие мобы освещают всё вокруг", true),
    ("hideBlockParticles", "Без частиц ломания", "Меньше нагрузки при копании", false),
    ("mouseTweaks", "Удобный инвентарь", "Перетаскивание и колесо мыши, как в Mouse Tweaks", false),
    ("sort", "Сортировка инвентаря", "Кнопка сортировки в сундуках и инвентаре", false),
    ("chatAnimation", "Плавный чат", "Сообщения появляются плавно", false),
    ("autoReconnect", "Автопереподключение", "Сам заходит обратно, если сервер отключил", false),
];

#[derive(Deserialize, Clone)]
pub struct Project {
    pub id: String,
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub summary: String,
    pub icon: Option<String>,
    #[serde(default)]
    pub downloads: u64,
    pub versions: Vec<ProjectVersion>,
}

#[derive(Deserialize, Clone)]
pub struct ProjectVersion {
    pub version: String,
    pub file: ProjectFile,
}

#[derive(Deserialize, Clone)]
pub struct ProjectFile {
    pub name: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Deserialize)]
struct Index {
    projects: Vec<Project>,
}

impl Project {
    /// Kinds that install as files, without building Horizon from source.
    pub fn installable(&self) -> bool {
        matches!(self.kind.as_str(), "mod" | "resourcepack" | "shader")
    }

    pub fn kind_ru(&self) -> &'static str {
        match self.kind.as_str() {
            "mod" => "Контентный мод",
            "native-mod" => "Нативный мод",
            "resourcepack" => "Ресурс-пак",
            "shader" => "Шейдеры",
            "modpack" => "Сборка",
            _ => "Дополнение",
        }
    }

    fn folder(&self) -> &'static str {
        match self.kind.as_str() {
            "resourcepack" => "resourcepacks",
            "shader" => "shaderpacks",
            _ => "horizon-mods",
        }
    }
}

/// Projects in Horizon's registry, most downloaded first.
pub fn fetch_index() -> Result<Vec<Project>> {
    let mut index: Index = net::get_json(INDEX_URL)?;
    index.projects.retain(|p| !p.versions.is_empty());
    index.projects.sort_by(|a, b| b.downloads.cmp(&a.downloads));
    Ok(index.projects)
}

pub fn sha256_of(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Switches mod `id` on or off in horizon-mods.txt (`mod.<id>:true`).
pub fn set_enabled(game: &Path, id: &str, on: bool) -> Result<()> {
    let path = game.join(SWITCHES);
    let prefix = format!("mod.{id}:");
    let mut lines: Vec<String> = fs::read_to_string(&path).unwrap_or_default().lines().filter(|l| !l.starts_with(&prefix)).map(str::to_owned).collect();
    lines.push(format!("{prefix}{on}"));
    fs::write(path, lines.join("\n") + "\n")?;
    Ok(())
}

fn switches(game: &Path) -> BTreeMap<String, bool> {
    fs::read_to_string(game.join(SWITCHES))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (key, value) = l.strip_prefix("mod.")?.split_once(':')?;
            Some((key.to_owned(), value.trim() == "true"))
        })
        .collect()
}

/// Downloads a registry project into the Horizon game folder, checks its SHA-256 and turns
/// a mod on (a resource pack is also added to the selected packs).
pub fn install(project: &Project, game: &Path, task: &Task) -> Result<()> {
    if !project.installable() {
        bail!("{} — нативный мод: его нужно собирать вместе с Horizon из исходников", project.name);
    }
    let version = &project.versions[0];
    let dir = game.join(project.folder());
    fs::create_dir_all(&dir)?;
    let path = dir.join(&version.file.name);
    task.stage(format!("Horizon: {} {}", project.name, version.version));
    net::download(&Job { url: version.file.url.clone(), path: path.clone(), sha1: None, size: Some(version.file.size) }, task)?;
    if !sha256_of(&path)?.eq_ignore_ascii_case(&version.file.sha256) {
        fs::remove_file(&path).ok();
        bail!("{}: контрольная сумма не совпала с каталогом Horizon — файл удалён", version.file.name);
    }
    match project.kind.as_str() {
        "mod" => set_enabled(game, &project.id, true)?,
        "resourcepack" => crate::skin::enable_pack(&game.join("options.txt"), &format!("file/{}", version.file.name))?,
        _ => {}
    }
    Ok(())
}

/// Unpacks the bundled Zyntra Crystals into horizon-mods/ and turns it on.
pub fn install_crystals(game: &Path) -> Result<()> {
    let dir = game.join("horizon-mods").join(CRYSTALS_ID);
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
    }
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(CRYSTALS_ZIP))?;
    zip.extract(&dir).context("распаковка Zyntra Crystals")?;
    set_enabled(game, CRYSTALS_ID, true)
}

/// Writes the bundled Xray pack into `game/resourcepacks/` and selects it (topmost).
pub fn install_xray(game: &Path) -> Result<()> {
    let dir = game.join("resourcepacks");
    fs::create_dir_all(&dir)?;
    fs::write(dir.join(XRAY_FILE), XRAY_ZIP)?;
    crate::skin::enable_pack(&game.join("options.txt"), &format!("file/{XRAY_FILE}"))
}

/// One installed add-on in the Horizon game folder.
pub struct Installed {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    /// Some for mods (they have an on/off switch), None for packs.
    pub enabled: Option<bool>,
    pub kind: &'static str,
}

/// Mods (folders with horizon-mod.toml, or .hmod files), resource packs and shader packs.
pub fn installed(game: &Path) -> Vec<Installed> {
    let switches = switches(game);
    let mut out = Vec::new();
    let entries = |folder: &str| -> Vec<PathBuf> {
        let mut list: Vec<PathBuf> = fs::read_dir(game.join(folder)).into_iter().flatten().flatten().map(|e| e.path()).collect();
        list.sort();
        list
    };
    for path in entries("horizon-mods") {
        let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let id = if path.is_dir() {
            match fs::read_to_string(path.join("horizon-mod.toml")).ok().and_then(|t| manifest_value(&t, "id")) {
                Some(id) => id,
                None => continue,
            }
        } else if file_name.ends_with(".hmod") {
            // <id>-<version>.hmod
            file_name.trim_end_matches(".hmod").split('-').next().unwrap_or_default().to_owned()
        } else {
            continue;
        };
        let name = fs::read_to_string(path.join("horizon-mod.toml")).ok().and_then(|t| manifest_value(&t, "name")).unwrap_or_else(|| id.clone());
        out.push(Installed { enabled: Some(*switches.get(&id).unwrap_or(&false)), id, name, path, kind: "Мод" });
    }
    for (folder, kind) in [("resourcepacks", "Ресурс-пак"), ("shaderpacks", "Шейдеры")] {
        for path in entries(folder) {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if name.ends_with(".txt") {
                continue; // a shader pack's saved options
            }
            out.push(Installed { id: name.clone(), name, path, enabled: None, kind });
        }
    }
    // Native mods in the build (horizon-native-mods/*.hnmod); ✖ takes one out of the build.
    for path in entries(crate::native::MODS_DIR) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name.ends_with(".hnmod") {
            out.push(Installed { id: name.clone(), name, path, enabled: None, kind: "Нативный" });
        }
    }
    out
}

fn manifest_value(toml: &str, key: &str) -> Option<String> {
    toml.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == key).then(|| v.trim().trim_matches('"').to_owned())
    })
}

pub fn remove(item: &Installed) -> Result<()> {
    if item.path.is_dir() {
        fs::remove_dir_all(&item.path)?;
    } else {
        fs::remove_file(&item.path)?;
    }
    Ok(())
}

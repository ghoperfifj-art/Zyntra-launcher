//! Player skins: loading PNG files, the launcher's own default character, and a
//! resource pack that shows the chosen skin in offline games.

use anyhow::{bail, Context, Result};
use eframe::egui::ColorImage;
use image::{imageops, RgbaImage};
use std::fs;
use std::path::Path;

pub const PACK_NAME: &str = "ZyntraSkin";

/// Loads a 64×64 or legacy 64×32 skin and returns it as 64×64.
pub fn load(path: &Path) -> Result<RgbaImage> {
    let img = image::open(path).with_context(|| format!("Не удалось открыть {}", path.display()))?.to_rgba8();
    if img.width() != 64 || !(img.height() == 64 || img.height() == 32) {
        bail!("Скин должен быть PNG 64×64 или 64×32 (а этот {}×{})", img.width(), img.height());
    }
    let mut full = RgbaImage::new(64, 64);
    imageops::overlay(&mut full, &img, 0, 0);
    if img.height() == 32 {
        // Old skins have no separate left limbs: reuse the right ones.
        let leg = imageops::crop_imm(&img, 0, 16, 16, 16).to_image();
        let arm = imageops::crop_imm(&img, 40, 16, 16, 16).to_image();
        imageops::overlay(&mut full, &leg, 16, 48);
        imageops::overlay(&mut full, &arm, 32, 48);
    }
    Ok(full)
}

pub fn to_color_image(img: &RgbaImage) -> ColorImage {
    ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw())
}

/// Default skin shipped with the launcher (assets/default_skin.png, chosen by the user).
pub fn default_skin() -> RgbaImage {
    let img = image::load_from_memory(include_bytes!("../assets/default_skin.png")).expect("default_skin.png").to_rgba8();
    let mut full = RgbaImage::new(64, 64);
    imageops::overlay(&mut full, &img, 0, 0);
    full
}

/// The skin file to use in game: the chosen one, or the launcher's default character
/// saved once into the game folder, so the game matches the launcher's preview.
pub fn effective_path(chosen: Option<&Path>, game_dir: &Path) -> Option<std::path::PathBuf> {
    if let Some(path) = chosen {
        return Some(path.to_path_buf());
    }
    // Rewritten every time so a launcher update with a new default skin takes effect.
    let path = game_dir.join("zyntra-default-skin.png");
    fs::create_dir_all(game_dir).ok()?;
    default_skin().save(&path).ok()?;
    Some(path)
}

/// Puts the chosen skin into `root/resourcepacks/ZyntraSkin` (replacing the game's default
/// player textures) and enables the pack in options.txt. With no skin, removes the pack.
pub fn apply_pack(root: &Path, skin_path: Option<&Path>, modern: bool, forge: bool) -> Result<()> {
    let pack = root.join("resourcepacks").join(PACK_NAME);
    let Some(skin_path) = skin_path else {
        if pack.exists() {
            fs::remove_dir_all(&pack).ok();
        }
        return Ok(());
    };
    let skin = load(skin_path)?;
    fs::create_dir_all(&pack)?;
    fs::write(
        pack.join("pack.mcmeta"),
        r#"{"pack":{"pack_format":15,"supported_formats":[1,999],"min_format":1,"max_format":999,"description":"Zyntra Launcher: your skin"}}"#,
    )?;
    let textures = pack.join("assets/minecraft/textures/entity");
    let names = ["steve", "alex", "ari", "efe", "kai", "makena", "noor", "sunny", "zuri"];
    for dir in ["player/wide", "player/slim"] {
        fs::create_dir_all(textures.join(dir))?;
        for name in names {
            skin.save(textures.join(dir).join(format!("{name}.png")))?;
        }
    }
    skin.save(textures.join("steve.png"))?;
    skin.save(textures.join("alex.png"))?;
    enable_in_options(&root.join("options.txt"), if modern { "file/ZyntraSkin" } else { PACK_NAME }, modern, forge)
}

/// Horizon reads player skins straight from its prepared assets folder (no resource packs),
/// so the chosen skin replaces those PNGs; the originals are kept as `*.png.orig` and put
/// back when `skin_path` is None.
pub fn apply_to_assets(assets: &Path, skin_path: Option<&Path>) -> Result<()> {
    let skin = skin_path.map(load).transpose()?;
    for sub in ["wide", "slim"] {
        let dir = assets.join("textures/entity/player").join(sub);
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("png") {
                continue;
            }
            let backup = path.with_extension("png.orig");
            match &skin {
                Some(img) => {
                    if !backup.exists() {
                        fs::copy(&path, &backup)?;
                    }
                    img.save(&path)?;
                }
                None if backup.exists() => {
                    fs::copy(&backup, &path)?;
                    fs::remove_file(&backup)?;
                }
                None => {}
            }
        }
    }
    Ok(())
}

/// Adds the pack to `resourcePacks` (and to `incompatibleResourcePacks` so no prompt appears).
/// Selects a resource pack (`file/<name>`) in a modern options.txt.
pub fn enable_pack(options: &Path, entry: &str) -> Result<()> {
    enable_in_options(options, entry, true, false)
}

/// With `forge`, also keeps Forge's own "mod_resources" pack in the list — without it Forge
/// throws away the custom selection and loads only its defaults.
fn enable_in_options(path: &Path, entry: &str, modern: bool, forge: bool) -> Result<()> {
    let text = fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    // The pack must NOT be in incompatibleResourcePacks: when a listed pack turns out to be
    // compatible, Minecraft only drops it from that list and skips loading it that run.
    if let Some(i) = lines.iter().position(|l| l.starts_with("incompatibleResourcePacks:")) {
        let mut list: Vec<String> = serde_json::from_str(&lines[i]["incompatibleResourcePacks:".len()..]).unwrap_or_default();
        list.retain(|e| e != entry);
        lines[i] = format!("incompatibleResourcePacks:{}", serde_json::to_string(&list)?);
    }
    let prefix = "resourcePacks:";
    let pos = lines.iter().position(|l| l.starts_with(prefix));
    let mut list: Vec<String> = pos.and_then(|i| serde_json::from_str(&lines[i][prefix.len()..]).ok()).unwrap_or_default();
    list.retain(|e| e != entry);
    if modern && !list.iter().any(|e| e == "vanilla") {
        list.insert(0, "vanilla".into());
    }
    if forge && !list.iter().any(|e| e == "mod_resources") {
        list.push("mod_resources".into());
    }
    // Last in the list = highest priority, so the skin wins over everything else.
    list.push(entry.to_owned());
    let line = format!("{prefix}{}", serde_json::to_string(&list)?);
    match pos {
        Some(i) => lines[i] = line,
        None => lines.push(line),
    }
    fs::write(path, lines.join("\n") + "\n")?;
    Ok(())
}

/// Front of the head (base + hat layer), for avatars.
pub const HEAD_UV: (f32, f32) = (8.0 / 64.0, 8.0 / 64.0);
pub const HAT_UV: (f32, f32) = (40.0 / 64.0, 8.0 / 64.0);

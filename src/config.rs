//! Launcher settings, stored as `zyntra.json` next to the executable (portable; on macOS in Application Support).

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// What the Play button starts.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum Target {
    /// Official Mojang version (release, snapshot, beta, alpha).
    Vanilla(String),
    /// Latest stable Fabric loader for the given game version.
    Fabric(String),
    /// A version folder that already exists (`tl` = it lives in the TLauncher folder).
    Installed { id: String, tl: bool },
    /// Horizon: Minecraft 26.3 rewritten in Rust, as it comes.
    Horizon,
    /// The same Horizon install, launched with Zyntra Fine (OptiFine-style settings) and zoom.
    HorizonFine,
}

impl Target {
    pub fn is_horizon(&self) -> bool {
        matches!(self, Target::Horizon | Target::HorizonFine)
    }

    pub fn title(&self) -> String {
        match self {
            Target::Vanilla(id) => id.clone(),
            Target::Fabric(game) => format!("Fabric {game}"),
            Target::Installed { id, .. } => id.clone(),
            Target::Horizon => "Horizon 26.3".into(),
            Target::HorizonFine => "Horizon OptiFine 26.3".into(),
        }
    }

    pub fn kind(&self) -> String {
        match self {
            Target::Vanilla(_) => "Vanilla".into(),
            Target::Fabric(_) => "Fabric".into(),
            Target::Installed { id, tl } => {
                let kind = loader_of(id);
                if *tl { format!("{kind} · из TLauncher") } else { kind.into() }
            }
            Target::Horizon => "Horizon · Minecraft на Rust".into(),
            Target::HorizonFine => "Horizon · Zyntra Fine + зум".into(),
        }
    }
}

pub fn loader_of(id: &str) -> &'static str {
    let l = id.to_lowercase();
    if l.contains("fabric") {
        "Fabric"
    } else if l.contains("neoforge") {
        "NeoForge"
    } else if l.contains("forge") {
        "Forge"
    } else if l.contains("optifine") {
        "OptiFine"
    } else if l.contains("quilt") {
        "Quilt"
    } else {
        "Vanilla"
    }
}

/// Zyntra Fine: OptiFine-style video settings, written into Horizon's options.txt and
/// horizon-graphics.txt before every launch of the «Horizon OptiFine» version.
#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Fine {
    // Performance
    pub render_distance: u32,
    pub simulation_distance: u32,
    /// 260 = unlimited, as in vanilla's slider.
    pub max_fps: u32,
    pub vsync: bool,
    /// Terrain sections built per frame; 0 = all at once.
    pub chunk_updates: u32,
    // Quality
    pub smooth_lighting: bool,
    pub mipmaps: u32,
    pub fxaa: bool,
    pub resolution_scale: u32,
    // Details
    /// 0 off, 1 fast, 2 fancy.
    pub clouds: u32,
    pub sky: bool,
    pub fog: bool,
    pub weather: bool,
    pub entity_shadows: bool,
    pub fancy_leaves: bool,
    pub biome_blend: u32,
    // Animations
    pub animated_textures: bool,
    /// 0 all, 1 decreased, 2 minimal.
    pub particles: u32,
    // Culling
    pub entity_culling: bool,
    pub cave_culling: bool,
    pub leaf_culling: bool,
    /// Pack name in shaderpacks/; empty = no shaders.
    pub shader_pack: String,
    /// OptiFine-style zoom while the key is held.
    pub zoom: bool,
    /// One of zoom::KEYS' labels.
    pub zoom_key: String,
}

impl Fine {
    /// For weak PCs: short distance, chunks in small portions, no heavy details.
    pub fn fast() -> Self {
        Self {
            render_distance: 8,
            simulation_distance: 6,
            max_fps: 60,
            vsync: false,
            chunk_updates: 4,
            smooth_lighting: true,
            mipmaps: 0,
            fxaa: false,
            resolution_scale: 100,
            clouds: 0,
            sky: true,
            fog: true,
            weather: true,
            entity_shadows: false,
            fancy_leaves: false,
            biome_blend: 0,
            animated_textures: false,
            particles: 2,
            entity_culling: true,
            cave_culling: true,
            leaf_culling: true,
            shader_pack: String::new(),
            zoom: true,
            zoom_key: "C".into(),
        }
    }

    pub fn balanced() -> Self {
        Self { render_distance: 12, simulation_distance: 8, max_fps: 120, chunk_updates: 8, mipmaps: 2, clouds: 1, entity_shadows: true, fancy_leaves: true, biome_blend: 1, animated_textures: true, particles: 1, ..Self::fast() }
    }

    pub fn quality() -> Self {
        Self { render_distance: 16, simulation_distance: 10, max_fps: 260, vsync: true, chunk_updates: 0, mipmaps: 4, fxaa: true, clouds: 2, biome_blend: 2, particles: 0, leaf_culling: false, ..Self::balanced() }
    }
}

impl Default for Fine {
    fn default() -> Self {
        // Weak PCs (under 12 GB of RAM, usually integrated graphics) start on the fast preset.
        if crate::system::total_ram_mb() < 12 * 1024 { Self::fast() } else { Self::balanced() }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Config {
    pub nickname: String,
    pub ram_mb: u32,
    /// Pick the RAM limit from the PC's memory automatically.
    pub ram_auto: bool,
    pub game_dir: PathBuf,
    /// Empty = use Mojang's Java runtime for each version (downloaded automatically).
    pub java_path: String,
    pub width: u32,
    pub height: u32,
    pub close_on_launch: bool,
    /// Hide the launcher while the game runs and bring it back when the game exits.
    pub hide_on_launch: bool,
    /// Zyntra Fine: Horizon's video settings, OptiFine style.
    pub fine: Fine,
    /// Zyntra Fine: Horizon's built-in extras switched on (key in horizon-extras.txt).
    pub hz_extras: std::collections::BTreeMap<String, bool>,
    /// Brightness (gamma) at its maximum in every version, to see in the dark.
    pub max_gamma: bool,
    pub show_tlauncher: bool,
    pub tlauncher_dir: PathBuf,
    pub target: Option<Target>,
    /// Chosen skin PNG; None = the launcher's own default character.
    pub skin_path: Option<PathBuf>,
    /// Slim (3px) arms.
    pub skin_slim: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            nickname: "Player".into(),
            ram_mb: crate::system::recommended_ram_mb(crate::system::total_ram_mb()),
            ram_auto: true,
            game_dir: default_game_dir(),
            java_path: String::new(),
            width: 1280,
            height: 720,
            close_on_launch: false,
            hide_on_launch: true,
            fine: Fine::default(),
            hz_extras: Default::default(),
            max_gamma: false,
            show_tlauncher: true,
            tlauncher_dir: official_minecraft_dir(),
            target: None,
            skin_path: None,
            skin_slim: false,
        }
    }
}

/// The official launcher's (and TLauncher's) game folder.
pub fn official_minecraft_dir() -> PathBuf {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default().join(".minecraft")
    } else {
        mac_support_dir().join("minecraft")
    }
}

/// ~/Library/Application Support (macOS).
fn mac_support_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join("Library").join("Application Support")
}

fn default_game_dir() -> PathBuf {
    if cfg!(windows) {
        if Path::new("D:\\").exists() {
            PathBuf::from("D:\\Zlauncher\\minecraft")
        } else {
            std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default().join(".zyntra")
        }
    } else {
        mac_support_dir().join("Zyntra Launcher").join("minecraft")
    }
}

/// Windows: next to the executable (portable). macOS: the app bundle is read-only, so the
/// settings live in ~/Library/Application Support/Zyntra Launcher.
fn exe_dir() -> PathBuf {
    if cfg!(windows) {
        std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_default()
    } else {
        let dir = mac_support_dir().join("Zyntra Launcher");
        let _ = fs::create_dir_all(&dir);
        dir
    }
}

fn config_path() -> PathBuf {
    exe_dir().join("zyntra.json")
}

impl Config {
    pub fn load() -> Self {
        // Settings from before the rename (Zlauncher) are picked up once.
        fs::read_to_string(config_path())
            .or_else(|_| fs::read_to_string(exe_dir().join("zlauncher.json")))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = fs::write(config_path(), s);
        }
    }

    /// Folder the game runs in (--gameDir: mods, saves, options) for `target`.
    /// Modded versions installed by Zyntra get their own profile so mods for different
    /// game versions never mix; TLauncher versions keep TLauncher's folder (and its mods).
    pub fn game_dir_for(&self, target: &Target) -> PathBuf {
        let profiles = self.game_dir.join("profiles");
        match target {
            Target::Vanilla(_) | Target::Horizon | Target::HorizonFine => self.game_dir.clone(),
            Target::Fabric(game) => profiles.join(format!("fabric-{game}")),
            Target::Installed { tl: true, .. } => self.tlauncher_dir.clone(),
            Target::Installed { id, tl: false } => match loader_of(id) {
                "Vanilla" => self.game_dir.clone(),
                // fabric-loader-<loader>-<game>: one profile per game version
                "Fabric" => profiles.join(format!("fabric-{}", id.rsplit('-').next().unwrap_or(id))),
                _ => profiles.join(id),
            },
        }
    }

    pub fn nickname_valid(&self) -> bool {
        let n = &self.nickname;
        (3..=16).contains(&n.len()) && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    }
}

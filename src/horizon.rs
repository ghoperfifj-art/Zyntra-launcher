//! Horizon (github.com/horizon-rust/horizon): Minecraft Java Edition 26.3 rewritten in Rust.
//! Horizon ships no Minecraft assets; `horizon-prepare` builds them from the 26.3 files
//! that the launcher installs into its game folder.

use crate::minecraft::{self, Dirs, Manifest};
use crate::net::{self, Job};
use crate::task::Task;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

const RELEASE_API: &str = "https://api.github.com/repos/horizon-rust/horizon/releases/tags/nightly";
/// This platform's build in the nightly release.
pub const ASSET: &str = if cfg!(windows) {
    "horizon-windows-x64.zip"
} else if cfg!(target_arch = "aarch64") {
    "horizon-macos-arm64.zip"
} else {
    "horizon-macos-x64.zip"
};

/// `name` plus ".exe" on Windows.
pub fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}
pub const MC_VERSION: &str = "26.3";

#[derive(Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct Build {
    pub asset_id: u64,
    pub updated_at: String,
    pub url: String,
    pub size: u64,
}

pub fn dir(game_dir: &Path) -> PathBuf {
    game_dir.join("horizon")
}
/// Horizon's game folder (options.txt, horizon-mods/, resourcepacks/, shaderpacks/).
pub fn game_folder(game_dir: &Path) -> PathBuf {
    bin_dir(game_dir)
}
fn bin_dir(game_dir: &Path) -> PathBuf {
    dir(game_dir).join("game")
}
pub fn home_dir(game_dir: &Path) -> PathBuf {
    dir(game_dir).join("home")
}
/// Prepared Minecraft assets that horizon-client reads (textures, sounds, …).
fn assets_dir(game_dir: &Path) -> PathBuf {
    bin_dir(game_dir).join(".local").join("horizon-client").join(MC_VERSION).join("assets").join("minecraft")
}

/// Latest Windows nightly published on GitHub.
pub fn remote() -> Result<Build> {
    let release: Value = net::get_json(RELEASE_API)?;
    let asset = release["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["name"].as_str() == Some(ASSET))
        .context("В релизе Horizon нет сборки для Windows")?;
    Ok(Build {
        asset_id: asset["id"].as_u64().unwrap_or_default(),
        updated_at: asset["updated_at"].as_str().unwrap_or_default().to_owned(),
        url: asset["browser_download_url"].as_str().unwrap_or_default().to_owned(),
        size: asset["size"].as_u64().unwrap_or_default(),
    })
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default()
}

/// True when GitHub hasn't been asked about a newer nightly in the last 6 hours.
/// Keeps Play fast: the update check is a network round-trip.
pub fn checked_recently(game_dir: &Path) -> bool {
    let last: u64 = fs::read_to_string(dir(game_dir).join("last_check")).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    now_secs().saturating_sub(last) < 6 * 3600
}

pub fn mark_checked(game_dir: &Path) {
    let _ = fs::write(dir(game_dir).join("last_check"), now_secs().to_string());
}

/// Resources were prepared for exactly this build, so Play can skip all checks.
fn prepared_for(game_dir: &Path, build: &Build) -> bool {
    bin_dir(game_dir).join(".local").exists()
        && fs::read_to_string(dir(game_dir).join("prepared")).ok().and_then(|s| s.trim().parse::<u64>().ok()) == Some(build.asset_id)
}

fn mark_prepared(game_dir: &Path) {
    if let Some(build) = installed(game_dir) {
        let _ = fs::write(dir(game_dir).join("prepared"), build.asset_id.to_string());
    }
}

pub fn installed(game_dir: &Path) -> Option<Build> {
    if !bin_dir(game_dir).join(exe("horizon-client")).exists() {
        return None;
    }
    let text = fs::read_to_string(dir(game_dir).join("build.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// Downloads and unpacks the latest build (keeps prepared resources and saves).
pub fn install(game_dir: &Path, task: &Task) -> Result<Build> {
    task.stage("Horizon: проверка последней сборки");
    let build = remote()?;
    let root = dir(game_dir);
    fs::create_dir_all(&root)?;
    let zip_path = root.join("download.zip");
    task.stage(format!("Horizon: загрузка {} МБ", build.size / 1_048_576));
    net::download(&Job { url: build.url.clone(), path: zip_path.clone(), sha1: None, size: Some(build.size) }, task)?;

    task.stage("Horizon: распаковка");
    let staging = root.join("game.new");
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    let mut zip = zip::ZipArchive::new(fs::File::open(&zip_path)?)?;
    task.set_total(zip.len() as u64);
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        task.inc();
        let Some(rel) = entry.enclosed_name() else { continue };
        // Strip the top-level "horizon-<platform>/" folder.
        let rel: PathBuf = rel.components().skip(1).collect();
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = staging.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&out)?;
        } else {
            fs::create_dir_all(out.parent().unwrap())?;
            std::io::copy(&mut entry, &mut fs::File::create(&out)?)?;
            // macOS: the game binaries keep their executable bit from the archive.
            #[cfg(unix)]
            if let Some(mode) = entry.unix_mode() {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&out, fs::Permissions::from_mode(mode & 0o777))?;
            }
        }
    }
    let target = bin_dir(game_dir);
    if target.exists() {
        // Keep what lives in the game folder across updates. Everything the new build doesn't
        // bring is the player's (worlds in saves/, servers.dat, screenshots, the player name...)
        // and moves over as it is; of what the build does bring, these stay the player's own:
        // prepared resources (.local, so no new prepare), mods and their switches, settings.
        const OWN: [&str; 11] = [
            ".local",
            "horizon-mods",
            "horizon-native-mods",
            "horizon-mods.txt",
            "options.txt",
            "config",
            "saves",
            "shaderpacks",
            "resourcepacks",
            "horizon-graphics.txt",
            "horizon-extras.txt",
        ];
        for entry in fs::read_dir(&target)?.flatten() {
            let name = entry.file_name();
            let new = staging.join(&name);
            let own = OWN.iter().any(|o| name == *o);
            if new.exists() && !own {
                continue;
            }
            if new.exists() {
                if new.is_dir() { fs::remove_dir_all(&new)? } else { fs::remove_file(&new)? }
            }
            // A world must never be deleted with the old build: stop here instead.
            fs::rename(entry.path(), &new).with_context(|| format!("Не удалось перенести {} в новую версию Horizon: закрой игру и попробуй снова", name.to_string_lossy()))?;
        }
        fs::remove_dir_all(&target).context("Закрой Horizon перед обновлением")?;
    }
    fs::rename(&staging, &target)?;
    fs::remove_file(&zip_path).ok();
    fs::write(root.join("build.json"), serde_json::to_string_pretty(&build)?)?;
    Ok(build)
}

fn run_logged(mut cmd: Command, task: &Task) -> Result<bool> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn()?;
    let out = child.stdout.take().unwrap();
    let err = child.stderr.take().unwrap();
    fn pump(r: impl Read, task: &Task) {
        for line in BufReader::new(r).lines().map_while(Result::ok) {
            if !line.trim().is_empty() {
                task.log(format!("  {line}"));
            }
        }
    }
    std::thread::scope(|s| {
        s.spawn(|| pump(out, task));
        s.spawn(|| pump(err, task));
    });
    Ok(child.wait()?.success())
}

/// Installs Minecraft 26.3 files + server jar + Java 25, then runs `horizon-prepare`.
pub fn prepare(game_dir: &Path, manifest: Option<&Manifest>, java_override: &str, task: &Task) -> Result<()> {
    let bin = bin_dir(game_dir);
    let prepare_exe = bin.join(exe("horizon-prepare"));

    // Fast path: this build was already prepared — nothing to download or verify.
    if installed(game_dir).is_some_and(|b| prepared_for(game_dir, &b)) {
        return Ok(());
    }

    let dirs = Dirs::new(game_dir.to_path_buf(), game_dir);
    let mc = minecraft::install(&dirs, MC_VERSION, manifest, java_override, task)?;
    let server = &mc.v["downloads"]["server"];
    let server_jar = dirs.version_dir(MC_VERSION).join("server.jar");
    net::download_all(
        vec![Job {
            url: server["url"].as_str().context("нет ссылки на server.jar 26.3")?.to_owned(),
            path: server_jar.clone(),
            sha1: server["sha1"].as_str().map(str::to_owned),
            size: server["size"].as_u64(),
        }],
        task,
        "Сервер 26.3 для подготовки",
    )?;

    // Resources go to the build's default location (game/.local), where horizon-client finds them.
    let java_home = mc.java.parent().and_then(Path::parent).map(Path::to_path_buf);
    let prepare_cmd = |args: &[&str]| {
        let mut cmd = Command::new(&prepare_exe);
        cmd.current_dir(&bin).args(args).arg("--minecraft-dir").arg(game_dir);
        if let Some(home) = &java_home {
            cmd.env("JAVA_HOME", home);
        }
        cmd
    };

    // Put the original skins back first so the check sees untouched resources.
    crate::skin::restore_assets(&assets_dir(game_dir)).ok();
    task.stage("Horizon: проверка ресурсов");
    if run_logged(prepare_cmd(&["--check"]), task).unwrap_or(false) {
        mark_prepared(game_dir);
        return Ok(());
    }

    task.stage("Horizon: подготовка ресурсов из Minecraft 26.3 (несколько минут, один раз)");
    let mut cmd = prepare_cmd(&["--prepare"]);
    cmd.arg("--server-jar").arg(&server_jar);
    if let Some(home) = &java_home {
        cmd.arg("--java-home").arg(home);
    }
    if !run_logged(cmd, task)? {
        bail!("horizon-prepare завершился с ошибкой — подробности во вкладке «Консоль»");
    }

    task.stage("Horizon: копирование звуков");
    if !run_logged(prepare_cmd(&["sounds"]), task).unwrap_or(false) {
        task.log("Звуки не подготовлены — игра запустится без звука");
    }
    mark_prepared(game_dir);
    Ok(())
}

pub struct Player<'a> {
    pub name: &'a str,
    pub skin: Option<&'a Path>,
    pub slim: bool,
    /// Join this server right after the game starts.
    pub server: Option<&'a str>,
}

/// Sets `key:value` lines in a Minecraft-style settings file, keeping every other line.
pub fn set_keys(path: &Path, pairs: &[(&str, &str)]) -> Result<()> {
    let text = fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    for (key, value) in pairs {
        let prefix = format!("{key}:");
        let line = format!("{prefix}{value}");
        match lines.iter().position(|l| l.starts_with(&prefix)) {
            Some(i) => lines[i] = line,
            None => lines.push(line),
        }
    }
    fs::write(path, lines.join("\n") + "\n")?;
    Ok(())
}

/// Zyntra Fine: writes the OptiFine-style settings into Horizon's own files. Vanilla's keys go to
/// options.txt, Horizon's to horizon-graphics.txt. Horizon can build and upload terrain in
/// portions (`chunkUpdates` sections and `uploadBudgetKiB` per frame; 0 = everything at once),
/// which keeps frames even instead of freezing while chunks load.
fn apply_fine(bin: &Path, f: &crate::config::Fine) -> Result<()> {
    let b = |v: bool| if v { "true" } else { "false" };
    let clouds = ["\"false\"", "\"fast\"", "\"true\""][f.clouds.min(2) as usize];
    let (rd, sim, fps, mips, blend, particles) =
        (f.render_distance.to_string(), f.simulation_distance.to_string(), f.max_fps.to_string(), f.mipmaps.to_string(), f.biome_blend.to_string(), f.particles.min(2).to_string());
    set_keys(
        &bin.join("options.txt"),
        &[
            ("renderDistance", &rd),
            ("simulationDistance", &sim),
            ("maxFps", &fps),
            ("enableVsync", b(f.vsync)),
            ("ao", b(f.smooth_lighting)),
            ("mipmapLevels", &mips),
            ("renderClouds", clouds),
            ("entityShadows", b(f.entity_shadows)),
            ("cutoutLeaves", b(f.fancy_leaves)),
            ("biomeBlendRadius", &blend),
            ("particles", &particles),
            ("graphicsPreset", "\"custom\""),
        ],
    )?;
    let (updates, budget, scale) = (f.chunk_updates.to_string(), if f.chunk_updates == 0 { "0".to_owned() } else { (f.chunk_updates * 256).to_string() }, f.resolution_scale.clamp(50, 100).to_string());
    set_keys(
        &bin.join("horizon-graphics.txt"),
        &[
            ("preset", "custom"),
            ("chunkUpdates", &updates),
            ("uploadBudgetKiB", &budget),
            ("sky", b(f.sky)),
            ("fog", b(f.fog)),
            ("weather", b(f.weather)),
            ("animatedTextures", b(f.animated_textures)),
            ("fxaa", b(f.fxaa)),
            ("resolutionScale", &scale),
            ("entityCulling", b(f.entity_culling)),
            ("caveCulling", b(f.cave_culling)),
            ("leafCulling", if f.leaf_culling { "solid" } else { "off" }),
            // A pack that isn't in shaderpacks/ (deleted, or lost in an update) is not picked.
            ("pickedShaderPack", if bin.join("shaderpacks").join(&f.shader_pack).exists() { f.shader_pack.as_str() } else { "" }),
        ],
    )
}

/// Shader packs in Horizon's shaderpacks/ folder, as Horizon lists them: its own (a folder with
/// horizon-shaders.toml, or .hshader) and OptiFine/Iris ones (a folder with shaders/, or .zip),
/// which Horizon translates itself. The name is what `pickedShaderPack` saves.
pub fn shader_packs(game_dir: &Path) -> Vec<String> {
    let mut packs: Vec<String> = fs::read_dir(bin_dir(game_dir).join("shaderpacks"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            let p = e.path();
            if p.is_dir() {
                p.join("shaders").is_dir() || p.join("horizon-shaders.toml").exists()
            } else {
                p.extension().is_some_and(|x| x == "hshader" || x == "zip")
            }
        })
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    packs.sort();
    packs
}

/// `fine`: Some for the «Horizon OptiFine» version (Zyntra Fine, its extras and zoom applied),
/// None for plain Horizon, which starts with the game's own settings.
pub fn launch(
    game_dir: &Path,
    player: &Player,
    fine: Option<(&crate::config::Fine, &std::collections::BTreeMap<String, bool>)>,
    task: &Task,
) -> Result<Child> {
    let bin = bin_dir(game_dir);
    let home = home_dir(game_dir);
    fs::create_dir_all(&home)?;
    if let Some((fine, extras)) = fine {
        // Every extra Zyntra Fine manages is written, so switching one off works too.
        let pairs: Vec<(&str, &str)> = crate::hz_mods::EXTRAS
            .iter()
            .map(|(key, _, _, default)| (*key, if extras.get(*key).copied().unwrap_or(*default) { "true" } else { "false" }))
            .collect();
        if let Err(e) = set_keys(&bin.join("horizon-extras.txt"), &pairs) {
            task.log(format!("Не удалось применить удобства Zyntra Fine: {e:#}"));
        }
        match apply_fine(&bin, fine) {
            Ok(()) => task.log(format!(
                "Zyntra Fine: дальность {}, симуляция {}, чанков за кадр: {}",
                fine.render_distance,
                fine.simulation_distance,
                if fine.chunk_updates == 0 { "все сразу".to_owned() } else { fine.chunk_updates.to_string() }
            )),
            Err(e) => task.log(format!("Не удалось применить Zyntra Fine: {e:#}")),
        }
    }
    // Connected glass belongs to Horizon OptiFine; plain Horizon keeps the vanilla look.
    // Xray and connected glass (Horizon OptiFine only) go into the prepared resources.
    let resources = assets_dir(game_dir).join("../..");
    if let Err(e) = crate::hz_mods::apply_own_packs(&bin, &resources, fine.is_some()) {
        task.log(format!("Xray или соединённое стекло не подключены: {e:#}"));
    }
    if let Err(e) = crate::skin::restore_assets(&assets_dir(game_dir)) {
        task.log(format!("Не удалось вернуть текстуры игроков: {e:#}"));
    }
    task.stage("Запуск Horizon");
    let log = fs::File::create(home.join("zyntra.log"))?;
    // «Horizon OptiFine» plays the game built with native mods, when it matches this Horizon.
    let exe = match fine.and_then(|_| crate::native::ready(game_dir)) {
        Some(native) => {
            task.log("Нативные моды: запускается собранная версия Horizon");
            native
        }
        None => bin.join(exe("horizon-client")),
    };
    let mut cmd = Command::new(exe);
    // No HORIZON_MEMORY_LIMIT_MB: Horizon is native and manages its own memory, and newer
    // nightlies refuse to start under a limit sized for Java versions.
    cmd.current_dir(&bin).env("HORIZON_HOME", &home);
    // The multiplayer name: without --name Horizon uses one it made up once and kept,
    // often the same "Player" for everyone, and a server lets only one of them in.
    if !player.name.is_empty() {
        cmd.arg("--name").arg(player.name);
    }
    if let Some(server) = player.server {
        cmd.arg("--server").arg(server);
    }
    // Horizon draws this skin on the offline local player, in worlds and on servers.
    match player.skin.map(|path| crate::skin::load(path).map(|_| path)) {
        Some(Ok(path)) => {
            cmd.env("HORIZON_OFFLINE_SKIN", path);
            cmd.env("HORIZON_OFFLINE_SKIN_MODEL", if player.slim { "slim" } else { "classic" });
        }
        Some(Err(e)) => task.log(format!("Скин не подключён: {e:#}")),
        None => {}
    }
    // Zoom talks to the game over its local control endpoint (localhost + token).
    let endpoint = crate::zoom::endpoint_file(&home);
    let zoom = fine.filter(|(f, _)| f.zoom).map(|(f, _)| f.zoom_key.clone());
    fs::remove_file(&endpoint).ok();
    if zoom.is_some() {
        cmd.arg("--control-socket").arg(&endpoint);
    }
    cmd
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no black console next to the game
    }
    let mut child = cmd.spawn().context("Не удалось запустить horizon-client")?;
    // When Horizon builds its resource pack cache anew (after an update, or with new packs),
    // Windows 11 can refuse it the folder links it has just made in that cache (os error 448)
    // and the game stops while loading. The cache is kept all the same, so a second start works.
    let game_log = home.join("zyntra.log");
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_secs(120) {
        match child.try_wait() {
            Ok(None) => {
                if fs::read_to_string(&game_log).is_ok_and(|t| t.contains("resources loaded in")) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            Ok(Some(status)) => {
                if !status.success() && fs::read_to_string(&game_log).is_ok_and(|t| t.contains("os error 448")) {
                    task.log("Horizon обновил кэш ресурс-паков: запускаю игру ещё раз");
                    child = cmd.spawn().context("Не удалось запустить horizon-client")?;
                }
                break;
            }
            Err(_) => break,
        }
    }
    if let Some(key_label) = zoom {
        let (pid, key, options) = (child.id(), crate::zoom::key_code(&key_label), bin.join("options.txt"));
        std::thread::spawn(move || crate::zoom::run(pid, endpoint, options, key));
        task.log(format!("Зум: зажми {key_label} в игре"));
    }
    Ok(child)
}

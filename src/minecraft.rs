//! Vanilla-compatible installer and launcher: version JSONs (with `inheritsFrom`),
//! libraries, natives, assets, Java runtime and the launch command line.

use crate::java;
use crate::net::{self, Job};
use crate::task::Task;
use anyhow::{bail, Context, Result};
use md5::{Digest, Md5};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

pub const MANIFEST_URL: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const LIBRARIES_URL: &str = "https://libraries.minecraft.net/";
const RESOURCES_URL: &str = "https://resources.download.minecraft.net/";

#[derive(Deserialize, Clone, Default)]
pub struct Manifest {
    pub latest: Latest,
    pub versions: Vec<ManifestEntry>,
}

#[derive(Deserialize, Clone, Default)]
pub struct Latest {
    pub release: String,
}

#[derive(Deserialize, Clone)]
pub struct ManifestEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    #[serde(rename = "releaseTime")]
    pub release_time: String,
}

impl Manifest {
    pub fn fetch() -> Result<Self> {
        net::get_json(MANIFEST_URL)
    }

    pub fn find(&self, id: &str) -> Option<&ManifestEntry> {
        self.versions.iter().find(|v| v.id == id)
    }
}

/// `root` is a .minecraft-style folder; Java runtimes always live in the launcher's own folder.
#[derive(Clone)]
pub struct Dirs {
    pub root: PathBuf,
    pub runtimes: PathBuf,
}

impl Dirs {
    pub fn new(root: PathBuf, game_dir: &Path) -> Self {
        Self { root, runtimes: game_dir.join("runtime") }
    }
    pub fn version_dir(&self, id: &str) -> PathBuf {
        self.root.join("versions").join(id)
    }
    pub fn version_json(&self, id: &str) -> PathBuf {
        self.version_dir(id).join(format!("{id}.json"))
    }
    pub fn libraries(&self) -> PathBuf {
        self.root.join("libraries")
    }
    pub fn assets(&self) -> PathBuf {
        self.root.join("assets")
    }
}

/// Version folders under `root/versions` that contain a matching JSON.
pub fn list_installed(root: &Path) -> Vec<String> {
    let mut out: Vec<(std::time::SystemTime, String)> = fs::read_dir(root.join("versions"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let id = e.file_name().to_string_lossy().into_owned();
            let json = e.path().join(format!("{id}.json"));
            let time = fs::metadata(&json).and_then(|m| m.modified()).ok()?;
            Some((time, id))
        })
        .collect();
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out.into_iter().map(|(_, id)| id).collect()
}

pub struct Features {
    pub custom_resolution: bool,
}

/// Mojang's name for this OS in rules and `natives`.
pub const OS_NAME: &str = if cfg!(windows) { "windows" } else { "osx" };

/// Evaluates Mojang library/argument rules for this PC (Windows x64 or macOS).
pub fn allowed(rules: Option<&Value>, features: &Features) -> bool {
    let Some(rules) = rules.and_then(Value::as_array) else { return true };
    // TLauncher writes `"rules": []` for "always" — treat an empty list as no rules.
    if rules.is_empty() {
        return true;
    }
    let mut allow = false;
    for rule in rules {
        let mut matches = true;
        if let Some(os) = rule.get("os") {
            if os["name"].as_str().is_some_and(|n| n != OS_NAME) {
                matches = false;
            }
            if os["arch"].as_str().is_some_and(|a| a != "x86_64" && a != "x64" && !(cfg!(target_arch = "aarch64") && a == "arm64")) {
                matches = false;
            }
        }
        if let Some(feats) = rule.get("features").and_then(Value::as_object) {
            for (name, want) in feats {
                let have = name == "has_custom_resolution" && features.custom_resolution;
                if want.as_bool() != Some(have) {
                    matches = false;
                }
            }
        }
        if matches {
            allow = rule["action"].as_str() == Some("allow");
        }
    }
    allow
}

/// "group:artifact:version[:classifier][@ext]" → relative Maven path.
pub fn maven_path(name: &str) -> PathBuf {
    let (name, ext) = name.split_once('@').unwrap_or((name, "jar"));
    let p: Vec<&str> = name.split(':').collect();
    if p.len() < 3 {
        return PathBuf::from(name);
    }
    let file = match p.get(3) {
        Some(cls) => format!("{}-{}-{}.{ext}", p[1], p[2], cls),
        None => format!("{}-{}.{ext}", p[1], p[2]),
    };
    let mut path = PathBuf::new();
    for part in p[0].split('.') {
        path.push(part);
    }
    path.join(p[1]).join(p[2]).join(file)
}

fn load_json(dirs: &Dirs, id: &str, manifest: Option<&Manifest>, task: &Task) -> Result<Value> {
    let path = dirs.version_json(id);
    if !path.exists() {
        let m = manifest.context("Список версий Mojang ещё не загружен — проверь интернет")?;
        let entry = m.find(id).with_context(|| format!("Версия {id} не найдена"))?;
        task.stage(format!("Загрузка описания {id}"));
        net::download(&Job::new(entry.url.clone(), path.clone()), task)?;
    }
    let text = fs::read_to_string(&path)?;
    serde_json::from_str(&text).with_context(|| format!("Повреждён файл {}", path.display()))
}

/// Loads a version and merges every parent from `inheritsFrom`.
pub fn resolve(dirs: &Dirs, id: &str, manifest: Option<&Manifest>, task: &Task) -> Result<Value> {
    let mut v = load_json(dirs, id, manifest, task)?;
    if let Some(parent_id) = v["inheritsFrom"].as_str().map(str::to_owned) {
        let parent = resolve(dirs, &parent_id, manifest, task)?;
        v = merge(parent, v);
    } else if v.get("jar").is_none() {
        v["jar"] = json!(id);
    }
    Ok(v)
}

fn merge(parent: Value, child: Value) -> Value {
    let mut out = parent;
    let (Some(o), Value::Object(c)) = (out.as_object_mut(), child) else { return out };
    for (key, val) in c {
        match key.as_str() {
            "libraries" => {
                let mut libs = val.as_array().cloned().unwrap_or_default();
                libs.extend(o.get("libraries").and_then(Value::as_array).cloned().unwrap_or_default());
                o.insert(key, Value::Array(libs));
            }
            "arguments" => {
                let mut args = o.get("arguments").cloned().unwrap_or_else(|| json!({}));
                for side in ["game", "jvm"] {
                    if let Some(extra) = val.get(side).and_then(Value::as_array) {
                        let mut list = args[side].as_array().cloned().unwrap_or_default();
                        list.extend(extra.iter().cloned());
                        args[side] = Value::Array(list);
                    }
                }
                o.insert(key, args);
            }
            "inheritsFrom" => {}
            _ => {
                o.insert(key, val);
            }
        }
    }
    out
}

struct Libraries {
    classpath: Vec<PathBuf>,
    natives: Vec<PathBuf>,
    jobs: Vec<Job>,
}

fn artifact_job(dirs: &Dirs, art: &Value, fallback: &Path) -> (PathBuf, Option<Job>) {
    // TLauncher's paths start with "libraries/"; Mojang's are relative to the libraries folder.
    let rel = art["path"]
        .as_str()
        .map(|p| PathBuf::from(p.strip_prefix("libraries/").unwrap_or(p)))
        .unwrap_or_else(|| fallback.to_path_buf());
    let path = dirs.libraries().join(rel);
    let url = art["url"].as_str().unwrap_or_default();
    let job = (!url.is_empty()).then(|| Job {
        url: url.to_owned(),
        path: path.clone(),
        sha1: art["sha1"].as_str().map(str::to_owned),
        size: art["size"].as_u64(),
    });
    (path, job)
}

fn collect_libraries(v: &Value, dirs: &Dirs, features: &Features) -> Libraries {
    let mut libs = Libraries { classpath: vec![], natives: vec![], jobs: vec![] };
    let mut seen = HashSet::new();
    for lib in v["libraries"].as_array().into_iter().flatten() {
        if !allowed(lib.get("rules"), features) {
            continue;
        }
        let name = lib["name"].as_str().unwrap_or_default();
        let parts: Vec<&str> = name.split(':').collect();
        let key = match parts.len() {
            n if n >= 4 => format!("{}:{}:{}", parts[0], parts[1], parts[3]),
            n if n >= 2 => format!("{}:{}", parts[0], parts[1]),
            _ => name.to_owned(),
        };
        let has_downloads = lib.get("downloads").is_some();
        let native = lib["natives"][OS_NAME].as_str().map(|c| c.replace("${arch}", "64"));

        // TLauncher puts the artifact at the top level instead of under "downloads".
        let main = if let Some(art) = lib.pointer("/downloads/artifact").or_else(|| lib.get("artifact")) {
            Some(artifact_job(dirs, art, &maven_path(name)))
        } else if !has_downloads && native.is_none() {
            let rel = maven_path(name);
            let base = lib["url"].as_str().unwrap_or(LIBRARIES_URL);
            let url = format!("{}/{}", base.trim_end_matches('/'), rel.to_string_lossy().replace('\\', "/"));
            let path = dirs.libraries().join(&rel);
            let job = Job { url, path: path.clone(), sha1: lib["sha1"].as_str().map(str::to_owned), size: lib["size"].as_u64() };
            Some((path, Some(job)))
        } else {
            None
        };
        if let Some((path, job)) = main {
            if seen.insert(key) {
                libs.classpath.push(path);
                libs.jobs.extend(job);
            }
        }

        if let Some(cls) = native {
            let found = lib["downloads"]["classifiers"].get(&cls).map(|art| {
                artifact_job(dirs, art, &maven_path(&format!("{name}:{cls}")))
            });
            let found = found.or_else(|| {
                (!has_downloads).then(|| {
                    let rel = maven_path(&format!("{name}:{cls}"));
                    let base = lib["url"].as_str().unwrap_or(LIBRARIES_URL);
                    let url = format!("{}/{}", base.trim_end_matches('/'), rel.to_string_lossy().replace('\\', "/"));
                    let path = dirs.libraries().join(rel);
                    (path.clone(), Some(Job::new(url, path)))
                })
            });
            if let Some((path, job)) = found {
                libs.natives.push(path);
                libs.jobs.extend(job);
            }
        }
    }
    libs
}

fn extract_natives(jars: &[PathBuf], dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)?;
    for jar in jars {
        let file = fs::File::open(jar).with_context(|| format!("нет файла {}", jar.display()))?;
        let mut zip = zip::ZipArchive::new(file)?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            let Some(rel) = entry.enclosed_name() else { continue };
            if entry.is_dir() || rel.starts_with("META-INF") {
                continue;
            }
            let out = dir.join(rel);
            if out.exists() {
                continue;
            }
            if let Some(p) = out.parent() {
                fs::create_dir_all(p)?;
            }
            std::io::copy(&mut entry, &mut fs::File::create(&out)?)?;
        }
    }
    Ok(())
}

/// Everything needed to start a version after installation.
pub struct Prepared {
    pub id: String,
    pub v: Value,
    pub java: PathBuf,
    pub classpath: Vec<PathBuf>,
    pub natives_dir: PathBuf,
    pub game_assets: PathBuf,
    pub dirs: Dirs,
}

/// Downloads whatever is missing for version `id` and returns the launch data.
pub fn install(dirs: &Dirs, id: &str, manifest: Option<&Manifest>, java_override: &str, task: &Task) -> Result<Prepared> {
    let v = resolve(dirs, id, manifest, task)?;
    let features = Features { custom_resolution: true };
    let mut jobs = Vec::new();

    // Client jar
    let jar_id = v["jar"].as_str().unwrap_or(id).to_owned();
    let client_jar = dirs.version_dir(&jar_id).join(format!("{jar_id}.jar"));
    let client = &v["downloads"]["client"];
    if let Some(url) = client["url"].as_str() {
        jobs.push(Job {
            url: url.to_owned(),
            path: client_jar.clone(),
            sha1: client["sha1"].as_str().map(str::to_owned),
            size: client["size"].as_u64(),
        });
    }

    let libs = collect_libraries(&v, dirs, &features);
    jobs.extend(libs.jobs);
    net::download_all(jobs, task, "Игра и библиотеки")?;
    if !client_jar.exists() {
        bail!("Нет файла игры {}", client_jar.display());
    }

    // Assets
    let index_id = v["assetIndex"]["id"].as_str().or(v["assets"].as_str()).unwrap_or("legacy").to_owned();
    let index_path = dirs.assets().join("indexes").join(format!("{index_id}.json"));
    if let Some(url) = v["assetIndex"]["url"].as_str() {
        let job = Job {
            url: url.to_owned(),
            path: index_path.clone(),
            sha1: v["assetIndex"]["sha1"].as_str().map(str::to_owned),
            size: v["assetIndex"]["size"].as_u64(),
        };
        if net::needs(&job) {
            task.stage("Загрузка индекса ресурсов");
            net::download(&job, task)?;
        }
    }
    let mut game_assets = dirs.assets().join("virtual").join(&index_id);
    if let Ok(text) = fs::read_to_string(&index_path) {
        let index: Value = serde_json::from_str(&text)?;
        let objects = index["objects"].as_object().cloned().unwrap_or_default();
        let object_path = |hash: &str| dirs.assets().join("objects").join(&hash[..2]).join(hash);
        let asset_jobs = objects
            .values()
            .filter_map(|o| {
                let hash = o["hash"].as_str()?;
                Some(Job {
                    url: format!("{RESOURCES_URL}{}/{hash}", &hash[..2]),
                    path: object_path(hash),
                    sha1: Some(hash.to_owned()),
                    size: o["size"].as_u64(),
                })
            })
            .collect();
        net::download_all(asset_jobs, task, "Ресурсы (звуки, языки)")?;

        let virtual_ = index["virtual"].as_bool() == Some(true);
        let to_resources = index["map_to_resources"].as_bool() == Some(true);
        if to_resources {
            game_assets = dirs.root.join("resources");
        }
        if virtual_ || to_resources {
            task.stage("Подготовка старых ресурсов");
            for (name, o) in &objects {
                let Some(hash) = o["hash"].as_str() else { continue };
                let dst = game_assets.join(name);
                if !dst.exists() {
                    if let Some(p) = dst.parent() {
                        fs::create_dir_all(p)?;
                    }
                    fs::copy(object_path(hash), &dst)?;
                }
            }
        }
    }

    // Natives (old versions ship them as separate classifier jars)
    let natives_dir = dirs.version_dir(id).join("natives");
    if !libs.natives.is_empty() {
        task.stage("Распаковка нативных библиотек");
        extract_natives(&libs.natives, &natives_dir)?;
    }
    fs::create_dir_all(&natives_dir)?;

    // Java
    let java = if !java_override.trim().is_empty() {
        PathBuf::from(java_override.trim())
    } else {
        let component = v["javaVersion"]["component"].as_str().unwrap_or("jre-legacy");
        match java::ensure(&dirs.runtimes, component, has_arm64_natives(&v), task) {
            Ok(path) => path,
            Err(e) => {
                task.log(format!("Java от Mojang недоступна ({e:#}), пробую системную Java"));
                PathBuf::from(if cfg!(windows) { "javaw" } else { "java" })
            }
        }
    };

    let mut classpath = libs.classpath;
    classpath.push(client_jar);
    Ok(Prepared { id: id.to_owned(), v, java, classpath, natives_dir, game_assets, dirs: dirs.clone() })
}

/// Whether the version ships natives for Apple Silicon (1.19 and newer do). Older ones only
/// have Intel natives, so on an M1 they run with an Intel Java under Rosetta.
fn has_arm64_natives(v: &Value) -> bool {
    v["libraries"].as_array().into_iter().flatten().any(|l| l["name"].as_str().is_some_and(|n| n.contains("natives-macos-arm64")))
}

pub fn offline_uuid(name: &str) -> String {
    let mut hash = Md5::digest(format!("OfflinePlayer:{name}").as_bytes());
    hash[6] = (hash[6] & 0x0f) | 0x30;
    hash[8] = (hash[8] & 0x3f) | 0x80;
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

pub struct LaunchOptions {
    /// --gameDir: saves, options, mods, logs. Modded versions get their own folder.
    pub game_dir: PathBuf,
    pub nickname: String,
    pub ram_mb: u32,
    pub width: u32,
    pub height: u32,
    /// Join this server (HOST:PORT) right after the game starts.
    pub server: Option<String>,
}

fn collect_args(list: &Value, features: &Features, out: &mut Vec<String>) {
    for arg in list.as_array().into_iter().flatten() {
        match arg {
            Value::String(s) => out.push(s.clone()),
            // Mojang uses "value", TLauncher's own JSONs use "values".
            Value::Object(_) if allowed(arg.get("rules"), features) => match arg.get("value").unwrap_or(&arg["values"]) {
                Value::String(s) => out.push(s.clone()),
                Value::Array(items) => out.extend(items.iter().filter_map(|x| x.as_str().map(str::to_owned))),
                _ => {}
            },
            _ => {}
        }
    }
}

fn substitute(arg: &str, vars: &HashMap<&str, String>) -> String {
    let mut out = arg.to_owned();
    while let Some(start) = out.find("${") {
        let Some(len) = out[start..].find('}') else { break };
        let key = &out[start + 2..start + len];
        let value = vars.get(key).cloned().unwrap_or_default();
        out.replace_range(start..start + len + 1, &value);
    }
    out
}

/// Builds the command line and starts the game. Output goes to `logs/zyntra-game.log`.
pub fn launch(p: &Prepared, opts: &LaunchOptions, task: &Task) -> Result<Child> {
    task.stage(format!("Запуск {}", p.id));
    let features = Features { custom_resolution: true };
    let sep = if cfg!(windows) { ";" } else { ":" };
    let classpath = p.classpath.iter().map(|c| c.to_string_lossy().into_owned()).collect::<Vec<_>>().join(sep);
    let index_id = p.v["assetIndex"]["id"].as_str().or(p.v["assets"].as_str()).unwrap_or("legacy");

    let vars: HashMap<&str, String> = HashMap::from([
        ("auth_player_name", opts.nickname.clone()),
        ("version_name", p.id.clone()),
        ("game_directory", opts.game_dir.to_string_lossy().into_owned()),
        ("assets_root", p.dirs.assets().to_string_lossy().into_owned()),
        ("game_assets", p.game_assets.to_string_lossy().into_owned()),
        ("assets_index_name", index_id.to_owned()),
        ("auth_uuid", offline_uuid(&opts.nickname)),
        ("auth_access_token", "0".into()),
        ("auth_session", "0".into()),
        ("clientid", String::new()),
        ("auth_xuid", String::new()),
        ("user_type", "legacy".into()),
        ("user_properties", "{}".into()),
        ("version_type", "Zyntra".into()),
        ("natives_directory", p.natives_dir.to_string_lossy().into_owned()),
        ("launcher_name", "Zyntra".into()),
        ("launcher_version", env!("CARGO_PKG_VERSION").into()),
        ("classpath", classpath),
        ("classpath_separator", sep.into()),
        ("library_directory", p.dirs.libraries().to_string_lossy().into_owned()),
        ("resolution_width", opts.width.to_string()),
        ("resolution_height", opts.height.to_string()),
    ]);

    let mut raw = vec![
        format!("-Xmx{}M", opts.ram_mb),
        "-Xms512M".into(),
        "-Dlog4j2.formatMsgNoLookups=true".into(),
        // 1.16.4–1.19 ask Microsoft's services whether multiplayer is allowed, and an answer
        // for a player with no account can switch it off. A services address that can't be
        // reached makes the game treat the player as offline, with multiplayer on, as in TLauncher.
        // The other hosts are Mojang's own: authlib takes custom hosts only all four together.
        "-Dminecraft.api.auth.host=https://authserver.mojang.com".into(),
        "-Dminecraft.api.account.host=https://api.mojang.com".into(),
        "-Dminecraft.api.session.host=https://sessionserver.mojang.com".into(),
        "-Dminecraft.api.services.host=http://127.0.0.1:9".into(),
    ];
    if p.v["arguments"]["jvm"].is_array() {
        collect_args(&p.v["arguments"]["jvm"], &features, &mut raw);
    } else {
        raw.extend(["-Djava.library.path=${natives_directory}".into(), "-cp".into(), "${classpath}".into()]);
    }
    raw.push(p.v["mainClass"].as_str().context("в описании версии нет mainClass")?.to_owned());
    if p.v["arguments"]["game"].is_array() {
        collect_args(&p.v["arguments"]["game"], &features, &mut raw);
    } else if let Some(legacy) = p.v["minecraftArguments"].as_str() {
        raw.extend(legacy.split_whitespace().map(str::to_owned));
        raw.extend(["--width".into(), "${resolution_width}".into(), "--height".into(), "${resolution_height}".into()]);
    }
    // Without a Microsoft account newer versions ask Microsoft whether this player may play
    // online, get a refusal and switch multiplayer off. This switch skips that check.
    if p.v["arguments"]["game"].is_array() {
        raw.push("--offlineDeveloperMode".into());
    }
    if let Some(server) = &opts.server {
        // Versions with Quick Play ignore the old --server; the oldest ones know only that.
        if p.v["arguments"]["game"].to_string().contains("is_quick_play_multiplayer") {
            raw.extend(["--quickPlayMultiplayer".into(), server.clone()]);
        } else if let Some((host, port)) = server.rsplit_once(':') {
            raw.extend(["--server".into(), host.to_owned(), "--port".into(), port.to_owned()]);
        }
    }
    let args: Vec<String> = raw.iter().map(|a| substitute(a, &vars)).collect();

    fs::create_dir_all(&opts.game_dir)?;
    let logs = opts.game_dir.join("logs");
    fs::create_dir_all(&logs)?;
    let log = fs::File::create(logs.join("zyntra-game.log"))?;
    let mut cmd = Command::new(&p.java);
    cmd.args(&args)
        .current_dir(&opts.game_dir)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    task.log(format!("Java: {}", p.java.display()));
    cmd.spawn().with_context(|| format!("Не удалось запустить Java ({})", p.java.display()))
}

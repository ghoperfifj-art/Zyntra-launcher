//! Native Horizon mods (`.hnmod`: Rust that hooks into the game's own code). As Horizon's own
//! launcher does, they are applied to Horizon's source with horizon-mixins (MIT) and the game is
//! compiled with them; the result is `horizon-client-native.exe` beside the official game, sharing
//! its resources, mods and settings. «Horizon OptiFine» plays it; «Horizon 26.3» stays official.

use crate::horizon;
use crate::net::{self, Job};
use crate::task::Task;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const RELEASE_API: &str = "https://api.github.com/repos/horizon-rust/horizon/releases/tags/nightly";
const MIXINS_GIT: &str = "https://github.com/horizon-rust/horizon-mixins";
pub const EXE: &str = if cfg!(windows) { "horizon-client-native.exe" } else { "horizon-client-native" };
/// Where the player's `.hnmod` files live (kept across Horizon updates).
pub const MODS_DIR: &str = "horizon-native-mods";

/// Build workspace: source, tool, staged source, compiled crates (kept for faster rebuilds).
fn work(game_dir: &Path) -> PathBuf {
    horizon::dir(game_dir).join("native")
}

/// What the current native game was built from.
#[derive(Serialize, Deserialize, Default, PartialEq, Clone)]
pub struct Built {
    pub asset_id: u64,
    pub revision: String,
    pub mods: Vec<String>,
}

fn built_path(game_dir: &Path) -> PathBuf {
    work(game_dir).join("built.json")
}

/// The `.hnmod` files selected for the build, by file name.
pub fn selected(game_dir: &Path) -> Vec<String> {
    let mut list: Vec<String> = fs::read_dir(horizon::game_folder(game_dir).join(MODS_DIR))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".hnmod"))
        .collect();
    list.sort();
    list
}

/// The native game, when it was built for the installed Horizon and the selected mods.
pub fn ready(game_dir: &Path) -> Option<PathBuf> {
    let exe = horizon::game_folder(game_dir).join(EXE);
    let built: Built = serde_json::from_str(&fs::read_to_string(built_path(game_dir)).ok()?).ok()?;
    let installed = horizon::installed(game_dir)?;
    (exe.exists() && built.asset_id == installed.asset_id && built.mods == selected(game_dir) && !built.mods.is_empty()).then_some(exe)
}

/// Downloads a registry `.hnmod` (checked against its SHA-256) into the native mods folder.
pub fn add_mod(game_dir: &Path, project: &crate::hz_mods::Project, task: &Task) -> Result<()> {
    let version = &project.versions[0];
    let dir = horizon::game_folder(game_dir).join(MODS_DIR);
    fs::create_dir_all(&dir)?;
    // One version of a mod at a time.
    for old in selected(game_dir).into_iter().filter(|n| n.starts_with(&format!("{}-", project.id))) {
        fs::remove_file(dir.join(old)).ok();
    }
    let path = dir.join(&version.file.name);
    task.stage(format!("Нативный мод {} {}", project.name, version.version));
    net::download(&Job { url: version.file.url.clone(), path: path.clone(), sha1: None, size: Some(version.file.size) }, task)?;
    if !crate::hz_mods::sha256_of(&path)?.eq_ignore_ascii_case(&version.file.sha256) {
        fs::remove_file(&path).ok();
        bail!("{}: контрольная сумма не совпала с каталогом Horizon", version.file.name);
    }
    Ok(())
}

/// Minimap's corner, kept in its settings folder as the mod reads it (default top right).
fn minimap_corner_file(game_dir: &Path) -> PathBuf {
    horizon::game_folder(game_dir).join("horizon-mods").join(".horizon-mod-data").join("minimap").join("corner")
}

pub fn minimap_corner(game_dir: &Path) -> String {
    fs::read_to_string(minimap_corner_file(game_dir)).map(|s| s.trim().to_owned()).unwrap_or_else(|_| "top_right".into())
}

pub fn set_minimap_corner(game_dir: &Path, corner: &str) {
    let path = minimap_corner_file(game_dir);
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(path, corner);
}

pub fn remove_mod(game_dir: &Path, file: &str) {
    fs::remove_file(horizon::game_folder(game_dir).join(MODS_DIR).join(file)).ok();
}

fn cargo() -> PathBuf {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default();
    let path = home.join(".cargo").join("bin").join(horizon::exe("cargo"));
    if path.exists() { path } else { PathBuf::from("cargo") }
}

/// A command that runs hidden and at below-normal priority, so the PC stays usable.
fn quiet(program: &Path) -> Command {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000 | 0x0000_4000); // CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS
    }
    cmd
}

/// Runs `cmd`, sending its output lines to `on_line`; fails with the last lines on error.
fn run(mut cmd: Command, what: &str, task: &Task, mut on_line: impl FnMut(&str)) -> Result<()> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
    let mut child = cmd.spawn().with_context(|| format!("{what}: не удалось запустить"))?;
    let (out, err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let pump = |r: Box<dyn Read + Send>, tx: std::sync::mpsc::Sender<String>| {
        std::thread::spawn(move || {
            // Cargo's progress bar redraws with \r: split on it too.
            let mut buf = Vec::new();
            let mut reader = BufReader::new(r);
            let mut byte = [0u8; 1];
            while reader.read(&mut byte).map(|n| n == 1).unwrap_or(false) {
                if byte[0] == b'\n' || byte[0] == b'\r' {
                    if !buf.is_empty() {
                        let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
                        buf.clear();
                    }
                } else {
                    buf.push(byte[0]);
                }
            }
        })
    };
    let a = pump(Box::new(out), tx.clone());
    let b = pump(Box::new(err), tx);
    let mut tail: Vec<String> = Vec::new();
    for line in rx {
        let clean = strip_ansi(&line);
        on_line(&clean);
        if clean.starts_with("error") || clean.contains("E_") {
            task.log(format!("  {clean}"));
        }
        tail.push(clean);
        if tail.len() > 30 {
            tail.remove(0);
        }
    }
    let _ = (a.join(), b.join());
    let status = child.wait()?;
    if !status.success() {
        bail!("{what} завершилось с ошибкой:\n{}", tail.join("\n"));
    }
    Ok(())
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            while let Some(&n) = chars.peek() {
                chars.next();
                if n.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out.trim().to_owned()
}

/// Makes `to` equal to `from`, rewriting only files whose bytes changed, so Cargo's incremental
/// build recompiles only what the mods changed.
fn sync(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    let mut keep = std::collections::HashSet::new();
    for entry in fs::read_dir(from)?.flatten() {
        let name = entry.file_name();
        keep.insert(name.clone());
        let (src, dst) = (entry.path(), to.join(&name));
        if src.is_dir() {
            if dst.is_file() {
                fs::remove_file(&dst)?;
            }
            sync(&src, &dst)?;
        } else {
            let new = fs::read(&src)?;
            if fs::read(&dst).ok().as_deref() != Some(new.as_slice()) {
                if dst.is_dir() {
                    fs::remove_dir_all(&dst)?;
                }
                fs::write(&dst, new)?;
            }
        }
    }
    for entry in fs::read_dir(to)?.flatten() {
        if !keep.contains(&entry.file_name()) {
            let p = entry.path();
            if p.is_dir() { fs::remove_dir_all(p)? } else { fs::remove_file(p)? }
        }
    }
    Ok(())
}

/// Inner crate attributes horizon-mixins doesn't accept but the build needs.
const CRATE_ATTRS: [&str; 2] = ["#![recursion_limit", "#![type_length_limit"];

/// A crate root's lines that were taken out: (file relative to the workspace, the lines).
type Hidden = Vec<(PathBuf, Vec<String>)>;

/// Removes [`CRATE_ATTRS`] lines from every crate root (`crates/*/src/{lib,main}.rs`) of `source`.
fn hide_crate_attrs(source: &Path) -> Result<Hidden> {
    let mut hidden = Vec::new();
    for krate in fs::read_dir(source.join("crates"))?.flatten() {
        for root in ["lib.rs", "main.rs"] {
            let rel = PathBuf::from("crates").join(krate.file_name()).join("src").join(root);
            let Ok(text) = fs::read_to_string(source.join(&rel)) else { continue };
            let (taken, kept): (Vec<&str>, Vec<&str>) = text.lines().partition(|l| CRATE_ATTRS.iter().any(|a| l.trim_start().starts_with(a)));
            if !taken.is_empty() {
                fs::write(source.join(&rel), kept.join("\n") + "\n")?;
                hidden.push((rel, taken.into_iter().map(str::to_owned).collect()));
            }
        }
    }
    Ok(hidden)
}

/// Puts the hidden lines back, after the file's leading `//!` docs: into `source` itself
/// (`into` None), or into the applied workspace `into`.
fn restore_crate_attrs(source: &Path, hidden: &Hidden, into: Option<&Path>) -> Result<()> {
    let base = into.unwrap_or(source);
    for (rel, lines) in hidden {
        let path = base.join(rel);
        let Ok(text) = fs::read_to_string(&path) else { continue };
        let mut all: Vec<String> = text.lines().map(str::to_owned).collect();
        let at = all.iter().position(|l| !l.trim_start().starts_with("//!")).unwrap_or(all.len());
        for line in lines.iter().rev() {
            if !all.iter().any(|l| l == line) {
                all.insert(at, line.clone());
            }
        }
        fs::write(&path, all.join("\n") + "\n")?;
    }
    Ok(())
}

/// Builds Horizon with the selected native mods. Long the first time (the whole game compiles),
/// much shorter after (only what changed).
pub fn build(game_dir: &Path, task: &Task) -> Result<()> {
    let installed = horizon::installed(game_dir).context("Сначала установи Horizon: выбери «Horizon 26.3» и нажми «Играть»")?;
    let mods = selected(game_dir);
    if mods.is_empty() {
        bail!("Не выбрано ни одного нативного мода");
    }
    let work = work(game_dir);
    fs::create_dir_all(&work)?;

    // 1. The exact revision of the installed nightly, and its source.
    task.stage("Сборка: проверка версии Horizon");
    let release: Value = net::get_json(RELEASE_API)?;
    let asset_of = |name: &str| release["assets"].as_array().into_iter().flatten().find(|a| a["name"].as_str() == Some(name)).cloned();
    let game_asset = asset_of(horizon::ASSET).context("в релизе нет сборки Horizon для этой системы")?;
    if game_asset["id"].as_u64() != Some(installed.asset_id) {
        bail!("Вышла новая сборка Horizon. Запусти «Horizon 26.3», чтобы обновиться, и собери снова.");
    }
    let revision = release["target_commitish"].as_str().context("нет ревизии в релизе")?.to_owned();
    let source = work.join("source").join(&revision[..12]);
    if !source.join(".horizon-revision").exists() {
        let src = asset_of("horizon-source.tar.gz").context("в релизе нет исходников")?;
        task.stage("Сборка: загрузка исходников Horizon");
        let archive = work.join("source.tar.gz");
        net::download(&Job { url: src["browser_download_url"].as_str().unwrap_or_default().to_owned(), path: archive.clone(), sha1: None, size: src["size"].as_u64() }, task)?;
        task.stage("Сборка: распаковка исходников");
        let tmp = work.join("source").join(".unpack");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp)?;
        let mut tar = quiet(Path::new("tar"));
        tar.arg("-xzf").arg(&archive).arg("-C").arg(&tmp);
        run(tar, "Распаковка исходников", task, |_| {})?;
        let top = fs::read_dir(&tmp)?.flatten().next().context("пустой архив исходников")?.path();
        let _ = fs::remove_dir_all(&source);
        fs::rename(&top, &source)?;
        let _ = fs::remove_dir_all(&tmp);
        fs::write(source.join(".horizon-revision"), &revision)?;
        fs::remove_file(&archive).ok();
    }

    // 2. horizon-mixins at the revision this Horizon's own launcher uses.
    let launcher_src = fs::read_to_string(source.join("launcher/src/native_mods.rs")).unwrap_or_default();
    let mixins_rev = launcher_src
        .lines()
        .find_map(|l| l.trim().strip_prefix("pub const MIXINS_REV: &str = \"").and_then(|r| r.strip_suffix("\";")))
        .context("не найдена версия horizon-mixins в исходниках")?
        .to_owned();
    let tool_dir = work.join("tools").join(&mixins_rev[..12]);
    let tool = tool_dir.join("bin").join(horizon::exe("horizon-mixins"));
    if !tool.exists() {
        task.stage("Сборка: инструмент horizon-mixins (один раз, несколько минут)");
        let mut cmd = quiet(&cargo());
        cmd.args(["install", "--locked", "--git", MIXINS_GIT, "--rev", &mixins_rev, "--root"]).arg(&tool_dir).arg("horizon-mixins-cli");
        run(cmd, "Сборка horizon-mixins", task, |_| {})?;
    }

    // 3. The mods' bundles, pinned to this revision (as the official «Build anyway»).
    task.stage("Сборка: подготовка модов");
    let incoming = work.join("mods.incoming");
    let _ = fs::remove_dir_all(&incoming);
    fs::create_dir_all(&incoming)?;
    for file in &mods {
        let path = horizon::game_folder(game_dir).join(MODS_DIR).join(file);
        let mut zip = zip::ZipArchive::new(fs::File::open(&path)?).with_context(|| format!("{file}: не архив"))?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            let Some(rel) = entry.enclosed_name() else { continue };
            let Ok(inside) = rel.strip_prefix("mixins") else { continue };
            let out = incoming.join(inside);
            if entry.is_dir() {
                fs::create_dir_all(&out)?;
            } else {
                fs::create_dir_all(out.parent().unwrap())?;
                std::io::copy(&mut entry, &mut fs::File::create(&out)?)?;
            }
        }
    }
    for entry in fs::read_dir(&incoming)?.flatten() {
        let manifest = entry.path().join("horizon-mixin.toml");
        if let Ok(text) = fs::read_to_string(&manifest) {
            let pinned: Vec<String> = text
                .lines()
                .map(|l| if l.trim_start().starts_with("horizon_revision") { format!("horizon_revision = \"{revision}\"") } else { l.to_owned() })
                .collect();
            fs::write(&manifest, pinned.join("\n") + "\n")?;
        }
    }

    // 4. Apply the mods to a copy of the source.
    task.stage("Сборка: применение модов к исходникам");
    let fresh = work.join("stage.new");
    let _ = fs::remove_dir_all(&fresh);
    // horizon-mixins refuses a crate whose root has inner attributes it doesn't list (newer
    // nightlies' `#![recursion_limit = "256"]`): hide them while it plans, put them back after.
    let hidden = hide_crate_attrs(&source)?;
    let mut apply = quiet(&tool);
    apply.env("CARGO", cargo()).arg("apply").arg("--workspace").arg(&source).arg("--mods").arg(&incoming).arg("--out").arg(&fresh);
    let applied = run(apply, "horizon-mixins", task, |_| {});
    restore_crate_attrs(&source, &hidden, None)?;
    applied?;
    restore_crate_attrs(&source, &hidden, Some(&fresh))?;
    let stage = work.join("stage");
    sync(&fresh, &stage)?;
    let _ = fs::remove_dir_all(&fresh);

    // 5. Compile the game.
    let stage_toml = fs::read_to_string(stage.join("Cargo.toml")).unwrap_or_default();
    let profile = if stage_toml.contains("[profile.fast]") { "fast" } else { "release" };
    task.stage("Сборка: компиляция Horizon (первый раз 30–60 минут, дальше быстрее)");
    let target = work.join("target");
    let mut compile = quiet(&cargo());
    compile
        .env("CARGO_TARGET_DIR", &target)
        .env("CARGO_TERM_PROGRESS_WHEN", "always")
        .env("CARGO_TERM_PROGRESS_WIDTH", "100")
        .arg("build")
        .arg("--manifest-path")
        .arg(stage.join("Cargo.toml"))
        .args(["--locked", "--profile", profile, "-p", "horizon-client", "--bin", "horizon-client"]);
    let mut compiled = 0u64;
    run(compile, "Компиляция Horizon", task, |line| {
        // "Building [=====>   ] 412/803: horizon-world, ..."
        if let Some(rest) = line.strip_prefix("Building") {
            if let Some((done, total)) = rest.split(']').nth(1).and_then(|s| s.trim().split(':').next()).and_then(|s| s.split_once('/')) {
                if let (Ok(d), Ok(t)) = (done.trim().parse::<u64>(), total.trim().parse::<u64>()) {
                    task.set_total(t);
                    while compiled < d {
                        task.inc();
                        compiled += 1;
                    }
                }
            }
        }
    })?;

    // 6. Next to the official game, sharing everything else.
    let exe = target.join(profile).join(horizon::exe("horizon-client"));
    fs::copy(&exe, horizon::game_folder(game_dir).join(EXE)).context("не удалось положить собранную игру")?;
    let built = Built { asset_id: installed.asset_id, revision, mods };
    fs::write(built_path(game_dir), serde_json::to_string_pretty(&built)?)?;
    task.log(format!("Готово: собрано с модами {}", built.mods.join(", ")));
    Ok(())
}

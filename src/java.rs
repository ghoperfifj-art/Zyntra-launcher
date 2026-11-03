//! Mojang's official Java runtimes (the same ones the official launcher uses).

use crate::net::{self, Job};
use crate::task::Task;
use anyhow::{Context, Result};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

const ALL_RUNTIMES: &str = "https://launchermeta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";

/// Mojang's name for this platform in all.json. On Apple Silicon `arm64` picks the native
/// runtime; versions whose natives exist only for Intel run an Intel one under Rosetta.
fn platform(arm64: bool) -> &'static str {
    if cfg!(windows) {
        "windows-x64"
    } else if arm64 && cfg!(target_arch = "aarch64") {
        "mac-os-arm64"
    } else {
        "mac-os"
    }
}

/// The Java executable inside an installed runtime.
fn executable(dir: &Path) -> PathBuf {
    if cfg!(windows) {
        dir.join("bin").join("javaw.exe")
    } else {
        dir.join("jre.bundle").join("Contents").join("Home").join("bin").join("java")
    }
}

/// Ensures runtime `component` is installed under `runtimes` and returns the path to its Java.
pub fn ensure(runtimes: &Path, component: &str, arm64: bool, task: &Task) -> Result<PathBuf> {
    let platform = platform(arm64);
    // Windows keeps the old layout (one runtime per component); on a Mac both an arm64 and
    // an Intel copy of the same component can be needed.
    let dir = if cfg!(windows) { runtimes.join(component) } else { runtimes.join(format!("{component}-{platform}")) };
    let javaw = executable(&dir);
    let marker = dir.join(".zlauncher-ok");
    if javaw.exists() && marker.exists() {
        return Ok(javaw);
    }

    task.stage(format!("Java: поиск {component}"));
    let all: Value = net::get_json(ALL_RUNTIMES)?;
    let entry = all[platform][component]
        .get(0)
        .with_context(|| format!("Mojang не предоставляет Java «{component}» для {platform}"))?;
    let manifest_url = entry["manifest"]["url"].as_str().context("нет ссылки на манифест Java")?;
    let manifest: Value = net::get_json(manifest_url)?;

    let mut jobs = Vec::new();
    let mut executables = Vec::new();
    let mut links = Vec::new();
    for (path, file) in manifest["files"].as_object().into_iter().flatten() {
        let target = dir.join(path);
        if file["executable"].as_bool() == Some(true) {
            executables.push(target.clone());
        }
        match file["type"].as_str() {
            Some("directory") => fs::create_dir_all(&target)?,
            Some("link") => links.push((target, file["target"].as_str().unwrap_or_default().to_owned())),
            Some("file") => {
                let raw = &file["downloads"]["raw"];
                jobs.push(Job {
                    url: raw["url"].as_str().unwrap_or_default().to_owned(),
                    path: target,
                    sha1: raw["sha1"].as_str().map(str::to_owned),
                    size: raw["size"].as_u64(),
                });
            }
            _ => {}
        }
    }
    net::download_all(jobs, task, &format!("Java {component}"))?;
    finish_unix(&executables, &links)?;
    fs::write(&marker, entry["version"]["name"].as_str().unwrap_or("ok"))?;
    Ok(javaw)
}

/// macOS runtimes need their executables marked as such and their symlinks recreated.
#[cfg(unix)]
fn finish_unix(executables: &[PathBuf], links: &[(PathBuf, String)]) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for path in executables {
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    for (link, target) in links {
        if let Some(parent) = link.parent() {
            fs::create_dir_all(parent)?;
        }
        let _ = fs::remove_file(link);
        std::os::unix::fs::symlink(target, link)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn finish_unix(_: &[PathBuf], _: &[(PathBuf, String)]) -> Result<()> {
    Ok(())
}

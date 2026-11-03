//! Fabric loader profiles from meta.fabricmc.net.

use crate::minecraft::Dirs;
use crate::net;
use crate::task::Task;
use anyhow::{Context, Result};
use serde_json::Value;
use std::fs;

const META: &str = "https://meta.fabricmc.net/v2";

/// Game versions supported by Fabric: (version, stable).
pub fn game_versions() -> Result<Vec<(String, bool)>> {
    let list: Vec<Value> = net::get_json(&format!("{META}/versions/game"))?;
    Ok(list
        .iter()
        .filter_map(|v| Some((v["version"].as_str()?.to_owned(), v["stable"].as_bool().unwrap_or(false))))
        .collect())
}

/// Writes the Fabric profile for `game` into the versions folder and returns its version id.
pub fn install(dirs: &Dirs, game: &str, task: &Task) -> Result<String> {
    task.stage(format!("Fabric: поиск загрузчика для {game}"));
    let loaders: Vec<Value> = net::get_json(&format!("{META}/versions/loader/{game}"))?;
    let entry = loaders
        .iter()
        .find(|l| l["loader"]["stable"].as_bool() == Some(true))
        .or(loaders.first())
        .with_context(|| format!("Fabric не поддерживает {game}"))?;
    let loader = entry["loader"]["version"].as_str().context("нет версии загрузчика")?;
    let id = format!("fabric-loader-{loader}-{game}");
    let path = dirs.version_json(&id);
    if !path.exists() {
        let profile = net::get_text(&format!("{META}/versions/loader/{game}/{loader}/profile/json"))?;
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(&path, profile)?;
    }
    Ok(id)
}

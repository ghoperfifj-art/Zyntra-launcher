//! Mods from Modrinth (api.modrinth.com): search, available versions, and installing a mod
//! together with every mod it requires.

use crate::net::{self, Job};
use crate::task::Task;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;

const API: &str = "https://api.modrinth.com/v2";

/// Loaders the launcher can run.
pub const LOADERS: [&str; 3] = ["fabric", "forge", "neoforge"];

#[derive(Deserialize, Clone)]
pub struct Hit {
    pub project_id: String,
    pub title: String,
    pub description: String,
    pub icon_url: Option<String>,
    pub downloads: u64,
    #[serde(default)]
    pub categories: Vec<String>,
}

#[derive(Deserialize)]
struct SearchResponse {
    hits: Vec<Hit>,
}

#[derive(Deserialize, Clone)]
pub struct ModFile {
    pub url: String,
    pub filename: String,
    pub primary: bool,
    pub size: u64,
    pub hashes: Hashes,
}

#[derive(Deserialize, Clone)]
pub struct Hashes {
    pub sha1: Option<String>,
}

#[derive(Deserialize, Clone)]
pub struct Dependency {
    pub project_id: Option<String>,
    pub version_id: Option<String>,
    pub dependency_type: String,
}

#[derive(Deserialize, Clone)]
pub struct Version {
    pub project_id: String,
    pub version_number: String,
    pub game_versions: Vec<String>,
    pub loaders: Vec<String>,
    pub files: Vec<ModFile>,
    pub dependencies: Vec<Dependency>,
}

impl Version {
    pub fn primary_file(&self) -> Option<&ModFile> {
        self.files.iter().find(|f| f.primary).or(self.files.first())
    }
}

/// Mods matching `query` (most downloaded first when the query is empty).
pub fn search(query: &str, loader: Option<&str>) -> Result<Vec<Hit>> {
    let mut facets = vec![vec!["project_type:mod".to_owned()]];
    match loader {
        Some(l) => facets.push(vec![format!("categories:{l}")]),
        None => facets.push(LOADERS.iter().map(|l| format!("categories:{l}")).collect()),
    }
    let index = if query.trim().is_empty() { "downloads" } else { "relevance" };
    let resp: SearchResponse = net::client()
        .get(format!("{API}/search"))
        .query(&[("query", query), ("facets", &serde_json::to_string(&facets)?), ("limit", "30"), ("index", index)])
        .send()?
        .error_for_status()?
        .json()?;
    Ok(resp.hits)
}

/// Shader packs matching `query` (most downloaded first when the query is empty).
pub fn search_shaders(query: &str) -> Result<Vec<Hit>> {
    let facets = vec![vec!["project_type:shader".to_owned()]];
    let index = if query.trim().is_empty() { "downloads" } else { "relevance" };
    let resp: SearchResponse = net::client()
        .get(format!("{API}/search"))
        .query(&[("query", query), ("facets", &serde_json::to_string(&facets)?), ("limit", "30"), ("index", index)])
        .send()?
        .error_for_status()?
        .json()?;
    Ok(resp.hits)
}

/// All versions of a project, newest first.
pub fn versions(project: &str) -> Result<Vec<Version>> {
    net::get_json(&format!("{API}/project/{project}/version"))
}

/// (loader, game version) pairs a project supports, newest first, without duplicates.
pub fn targets(versions: &[Version]) -> Vec<(String, String)> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for v in versions {
        for loader in v.loaders.iter().filter(|l| LOADERS.contains(&l.as_str())) {
            // Releases only: snapshots, pre-releases and release candidates are skipped.
            for game in v.game_versions.iter().filter(|g| g.chars().all(|c| c.is_ascii_digit() || c == '.')) {
                if seen.insert((loader.clone(), game.clone())) {
                    out.push((loader.clone(), game.clone()));
                }
            }
        }
    }
    out
}

/// Newest version of `project` for this game version and loader.
fn pick(project: &str, game: &str, loader: &str) -> Result<Version> {
    let list: Vec<Version> = net::client()
        .get(format!("{API}/project/{project}/version"))
        .query(&[("game_versions", format!("[\"{game}\"]")), ("loaders", format!("[\"{loader}\"]"))])
        .send()?
        .error_for_status()?
        .json()?;
    list.into_iter().next().with_context(|| format!("У мода {project} нет версии для {loader} {game}"))
}

/// Installs the mod and, recursively, every required dependency into `mods_dir`.
/// Returns the file names that were added.
pub fn install(project: &str, game: &str, loader: &str, mods_dir: &Path, task: &Task) -> Result<Vec<String>> {
    task.stage("Моды: подбор версии и зависимостей");
    let mut queue = vec![pick(project, game, loader)?];
    let mut seen: HashSet<String> = HashSet::from([project.to_owned()]);
    let mut jobs = Vec::new();
    let mut names = Vec::new();
    while let Some(version) = queue.pop() {
        let file = version.primary_file().with_context(|| format!("У {} нет файла", version.version_number))?;
        names.push(file.filename.clone());
        task.log(format!("  {} {}", file.filename, if version.project_id == project { "" } else { "(нужен для работы)" }));
        jobs.push(Job {
            url: file.url.clone(),
            path: mods_dir.join(&file.filename),
            sha1: file.hashes.sha1.clone(),
            size: Some(file.size),
        });
        for dep in version.dependencies.iter().filter(|d| d.dependency_type == "required") {
            let dep_version = match (&dep.version_id, &dep.project_id) {
                (Some(id), _) => net::get_json::<Version>(&format!("{API}/version/{id}"))?,
                (None, Some(p)) => pick(p, game, loader)?,
                _ => continue,
            };
            if seen.insert(dep_version.project_id.clone()) {
                queue.push(dep_version);
            }
        }
    }
    std::fs::create_dir_all(mods_dir)?;
    net::download_all(jobs, task, "Моды")?;
    Ok(names)
}

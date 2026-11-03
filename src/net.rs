//! HTTP helpers: JSON fetching and verified, parallel file downloads.

use crate::task::Task;
use anyhow::{bail, Context, Result};
use rayon::prelude::*;
use sha1::{Digest, Sha1};
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub fn client() -> &'static reqwest::blocking::Client {
    static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .user_agent(concat!("ZyntraLauncher/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(900))
            .build()
            .expect("http client")
    })
}

pub fn get_json<T: serde::de::DeserializeOwned>(url: &str) -> Result<T> {
    let resp = client().get(url).send()?.error_for_status()?;
    resp.json().with_context(|| format!("Некорректный ответ от {url}"))
}

pub fn get_text(url: &str) -> Result<String> {
    Ok(client().get(url).send()?.error_for_status()?.text()?)
}

#[derive(Clone, Debug)]
pub struct Job {
    pub url: String,
    pub path: PathBuf,
    pub sha1: Option<String>,
    pub size: Option<u64>,
}

impl Job {
    pub fn new(url: impl Into<String>, path: PathBuf) -> Self {
        Self { url: url.into(), path, sha1: None, size: None }
    }
}

/// Other game folders (TLauncher, the official launcher) whose `assets/` and `libraries/`
/// can be reused instead of downloading the same files again.
static MIRRORS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

pub fn set_mirrors(roots: Vec<PathBuf>) {
    *MIRRORS.lock().unwrap() = roots;
}

fn file_sha1(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let mut hasher = Sha1::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(format!("{:x}", hasher.finalize()))
}

/// Takes the file from another game folder when it is already there with the same size and
/// checksum: a hard link on the same disk (instant, no extra space), otherwise a copy.
/// Returns the number of bytes reused, or None if nothing suitable was found.
fn reuse(job: &Job) -> Option<u64> {
    let comps: Vec<_> = job.path.components().collect();
    let idx = comps.iter().position(|c| c.as_os_str() == "assets" || c.as_os_str() == "libraries")?;
    let rel: PathBuf = comps[idx..].iter().collect();
    let mirrors = MIRRORS.lock().unwrap().clone();
    for root in mirrors {
        let candidate = root.join(&rel);
        if candidate == job.path {
            continue;
        }
        let Ok(meta) = fs::metadata(&candidate) else { continue };
        if job.size.is_some_and(|s| s != meta.len()) {
            continue;
        }
        if let Some(expected) = &job.sha1 {
            if !file_sha1(&candidate).is_some_and(|got| got.eq_ignore_ascii_case(expected)) {
                continue;
            }
        }
        fs::create_dir_all(job.path.parent()?).ok()?;
        if fs::hard_link(&candidate, &job.path).is_err() && fs::copy(&candidate, &job.path).is_err() {
            continue;
        }
        return Some(meta.len());
    }
    None
}

/// A file is considered present when it exists and its size matches (when known).
/// Full SHA-1 checks happen right after each download.
pub fn needs(job: &Job) -> bool {
    match fs::metadata(&job.path) {
        Ok(m) => job.size.is_some_and(|s| m.len() != s),
        Err(_) => true,
    }
}

pub fn download(job: &Job, task: &Task) -> Result<()> {
    let mut last = None;
    // Generous retries: flaky DNS / Wi-Fi drops are common and should not abort an install.
    for attempt in 0..6u64 {
        match try_download(job, task) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(Duration::from_millis(1000 * (attempt + 1)));
            }
        }
    }
    Err(last.unwrap()).with_context(|| format!("Не удалось скачать {}", job.url))
}

fn try_download(job: &Job, task: &Task) -> Result<()> {
    if let Some(parent) = job.path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut resp = client().get(&job.url).send()?.error_for_status()?;
    let tmp = PathBuf::from(format!("{}.part", job.path.display()));
    let mut file = fs::File::create(&tmp)?;
    let mut hasher = Sha1::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        hasher.update(&buf[..n]);
        task.add_bytes(n as u64);
    }
    drop(file);
    if let Some(expected) = &job.sha1 {
        let got = format!("{:x}", hasher.finalize());
        if !got.eq_ignore_ascii_case(expected) {
            fs::remove_file(&tmp).ok();
            bail!("контрольная сумма не совпала ({got} вместо {expected})");
        }
    }
    fs::rename(&tmp, &job.path)?;
    Ok(())
}

/// Downloads every missing file of `jobs` in parallel. Files that fail are retried
/// in a slower sequential pass, so one network hiccup doesn't stop the install.
pub fn download_all(jobs: Vec<Job>, task: &Task, label: &str) -> Result<()> {
    let mut seen = HashSet::new();
    let todo: Vec<Job> = jobs
        .into_iter()
        .filter(|j| !j.url.is_empty() && seen.insert(j.path.clone()) && needs(j))
        .collect();
    if todo.is_empty() {
        return Ok(());
    }
    task.stage(format!("{label}: {} файл(ов)", todo.len()));
    task.set_total(todo.len() as u64);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(8).build()?;
    let (reused_files, reused_bytes) = (AtomicU64::new(0), AtomicU64::new(0));
    let failed: Vec<Job> = pool.install(|| {
        todo.par_iter()
            .filter_map(|job| {
                // Already on this PC (e.g. downloaded by TLauncher)? Take it from there.
                if let Some(bytes) = reuse(job) {
                    reused_files.fetch_add(1, Ordering::Relaxed);
                    reused_bytes.fetch_add(bytes, Ordering::Relaxed);
                    task.inc();
                    return None;
                }
                match download(job, task) {
                    Ok(()) => {
                        task.inc();
                        None
                    }
                    Err(_) => Some(job.clone()),
                }
            })
            .collect()
    });
    let reused = reused_files.load(Ordering::Relaxed);
    if reused > 0 {
        task.log(format!(
            "  Взято с компьютера вместо скачивания: {reused} файл(ов), {} МБ",
            reused_bytes.load(Ordering::Relaxed) / 1_048_576
        ));
    }
    if failed.is_empty() {
        return Ok(());
    }
    task.log(format!("Повторная загрузка {} файл(ов) после сбоя сети…", failed.len()));
    std::thread::sleep(Duration::from_secs(3));
    for job in &failed {
        download(job, task).context("Проверь интернет и нажми «Играть» ещё раз — скачанное сохранится")?;
        task.inc();
    }
    Ok(())
}

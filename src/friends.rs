//! Friends: each launcher posts its status to its own secret ntfy.sh topic (the friend code) and
//! reads its friends' topics. A friend's server is a horizon-server on their computer, reachable
//! from anywhere through the built-in tunnel (tunnel.rs); the address travels in the status.

use crate::horizon;
use crate::minecraft::{self, Dirs, Manifest};
use crate::net::{self, Job};
use crate::task::Task;
use anyhow::{bail, Context, Result};
use eframe::egui;
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const NTFY: &str = "https://ntfy.sh";
/// A status is repeated this often while nothing changes, within ntfy.sh's free daily limit.
const REPEAT: Duration = Duration::from_secs(5 * 60);
const POLL: Duration = Duration::from_secs(30);
/// A friend with no status for this long is offline.
const STALE_SECS: u64 = 7 * 60;
/// Friend requests wait on ntfy.sh for its cache time; read this often, this far back.
const INVITE_POLL: Duration = Duration::from_secs(2 * 60);
const INVITE_SINCE: &str = "12h";
pub const SERVER_PORT: u16 = 25590;
const NO_TUNNEL: &str = "Связь пропала: включи VPN и открой сервер снова";

#[derive(Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct ModRef {
    pub id: String,
    pub name: String,
}

/// What a launcher tells its friends.
#[derive(Serialize, Deserialize, Clone, Default, PartialEq)]
#[serde(default)]
pub struct Presence {
    pub name: String,
    pub offline: bool,
    pub playing: bool,
    /// The chosen version, as shown in the launcher.
    pub game: String,
    /// Minecraft version a guest needs (ordinary versions; Horizon has its own).
    pub game_id: String,
    pub horizon: bool,
    pub fine: bool,
    /// Horizon add-ons that are switched on; a guest needs the same ones.
    pub mods: Vec<ModRef>,
    /// Native mods of this player's build: their own client only.
    pub native: Vec<String>,
    /// bore.pub:PORT while their server is open.
    pub address: Option<String>,
    /// Face and hat, 8×8 RGB as hex.
    pub face: String,
    /// While their server is open: its world, who is on it and how many fit.
    pub world: String,
    pub players: Vec<String>,
    pub max_players: u32,
    /// Nicknames their server lets in.
    pub allowed: Vec<String>,
    /// Whose server they are playing on, and its address.
    pub visiting: Option<String>,
    pub visiting_address: Option<String>,
}

/// A world that can be opened for friends.
#[derive(Clone, PartialEq)]
pub struct WorldEntry {
    pub name: String,
    pub path: PathBuf,
    /// Already in the server's folder; otherwise a single-player save, copied on first use.
    pub on_server: bool,
}

pub enum WorldChoice {
    New(String),
    Existing(WorldEntry),
}

/// What runs the server: horizon-server, or Mojang's server.jar for ordinary versions.
pub enum ServerKind {
    Horizon,
    Java { java: PathBuf, jar: PathBuf, version: String, ram_mb: u32 },
}

pub struct ServerPlan {
    pub kind: ServerKind,
    /// Single-player worlds of the same game: the server plays them and gives them back there.
    pub saves: PathBuf,
    pub world: WorldChoice,
    pub max_players: u32,
    pub creative: bool,
}

#[derive(Clone, PartialEq)]
pub enum HostState {
    Off,
    Starting(String),
    On(String),
    Failed(String),
}

pub struct Hub {
    pub code: String,
    pub me: Mutex<Presence>,
    pub friends: Mutex<Vec<String>>,
    seen: Mutex<HashMap<String, (Presence, u64)>>,
    pub error: Mutex<Option<String>>,
    pub host: Mutex<HostState>,
    /// Nicknames allowed on the server: this player and their friends.
    pub allowed: Mutex<Vec<String>>,
    tunnel: Mutex<Option<crate::tunnel::Tunnel>>,
    /// The running server.jar; its console is this process's standard input.
    java: Mutex<Option<Child>>,
    server_dir: Mutex<Option<PathBuf>>,
    /// The open world in the server's folder and its place among the single-player worlds,
    /// where it is copied back when the server closes.
    world_sync: Mutex<Option<(PathBuf, PathBuf)>>,
    refresh: AtomicBool,
    /// Codes to tell "I added you": the friend then adds this player back by itself.
    pub invites: Mutex<Vec<String>>,
    /// Codes of players who added this one; the app saves them into its friends list.
    pub added_me: Mutex<Vec<String>>,
}

/// A new friend code: 100 random bits in Crockford base32, grouped.
pub fn new_code() -> String {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    // RandomState is seeded by the operating system's random source.
    let word = || RandomState::new().build_hasher().finish() as u128;
    let mut bits = word() << 64 | word();
    let mut out = String::from("ZYN");
    for i in 0..20 {
        if i % 5 == 0 {
            out.push('-');
        }
        out.push(ALPHABET[(bits & 31) as usize] as char);
        bits >>= 5;
    }
    out
}

/// A pasted code in its canonical form, or None if it can't be a friend code.
pub fn parse_code(input: &str) -> Option<String> {
    let clean: String = input.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_uppercase();
    let body = clean.strip_prefix("ZYN")?;
    if body.len() != 20 {
        return None;
    }
    let groups: Vec<&str> = (0..4).map(|i| &body[i * 5..(i + 1) * 5]).collect();
    Some(format!("ZYN-{}", groups.join("-")))
}

fn topic(code: &str) -> String {
    format!("zyntra{}", code.replace('-', "").to_ascii_lowercase())
}

/// Where others leave "I added you" for this code; only its owner reads it.
fn invite_topic(code: &str) -> String {
    format!("zyntrainv{}", code.replace('-', "").to_ascii_lowercase())
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

/// The skin's face with its hat layer on top, as 8×8 RGB hex.
pub fn face_hex(skin: &RgbaImage) -> String {
    let mut out = String::with_capacity(8 * 8 * 6);
    for y in 0..8 {
        for x in 0..8 {
            let hat = skin.get_pixel(40 + x, 8 + y).0;
            let px = if hat[3] > 127 { hat } else { skin.get_pixel(8 + x, 8 + y).0 };
            out.push_str(&format!("{:02x}{:02x}{:02x}", px[0], px[1], px[2]));
        }
    }
    out
}

pub fn face_image(hex: &str) -> Option<egui::ColorImage> {
    if hex.len() != 8 * 8 * 6 || !hex.is_ascii() {
        return None;
    }
    let rgb: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)).collect::<Result<_, _>>().ok()?;
    Some(egui::ColorImage::from_rgb([8, 8], &rgb))
}

#[derive(Deserialize)]
struct Line {
    #[serde(default)]
    event: String,
    #[serde(default)]
    topic: String,
    #[serde(default)]
    time: u64,
    #[serde(default)]
    message: String,
}

impl Hub {
    pub fn new(code: String, friends: Vec<String>) -> Arc<Self> {
        Arc::new(Self {
            code,
            me: Mutex::default(),
            friends: Mutex::new(friends),
            seen: Mutex::default(),
            error: Mutex::default(),
            host: Mutex::new(HostState::Off),
            allowed: Mutex::default(),
            tunnel: Mutex::default(),
            java: Mutex::default(),
            server_dir: Mutex::default(),
            world_sync: Mutex::default(),
            refresh: AtomicBool::new(true),
            invites: Mutex::default(),
            added_me: Mutex::default(),
        })
    }

    /// Asks for a status check now, not at the next round.
    pub fn refresh(&self) {
        self.refresh.store(true, Ordering::Relaxed);
    }

    /// A friend's latest status; `online` is false when it is too old or says goodbye.
    pub fn status(&self, code: &str) -> Option<(Presence, bool)> {
        let seen = self.seen.lock().unwrap();
        let (presence, time) = seen.get(code)?;
        Some((presence.clone(), !presence.offline && now().saturating_sub(*time) < STALE_SECS))
    }

    fn publish(&self, me: &Presence) -> Result<()> {
        crate::net::client()
            .post(format!("{NTFY}/{}", topic(&self.code)))
            .timeout(Duration::from_secs(15))
            .body(serde_json::to_string(me)?)
            .send()?
            .error_for_status()?;
        Ok(())
    }

    fn send_invite(&self, to: &str) -> Result<()> {
        crate::net::client()
            .post(format!("{NTFY}/{}", invite_topic(to)))
            .timeout(Duration::from_secs(15))
            .body(self.code.clone())
            .send()?
            .error_for_status()?;
        Ok(())
    }

    /// Players who added this one by its code become its friends too.
    fn poll_invites(&self) -> Result<()> {
        let body = crate::net::client()
            .get(format!("{NTFY}/{}/json?poll=1&since={INVITE_SINCE}", invite_topic(&self.code)))
            .timeout(Duration::from_secs(15))
            .send()?
            .error_for_status()?
            .text()?;
        let mut friends = self.friends.lock().unwrap();
        for line in body.lines().filter_map(|l| serde_json::from_str::<Line>(l).ok()).filter(|l| l.event == "message") {
            let Some(code) = parse_code(&line.message) else { continue };
            if code != self.code && !friends.contains(&code) {
                friends.push(code.clone());
                self.added_me.lock().unwrap().push(code);
                self.refresh();
            }
        }
        Ok(())
    }

    fn poll(&self) -> Result<()> {
        let codes = self.friends.lock().unwrap().clone();
        if codes.is_empty() {
            return Ok(());
        }
        let topics: Vec<String> = codes.iter().map(|c| topic(c)).collect();
        let body = crate::net::client()
            .get(format!("{NTFY}/{}/json?poll=1&since={}s", topics.join(","), STALE_SECS))
            .timeout(Duration::from_secs(15))
            .send()?
            .error_for_status()?
            .text()?;
        let mut seen = self.seen.lock().unwrap();
        for line in body.lines().filter_map(|l| serde_json::from_str::<Line>(l).ok()).filter(|l| l.event == "message") {
            let Some(code) = codes.iter().find(|c| topic(c) == line.topic) else { continue };
            let Ok(presence) = serde_json::from_str::<Presence>(&line.message) else { continue };
            if seen.get(code).is_none_or(|(_, time)| *time <= line.time) {
                seen.insert(code.clone(), (presence, line.time));
            }
        }
        Ok(())
    }

    /// The status loop: runs while the launcher is open, also when its window is hidden.
    pub fn run(self: Arc<Self>, ctx: egui::Context) {
        std::thread::spawn(move || {
            let mut sent: Option<(Presence, Instant)> = None;
            let mut polled: Option<Instant> = None;
            let mut invites_polled: Option<Instant> = None;
            loop {
                let pending: Vec<String> = std::mem::take(&mut *self.invites.lock().unwrap());
                for (i, to) in pending.iter().enumerate() {
                    if let Err(e) = self.send_invite(to) {
                        *self.error.lock().unwrap() = Some(format!("Другу не ушёл запрос: {e}"));
                        self.invites.lock().unwrap().extend(pending[i..].iter().cloned());
                        break;
                    }
                }
                if invites_polled.is_none_or(|t| t.elapsed() >= INVITE_POLL) {
                    invites_polled = Some(Instant::now());
                    self.poll_invites().ok();
                }
                let me = self.me.lock().unwrap().clone();
                let due = sent.as_ref().is_none_or(|(last, at)| *last != me || at.elapsed() >= REPEAT);
                if due && !me.name.is_empty() {
                    match self.publish(&me) {
                        Ok(()) => {
                            sent = Some((me, Instant::now()));
                            *self.error.lock().unwrap() = None;
                        }
                        Err(e) => {
                            *self.error.lock().unwrap() = Some(format!("Статус не отправлен: {e}"));
                            // Not every second: try again at the next round.
                            sent = Some((Presence::default(), Instant::now()));
                        }
                    }
                }
                if self.refresh.swap(false, Ordering::Relaxed) || polled.is_none_or(|t| t.elapsed() >= POLL) {
                    polled = Some(Instant::now());
                    if let Err(e) = self.poll() {
                        *self.error.lock().unwrap() = Some(format!("Нет связи со списком друзей: {e}"));
                    }
                    ctx.request_repaint();
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        });
    }

    /// Launcher closing: friends see it at once, and the server closes with it.
    pub fn goodbye(&self) {
        self.stop_server();
        let mut me = self.me.lock().unwrap().clone();
        me.offline = true;
        me.playing = false;
        me.address = None;
        self.publish(&me).ok();
    }

    /// Opens this player's server to the internet. Blocks until the address is known.
    /// Horizon must already be installed and prepared.
    pub fn start_server(self: &Arc<Self>, game_dir: &Path, plan: ServerPlan, task: &Task) -> Result<()> {
        let version = match &plan.kind {
            ServerKind::Horizon => None,
            ServerKind::Java { version, .. } => Some(version.clone()),
        };
        let dir = server_folder(game_dir, version.as_deref());
        match self.open_server(game_dir, &dir, plan, task) {
            Ok(address) => {
                self.me.lock().unwrap().address = Some(address.clone());
                *self.host.lock().unwrap() = HostState::On(address);
                self.clone().watch_server(game_dir, dir);
                Ok(())
            }
            Err(e) => {
                self.stop_server();
                *self.host.lock().unwrap() = HostState::Failed(format!("{e}"));
                Err(e)
            }
        }
    }

    /// While the server is open: keeps its whitelist equal to the friends list and reports who plays.
    fn watch_server(self: Arc<Self>, game_dir: &Path, dir: PathBuf) {
        let exe = horizon::game_folder(game_dir).join(horizon::exe("horizon-server"));
        std::thread::spawn(move || {
            let mut applied = self.allowed.lock().unwrap().clone();
            let mut unreachable = 0;
            while matches!(*self.host.lock().unwrap(), HostState::On(_)) {
                let java_died = self.java.lock().unwrap().as_mut().is_some_and(|c| !matches!(c.try_wait(), Ok(None)));
                // horizon-server runs on its own: it is alive while its port answers.
                let address: SocketAddr = ([127, 0, 0, 1], SERVER_PORT).into();
                unreachable = if TcpStream::connect_timeout(&address, Duration::from_secs(2)).is_ok() { 0 } else { unreachable + 1 };
                if java_died || unreachable >= 3 {
                    *self.host.lock().unwrap() = HostState::Failed(format!("Сервер остановился. {}", last_log_line(&dir)));
                    self.stop_server();
                    return;
                }
                let allowed = self.allowed.lock().unwrap().clone();
                if allowed != applied && write_whitelist(&dir, &allowed).is_ok() {
                    self.console(&exe, &dir, &["whitelist", "reload"]);
                    applied = allowed;
                }
                // server.jar answers in its output, read by the thread in `spawn_java`.
                if let Some(list) = parse_list(&self.console(&exe, &dir, &["list"])) {
                    self.set_players(list);
                }
                std::thread::sleep(Duration::from_secs(10));
            }
        });
    }

    fn set_players(&self, (players, max): (Vec<String>, u32)) {
        let mut me = self.me.lock().unwrap();
        me.players = players;
        me.max_players = max;
    }

    /// A console command on the running server, and its answer when it gives one back directly.
    fn console(&self, exe: &Path, dir: &Path, command: &[&str]) -> String {
        if let Some(java) = self.java.lock().unwrap().as_mut() {
            if let Some(stdin) = java.stdin.as_mut() {
                writeln!(stdin, "{}", command.join(" ")).and_then(|()| stdin.flush()).ok();
            }
            return String::new();
        }
        server_command(exe, dir, command)
    }

    fn spawn_java(self: &Arc<Self>, dir: &Path, java: &Path, jar: &Path, ram_mb: u32) -> Result<()> {
        let mut child = quiet(java)
            .args(["-Xms256M".to_owned(), format!("-Xmx{ram_mb}M"), "-XX:+UseG1GC".to_owned(), "-XX:MaxGCPauseMillis=50".to_owned(), "-jar".to_owned()])
            .arg(jar)
            .arg("nogui")
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .context("Не удалось запустить server.jar")?;
        let out = child.stdout.take().context("Нет вывода сервера")?;
        *self.java.lock().unwrap() = Some(child);
        let hub = self.clone();
        std::thread::spawn(move || {
            // Bytes, not lines of text: the console may not be UTF-8, and this pipe must keep draining.
            for line in BufReader::new(out).split(b'\n').map_while(Result::ok) {
                if let Some(list) = parse_list(&String::from_utf8_lossy(&line)) {
                    hub.set_players(list);
                }
            }
        });
        Ok(())
    }

    fn open_server(self: &Arc<Self>, game_dir: &Path, dir: &Path, plan: ServerPlan, task: &Task) -> Result<String> {
        let state = |s: &str| {
            task.stage(s);
            *self.host.lock().unwrap() = HostState::Starting(s.to_owned());
        };
        let exe = horizon::game_folder(game_dir).join(horizon::exe("horizon-server"));
        fs::create_dir_all(dir)?;
        let horizon_server = matches!(plan.kind, ServerKind::Horizon);
        if horizon_server {
            *self.server_dir.lock().unwrap() = Some(dir.to_owned());
        }
        if horizon_server && !dir.join("server.toml").exists() {
            state("Сервер: создание мира");
            let done = quiet(&exe)
                .arg("--dir")
                .arg(dir)
                .args(["init", "--eula", "--world", "world", "--port", &SERVER_PORT.to_string(), "--whitelist", "--offline", "--yes"])
                .status()
                .context("Не удалось запустить horizon-server")?;
            if !done.success() {
                bail!("horizon-server не смог создать сервер");
            }
        }
        state("Сервер: подготовка мира");
        give_back_all(dir, &plan.saves);
        let world = prepare_world(dir, &plan.saves, &plan.world)?;
        // Single-player is where the world lives: a newer copy there replaces the server's.
        let (on_server, in_saves) = (dir.join(&world), plan.saves.join(&world));
        if is_newer(&in_saves, &on_server) {
            if on_server.exists() {
                fs::remove_dir_all(&on_server)?;
            }
            copy_world(&in_saves, &on_server).with_context(|| format!("Не удалось скопировать мир {world}"))?;
        }
        *self.world_sync.lock().unwrap() = Some((on_server, in_saves));
        match &plan.kind {
            ServerKind::Horizon => apply_settings(&dir.join("server.toml"), &world, &plan)?,
            ServerKind::Java { version, .. } => {
                // The player ticked the EULA box before the button could be pressed.
                fs::write(dir.join("eula.txt"), "eula=true\n")?;
                write_properties(dir, &world, &plan, version)?;
            }
        }
        write_whitelist(dir, &self.allowed.lock().unwrap())?;
        {
            let mut me = self.me.lock().unwrap();
            me.world.clone_from(&world);
            me.max_players = plan.max_players;
            me.players.clear();
        }

        state("Сервер: запуск");
        match &plan.kind {
            ServerKind::Horizon => {
                let log = fs::File::create(dir.join("start.log"))?;
                quiet(&exe).arg("--dir").arg(dir).arg("start").stdout(log.try_clone()?).stderr(log).spawn().context("Не удалось запустить сервер")?;
            }
            ServerKind::Java { java, jar, ram_mb, .. } => self.spawn_java(dir, java, jar, *ram_mb)?,
        }
        let address: SocketAddr = ([127, 0, 0, 1], SERVER_PORT).into();
        let started = Instant::now();
        while TcpStream::connect_timeout(&address, Duration::from_secs(1)).is_err() {
            let died = self.java.lock().unwrap().as_mut().is_some_and(|c| !matches!(c.try_wait(), Ok(None)));
            if died || started.elapsed() > Duration::from_secs(180) {
                bail!("Сервер не запустился: смотри {}", dir.join("logs").display());
            }
            std::thread::sleep(Duration::from_secs(1));
        }

        state("Сервер: открываю доступ для друзей");
        let (hub, moved) = (self.clone(), self.clone());
        let on_moved = move |address: String| {
            // Back after a break, on another port: friends get the new address with the status.
            let mut host = moved.host.lock().unwrap();
            if matches!(*host, HostState::On(_)) {
                *host = HostState::On(address.clone());
                moved.me.lock().unwrap().address = Some(address);
            }
        };
        let tunnel = crate::tunnel::open(SERVER_PORT, on_moved, move |reason: String| {
            // The tunnel fell and didn't come back: friends can't reach the server any more.
            let mut host = hub.host.lock().unwrap();
            if matches!(*host, HostState::On(_)) {
                eprintln!("Туннель закрыт: {reason}");
                *host = HostState::Failed(NO_TUNNEL.into());
                drop(host);
                hub.stop_server();
            }
        })?;
        let address = tunnel.address.clone();
        *self.tunnel.lock().unwrap() = Some(tunnel);
        Ok(address)
    }

    pub fn stop_server(&self) {
        {
            let mut me = self.me.lock().unwrap();
            me.address = None;
            me.world.clear();
            me.players.clear();
            me.max_players = 0;
        }
        let mut host = self.host.lock().unwrap();
        if !matches!(*host, HostState::Failed(_)) {
            *host = HostState::Off;
        }
        drop(host);
        if let Some(tunnel) = self.tunnel.lock().unwrap().take() {
            tunnel.close();
        }
        if let Some(mut java) = self.java.lock().unwrap().take() {
            if let Some(stdin) = java.stdin.as_mut() {
                writeln!(stdin, "stop").and_then(|()| stdin.flush()).ok();
            }
            let asked = Instant::now();
            while matches!(java.try_wait(), Ok(None)) && asked.elapsed() < Duration::from_secs(30) {
                std::thread::sleep(Duration::from_millis(200));
            }
            java.kill().ok();
            java.wait().ok();
        }
        if let Some(dir) = self.server_dir.lock().unwrap().take() {
            let exe = dir.parent().map(|h| h.join("game").join(horizon::exe("horizon-server")));
            if let Some(exe) = exe {
                quiet(&exe).arg("--dir").arg(&dir).arg("stop").status().ok();
            }
        }
        // The server has saved and stopped: its world goes back to single-player.
        if let Some((on_server, in_saves)) = self.world_sync.lock().unwrap().take() {
            if let Err(e) = give_back(&on_server, &in_saves) {
                *self.error.lock().unwrap() = Some(format!("Мир не скопирован в одиночную игру: {e:#}"));
            }
        }
    }
}

/// The last error or line of the server's log, for the message the player sees.
fn last_log_line(dir: &Path) -> String {
    let log = fs::read_to_string(dir.join("logs").join("latest.log")).unwrap_or_default();
    let lines: Vec<&str> = log.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let line = lines.iter().rev().find(|l| l.contains("ERROR") || l.contains("panick")).or(lines.last());
    match line {
        Some(l) => format!("Последняя запись лога: {}", l.chars().take(200).collect::<String>()),
        None => format!("Смотри {}", dir.join("logs").display()),
    }
}

/// A background command with no console window and no inherited pipes.
fn quiet(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

/// The output of a console command on the running server.
fn server_command(exe: &Path, dir: &Path, command: &[&str]) -> String {
    let output = quiet(exe).arg("--dir").arg(dir).arg("cmd").args(command).stdout(Stdio::piped()).output();
    output.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

/// `There are 2 of a max of 5 players online: Anna, Oleg`, or `There are 2/5 players online:`.
fn parse_list(text: &str) -> Option<(Vec<String>, u32)> {
    let line = text.lines().rev().find(|l| l.contains(" players online:") && l.contains("There are "))?;
    let (head, names) = line.split_once(" players online:")?;
    let max = head.rsplit([' ', '/']).next()?.parse().ok()?;
    Some((names.split(',').map(str::trim).filter(|n| !n.is_empty()).map(str::to_owned).collect(), max))
}

/// The id an offline server gives a nickname: name-based (version 3) and case-sensitive.
fn offline_uuid(name: &str) -> String {
    let hex = minecraft::offline_uuid(name);
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

/// Written by hand: the console's `whitelist add` lowercases the name, which gives another id
/// than the player's own on an offline server, so a «Player» would never get in.
fn write_whitelist(dir: &Path, names: &[String]) -> Result<()> {
    let list: Vec<serde_json::Value> = names.iter().map(|n| serde_json::json!({ "uuid": offline_uuid(n), "name": n })).collect();
    fs::write(dir.join("whitelist.json"), serde_json::to_string_pretty(&list)?)?;
    Ok(())
}

/// The server's folder: Horizon's, or one per Minecraft version.
pub fn server_folder(game_dir: &Path, version: Option<&str>) -> PathBuf {
    match version {
        Some(version) => game_dir.join("friend-servers").join(version),
        None => horizon::dir(game_dir).join("friend-server"),
    }
}

/// `N` of a version named `1.N` or `1.N.M`; None for snapshots and years (`26.3`), all modern.
fn minor_version(version: &str) -> Option<u32> {
    version.strip_prefix("1.")?.split(['.', '-']).next()?.parse().ok()
}

/// Installs the version and downloads Mojang's server.jar for it.
pub fn prepare_java(game_dir: &Path, java_override: &str, manifest: Option<&Manifest>, version: &str, ram_mb: u32, task: &Task) -> Result<ServerKind> {
    if minor_version(version).is_some_and(|minor| minor < 8) {
        bail!("Сервер с друзьями работает на версиях 1.8 и новее");
    }
    let dirs = Dirs::new(game_dir.to_path_buf(), game_dir);
    let prepared = minecraft::install(&dirs, version, manifest, java_override, task)?;
    let server = &prepared.v["downloads"]["server"];
    let jar = dirs.version_dir(version).join("server.jar");
    net::download_all(
        vec![Job {
            url: server["url"].as_str().with_context(|| format!("У версии {version} нет официального сервера"))?.to_owned(),
            path: jar.clone(),
            sha1: server["sha1"].as_str().map(str::to_owned),
            size: server["size"].as_u64(),
        }],
        task,
        "Сервер Minecraft",
    )?;
    Ok(ServerKind::Java { java: prepared.java, jar, version: version.to_owned(), ram_mb })
}

/// server.properties of server.jar: keeps what the player changed, sets what friends need.
fn write_properties(dir: &Path, world: &str, plan: &ServerPlan, version: &str) -> Result<()> {
    let path = dir.join("server.properties");
    // Versions before 1.13 read the game mode as a number.
    let numeric = minor_version(version).is_some_and(|minor| minor < 13);
    let mode = match (plan.creative, numeric) {
        (true, true) => "1",
        (false, true) => "0",
        (true, false) => "creative",
        (false, false) => "survival",
    };
    let max = plan.max_players.to_string();
    let port = SERVER_PORT.to_string();
    let settings = [
        ("online-mode", "false"),
        ("white-list", "true"),
        ("enforce-whitelist", "true"),
        ("server-ip", "127.0.0.1"),
        ("server-port", port.as_str()),
        ("max-players", max.as_str()),
        ("level-name", world),
        ("gamemode", mode),
        // The server shares the computer with the game: a smaller world around the players
        // keeps it from falling behind and dropping them with «Timed out».
        ("view-distance", "6"),
        ("simulation-distance", "5"),
        ("sync-chunk-writes", "false"),
    ];
    let text = fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<String> = text.lines().filter(|l| !settings.iter().any(|(k, _)| l.starts_with(&format!("{k}=")))).map(str::to_owned).collect();
    lines.extend(settings.iter().map(|(k, v)| format!("{k}={v}")));
    fs::write(path, lines.join("\n") + "\n")?;
    Ok(())
}

/// Worlds that can be opened: the server's own, then single-player saves not copied yet.
pub fn worlds(server: &Path, saves: &Path) -> Vec<WorldEntry> {
    let list = |folder: PathBuf, on_server: bool| -> Vec<WorldEntry> {
        let mut found: Vec<WorldEntry> = fs::read_dir(folder)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().join("level.dat").exists())
            .map(|e| WorldEntry { name: e.file_name().to_string_lossy().into_owned(), path: e.path(), on_server })
            .collect();
        found.sort_by(|a, b| a.name.cmp(&b.name));
        found
    };
    let mut all = list(server.to_path_buf(), true);
    for save in list(saves.to_path_buf(), false) {
        if !all.iter().any(|w| w.name == save.name) {
            all.push(save);
        }
    }
    all
}

/// The world's folder name in the server's folder; a save is copied, its original stays as it was.
fn prepare_world(dir: &Path, saves: &Path, choice: &WorldChoice) -> Result<String> {
    match choice {
        WorldChoice::New(wanted) => {
            let clean: String = wanted.chars().filter(|c| c.is_alphanumeric() || " -_".contains(*c)).collect();
            let base = clean.trim().to_owned();
            let base = if base.is_empty() { "Мир друзей".to_owned() } else { base };
            let free = std::iter::once(base.clone()).chain((2..).map(|n| format!("{base} {n}")));
            // Free here and in single-player, where the world will be copied back.
            Ok(free.into_iter().find(|name| !dir.join(name).exists() && !saves.join(name).exists()).unwrap_or(base))
        }
        WorldChoice::Existing(world) => {
            if !world.on_server && !dir.join(&world.name).exists() {
                copy_world(&world.path, &dir.join(&world.name)).with_context(|| format!("Не удалось скопировать мир {}", world.name))?;
            }
            Ok(world.name.clone())
        }
    }
}

/// Every world of the server's folder that single-player lacks, or has an older copy of,
/// is copied there: worlds made for friends can be played alone too.
pub fn give_back_all(server: &Path, saves: &Path) {
    for entry in fs::read_dir(server).into_iter().flatten().flatten() {
        let (on_server, in_saves) = (entry.path(), saves.join(entry.file_name()));
        if on_server.join("level.dat").exists() && is_newer(&on_server, &in_saves) {
            give_back(&on_server, &in_saves).ok();
        }
    }
}

/// `a`'s world was saved after `b`'s (or `b` has none).
fn is_newer(a: &Path, b: &Path) -> bool {
    let time = |p: &Path| fs::metadata(p.join("level.dat")).and_then(|m| m.modified()).ok();
    match (time(a), time(b)) {
        (Some(a), Some(b)) => a > b,
        (Some(_), None) => true,
        _ => false,
    }
}

/// Copies the server's world over its single-player copy, through a temporary folder so a
/// failed copy never leaves the player without one.
fn give_back(on_server: &Path, in_saves: &Path) -> Result<()> {
    if !on_server.join("level.dat").exists() {
        return Ok(());
    }
    let parent = in_saves.parent().context("нет папки миров")?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".{}.zyntra", in_saves.file_name().unwrap_or_default().to_string_lossy()));
    if temp.exists() {
        fs::remove_dir_all(&temp)?;
    }
    copy_world(on_server, &temp)?;
    if in_saves.exists() {
        fs::remove_dir_all(in_saves)?;
    }
    fs::rename(&temp, in_saves)?;
    Ok(())
}

fn copy_world(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)?.flatten() {
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        if source.is_dir() {
            copy_world(&source, &target)?;
        } else if entry.file_name() != "session.lock" {
            fs::copy(&source, &target)?;
        }
    }
    Ok(())
}

/// The chosen world and limits. Friends only, the game port stays on this computer (only the
/// tunnel reaches it) and the web panel stays off.
fn apply_settings(path: &Path, world: &str, plan: &ServerPlan) -> Result<()> {
    let text = fs::read_to_string(path).with_context(|| format!("Нет {}", path.display()))?;
    let (mut section, mut lines) = (String::new(), Vec::new());
    let settings: [(&str, &str, String); 7] = [
        ("[server]", "bind", "\"127.0.0.1\"".to_owned()),
        ("[server]", "max_players", plan.max_players.to_string()),
        ("[server]", "whitelist", "true".to_owned()),
        ("[server]", "enforce_whitelist", "true".to_owned()),
        ("[world]", "name", serde_json::to_string(world)?),
        ("[world]", "game_mode", if plan.creative { "\"creative\"" } else { "\"survival\"" }.to_owned()),
        ("[panel]", "bind", "\"\"".to_owned()),
    ];
    for line in text.lines() {
        if line.starts_with('[') {
            section = line.to_owned();
        }
        let key = line.split(' ').next().unwrap_or_default();
        let setting = settings.iter().find(|(s, k, _)| *s == section && *k == key);
        lines.push(setting.map_or_else(|| line.to_owned(), |(_, k, value)| format!("{k} = {value}")));
    }
    fs::write(path, lines.join("\n") + "\n")?;
    Ok(())
}

/// Makes Horizon's add-ons match the server's: installs what is missing and switches the rest off.
pub fn match_mods(game: &Path, wanted: &[ModRef], task: &Task) -> Result<()> {
    let installed = crate::hz_mods::installed(game);
    let mut index = None;
    for m in wanted {
        if !installed.iter().any(|i| i.id == m.id && i.enabled.is_some()) {
            task.stage(&format!("Установка мода {}", m.name));
            if m.id == crate::hz_mods::CRYSTALS_ID {
                crate::hz_mods::install_crystals(game)?;
            } else {
                if index.is_none() {
                    index = Some(crate::hz_mods::fetch_index()?);
                }
                let project = index.as_ref().and_then(|i| i.iter().find(|p| p.id == m.id));
                let Some(project) = project else { bail!("Мод {} не найден в каталоге Horizon", m.name) };
                crate::hz_mods::install(project, game, task)?;
            }
        }
        crate::hz_mods::set_enabled(game, &m.id, true)?;
    }
    for item in installed.iter().filter(|i| i.enabled == Some(true) && !wanted.iter().any(|m| m.id == i.id)) {
        crate::hz_mods::set_enabled(game, &item.id, false)?;
    }
    Ok(())
}

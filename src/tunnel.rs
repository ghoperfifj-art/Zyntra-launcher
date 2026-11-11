//! A public TCP tunnel built into the launcher: a small client for the bore protocol
//! (github.com/ekzhang/bore, MIT). Lets friends reach a server behind mobile internet or a VPN.
//!
//! Frames are JSON, ended by a zero byte, on the service's port 7835. `Hello` gets a public port;
//! each visitor then arrives as `Connection(id)`, answered by a new stream that says `Accept(id)`
//! and carries that visitor's bytes from then on.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const HOST: &str = "bore.pub";
const CONTROL_PORT: u16 = 7835;
const MAX_FRAME: usize = 256;
const TIMEOUT: Duration = Duration::from_secs(20);
/// The service sends a heartbeat every few hundred ms; this long without one means it is gone.
const SILENCE: Duration = Duration::from_secs(20);
const RETRIES: u32 = 12;
const RETRY_PAUSE: Duration = Duration::from_secs(5);

pub struct Tunnel {
    pub address: String,
    control: Arc<Mutex<Option<TcpStream>>>,
    closed: Arc<AtomicBool>,
}

impl Tunnel {
    pub fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        if let Some(control) = self.control.lock().unwrap().as_ref() {
            control.shutdown(Shutdown::Both).ok();
        }
    }
}

fn connect(host: &str, port: u16) -> Result<TcpStream> {
    let mut addresses: Vec<_> = (host, port).to_socket_addrs()?.collect();
    addresses.sort_by_key(|a| a.is_ipv6());
    let mut last = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, TIMEOUT) {
            Ok(stream) => {
                stream.set_nodelay(true)?;
                return Ok(stream);
            }
            Err(e) => last = Some(e),
        }
    }
    Err(last.map_or_else(|| anyhow::anyhow!("Не найден адрес {host}"), Into::into))
}

fn send(stream: &mut TcpStream, message: &Value) -> Result<()> {
    let mut frame = serde_json::to_vec(message)?;
    frame.push(0);
    stream.write_all(&frame)?;
    Ok(())
}

fn receive(reader: &mut BufReader<TcpStream>) -> Result<Value> {
    let mut frame = Vec::new();
    if reader.read_until(0, &mut frame)? == 0 {
        bail!("Соединение закрыто");
    }
    if frame.pop() != Some(0) || frame.len() > MAX_FRAME {
        bail!("Неверный ответ туннеля");
    }
    Ok(serde_json::from_slice(&frame)?)
}

/// The control connection: asks for `port` (0: any free one) and returns the one given.
fn hello(port: u64) -> Result<(TcpStream, BufReader<TcpStream>, u64)> {
    let mut control = connect(HOST, CONTROL_PORT).context("Нет связи: включи VPN и попробуй снова")?;
    control.set_read_timeout(Some(TIMEOUT))?;
    let mut reader = BufReader::new(control.try_clone()?);
    send(&mut control, &json!({ "Hello": port }))?;
    let answer = receive(&mut reader)?;
    let Some(given) = answer["Hello"].as_u64() else {
        bail!("bore.pub не выдал порт: {answer}");
    };
    control.set_read_timeout(Some(SILENCE))?;
    Ok((control, reader, given))
}

/// Opens a public port for `local_port`. When the connection to the service drops (a short
/// break in Wi-Fi or mobile internet), it connects again and asks for the same port;
/// `on_moved` gets the new address if that port was taken meanwhile. `on_close` runs once,
/// with the reason, when the tunnel can't be brought back.
pub fn open(local_port: u16, on_moved: impl Fn(String) + Send + 'static, on_close: impl FnOnce(String) + Send + 'static) -> Result<Tunnel> {
    let (control, mut reader, mut port) = hello(0)?;
    let shared = Arc::new(Mutex::new(Some(control)));
    let closed = Arc::new(AtomicBool::new(false));
    let tunnel = Tunnel { address: format!("{HOST}:{port}"), control: shared.clone(), closed: closed.clone() };
    std::thread::spawn(move || {
        let reason = loop {
            let lost = loop {
                match receive(&mut reader) {
                    Ok(message) => {
                        if let Some(id) = message["Connection"].as_str() {
                            let id = id.to_owned();
                            std::thread::spawn(move || {
                                forward(&id, local_port).ok();
                            });
                        } else if let Some(e) = message.get("Error") {
                            break format!("bore.pub: {e}");
                        }
                    }
                    Err(e) => break format!("{e:#}"),
                }
            };
            if closed.load(Ordering::Relaxed) {
                return;
            }
            // Try again for about a minute before giving up.
            let mut back = None;
            for _ in 0..RETRIES {
                std::thread::sleep(RETRY_PAUSE);
                if closed.load(Ordering::Relaxed) {
                    return;
                }
                if let Ok(found) = hello(port) {
                    back = Some(found);
                    break;
                }
            }
            let Some((control, new_reader, given)) = back else { break lost };
            *shared.lock().unwrap() = Some(control);
            if closed.load(Ordering::Relaxed) {
                if let Some(c) = shared.lock().unwrap().take() {
                    c.shutdown(Shutdown::Both).ok();
                }
                return;
            }
            reader = new_reader;
            if given != port {
                port = given;
                on_moved(format!("{HOST}:{port}"));
            }
        };
        on_close(reason);
    });
    Ok(tunnel)
}

/// Carries one visitor between the service and the local port until either side closes.
fn forward(id: &str, local_port: u16) -> Result<()> {
    let mut remote = connect(HOST, CONTROL_PORT)?;
    send(&mut remote, &json!({ "Accept": id }))?;
    let mut local = connect("127.0.0.1", local_port)?;
    let (mut remote_read, mut local_write) = (remote.try_clone()?, local.try_clone()?);
    let upload = std::thread::spawn(move || {
        std::io::copy(&mut remote_read, &mut local_write).ok();
        local_write.shutdown(Shutdown::Both).ok();
        remote_read.shutdown(Shutdown::Both).ok();
    });
    std::io::copy(&mut local, &mut remote).ok();
    remote.shutdown(Shutdown::Both).ok();
    local.shutdown(Shutdown::Both).ok();
    upload.join().ok();
    Ok(())
}

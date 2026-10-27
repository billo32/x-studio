//! Wi‑Fi transport — the firmware's File Transfer server.
//!
//! - Discovery: UDP `hello` to port 8134 → `crosspoint (on <hostname>);<wsPort>`.
//! - HTTP :80: `GET /api/status`, `GET /api/files?path=`, `GET /download?path=`,
//!   `POST /mkdir` (name, path), `POST /delete` (path), `POST /rename` (path, name),
//!   `POST /move` (path, dest). Rename and move work on files only.
//! - WebSocket :81 uploads: text `START:<name>:<size>:<dir>` → `READY`, binary
//!   frames, device sends `PROGRESS:<got>:<total>` then `DONE` or `ERROR:<msg>`.
//!   The WebSocket path refuses to overwrite, so overwrite deletes first.

use std::io::{ErrorKind, Read};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::{
    join_path, split_path, DeviceStatus, Entry, Error, Progress, Result, Transport, TransportKind,
};

pub const DISCOVERY_PORT: u16 = 8134;
pub const DEFAULT_WS_PORT: u16 = 81;
const WS_CHUNK: usize = 4096;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FoundDevice {
    pub ip: String,
    pub hostname: String,
    pub ws_port: u16,
}

/// Broadcasts a discovery probe and collects answers for `wait`.
pub fn discover(wait: Duration) -> Result<Vec<FoundDevice>> {
    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    sock.set_broadcast(true)?;
    sock.set_read_timeout(Some(Duration::from_millis(150)))?;
    let target = SocketAddr::from((Ipv4Addr::BROADCAST, DISCOVERY_PORT));
    let deadline = Instant::now() + wait;
    let mut found: Vec<FoundDevice> = Vec::new();
    let mut next_probe = Instant::now();
    let mut buf = [0u8; 256];
    while Instant::now() < deadline {
        if Instant::now() >= next_probe {
            sock.send_to(b"hello", target)?;
            next_probe = Instant::now() + Duration::from_millis(500);
        }
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => {
                if let Some(dev) = parse_discovery(&String::from_utf8_lossy(&buf[..n]), from.ip()) {
                    if !found.iter().any(|d| d.ip == dev.ip) {
                        found.push(dev);
                    }
                }
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(found)
}

fn parse_discovery(msg: &str, ip: IpAddr) -> Option<FoundDevice> {
    let rest = msg.strip_prefix("crosspoint (on ")?;
    let (host, port) = rest.split_once(");")?;
    Some(FoundDevice {
        ip: ip.to_string(),
        hostname: host.to_string(),
        ws_port: port.trim().parse().ok()?,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiStatus {
    #[serde(default)]
    version: String,
    #[serde(default)]
    device: String,
    #[serde(default)]
    free_heap: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiEntry {
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    is_directory: bool,
}

pub struct WifiTransport {
    host: String,
    ws_port: u16,
    agent: ureq::Agent,
}

/// Connects to `host` (IP or `crosspoint.local`, optionally `host:port` for HTTP).
pub fn open(host: &str, ws_port: Option<u16>) -> Result<WifiTransport> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(4))
        .timeout_read(Duration::from_secs(30))
        .build();
    let mut t = WifiTransport {
        host: host.trim().trim_end_matches('/').to_string(),
        ws_port: ws_port.unwrap_or(DEFAULT_WS_PORT),
        agent,
    };
    t.status()?;
    Ok(t)
}

fn http_err(e: ureq::Error) -> Error {
    match e {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            Error::Http(format!("{code}: {}", body.trim()))
        }
        ureq::Error::Transport(t) => Error::Http(t.to_string()),
    }
}

impl WifiTransport {
    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.host, path)
    }

    fn post_form(&self, path: &str, form: &[(&str, &str)]) -> Result<()> {
        self.agent
            .post(&self.url(path))
            .send_form(form)
            .map_err(http_err)?;
        Ok(())
    }

    fn ws_host(&self) -> &str {
        // Strip an explicit HTTP port; uploads always go to the WebSocket port.
        match self.host.rsplit_once(':') {
            Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => h,
            _ => &self.host,
        }
    }

    fn ws_upload(&self, dir: &str, name: &str, data: &[u8], progress: Progress) -> Result<()> {
        let addr = format!("{}:{}", self.ws_host(), self.ws_port);
        let stream = TcpStream::connect(
            std::net::ToSocketAddrs::to_socket_addrs(&addr)?
                .next()
                .ok_or_else(|| Error::Http(format!("address {addr}")))?,
        )?;
        stream.set_nodelay(true)?;
        let (mut ws, _) =
            tungstenite::client(format!("ws://{addr}/"), MaybeTlsStream::Plain(stream))
                .map_err(|e| Error::Http(format!("WebSocket: {e}")))?;

        let total = data.len() as u64;
        // ':' would break the firmware's START parser.
        let safe_name = name.replace(':', "_");
        send(
            &mut ws,
            Message::text(format!("START:{safe_name}:{total}:{dir}")),
        )?;
        set_timeout(&ws, Some(Duration::from_secs(10)))?;
        match next_text(&mut ws)?.as_str() {
            "READY" => {}
            "DONE" if total == 0 => return Ok(()),
            other => return Err(ws_error(other)),
        }

        progress(0, total);
        set_timeout(&ws, Some(Duration::from_millis(2)))?;
        let mut acked = 0u64;
        for chunk in data.chunks(WS_CHUNK) {
            send(&mut ws, Message::binary(chunk.to_vec()))?;
            // Pick up PROGRESS/ERROR without blocking the stream.
            while let Some(msg) = try_text(&mut ws)? {
                acked = handle_progress(&msg, acked)?;
                progress(acked, total);
            }
        }
        set_timeout(&ws, Some(Duration::from_secs(30)))?;
        loop {
            let msg = next_text(&mut ws)?;
            if msg == "DONE" {
                progress(total, total);
                // Finish the close handshake so the reader frees the connection before the
                // next file opens a new one.
                let _ = ws.close(None);
                let _ = set_timeout(&ws, Some(Duration::from_secs(1)));
                while ws.read().is_ok() {}
                return Ok(());
            }
            acked = handle_progress(&msg, acked)?;
            progress(acked, total);
        }
    }
}

fn send(ws: &mut WebSocket<MaybeTlsStream<TcpStream>>, msg: Message) -> Result<()> {
    ws.send(msg)
        .map_err(|e| Error::Http(format!("WebSocket: {e}")))
}

fn set_timeout(ws: &WebSocket<MaybeTlsStream<TcpStream>>, t: Option<Duration>) -> Result<()> {
    if let MaybeTlsStream::Plain(s) = ws.get_ref() {
        s.set_read_timeout(t)?;
    }
    Ok(())
}

fn try_text(ws: &mut WebSocket<MaybeTlsStream<TcpStream>>) -> Result<Option<String>> {
    match ws.read() {
        Ok(Message::Text(t)) => Ok(Some(t.to_string())),
        Ok(_) => Ok(None),
        Err(tungstenite::Error::Io(e))
            if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
        {
            Ok(None)
        }
        Err(e) => Err(Error::Http(format!("WebSocket: {e}"))),
    }
}

fn next_text(ws: &mut WebSocket<MaybeTlsStream<TcpStream>>) -> Result<String> {
    loop {
        match ws.read() {
            Ok(Message::Text(t)) => return Ok(t.to_string()),
            Ok(Message::Close(_)) => {
                return Err(Error::Http("the reader closed the connection".into()))
            }
            Ok(_) => continue,
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
            {
                return Err(Error::Timeout("WebSocket reply".into()))
            }
            Err(e) => return Err(Error::Http(format!("WebSocket: {e}"))),
        }
    }
}

fn handle_progress(msg: &str, acked: u64) -> Result<u64> {
    if let Some(rest) = msg.strip_prefix("PROGRESS:") {
        return Ok(rest
            .split(':')
            .next()
            .and_then(|v| v.parse().ok())
            .unwrap_or(acked));
    }
    Err(ws_error(msg))
}

fn ws_error(msg: &str) -> Error {
    let text = msg.strip_prefix("ERROR:").unwrap_or(msg);
    if text.starts_with("File already exists") {
        return Error::Device("target_exists".into());
    }
    Error::Http(text.to_string())
}

impl Transport for WifiTransport {
    fn status(&mut self) -> Result<DeviceStatus> {
        let s: ApiStatus = self
            .agent
            .get(&self.url("/api/status"))
            .call()
            .map_err(http_err)?
            .into_json()?;
        Ok(DeviceStatus {
            transport: TransportKind::Wifi,
            address: self.host.clone(),
            device: s.device,
            firmware: s.version,
            free_heap: s.free_heap,
        })
    }

    fn list(&mut self, dir: &str) -> Result<Vec<Entry>> {
        let items: Vec<ApiEntry> = self
            .agent
            .get(&self.url("/api/files"))
            .query("path", dir)
            .call()
            .map_err(http_err)?
            .into_json()?;
        Ok(items
            .into_iter()
            .map(|e| Entry {
                path: join_path(dir, &e.name),
                name: e.name,
                is_dir: e.is_directory,
                size: e.size,
            })
            .collect())
    }

    fn mkdir(&mut self, path: &str) -> Result<()> {
        let (parent, name) = split_path(path);
        self.post_form("/mkdir", &[("name", &name), ("path", &parent)])
    }

    fn write(
        &mut self,
        path: &str,
        data: &[u8],
        overwrite: bool,
        progress: Progress,
    ) -> Result<()> {
        let (dir, name) = split_path(path);
        if overwrite {
            match self.remove(path) {
                Ok(()) => {}
                Err(Error::Http(m)) if m.starts_with("404") => {}
                Err(_) if !self.list(&dir)?.iter().any(|e| e.name == name) => {}
                Err(e) => return Err(e),
            }
        }
        self.ws_upload(&dir, &name, data, progress)
    }

    fn read(&mut self, path: &str, progress: Progress) -> Result<Vec<u8>> {
        let resp = self
            .agent
            .get(&self.url("/download"))
            .query("path", path)
            .call()
            .map_err(http_err)?;
        let total: u64 = resp
            .header("Content-Length")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let mut reader = resp.into_reader();
        let mut out = Vec::with_capacity(total as usize);
        let mut buf = [0u8; 16 * 1024];
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
            progress(out.len() as u64, total.max(out.len() as u64));
        }
        Ok(out)
    }

    fn remove(&mut self, path: &str) -> Result<()> {
        self.post_form("/delete", &[("path", path)])
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        let (from_dir, from_name) = split_path(from);
        let (to_dir, to_name) = split_path(to);
        if from_dir != to_dir {
            self.post_form("/move", &[("path", from), ("dest", &to_dir)])?;
        }
        if from_name != to_name {
            let current = join_path(&to_dir, &from_name);
            self.post_form("/rename", &[("path", &current), ("name", &to_name)])?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_reply() {
        let d = parse_discovery(
            "crosspoint (on crosspoint-ab12);81",
            "192.168.1.50".parse().unwrap(),
        )
        .unwrap();
        assert_eq!(d.ip, "192.168.1.50");
        assert_eq!(d.hostname, "crosspoint-ab12");
        assert_eq!(d.ws_port, 81);
        assert!(parse_discovery("hello", "1.1.1.1".parse().unwrap()).is_none());
    }

    #[test]
    fn progress_and_errors() {
        assert_eq!(handle_progress("PROGRESS:4096:10000", 0).unwrap(), 4096);
        assert!(
            matches!(handle_progress("ERROR:File already exists: a.epub", 0), Err(Error::Device(c)) if c == "target_exists")
        );
    }
}

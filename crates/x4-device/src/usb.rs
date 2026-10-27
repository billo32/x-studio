//! USB serial transport — the firmware's `CMND` protocol.
//!
//! Wire format (from `UsbSerialFileTransfer.cpp`, protocol=1):
//! - Request: `CMND` + one command byte + arguments.
//!   Paths are `u16 LE length` + UTF-8 bytes (max 255), sizes and CRCs are `u32 LE`.
//! - Replies are text lines (`OK`, `END`, `READY`, `ERR:<code>`, `STATUS:…`, `DIR:…`,
//!   `d|name`, `f|name|size|0`), except raw bytes inside write/read transfers.
//! - The same port carries the firmware's debug log, so unknown lines are skipped.
//! - `BUSY:<kind>:<n>/<m>` lines mean "still working" and extend the deadline.
//!
//! Commands: `S` status, `A` list, `K` mkdir, `W` write, `R` remove (recursive),
//! `N` rename, `T` read. Everything but status needs the home screen open.

use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::{join_path, DeviceStatus, Entry, Error, Progress, Result, Transport, TransportKind};

const MAGIC: &[u8; 4] = b"CMND";
const ACK: u8 = 0x06;
/// The firmware reads exactly this much per chunk and ACKs each chunk.
pub const CHUNK: usize = 256;
const MAX_PATH: usize = 255;

const SHORT: Duration = Duration::from_millis(1500);
/// The firmware answers a command only from its main loop, which can be busy for a few
/// seconds right after a transfer (cache cleanup, screen refresh). Waiting longer is safe:
/// the command is already queued on the device, and giving up early desynchronises it.
const HEADER: Duration = Duration::from_secs(20);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// Timeout label for a write that never started (safe to retry).
pub const START_OF_WRITE: &str = "start of write";
/// How long a refused command waits for the reader to return to its home screen.
const HOME_WAIT: Duration = Duration::from_secs(8);
const CHUNK_TIMEOUT: Duration = Duration::from_secs(45);
const CRC_TIMEOUT: Duration = Duration::from_secs(15);
const BUSY_EXTENSION: Duration = Duration::from_secs(8);

/// Espressif's USB vendor id (ESP32-C3 USB-Serial-JTAG).
pub const ESPRESSIF_VID: u16 = 0x303A;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortInfo {
    pub name: String,
    pub vid: Option<u16>,
    pub pid: Option<u16>,
    pub product: Option<String>,
    pub manufacturer: Option<String>,
    /// Looks like an ESP32 reader (Espressif VID).
    pub likely_reader: bool,
}

/// Lists USB serial ports, most likely readers first. On macOS only `/dev/cu.*` is kept:
/// opening `/dev/tty.*` blocks until carrier detect.
pub fn list_ports() -> Result<Vec<PortInfo>> {
    let mut out: Vec<PortInfo> = serialport::available_ports()?
        .into_iter()
        .filter(|p| !(cfg!(target_os = "macos") && p.port_name.starts_with("/dev/tty.")))
        .filter_map(|p| match p.port_type {
            serialport::SerialPortType::UsbPort(u) => Some(PortInfo {
                likely_reader: u.vid == ESPRESSIF_VID,
                vid: Some(u.vid),
                pid: Some(u.pid),
                product: u.product,
                manufacturer: u.manufacturer,
                name: p.port_name,
            }),
            _ => None,
        })
        .collect();
    out.sort_by_key(|p| !p.likely_reader);
    Ok(out)
}

/// Opens a serial port and checks that a CrossInk/CrossPoint firmware answers on it.
pub fn open(port_name: &str) -> Result<UsbTransport<Box<dyn serialport::SerialPort>>> {
    // Baud is ignored by USB CDC. DTR/RTS are left at the OS defaults (both asserted),
    // the same state Web Serial uses: toggling them in the wrong order resets the ESP32-C3.
    let port = serialport::new(port_name, 115_200)
        .timeout(Duration::from_millis(30))
        .open()?;
    let mut t = UsbTransport::new(port, port_name.to_string());
    t.status()?;
    Ok(t)
}

/// The `CMND` protocol over any byte stream (a real serial port, or a fake in tests).
/// Reads must return quickly when no data is available (timeout or `Ok(0)`).
pub struct UsbTransport<P: Read + Write + Send> {
    port: P,
    name: String,
    rx: VecDeque<u8>,
    /// Optional protocol log: commands sent and every line received, firmware logs included.
    log: Option<Box<dyn Write + Send>>,
}

impl<P: Read + Write + Send> UsbTransport<P> {
    pub fn new(port: P, name: String) -> Self {
        Self {
            port,
            name,
            rx: VecDeque::new(),
            log: None,
        }
    }

    pub fn set_log(&mut self, log: Box<dyn Write + Send>) {
        self.log = Some(log);
    }

    fn log(&mut self, msg: std::fmt::Arguments) {
        if let Some(l) = self.log.as_mut() {
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            let _ = writeln!(l, "{t} {msg}");
            let _ = l.flush();
        }
    }

    /// Pulls whatever is available into the receive buffer. Returns bytes read.
    fn fill(&mut self) -> Result<usize> {
        let mut tmp = [0u8; 4096];
        match self.port.read(&mut tmp) {
            Ok(0) => {
                std::thread::sleep(Duration::from_millis(1));
                Ok(0)
            }
            Ok(n) => {
                self.rx.extend(&tmp[..n]);
                Ok(n)
            }
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::Interrupted
                ) =>
            {
                Ok(0)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Drops stale bytes (log noise, leftovers of an aborted command).
    fn drain(&mut self) -> Result<()> {
        self.rx.clear();
        let until = Instant::now() + Duration::from_millis(40);
        while Instant::now() < until {
            self.fill()?;
            self.rx.clear();
        }
        Ok(())
    }

    /// Writes all bytes. The serial port's short read timeout also applies to writes, so a
    /// momentarily busy device makes `write` time out; keep trying until `WRITE_TIMEOUT`.
    fn send(&mut self, bytes: &[u8]) -> Result<()> {
        let deadline = Instant::now() + WRITE_TIMEOUT;
        let mut off = 0;
        while off < bytes.len() {
            match self.port.write(&bytes[off..]) {
                Ok(0) => {}
                Ok(n) => {
                    off += n;
                    continue;
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::Interrupted
                    ) => {}
                Err(e) => return Err(e.into()),
            }
            if Instant::now() >= deadline {
                self.log(format_args!("! write stalled at {off}/{}", bytes.len()));
                return Err(Error::Timeout("sending to the reader".into()));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        loop {
            match self.port.flush() {
                Ok(()) => return Ok(()),
                Err(e)
                    if matches!(
                        e.kind(),
                        ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::Interrupted
                    ) && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(2))
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    fn send_command(&mut self, cmd: u8, paths: &[&str], extra: &[u8]) -> Result<()> {
        self.drain()?;
        let mut msg = Vec::with_capacity(8 + extra.len());
        msg.extend_from_slice(MAGIC);
        msg.push(cmd);
        for p in paths {
            let bytes = p.as_bytes();
            if bytes.is_empty() || bytes.len() > MAX_PATH {
                return Err(Error::Device("path_too_long".into()));
            }
            msg.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
            msg.extend_from_slice(bytes);
        }
        msg.extend_from_slice(extra);
        self.log(format_args!(
            "> {} {}{}",
            cmd as char,
            paths.join(" -> "),
            if extra.len() == 4 {
                format!(" [{}]", u32::from_le_bytes(extra.try_into().unwrap()))
            } else {
                String::new()
            }
        ));
        self.send(&msg)
    }

    /// Next protocol line other than `STATUS:` (a late answer to an earlier ping).
    fn line(&mut self, timeout: Duration, what: &str) -> Result<String> {
        self.line_matching(timeout, what, |l| !l.starts_with("STATUS:"))
    }

    /// Next protocol line accepted by `accept`. Skips firmware log lines and unaccepted protocol
    /// lines, honours `BUSY:` keep-alives, turns `ERR:` into errors.
    fn line_matching(
        &mut self,
        timeout: Duration,
        what: &str,
        accept: impl Fn(&str) -> bool,
    ) -> Result<String> {
        let mut deadline = Instant::now() + timeout;
        loop {
            if let Some(pos) = self.rx.iter().position(|&b| b == b'\n') {
                let raw: Vec<u8> = self.rx.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&raw)
                    .trim_matches(|c| c == '\r' || c == '\n')
                    .to_string();
                self.log(format_args!("< {line}"));
                if line.starts_with("BUSY:") {
                    deadline = Instant::now() + BUSY_EXTENSION;
                    continue;
                }
                if let Some(code) = line.strip_prefix("ERR:") {
                    return Err(Error::Device(code.to_string()));
                }
                if is_protocol_line(&line) && accept(&line) {
                    return Ok(line);
                }
                continue; // firmware log output, or a stale reply
            }
            if Instant::now() >= deadline {
                self.log(format_args!("! timeout: {what}"));
                return Err(Error::Timeout(what.to_string()));
            }
            self.fill()?;
        }
    }

    fn expect_ok(&mut self, timeout: Duration, what: &str) -> Result<()> {
        match self.line(timeout, what)?.as_str() {
            "OK" => Ok(()),
            other => Err(Error::Protocol(other.to_string())),
        }
    }

    /// Waits for the 0x06 chunk acknowledgement. Text lines seen meanwhile are still
    /// interpreted, so an `ERR:` instead of an ACK fails fast.
    fn ack(&mut self, timeout: Duration) -> Result<()> {
        let mut deadline = Instant::now() + timeout;
        let mut pending: Vec<u8> = Vec::new();
        loop {
            while let Some(b) = self.rx.pop_front() {
                if b == ACK {
                    // Anything half-read before the ACK goes back so a later line() sees it.
                    for &p in pending.iter().rev() {
                        self.rx.push_front(p);
                    }
                    return Ok(());
                }
                if b == b'\n' {
                    let line = String::from_utf8_lossy(&pending).trim().to_string();
                    pending.clear();
                    self.log(format_args!("< {line}"));
                    if let Some(code) = line.strip_prefix("ERR:") {
                        return Err(Error::Device(code.to_string()));
                    }
                    if line.starts_with("BUSY:") {
                        deadline = Instant::now() + BUSY_EXTENSION;
                    }
                } else {
                    pending.push(b);
                }
            }
            if Instant::now() >= deadline {
                self.log(format_args!("! timeout: chunk acknowledgement"));
                return Err(Error::Timeout("chunk acknowledgement".into()));
            }
            self.fill()?;
        }
    }

    fn raw(&mut self, n: usize, timeout: Duration) -> Result<Vec<u8>> {
        let mut deadline = Instant::now() + timeout;
        let mut out = Vec::with_capacity(n);
        while out.len() < n {
            let take = (n - out.len()).min(self.rx.len());
            if take > 0 {
                out.extend(self.rx.drain(..take));
                deadline = Instant::now() + timeout;
                continue;
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout(format!("data {}/{} bytes", out.len(), n)));
            }
            self.fill()?;
        }
        Ok(out)
    }
}

fn is_protocol_line(line: &str) -> bool {
    matches!(line, "OK" | "END" | "READY")
        || line.starts_with("STATUS:")
        || line.starts_with("DIR:")
        || line.starts_with("d|")
        || line.starts_with("f|")
}

/// `STATUS:protocol=1,device=x4,firmware=1.6.1,free=123,largest=45`
fn parse_status(line: &str, address: &str) -> Result<DeviceStatus> {
    let body = line
        .strip_prefix("STATUS:")
        .ok_or_else(|| Error::Protocol(line.into()))?;
    let field = |key: &str| {
        body.split(',')
            .filter_map(|kv| kv.split_once('='))
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.to_string())
    };
    if field("protocol").as_deref() != Some("1") {
        return Err(Error::Unsupported(format!("USB protocol version: {line}")));
    }
    Ok(DeviceStatus {
        transport: TransportKind::Usb,
        address: address.to_string(),
        device: field("device").unwrap_or_default(),
        firmware: field("firmware").unwrap_or_default(),
        free_heap: field("free").and_then(|v| v.parse().ok()),
    })
}

impl<P: Read + Write + Send> Transport for UsbTransport<P> {
    fn status(&mut self) -> Result<DeviceStatus> {
        self.send_command(b'S', &[], &[])?;
        let line = self.line_matching(SHORT, "status", |l| l.starts_with("STATUS:"))?;
        parse_status(&line, &self.name)
    }

    fn wait_ready(&mut self, max: Duration) -> Result<()> {
        let deadline = Instant::now() + max;
        loop {
            self.send_command(b'S', &[], &[])?;
            let result = self.line_matching(Duration::from_secs(3), "status", |l| {
                l.starts_with("STATUS:")
            });
            match result {
                Ok(_) => return Ok(()),
                Err(e)
                    if (e.is_not_on_home() || matches!(e, Error::Timeout(_)))
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(400));
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn list(&mut self, dir: &str) -> Result<Vec<Entry>> {
        self.home_retry(|s| s.list_once(dir))
    }

    fn mkdir(&mut self, path: &str) -> Result<()> {
        self.home_retry(|s| s.mkdir_once(path))
    }

    fn write(
        &mut self,
        path: &str,
        data: &[u8],
        overwrite: bool,
        progress: Progress,
    ) -> Result<()> {
        self.home_retry(|s| s.write_once(path, data, overwrite, &mut *progress))
    }

    fn read(&mut self, path: &str, progress: Progress) -> Result<Vec<u8>> {
        self.home_retry(|s| s.read_once(path, &mut *progress))
    }

    fn remove(&mut self, path: &str) -> Result<()> {
        self.home_retry(|s| s.remove_once(path))
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        self.home_retry(|s| s.rename_once(from, to))
    }
}

/// Single attempts of each command, plus the retry used by the `Transport` methods.
impl<P: Read + Write + Send> UsbTransport<P> {
    /// The firmware refuses file commands while it is off the home screen, which also happens
    /// for a moment right after it receives a file. A refused command was not executed, so it
    /// is safe to wait for the home screen and send it again.
    fn home_retry<T>(&mut self, mut f: impl FnMut(&mut Self) -> Result<T>) -> Result<T> {
        match f(self) {
            Err(e) if e.is_not_on_home() => {
                self.log(format_args!("! not on home, waiting"));
                self.wait_ready(HOME_WAIT)?;
                f(self)
            }
            other => other,
        }
    }

    fn list_once(&mut self, dir: &str) -> Result<Vec<Entry>> {
        self.send_command(b'A', &[dir], &[])?;
        let mut entries = Vec::new();
        loop {
            let line = self.line(HEADER, "file list")?;
            if line == "END" {
                break;
            }
            if line.starts_with("DIR:") {
                continue;
            }
            let mut parts = line.splitn(4, '|');
            match (parts.next(), parts.next()) {
                (Some("d"), Some(name)) => entries.push(Entry {
                    path: join_path(dir, name),
                    name: name.to_string(),
                    is_dir: true,
                    size: 0,
                }),
                (Some("f"), Some(name)) => entries.push(Entry {
                    path: join_path(dir, name),
                    name: name.to_string(),
                    is_dir: false,
                    size: parts.next().and_then(|s| s.parse().ok()).unwrap_or(0),
                }),
                _ => return Err(Error::Protocol(line)),
            }
        }
        Ok(entries)
    }

    fn mkdir_once(&mut self, path: &str) -> Result<()> {
        self.send_command(b'K', &[path], &[])?;
        self.expect_ok(HEADER, "create folder")
    }

    fn write_once(
        &mut self,
        path: &str,
        data: &[u8],
        overwrite: bool,
        progress: Progress,
    ) -> Result<()> {
        if !overwrite {
            let (dir, name) = crate::split_path(path);
            if self
                .list(&dir)
                .map(|l| l.iter().any(|e| e.name == name))
                .unwrap_or(false)
            {
                return Err(Error::Device("target_exists".into()));
            }
        }
        let size = u32::try_from(data.len()).map_err(|_| Error::Device("too_large".into()))?;
        self.send_command(b'W', &[path], &size.to_le_bytes())?;
        match self.line(HEADER, START_OF_WRITE)?.as_str() {
            "READY" => {}
            other => return Err(Error::Protocol(other.into())),
        }
        let total = data.len() as u64;
        let mut done = 0u64;
        progress(0, total);
        // Every chunk is ACKed: the firmware ACKs all but the last chunk on receipt, and
        // the last one after the file is flushed to the SD card.
        for chunk in data.chunks(CHUNK) {
            self.send(chunk)?;
            self.ack(CHUNK_TIMEOUT)?;
            done += chunk.len() as u64;
            progress(done, total);
        }
        let crc = crc32fast::hash(data);
        self.send(&crc.to_le_bytes())?;
        self.expect_ok(CRC_TIMEOUT, "CRC check")
    }

    fn read_once(&mut self, path: &str, progress: Progress) -> Result<Vec<u8>> {
        self.send_command(b'T', &[path], &[])?;
        match self.line(HEADER, "start of read")?.as_str() {
            "READY" => {}
            other => return Err(Error::Protocol(other.into())),
        }
        let size = u32::from_le_bytes(self.raw(4, HEADER)?.try_into().unwrap()) as usize;
        let mut data = Vec::with_capacity(size);
        progress(0, size as u64);
        while data.len() < size {
            let n = (size - data.len()).min(4096);
            data.extend(self.raw(n, CHUNK_TIMEOUT)?);
            progress(data.len() as u64, size as u64);
        }
        let crc = u32::from_le_bytes(self.raw(4, CRC_TIMEOUT)?.try_into().unwrap());
        if crc != crc32fast::hash(&data) {
            return Err(Error::Device("crc".into()));
        }
        Ok(data)
    }

    fn remove_once(&mut self, path: &str) -> Result<()> {
        self.send_command(b'R', &[path], &[])?;
        self.expect_ok(CHUNK_TIMEOUT, "delete")
    }

    fn rename_once(&mut self, from: &str, to: &str) -> Result<()> {
        self.send_command(b'N', &[from, to], &[])?;
        self.expect_ok(HEADER, "rename")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_line() {
        let s = parse_status(
            "STATUS:protocol=1,device=x4,firmware=1.6.1,free=91234,largest=40000",
            "/dev/cu.x",
        )
        .unwrap();
        assert_eq!(s.device, "x4");
        assert_eq!(s.firmware, "1.6.1");
        assert_eq!(s.free_heap, Some(91234));
        assert!(parse_status("STATUS:protocol=2,device=x4", "p").is_err());
    }
}

//! Transports for Xteink readers running CrossInk (or upstream CrossPoint).
//!
//! Two ways to reach the device, both reverse-engineered from the open firmware:
//! - [`usb`]: the binary `CMND` protocol on the USB CDC serial port
//!   (`src/network/UsbSerialFileTransfer.cpp`). Works only while the device
//!   is on its home screen.
//! - [`wifi`]: the File Transfer web server (HTTP :80, WebSocket uploads :81,
//!   UDP discovery :8134). Works only while File Transfer is open on the device.

pub mod error;
pub mod usb;
pub mod wifi;

pub use error::{Error, Result};

use serde::Serialize;

/// One file or folder on the device's SD card.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TransportKind {
    Usb,
    Wifi,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceStatus {
    pub transport: TransportKind,
    /// Human label of the connection: serial port name or host.
    pub address: String,
    /// Hardware model as the firmware reports it ("x4", "X4", "x4classic"…).
    pub device: String,
    pub firmware: String,
    pub free_heap: Option<u64>,
}

/// Progress callback: (bytes done, bytes total).
pub type Progress<'a> = &'a mut dyn FnMut(u64, u64);

/// Common operations both transports support. All calls block.
pub trait Transport: Send {
    fn status(&mut self) -> Result<DeviceStatus>;
    fn list(&mut self, dir: &str) -> Result<Vec<Entry>>;
    fn mkdir(&mut self, path: &str) -> Result<()>;
    /// Writes a whole file. `overwrite = false` fails if the file exists.
    fn write(&mut self, path: &str, data: &[u8], overwrite: bool, progress: Progress)
        -> Result<()>;
    fn read(&mut self, path: &str, progress: Progress) -> Result<Vec<u8>>;
    fn remove(&mut self, path: &str) -> Result<()>;
    fn rename(&mut self, from: &str, to: &str) -> Result<()>;
    /// Waits until the reader answers and accepts file commands (up to `max`).
    fn wait_ready(&mut self, _max: std::time::Duration) -> Result<()> {
        self.status().map(|_| ())
    }
}

/// Writes one file as part of a batch: waits for the reader to settle after the previous
/// file, and retries once if the write was refused before any data was sent.
pub fn write_in_batch(
    t: &mut dyn Transport,
    path: &str,
    data: &[u8],
    overwrite: bool,
    progress: Progress,
) -> Result<()> {
    const SETTLE: std::time::Duration = std::time::Duration::from_secs(20);
    t.wait_ready(SETTLE)?;
    match t.write(path, data, overwrite, progress) {
        Err(e)
            if e.is_not_on_home()
                || matches!(&e, Error::Timeout(w) if w == usb::START_OF_WRITE) =>
        {
            t.wait_ready(SETTLE)?;
            t.write(path, data, overwrite, progress)
        }
        other => other,
    }
}

/// Joins a directory and a name into a device path.
pub fn join_path(dir: &str, name: &str) -> String {
    let dir = dir.trim_end_matches('/');
    if dir.is_empty() {
        format!("/{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// Splits a device path into (parent, name).
pub fn split_path(path: &str) -> (String, String) {
    let path = path.trim_end_matches('/');
    match path.rfind('/') {
        Some(0) | None => ("/".to_string(), path.trim_start_matches('/').to_string()),
        Some(i) => (path[..i].to_string(), path[i + 1..].to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(join_path("/", "a.epub"), "/a.epub");
        assert_eq!(join_path("/Books/", "a.epub"), "/Books/a.epub");
        assert_eq!(split_path("/a.epub"), ("/".into(), "a.epub".into()));
        assert_eq!(
            split_path("/Books/Sci/a.epub"),
            ("/Books/Sci".into(), "a.epub".into())
        );
    }
}

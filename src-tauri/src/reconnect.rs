//! Getting the reader back after it drops off USB.
//!
//! The reader disappears from USB when it goes to sleep, reboots or is unplugged; the open
//! serial handle then fails with "Broken pipe". The port usually comes back under the same
//! name once the reader wakes up, so the app remembers it and opens it again.
//!
//! Reopening is not free: closing and opening the port changes DTR/RTS, which can reset an
//! ESP32-C3. So it happens only on an unambiguous "port is gone" error, at most once per
//! action, and every attempt is written to usb.log.

use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tauri::AppHandle;
use x4_device::{usb, Transport};

/// How long a write waits for a sleeping reader's port to reappear.
const COME_BACK: Duration = Duration::from_secs(15);

static USB_PORT: Mutex<Option<String>> = Mutex::new(None);
static APP: OnceLock<AppHandle> = OnceLock::new();

/// The port of the current USB connection (called on connect).
pub(crate) fn remember_usb(app: &AppHandle, port: &str) {
    let _ = APP.set(app.clone());
    if let Ok(mut p) = USB_PORT.lock() {
        *p = Some(port.to_owned());
    }
}

/// No USB connection to come back to (Wi-Fi connect, manual disconnect).
pub(crate) fn forget_usb() {
    if let Ok(mut p) = USB_PORT.lock() {
        *p = None;
    }
}

fn log(app: Option<&AppHandle>, msg: &str) -> Option<Box<dyn Write + Send>> {
    let mut log = app.or(APP.get()).and_then(crate::usb_log)?;
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let _ = writeln!(log, "{t} ! {msg}");
    let _ = log.flush();
    Some(log)
}

/// Opens the remembered USB port again into `slot` after `cause` killed the old handle.
/// With `wait`, a port that is missing (reader asleep) is polled for up to [`COME_BACK`];
/// any other failure returns at once. The error text is meant for the user.
pub(crate) fn reopen(
    app: Option<&AppHandle>,
    slot: &mut Option<Box<dyn Transport>>,
    wait: bool,
    cause: &x4_device::Error,
) -> Result<(), String> {
    let port = USB_PORT
        .lock()
        .ok()
        .and_then(|p| p.clone())
        .ok_or("The reader's connection was lost. Connect it again.")?;
    *slot = None; // close the dead handle first
    log(app, &format!("port dropped ({cause}); reopening {port}"));
    let deadline = Instant::now() + if wait { COME_BACK } else { Duration::ZERO };
    loop {
        match usb::open(&port) {
            Ok(mut t) => {
                if let Some(l) = log(app, "reopened") {
                    t.set_log(l);
                }
                *slot = Some(Box::new(t));
                return Ok(());
            }
            Err(e) if e.is_port_missing() && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(500))
            }
            Err(e) => {
                log(app, &format!("reopen failed: {e}"));
                return Err(if e.is_port_missing() {
                    "The reader dropped off USB (it may have gone to sleep). Wake it up on the home screen and try again.".into()
                } else {
                    format!("Could not reconnect to the reader: {e}")
                });
            }
        }
    }
}

/// [`x4_device::write_in_batch`] that survives the reader having dropped off USB since the
/// last command: the port is opened again (waiting for a sleeping reader) and the write
/// repeated once.
pub(crate) fn write_reconnecting(
    app: Option<&AppHandle>,
    slot: &mut Option<Box<dyn Transport>>,
    path: &str,
    data: &[u8],
    overwrite: bool,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<(), String> {
    let t = slot
        .as_mut()
        .ok_or_else(|| x4_device::Error::NotConnected.to_string())?;
    match x4_device::write_in_batch(t.as_mut(), path, data, overwrite, progress) {
        Err(e) if e.is_disconnected() => {
            reopen(app, slot, true, &e)?;
            let t = slot
                .as_mut()
                .ok_or_else(|| x4_device::Error::NotConnected.to_string())?;
            x4_device::write_in_batch(t.as_mut(), path, data, overwrite, progress)
                .map_err(|e| e.to_string())
        }
        other => other.map_err(|e| e.to_string()),
    }
}

//! Tauri commands: device connection, file management and an upload queue.
//! Device calls block, so each command runs on the blocking pool and the
//! connection lives behind a mutex (one operation at a time, as the firmware expects).

mod library;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use x4_device::{join_path, usb, wifi, DeviceStatus, Entry, Transport};

pub(crate) struct AppState {
    device: Arc<Mutex<Option<Box<dyn Transport>>>>,
    cache: library::Cache,
}

pub(crate) type CmdResult<T> = Result<T, String>;

/// Runs `f` with the connected device on the blocking pool.
pub(crate) async fn with_device<T, F>(state: &State<'_, AppState>, f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce(&mut dyn Transport) -> x4_device::Result<T> + Send + 'static,
{
    let device = state.device.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut guard = device
            .lock()
            .map_err(|_| "Device state is corrupted".to_string())?;
        let t = guard
            .as_mut()
            .ok_or_else(|| x4_device::Error::NotConnected.to_string())?;
        f(t.as_mut()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn list_serial_ports() -> CmdResult<Vec<usb::PortInfo>> {
    tauri::async_runtime::spawn_blocking(|| usb::list_ports().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn discover_wifi() -> CmdResult<Vec<wifi::FoundDevice>> {
    tauri::async_runtime::spawn_blocking(|| {
        wifi::discover(Duration::from_millis(1800)).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn connect_usb(
    app: AppHandle,
    state: State<'_, AppState>,
    port: String,
) -> CmdResult<DeviceStatus> {
    let device = state.device.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut guard = device.lock().map_err(|e| e.to_string())?;
        *guard = None; // release a previously opened port first
        let mut t = usb::open(&port).map_err(|e| e.to_string())?;
        if let Some(log) = usb_log(&app) {
            t.set_log(log);
        }
        let status = t.status().map_err(|e| e.to_string())?;
        *guard = Some(Box::new(t));
        Ok(status)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Protocol log for diagnosing transfers: ~/Library/Logs/dev.tls1.xstudio/usb.log
/// (the previous one is kept as usb.log.1 once it passes 2 MB).
fn usb_log(app: &AppHandle) -> Option<Box<dyn std::io::Write + Send>> {
    let dir = app.path().app_log_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("usb.log");
    if std::fs::metadata(&path)
        .map(|m| m.len() > 2 * 1024 * 1024)
        .unwrap_or(false)
    {
        let _ = std::fs::rename(&path, dir.join("usb.log.1"));
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()?;
    Some(Box::new(file))
}

#[tauri::command]
async fn connect_wifi(
    state: State<'_, AppState>,
    host: String,
    ws_port: Option<u16>,
) -> CmdResult<DeviceStatus> {
    let device = state.device.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut guard = device.lock().map_err(|e| e.to_string())?;
        *guard = None;
        let mut t = wifi::open(&host, ws_port).map_err(|e| e.to_string())?;
        let status = t.status().map_err(|e| e.to_string())?;
        *guard = Some(Box::new(t));
        Ok(status)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn disconnect(state: State<'_, AppState>) -> CmdResult<()> {
    *state.device.lock().map_err(|e| e.to_string())? = None;
    Ok(())
}

#[tauri::command]
async fn device_status(state: State<'_, AppState>) -> CmdResult<DeviceStatus> {
    with_device(&state, |t| t.status()).await
}

#[tauri::command]
async fn list_dir(state: State<'_, AppState>, path: String) -> CmdResult<Vec<Entry>> {
    with_device(&state, move |t| {
        let mut entries = t.list(&path)?;
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(entries)
    })
    .await
}

#[tauri::command]
async fn make_dir(state: State<'_, AppState>, path: String) -> CmdResult<()> {
    with_device(&state, move |t| t.mkdir(&path)).await
}

#[tauri::command]
async fn remove_path(state: State<'_, AppState>, path: String) -> CmdResult<()> {
    with_device(&state, move |t| t.remove(&path)).await
}

#[tauri::command]
async fn rename_path(state: State<'_, AppState>, from: String, to: String) -> CmdResult<()> {
    with_device(&state, move |t| t.rename(&from, &to)).await
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TransferEvent {
    id: String,
    name: String,
    done: u64,
    total: u64,
    /// "running" | "done" | "error"
    state: &'static str,
    error: Option<String>,
}

/// Emits progress at most ~15 times a second so the UI is not flooded.
pub(crate) fn throttled_emitter(app: AppHandle, id: String, name: String) -> impl FnMut(u64, u64) {
    let mut last = Instant::now() - Duration::from_secs(1);
    move |done, total| {
        if done == total || last.elapsed() >= Duration::from_millis(66) {
            last = Instant::now();
            let _ = app.emit(
                "transfer",
                TransferEvent {
                    id: id.clone(),
                    name: name.clone(),
                    done,
                    total,
                    state: "running",
                    error: None,
                },
            );
        }
    }
}

/// Uploads local files into `dest_dir`, one by one. `id`s come from the UI so it can
/// track queue rows; progress goes out as `transfer` events.
#[tauri::command]
async fn upload_files(
    app: AppHandle,
    state: State<'_, AppState>,
    items: Vec<(String, String)>,
    dest_dir: String,
    overwrite: bool,
) -> CmdResult<()> {
    let device = state.device.clone();
    tauri::async_runtime::spawn_blocking(move || {
        for (id, local) in items {
            let path = PathBuf::from(&local);
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| local.clone());
            let result = (|| -> Result<(), String> {
                let data = std::fs::read(&path).map_err(|e| format!("{local}: {e}"))?;
                let mut guard = device.lock().map_err(|e| e.to_string())?;
                let t = guard
                    .as_mut()
                    .ok_or_else(|| x4_device::Error::NotConnected.to_string())?;
                let mut progress = throttled_emitter(app.clone(), id.clone(), name.clone());
                x4_device::write_in_batch(
                    t.as_mut(),
                    &join_path(&dest_dir, &name),
                    &data,
                    overwrite,
                    &mut progress,
                )
                .map_err(|e| e.to_string())
            })();
            let (state, error) = match result {
                Ok(()) => ("done", None),
                Err(e) => ("error", Some(e)),
            };
            let _ = app.emit(
                "transfer",
                TransferEvent {
                    id,
                    name,
                    done: 0,
                    total: 0,
                    state,
                    error,
                },
            );
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn download_file(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    save_to: String,
) -> CmdResult<()> {
    let name = x4_device::split_path(&path).1;
    let id = format!("dl:{path}");
    let (app2, id2, name2) = (app.clone(), id.clone(), name.clone());
    let result = with_device(&state, move |t| {
        let mut progress = throttled_emitter(app2, id2, name2);
        t.read(&path, &mut progress)
    })
    .await
    .and_then(|data| std::fs::write(&save_to, data).map_err(|e| e.to_string()));
    let (st, error) = match &result {
        Ok(()) => ("done", None),
        Err(e) => ("error", Some(e.clone())),
    };
    let _ = app.emit(
        "transfer",
        TransferEvent {
            id,
            name,
            done: 0,
            total: 0,
            state: st,
            error,
        },
    );
    result
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let cache = library::Cache::new(app.handle());
            app.manage(AppState {
                device: Arc::default(),
                cache,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_serial_ports,
            discover_wifi,
            connect_usb,
            connect_wifi,
            disconnect,
            device_status,
            list_dir,
            make_dir,
            remove_path,
            rename_path,
            upload_files,
            download_file,
            library::scan_device,
            library::book_info,
            library::book_cover,
            library::image_thumb,
            library::load_local_image,
            library::save_book,
            library::export_book,
        ])
        .run(tauri::generate_context!())
        .expect("error while running X Studio");
}

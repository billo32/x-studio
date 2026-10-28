//! Tauri commands: device connection, file management and an upload queue.
//! Device calls block, so each command runs on the blocking pool and the
//! connection lives behind a mutex (one operation at a time, as the firmware expects).

mod fonts;
mod library;
mod plugins;
mod reconnect;

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
        match f(t.as_mut()) {
            // The handle died (the reader slept or was replugged): reconnect now so the
            // next action works, and say so instead of a bare "Broken pipe".
            Err(e) if e.is_disconnected() => {
                Err(match reconnect::reopen(None, &mut guard, false, &e) {
                    Ok(()) => {
                        "The reader was reconnected after dropping off USB. Try again.".into()
                    }
                    Err(msg) => msg,
                })
            }
            other => other.map_err(|e| e.to_string()),
        }
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
        reconnect::remember_usb(&app, &port);
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
        reconnect::forget_usb();
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
    reconnect::forget_usb();
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

/// Final `transfer` event for a queue row: done, or the error.
pub(crate) fn emit_finished(app: &AppHandle, id: String, name: String, error: Option<String>) {
    let _ = app.emit(
        "transfer",
        TransferEvent {
            id,
            name,
            done: 0,
            total: 0,
            state: if error.is_some() { "error" } else { "done" },
            error,
        },
    );
}

/// Writes local files into `dest_dir` one by one. `prepare` turns a local path into the
/// name and bytes to write. `id`s come from the UI so it can track queue rows.
fn upload_each(
    app: AppHandle,
    device: Arc<Mutex<Option<Box<dyn Transport>>>>,
    items: Vec<(String, String)>,
    dest_dir: String,
    overwrite: bool,
    prepare: impl Fn(&str) -> Result<(String, Vec<u8>), String>,
) {
    for (id, local) in items {
        let shown = PathBuf::from(&local)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| local.clone());
        let result = (|| -> Result<(), String> {
            let (name, data) = prepare(&local)?;
            let mut guard = device.lock().map_err(|e| e.to_string())?;
            let mut progress = throttled_emitter(app.clone(), id.clone(), shown.clone());
            crate::reconnect::write_reconnecting(
                Some(&app),
                &mut guard,
                &join_path(&dest_dir, &name),
                &data,
                overwrite,
                &mut progress,
            )
        })();
        emit_finished(&app, id, shown, result.err());
    }
}

/// Uploads local files into `dest_dir` as they are; progress goes out as `transfer` events.
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
        upload_each(app, device, items, dest_dir, overwrite, |local| {
            let name = PathBuf::from(local)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| local.to_string());
            let data = std::fs::read(local).map_err(|e| format!("{local}: {e}"))?;
            Ok((name, data))
        })
    })
    .await
    .map_err(|e| e.to_string())
}

/// Turns pictures into sleep screens — `width`×`height`, filled and cropped, 8-bit grayscale
/// BMP as the firmware wants — and uploads them into `dest_dir` as `<name>.bmp`.
#[tauri::command]
async fn upload_sleep_images(
    app: AppHandle,
    state: State<'_, AppState>,
    items: Vec<(String, String)>,
    dest_dir: String,
    width: u32,
    height: u32,
) -> CmdResult<()> {
    let device = state.device.clone();
    tauri::async_runtime::spawn_blocking(move || {
        upload_each(app, device, items, dest_dir, true, |local| {
            let path = PathBuf::from(local);
            let stem = path
                .file_stem()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "sleep".into());
            let bytes = std::fs::read(&path).map_err(|e| format!("{local}: {e}"))?;
            let bmp = x4_books::imaging::sleep_screen(&bytes, width, height)
                .map_err(|e| format!("{stem}: {e}"))?;
            Ok((format!("{stem}.bmp"), bmp))
        })
    })
    .await
    .map_err(|e| e.to_string())
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
        .register_uri_scheme_protocol(plugins::SCHEME, plugins::serve)
        .setup(|app| {
            app.manage(plugins::Plugins::new(app.handle()));
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
            upload_sleep_images,
            download_file,
            plugins::plugins_list,
            plugins::plugin_inspect,
            plugins::plugin_install,
            plugins::plugin_remove,
            plugins::plugin_set_enabled,
            plugins::plugin_call,
            plugins::plugin_save_file,
            plugins::read_book_bytes,
            plugins::write_book_bytes,
            library::scan_device,
            library::book_info,
            library::book_cover,
            library::image_thumb,
            library::load_local_image,
            library::save_book,
            library::export_book,
            fonts::font_catalog,
            fonts::install_font,
        ])
        .run(tauri::generate_context!())
        .expect("error while running X Studio");
}

//! Plugins: installed from ZIP, served over `xsplugin://` into a sandboxed iframe, and given
//! an API (`plugin_call`) whose every method is checked against the plugin's manifest.
//!
//! Layout under the app data folder:
//!   plugins/<id>/…                 the unpacked plugin
//!   plugin-data/<id>/storage.json  its settings (readable by the plugin)
//!   plugin-data/<id>/secrets.json  its tokens (owner-only file, never handed back to it)
//!   plugins.json                   which plugins are switched off

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tauri::http::{Request, Response, StatusCode};
use tauri::{
    AppHandle, Manager, Runtime, State, UriSchemeContext, WebviewUrl, WebviewWindowBuilder,
};
use x4_plugins::{DeviceAccess, Manifest, Package, PermissionLine};

use crate::{AppState, CmdResult};

pub const SCHEME: &str = "xsplugin";
const SDK_JS: &str = include_str!("../../plugin-sdk/xstudio.js");
const SDK_CSS: &str = include_str!("../../plugin-sdk/xstudio.css");
const MAX_STORAGE: usize = 1024 * 1024;
const MAX_RESPONSE: u64 = 100 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;
const AUTH_WINDOW: &str = "plugin-auth";
const AUTH_TIMEOUT: Duration = Duration::from_secs(15 * 60);

fn err(e: impl ToString) -> String {
    e.to_string()
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

pub struct Plugins {
    root: PathBuf,
    data: PathBuf,
    registry: PathBuf,
    /// Serialises writes to storage, secrets and the registry.
    write_lock: Mutex<()>,
    transfer_seq: AtomicU64,
    signing_in: AtomicBool,
}

impl Plugins {
    pub fn new<R: Runtime>(app: &AppHandle<R>) -> Self {
        let base = app
            .path()
            .app_data_dir()
            .unwrap_or_else(|_| std::env::temp_dir().join("x-studio"));
        Plugins {
            root: base.join("plugins"),
            data: base.join("plugin-data"),
            registry: base.join("plugins.json"),
            write_lock: Mutex::new(()),
            transfer_seq: AtomicU64::new(0),
            signing_in: AtomicBool::new(false),
        }
    }

    fn dir(&self, id: &str) -> Result<PathBuf, String> {
        if !x4_plugins::valid_id(id) {
            return Err(format!("unknown plugin {id}"));
        }
        Ok(self.root.join(id))
    }

    fn manifest(&self, id: &str) -> Result<Manifest, String> {
        x4_plugins::installed_manifest(&self.dir(id)?)
            .map_err(|_| format!("plugin {id} is not installed"))
    }

    fn disabled(&self) -> Vec<String> {
        std::fs::read(&self.registry)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| serde_json::from_value(v["disabled"].clone()).ok())
            .unwrap_or_default()
    }

    fn set_disabled(&self, id: &str, off: bool) -> Result<(), String> {
        let _g = self.write_lock.lock().map_err(err)?;
        let mut list = self.disabled();
        list.retain(|x| x != id);
        if off {
            list.push(id.to_string());
        }
        write_private(
            &self.registry,
            &serde_json::to_vec_pretty(&json!({ "disabled": list })).map_err(err)?,
        )
    }

    fn enabled_manifest(&self, id: &str) -> Result<Manifest, String> {
        if self.disabled().iter().any(|x| x == id) {
            return Err(format!("plugin {id} is switched off"));
        }
        self.manifest(id)
    }

    fn data_file(&self, id: &str, name: &str) -> Result<PathBuf, String> {
        self.dir(id)?;
        Ok(self.data.join(id).join(name))
    }

    fn read_map(&self, id: &str, name: &str) -> Map<String, Value> {
        self.data_file(id, name)
            .ok()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn update_map(
        &self,
        id: &str,
        name: &str,
        f: impl FnOnce(&mut Map<String, Value>),
    ) -> Result<(), String> {
        let _g = self.write_lock.lock().map_err(err)?;
        let mut map = self.read_map(id, name);
        f(&mut map);
        let bytes = serde_json::to_vec(&map).map_err(err)?;
        if bytes.len() > MAX_STORAGE {
            return Err(format!(
                "plugin storage is limited to {} KB",
                MAX_STORAGE / 1024
            ));
        }
        write_private(&self.data_file(id, name)?, &bytes)
    }

    fn secret(&self, id: &str, name: &str) -> Option<String> {
        self.read_map(id, "secrets.json")
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_owned)
    }
}

/// Writes a file only the user can read (tokens, settings).
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(err)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(err)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path).map_err(err)
}

// ---- serving plugin files ----------------------------------------------------------------

/// `xsplugin://localhost/<id>/<path>` → the plugin's file; `/_sdk/…` → the SDK.
pub fn serve<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    req: Request<Vec<u8>>,
) -> Response<Cow<'static, [u8]>> {
    let plugins = ctx.app_handle().state::<Plugins>();
    let path = percent_decode(req.uri().path());
    let path = path.trim_start_matches('/');
    let reply = |status: StatusCode, mime: &str, body: Cow<'static, [u8]>, csp: Option<String>| {
        let mut b = Response::builder()
            .status(status)
            .header("Content-Type", mime)
            .header("Cache-Control", "no-store")
            // Module scripts in an opaque-origin (sandboxed) frame are fetched in CORS mode.
            .header("Access-Control-Allow-Origin", "*")
            .header("X-Content-Type-Options", "nosniff");
        if let Some(csp) = csp {
            b = b.header("Content-Security-Policy", csp);
        }
        b.body(body)
            .unwrap_or_else(|_| Response::new(Cow::Borrowed(&b"error"[..])))
    };
    let not_found = || {
        reply(
            StatusCode::NOT_FOUND,
            "text/plain",
            Cow::Borrowed(b"not found"),
            None,
        )
    };

    match path {
        "_sdk/xstudio.js" => {
            return reply(
                StatusCode::OK,
                "text/javascript",
                Cow::Borrowed(SDK_JS.as_bytes()),
                None,
            )
        }
        "_sdk/xstudio.css" => {
            return reply(
                StatusCode::OK,
                "text/css",
                Cow::Borrowed(SDK_CSS.as_bytes()),
                None,
            )
        }
        _ => {}
    }
    let Some((id, rel)) = path.split_once('/') else {
        return not_found();
    };
    let Ok(manifest) = plugins.enabled_manifest(id) else {
        return not_found();
    };
    let Ok(dir) = plugins.dir(id) else {
        return not_found();
    };
    let Some(file) = x4_plugins::resolve(&dir, rel) else {
        return not_found();
    };
    match std::fs::read(&file) {
        Ok(data) => reply(
            StatusCode::OK,
            mime(&file),
            Cow::Owned(data),
            Some(csp(&manifest)),
        ),
        Err(_) => not_found(),
    }
}

/// No network of its own (`connect-src 'none'`), no frames, no forms. Images may come only
/// from hosts the plugin may already reach through the app.
fn csp(m: &Manifest) -> String {
    let own = format!("{SCHEME}: http://{SCHEME}.localhost");
    let img_hosts = if m.permissions.network.iter().any(|h| h == "*") {
        " https:".to_string()
    } else {
        m.permissions
            .network
            .iter()
            .map(|h| format!(" https://{h}"))
            .collect()
    };
    format!(
        "default-src 'none'; script-src {own} 'unsafe-inline'; style-src {own} 'unsafe-inline'; \
         img-src {own} data: blob:{img_hosts}; font-src {own} data:; media-src {own} data: blob:; \
         connect-src 'none'; frame-src 'none'; worker-src 'none'; form-action 'none'; base-uri 'none'"
    )
}

fn mime(p: &Path) -> &'static str {
    match p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        _ => "application/octet-stream",
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) =
                u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16)
            {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---- managing plugins ----------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInfo {
    manifest: Manifest,
    enabled: bool,
    permissions: Vec<PermissionLine>,
}

#[tauri::command]
pub fn plugins_list(plugins: State<'_, Plugins>) -> Vec<PluginInfo> {
    let disabled = plugins.disabled();
    let mut out: Vec<PluginInfo> = std::fs::read_dir(&plugins.root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| x4_plugins::installed_manifest(&e.path()).ok())
        .filter(|m| e_matches_dir(m, &plugins.root))
        .map(|m| PluginInfo {
            enabled: !disabled.contains(&m.id),
            permissions: m.permission_summary(),
            manifest: m,
        })
        .collect();
    out.sort_by(|a, b| {
        a.manifest
            .name
            .to_lowercase()
            .cmp(&b.manifest.name.to_lowercase())
    });
    out
}

/// A folder renamed by hand doesn't get served under another plugin's id.
fn e_matches_dir(m: &Manifest, root: &Path) -> bool {
    root.join(&m.id).join("manifest.json").is_file()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inspected {
    manifest: Manifest,
    permissions: Vec<PermissionLine>,
    installed_version: Option<String>,
}

/// Reads a plugin ZIP for the install dialog (nothing is written).
#[tauri::command]
pub async fn plugin_inspect(plugins: State<'_, Plugins>, path: String) -> CmdResult<Inspected> {
    let installed = |id: &str| plugins.manifest(id).ok().map(|m| m.version);
    let pkg = tauri::async_runtime::spawn_blocking(move || {
        Package::read_file(Path::new(&path)).map_err(err)
    })
    .await
    .map_err(err)??;
    Ok(Inspected {
        installed_version: installed(&pkg.manifest.id),
        permissions: pkg.manifest.permission_summary(),
        manifest: pkg.manifest,
    })
}

#[tauri::command]
pub async fn plugin_install(app: AppHandle, path: String) -> CmdResult<Manifest> {
    tauri::async_runtime::spawn_blocking(move || {
        let plugins = app.state::<Plugins>();
        let pkg = Package::read_file(Path::new(&path)).map_err(err)?;
        pkg.install(&plugins.root).map_err(err)?;
        plugins.set_disabled(&pkg.manifest.id, false)?;
        Ok(pkg.manifest)
    })
    .await
    .map_err(err)?
}

/// Removes the plugin; `keep_data` leaves its settings and tokens for a reinstall.
#[tauri::command]
pub fn plugin_remove(plugins: State<'_, Plugins>, id: String, keep_data: bool) -> CmdResult<()> {
    let dir = plugins.dir(&id)?;
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(err)?;
    }
    if !keep_data {
        let _ = std::fs::remove_dir_all(plugins.data.join(&id));
    }
    plugins.set_disabled(&id, false)
}

#[tauri::command]
pub fn plugin_set_enabled(plugins: State<'_, Plugins>, id: String, enabled: bool) -> CmdResult<()> {
    plugins.manifest(&id)?;
    plugins.set_disabled(&id, !enabled)
}

// ---- the plugin API ------------------------------------------------------------------------

/// One API call from a plugin page. The frontend relays it from the iframe it created for
/// `id`, so the plugin can't claim to be another one; permissions are checked here.
#[tauri::command]
pub async fn plugin_call(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    method: String,
    params: Value,
) -> CmdResult<Value> {
    let manifest = app.state::<Plugins>().enabled_manifest(&id)?;
    if method == "auth.signIn" {
        return sign_in(app, manifest, params).await;
    }
    let device = state.device.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let plugins = app.state::<Plugins>();
        call(&app, &plugins, &manifest, device, &method, params)
    })
    .await
    .map_err(err)?
}

fn str_param<'a>(p: &'a Value, key: &str) -> Result<&'a str, String> {
    p.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing \"{key}\""))
}

fn bytes_param(p: &Value, key: &str) -> Result<Vec<u8>, String> {
    b64()
        .decode(str_param(p, key)?)
        .map_err(|e| format!("\"{key}\" is not base64: {e}"))
}

fn call(
    app: &AppHandle,
    plugins: &Plugins,
    m: &Manifest,
    device: Arc<Mutex<Option<Box<dyn x4_device::Transport>>>>,
    method: &str,
    p: Value,
) -> Result<Value, String> {
    let id = m.id.as_str();
    let need_secrets = || {
        if m.permissions.secrets {
            Ok(())
        } else {
            Err("the plugin has no \"secrets\" permission".to_string())
        }
    };
    match method {
        "http.fetch" => http_fetch(plugins, m, &p),

        "storage.get" => Ok(plugins
            .read_map(id, "storage.json")
            .get(str_param(&p, "key")?)
            .cloned()
            .unwrap_or(Value::Null)),
        "storage.keys" => Ok(json!(plugins
            .read_map(id, "storage.json")
            .keys()
            .collect::<Vec<_>>())),
        "storage.set" => {
            let key = storage_key(&p)?;
            let value = p.get("value").cloned().unwrap_or(Value::Null);
            plugins.update_map(id, "storage.json", |m| {
                m.insert(key, value);
            })?;
            Ok(Value::Null)
        }
        "storage.delete" => {
            let key = storage_key(&p)?;
            plugins.update_map(id, "storage.json", |m| {
                m.remove(&key);
            })?;
            Ok(Value::Null)
        }

        "secrets.set" => {
            need_secrets()?;
            let name = secret_name(&p)?;
            let value = str_param(&p, "value")?.trim().to_string();
            if value.is_empty() || value.len() > 8192 || value.contains(['\r', '\n']) {
                return Err("a secret must be one line, up to 8 KB".into());
            }
            plugins.update_map(id, "secrets.json", |m| {
                m.insert(name, Value::String(value));
            })?;
            Ok(Value::Null)
        }
        "secrets.has" => {
            need_secrets()?;
            Ok(json!(plugins.secret(id, &secret_name(&p)?).is_some()))
        }
        "secrets.delete" => {
            need_secrets()?;
            let name = secret_name(&p)?;
            plugins.update_map(id, "secrets.json", |m| {
                m.remove(&name);
            })?;
            Ok(Value::Null)
        }

        "device.sendBook" => {
            if !m.allows_device(DeviceAccess::Write) {
                return Err("the plugin has no \"device: write\" permission".into());
            }
            let name = str_param(&p, "name")?;
            let data = bytes_param(&p, "data")?;
            let queue_id = format!(
                "plugin:{id}:{}",
                plugins.transfer_seq.fetch_add(1, Ordering::Relaxed)
            );
            let path = send_book(app, &device, &queue_id, name, &data)?;
            Ok(json!({ "path": path }))
        }

        "epub.prepare" => {
            let data = bytes_param(&p, "data")?;
            let metadata: x4_epub::BookMetadata =
                serde_json::from_value(p.get("metadata").cloned().unwrap_or(json!({})))
                    .map_err(|e| format!("metadata: {e}"))?;
            let cover = match p.get("cover").and_then(Value::as_str) {
                Some(c) => Some(b64().decode(c).map_err(|e| format!("cover: {e}"))?),
                None => None,
            };
            let out = x4_epub::prepare(&data, &metadata, cover.as_deref()).map_err(err)?;
            Ok(
                json!({ "data": b64().encode(out), "fileName": x4_epub::file_name(&metadata.title, &metadata.authors) }),
            )
        }

        other => Err(format!("unknown method \"{other}\"")),
    }
}

fn storage_key(p: &Value) -> Result<String, String> {
    let key = str_param(p, "key")?;
    if key.is_empty() || key.len() > 200 {
        return Err("storage keys are 1–200 characters".into());
    }
    Ok(key.to_string())
}

fn secret_name(p: &Value) -> Result<String, String> {
    secret_name_in(p, "name")
}

/// The secret name in `key` of the params (`name` for secrets.*, `secret` for auth.signIn).
fn secret_name_in(p: &Value, key: &str) -> Result<String, String> {
    let name = str_param(p, key)?;
    if !x4_plugins::valid_secret_name(name) {
        return Err(format!("bad secret name \"{name}\""));
    }
    Ok(name.to_string())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FetchRequest {
    url: String,
    #[serde(default = "get")]
    method: String,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    /// Body as text, or `bodyBase64` for bytes.
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    body_base64: Option<String>,
}

fn get() -> String {
    "GET".into()
}

/// HTTPS to the hosts in the manifest only; redirects are followed here so each hop is
/// checked too. `{{secret:NAME}}` in a header is replaced with the stored secret.
fn http_fetch(plugins: &Plugins, m: &Manifest, p: &Value) -> Result<Value, String> {
    let req: FetchRequest =
        serde_json::from_value(p.clone()).map_err(|e| format!("http.fetch: {e}"))?;
    let method = req.method.to_ascii_uppercase();
    if !["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"].contains(&method.as_str()) {
        return Err(format!("method {method} is not allowed"));
    }
    let mut headers = Vec::new();
    for (k, v) in &req.headers {
        let lk = k.to_ascii_lowercase();
        if [
            "host",
            "content-length",
            "connection",
            "transfer-encoding",
            "upgrade",
        ]
        .contains(&lk.as_str())
            || !k
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(format!("header \"{k}\" can't be set"));
        }
        let v = x4_plugins::fill_secrets(v, |name| {
            m.permissions
                .secrets
                .then(|| plugins.secret(&m.id, name))
                .flatten()
        })?;
        if v.contains(['\r', '\n']) {
            return Err(format!("header \"{k}\" has a line break"));
        }
        headers.push((k.clone(), v));
    }
    let body = match (&req.body, &req.body_base64) {
        (Some(t), _) => Some(t.as_bytes().to_vec()),
        (None, Some(b)) => Some(b64().decode(b).map_err(|e| format!("bodyBase64: {e}"))?),
        _ => None,
    };

    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(120))
        .timeout_write(Duration::from_secs(60))
        .build();
    let mut url = req.url.clone();
    let mut method = method;
    let mut body = body;
    for _ in 0..=MAX_REDIRECTS {
        let host = x4_plugins::https_host(&url)
            .ok_or_else(|| format!("only https:// URLs are allowed: {url}"))?;
        if !m.allows_network(&host) {
            return Err(format!("the plugin may not connect to {host}"));
        }
        let mut r = agent.request(&method, &url);
        for (k, v) in &headers {
            r = r.set(k, v);
        }
        let result = match &body {
            Some(b) => r.send_bytes(b),
            None => r.call(),
        };
        let resp = match result {
            Ok(r) => r,
            Err(ureq::Error::Status(_, r)) => r,
            Err(ureq::Error::Transport(t)) => return Err(format!("network error: {t}")),
        };
        let status = resp.status();
        if (300..400).contains(&status) {
            if let Some(next) = resp.header("location") {
                url = join_url(&url, next);
                if status == 303 || ((status == 301 || status == 302) && method == "POST") {
                    method = "GET".into();
                    body = None;
                }
                continue;
            }
        }
        let mut out_headers = Map::new();
        for name in resp.headers_names() {
            if let Some(v) = resp.header(&name) {
                out_headers.insert(name.to_ascii_lowercase(), Value::String(v.to_string()));
            }
        }
        let final_url = resp.get_url().to_string();
        let mut data = Vec::new();
        resp.into_reader()
            .take(MAX_RESPONSE + 1)
            .read_to_end(&mut data)
            .map_err(|e| format!("network error: {e}"))?;
        if data.len() as u64 > MAX_RESPONSE {
            return Err(format!(
                "response larger than {} MB",
                MAX_RESPONSE / 1024 / 1024
            ));
        }
        return Ok(json!({
            "status": status,
            "url": final_url,
            "headers": out_headers,
            "bodyBase64": b64().encode(data),
        }));
    }
    Err("too many redirects".into())
}

fn join_url(base: &str, location: &str) -> String {
    if location.starts_with("https://") || location.starts_with("http://") {
        return location.to_string();
    }
    let origin_end = base
        .strip_prefix("https://")
        .and_then(|r| r.find('/').map(|i| i + "https://".len()))
        .unwrap_or(base.len());
    let origin = &base[..origin_end];
    if location.starts_with('/') {
        format!("{origin}{location}")
    } else {
        let dir = base[..base
            .rfind('/')
            .filter(|&i| i >= origin_end)
            .map_or(base.len(), |i| i + 1)]
            .to_string();
        let dir = if dir.ends_with('/') {
            dir
        } else {
            format!("{dir}/")
        };
        format!("{dir}{location}")
    }
}

/// Writes a book into /Books (or the card's root when there is no such folder), with the
/// usual transfer events so the sidebar shows progress.
pub(crate) fn send_book(
    app: &AppHandle,
    device: &Arc<Mutex<Option<Box<dyn x4_device::Transport>>>>,
    queue_id: &str,
    name: &str,
    data: &[u8],
) -> Result<String, String> {
    let name = x4_plugins::safe_book_name(name)
        .ok_or_else(|| format!("\"{name}\" is not a book file name"))?;
    let result = (|| {
        let mut guard = device.lock().map_err(err)?;
        let books = guard
            .as_mut()
            .ok_or_else(|| x4_device::Error::NotConnected.to_string())?
            .list("/")
            .map_err(err)?
            .into_iter()
            .find(|e| e.is_dir && e.name.eq_ignore_ascii_case("books"))
            .map(|e| e.path)
            .unwrap_or_else(|| "/".into());
        let path = x4_device::join_path(&books, &name);
        let mut progress =
            crate::throttled_emitter(app.clone(), queue_id.to_string(), name.clone());
        crate::reconnect::write_reconnecting(
            Some(app),
            &mut guard,
            &path,
            data,
            true,
            &mut progress,
        )?;
        Ok(path)
    })();
    crate::emit_finished(
        app,
        queue_id.to_string(),
        name,
        result.as_ref().err().cloned(),
    );
    result
}

struct SignIn {
    url: String,
    secret: String,
    param: String,
}

/// `auth.signIn({url, secret, param})`: an https page on an authWindow host, the secret to
/// store the captured value under, and the URL parameter that carries it.
fn sign_in_params(m: &Manifest, p: &Value) -> Result<SignIn, String> {
    let url = str_param(p, "url")?.to_string();
    let secret = secret_name_in(p, "secret")?;
    let param = match p.get("param") {
        None | Some(Value::Null) => "access_token".to_string(),
        Some(Value::String(s))
            if !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') =>
        {
            s.clone()
        }
        Some(other) => return Err(format!("bad \"param\": {other}")),
    };
    let host = x4_plugins::https_host(&url).ok_or("the sign-in page must be https://")?;
    if !m.allows_auth_window(&host) {
        return Err(format!(
            "the plugin may not open a sign-in window for {host}"
        ));
    }
    Ok(SignIn { url, secret, param })
}

/// Opens a sign-in page and waits for the redirect carrying `param`; the value goes straight
/// into the plugin's secrets. The window has no access to the app's API.
async fn sign_in(app: AppHandle, m: Manifest, p: Value) -> CmdResult<Value> {
    if !m.permissions.secrets {
        return Err("the plugin has no \"secrets\" permission".into());
    }
    let SignIn { url, secret, param } = sign_in_params(&m, &p)?;
    let plugins = app.state::<Plugins>();
    if plugins.signing_in.swap(true, Ordering::SeqCst) {
        if let Some(w) = app.get_webview_window(AUTH_WINDOW) {
            let _ = w.set_focus();
        }
        return Err("another sign-in window is already open".into());
    }
    struct Reset<'a>(&'a AtomicBool);
    impl Drop for Reset<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }
    let _reset = Reset(&plugins.signing_in);

    let (tx, rx) = mpsc::channel::<Option<String>>();
    let done = Arc::new(AtomicBool::new(false));
    let report = {
        let (tx, done) = (tx.clone(), done.clone());
        move |token: Option<String>| {
            if !done.swap(true, Ordering::SeqCst) {
                let _ = tx.send(token);
            }
        }
    };
    let on_nav = report.clone();
    let nav_param = param.clone();
    let window = WebviewWindowBuilder::new(
        &app,
        AUTH_WINDOW,
        WebviewUrl::External(url.parse().map_err(err)?),
    )
    .title(format!("Sign in — {}", m.name))
    .inner_size(480.0, 700.0)
    .on_navigation(
        move |u| match x4_plugins::param_from_url(u.as_str(), &nav_param) {
            Some(t) => {
                on_nav(Some(t));
                false
            }
            None => true,
        },
    )
    .build()
    .map_err(err)?;
    // A fragment-only redirect may not pass through on_navigation; watch the URL as well.
    {
        let (report, done, window, param) =
            (report.clone(), done.clone(), window.clone(), param.clone());
        std::thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                match window.url() {
                    Ok(u) => {
                        if let Some(t) = x4_plugins::param_from_url(u.as_str(), &param) {
                            report(Some(t));
                            return;
                        }
                    }
                    Err(_) => return,
                }
                std::thread::sleep(Duration::from_millis(400));
            }
        });
    }
    {
        let report = report.clone();
        window.on_window_event(move |ev| {
            if matches!(ev, tauri::WindowEvent::Destroyed) {
                report(None);
            }
        });
    }

    let token =
        tauri::async_runtime::spawn_blocking(move || rx.recv_timeout(AUTH_TIMEOUT).ok().flatten())
            .await
            .map_err(err)?;
    if let Some(w) = app.get_webview_window(AUTH_WINDOW) {
        let _ = w.close();
    }
    let token = token.ok_or("sign-in was cancelled")?;
    plugins.update_map(&m.id, "secrets.json", |s| {
        s.insert(secret, Value::String(token));
    })?;
    Ok(json!({ "ok": true }))
}

// ---- host-side helpers (called by the app's own UI, not by plugins) -------------------------

/// Saves bytes a plugin produced to a path the user picked in a save dialog.
#[tauri::command]
pub async fn plugin_save_file(path: String, data: String) -> CmdResult<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let data = b64().decode(data).map_err(err)?;
        std::fs::write(path, data).map_err(err)
    })
    .await
    .map_err(err)?
}

/// A book's bytes from the reader, for a plugin's book action.
#[tauri::command]
pub async fn read_book_bytes(state: State<'_, AppState>, path: String) -> CmdResult<String> {
    let data = crate::with_device(&state, move |t| t.read(&path, &mut |_, _| {})).await?;
    Ok(b64().encode(data))
}

/// Writes a book action's result back over the book.
#[tauri::command]
pub async fn write_book_bytes(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    data: String,
) -> CmdResult<()> {
    let device = state.device.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let data = b64().decode(data).map_err(err)?;
        let name = x4_device::split_path(&path).1;
        let id = format!("action:{path}");
        let result = (|| {
            let mut guard = device.lock().map_err(err)?;
            let mut progress = crate::throttled_emitter(app.clone(), id.clone(), name.clone());
            crate::reconnect::write_reconnecting(
                Some(&app),
                &mut guard,
                &path,
                &data,
                true,
                &mut progress,
            )
        })();
        crate::emit_finished(&app, id, name, result.as_ref().err().cloned());
        result
    })
    .await
    .map_err(err)?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_sign_in_params_as_the_sdk_sends_them() {
        let m = Manifest::parse(
            br#"{"id":"dev.t.p","name":"P","version":"1","apiVersion":1,
                "permissions":{"secrets":true,"authWindow":["oauth.example.com"]},
                "contributes":{"section":{"title":"P"}}}"#,
        )
        .unwrap();
        // The SDK's xstudio.auth.signIn({url, secret, param}); param may be left out (undefined).
        let ok = sign_in_params(
            &m,
            &json!({"url": "https://oauth.example.com/authorize?x=1", "secret": "token"}),
        )
        .unwrap();
        assert_eq!(
            (ok.secret.as_str(), ok.param.as_str()),
            ("token", "access_token")
        );
        let code = sign_in_params(
            &m,
            &json!({"url": "https://oauth.example.com/a", "secret": "t", "param": "code"}),
        )
        .unwrap();
        assert_eq!(code.param, "code");
        let err = |p: Value| sign_in_params(&m, &p).err().unwrap();
        assert!(err(json!({"url": "https://evil.com/", "secret": "token"})).contains("evil.com"));
        assert!(
            err(json!({"url": "http://oauth.example.com/", "secret": "token"})).contains("https")
        );
        assert!(err(json!({"url": "https://oauth.example.com/"})).contains("\"secret\""));
        assert!(
            err(json!({"url": "https://oauth.example.com/", "secret": "../x"}))
                .contains("bad secret")
        );
    }

    #[test]
    fn joins_redirects() {
        assert_eq!(
            join_url("https://a.test/x/y?q", "https://b.test/z"),
            "https://b.test/z"
        );
        assert_eq!(join_url("https://a.test/x/y?q", "/z"), "https://a.test/z");
        assert_eq!(join_url("https://a.test/x/y", "z"), "https://a.test/x/z");
        assert_eq!(join_url("https://a.test", "z"), "https://a.test/z");
    }

    #[test]
    fn locks_down_plugin_pages() {
        let m = Manifest::parse(
            br#"{"id":"dev.t.p","name":"P","version":"1","apiVersion":1,
                "permissions":{"network":["api.example.com"]},"contributes":{"section":{"title":"P"}}}"#,
        )
        .unwrap();
        let c = csp(&m);
        assert!(c.contains("connect-src 'none'"));
        assert!(c.contains(
            "img-src xsplugin: http://xsplugin.localhost data: blob: https://api.example.com;"
        ));
        assert!(c.contains("frame-src 'none'"));
        assert_eq!(percent_decode("/dev.t.p/%D0%90.html"), "/dev.t.p/А.html");
    }
}

//! CrossInk's SD font library — the same `fonts.json` the reader downloads in
//! Settings → Reader → Font Options → Manage Fonts. Files are fetched on the Mac, checked
//! against the manifest's CRC32, cached, and copied to `<fonts root>/<Family>/` on the card.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use x4_device::{join_path, Entry};

use crate::{AppState, CmdResult};

/// Manifest schema 1, `.cpfont` format 4 (`FONTS_MANIFEST_VERSION` / `CPFONT_VERSION` in
/// CrossInk). A firmware that moves to a new font format will look under a new prefix.
const MANIFEST_URL: &str =
    "https://crossink-fonts.s3.us-east-1.amazonaws.com/sd-fonts-m1-b4/fonts.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub version: u32,
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    pub families: Vec<Family>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Family {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub languages: String,
    #[serde(default)]
    pub styles: Vec<String>,
    pub files: Vec<FontFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FontFile {
    pub name: String,
    pub size: u64,
    pub crc32: u32,
    /// From the `<Family>_<size>.cpfont` file name; filled in after parsing.
    #[serde(default)]
    pub point_size: u8,
}

/// `<Family>_<size>.cpfont` → size, as the firmware parses it.
pub fn point_size(file_name: &str) -> Option<u8> {
    let stem = file_name.strip_suffix(".cpfont")?;
    let (_, size) = stem.rsplit_once('_')?;
    size.parse().ok()
}

pub fn parse_manifest(json: &[u8]) -> Result<Manifest, String> {
    let mut m: Manifest =
        serde_json::from_slice(json).map_err(|e| format!("Invalid font manifest: {e}"))?;
    for f in &mut m.families {
        f.files.retain_mut(|file| match point_size(&file.name) {
            Some(p) => {
                file.point_size = p;
                true
            }
            None => false,
        });
        f.files.sort_by_key(|file| file.point_size);
    }
    m.families.retain(|f| !f.files.is_empty());
    m.families.sort_by_key(|f| f.name.to_lowercase());
    Ok(m)
}

/// Percent-encodes one URL path segment ("Lexend Deca_12.cpfont" → "Lexend%20Deca_12.cpfont").
pub fn encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The S3 bucket is published with an `http://` base for the reader (TLS is too heavy for
/// the ESP32-C3); the Mac uses HTTPS. Files are verified by CRC32 either way.
pub fn file_url(base_url: &str, name: &str) -> String {
    let base = match base_url.strip_prefix("http://") {
        Some(rest) if rest.contains(".amazonaws.com") => format!("https://{rest}"),
        _ => base_url.to_string(),
    };
    let sep = if base.ends_with('/') { "" } else { "/" };
    format!("{base}{sep}{}", encode_segment(name))
}

fn cache_dir(app: &AppHandle) -> PathBuf {
    let dir = app
        .path()
        .app_cache_dir()
        .unwrap_or_else(|_| std::env::temp_dir())
        .join("fonts");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(60))
        .build()
}

fn fetch(url: &str) -> Result<Vec<u8>, String> {
    fetch_with_progress(url, &mut |_| {})
}

/// Downloads `url`, reporting bytes received so far.
fn fetch_with_progress(url: &str, progress: &mut dyn FnMut(u64)) -> Result<Vec<u8>, String> {
    let resp = agent().get(url).call().map_err(|e| match e {
        ureq::Error::Status(code, _) => format!("{url}: HTTP {code}"),
        ureq::Error::Transport(t) => format!("Can't reach the CrossInk font library: {t}"),
    })?;
    let mut reader = resp.into_reader();
    let mut body = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = std::io::Read::read(&mut reader, &mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&buf[..n]);
        progress(body.len() as u64);
    }
    Ok(body)
}

/// The catalog: fresh from the network, else the last copy that worked.
fn load_manifest(app: &AppHandle) -> Result<Manifest, String> {
    let cached = cache_dir(app).join("fonts.json");
    match fetch(MANIFEST_URL).and_then(|b| parse_manifest(&b).map(|m| (m, b))) {
        Ok((m, bytes)) => {
            let _ = std::fs::write(&cached, bytes);
            Ok(m)
        }
        Err(e) => match std::fs::read(&cached) {
            Ok(bytes) => parse_manifest(&bytes),
            Err(_) => Err(e),
        },
    }
}

#[tauri::command]
pub async fn font_catalog(app: AppHandle) -> CmdResult<Vec<Family>> {
    tauri::async_runtime::spawn_blocking(move || load_manifest(&app).map(|m| m.families))
        .await
        .map_err(|e| e.to_string())?
}

/// The file's bytes from the cache, else downloaded and checked against the manifest.
fn font_bytes(
    app: &AppHandle,
    base_url: &str,
    family: &str,
    f: &FontFile,
    progress: &mut dyn FnMut(u64),
) -> Result<Vec<u8>, String> {
    let dir = cache_dir(app).join(family);
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(&f.name);
    if let Ok(bytes) = std::fs::read(&path) {
        if crc32fast::hash(&bytes) == f.crc32 {
            return Ok(bytes);
        }
    }
    let bytes = fetch_with_progress(&file_url(base_url, &f.name), progress)?;
    if bytes.len() as u64 != f.size || crc32fast::hash(&bytes) != f.crc32 {
        return Err(format!("{}: download is damaged, try again", f.name));
    }
    let _ = std::fs::write(&path, &bytes);
    Ok(bytes)
}

/// Downloads the family's files in `sizes` and writes them to `<root>/<family>/` on the
/// reader, replacing files of the same name. Progress goes out as `transfer` events.
/// Returns the entries written.
#[tauri::command]
pub async fn install_font(
    app: AppHandle,
    state: State<'_, AppState>,
    family: String,
    sizes: Vec<u8>,
    root: String,
) -> CmdResult<Vec<Entry>> {
    let device = state.device.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let manifest = load_manifest(&app)?;
        let fam = manifest
            .families
            .iter()
            .find(|f| f.name == family)
            .ok_or_else(|| format!("{family} is not in the font library"))?;
        let files: Vec<&FontFile> = fam
            .files
            .iter()
            .filter(|f| sizes.is_empty() || sizes.contains(&f.point_size))
            .collect();
        if files.is_empty() {
            return Err(format!("{family} has no files in these sizes"));
        }

        // Everything is downloaded first: a network problem must not leave half a family.
        // The download is one queue row ("fontdl:<family>"), each file written another.
        let dl_id = format!("fontdl:{}", fam.name);
        let dl_name = format!("Downloading {}", fam.name);
        let total: u64 = files.iter().map(|f| f.size).sum();
        let mut report = crate::throttled_emitter(app.clone(), dl_id.clone(), dl_name.clone());
        let mut payload = Vec::new();
        let mut before = 0u64;
        for f in &files {
            let got = font_bytes(&app, &manifest.base_url, &fam.name, f, &mut |n| {
                report(before + n.min(f.size), total)
            });
            match got {
                Ok(bytes) => payload.push((f.name.clone(), bytes)),
                Err(e) => {
                    crate::emit_finished(&app, dl_id, dl_name, Some(e.clone()));
                    return Err(e);
                }
            }
            before += f.size;
            report(before, total);
        }
        crate::emit_finished(&app, dl_id, dl_name, None);

        let dir = join_path(&root, &fam.name);
        let mut written = Vec::new();
        {
            let mut guard = device.lock().map_err(|e| e.to_string())?;
            let t = guard
                .as_mut()
                .ok_or_else(|| x4_device::Error::NotConnected.to_string())?;
            // mkdir fails on an existing folder (over Wi‑Fi); a real problem shows up in write.
            let _ = t.mkdir(&root);
            let _ = t.mkdir(&dir);
        }
        for (name, data) in payload {
            let path = join_path(&dir, &name);
            let id = format!("font:{path}");
            let result = (|| -> Result<(), String> {
                let mut guard = device.lock().map_err(|e| e.to_string())?;
                let t = guard
                    .as_mut()
                    .ok_or_else(|| x4_device::Error::NotConnected.to_string())?;
                let mut progress = crate::throttled_emitter(app.clone(), id.clone(), name.clone());
                x4_device::write_in_batch(t.as_mut(), &path, &data, true, &mut progress)
                    .map_err(|e| e.to_string())
            })();
            crate::emit_finished(&app, id, name.clone(), result.as_ref().err().cloned());
            result?;
            written.push(Entry {
                name,
                path,
                is_dir: false,
                size: data.len() as u64,
            });
        }
        Ok(written)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "version": 1,
      "baseUrl": "http://crossink-fonts.s3.us-east-1.amazonaws.com/sd-fonts-m1-b4/",
      "families": [
        {"name": "Lexend Deca", "description": "Sans", "languages": "Latin",
         "styles": ["regular", "bold"],
         "files": [{"name": "Lexend Deca_12.cpfont", "size": 10, "crc32": 1},
                   {"name": "Lexend Deca_8.cpfont", "size": 10, "crc32": 2},
                   {"name": "README.txt", "size": 1, "crc32": 3}]},
        {"name": "Bitter", "files": [{"name": "Bitter_10.cpfont", "size": 5, "crc32": 4}]},
        {"name": "Empty", "files": []}
      ]
    }"#;

    #[test]
    fn manifest_sizes_and_order() {
        let m = parse_manifest(SAMPLE.as_bytes()).unwrap();
        let names: Vec<_> = m.families.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["Bitter", "Lexend Deca"]);
        let lexend = &m.families[1];
        let sizes: Vec<_> = lexend.files.iter().map(|f| f.point_size).collect();
        assert_eq!(sizes, [8, 12]);
        assert_eq!(point_size("Noto_Serif_14.cpfont"), Some(14));
        assert_eq!(point_size("Bitter.cpfont"), None);
    }

    #[test]
    fn urls() {
        assert_eq!(
            file_url(
                "http://crossink-fonts.s3.us-east-1.amazonaws.com/sd-fonts-m1-b4/",
                "Lexend Deca_12.cpfont"
            ),
            "https://crossink-fonts.s3.us-east-1.amazonaws.com/sd-fonts-m1-b4/Lexend%20Deca_12.cpfont"
        );
        assert_eq!(
            file_url("https://example.org/f", "a+b.cpfont"),
            "https://example.org/f/a%2Bb.cpfont"
        );
    }
}

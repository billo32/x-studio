//! Book and image commands: SD-card scan, metadata with a disk cache, cover previews, saving edits.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use x4_books::epub::Edits;
use x4_books::imaging::{self, Fit, Preview};
use x4_books::{BookInfo, Format};
use x4_device::{join_path, split_path, Entry, Transport};

use crate::{AppState, CmdResult};

/// Folders the firmware never shows and the scan skips.
const SKIP_DIRS: &[&str] = &["System Volume Information", "XTCache", "LOST.DIR"];
const MAX_ENTRIES: usize = 10_000;
const MAX_DEPTH: usize = 8;

fn err(e: impl ToString) -> String {
    e.to_string()
}

/// Lists every file on the card (breadth-first), folders included.
#[tauri::command]
pub async fn scan_device(state: State<'_, AppState>) -> CmdResult<Vec<Entry>> {
    crate::with_device(&state, |t| {
        let mut out = Vec::new();
        let mut queue: VecDeque<(String, usize)> = VecDeque::from([("/".to_string(), 0)]);
        while let Some((dir, depth)) = queue.pop_front() {
            for e in t.list(&dir)? {
                if e.is_dir
                    && depth < MAX_DEPTH
                    && !e.name.starts_with('.')
                    && !SKIP_DIRS.contains(&e.name.as_str())
                {
                    queue.push_back((e.path.clone(), depth + 1));
                }
                out.push(e);
                if out.len() >= MAX_ENTRIES {
                    return Ok(out);
                }
            }
        }
        Ok(out)
    })
    .await
}

/// On-disk cache of book files and their parsed info, keyed by device path and size.
pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    pub fn new(app: &AppHandle) -> Self {
        let dir = app
            .path()
            .app_cache_dir()
            .unwrap_or_else(|_| std::env::temp_dir())
            .join("books");
        let _ = std::fs::create_dir_all(&dir);
        Cache { dir }
    }

    fn key(path: &str, size: u64) -> String {
        format!("{:08x}-{size}", crc32fast::hash(path.as_bytes()))
    }

    fn file(&self, path: &str, size: u64, ext: &str) -> PathBuf {
        self.dir.join(format!("{}.{ext}", Self::key(path, size)))
    }

    fn read<T: for<'de> Deserialize<'de>>(&self, path: &str, size: u64, ext: &str) -> Option<T> {
        let bytes = std::fs::read(self.file(path, size, ext)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn write<T: Serialize>(&self, path: &str, size: u64, ext: &str, value: &T) {
        if let Ok(bytes) = serde_json::to_vec(value) {
            let _ = std::fs::write(self.file(path, size, ext), bytes);
        }
    }
}

/// The book's bytes: from the cache, else read from the reader and cached.
async fn book_bytes(state: &State<'_, AppState>, path: &str, size: u64) -> CmdResult<Vec<u8>> {
    let cache_file = state.cache.file(path, size, "bin");
    if let Ok(bytes) = std::fs::read(&cache_file) {
        if bytes.len() as u64 == size {
            return Ok(bytes);
        }
    }
    let p = path.to_string();
    let bytes = crate::with_device(state, move |t| t.read(&p, &mut |_, _| {})).await?;
    let _ = std::fs::write(&cache_file, &bytes);
    Ok(bytes)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookCard {
    #[serde(flatten)]
    pub info: BookInfo,
    pub format: Format,
    /// Small cover for the grid.
    pub thumb: Option<String>,
    /// Size of the original cover image.
    pub cover_width: Option<u32>,
    pub cover_height: Option<u32>,
}

fn card(name: &str, bytes: &[u8]) -> CmdResult<BookCard> {
    let (info, cover) = x4_books::read_book(name, bytes).map_err(err)?;
    let preview = cover.and_then(|c| imaging::preview(&c.bytes, 300, 500).ok());
    Ok(BookCard {
        info,
        format: Format::from_name(name),
        cover_width: preview.as_ref().map(|p| p.width),
        cover_height: preview.as_ref().map(|p| p.height),
        thumb: preview.map(|p| p.data_url),
    })
}

/// Title, author, stats and a cover thumbnail. The first call reads the whole file from the reader.
#[tauri::command]
pub async fn book_info(state: State<'_, AppState>, path: String, size: u64) -> CmdResult<BookCard> {
    if let Some(c) = state.cache.read::<BookCard>(&path, size, "json") {
        return Ok(c);
    }
    let bytes = book_bytes(&state, &path, size).await?;
    let name = split_path(&path).1;
    let c = tauri::async_runtime::spawn_blocking(move || card(&name, &bytes))
        .await
        .map_err(err)??;
    state.cache.write(&path, size, "json", &c);
    Ok(c)
}

/// The cover at editor size (fits 480×800, keeps the original size in the result).
#[tauri::command]
pub async fn book_cover(
    state: State<'_, AppState>,
    path: String,
    size: u64,
) -> CmdResult<Option<Preview>> {
    let bytes = book_bytes(&state, &path, size).await?;
    let name = split_path(&path).1;
    tauri::async_runtime::spawn_blocking(move || {
        let (_, cover) = x4_books::read_book(&name, &bytes).map_err(err)?;
        cover
            .map(|c| imaging::preview(&c.bytes, 480, 800).map_err(err))
            .transpose()
    })
    .await
    .map_err(err)?
}

/// Thumbnail of an image file on the reader (sleep screens, boot screens…).
#[tauri::command]
pub async fn image_thumb(
    state: State<'_, AppState>,
    path: String,
    size: u64,
) -> CmdResult<Preview> {
    if let Some(p) = state.cache.read::<Preview>(&path, size, "thumb.json") {
        return Ok(p);
    }
    let bytes = book_bytes(&state, &path, size).await?;
    let p = tauri::async_runtime::spawn_blocking(move || {
        imaging::preview(&bytes, 300, 500).map_err(err)
    })
    .await
    .map_err(err)??;
    state.cache.write(&path, size, "thumb.json", &p);
    Ok(p)
}

/// A local picture dropped onto the editor.
#[tauri::command]
pub async fn load_local_image(local_path: String) -> CmdResult<Preview> {
    tauri::async_runtime::spawn_blocking(move || {
        let bytes = std::fs::read(&local_path).map_err(|e| format!("{local_path}: {e}"))?;
        imaging::preview(&bytes, 480, 800).map_err(err)
    })
    .await
    .map_err(err)?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverInput {
    /// Local image to use; `None` re-bakes the book's current cover.
    pub source: Option<String>,
    pub fit: Fit,
    pub width: u32,
    pub height: u32,
    /// Bars for `contain`: "white" or "black".
    pub background: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveInput {
    pub title: Option<String>,
    pub author: Option<String>,
    pub language: Option<String>,
    pub series: Option<String>,
    /// Target folder on the reader; moves the book when it differs.
    pub folder: Option<String>,
    pub cover: Option<CoverInput>,
}

fn build(name: &str, bytes: &[u8], input: &SaveInput) -> CmdResult<Vec<u8>> {
    if Format::from_name(name) != Format::Epub {
        return Err(x4_books::Error::NotEditable.to_string());
    }
    let cover = match &input.cover {
        None => None,
        Some(c) => {
            let src = match &c.source {
                Some(p) => std::fs::read(p).map_err(|e| format!("{p}: {e}"))?,
                None => {
                    x4_books::read_book(name, bytes)
                        .map_err(err)?
                        .1
                        .ok_or("This book has no cover: drop an image on it")?
                        .bytes
                }
            };
            let bg = if c.background == "black" {
                [0, 0, 0]
            } else {
                [255, 255, 255]
            };
            let baked = imaging::bake(&src, c.width, c.height, c.fit, bg).map_err(err)?;
            Some((imaging::encode(&baked, "jpg").map_err(err)?, "jpg"))
        }
    };
    let edits = Edits {
        title: input.title.clone(),
        author: input.author.clone(),
        language: input.language.clone(),
        series: input.series.clone(),
        cover,
    };
    x4_books::epub::rewrite(bytes, &edits).map_err(err)
}

/// Applies edits and writes the book back to the reader (moving it if `folder` changed).
/// Returns the book's new entry.
#[tauri::command]
pub async fn save_book(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    size: u64,
    input: SaveInput,
) -> CmdResult<Entry> {
    let original = book_bytes(&state, &path, size).await?;
    let (dir, name) = split_path(&path);
    let name2 = name.clone();
    let folder = input
        .folder
        .clone()
        .filter(|f| !f.trim().is_empty())
        .map(|f| normalize_dir(&f))
        .unwrap_or(dir.clone());
    let new_bytes = tauri::async_runtime::spawn_blocking(move || build(&name2, &original, &input))
        .await
        .map_err(err)??;

    let new_path = join_path(&folder, &name);
    let new_size = new_bytes.len() as u64;
    let data = Arc::new(new_bytes);
    let data2 = data.clone();
    let (old, target) = (path.clone(), new_path.clone());
    let mut progress = crate::throttled_emitter(app.clone(), format!("save:{path}"), name.clone());
    crate::with_device(&state, move |t: &mut dyn Transport| {
        if target != old {
            // Wi-Fi mkdir fails on an existing folder; the rename below reports real problems.
            let _ = t.mkdir(&split_path(&target).0);
            t.rename(&old, &target)?;
        }
        t.write(&target, &data2, true, &mut progress)
    })
    .await?;
    let _ = std::fs::write(state.cache.file(&new_path, new_size, "bin"), &*data);
    Ok(Entry {
        name,
        path: new_path,
        is_dir: false,
        size: new_size,
    })
}

/// Applies edits and saves the EPUB to the Mac instead.
#[tauri::command]
pub async fn export_book(
    state: State<'_, AppState>,
    path: String,
    size: u64,
    input: SaveInput,
    save_to: String,
) -> CmdResult<()> {
    let original = book_bytes(&state, &path, size).await?;
    let name = split_path(&path).1;
    tauri::async_runtime::spawn_blocking(move || {
        let bytes = build(&name, &original, &input)?;
        std::fs::write(&save_to, bytes).map_err(|e| format!("{save_to}: {e}"))
    })
    .await
    .map_err(err)?
}

fn normalize_dir(d: &str) -> String {
    let t = d.trim().trim_end_matches('/');
    if t.is_empty() {
        "/".into()
    } else if t.starts_with('/') {
        t.into()
    } else {
        format!("/{t}")
    }
}

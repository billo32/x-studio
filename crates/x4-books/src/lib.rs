//! Book files for e-ink readers: metadata, reading stats, covers, and EPUB editing.
//!
//! Everything works on in-memory bytes so the caller decides where files come
//! from (the reader's SD card, a local cache, a drag-and-drop).

pub mod epub;
pub mod fb2;
pub mod imaging;
pub mod text;

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Damaged archive: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("Read error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid book structure: {0}")]
    Format(String),
    #[error("Could not process the image: {0}")]
    Image(#[from] image::ImageError),
    #[error("This format cannot be edited yet")]
    NotEditable,
}

pub type Result<T> = std::result::Result<T, Error>;

/// Characters per "stable page", the same unit CrossInk's optimizer uses.
pub const CHARS_PER_PAGE: usize = 1500;
/// Rough silent-reading speed used for the time estimate.
pub const CHARS_PER_MINUTE: usize = 1100;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Epub,
    Fb2,
    Txt,
    Other,
}

impl Format {
    pub fn from_name(name: &str) -> Format {
        let lower = name.to_lowercase();
        if lower.ends_with(".epub") {
            Format::Epub
        } else if lower.ends_with(".fb2") || lower.ends_with(".fb2.zip") {
            Format::Fb2
        } else if lower.ends_with(".txt") || lower.ends_with(".md") {
            Format::Txt
        } else {
            Format::Other
        }
    }
}

/// Raw cover image as found in the book.
#[derive(Debug, Clone)]
pub struct Cover {
    pub bytes: Vec<u8>,
    /// Path inside the archive, when the cover is a separate file.
    pub path: Option<String>,
}

/// Everything the UI shows about a book.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookInfo {
    pub title: String,
    pub authors: Vec<String>,
    pub language: Option<String>,
    pub series: Option<String>,
    pub series_index: Option<String>,
    /// Plain-text characters in the reading order.
    pub chars: usize,
    pub chapters: usize,
    pub images: usize,
    /// Stable pages (`chars / CHARS_PER_PAGE`, at least 1).
    pub pages: usize,
    pub reading_minutes: usize,
    pub editable: bool,
}

impl BookInfo {
    pub(crate) fn finish_stats(&mut self) {
        self.pages = self.chars.div_ceil(CHARS_PER_PAGE).max(1);
        self.reading_minutes = self.chars.div_ceil(CHARS_PER_MINUTE);
    }
}

/// Reads a book of any supported format. Returns the info and its cover, if any.
pub fn read_book(file_name: &str, bytes: &[u8]) -> Result<(BookInfo, Option<Cover>)> {
    let (mut info, cover) = match Format::from_name(file_name) {
        Format::Epub => epub::read(bytes)?,
        Format::Fb2 => fb2::read(bytes)?,
        Format::Txt | Format::Other => (text::read(bytes), None),
    };
    // Fill gaps from the file name, which is often "Author - Title.ext".
    let (name_author, name_title) = guess_from_file_name(file_name);
    if info.title.trim().is_empty() {
        info.title = name_title;
    }
    if info.authors.is_empty() {
        if let Some(a) = name_author {
            info.authors.push(a);
        }
    }
    Ok((info, cover))
}

/// "Лев Толстой - Анна Каренина.epub" → (Some("Лев Толстой"), "Анна Каренина").
pub fn guess_from_file_name(file_name: &str) -> (Option<String>, String) {
    let base = file_name.rsplit('/').next().unwrap_or(file_name);
    let lower = base.to_lowercase();
    let stem_len = [".fb2.zip", ".epub", ".fb2", ".txt", ".md"]
        .iter()
        .find(|ext| lower.ends_with(*ext))
        .map(|ext| base.len() - ext.len())
        .unwrap_or(base.len());
    let stem = base[..stem_len].replace('_', " ");
    match stem.split_once(" - ") {
        Some((a, t)) if !a.trim().is_empty() && !t.trim().is_empty() => {
            (Some(a.trim().to_string()), t.trim().to_string())
        }
        _ => (None, stem.trim().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_guess() {
        assert_eq!(
            guess_from_file_name("/Books/Лев Толстой - Анна Каренина.epub"),
            (Some("Лев Толстой".into()), "Анна Каренина".into())
        );
        assert_eq!(
            guess_from_file_name("moby_dick.fb2.zip"),
            (None, "moby dick".into())
        );
    }
}

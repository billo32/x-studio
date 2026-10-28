//! Book metadata the EPUB clean-up writes into the OPF, and the file naming rule.
//! Ported from yaebook-core (MIT, see LICENSE-yaebook).

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SeriesInfo {
    pub title: String,
    pub position_label: String,
    pub uuid: String,
}

/// What a plugin knows about a book; everything is optional except a title.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BookMetadata {
    pub uuid: String,
    pub title: String,
    pub authors: Vec<String>,
    pub annotation: String,
    pub genres: Vec<String>,
    pub publishers: Vec<String>,
    pub series: Vec<SeriesInfo>,
    pub cover_url: Option<String>,
    /// Where the book came from (a URL), written as `dc:source`.
    pub source: String,
    pub original_year: Option<String>,
    pub rights: Option<String>,
}

/// "Фамилия Имя. Название.epub" — the file name a book gets on the reader.
pub fn file_name(title: &str, authors: &[String]) -> String {
    let author = authors
        .first()
        .map(String::as_str)
        .filter(|a| !a.is_empty())
        .map(format_author_name)
        .unwrap_or_else(|| "Unknown author".into());
    let title = if title.is_empty() {
        "Unknown title"
    } else {
        title
    };
    sanitize_filename(&format!(
        "{}. {}.epub",
        sanitize_filename(&author),
        sanitize_filename(title)
    ))
}

/// Like [`file_name`], as a path under `dir`.
pub fn output_path(dir: &Path, meta: &BookMetadata) -> std::path::PathBuf {
    dir.join(file_name(&meta.title, &meta.authors))
}

pub fn sanitize_filename(value: &str) -> String {
    let replaced: String = value
        .chars()
        .map(|c| match c {
            '\\' | '/' | '|' => '-',
            ':' => ';',
            '*' => 'x',
            '?' => '!',
            '"' => '\'',
            '<' => '(',
            '>' => ')',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let cleaned = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "Unknown".into()
    } else {
        cleaned
    }
}

/// "Имя Фамилия" → "Фамилия Имя", "Имя Отчество Фамилия" → "Фамилия Имя Отчество".
pub fn format_author_name(name: &str) -> String {
    let parts: Vec<_> = name.split_whitespace().collect();
    match parts.as_slice() {
        [first, last] => format!("{last} {first}"),
        [first, middle, last] => format!("{last} {first} {middle}"),
        _ => name.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_plugin_metadata() {
        let m: BookMetadata = serde_json::from_str(
            r#"{"title":"Т","authors":["А Б"],"series":[{"title":"Цикл","positionLabel":"2"}],"originalYear":"2024"}"#,
        )
        .unwrap();
        assert_eq!(m.series[0].position_label, "2");
        assert_eq!(m.original_year.as_deref(), Some("2024"));
        assert!(m.genres.is_empty());
    }

    #[test]
    fn names_files() {
        assert_eq!(
            file_name("Мастер и Маргарита", &["Михаил Булгаков".into()]),
            "Булгаков Михаил. Мастер и Маргарита.epub"
        );
        assert_eq!(file_name("", &[]), "Unknown author. Unknown title.epub");
        assert_eq!(sanitize_filename("  A/B: C?  "), "A-B; C!");
        assert_eq!(sanitize_filename(".."), "Unknown");
    }
}

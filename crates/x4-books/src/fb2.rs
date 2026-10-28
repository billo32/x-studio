//! FictionBook 2 (`.fb2`, `.fb2.zip`): metadata, stats and the embedded cover. Read-only for now.

use std::io::{Cursor, Read};

use base64::Engine;
use quick_xml::events::Event;
use quick_xml::Reader;

use crate::text::{count_chars, strip_markup};
use crate::{BookInfo, Cover, Error, Result};

/// Unwraps `.fb2.zip` and decodes the XML using the encoding from its prolog.
pub fn xml_text(bytes: &[u8]) -> Result<String> {
    let raw: Vec<u8> = if bytes.starts_with(b"PK") {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
        let idx = (0..zip.len())
            .find(|&i| {
                zip.by_index_raw(i)
                    .map(|f| f.name().to_lowercase().ends_with(".fb2"))
                    .unwrap_or(false)
            })
            .ok_or_else(|| Error::Format("no .fb2 inside the archive".into()))?;
        let mut out = Vec::new();
        zip.by_index(idx)?.read_to_end(&mut out)?;
        out
    } else {
        bytes.to_vec()
    };
    let head = String::from_utf8_lossy(&raw[..raw.len().min(200)]).to_lowercase();
    let label = head
        .split("encoding=")
        .nth(1)
        .and_then(|s| s.trim_start_matches(['"', '\'']).split(['"', '\'']).next())
        .unwrap_or("utf-8")
        .to_string();
    let enc = encoding_rs::Encoding::for_label(label.as_bytes()).unwrap_or(encoding_rs::UTF_8);
    let (text, _, _) = enc.decode(&raw);
    Ok(text.into_owned())
}

pub fn read(bytes: &[u8]) -> Result<(BookInfo, Option<Cover>)> {
    let xml = xml_text(bytes)?;
    let mut r = Reader::from_str(&xml);
    r.config_mut().check_end_names = false;

    let mut path: Vec<Vec<u8>> = Vec::new();
    let mut info = BookInfo::default();
    let mut author_parts: Vec<String> = Vec::new();
    let mut cover_id: Option<String> = None;
    let mut binary_id: Option<String> = None;
    let mut binary_data = String::new();
    let mut cover_bytes: Option<Vec<u8>> = None;
    let mut images = 0;
    let mut chapters = 0;
    let mut text = String::new();

    let in_title_info = |path: &[Vec<u8>]| path.iter().any(|p| p == b"title-info");
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => {
                let qn = e.name();
                let name = local(qn.as_ref());
                if name == b"section" && path.last().map(|p| p == b"body").unwrap_or(false) {
                    chapters += 1;
                }
                if name == b"binary" {
                    images += 1;
                    binary_id = attr(&e, b"id");
                    binary_data.clear();
                }
                if name == b"author" && in_title_info(&path) {
                    author_parts.clear();
                }
                path.push(name.to_vec());
            }
            Ok(Event::Empty(e)) => {
                let qn = e.name();
                let name = local(qn.as_ref());
                if name == b"image" && path.iter().any(|p| p == b"coverpage") && cover_id.is_none()
                {
                    cover_id = attr(&e, b"href").map(|h| h.trim_start_matches('#').to_string());
                }
                if name == b"sequence" && in_title_info(&path) && info.series.is_none() {
                    info.series = attr(&e, b"name").filter(|s| !s.is_empty());
                    info.series_index = attr(&e, b"number");
                }
            }
            Ok(Event::Text(t)) => {
                let s = t.unescape().map(|c| c.into_owned()).unwrap_or_default();
                let cur = path.last().map(|p| p.as_slice()).unwrap_or(b"");
                if in_title_info(&path) {
                    match cur {
                        b"book-title" => info.title.push_str(s.trim()),
                        b"lang" if info.language.is_none() => {
                            info.language = Some(s.trim().to_string())
                        }
                        b"first-name" | b"middle-name" | b"last-name"
                            if path.iter().any(|p| p == b"author") && !s.trim().is_empty() =>
                        {
                            author_parts.push(s.trim().to_string());
                        }
                        _ => {}
                    }
                } else if cur == b"binary" {
                    binary_data.push_str(&s);
                } else if path.iter().any(|p| p == b"body") {
                    text.push_str(&s);
                    text.push(' ');
                }
            }
            Ok(Event::End(e)) => {
                let name = local(e.name().as_ref()).to_vec();
                if name == b"author"
                    && in_title_info(&path[..path.len().saturating_sub(1)])
                    && !author_parts.is_empty()
                {
                    info.authors.push(author_parts.join(" "));
                    author_parts.clear();
                }
                if name == b"binary" {
                    if cover_bytes.is_none() && binary_id.is_some() && binary_id == cover_id {
                        let clean: String =
                            binary_data.chars().filter(|c| !c.is_whitespace()).collect();
                        cover_bytes = base64::engine::general_purpose::STANDARD.decode(clean).ok();
                    }
                    binary_id = None;
                }
                path.pop();
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(Error::Format(format!("FB2: {e}"))),
            _ => {}
        }
    }

    info.chars = count_chars(&strip_markup(&text));
    info.chapters = chapters.max(1);
    info.images = images;
    info.editable = false;
    info.finish_stats();
    Ok((info, cover_bytes.map(|bytes| Cover { bytes, path: None })))
}

fn local(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|&b| b == b':') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

fn attr(e: &quick_xml::events::BytesStart, key: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| local(a.key.as_ref()) == key)
        .map(|a| String::from_utf8_lossy(&a.value).into_owned())
}

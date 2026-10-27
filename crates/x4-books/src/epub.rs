//! EPUB 2/3: package metadata, stats, cover lookup, and rewriting with edits.

use std::collections::HashMap;
use std::io::{Cursor, Read, Write};

use quick_xml::events::Event;
use quick_xml::Reader;
use regex::Regex;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::text::{count_chars, strip_markup, xml_escape, xml_unescape};
use crate::{BookInfo, Cover, Error, Result};

#[derive(Debug, Clone)]
struct Item {
    id: String,
    /// Full path inside the archive.
    path: String,
    media_type: String,
    properties: String,
}

/// The parsed package document.
#[derive(Debug, Clone, Default)]
pub struct Package {
    pub opf_path: String,
    pub opf: String,
    items: Vec<Item>,
    spine: Vec<String>,
    toc_id: Option<String>,
    cover_id: Option<String>,
    pub title: String,
    pub authors: Vec<String>,
    pub language: Option<String>,
    pub series: Option<String>,
    pub series_index: Option<String>,
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
        .map(|a| xml_unescape(&String::from_utf8_lossy(&a.value)))
}

/// Resolves `href` relative to the directory of `base`, decoding `%xx`.
fn resolve(base: &str, href: &str) -> String {
    let href = href.split('#').next().unwrap_or(href);
    let href = percent_decode(href);
    let mut parts: Vec<&str> = match base.rfind('/') {
        Some(i) => base[..i].split('/').collect(),
        None => Vec::new(),
    };
    for seg in href.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(v) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn read_entry(zip: &mut ZipArchive<Cursor<&[u8]>>, path: &str) -> Option<Vec<u8>> {
    let mut f = zip.by_name(path).ok()?;
    let mut buf = Vec::with_capacity(f.size() as usize);
    f.read_to_end(&mut buf).ok()?;
    Some(buf)
}

fn read_text(zip: &mut ZipArchive<Cursor<&[u8]>>, path: &str) -> Option<String> {
    read_entry(zip, path).map(|b| crate::text::decode(&b))
}

fn container_rootfile(zip: &mut ZipArchive<Cursor<&[u8]>>) -> Result<String> {
    let xml = read_text(zip, "META-INF/container.xml")
        .ok_or_else(|| Error::Format("missing META-INF/container.xml".into()))?;
    let mut r = Reader::from_str(&xml);
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e))
                if local(e.name().as_ref()) == b"rootfile" =>
            {
                if let Some(p) = attr(&e, b"full-path") {
                    return Ok(p);
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(Error::Format(format!("container.xml: {e}"))),
            _ => {}
        }
    }
    Err(Error::Format("container.xml has no rootfile".into()))
}

/// Parses the OPF of an opened archive.
fn package(zip: &mut ZipArchive<Cursor<&[u8]>>) -> Result<Package> {
    let opf_path = container_rootfile(zip)?;
    let opf =
        read_text(zip, &opf_path).ok_or_else(|| Error::Format(format!("missing {opf_path}")))?;
    let mut p = Package {
        opf_path: opf_path.clone(),
        opf: opf.clone(),
        ..Default::default()
    };

    let mut r = Reader::from_str(&opf);
    let mut text_target: Option<&'static str> = None;
    let mut buf = String::new();
    let mut in_metadata = false;
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => {
                let name = local(e.name().as_ref()).to_vec();
                match name.as_slice() {
                    b"metadata" => in_metadata = true,
                    b"title" if in_metadata && p.title.is_empty() => text_target = Some("title"),
                    b"creator" if in_metadata => text_target = Some("creator"),
                    b"language" if in_metadata && p.language.is_none() => {
                        text_target = Some("language")
                    }
                    b"meta"
                        if in_metadata
                            && attr(&e, b"property").as_deref()
                                == Some("belongs-to-collection") =>
                    {
                        text_target = Some("collection")
                    }
                    _ => {}
                }
                handle_empty(&mut p, &e, &opf_path);
                buf.clear();
            }
            Ok(Event::Empty(e)) => handle_empty(&mut p, &e, &opf_path),
            Ok(Event::Text(t)) if text_target.is_some() => {
                buf.push_str(&t.unescape().map(|c| c.into_owned()).unwrap_or_default());
            }
            Ok(Event::CData(t)) if text_target.is_some() => {
                buf.push_str(&String::from_utf8_lossy(&t))
            }
            Ok(Event::End(e)) => {
                if local(e.name().as_ref()) == b"metadata" {
                    in_metadata = false;
                }
                if let Some(target) = text_target.take() {
                    let v = buf.trim().to_string();
                    if !v.is_empty() {
                        match target {
                            "title" => p.title = v,
                            "creator" => p.authors.push(v),
                            "language" => p.language = Some(v),
                            "collection" if p.series.is_none() => p.series = Some(v),
                            _ => {}
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(Error::Format(format!("OPF: {e}"))),
            _ => {}
        }
    }
    Ok(p)
}

fn handle_empty(p: &mut Package, e: &quick_xml::events::BytesStart, opf_path: &str) {
    match local(e.name().as_ref()) {
        b"item" => {
            if let (Some(id), Some(href)) = (attr(e, b"id"), attr(e, b"href")) {
                p.items.push(Item {
                    id,
                    path: resolve(opf_path, &href),
                    media_type: attr(e, b"media-type").unwrap_or_default(),
                    properties: attr(e, b"properties").unwrap_or_default(),
                });
            }
        }
        b"itemref" => {
            if let Some(id) = attr(e, b"idref") {
                p.spine.push(id);
            }
        }
        b"spine" => p.toc_id = attr(e, b"toc"),
        b"meta" => match attr(e, b"name").as_deref() {
            Some("cover") => p.cover_id = attr(e, b"content"),
            Some("calibre:series") => p.series = attr(e, b"content").filter(|s| !s.is_empty()),
            Some("calibre:series_index") => p.series_index = attr(e, b"content"),
            _ => {}
        },
        _ => {}
    }
}

impl Package {
    fn item(&self, id: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    /// Archive path of the cover image, by the usual conventions in order of reliability.
    fn cover_path(&self) -> Option<String> {
        let is_image = |i: &&Item| i.media_type.starts_with("image/");
        if let Some(i) = self
            .cover_id
            .as_deref()
            .and_then(|id| self.item(id))
            .filter(is_image)
        {
            return Some(i.path.clone());
        }
        if let Some(i) = self
            .items
            .iter()
            .find(|i| i.properties.split_whitespace().any(|p| p == "cover-image"))
        {
            return Some(i.path.clone());
        }
        self.items
            .iter()
            .filter(is_image)
            .find(|i| {
                i.id.to_lowercase().contains("cover") || i.path.to_lowercase().contains("cover")
            })
            .map(|i| i.path.clone())
    }
}

/// Reads metadata, stats and the cover.
pub fn read(bytes: &[u8]) -> Result<(BookInfo, Option<Cover>)> {
    let mut zip = ZipArchive::new(Cursor::new(bytes))?;
    let p = package(&mut zip)?;

    let mut chars = 0;
    let mut first_doc: Option<(String, String)> = None;
    for id in &p.spine {
        if let Some(item) = p.item(id) {
            if let Some(html) = read_text(&mut zip, &item.path) {
                chars += count_chars(&strip_markup(&html));
                if first_doc.is_none() {
                    first_doc = Some((item.path.clone(), html));
                }
            }
        }
    }

    let chapters = toc_entries(&mut zip, &p)
        .filter(|&n| n > 0)
        .unwrap_or(p.spine.len());
    let images = p
        .items
        .iter()
        .filter(|i| i.media_type.starts_with("image/"))
        .count();

    // Cover: declared one, else the first image of the first document (a common "cover page").
    let mut cover_path = p.cover_path();
    if cover_path.is_none() {
        if let Some((doc_path, html)) = &first_doc {
            let re = Regex::new(r#"(?i)<(?:img|image)\b[^>]*?(?:src|href)\s*=\s*["']([^"']+)["']"#)
                .unwrap();
            cover_path = re.captures(html).map(|c| resolve(doc_path, &c[1]));
        }
    }
    let cover = cover_path.and_then(|path| {
        read_entry(&mut zip, &path).map(|bytes| Cover {
            bytes,
            path: Some(path),
        })
    });

    let mut info = BookInfo {
        title: p.title.clone(),
        authors: p.authors.clone(),
        language: p.language.clone(),
        series: p.series.clone(),
        series_index: p.series_index.clone(),
        chars,
        chapters,
        images,
        editable: true,
        ..Default::default()
    };
    info.finish_stats();
    Ok((info, cover))
}

/// Entries in the NCX (EPUB 2) or the nav document (EPUB 3).
fn toc_entries(zip: &mut ZipArchive<Cursor<&[u8]>>, p: &Package) -> Option<usize> {
    if let Some(nav) = p
        .items
        .iter()
        .find(|i| i.properties.split_whitespace().any(|x| x == "nav"))
    {
        let html = read_text(zip, &nav.path)?;
        let re =
            Regex::new(r#"(?is)<nav\b[^>]*epub:type\s*=\s*["']toc["'][^>]*>(.*?)</nav>"#).unwrap();
        let body = re.captures(&html).map(|c| c[1].to_string()).unwrap_or(html);
        return Some(body.matches("<a ").count() + body.matches("<a>").count());
    }
    let ncx = p.toc_id.as_deref().and_then(|id| p.item(id))?;
    let xml = read_text(zip, &ncx.path)?;
    Some(xml.matches("<navPoint").count())
}

/// Changes to apply when saving an EPUB. `None` leaves a field as it is.
#[derive(Debug, Clone, Default)]
pub struct Edits {
    pub title: Option<String>,
    /// Replaces all creators with this single one.
    pub author: Option<String>,
    pub language: Option<String>,
    /// `Some("")` removes the series.
    pub series: Option<String>,
    /// New cover image bytes (already sized for the reader) and its extension ("jpg" | "png").
    pub cover: Option<(Vec<u8>, &'static str)>,
}

/// Writes a new EPUB with `edits` applied. Unchanged entries are copied without recompression.
pub fn rewrite(bytes: &[u8], edits: &Edits) -> Result<Vec<u8>> {
    let mut zip = ZipArchive::new(Cursor::new(bytes))?;
    let p = package(&mut zip)?;
    let mut opf = p.opf.clone();

    if let Some(t) = &edits.title {
        opf = set_dc(&opf, "title", t, false);
    }
    if let Some(a) = &edits.author {
        opf = set_dc(&opf, "creator", a, true);
    }
    if let Some(l) = &edits.language {
        opf = set_dc(&opf, "language", l, false);
    }
    if let Some(s) = &edits.series {
        opf = set_series(&opf, s);
    }

    // Where the new cover goes: over the existing one, or a new manifest item.
    let mut replace: HashMap<String, Vec<u8>> = HashMap::new();
    let mut add: Vec<(String, Vec<u8>)> = Vec::new();
    if let Some((img, ext)) = &edits.cover {
        match p.cover_path() {
            Some(path) => {
                let same_kind = path.to_lowercase().ends_with(ext)
                    || (*ext == "jpg" && path.to_lowercase().ends_with(".jpeg"));
                if same_kind {
                    replace.insert(path, img.clone());
                } else {
                    // Keep the href, change the bytes and media-type to match.
                    replace.insert(path.clone(), img.clone());
                    let mime = if *ext == "png" {
                        "image/png"
                    } else {
                        "image/jpeg"
                    };
                    opf = set_item_media_type(&opf, &p, &path, mime);
                }
            }
            None => {
                let dir = p
                    .opf_path
                    .rfind('/')
                    .map(|i| &p.opf_path[..=i])
                    .unwrap_or("");
                let name = format!("xstudio-cover.{ext}");
                let mime = if *ext == "png" {
                    "image/png"
                } else {
                    "image/jpeg"
                };
                let epub3 = Regex::new(r#"<package\b[^>]*version\s*=\s*["']3"#)
                    .unwrap()
                    .is_match(&opf);
                let props = if epub3 {
                    r#" properties="cover-image""#
                } else {
                    ""
                };
                opf = insert_before(
                    &opf,
                    "</manifest>",
                    &format!(
                        r#"<item id="xstudio-cover" href="{name}" media-type="{mime}"{props}/>"#
                    ),
                );
                opf = remove_meta_named(&opf, "cover");
                opf = insert_before(
                    &opf,
                    "</metadata>",
                    r#"<meta name="cover" content="xstudio-cover"/>"#,
                );
                add.push((format!("{dir}{name}"), img.clone()));
            }
        }
    }
    replace.insert(p.opf_path.clone(), opf.into_bytes());

    let mut out = ZipWriter::new(Cursor::new(Vec::with_capacity(bytes.len() + 64 * 1024)));
    // The mimetype entry must come first and be stored.
    out.start_file(
        "mimetype",
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
    )?;
    out.write_all(b"application/epub+zip")?;
    for i in 0..zip.len() {
        let name = zip.by_index_raw(i)?.name().to_string();
        if name == "mimetype" {
            continue;
        }
        if let Some(data) = replace.remove(&name) {
            out.start_file(
                name,
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
            )?;
            out.write_all(&data)?;
        } else {
            let f = zip.by_index_raw(i)?;
            out.raw_copy_file(f)?;
        }
    }
    for (name, data) in add {
        out.start_file(
            name,
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )?;
        out.write_all(&data)?;
    }
    Ok(out.finish()?.into_inner())
}

fn insert_before(s: &str, marker: &str, insert: &str) -> String {
    match s.find(marker) {
        Some(i) => format!("{}{}\n{}", &s[..i], insert, &s[i..]),
        None => s.to_string(),
    }
}

/// Sets the first `<dc:NAME>`; with `only_one`, drops the others. Adds one if missing.
fn set_dc(opf: &str, name: &str, value: &str, only_one: bool) -> String {
    let re = Regex::new(&format!(
        r"(?s)<((?:\w+:)?{name})\b([^>]*?)(?:/>|>.*?</(?:\w+:)?{name}\s*>)"
    ))
    .unwrap();
    let file_as = Regex::new(r#"\s(?:opf:)?file-as\s*=\s*("[^"]*"|'[^']*')"#).unwrap();
    let mut seen = false;
    let mut out = String::with_capacity(opf.len());
    let mut last = 0;
    for m in re.captures_iter(opf) {
        let whole = m.get(0).unwrap();
        out.push_str(&opf[last..whole.start()]);
        if !seen {
            let tag = &m[1];
            // Drop opf:file-as and similar attributes that would now be stale.
            let attrs = file_as.replace_all(&m[2], "");
            out.push_str(&format!("<{tag}{attrs}>{}</{tag}>", xml_escape(value)));
            seen = true;
        } else if !only_one {
            out.push_str(whole.as_str());
        }
        last = whole.end();
    }
    out.push_str(&opf[last..]);
    if !seen {
        return add_to_metadata(
            &out,
            &format!("<dc:{name}>{}</dc:{name}>", xml_escape(value)),
        );
    }
    out
}

/// Inserts right after the opening `<metadata …>` tag.
fn add_to_metadata(opf: &str, element: &str) -> String {
    let re = Regex::new(r"<(?:\w+:)?metadata\b[^>]*>").unwrap();
    match re.find(opf) {
        Some(m) => format!("{}\n{}{}", &opf[..m.end()], element, &opf[m.end()..]),
        None => opf.to_string(),
    }
}

fn remove_meta_named(opf: &str, name: &str) -> String {
    let re = Regex::new(&format!(
        r#"(?s)<(?:\w+:)?meta\b[^>]*\bname\s*=\s*["']{}["'][^>]*?(?:/>|>.*?</(?:\w+:)?meta>)\s*"#,
        regex::escape(name)
    ))
    .unwrap();
    re.replace_all(opf, "").into_owned()
}

fn set_series(opf: &str, series: &str) -> String {
    let mut out = remove_meta_named(opf, "calibre:series");
    if !series.trim().is_empty() {
        out = insert_before(
            &out,
            "</metadata>",
            &format!(
                r#"<meta name="calibre:series" content="{}"/>"#,
                xml_escape(series.trim())
            ),
        );
    } else {
        out = remove_meta_named(&out, "calibre:series_index");
    }
    out
}

fn set_item_media_type(opf: &str, p: &Package, path: &str, mime: &str) -> String {
    let Some(item) = p.items.iter().find(|i| i.path == path) else {
        return opf.to_string();
    };
    let re = Regex::new(&format!(
        r#"(<(?:\w+:)?item\b[^>]*\bid\s*=\s*["']{}["'][^>]*?\bmedia-type\s*=\s*["'])[^"']*(["'])"#,
        regex::escape(&item.id)
    ))
    .unwrap();
    re.replace(opf, format!("${{1}}{mime}${{2}}").as_str())
        .into_owned()
}

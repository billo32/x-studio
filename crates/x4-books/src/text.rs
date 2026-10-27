//! Plain text: decoding (UTF-8 or Windows-1251) and markup stripping shared by the other formats.

use std::sync::OnceLock;

use regex::Regex;

use crate::BookInfo;

/// Decodes UTF-8 (with or without BOM), falling back to Windows-1251, the usual
/// encoding of older Russian TXT and FB2 files.
pub fn decode(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::WINDOWS_1251.decode(bytes).0.into_owned(),
    }
}

pub fn read(bytes: &[u8]) -> BookInfo {
    let text = decode(bytes);
    let mut info = BookInfo {
        chars: count_chars(&text),
        chapters: 1,
        ..Default::default()
    };
    info.finish_stats();
    info
}

/// Counts visible characters, collapsing runs of whitespace into one.
pub fn count_chars(text: &str) -> usize {
    let mut n = 0;
    let mut in_space = true;
    for c in text.chars() {
        if c.is_whitespace() {
            if !in_space {
                n += 1;
                in_space = true;
            }
        } else {
            n += 1;
            in_space = false;
        }
    }
    n
}

/// Text content of an (X)HTML/XML fragment: drops head/script/style, tags and entities.
pub fn strip_markup(markup: &str) -> String {
    static BLOCKS: OnceLock<Regex> = OnceLock::new();
    static TAGS: OnceLock<Regex> = OnceLock::new();
    static ENTITIES: OnceLock<Regex> = OnceLock::new();
    let blocks = BLOCKS.get_or_init(|| {
        Regex::new(r"(?is)<(head|script|style|binary)\b.*?</(head|script|style|binary)>").unwrap()
    });
    let tags = TAGS.get_or_init(|| Regex::new(r"(?s)<[^>]*>").unwrap());
    let entities = ENTITIES.get_or_init(|| Regex::new(r"&#?[A-Za-z0-9]+;").unwrap());
    let no_blocks = blocks.replace_all(markup, " ");
    let no_tags = tags.replace_all(&no_blocks, " ");
    entities.replace_all(&no_tags, "x").into_owned()
}

/// Escapes text for XML content and attribute values.
pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Undoes the five XML entities and numeric references.
pub fn xml_unescape(s: &str) -> String {
    quick_xml::escape::unescape(s)
        .map(|c| c.into_owned())
        .unwrap_or_else(|_| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cp1251_and_counting() {
        let (bytes, _, _) = encoding_rs::WINDOWS_1251.encode("Привет,   мир");
        assert_eq!(decode(&bytes), "Привет,   мир");
        assert_eq!(count_chars("  a  b\n\nc "), 6);
        assert_eq!(
            count_chars(&strip_markup(
                "<html><head><title>x</title></head><body><p>Да &amp; нет</p></body></html>"
            ))
            .min(20),
            9
        );
    }
}

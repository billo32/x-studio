//! X Studio plugins: the manifest format, its permission rules, and installing from ZIP.
//!
//! A plugin is a folder with `manifest.json` and a web page. The app runs the page in a
//! sandboxed iframe with no network access of its own; everything it does (HTTP, the reader,
//! stored tokens) goes through the app, which checks the manifest's permissions. This crate
//! holds the parts of that which don't need Tauri, so they can be tested.

mod install;
mod manifest;

use thiserror::Error;

pub use install::{installed_manifest, resolve, Package, MAX_FILES, MAX_UNPACKED, MAX_ZIP};
pub use manifest::{
    host_matches, valid_id, BookAction, Contributes, DeviceAccess, Manifest, PermissionLine,
    Permissions, Section, API_VERSION,
};

#[derive(Debug, Error)]
pub enum Error {
    #[error("manifest.json: {0}")]
    Manifest(String),
    #[error("{0}")]
    Package(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// The value of `param` in a URL's fragment or query (`…#access_token=…&…`), as a sign-in
/// redirect carries it. Percent-escapes are decoded.
pub fn param_from_url(url: &str, param: &str) -> Option<String> {
    let fragment = url.split_once('#').map(|(_, f)| f);
    let query = url
        .split_once('?')
        .map(|(_, q)| q.split('#').next().unwrap_or(q));
    let key = format!("{param}=");
    fragment
        .into_iter()
        .chain(query)
        .flat_map(|p| p.split('&'))
        .find_map(|kv| kv.strip_prefix(&key))
        .filter(|v| !v.is_empty())
        .map(percent_decode)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Replaces `{{secret:NAME}}` in a header value with the stored secret. Fails on a secret
/// that isn't set, naming it, so the plugin can ask the user to sign in.
pub fn fill_secrets(
    value: &str,
    lookup: impl Fn(&str) -> Option<String>,
) -> std::result::Result<String, String> {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("{{secret:") {
        out.push_str(&rest[..start]);
        let after = &rest[start + "{{secret:".len()..];
        let end = after
            .find("}}")
            .ok_or_else(|| "unterminated {{secret:…}}".to_string())?;
        let name = &after[..end];
        if !valid_secret_name(name) {
            return Err(format!("bad secret name \"{name}\""));
        }
        out.push_str(&lookup(name).ok_or_else(|| format!("secret \"{name}\" is not set"))?);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

pub fn valid_secret_name(name: &str) -> bool {
    (1..=40).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Host of an `https://` URL; `None` for anything else (plain http, file:, credentials in
/// the URL, IP literals with ports are fine).
pub fn https_host(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.contains('@') || authority.is_empty() {
        return None;
    }
    let host = match authority.rsplit_once(':') {
        Some((h, port)) if port.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => authority,
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// A file name a plugin may give a book on the reader: no folders, a book extension.
pub fn safe_book_name(name: &str) -> Option<String> {
    const EXT: &[&str] = &["epub", "fb2", "txt", "md", "xtc", "xtch"];
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim().to_string();
    let (stem, ext) = cleaned.rsplit_once('.')?;
    let ok = !stem.trim().is_empty()
        && cleaned.len() <= 200
        && EXT.contains(&ext.to_ascii_lowercase().as_str());
    ok.then_some(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_redirect_params() {
        assert_eq!(
            param_from_url(
                "https://login.example.test/done#access_token=y0_a%2Bb&token_type=bearer",
                "access_token"
            )
            .as_deref(),
            Some("y0_a+b")
        );
        assert_eq!(
            param_from_url("https://x.test/cb?code=abc#frag", "code").as_deref(),
            Some("abc")
        );
        assert_eq!(
            param_from_url("https://x.test/#access_token=", "access_token"),
            None
        );
        assert_eq!(param_from_url("https://x.test/?a=1", "access_token"), None);
        assert_eq!(
            param_from_url("https://x.test/?t=%zz", "t").as_deref(),
            Some("%zz")
        );
    }

    #[test]
    fn fills_secrets() {
        let look = |n: &str| (n == "token").then(|| "y0_abc".to_string());
        assert_eq!(
            fill_secrets("OAuth {{secret:token}}", look).unwrap(),
            "OAuth y0_abc"
        );
        assert_eq!(fill_secrets("plain", look).unwrap(), "plain");
        assert!(fill_secrets("{{secret:other}}", look)
            .unwrap_err()
            .contains("other"));
        assert!(fill_secrets("{{secret:token", look).is_err());
        assert!(fill_secrets("{{secret:../x}}", look).is_err());
    }

    #[test]
    fn checks_urls_and_names() {
        assert_eq!(
            https_host("https://API.example.com:443/v5?x").as_deref(),
            Some("api.example.com")
        );
        assert_eq!(https_host("http://example.com/"), None);
        assert_eq!(https_host("https://user@evil.com/"), None);
        assert_eq!(https_host("https:///x"), None);
        assert_eq!(
            safe_book_name("Булгаков. Мастер и Маргарита.epub").as_deref(),
            Some("Булгаков. Мастер и Маргарита.epub")
        );
        assert_eq!(
            safe_book_name("../../a.epub").as_deref(),
            Some("-..-a.epub")
        );
        assert_eq!(safe_book_name("run.sh"), None);
        assert_eq!(safe_book_name(".epub"), None);
    }
}

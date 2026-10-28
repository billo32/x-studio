//! `manifest.json`: who the plugin is, what it may touch, and where it plugs into the app.

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// The plugin API this build of X Studio implements.
pub const API_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    /// Reverse-DNS style id, e.g. `dev.example.book-store`. Also the install folder name.
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub homepage: String,
    pub api_version: u32,
    /// The page the plugin runs in, relative to the plugin root.
    #[serde(default = "default_entry")]
    pub entry: String,
    /// Icon (SVG or PNG) relative to the plugin root.
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub permissions: Permissions,
    #[serde(default)]
    pub contributes: Contributes,
}

fn default_entry() -> String {
    "index.html".into()
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Permissions {
    /// Hosts the plugin may send HTTPS requests to: `example.com`, `*.example.com`, or `*`.
    #[serde(default)]
    pub network: Vec<String>,
    /// Hosts a sign-in window may open (the token is captured by the app, not the plugin).
    #[serde(default)]
    pub auth_window: Vec<String>,
    /// Access to the reader: `read` (list books), `write` (send books).
    #[serde(default)]
    pub device: Vec<DeviceAccess>,
    /// Write-only secrets (tokens) the app inserts into requests for the plugin.
    #[serde(default)]
    pub secrets: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DeviceAccess {
    Read,
    Write,
}

/// Unknown keys are ignored here, so a plugin keeps loading when a contribution point is
/// added or retired between app versions.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Contributes {
    /// A page of its own in the sidebar.
    #[serde(default)]
    pub section: Option<Section>,
    /// Commands in the book editor.
    #[serde(default)]
    pub book_actions: Vec<BookAction>,
    /// The plugin page has a settings view (opened from the Plugins page).
    #[serde(default)]
    pub settings: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Section {
    pub title: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BookAction {
    pub id: String,
    pub title: String,
    /// File extensions the action applies to (`epub`, `fb2`, …); empty = all books.
    #[serde(default)]
    pub formats: Vec<String>,
}

impl Manifest {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let m: Manifest =
            serde_json::from_slice(bytes).map_err(|e| Error::Manifest(e.to_string()))?;
        m.validate()?;
        Ok(m)
    }

    pub fn validate(&self) -> Result<()> {
        let bad = |what: String| Err(Error::Manifest(what));
        if !valid_id(&self.id) {
            return bad(format!(
                "id \"{}\": use lowercase letters, digits, dots and dashes (3–64 chars)",
                self.id
            ));
        }
        if self.name.trim().is_empty() || self.name.chars().count() > 60 {
            return bad("name: 1–60 characters".into());
        }
        if self.version.is_empty()
            || self.version.len() > 32
            || !self
                .version
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || ".-+".contains(c))
        {
            return bad(format!("version \"{}\"", self.version));
        }
        if self.api_version != API_VERSION {
            return bad(format!(
                "apiVersion {} — this X Studio supports {API_VERSION}",
                self.api_version
            ));
        }
        if !valid_rel_path(&self.entry) {
            return bad(format!("entry \"{}\"", self.entry));
        }
        if let Some(icon) = &self.icon {
            if !valid_rel_path(icon) {
                return bad(format!("icon \"{icon}\""));
            }
        }
        for h in self
            .permissions
            .network
            .iter()
            .chain(&self.permissions.auth_window)
        {
            if !valid_host_pattern(h) {
                return bad(format!("host \"{h}\": use example.com, *.example.com or *"));
            }
        }
        if self.permissions.auth_window.iter().any(|h| h == "*") {
            return bad("authWindow: \"*\" is not allowed, list the sign-in hosts".into());
        }
        let c = &self.contributes;
        if let Some(s) = &c.section {
            if s.title.trim().is_empty() || s.title.chars().count() > 40 {
                return bad("contributes.section.title: 1–40 characters".into());
            }
        }
        let mut ids = std::collections::HashSet::new();
        for a in &c.book_actions {
            let id_ok = (1..=40).contains(&a.id.len())
                && a.id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
            if !id_ok {
                return bad(format!("bookActions id \"{}\"", a.id));
            }
            if !ids.insert(a.id.as_str()) {
                return bad(format!("bookActions id \"{}\" repeats", a.id));
            }
            if a.title.trim().is_empty() || a.title.chars().count() > 40 {
                return bad("bookActions title: 1–40 characters".into());
            }
        }
        if c.section.is_none() && c.book_actions.is_empty() {
            return bad("contributes: add a section or bookActions".into());
        }
        Ok(())
    }

    /// May the plugin send requests to `host`?
    pub fn allows_network(&self, host: &str) -> bool {
        self.permissions
            .network
            .iter()
            .any(|p| host_matches(p, host))
    }

    /// May the plugin open a sign-in window on `host`?
    pub fn allows_auth_window(&self, host: &str) -> bool {
        self.permissions
            .auth_window
            .iter()
            .any(|p| host_matches(p, host))
    }

    pub fn allows_device(&self, access: DeviceAccess) -> bool {
        self.permissions.device.contains(&access)
    }

    /// Permission lines for the install dialog, most sensitive first.
    pub fn permission_summary(&self) -> Vec<PermissionLine> {
        let p = &self.permissions;
        let mut out = Vec::new();
        if p.network.iter().any(|h| h == "*") {
            out.push(PermissionLine::warn(
                "Send requests to any website (it can send anything it has to anyone)",
            ));
        } else if !p.network.is_empty() {
            out.push(PermissionLine::info(format!(
                "Connect to {}",
                p.network.join(", ")
            )));
        }
        if !p.auth_window.is_empty() {
            out.push(PermissionLine::info(format!(
                "Open a sign-in window for {}",
                p.auth_window.join(", ")
            )));
        }
        if p.secrets {
            out.push(PermissionLine::info(
                "Keep sign-in tokens (stored by X Studio, the plugin can't read them back)",
            ));
        }
        if self.allows_device(DeviceAccess::Write) {
            out.push(PermissionLine::info("Send books to the reader"));
        }
        if self.allows_device(DeviceAccess::Read) {
            out.push(PermissionLine::info("See the list of books on the reader"));
        }
        if !self.contributes.book_actions.is_empty() {
            out.push(PermissionLine::info(
                "Change a book when you run one of its commands in the editor",
            ));
        }
        if out.is_empty() {
            out.push(PermissionLine::info(
                "No access to the network or the reader",
            ));
        }
        out
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PermissionLine {
    pub text: String,
    pub warning: bool,
}

impl PermissionLine {
    fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            warning: false,
        }
    }
    fn warn(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            warning: true,
        }
    }
}

pub fn valid_id(id: &str) -> bool {
    (3..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id.as_bytes()[id.len() - 1].is_ascii_alphanumeric()
        && !id.contains("..")
}

/// A relative path inside the plugin: no `..`, no absolute path, no backslashes.
pub fn valid_rel_path(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 255
        && !p.starts_with('/')
        && !p.contains('\\')
        && !p.contains('\0')
        && p.split('/').all(|s| !s.is_empty() && s != "." && s != "..")
}

fn valid_host_pattern(p: &str) -> bool {
    if p == "*" {
        return true;
    }
    let host = p.strip_prefix("*.").unwrap_or(p);
    valid_host(host) && host.contains('.')
}

fn valid_host(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 253
        && h.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !l.starts_with('-')
                && !l.ends_with('-')
        })
}

/// `example.com` matches only itself; `*.example.com` matches subdomains (not the bare domain);
/// `*` matches anything.
pub fn host_matches(pattern: &str, host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let pattern = pattern.to_ascii_lowercase();
    if pattern == "*" {
        return true;
    }
    match pattern.strip_prefix("*.") {
        Some(base) => host.len() > base.len() + 1 && host.ends_with(&format!(".{base}")),
        None => host == pattern,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(json: &str) -> Result<Manifest> {
        Manifest::parse(json.as_bytes())
    }

    const OK: &str = r#"{
        "id": "dev.example.books", "name": "Books", "version": "1.0.0", "apiVersion": 1,
        "permissions": {"network": ["api.example.com", "*.cdn.example.com"], "device": ["write"],
                        "secrets": true, "authWindow": ["oauth.example.com"]},
        "contributes": {"section": {"title": "Books"},  
                        "bookActions": [{"id": "fix", "title": "Fix it", "formats": ["epub"]}], "settings": true}
    }"#;

    #[test]
    fn parses_and_checks_permissions() {
        let m = manifest(OK).unwrap();
        assert_eq!(m.entry, "index.html");
        assert!(m.allows_network("api.example.com"));
        assert!(m.allows_network("API.example.com."));
        assert!(m.allows_network("img.cdn.example.com"));
        assert!(!m.allows_network("cdn.example.com"));
        assert!(!m.allows_network("evil.com"));
        assert!(!m.allows_network("api.example.com.evil.com"));
        assert!(m.allows_auth_window("oauth.example.com"));
        assert!(!m.allows_auth_window("api.example.com"));
        assert!(m.allows_device(DeviceAccess::Write));
        assert!(!m.allows_device(DeviceAccess::Read));
        assert!(m.permission_summary().iter().all(|l| !l.warning));
    }

    #[test]
    fn rejects_bad_manifests() {
        let with = |from: &str, to: &str| manifest(&OK.replacen(from, to, 1));
        assert!(with("dev.example.books", "../evil").is_err());
        assert!(with("dev.example.books", "Dev.Books").is_err());
        assert!(with("\"apiVersion\": 1", "\"apiVersion\": 2").is_err());
        assert!(with("api.example.com", "http://api.example.com").is_err());
        assert!(with("oauth.example.com", "*").is_err());
        assert!(with("\"apiVersion\": 1", "\"apiVersion\": 1, \"extra\": 1").is_err());
        // A retired contribution point (bookSource) doesn't break an installed plugin.
        assert!(with(
            "\"settings\": true",
            "\"settings\": true, \"bookSource\": {\"title\": \"x\"}"
        )
        .is_ok());
        assert!(with("\"version\": \"1.0.0\"", "\"version\": \"1 0\"").is_err());
        assert!(with(
            "\"apiVersion\": 1",
            "\"apiVersion\": 1, \"entry\": \"../x.html\""
        )
        .is_err());
        assert!(manifest(
            r#"{"id":"a.b.c","name":"x","version":"1","apiVersion":1,"contributes":{}}"#
        )
        .is_err());
    }

    #[test]
    fn warns_about_any_website() {
        let m = manifest(&OK.replacen("\"api.example.com\"", "\"*\"", 1)).unwrap();
        assert!(m.permission_summary()[0].warning);
        assert!(m.allows_network("anything.org"));
    }

    #[test]
    fn checks_paths() {
        assert!(valid_rel_path("ui/index.html"));
        assert!(!valid_rel_path("../index.html"));
        assert!(!valid_rel_path("/etc/passwd"));
        assert!(!valid_rel_path("a//b"));
        assert!(!valid_rel_path("a\\b"));
    }
}

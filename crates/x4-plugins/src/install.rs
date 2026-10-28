//! Installing a plugin from a ZIP: find and check `manifest.json`, unpack safely, swap in.

use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

use crate::manifest::{valid_rel_path, Manifest};
use crate::{Error, Result};

pub const MAX_ZIP: u64 = 50 * 1024 * 1024;
pub const MAX_UNPACKED: u64 = 200 * 1024 * 1024;
pub const MAX_FILES: usize = 2_000;
const MANIFEST: &str = "manifest.json";

/// A checked plugin archive, ready to install.
pub struct Package {
    pub manifest: Manifest,
    /// (path inside the plugin, contents)
    files: Vec<(String, Vec<u8>)>,
}

impl Package {
    /// Reads and checks a plugin ZIP. The plugin may sit at the archive root or in one
    /// top-level folder (what "Compress" in Finder produces).
    pub fn read(mut zip: impl Read + Seek) -> Result<Self> {
        let len = zip.seek(std::io::SeekFrom::End(0))?;
        zip.rewind()?;
        if len > MAX_ZIP {
            return Err(Error::Package(format!(
                "the archive is larger than {} MB",
                MAX_ZIP / 1024 / 1024
            )));
        }
        let mut archive =
            zip::ZipArchive::new(zip).map_err(|e| Error::Package(format!("not a ZIP: {e}")))?;
        if archive.len() > MAX_FILES {
            return Err(Error::Package(format!("more than {MAX_FILES} files")));
        }

        // Plain file names, junk skipped.
        let mut names = Vec::new();
        for i in 0..archive.len() {
            let f = archive.by_index(i).map_err(zip_err)?;
            let name = f.name().to_string();
            if f.is_dir() || is_junk(&name) {
                continue;
            }
            if f.is_symlink() {
                return Err(Error::Package(format!("{name}: links are not allowed")));
            }
            if !valid_rel_path(&name) {
                return Err(Error::Package(format!("{name}: unsafe path")));
            }
            names.push((i, name));
        }
        let prefix = root_prefix(names.iter().map(|(_, n)| n.as_str()))
            .ok_or_else(|| Error::Package("manifest.json not found".into()))?;

        let mut total = 0u64;
        let mut files = Vec::new();
        for (i, name) in names {
            let Some(rel) = name.strip_prefix(&prefix) else {
                continue; // outside the plugin folder
            };
            let mut f = archive.by_index(i).map_err(zip_err)?;
            let mut data = Vec::new();
            // Don't trust the declared size: cap what is actually inflated.
            (&mut f)
                .take(MAX_UNPACKED - total + 1)
                .read_to_end(&mut data)?;
            total += data.len() as u64;
            if total > MAX_UNPACKED {
                return Err(Error::Package(format!(
                    "unpacks to more than {} MB",
                    MAX_UNPACKED / 1024 / 1024
                )));
            }
            files.push((rel.to_string(), data));
        }

        let manifest = files
            .iter()
            .find(|(n, _)| n == MANIFEST)
            .map(|(_, d)| Manifest::parse(d))
            .transpose()?
            .ok_or_else(|| Error::Package("manifest.json not found".into()))?;
        for needed in std::iter::once(&manifest.entry).chain(manifest.icon.iter()) {
            if !files.iter().any(|(n, _)| n == needed) {
                return Err(Error::Package(format!(
                    "{needed} is missing from the archive"
                )));
            }
        }
        Ok(Package { manifest, files })
    }

    pub fn read_file(path: &Path) -> Result<Self> {
        Self::read(std::fs::File::open(path)?)
    }

    /// Unpacks into `<root>/<id>`, replacing a previous version only once the new one is
    /// fully written.
    pub fn install(&self, root: &Path) -> Result<PathBuf> {
        std::fs::create_dir_all(root)?;
        let dest = root.join(&self.manifest.id);
        let staging = root.join(format!(".{}.new", self.manifest.id));
        let old = root.join(format!(".{}.old", self.manifest.id));
        let _ = std::fs::remove_dir_all(&staging);
        let _ = std::fs::remove_dir_all(&old);
        for (rel, data) in &self.files {
            let path = staging.join(rel);
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, data)?;
        }
        if dest.exists() {
            std::fs::rename(&dest, &old)?;
        }
        if let Err(e) = std::fs::rename(&staging, &dest) {
            if old.exists() {
                let _ = std::fs::rename(&old, &dest); // put the previous version back
            }
            return Err(e.into());
        }
        let _ = std::fs::remove_dir_all(&old);
        Ok(dest)
    }
}

/// "" when manifest.json is at the root, "folder/" when everything sits in one folder.
fn root_prefix<'a>(names: impl Iterator<Item = &'a str> + Clone) -> Option<String> {
    if names.clone().any(|n| n == MANIFEST) {
        return Some(String::new());
    }
    let nested: Vec<&str> = names
        .clone()
        .filter(|n| n.ends_with(&format!("/{MANIFEST}")) && n.matches('/').count() == 1)
        .collect();
    match nested.as_slice() {
        [one] => Some(one[..one.len() - MANIFEST.len()].to_string()),
        _ => None,
    }
}

fn is_junk(name: &str) -> bool {
    name.starts_with("__MACOSX/")
        || name
            .split('/')
            .any(|s| s == ".DS_Store" || s.starts_with("._"))
}

fn zip_err(e: zip::result::ZipError) -> Error {
    Error::Package(e.to_string())
}

/// Reads an installed plugin's manifest.
pub fn installed_manifest(dir: &Path) -> Result<Manifest> {
    Manifest::parse(&std::fs::read(dir.join(MANIFEST))?)
}

/// Resolves a request path inside an installed plugin, refusing anything that escapes it.
pub fn resolve(plugin_dir: &Path, rel: &str) -> Option<PathBuf> {
    let rel = rel.trim_start_matches('/');
    if !valid_rel_path(rel) {
        return None;
    }
    let path = plugin_dir.join(rel);
    let canon = path.canonicalize().ok()?;
    let root = plugin_dir.canonicalize().ok()?;
    (canon.starts_with(&root) && canon.is_file()).then_some(canon)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;

    const MANIFEST_JSON: &str = r#"{"id":"dev.test.plugin","name":"Test","version":"1.0.0","apiVersion":1,
        "contributes":{"section":{"title":"Test"}}}"#;

    fn zip(files: &[(&str, &[u8])]) -> Cursor<Vec<u8>> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in files {
            w.start_file(*name, SimpleFileOptions::default()).unwrap();
            w.write_all(data).unwrap();
        }
        let mut c = w.finish().unwrap();
        c.set_position(0);
        c
    }

    #[test]
    fn installs_from_root_or_one_folder() {
        let dir = tempfile::tempdir().unwrap();
        for archive in [
            zip(&[
                ("manifest.json", MANIFEST_JSON.as_bytes()),
                ("index.html", b"<p>v1"),
                ("js/main.js", b"1"),
            ]),
            zip(&[
                ("test-plugin/manifest.json", MANIFEST_JSON.as_bytes()),
                ("test-plugin/index.html", b"<p>v2"),
                ("__MACOSX/test-plugin/._index.html", b"junk"),
                ("test-plugin/.DS_Store", b"junk"),
            ]),
        ] {
            let pkg = Package::read(archive).unwrap();
            let dest = pkg.install(dir.path()).unwrap();
            assert!(dest.join("index.html").is_file());
        }
        // The second install replaced the first one completely.
        let dest = dir.path().join("dev.test.plugin");
        assert_eq!(std::fs::read(dest.join("index.html")).unwrap(), b"<p>v2");
        assert!(!dest.join("js/main.js").exists());
        assert!(!dest.join(".DS_Store").exists());
        assert_eq!(installed_manifest(&dest).unwrap().id, "dev.test.plugin");
        assert!(resolve(&dest, "index.html").is_some());
        assert!(resolve(&dest, "../dev.test.plugin/index.html").is_none());
        assert!(resolve(&dest, "manifest.json").is_some());
        assert!(resolve(&dest, "missing.js").is_none());
    }

    #[test]
    fn refuses_unsafe_archives() {
        let err = |files: &[(&str, &[u8])]| Package::read(zip(files)).err().unwrap().to_string();
        assert!(err(&[
            ("manifest.json", MANIFEST_JSON.as_bytes()),
            ("index.html", b""),
            ("../../evil.sh", b"x")
        ])
        .contains("unsafe path"));
        assert!(err(&[("index.html", b"")]).contains("manifest.json not found"));
        assert!(
            err(&[("manifest.json", MANIFEST_JSON.as_bytes())]).contains("index.html is missing")
        );
        assert!(err(&[
            ("a/manifest.json", MANIFEST_JSON.as_bytes()),
            ("b/manifest.json", MANIFEST_JSON.as_bytes())
        ])
        .contains("manifest.json not found"));
        assert!(Package::read(Cursor::new(b"not a zip".to_vec())).is_err());
    }
}

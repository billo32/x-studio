//! A plugin ZIP made by scripts/build-plugin.mjs installs. Set PLUGIN_ZIP to check one:
//!   node scripts/build-plugin.mjs path/to/plugin /tmp/out
//!   PLUGIN_ZIP=/tmp/out/<id>-<version>.zip cargo test -p x4-plugins --test bundled

use std::path::Path;

#[test]
fn built_zip_installs() {
    let Ok(zip) = std::env::var("PLUGIN_ZIP") else {
        return;
    };
    let pkg = x4_plugins::Package::read_file(Path::new(&zip)).unwrap();
    let dir = std::env::temp_dir().join(format!("x4-plugins-test-{}", std::process::id()));
    let dest = pkg.install(&dir).unwrap();
    assert!(dest.join(&pkg.manifest.entry).is_file());
    let _ = std::fs::remove_dir_all(dir);
}

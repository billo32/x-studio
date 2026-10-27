# X Studio

A macOS companion app for Xteink e-readers (X4, X4 Classic, X4 Pro, X3) running the
[CrossInk](https://github.com/uxjulia/CrossInk) firmware — or upstream
[CrossPoint](https://github.com/crosspoint-reader/crosspoint-reader) over Wi‑Fi.

- **Books, sleep screens, fonts, dictionaries** — the reader's SD card sorted into sections, not a file tree
- **Library view** in a grid with covers or as a list, with title, author, format and stable page counts
- **Book editor** — title, author, language, series, folder; drop an image on the cover and see
  exactly how it fills the screen (fill, fit or stretch), then write the EPUB back to the reader
- **USB or Wi‑Fi** — the firmware's USB serial protocol (from the home screen) or its File Transfer server

Built with Tauri 2, Rust and Svelte 5.

## Develop

```bash
npm install
npm run tauri dev
```

Requirements: Node 22, Rust stable, Xcode command line tools.

```bash
npm test                      # UI unit tests (Vitest)
cargo test --workspace        # protocol and book tests — no reader needed
npm run check                 # both, plus clippy
```

## Release (signed and notarized)

Needs a **Developer ID Application** certificate in the keychain.

```bash
cp signing.env.example signing.env   # once: notarization credentials (a notarytool keychain profile is simplest)
npm run release                      # signed + notarized .app and .dmg in target/release/bundle
```

Full release — version bump, tests, universal build, notarization, tag and GitHub Release:
double-click `Release.command`, or run `scripts/release.sh 0.2.0`. Artifacts land in `release/<tag>/`.
`scripts/notarize-release.sh <tag>` re-notarizes an existing release and replaces its files.

`npm run verify` checks the newest build the way Gatekeeper does — signature, hardened runtime,
stapled tickets, Gatekeeper verdict for the app, the disk image (also as a quarantined download)
and the app inside it, checksums and recent notarization history. Pass a path to check something
else: `scripts/verify-release.sh release/v0.2.0`.

Pushing a `v*` tag also builds a signed and notarized `.dmg` on GitHub Actions and attaches it to a
draft release; the required repository secrets are listed in `.github/workflows/release.yml`.

## Project layout

```
crates/x4-device/   reader transports, independent of Tauri
  src/usb.rs        CrossInk's binary CMND protocol over USB CDC serial
  src/wifi.rs       File Transfer server: HTTP :80, WebSocket uploads :81, UDP discovery :8134
  tests/            in-memory models of the firmware for both transports
crates/x4-books/    EPUB / FB2 / TXT metadata, stable pages, covers, EPUB rewriting
src-tauri/          Tauri commands, book cache, transfer queue
src/                UI (Svelte 5)
scripts/            build, notarization and release scripts
```

## How it talks to the reader

**USB** (`UsbSerialFileTransfer.cpp` in CrossInk): `CMND` + a command byte. Paths are a `u16 LE`
length plus UTF‑8 (≤ 255 bytes); sizes and CRC32s are `u32 LE`. Commands: `S` status, `A` list,
`K` mkdir, `W` write, `R` remove, `N` rename, `T` read. A write is `READY` → 256‑byte chunks,
each acknowledged with `0x06` → CRC32 → `OK`. The port also carries the firmware's log, so
non-protocol lines are skipped and `BUSY:` lines extend timeouts. File commands are accepted only
while the reader is on its home screen; the app waits for it to return there between files.

**Wi‑Fi** (`CrossPointWebServer.cpp`, `docs/webserver-endpoints.md`): uploads go over WebSocket
as `START:<name>:<size>:<dir>` → `READY` → 4 KB binary frames → `PROGRESS` / `DONE`.

A protocol log is written to `~/Library/Logs/dev.tls1.xstudio/usb.log`.

## Books

On connect the app scans the card and reads each book once in the background — title, author,
cover, stable pages (1,500 characters, as in CrossInk's optimizer and KOReader), chapters. Results
and files are cached in `~/Library/Caches/dev.tls1.xstudio/books`. Hidden folders (`.sleep`,
`.fonts`…) are only visible when *Show hidden files* is on in the reader's settings.

Saving an EPUB rewrites its package metadata and, if the cover was changed, bakes the cover to the
model's screen size (480×800 or 528×792) with the chosen fit.

## Roadmap

1. EPUB optimizer (images, chapter splitting) based on CrossInk's web optimizer
2. FB2 / TXT / DOCX → EPUB conversion (makes FB2 editable)
3. Sleep-screen generator (dithered BMP)
4. Local library for books that are not on the reader yet

## License

MIT

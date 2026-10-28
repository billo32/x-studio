#!/usr/bin/env node
// Packs a plugin folder into an installable ZIP: <out>/<id>-<version>.zip
//
//   node scripts/build-plugin.mjs path/to/my-plugin [out-dir]   (default out-dir: release/plugins)
//
// Checks the basics the app checks on install (manifest fields, entry and icon present).
// Tests (*.test.js) and dotfiles are left out.
import { readFileSync, readdirSync, statSync, mkdirSync, writeFileSync } from "node:fs";
import { join, relative, sep } from "node:path";
import { deflateRawSync } from "node:zlib";

const [dir, out = "release/plugins"] = process.argv.slice(2);
if (!dir) {
  console.error("usage: node scripts/build-plugin.mjs <plugin-folder> [out-dir]");
  process.exit(2);
}

const manifest = JSON.parse(readFileSync(join(dir, "manifest.json"), "utf8"));
for (const k of ["id", "name", "version", "apiVersion"]) {
  if (manifest[k] == null) throw new Error(`manifest.json: "${k}" is missing`);
}
if (!/^[a-z0-9][a-z0-9.-]{1,62}[a-z0-9]$/.test(manifest.id)) throw new Error(`manifest.json: bad id "${manifest.id}"`);

function walk(d) {
  return readdirSync(d).flatMap((name) => {
    if (name.startsWith(".")) return [];
    const p = join(d, name);
    if (statSync(p).isDirectory()) return walk(p);
    return name.endsWith(".test.js") ? [] : [p];
  });
}
const files = walk(dir)
  .map((p) => ({ path: relative(dir, p).split(sep).join("/"), data: readFileSync(p) }))
  .sort((a, b) => a.path.localeCompare(b.path));
for (const needed of [manifest.entry ?? "index.html", manifest.icon].filter(Boolean)) {
  if (!files.some((f) => f.path === needed)) throw new Error(`${needed} is missing from ${dir}`);
}

// ---- a minimal ZIP writer (deflate, no extras) ----
const CRC = new Int32Array(256).map((_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c;
});
const crc32 = (buf) => {
  let c = -1;
  for (const b of buf) c = CRC[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ -1) >>> 0;
};
// A fixed timestamp keeps builds reproducible (1 Jan 2026, 00:00).
const DOS_TIME = 0;
const DOS_DATE = ((2026 - 1980) << 9) | (1 << 5) | 1;

const chunks = [];
const central = [];
let offset = 0;
for (const f of files) {
  const name = Buffer.from(f.path, "utf8");
  const packed = deflateRawSync(f.data, { level: 9 });
  const crc = crc32(f.data);
  const local = Buffer.alloc(30);
  local.writeUInt32LE(0x04034b50, 0);
  local.writeUInt16LE(20, 4); // version needed
  local.writeUInt16LE(0x0800, 6); // UTF-8 names
  local.writeUInt16LE(8, 8); // deflate
  local.writeUInt16LE(DOS_TIME, 10);
  local.writeUInt16LE(DOS_DATE, 12);
  local.writeUInt32LE(crc, 14);
  local.writeUInt32LE(packed.length, 18);
  local.writeUInt32LE(f.data.length, 22);
  local.writeUInt16LE(name.length, 26);
  chunks.push(local, name, packed);

  const c = Buffer.alloc(46);
  c.writeUInt32LE(0x02014b50, 0);
  c.writeUInt16LE(20, 4);
  c.writeUInt16LE(20, 6);
  c.writeUInt16LE(0x0800, 8);
  c.writeUInt16LE(8, 10);
  c.writeUInt16LE(DOS_TIME, 12);
  c.writeUInt16LE(DOS_DATE, 14);
  c.writeUInt32LE(crc, 16);
  c.writeUInt32LE(packed.length, 20);
  c.writeUInt32LE(f.data.length, 24);
  c.writeUInt16LE(name.length, 28);
  c.writeUInt32LE((0o100644 << 16) >>> 0, 38); // regular file, rw-r--r--
  c.writeUInt32LE(offset, 42);
  central.push(c, name);
  offset += local.length + name.length + packed.length;
}
const centralBuf = Buffer.concat(central);
const end = Buffer.alloc(22);
end.writeUInt32LE(0x06054b50, 0);
end.writeUInt16LE(files.length, 8);
end.writeUInt16LE(files.length, 10);
end.writeUInt32LE(centralBuf.length, 12);
end.writeUInt32LE(offset, 16);

mkdirSync(out, { recursive: true });
const target = join(out, `${manifest.id}-${manifest.version}.zip`);
writeFileSync(target, Buffer.concat([...chunks, centralBuf, end]));
console.log(`${target} (${files.length} files)`);

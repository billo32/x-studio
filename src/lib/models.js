// Reader models, SD-card sections, and where uploads go.

/** Screen sizes are portrait (short × long edge). */
export const MODELS = [
  { id: "x4", name: "X4", width: 480, height: 800, connector: "USB-C" },
  { id: "x4-classic", name: "X4 Classic", width: 480, height: 800, connector: "POGO" },
  { id: "x4-pro", name: "X4 Pro", width: 480, height: 800, connector: "USB-C" },
  { id: "x3", name: "X3", width: 528, height: 792, connector: "POGO" },
  { id: "sticky", name: "Seeed Sticky", width: 480, height: 800, connector: "USB-C", unsure: true },
];

export const modelById = (id) => MODELS.find((m) => m.id === id) ?? MODELS[0];

/**
 * Best guess from what the firmware reports: USB status gives the build type
 * ("x3-x4", "x4-classic", "x4-pro", "sticky"), Wi‑Fi gives "X3"/"X4".
 * A manual choice (remembered per address) wins.
 */
export function detectModel(status) {
  try {
    const saved = localStorage.getItem(`model:${status.address}`);
    if (saved) return saved;
  } catch {}
  const d = (status.device || "").toLowerCase();
  if (d === "x4-classic" || d === "x4-pro" || d === "sticky" || d === "x3") return d;
  return "x4";
}

export function rememberModel(address, id) {
  try {
    localStorage.setItem(`model:${address}`, id);
  } catch {}
}

const BOOK = ["epub", "fb2", "txt", "xtc", "xtch", "md"];
const IMAGE = ["bmp", "png", "jpg", "jpeg", "webp"];
const FONT = ["ttf", "otf", "cpfont"];
const DICT = ["dict", "idx", "ifo", "syn", "oft", "cspt", "qidx", "dz"];

function ext(name) {
  const lower = name.toLowerCase();
  if (lower.endsWith(".fb2.zip")) return "fb2";
  return lower.includes(".") ? lower.split(".").pop() : "";
}

export function isImage(name) {
  return IMAGE.includes(ext(name));
}

function under(path, ...roots) {
  const p = path.toLowerCase();
  return roots.some((r) => p === r || p.startsWith(r + "/"));
}

/** books | images | fonts | dicts | other */
export function sectionOf(entry) {
  const e = ext(entry.name);
  // The firmware's own highlights file, not a book.
  if (entry.name.toLowerCase() === "my clippings.txt") return "other";
  if (under(entry.path, "/dictionaries", "/.dictionaries") || DICT.includes(e)) return "dicts";
  if (under(entry.path, "/fonts", "/.fonts") || FONT.includes(e)) return "fonts";
  if (BOOK.includes(e)) return "books";
  // Only pictures the firmware shows on its own; covers saved next to books are not screens.
  if (IMAGE.includes(e) && imageRole(entry.path) !== "Image") return "images";
  return "other";
}

export const SECTIONS = [
  { id: "books", label: "Books" },
  { id: "images", label: "Sleep & boot screens" },
  { id: "fonts", label: "Fonts" },
  { id: "dicts", label: "Dictionaries" },
  { id: "other", label: "Other" },
];

/** Image role by folder: sleep screen, boot screen, or just a picture. */
export function imageRole(path) {
  const p = path.toLowerCase();
  if (under(p, "/sleep", "/.sleep") || p === "/sleep.bmp" || p === "/sleep.png") return "Sleep screen";
  if (under(p, "/bootscreen", "/.bootscreen")) return "Boot screen";
  return "Image";
}

/** Where a dropped local file should go on the card. */
export function destinationFor(localPath, entries) {
  const name = localPath.split("/").pop();
  if (isImage(name)) return "/sleep";
  const s = sectionOf({ name, path: "/" + name });
  const hasDir = (d) => entries.some((e) => e.isDir && e.path.toLowerCase() === d);
  if (s === "books") return hasDir("/books") ? "/Books" : "/";
  if (s === "fonts") return fontsRoot(entries);
  if (s === "dicts") return "/dictionaries";
  return "/";
}

/** The card's visible fonts folder ("/Fonts" if that's how it is spelled), else "/fonts". */
export function fontsRoot(entries) {
  const dir = entries.find((e) => e.isDir && e.path.toLowerCase() === "/fonts");
  return dir ? dir.path : "/fonts";
}

/**
 * Font families on the card: folders right under /fonts or /.fonts.
 * → Map(lowercase name → { name, path, sizes: [pt…] })
 */
export function installedFonts(entries) {
  const out = new Map();
  for (const e of entries) {
    const parts = e.path.split("/").filter(Boolean);
    if (parts.length < 2 || !["fonts", ".fonts"].includes(parts[0].toLowerCase())) continue;
    const key = parts[1].toLowerCase();
    const fam = out.get(key) ?? { name: parts[1], path: `/${parts[0]}/${parts[1]}`, sizes: [] };
    const m = parts.length === 3 && !e.isDir && /_(\d+)\.cpfont$/i.exec(parts[2]);
    if (m) fam.sizes.push(Number(m[1]));
    if (parts.length === 2 && !e.isDir) continue; // a loose file, not a family
    out.set(key, fam);
  }
  for (const f of out.values()) f.sizes.sort((a, b) => a - b);
  return out;
}

/** The firmware's "Download Font Size Range" choices (Settings → Reader → Font Options). */
export const FONT_SIZE_RANGES = [
  { id: "small", label: "8–12 pt", sizes: [8, 9, 10, 12] },
  { id: "medium", label: "10–16 pt", sizes: [10, 12, 14, 16] },
  { id: "large", label: "14–20 pt", sizes: [14, 16, 18, 20] },
  { id: "all", label: "All sizes", sizes: [8, 9, 10, 12, 14, 16, 18, 20] },
];

export function bookFormat(name) {
  const e = ext(name);
  return e === "fb2" ? "FB2" : e.toUpperCase();
}

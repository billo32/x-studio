// How a cover image lands on a reader screen — the same maths as the Rust bake
// (crates/x4-books/src/imaging.rs), so the preview matches what the reader shows.

/**
 * Placement of an image of `imgW`×`imgH` on a `W`×`H` box.
 * fit: "cover" (fill, crop overflow) | "contain" (whole image, bars) | "stretch" (fill, distort)
 * @returns {{ w: number, h: number, left: number, top: number }}
 */
export function placeImage(imgW, imgH, W, H, fit) {
  const ia = imgW / imgH;
  const sa = W / H;
  let w, h;
  if (fit === "stretch") [w, h] = [W, H];
  else if ((fit === "cover") === (sa < ia)) [w, h] = [H * ia, H];
  else [w, h] = [W, W / ia];
  return { w, h, left: (W - w) / 2, top: (H - h) / 2 };
}

/**
 * Human description of the result in device pixels.
 * `W`×`H` is the preview box, `screenH` the real screen height (to convert to device px).
 */
export function describeFit(imgW, imgH, W, H, screenH, fit) {
  const g = placeImage(imgW, imgH, W, H, fit);
  const scale = screenH / H;
  const px = (v) => Math.round(Math.max(0, v) * scale);
  if (fit === "cover") {
    const cx = px((g.w - W) / 2), cy = px((g.h - H) / 2);
    if (cx) return `Fills the screen; ${cx} px are cropped on the left and right.`;
    if (cy) return `Fills the screen; ${cy} px are cropped at the top and bottom.`;
    return "Same proportions as the screen — fits as is.";
  }
  if (fit === "contain") {
    const bx = px((W - g.w) / 2), by = px((H - g.h) / 2);
    if (by) return `Whole cover, with ${by} px bars at the top and bottom.`;
    if (bx) return `Whole cover, with ${bx} px bars on the sides.`;
    return "Same proportions as the screen — no bars.";
  }
  const k = Math.round((W / H / (imgW / imgH) - 1) * 100);
  if (!k) return "Same proportions as the screen — no distortion.";
  return `Stretched to the screen: ${k < 0 ? `${-k}% narrower` : `${k}% wider`} than the original.`;
}

// ---- cover layout (mirrors crates/x4-books/src/cover.rs) ----

export const FULL = { x: 0, y: 0, w: 1, h: 1 };

const isFull = (r) => !r || (r.x <= 0.001 && r.y <= 0.001 && r.w >= 0.999 && r.h >= 0.999);

/** Size in source pixels of the part of `img` inside `crop` (fractions). */
export function cropSize(img, crop) {
  const r = crop ?? FULL;
  return [img.width * r.w, img.height * r.h];
}

/**
 * Placement with zoom (1 = as `fit` places it) and shift (fractions of the screen).
 * @returns {{ w: number, h: number, left: number, top: number }}
 */
export function placeLayout(cw, ch, W, H, { fit, zoom = 1, dx = 0, dy = 0 }) {
  const g = placeImage(cw, ch, W, H, fit);
  const w = g.w * zoom;
  const h = g.h * zoom;
  return { w, h, left: (W - w) / 2 + dx * W, top: (H - h) / 2 + dy * H };
}

/** Uncovered strips of the screen around a placement, in the same units. */
export function gaps(g, W, H) {
  const out = {
    top: Math.max(0, g.top),
    bottom: Math.max(0, H - (g.top + g.h)),
    left: Math.max(0, g.left),
    right: Math.max(0, W - (g.left + g.w)),
  };
  out.any = out.top > 0.5 || out.bottom > 0.5 || out.left > 0.5 || out.right > 0.5;
  return out;
}

/** Did the analysis find bars baked into the image? */
export function hasBars(analysis) {
  return !!analysis && !isFull(analysis.trim);
}

/**
 * A good starting layout for `img` (a Preview with `analysis`) on a W×H screen:
 * drop baked-in bars; if the proportions match, fill; if the cover has a plain background
 * where bars would be, fit it whole and extend that background; else fill unless that
 * crops too much, in which case fit it over a blurred copy.
 */
export function suggestLayout(img, W, H) {
  const a = img.analysis;
  const trim = hasBars(a);
  const [cw, ch] = cropSize(img, trim ? a.trim : FULL);
  const r = cw / ch / (W / H);
  const base = { trim, zoom: 1, dx: 0, dy: 0 };
  if (Math.abs(r - 1) < 0.04) return { ...base, fit: "cover", fill: "extend" };
  const sides = r > 1 ? ["top", "bottom"] : ["left", "right"];
  if (a && sides.every((s) => a[s].uniform)) return { ...base, fit: "contain", fill: "extend" };
  const loss = 1 - Math.min(r, 1 / r);
  return loss < 0.12 ? { ...base, fit: "cover", fill: "extend" } : { ...base, fit: "contain", fill: "blur" };
}

/** The `fill` sent to Rust for a UI choice: extend | blur | white | black. */
export function fillFor(kind, analysis) {
  if (kind === "blur") return { kind: "blur" };
  if (kind === "black") return { kind: "color", color: [0, 0, 0] };
  if (kind === "extend" && analysis) {
    const { top, bottom, left, right } = analysis;
    return { kind: "sides", top: top.color, bottom: bottom.color, left: left.color, right: right.color };
  }
  return { kind: "color", color: [255, 255, 255] };
}

/** CSS color of a [r, g, b] triple. */
export const rgb = (c) => `rgb(${c[0]} ${c[1]} ${c[2]})`;

/** One line about the result, in device pixels (`scale` = device px per preview px). */
export function describeLayout(g, W, H, scale, fit, fill) {
  const px = (v) => Math.round(v * scale);
  const gp = gaps(g, W, H);
  if (fit === "stretch" && !gp.any) return "Stretched to fill the screen.";
  const cropX = px(Math.max(0, -g.left) + Math.max(0, g.left + g.w - W));
  const cropY = px(Math.max(0, -g.top) + Math.max(0, g.top + g.h - H));
  const parts = [];
  if (cropX || cropY) parts.push(`${cropX ? `${cropX} px cropped across` : ""}${cropX && cropY ? ", " : ""}${cropY ? `${cropY} px cropped down` : ""}`);
  if (gp.any) {
    const how = { extend: "filled with the cover's own background", blur: "filled with a blurred copy", white: "white", black: "black" }[fill];
    parts.push(`edges ${how}`);
  }
  if (!parts.length) return "Fills the screen exactly.";
  const s = parts.join("; ");
  return s[0].toUpperCase() + s.slice(1) + ".";
}

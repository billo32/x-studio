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

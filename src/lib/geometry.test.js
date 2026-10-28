import { describe, expect, it } from "vitest";
import { describeFit, placeImage } from "./geometry.js";

// A 2:3 cover on the 3:5 X4 screen (480×800) and on the 2:3 X3 screen (528×792).
const X4 = [480, 800];
const X3 = [528, 792];

describe("placeImage", () => {
  it("cover fills the screen and overflows sideways for a wider image", () => {
    const g = placeImage(600, 900, ...X4, "cover");
    expect(g.h).toBe(800);
    expect(g.w).toBeCloseTo(533.33, 1);
    expect(g.left).toBeCloseTo(-26.67, 1);
    expect(g.top).toBe(0);
  });

  it("contain shows the whole image with bars", () => {
    const g = placeImage(600, 900, ...X4, "contain");
    expect(g.w).toBe(480);
    expect(g.h).toBe(720);
    expect(g.top).toBe(40);
  });

  it("stretch always matches the screen", () => {
    expect(placeImage(100, 1000, ...X4, "stretch")).toEqual({ w: 480, h: 800, left: 0, top: 0 });
  });

  it("an image with the screen's proportions fits exactly in every mode", () => {
    for (const fit of ["cover", "contain", "stretch"]) {
      const g = placeImage(1200, 1800, ...X3, fit);
      expect(g.w).toBeCloseTo(528, 6);
      expect(g.h).toBeCloseTo(792, 6);
    }
  });
});

describe("describeFit", () => {
  it("reports cropping in device pixels, even from a smaller preview", () => {
    // Preview is half size; numbers must still be in device px.
    expect(describeFit(600, 900, 240, 400, 800, "cover")).toBe("Fills the screen; 27 px are cropped on the left and right.");
  });

  it("reports bars for contain", () => {
    expect(describeFit(600, 900, 480, 800, 800, "contain")).toBe("Whole cover, with 40 px bars at the top and bottom.");
    expect(describeFit(1000, 500, 480, 800, 800, "contain")).toContain("bars at the top and bottom");
  });

  it("reports distortion for stretch", () => {
    expect(describeFit(600, 900, 480, 800, 800, "stretch")).toBe("Stretched to the screen: 10% narrower than the original.");
    expect(describeFit(528, 792, 528, 792, 792, "stretch")).toBe("Same proportions as the screen — no distortion.");
  });
});

import { describeLayout, fillFor, gaps, placeLayout, suggestLayout } from "./geometry.js";

const side = (color, uniform = true) => ({ color, uniform });
const analysis = (trim, uniform = true) => ({
  trim,
  top: side([250, 250, 250], uniform),
  bottom: side([240, 240, 240], uniform),
  left: side([250, 250, 250], uniform),
  right: side([250, 250, 250], uniform),
});

describe("cover layout", () => {
  it("zoom and shift work around the fitted placement", () => {
    expect(placeLayout(1, 1, 400, 800, { fit: "stretch", zoom: 0.5, dx: 0.25 })).toEqual({ w: 200, h: 400, left: 200, top: 200 });
    const g = placeLayout(600, 900, ...X4, { fit: "contain" });
    expect(gaps(g, ...X4)).toMatchObject({ top: 40, bottom: 40, any: true });
  });

  it("a letterboxed 3:5 cover loses its bars and gets its own background extended", () => {
    // 480×800 image whose middle 600 px is a 4:5 cover on white.
    const img = { width: 480, height: 800, analysis: analysis({ x: 0, y: 0.125, w: 1, h: 0.75 }) };
    expect(suggestLayout(img, ...X4)).toMatchObject({ trim: true, fit: "contain", fill: "extend" });
  });

  it("busy edges: fill when the crop is small, blur bars when it is not", () => {
    const close = { width: 500, height: 800, analysis: analysis({ x: 0, y: 0, w: 1, h: 1 }, false) };
    expect(suggestLayout(close, ...X4)).toMatchObject({ trim: false, fit: "cover" });
    const square = { width: 800, height: 800, analysis: analysis({ x: 0, y: 0, w: 1, h: 1 }, false) };
    expect(suggestLayout(square, ...X4)).toMatchObject({ fit: "contain", fill: "blur" });
    const exact = { width: 480, height: 800 };
    expect(suggestLayout(exact, ...X4)).toMatchObject({ fit: "cover", trim: false });
  });

  it("fills and descriptions", () => {
    const a = analysis({ x: 0, y: 0, w: 1, h: 1 });
    expect(fillFor("extend", a)).toEqual({ kind: "sides", top: [250, 250, 250], bottom: [240, 240, 240], left: [250, 250, 250], right: [250, 250, 250] });
    expect(fillFor("extend", null)).toEqual({ kind: "color", color: [255, 255, 255] });
    expect(fillFor("black")).toEqual({ kind: "color", color: [0, 0, 0] });
    const g = placeLayout(600, 900, ...X4, { fit: "contain" });
    expect(describeLayout(g, ...X4, 1, "contain", "extend")).toBe("Edges filled with the cover's own background.");
    const c = placeLayout(600, 900, ...X4, { fit: "cover" });
    expect(describeLayout(c, ...X4, 1, "cover", "extend")).toBe("53 px cropped across.");
  });
});

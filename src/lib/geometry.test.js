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

import { describe, expect, it } from "vitest";
import { bookFormat, destinationFor, detectModel, fontsRoot, imageRole, installedFonts, modelById, sectionOf } from "./models.js";

const file = (path) => ({ path, name: path.split("/").pop(), isDir: false, size: 1 });

describe("sectionOf", () => {
  it.each([
    ["/Books/Anna.epub", "books"],
    ["/Gogol.fb2.zip", "books"],
    ["/notes.txt", "books"],
    ["/sleep/waves.bmp", "images"],
    ["/cover.png", "other"],
    ["/Books/Greene/48 Laws.jpg", "other"],
    ["/sleep.bmp", "images"],
    ["/.bootscreen/logo.bmp", "images"],
    ["/fonts/Literata/Literata.ttf", "fonts"],
    ["/fonts/Literata/readme.txt", "fonts"],
    ["/dictionaries/Cambridge/Cambridge.idx", "dicts"],
    ["/dictionaries/Cambridge/about.txt", "dicts"],
    ["/My Clippings.txt", "other"],
    ["/firmware.bin", "other"],
  ])("%s → %s", (path, section) => {
    expect(sectionOf(file(path))).toBe(section);
  });
});

describe("destinationFor", () => {
  const withBooks = [{ path: "/Books", name: "Books", isDir: true, size: 0 }];
  it("sends books to /Books when the folder exists, else to the root", () => {
    expect(destinationFor("/Users/me/Moby.epub", withBooks)).toBe("/Books");
    expect(destinationFor("/Users/me/Moby.epub", [])).toBe("/");
  });
  it("routes images, fonts and dictionaries to their firmware folders", () => {
    expect(destinationFor("/x/sky.bmp", [])).toBe("/sleep");
    expect(destinationFor("/x/Inter.ttf", [])).toBe("/fonts");
    expect(destinationFor("/x/en.dict", [])).toBe("/dictionaries");
    expect(destinationFor("/x/readme.pdf", [])).toBe("/");
  });
});

describe("models", () => {
  it("detects the model from the firmware build type and falls back to X4", () => {
    expect(detectModel({ address: "a", device: "x4-classic" })).toBe("x4-classic");
    expect(detectModel({ address: "b", device: "X3" })).toBe("x3");
    expect(detectModel({ address: "c", device: "x3-x4" })).toBe("x4");
    expect(detectModel({ address: "d", device: "" })).toBe("x4");
  });
  it("knows each screen size", () => {
    expect(modelById("x3")).toMatchObject({ width: 528, height: 792 });
    expect(modelById("x4-pro")).toMatchObject({ width: 480, height: 800 });
    expect(modelById("nope").id).toBe("x4");
  });
  it("labels images and formats", () => {
    expect(imageRole("/.sleep/a.bmp")).toBe("Sleep screen");
    expect(imageRole("/bootscreen/logo.bmp")).toBe("Boot screen");
    expect(imageRole("/pic.png")).toBe("Image");
    expect(bookFormat("a.fb2.zip")).toBe("FB2");
    expect(bookFormat("a.epub")).toBe("EPUB");
  });
});

const dir = (path) => ({ path, name: path.split("/").pop(), isDir: true, size: 0 });

describe("fonts on the card", () => {
  const entries = [
    dir("/Fonts"),
    file("/Fonts/26-Bitter.xtf"),
    dir("/Fonts/Lora"),
    file("/Fonts/Lora/Lora_14.cpfont"),
    file("/Fonts/Lora/Lora_10.cpfont"),
    dir("/Fonts/Inter"),
    file("/Fonts/Inter/Inter-Regular.ttf"),
    file("/Books/a.epub"),
  ];

  it("uses the folder as spelled on the card", () => {
    expect(fontsRoot(entries)).toBe("/Fonts");
    expect(fontsRoot([])).toBe("/fonts");
  });

  it("finds families and their sizes", () => {
    const f = installedFonts(entries);
    expect([...f.keys()].sort()).toEqual(["inter", "lora"]);
    expect(f.get("lora")).toEqual({ name: "Lora", path: "/Fonts/Lora", sizes: [10, 14] });
    expect(f.get("inter").sizes).toEqual([]);
  });
});

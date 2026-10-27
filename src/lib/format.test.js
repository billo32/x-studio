import { describe, expect, it, vi } from "vitest";

// api.js imports the Tauri bridge; the helpers under test don't use it.
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const { baseName, count, formatDuration, formatSize, joinPath, parentPath, plural } = await import("./api.js");

describe("paths", () => {
  it("joins and splits device paths", () => {
    expect(joinPath("/", "a.epub")).toBe("/a.epub");
    expect(joinPath("/Books/", "a.epub")).toBe("/Books/a.epub");
    expect(parentPath("/Books/Sci/a.epub")).toBe("/Books/Sci");
    expect(parentPath("/a.epub")).toBe("/");
    expect(baseName("/Books/a.epub")).toBe("a.epub");
  });
});

describe("formatting", () => {
  it("formats sizes", () => {
    expect(formatSize(512)).toBe("512 B");
    expect(formatSize(1536)).toBe("1.5 KB");
    expect(formatSize(1258291)).toBe("1.2 MB");
    expect(formatSize(30 * 1024 * 1024)).toBe("30 MB");
  });
  it("formats durations and counts", () => {
    expect(formatDuration(45)).toBe("≈ 45 min");
    expect(formatDuration(1036)).toBe("≈ 17 h");
    expect(plural(1, "book", "books")).toBe("book");
    expect(count(1, "book")).toBe("1 book");
    expect(count(4662, "page")).toBe("4,662 pages");
  });
});

import { describe, expect, it } from "vitest";
import { colorIndex, displayFolder, relativeTime, shortHash } from "./format";

describe("shortHash", () => {
  it("returns the first 8 characters of a hash", () => {
    expect(shortHash("471663edabcdef")).toBe("471663ed");
  });
});

describe("relativeTime", () => {
  const now = Date.UTC(2026, 10, 13, 12, 0, 0);
  const ago = (ms: number) => relativeTime(now - ms, now);
  const MIN = 60_000;
  const HOUR = 60 * MIN;
  const DAY = 24 * HOUR;

  it("speaks in the largest whole unit", () => {
    expect(ago(10_000)).toBe("just now");
    expect(ago(-5_000)).toBe("just now");
    expect(ago(MIN)).toBe("1 min ago");
    expect(ago(45 * MIN)).toBe("45 min ago");
    expect(ago(2 * HOUR)).toBe("2 h ago");
    expect(ago(DAY)).toBe("yesterday");
    expect(ago(3 * DAY)).toBe("3 days ago");
    expect(ago(14 * DAY)).toBe("2 weeks ago");
    expect(ago(60 * DAY)).toBe("2 months ago");
    expect(ago(800 * DAY)).toBe("2 years ago");
  });
});

describe("displayFolder", () => {
  it("shows the parent folder with the home folder as ~", () => {
    expect(displayFolder("/Users/ada/Dev/gg", "/Users/ada")).toBe("~/Dev");
    expect(displayFolder("/home/ada/src/x", "/home/ada")).toBe("~/src");
    expect(displayFolder("/Users/ada/x", "/Users/ada")).toBe("~");
    expect(displayFolder("/Users/ada/Dev/x")).toBe("/Users/ada/Dev");
    expect(displayFolder("/srv/repos/x", "/Users/ada")).toBe("/srv/repos");
    expect(displayFolder("/x")).toBe("/");
    expect(displayFolder("C:\\Users\\ada\\code\\x", "C:\\Users\\ada")).toBe("~\\code");
    expect(displayFolder("D:\\work\\x")).toBe("D:\\work");
  });
});

describe("colorIndex", () => {
  it("gives each key a stable lane colour index in 0..9", () => {
    const keys = ["/a", "/b", "/Users/ada/Dev/gg", "", "ünïcode path"];
    for (const k of keys) {
      const i = colorIndex(k);
      expect(i).toBeGreaterThanOrEqual(0);
      expect(i).toBeLessThan(10);
      expect(Number.isInteger(i)).toBe(true);
      expect(colorIndex(k)).toBe(i);
    }
    const spread = new Set(Array.from({ length: 50 }, (_, n) => colorIndex(`/repo/${n}`)));
    expect(spread.size).toBeGreaterThan(5);
  });
});

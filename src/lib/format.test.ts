// Runs with TZ=UTC (vite.config.ts `test.env`), so local-time formatting is deterministic.
import { describe, expect, it } from "vitest";
import { formatDate, formatDateLong, groupDigits, shortenPath, visibleText } from "./format";

describe("formatDate", () => {
  it("formats unix seconds as day, short month, year and 24-hour time", () => {
    expect(formatDate(1794477720)).toBe("12 Nov 2026 10:02");
  });

  it("pads the time and keeps single-digit days unpadded", () => {
    // 2026-01-05 09:00:00 UTC
    expect(formatDate(Date.UTC(2026, 0, 5, 9, 0, 0) / 1000)).toBe("5 Jan 2026 09:00");
    // 2025-12-31 23:59:59 UTC
    expect(formatDate(Date.UTC(2025, 11, 31, 23, 59, 59) / 1000)).toBe("31 Dec 2025 23:59");
  });
});

describe("formatDateLong", () => {
  it("adds the weekday, seconds and the zone offset", () => {
    expect(formatDateLong(1794477720)).toBe("Thursday, 12 Nov 2026 10:02:00 (UTC+00:00)");
  });
});

describe("shortenPath", () => {
  it("shows the home folder as ~", () => {
    expect(shortenPath("/Users/a/Dev/x", "/Users/a")).toBe("~/Dev/x");
    expect(shortenPath("/Users/a", "/Users/a")).toBe("~");
    expect(shortenPath("/Users/a/", "/Users/a/")).toBe("~/");
  });

  it("only replaces a whole leading folder", () => {
    expect(shortenPath("/Users/ab/x", "/Users/a")).toBe("/Users/ab/x");
    expect(shortenPath("/srv/Users/a/x", "/Users/a")).toBe("/srv/Users/a/x");
  });

  it("leaves the path alone without a home folder", () => {
    expect(shortenPath("/Users/a/Dev/x")).toBe("/Users/a/Dev/x");
    expect(shortenPath("/Users/a/Dev/x", "")).toBe("/Users/a/Dev/x");
  });

  it("handles Windows paths", () => {
    expect(shortenPath("C:\\Users\\ada\\code\\x", "C:\\Users\\ada")).toBe("~\\code\\x");
    expect(shortenPath("c:\\users\\ada\\code", "C:\\Users\\ada")).toBe("~\\code");
  });
});

describe("visibleText", () => {
  it("shows control characters as their pictures", () => {
    expect(visibleText("odd\nname\t.txt")).toBe("odd\u240Aname\u2409.txt");
    expect(visibleText("a\u0000b\u001Fc\u007Fd\re")).toBe("a\u2400b\u241Fc\u2421d\u240De");
  });

  it("shows bidirectional overrides and isolates by name, so they cannot reorder the text", () => {
    expect(visibleText("evil\u202Etxt.exe")).toBe("evil\u2039U+202E\u203Atxt.exe");
    expect(visibleText("a\u2066b\u2069")).toBe("a\u2039U+2066\u203Ab\u2039U+2069\u203A");
  });

  it("leaves ordinary text, other scripts included, as it is", () => {
    for (const s of ["src/app.ts", "dir with space/\u00FCn\u00EF \u2728.txt", "\u05E9\u05DC\u05D5\u05DD.md", ""]) {
      expect(visibleText(s)).toBe(s);
    }
  });
});

describe("groupDigits", () => {
  it("groups thousands with commas, as en-US does", () => {
    for (const n of [0, 7, 999, 1000, 10_000, 254_301, 1_234_567, 1_000_000_000]) {
      expect(groupDigits(n)).toBe(n.toLocaleString("en-US"));
    }
  });
});

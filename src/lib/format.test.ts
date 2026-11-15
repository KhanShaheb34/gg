// Runs with TZ=UTC (vite.config.ts `test.env`), so local-time formatting is deterministic.
import { describe, expect, it } from "vitest";
import {
  clockTime,
  commandLine,
  formatClock,
  formatDate,
  formatDateLong,
  formatDuration,
  groupDigits,
  outputLines,
  repeatsMergeName,
  shellQuote,
  shortenPath,
  visibleText,
} from "./format";

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

// The date format setting (Settings › Appearance): the graph's date column, the details,
// the command log's times, and tooltips. "standard" is what the app always showed.
describe("date formats", () => {
  const when = 1794477720; // Thursday 12 Nov 2026 10:02:00 UTC
  const now = (when + 3 * 86_400 + 600) * 1000; // three days and ten minutes later

  it("standard: day, short month, year and 24-hour time, as before (the default)", () => {
    expect(formatDate(when)).toBe("12 Nov 2026 10:02");
    expect(formatDate(when, "standard", now)).toBe("12 Nov 2026 10:02");
    expect(formatDateLong(when, "standard")).toBe("Thursday, 12 Nov 2026 10:02:00 (UTC+00:00)");
    expect(formatClock(when * 1000 + 7000, "standard")).toBe("10:02:07");
  });

  it("relative: how long ago, and the date itself where there is room (details, tooltips)", () => {
    expect(formatDate(when, "relative", now)).toBe("3 days ago");
    expect(formatDateLong(when, "relative", now)).toBe("3 days ago · Thursday, 12 Nov 2026 10:02:00 (UTC+00:00)");
    expect(formatClock(now - 120_000, "relative", now)).toBe("2 min ago");
    expect(formatClock(now - 5_000, "relative", now)).toBe("just now");
  });

  it("iso: ISO 8601, sortable, with the offset in full", () => {
    expect(formatDate(when, "iso", now)).toBe("2026-11-12 10:02");
    expect(formatDateLong(when, "iso", now)).toBe("2026-11-12T10:02:00+00:00");
    expect(formatClock(when * 1000 + 7000, "iso")).toBe("10:02:07");
    // Padded months, days and hours.
    expect(formatDate(Date.UTC(2026, 0, 5, 9, 0, 0) / 1000, "iso")).toBe("2026-01-05 09:00");
  });

  it("locale: the system's own format for the user's language and region", () => {
    const at = new Date(when * 1000);
    expect(formatDate(when, "locale", now)).toBe(
      new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(at),
    );
    expect(formatDateLong(when, "locale", now)).toBe(
      new Intl.DateTimeFormat(undefined, { dateStyle: "full", timeStyle: "long" }).format(at),
    );
    expect(formatClock(when * 1000, "locale")).toBe(
      new Intl.DateTimeFormat(undefined, { timeStyle: "medium" }).format(at),
    );
    expect(formatDate(when, "locale")).not.toBe("");
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

  it("shows every character that is invisible or reorders text by its code point", () => {
    const hidden = [
      // C1 controls
      [0x80, 0x85, 0x9f],
      // soft hyphen, Arabic letter mark
      [0xad, 0x61c],
      // zero-width space, joiners, LRM, RLM
      [0x200b, 0x200c, 0x200d, 0x200e, 0x200f],
      // line and paragraph separators, bidi embeddings and overrides
      [0x2028, 0x2029, 0x202a, 0x202e],
      // word joiner, invisible operators, isolates
      [0x2060, 0x2061, 0x2064, 0x2065, 0x2066, 0x2069],
      // byte order mark / zero-width no-break space
      [0xfeff],
    ].flat();
    for (const code of hidden) {
      const hex = code.toString(16).toUpperCase().padStart(4, "0");
      expect(visibleText(`a${String.fromCharCode(code)}b`), `U+${hex}`).toBe(`a\u2039U+${hex}\u203Ab`);
    }
  });

  it("tells apart two paths that differ only by a zero-width character", () => {
    const plain = "src/main.rs";
    const sneaky = "src/ma\u200Bin.rs";
    // Different once shown, even to someone who cannot see zero-width characters.
    const strip = (t: string) => t.replace(/[\u200B-\u200F\u2060\uFEFF]/g, "");
    expect(strip(visibleText(sneaky))).not.toBe(strip(visibleText(plain)));
    expect(visibleText(sneaky)).toBe("src/ma\u2039U+200B\u203Ain.rs");
    expect(visibleText(plain)).toBe(plain);
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

  it("groups huge and negative values without an exponent", () => {
    expect(groupDigits(-1234)).toBe("-1,234");
    expect(groupDigits(1e21)).toBe("1,000,000,000,000,000,000,000");
    expect(groupDigits(Number.MAX_SAFE_INTEGER)).toBe("9,007,199,254,740,991");
    expect(groupDigits(Infinity)).toBe("Infinity");
    expect(groupDigits(NaN)).toBe("NaN");
  });
});

describe("command log text", () => {
  it("quotes arguments for display as a shell would need them", () => {
    expect(shellQuote("status")).toBe("status");
    expect(shellQuote("--format=%H%x00")).toBe("--format=%H%x00");
    expect(shellQuote("refs/heads/feature/x")).toBe("refs/heads/feature/x");
    expect(shellQuote("a b.txt")).toBe("'a b.txt'");
    expect(shellQuote("")).toBe("''");
    expect(shellQuote("it's")).toBe("'it'\\''s'");
    expect(shellQuote("$HOME")).toBe("'$HOME'");
    // zsh expands a word starting with = (=git is git's path) and ~ (a home folder).
    expect(shellQuote("=git")).toBe("'=git'");
    expect(shellQuote("~ada")).toBe("'~ada'");
    expect(shellQuote("--a=b")).toBe("--a=b");
    expect(shellQuote("é.txt")).toBe("'é.txt'");
    expect(shellQuote("a*b")).toBe("'a*b'");
    expect(commandLine(["commit", "-m", "Fix the thing"])).toBe("git commit -m 'Fix the thing'");
    // Another tool the core ran shows as itself.
    expect(commandLine(["pr", "list", "--json", "a,b"], "gh")).toBe("gh pr list --json a,b");
  });

  it("shows progress redrawn with carriage returns as its last state, per line", () => {
    expect(outputLines("Counting: 10%\rCounting: 50%\rCounting: 100%, done.\nTotal 3\n")).toEqual([
      "Counting: 100%, done.",
      "Total 3",
    ]);
    expect(outputLines("a\r\nb\r\n")).toEqual(["a", "b"]);
    expect(outputLines("")).toEqual([]);
    expect(outputLines("no end")).toEqual(["no end"]);
    // Control characters show; tabs stay tabs; a right-to-left override is made visible.
    expect(outputLines("a\tb\u0007c ‮evil")).toEqual(["a\tb␇c ‹U+202E›evil"]);
  });

  it("says how long a command took", () => {
    expect(formatDuration(0)).toBe("0 ms");
    expect(formatDuration(999)).toBe("999 ms");
    expect(formatDuration(1000)).toBe("1.0 s");
    expect(formatDuration(12_345)).toBe("12.3 s");
    expect(formatDuration(64_000)).toBe("1 min 4 s");
  });

  it("gives the time of day, to the second", () => {
    expect(clockTime(Date.UTC(2026, 10, 14, 9, 5, 7))).toBe("09:05:07");
  });
});

describe("repeatsMergeName", () => {
  it("is true only for git's own merge subjects naming what comes in", () => {
    expect(repeatsMergeName("Merge branch 'feature/x'", "feature/x")).toBe(true);
    expect(repeatsMergeName("Merge branch 'feature/x' into main", "feature/x")).toBe(true);
    expect(repeatsMergeName("Merge remote-tracking branch 'origin/main'", "origin/main")).toBe(true);
    expect(repeatsMergeName("Merge remote-tracking branch 'origin/main' into staging", "origin/main")).toBe(true);
    expect(repeatsMergeName("Merge tag 'v1.2'", "v1.2")).toBe(true);
    // Someone's own words that happen to hold a short name are kept.
    expect(repeatsMergeName("Bring the ui fixes in", "ui")).toBe(false);
    expect(repeatsMergeName("Merge branch 'ui-2'", "ui")).toBe(false);
    expect(repeatsMergeName("Merge branch 'x' into main, fixing the build", "x")).toBe(false);
  });
});

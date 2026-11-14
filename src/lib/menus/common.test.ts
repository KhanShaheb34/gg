import { describe, expect, it } from "vitest";
import { defaultRemote, shown } from "./common";

const loneSurrogate = /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/;
const codePoints = (s: string) => [...s].length;

describe("shown", () => {
  it("keeps a short name as it is, with invisible characters shown", () => {
    expect(shown("feature/x")).toBe("feature/x");
    expect(shown("a‮b")).toBe("a‹U+202E›b");
  });

  it("shortens a long name in the middle, to 44 characters", () => {
    const out = shown(`${"a".repeat(30)}${"b".repeat(30)}`);
    expect(codePoints(out)).toBe(44);
    expect(out).toMatch(/^a+…b+$/);
  });

  it("never cuts a character in two", () => {
    for (let lead = 15; lead < 30; lead++) {
      const out = shown(`${"a".repeat(lead)}${"🚀".repeat(40)}`);
      expect(out, `lead ${lead}`).not.toMatch(loneSurrogate);
      expect(codePoints(out)).toBeLessThanOrEqual(44);
    }
  });

  it("never cuts a shown invisible character's marker", () => {
    for (let lead = 10; lead < 30; lead++) {
      const out = shown(`${"a".repeat(lead)}${"‮".repeat(10)}${"b".repeat(lead)}`);
      // Every marker that starts also ends.
      expect(out.split("‹").length, `lead ${lead}: ${out}`).toBe(out.split("›").length);
      expect(out, `lead ${lead}: ${out}`).not.toMatch(/‹[^›]*$|^[^‹]*›/);
    }
  });

  it("keeps a grapheme whole (a flag, an accent)", () => {
    const flag = "🇧🇩";
    const out = shown(`${"a".repeat(21)}${flag.repeat(30)}`);
    expect(out.replace(/^a+…?/, "").replace(/…/, "").length % 4).toBe(0);
  });
});

describe("defaultRemote", () => {
  it("is the preferred one, else origin, else the first", () => {
    expect(defaultRemote(["backup", "origin"], "backup")).toBe("backup");
    expect(defaultRemote(["backup", "origin"], "gone")).toBe("origin");
    expect(defaultRemote(["backup", "upstream"])).toBe("backup");
    expect(defaultRemote([])).toBe("");
  });
});

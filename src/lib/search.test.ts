import { describe, expect, it } from "vitest";
import { hitPosition, matchParts, searchPattern, stepHit } from "./search";

describe("searchPattern and matchParts", () => {
  it("splits text into matched and unmatched parts, ignoring case", () => {
    const p = searchPattern("fix");
    expect(matchParts("Fix the fixture", p)).toEqual([
      { text: "Fix", match: true },
      { text: " the ", match: false },
      { text: "fix", match: true },
      { text: "ture", match: false },
    ]);
  });

  it("trims the query and treats it literally", () => {
    const p = searchPattern("  a.b(  ");
    expect(matchParts("xa.b(y aXb(", p)).toEqual([
      { text: "x", match: false },
      { text: "a.b(", match: true },
      { text: "y aXb(", match: false },
    ]);
  });

  it("has no pattern for a blank query, and then one unmatched part", () => {
    expect(searchPattern("   ")).toBeNull();
    expect(matchParts("anything", null)).toEqual([{ text: "anything", match: false }]);
    expect(matchParts("no match here", searchPattern("zzz"))).toEqual([{ text: "no match here", match: false }]);
  });

  it("matches non-ASCII text case-insensitively", () => {
    expect(matchParts("Größe ÄNDERN", searchPattern("änd"))).toEqual([
      { text: "Größe ", match: false },
      { text: "ÄND", match: true },
      { text: "ERN", match: false },
    ]);
  });
});

describe("hitPosition", () => {
  const hits = [2, 5, 9, 40];
  it("finds a row among the ascending hits", () => {
    expect(hitPosition(hits, 2)).toBe(0);
    expect(hitPosition(hits, 9)).toBe(2);
    expect(hitPosition(hits, 40)).toBe(3);
  });
  it("is -1 for a row that is not a hit, or no row", () => {
    expect(hitPosition(hits, 3)).toBe(-1);
    expect(hitPosition(hits, null)).toBe(-1);
    expect(hitPosition([], 0)).toBe(-1);
  });
});

describe("stepHit", () => {
  const hits = [2, 5, 9, 40];
  it("moves to the next hit after the selected row, wrapping at the end", () => {
    expect(stepHit(hits, 2, 1)).toBe(5);
    expect(stepHit(hits, 6, 1)).toBe(9);
    expect(stepHit(hits, 40, 1)).toBe(2);
    expect(stepHit(hits, 100, 1)).toBe(2);
  });
  it("moves to the previous hit before the selected row, wrapping at the start", () => {
    expect(stepHit(hits, 9, -1)).toBe(5);
    expect(stepHit(hits, 6, -1)).toBe(5);
    expect(stepHit(hits, 2, -1)).toBe(40);
    expect(stepHit(hits, 0, -1)).toBe(40);
  });
  it("starts from the first (or last) hit without a selection, and has none without hits", () => {
    expect(stepHit(hits, null, 1)).toBe(2);
    expect(stepHit(hits, null, -1)).toBe(40);
    expect(stepHit([], 3, 1)).toBeNull();
  });
});

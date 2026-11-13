import { describe, expect, it } from "vitest";
import { countLabel, hashMatch, hitPosition, matchParts, needleOf, searchPattern, splitMatch, stepHit } from "./search";

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

describe("needleOf and splitMatch", () => {
  it("lowercases and trims the query; blank is null", () => {
    expect(needleOf("  Fix ")).toBe("fix");
    expect(needleOf("   ")).toBeNull();
  });
  it("splits around the first match, ignoring case", () => {
    expect(splitMatch("Fix the fixture", "fix")).toEqual(["", "Fix", " the fixture"]);
    expect(splitMatch("Add part 19", "part")).toEqual(["Add ", "part", " 19"]);
    expect(splitMatch("Größe ÄNDERN", "änd")).toEqual(["Größe ", "ÄND", "ERN"]);
  });
  it("leaves text without a match whole", () => {
    expect(splitMatch("Nothing here", "zzz")).toEqual(["Nothing here", "", ""]);
  });
  it("lowercases character by character, as the core does, even when that changes lengths", () => {
    expect(splitMatch("İstanbul trip", "trip")).toEqual(["İstanbul ", "trip", ""]);
    expect(splitMatch("Ünïcode 😀 Fix", "fix")).toEqual(["Ünïcode 😀 ", "Fix", ""]);
    // Σ lowercases to σ one character at a time (no final-sigma rule), like the core.
    expect(splitMatch("ΟΔΟΣ", "οδοσ")).toEqual(["", "ΟΔΟΣ", ""]);
  });
});

describe("hashMatch", () => {
  const hash = "0fa322a9c0ffee00000000000000000000000000";
  it("is the length of a hex prefix of 4 or more digits the hash starts with, at most the short hash", () => {
    expect(hashMatch(hash, "0fa3")).toBe(4);
    expect(hashMatch(hash, "0fa322a9c0")).toBe(8);
  });
  it("is 0 for short, non-hex or non-matching queries", () => {
    expect(hashMatch(hash, "0fa")).toBe(0);
    expect(hashMatch(hash, "fa32")).toBe(0);
    expect(hashMatch(hash, "0fa3 x")).toBe(0);
    expect(hashMatch(hash, null)).toBe(0);
  });
});

describe("countLabel", () => {
  it("says which hit is selected, how many there are, or that there are none", () => {
    expect(countLabel(2, 41)).toBe("3 of 41");
    expect(countLabel(-1, 41)).toBe("41 results");
    expect(countLabel(-1, 1)).toBe("1 result");
    expect(countLabel(-1, 0)).toBe("No results");
  });
  it("groups thousands", () => {
    expect(countLabel(0, 97169)).toBe("1 of 97,169");
    expect(countLabel(-1, 19440)).toBe("19,440 results");
  });
});

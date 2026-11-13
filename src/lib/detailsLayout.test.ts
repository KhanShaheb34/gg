import { describe, expect, it } from "vitest";
import { DEFAULT_SPLIT, FILES_MIN, INFO_MIN, infoWidth, splitFor } from "./detailsLayout";

describe("infoWidth", () => {
  it("takes the chosen share of the panel, 44 % by default", () => {
    expect(DEFAULT_SPLIT).toBe(0.44);
    expect(infoWidth(DEFAULT_SPLIT, 1280)).toBe(563);
    expect(infoWidth(0.5, 1000)).toBe(500);
  });

  it("keeps 320 px for the information and 280 px for the files", () => {
    expect(INFO_MIN).toBe(320);
    expect(FILES_MIN).toBe(280);
    expect(infoWidth(0.1, 1280)).toBe(320);
    expect(infoWidth(0.95, 1280)).toBe(1000);
  });

  it("shares a panel too narrow for both minimums in their proportion", () => {
    expect(infoWidth(0.44, 500)).toBe(267);
    expect(infoWidth(0.9, 0)).toBe(0);
  });
});

describe("splitFor", () => {
  it("is the share a dragged width stands for, within the minimums", () => {
    expect(splitFor(640, 1280)).toBe(0.5);
    expect(splitFor(100, 1280)).toBe(320 / 1280);
    expect(splitFor(1270, 1280)).toBe(1000 / 1280);
    expect(splitFor(300, 0)).toBe(DEFAULT_SPLIT);
  });
});

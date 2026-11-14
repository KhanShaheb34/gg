import { describe, expect, it } from "vitest";
import { DEFAULT_SPLIT, FILES_MIN, INFO_MIN, infoWidth, splitFor, LOG_MIN_H, MIN_GRAPH_H, detailsRoom, panelHeight, panelRoom } from "./detailsLayout";

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

describe("bottom panels", () => {
  it("leave the graph its minimum below the banner", () => {
    expect(panelRoom(600, 0, 0)).toBe(600 - MIN_GRAPH_H);
    expect(panelRoom(600, 200, 0)).toBe(600 - MIN_GRAPH_H - 200);
    expect(panelRoom(600, 200, LOG_MIN_H)).toBe(600 - MIN_GRAPH_H - 200 - LOG_MIN_H);
    expect(panelRoom(300, 250, 0)).toBe(0);
  });

  it("take the height wanted, at least their minimum, never more than the room", () => {
    expect(panelHeight(280, 120, 400)).toBe(280);
    expect(panelHeight(60, 120, 400)).toBe(120);
    expect(panelHeight(280, 120, 200)).toBe(200);
    expect(panelHeight(280, 120, 90)).toBe(90);
  });

  it("keep the log its minimum beside the details, or half when room is short", () => {
    expect(detailsRoom(400, false)).toBe(400);
    expect(detailsRoom(400, true)).toBe(400 - LOG_MIN_H);
    expect(detailsRoom(90, true)).toBe(45);
    expect(detailsRoom(0, true)).toBe(0);
  });
});

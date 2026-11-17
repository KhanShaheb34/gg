import { describe, expect, it } from "vitest";
import { DESCRIPTION_ROOM, fitColumns, MIN_W } from "./graphColumns.svelte";

const wide = { graph: 60, date: 140, author: 150, hash: 84 };

describe("fitColumns (final boundary review I2)", () => {
  it("leaves the widths as they are when the description has its room", () => {
    expect(fitColumns(wide, 1280)).toEqual(wide);
    // Exactly the room: nothing gives way.
    const at = wide.graph + wide.date + wide.author + wide.hash + DESCRIPTION_ROOM;
    expect(fitColumns(wide, at)).toEqual(wide);
  });

  it("narrows Author first, then Date, so the description keeps its room", () => {
    // 40 px short: Author alone gives them.
    const short40 = wide.graph + wide.date + wide.author + wide.hash + DESCRIPTION_ROOM - 40;
    expect(fitColumns(wide, short40)).toEqual({ ...wide, author: 110 });
    // More than Author can give: Author at its minimum, then Date.
    const short120 = short40 - 80;
    expect(fitColumns(wide, short120)).toEqual({ ...wide, author: MIN_W.author, date: 140 - (120 - (150 - MIN_W.author)) });
  });

  it("never goes below the minimums, and never touches Graph or Commit", () => {
    expect(fitColumns(wide, 300)).toEqual({ ...wide, author: MIN_W.author, date: MIN_W.date });
  });

  it("at 900 px with the side panel docked (660 px for the graph) a subject has at least 280 px", () => {
    const fitted = fitColumns(wide, 660);
    expect(660 - fitted.graph - fitted.date - fitted.author - fitted.hash).toBeGreaterThanOrEqual(DESCRIPTION_ROOM);
  });
});

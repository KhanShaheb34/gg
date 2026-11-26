import { describe, expect, it } from "vitest";
import { fetchedText, nextChangeIn } from "./statusBar";

const NOW = Date.UTC(2026, 10, 26, 12, 0, 0);
const S = 1000;
const MIN = 60 * S;
const H = 60 * MIN;
const D = 24 * H;

describe("fetchedText", () => {
  it("says how long ago, in the app's relative wording", () => {
    expect(fetchedText(NOW, NOW)).toBe("Fetched just now");
    expect(fetchedText(NOW - 59 * S, NOW)).toBe("Fetched just now");
    expect(fetchedText(NOW - 60 * S, NOW)).toBe("Fetched 1 min ago");
    expect(fetchedText(NOW - 2 * MIN - 30 * S, NOW)).toBe("Fetched 2 min ago");
    expect(fetchedText(NOW - 59 * MIN, NOW)).toBe("Fetched 59 min ago");
    expect(fetchedText(NOW - 3 * H, NOW)).toBe("Fetched 3 h ago");
    expect(fetchedText(NOW - 30 * H, NOW)).toBe("Fetched yesterday");
    expect(fetchedText(NOW - 3 * D, NOW)).toBe("Fetched 3 days ago");
    expect(fetchedText(NOW - 21 * D, NOW)).toBe("Fetched 3 weeks ago");
    expect(fetchedText(NOW - 400 * D, NOW)).toBe("Fetched 1 year ago");
  });

  it("reads a time slightly ahead of this clock as just now", () => {
    expect(fetchedText(NOW + 5 * S, NOW)).toBe("Fetched just now");
  });

  it("says nothing while nothing says when it was (a fresh clone, a last fetch that failed)", () => {
    expect(fetchedText(null, NOW)).toBeNull();
  });
});

describe("nextChangeIn", () => {
  /** The wording an instant before and at the change differ; a moment before, they match. */
  function changesExactlyThen(ms: number, now: number) {
    const wait = nextChangeIn(ms, now);
    expect(wait).toBeGreaterThan(0);
    expect(fetchedText(ms, now + wait - 1)).toBe(fetchedText(ms, now));
    expect(fetchedText(ms, now + wait)).not.toBe(fetchedText(ms, now));
  }

  it("is the moment the wording changes, at every scale", () => {
    for (const age of [0, 10 * S, 59 * S, MIN, 61 * S, 5 * MIN + 30 * S, 59 * MIN + 59 * S, H, 3 * H + 17 * MIN, 23 * H + 59 * MIN, D, 30 * H, 2 * D, 2 * D + 5 * H, 10 * D]) {
      changesExactlyThen(NOW - age, NOW);
    }
  });

  it("waits a minute at most while under an hour, an hour while under a day", () => {
    expect(nextChangeIn(NOW, NOW)).toBe(MIN);
    expect(nextChangeIn(NOW - 90 * S, NOW)).toBe(30 * S);
    expect(nextChangeIn(NOW - 2 * H - 10 * MIN, NOW)).toBe(50 * MIN);
    expect(nextChangeIn(NOW - 30 * H, NOW)).toBe(18 * H);
  });

  it("counts a time ahead of this clock from that time", () => {
    expect(nextChangeIn(NOW + 5 * S, NOW)).toBe(MIN + 5 * S);
  });
});

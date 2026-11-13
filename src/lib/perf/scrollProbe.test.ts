import { describe, expect, it } from "vitest";
import { percentile, stepTop, summarize, type Probe } from "./scrollProbe";

describe("percentile", () => {
  it("uses the nearest rank", () => {
    const v = [5, 1, 4, 2, 3, 6, 7, 8, 9, 10];
    expect(percentile(v, 50)).toBe(5);
    expect(percentile(v, 95)).toBe(10);
    expect(percentile(v, 10)).toBe(1);
    expect(percentile(v, 0)).toBe(1);
    expect(percentile(v, 100)).toBe(10);
  });

  it("is 0 without values and leaves the input unsorted", () => {
    expect(percentile([], 95)).toBe(0);
    const v = [3, 1, 2];
    percentile(v, 50);
    expect(v).toEqual([3, 1, 2]);
  });

  it("p95 of 100 values is the 95th smallest", () => {
    const v = Array.from({ length: 100 }, (_, i) => 100 - i);
    expect(percentile(v, 95)).toBe(95);
    expect(percentile(v, 99)).toBe(99);
  });
});

describe("stepTop", () => {
  it("goes from just below the top to exactly the bottom", () => {
    expect(stepTop(1, 200, 1000)).toBe(5);
    expect(stepTop(100, 200, 1001)).toBe(501);
    expect(stepTop(200, 200, 2_600_000)).toBe(2_600_000);
  });
});

describe("summarize", () => {
  it("reports frame percentiles, slow frames and skeleton counts", () => {
    const probe: Probe = { frames: [], skeletonMaxMs: 120, slowSkeletonFrames: 2 };
    const deltas = [8, 8, 9, 16, 17, 18, 33];
    expect(summarize(deltas, probe, 0)).toEqual({
      frames: 7,
      p50: 16,
      p95: 33,
      p99: 33,
      max: 33,
      over17: 2,
      over25: 1,
      skeletonMaxMs: 120,
      slowSkeletonFrames: 2,
      skeletonsAfterStop: 0,
    });
    expect(summarize([], probe, 3)).toMatchObject({ frames: 0, p95: 0, max: 0, skeletonsAfterStop: 3 });
  });
});

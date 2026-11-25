import { beforeEach, describe, expect, it, vi } from "vitest";
import type { PrDetails } from "../types";

// Each pr_details request, answered when the test says.
const asked = vi.hoisted(() => [] as { number: number; resolve: (d: unknown) => void; reject: (e: unknown) => void }[]);
vi.mock("../ipc", () => ({
  api: {
    getRepoPrefs: vi.fn(async () => ({ sidePanel: true })),
    prDetails: vi.fn((_repoId: number, number: number) => new Promise((resolve, reject) => asked.push({ number, resolve, reject }))),
  },
}));
vi.mock("./repo.svelte", () => ({ findRepoView: () => undefined }));
vi.mock("./app.svelte", () => ({ app: { settings: {} }, reportError: vi.fn(), toast: vi.fn() }));

import { detailsBusy, detailsOf, forgetDetails, loadDetails } from "./prDetails.svelte";

const details = (number: number, loadedAtMs: number): PrDetails => ({
  number,
  url: `https://github.com/o/r/pull/${number}`,
  state: "open",
  merge: "ready",
  review: "none",
  checks: [],
  checksSummary: { total: 0, passing: 0, failing: 0, pending: 0, skipped: 0, cancelled: 0, neutral: 0, unknown: 0 },
  comments: [],
  commentsOmitted: 0,
  threadsError: null,
  threadsPartial: false,
  loadedAtMs,
});

const settle = () => new Promise((r) => setTimeout(r, 0));

describe("a pull request's details", () => {
  beforeEach(() => {
    asked.length = 0;
    forgetDetails("/r");
  });

  it("are kept per pull request: a late answer for another never shows on this one", async () => {
    const twelve = loadDetails(1, "/r", 12);
    const thirteen = loadDetails(1, "/r", 13);
    asked[1].resolve(details(13, 2));
    await thirteen;
    asked[0].resolve(details(12, 1));
    await twelve;
    expect(detailsOf("/r", 13)?.data?.number).toBe(13);
    expect(detailsOf("/r", 12)?.data?.number).toBe(12);
  });

  it("take an answer only while it is the latest asked for", async () => {
    const older = loadDetails(1, "/r", 12);
    const newer = loadDetails(1, "/r", 12);
    asked[1].resolve(details(12, 2));
    await newer;
    asked[0].resolve(details(12, 1));
    await older;
    expect(detailsOf("/r", 12)?.data?.loadedAtMs).toBe(2);
    expect(detailsOf("/r", 12)?.loading).toBe(false);
  });

  it("keep what was shown when a reload fails, and say why in one line", async () => {
    const first = loadDetails(1, "/r", 12);
    asked[0].resolve(details(12, 1));
    await first;
    const again = loadDetails(1, "/r", 12);
    expect(detailsOf("/r", 12)?.loading).toBe(true);
    asked[1].reject({ code: "io", message: "Could not load the checks and comments: rate limit\nmore" });
    await again;
    const e = detailsOf("/r", 12)!;
    expect(e.data?.loadedAtMs).toBe(1);
    expect(e.error).toBe("Could not load the checks and comments: rate limit");
    expect(e.loading).toBe(false);
    // The next that works clears it.
    const third = loadDetails(1, "/r", 12);
    asked[2].resolve(details(12, 3));
    await third;
    expect(detailsOf("/r", 12)?.error).toBe(null);
  });

  it("show the Refresh button busy for a load the user would see, not for a poll", async () => {
    const poll = loadDetails(1, "/r", 12, true);
    await settle();
    expect(detailsBusy("/r")).toBe(false);
    asked[0].resolve(details(12, 1));
    await poll;
    const user = loadDetails(1, "/r", 12);
    expect(detailsBusy("/r")).toBe(true);
    expect(detailsBusy("/other")).toBe(false);
    asked[1].resolve(details(12, 2));
    await user;
    expect(detailsBusy("/r")).toBe(false);
  });
});

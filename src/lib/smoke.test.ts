import { describe, expect, it } from "vitest";
import { colorIndex, displayFolder, homeInText, relativeTime, shortHash, worktreePathsShown } from "./format";

describe("shortHash", () => {
  it("returns the first 8 characters of a hash", () => {
    expect(shortHash("471663edabcdef")).toBe("471663ed");
  });
});

describe("relativeTime", () => {
  const now = Date.UTC(2026, 10, 13, 12, 0, 0);
  const ago = (ms: number) => relativeTime(now - ms, now);
  const MIN = 60_000;
  const HOUR = 60 * MIN;
  const DAY = 24 * HOUR;

  it("speaks in the largest whole unit", () => {
    expect(ago(10_000)).toBe("just now");
    expect(ago(-5_000)).toBe("just now");
    expect(ago(MIN)).toBe("1 min ago");
    expect(ago(45 * MIN)).toBe("45 min ago");
    expect(ago(2 * HOUR)).toBe("2 h ago");
    expect(ago(DAY)).toBe("yesterday");
    expect(ago(3 * DAY)).toBe("3 days ago");
    expect(ago(14 * DAY)).toBe("2 weeks ago");
    expect(ago(60 * DAY)).toBe("2 months ago");
    expect(ago(800 * DAY)).toBe("2 years ago");
  });
});

describe("displayFolder", () => {
  it("shows the parent folder with the home folder as ~", () => {
    expect(displayFolder("/Users/ada/Dev/gg", "/Users/ada")).toBe("~/Dev");
    expect(displayFolder("/home/ada/src/x", "/home/ada")).toBe("~/src");
    expect(displayFolder("/Users/ada/x", "/Users/ada")).toBe("~");
    expect(displayFolder("/Users/ada/Dev/x")).toBe("/Users/ada/Dev");
    expect(displayFolder("/srv/repos/x", "/Users/ada")).toBe("/srv/repos");
    expect(displayFolder("/x")).toBe("/");
    expect(displayFolder("C:\\Users\\ada\\code\\x", "C:\\Users\\ada")).toBe("~\\code");
    expect(displayFolder("D:\\work\\x")).toBe("D:\\work");
  });
});

describe("colorIndex", () => {
  it("gives each key a stable lane colour index in 0..9", () => {
    const keys = ["/a", "/b", "/Users/ada/Dev/gg", "", "ünïcode path"];
    for (const k of keys) {
      const i = colorIndex(k);
      expect(i).toBeGreaterThanOrEqual(0);
      expect(i).toBeLessThan(10);
      expect(Number.isInteger(i)).toBe(true);
      expect(colorIndex(k)).toBe(i);
    }
    const spread = new Set(Array.from({ length: 50 }, (_, n) => colorIndex(`/repo/${n}`)));
    expect(spread.size).toBeGreaterThan(5);
  });
});

describe("worktreePathsShown", () => {
  const home = "/Users/ada";
  it("elides the folders every worktree shares, home as ~", () => {
    const paths = ["/Users/ada/Dev/app", "/Users/ada/Dev/app.worktrees/fix", "/Users/ada/Dev/app.worktrees/feature-x"];
    const shown = worktreePathsShown(paths, home);
    expect(shown.get(paths[0])).toBe("…/app");
    expect(shown.get(paths[1])).toBe("…/app.worktrees/fix");
    expect(shown.get(paths[2])).toBe("…/app.worktrees/feature-x");
  });

  it("shows one worktree, or worktrees sharing only ~ or the root, whole", () => {
    expect(worktreePathsShown(["/Users/ada/Dev/app"], home).get("/Users/ada/Dev/app")).toBe("~/Dev/app");
    const apart = worktreePathsShown(["/Users/ada/app", "/Users/ada/wt"], home);
    expect([...apart.values()]).toEqual(["~/app", "~/wt"]);
    const roots = worktreePathsShown(["/srv/app", "/tmp/wt"], home);
    expect([...roots.values()]).toEqual(["/srv/app", "/tmp/wt"]);
  });

  it("keeps each worktree's own folder, even one inside another's", () => {
    const shown = worktreePathsShown(["/Users/ada/Dev/app", "/Users/ada/Dev/app/wt"], home);
    expect([...shown.values()]).toEqual(["…/app", "…/app/wt"]);
  });

  it("works with Windows paths", () => {
    const shown = worktreePathsShown(["C:\\Users\\ada\\code\\app", "C:\\Users\\ada\\code\\app-fix"], "C:\\Users\\ada");
    expect([...shown.values()]).toEqual(["…\\app", "…\\app-fix"]);
  });
});

describe("homeInText", () => {
  it("shows paths under the home folder as ~ wherever a path starts", () => {
    const home = "/Users/ada";
    expect(homeInText("Remove worktree /Users/ada/Dev/app.worktrees/fix", home)).toBe("Remove worktree ~/Dev/app.worktrees/fix");
    expect(homeInText("/Users/ada/x and (/Users/ada/y) “/Users/ada/z”", home)).toBe("~/x and (~/y) “~/z”");
    // Not a path under home: a longer folder name, or in the middle of a word.
    expect(homeInText("/Users/adam/x a/Users/ada/y", home)).toBe("/Users/adam/x a/Users/ada/y");
    expect(homeInText("/Users/ada/x", null)).toBe("/Users/ada/x");
    expect(homeInText("C:\\Users\\Ada\\code\\x", "C:\\Users\\ada")).toBe("~\\code\\x");
    expect(homeInText("/x/y", "/")).toBe("/x/y");
  });
});

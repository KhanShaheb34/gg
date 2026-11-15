import { describe, expect, it } from "vitest";
import { colorIndex, cutToFit, displayFolder, homeInText, relativeTime, shortHash, worktreeFolders } from "./format";

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

describe("worktreeFolders", () => {
  const home = "/Users/ada";
  const wt = "/Users/ada/repos/app.worktrees";

  it("names the worktrees in the shared folder by their folder name, and says the folder once", () => {
    const paths = ["/Users/ada/repos/app", `${wt}/fix`, `${wt}/feature-x`];
    const f = worktreeFolders(paths, paths[0], home);
    expect(f.parent).toBe("~/repos/app.worktrees");
    expect(f.parentPath).toBe(wt);
    expect([...f.labels.values()]).toEqual(["app", "fix", "feature-x"]);
  });

  it("gives a worktree elsewhere the shortest end that tells it apart", () => {
    const paths = ["/Users/ada/repos/app", `${wt}/a`, `${wt}/b`, "/Users/ada/other/fix", "/srv/x/y/fix"];
    const f = worktreeFolders(paths, paths[0], home);
    expect(f.parent).toBe("~/repos/app.worktrees");
    expect(f.labels.get("/Users/ada/other/fix")).toBe("~/other/fix");
    expect(f.labels.get("/srv/x/y/fix")).toBe("…/y/fix");
    const deep = worktreeFolders(["/Users/ada/repos/app", `${wt}/a`, "/Users/ada/p/q/r/fix"], "/Users/ada/repos/app", home);
    expect(deep.labels.get("/Users/ada/p/q/r/fix")).toBe("~/…/r/fix");
  });

  it("makes names that would be the same longer, the main one too", () => {
    const paths = ["/Users/ada/repos/fix", `${wt}/fix`, `${wt}/b`];
    const f = worktreeFolders(paths, paths[0], home);
    expect(f.labels.get(paths[0])).toBe("~/repos/fix");
    expect(f.labels.get(paths[1])).toBe("~/…/app.worktrees/fix");
    expect(f.labels.get(paths[2])).toBe("b");
  });

  it("has no shared folder with only the main worktree", () => {
    const f = worktreeFolders(["/Users/ada/repos/app"], "/Users/ada/repos/app", home);
    expect(f.parent).toBeNull();
    expect(f.labels.get("/Users/ada/repos/app")).toBe("app");
  });

  it("works with Windows paths", () => {
    const paths = ["C:\\Users\\ada\\code\\app", "C:\\Users\\ada\\code\\app.worktrees\\fix"];
    const f = worktreeFolders(paths, paths[0], "C:\\Users\\ada");
    expect(f.parent).toBe("~\\code\\app.worktrees");
    expect([...f.labels.values()]).toEqual(["app", "fix"]);
  });
});

describe("cutToFit", () => {
  // One unit per character.
  const width = (s: string) => [...s].length;
  it("keeps the end and as much of the start as fits", () => {
    expect(cutToFit("feature/payments-service-v2", 30, 10, width)).toBeNull();
    expect(cutToFit("feature/payments-service-v2", 20, 10, width)).toBe("feature/p…service-v2");
    expect(cutToFit("abcdefgh", 5, 10, width)).toBe("…efgh");
    expect(cutToFit("abcdefgh", 7, 10, width)).toBe("ab…efgh");
  });

  it("lets the end give way too when nothing else fits", () => {
    expect(cutToFit("abcdefghij", 3, 4, width)).toBe("…ij");
    expect(cutToFit("abcdefghij", 1, 4, width)).toBe("…");
    expect(cutToFit("é😀abcdef", 5, 3, width)).toBe("é…def");
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

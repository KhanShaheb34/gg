import { describe, expect, it } from "vitest";
import { refNameProblem } from "./nameCheck";

// The same names as the core's own test (crates/gg-core/src/git/names.rs), which checks them
// against `git check-ref-format --branch`.
const GOOD = ["main", "feature/x", "feature/émoji-🚀", "a.b", "v1.2.3", "a@b", "under_score-dash", "deep/er/path"];
const BAD = [
  "",
  "-f",
  "--force",
  "HEAD",
  "@",
  "has space",
  "a..b",
  "a~1",
  "a^",
  "a:b",
  "a?",
  "a*",
  "a[b",
  "a\\b",
  "a@{1}",
  "/a",
  "a/",
  "a//b",
  "a.",
  ".a",
  "a/.b",
  "a.lock",
  "a.lock/b",
  "tab\there",
  "nl\nhere",
  "del\u007f",
];

describe("refNameProblem", () => {
  it("takes the names git takes", () => {
    for (const name of GOOD) expect(refNameProblem(name, []), name).toBeNull();
  });

  it("refuses the names the core refuses", () => {
    for (const name of BAD) expect(refNameProblem(name, []), JSON.stringify(name)).not.toBeNull();
  });

  it("says why, in plain words", () => {
    expect(refNameProblem("", [])).toBe("A name is needed.");
    expect(refNameProblem("-f", [])).toBe("It can’t start with a hyphen.");
    expect(refNameProblem("has space", [])).toBe("It can’t contain spaces.");
    expect(refNameProblem("a..b", [])).toBe("It can’t contain two dots in a row.");
    expect(refNameProblem("a~1", [])).toBe("It can’t contain any of: ~ ^ : ? * [ \\ or a space.");
    expect(refNameProblem("x.lock", [])).toBe("No part of it can end in “.lock”, git’s own ending.");
    expect(refNameProblem("x.", [])).toBe("It can’t end with a dot.");
    expect(refNameProblem("a/.b", [])).toBe("No part of it can start with a dot.");
    expect(refNameProblem("a//b", [])).toBe("It can’t start or end with a slash, or have two in a row.");
    expect(refNameProblem("a@{b", [])).toBe("It can’t contain “@{” anywhere.");
  });

  it("never ends a problem with a quote before its full stop", () => {
    for (const name of BAD) {
      const why = refNameProblem(name, []) ?? "";
      expect(why, JSON.stringify(name)).not.toMatch(/[”"]\.$/);
    }
    expect(refNameProblem("HEAD", [])).toBe("HEAD is git’s name for the current commit.");
  });

  it("refuses a name that is taken, naming what has it", () => {
    expect(refNameProblem("main", ["main", "dev"])).toBe("A branch named “main” already exists.");
    expect(refNameProblem("v1", ["v1"], "tag")).toBe("A tag named “v1” already exists.");
    expect(refNameProblem("Main", ["main"])).toBeNull();
  });

  it("puts a problem with the name itself before its being taken", () => {
    expect(refNameProblem("a b", ["a b"])).toBe("It can’t contain spaces.");
  });
});

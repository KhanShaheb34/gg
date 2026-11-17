import { describe, expect, it } from "vitest";
import { folderOf, launcherPath, preferredFolder } from "./cli";

describe("launcherPath", () => {
  it("puts gg in the folder with the folder's own separator", () => {
    expect(launcherPath("/Users/ada/.local/bin")).toBe("/Users/ada/.local/bin/gg");
    expect(launcherPath("/usr/local/bin/")).toBe("/usr/local/bin/gg");
    // Windows: the launcher is gg.cmd (Task 11).
    expect(launcherPath("C:\\Users\\ada\\bin")).toBe("C:\\Users\\ada\\bin\\gg.cmd");
    expect(launcherPath("C:\\")).toBe("C:\\gg.cmd");
  });
});

describe("folderOf", () => {
  it("gives the folder of a launcher", () => {
    expect(folderOf("/Users/ada/bin/gg")).toBe("/Users/ada/bin");
    expect(folderOf("/gg")).toBe("/");
    expect(folderOf("C:\\bin\\gg")).toBe("C:\\bin");
  });
});

describe("preferredFolder", () => {
  const candidates = ["/h/.local/bin", "/opt/homebrew/bin", "/h/bin"];

  it("offers where the command is now, if it can still go there", () => {
    expect(preferredFolder({ installed: "/h/bin/gg", shadowedBy: null, candidates })).toBe("/h/bin");
  });

  it("else the best folder", () => {
    expect(preferredFolder({ installed: null, shadowedBy: null, candidates })).toBe("/h/.local/bin");
    expect(preferredFolder({ installed: "/root-only/gg", shadowedBy: null, candidates })).toBe("/h/.local/bin");
  });

  it("none when no folder can take it", () => {
    expect(preferredFolder({ installed: null, shadowedBy: null, candidates: [] })).toBeNull();
  });
});

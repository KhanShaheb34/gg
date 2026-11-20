import { describe, expect, it } from "vitest";
import { folderOf, installedAt, launcherPath, linkNote, preferredFolder } from "./cli";
import type { CliState } from "./types";

describe("launcherPath", () => {
  it("puts gg in the folder with the folder's own separator", () => {
    expect(launcherPath("/Users/ada/.local/bin")).toBe("/Users/ada/.local/bin/gg");
    expect(launcherPath("/usr/local/bin/")).toBe("/usr/local/bin/gg");
    // Windows: the launcher is gg.cmd.
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
    expect(preferredFolder({ installed: "/h/bin/gg", link: null, shadowedBy: null, candidates })).toBe("/h/bin");
  });

  it("else the best folder", () => {
    expect(preferredFolder({ installed: null, link: null, shadowedBy: null, candidates })).toBe("/h/.local/bin");
    expect(preferredFolder({ installed: "/root-only/gg", link: null, shadowedBy: null, candidates })).toBe("/h/.local/bin");
  });

  it("none when no folder can take it", () => {
    expect(preferredFolder({ installed: null, link: null, shadowedBy: null, candidates: [] })).toBeNull();
  });
});

describe("installedAt and linkNote", () => {
  const state = (installed: string | null, link: CliState["link"]): CliState => ({
    installed,
    link,
    shadowedBy: null,
    candidates: [],
  });
  const short = (p: string) => p.replace("/h/", "~/");

  it("say where the command is and who made it", () => {
    expect(installedAt(state("/h/bin/gg", null), short)).toBe("installed at ~/bin/gg");
    expect(installedAt(state("/opt/homebrew/bin/gg", "homebrew"), short)).toBe("installed by Homebrew at /opt/homebrew/bin/gg");
    expect(installedAt(state("/usr/local/bin/gg", "other"), short)).toBe("installed at /usr/local/bin/gg, a link to gg’s launcher");
    expect(installedAt(state(null, null), short)).toBeNull();
  });

  it("say nothing more of a launcher gg wrote; a link is left to whoever made it", () => {
    expect(linkNote(null)).toBeNull();
    expect(linkNote("homebrew")).toContain("Homebrew made it");
    expect(linkNote("homebrew")).toContain("gg leaves it alone.");
    expect(linkNote("other")).toBe("gg did not make this link and leaves it alone.");
  });
});

import { describe, expect, it } from "vitest";
import {
  conflictWord,
  folderOf,
  installedAt,
  launcherPath,
  lineAddedNote,
  lineFailed,
  lineRemovedNote,
  linkNote,
  preferredFolder,
  turnOffCommand,
} from "./cli";
import type { CliState, ShellLineAdded } from "./types";

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
    expect(preferredFolder({ installed: "/h/bin/gg", link: null, shadowedBy: null, candidates, shadowConflict: null, turnOff: null, shellLine: null })).toBe("/h/bin");
  });

  it("else the best folder", () => {
    expect(preferredFolder({ installed: null, link: null, shadowedBy: null, candidates, shadowConflict: null, turnOff: null, shellLine: null })).toBe("/h/.local/bin");
    expect(preferredFolder({ installed: "/root-only/gg", link: null, shadowedBy: null, candidates, shadowConflict: null, turnOff: null, shellLine: null })).toBe("/h/.local/bin");
  });

  it("none when no folder can take it", () => {
    expect(preferredFolder({ installed: null, link: null, shadowedBy: null, candidates: [], shadowConflict: null, turnOff: null, shellLine: null })).toBeNull();
  });
});

describe("installedAt and linkNote", () => {
  const state = (installed: string | null, link: CliState["link"]): CliState => ({
    installed,
    link,
    shadowedBy: null,
    candidates: [],
    shadowConflict: null,
    turnOff: null,
    shellLine: null,
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

describe("turning off the shell's own gg", () => {
  const short = (p: string) => p.replace("/h/", "~/");
  const added = (over: Partial<ShellLineAdded> = {}): ShellLineAdded => ({
    file: "/h/.zshrc",
    conflict: "alias",
    added: true,
    problem: null,
    checked: true,
    ...over,
  });

  it("names what is turned off and the line that does it", () => {
    expect(conflictWord("alias")).toBe("alias");
    expect(conflictWord("function")).toBe("function");
    expect(turnOffCommand("alias")).toBe("unalias gg 2>/dev/null");
    expect(turnOffCommand("function")).toBe("unset -f gg 2>/dev/null");
  });

  it("says it worked only when the shell, asked again, has no gg of its own", () => {
    expect(lineAddedNote(added(), null, short)).toBe(
      "Turned off your shell’s gg alias: added one line to the end of ~/.zshrc. It takes effect in new terminal windows.",
    );
    expect(lineAddedNote(added({ added: false }), null, short)).toBe(
      "Your shell’s gg alias is turned off by gg’s line in ~/.zshrc. It takes effect in new terminal windows.",
    );
    expect(lineAddedNote(added(), "an alias for git gui citool", short)).toBe(
      "Added one line to the end of ~/.zshrc, but your shell still defines gg as an alias for git gui citool: something that runs after that line defines it again.",
    );
    expect(lineAddedNote(added({ checked: false, conflict: "function" }), null, short)).toBe(
      "Added one line to the end of ~/.zshrc to turn off your shell’s gg function, but gg could not ask your shell whether that worked. It takes effect in new terminal windows; run type gg in one to see.",
    );
  });

  it("says why the line could not be added", () => {
    const problem = "Could not add a line to ~/.zshrc: Permission denied (os error 13).";
    expect(lineAddedNote(added({ added: false, problem }), "an alias for x", short)).toBe(
      "Did not turn off your shell’s gg alias: Could not add a line to ~/.zshrc: Permission denied (os error 13).",
    );
  });

  it("says the line failed only when an alias or function it should turn off is still there", () => {
    const state: CliState = {
      installed: null,
      link: null,
      shadowedBy: "an alias for x",
      candidates: [],
      shadowConflict: "alias",
      turnOff: null,
      shellLine: "/h/.zshrc",
    };
    expect(lineFailed(state)).toBe(true);
    // Another program called gg: not the line's to turn off.
    expect(lineFailed({ ...state, shadowedBy: "the program /x/gg", shadowConflict: null })).toBe(false);
    // A function now, which another line would turn off (offered).
    expect(lineFailed({ ...state, shadowConflict: "function", turnOff: { conflict: "function", file: "/h/.zshrc" } })).toBe(false);
    expect(lineFailed({ ...state, shellLine: null })).toBe(false);
    expect(lineFailed({ ...state, shadowedBy: null, shadowConflict: null })).toBe(false);
  });

  it("says where the line was taken out of, and what stood in the way", () => {
    expect(lineRemovedNote({ lineRemovedFrom: [], lineProblem: null }, short)).toBeNull();
    expect(lineRemovedNote({ lineRemovedFrom: ["/h/.zshrc"], lineProblem: null }, short)).toBe(
      "Took gg’s line out of ~/.zshrc; new terminal windows no longer run it.",
    );
    const changed = "A line in ~/.zshrc is marked as added by gg but has been changed since; gg left it alone.";
    expect(lineRemovedNote({ lineRemovedFrom: [], lineProblem: changed }, short)).toBe(changed);
  });
});

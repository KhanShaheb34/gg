import { describe, expect, it } from "vitest";
import { compact, headline, partsOf, plainText, shape, spoken } from "./notice";

describe("notices", () => {
  it("takes plain text or parts", () => {
    expect(partsOf("Copied the hash.")).toEqual([{ text: "Copied the hash." }]);
    expect(partsOf("")).toEqual([]);
    expect(partsOf(undefined)).toEqual([]);
    const parts = [{ text: "Pushed " }, { ref: "main" }];
    expect(partsOf(parts)).toBe(parts);
    expect(plainText(parts)).toBe("Pushed main");
  });

  it("drops a headline's closing full stop, nothing else", () => {
    expect(headline("Copied the hash.")).toEqual([{ text: "Copied the hash" }]);
    expect(headline("Wait…")).toEqual([{ text: "Wait…" }]);
    expect(headline("Done?")).toEqual([{ text: "Done?" }]);
    expect(headline("v1.2..")).toEqual([{ text: "v1.2.." }]);
    // A name at the end is never cut.
    expect(headline([{ text: "Deleted " }, { ref: "x." }])).toEqual([{ text: "Deleted " }, { ref: "x." }]);
  });

  it("reads out as one sentence", () => {
    const title = [{ text: "Pull request #990 opened" }];
    const note = [{ ref: "shk/x" }, { text: " → " }, { ref: "main" }, { text: " · pushed to origin" }];
    expect(spoken(title, note)).toBe("Pull request #990 opened. shk/x into main, pushed to origin.");
    expect(spoken("Copied the hash.")).toBe("Copied the hash");
    expect(spoken("Could not reopen ~/x:", "Folder not found.")).toBe("Could not reopen ~/x: Folder not found.");
    expect(spoken("Dropped stash@{0}", "“WIP”")).toBe("Dropped stash@{0}. “WIP”");
  });

  it("shapes plain sentences as the core does: a short lead before a colon, else the first sentence", () => {
    expect(shape("Removed the gg command: ~/bin/gg.")).toEqual({ title: "Removed the gg command", note: "~/bin/gg." });
    expect(shape("Settings could not be saved. Fix it, then restart.")).toEqual({
      title: "Settings could not be saved",
      note: "Fix it, then restart.",
    });
    expect(shape("Copied the hash.")).toEqual({ title: "Copied the hash.", note: [] });
    // A long lead is a sentence that has a colon; a colon in a later sentence is not looked at.
    const long = "This repository has worktrees in folders that are not there: a, b.";
    expect(shape(long)).toEqual({ title: long, note: [] });
    expect(shape("Pick one. Then: go.")).toEqual({ title: "Pick one", note: "Then: go." });
    // A colon with no space after it (a time) is no break; a note given is kept.
    expect(shape("Fetched at 14:05 today")).toEqual({ title: "Fetched at 14:05 today", note: [] });
    expect(shape("A: b", "Given.")).toEqual({ title: "A: b", note: "Given." });
    expect(shape("Did not turn it off: gg found no line.")).toEqual({ title: "Did not turn it off", note: "gg found no line." });
  });

  it("keeps a short line of names on one line, and wraps prose", () => {
    const flow = [{ ref: "feature/a-very-long-branch-name" }, { text: " \u2192 " }, { ref: "main" }, { text: " \u00b7 pushed to origin" }];
    expect(compact(flow, "note")).toBe(true);
    expect(compact([{ text: "Pushed " }, { ref: "x" }, { text: " to origin" }], "title")).toBe(true);
    // A headline has less room for words.
    const held = [{ ref: "feature/login" }, { text: " is checked out in another worktree" }];
    expect(compact(held, "title")).toBe(false);
    expect(compact(held, "note")).toBe(true);
    // No names, or a sentence around one: wrapped.
    expect(compact("Pull to bring them in first.", "note")).toBe(false);
    expect(compact([{ text: "At " }, { path: "/w/x" }, { text: ". Switch to that worktree, or check out another branch here." }], "note")).toBe(false);
  });
});

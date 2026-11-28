import { describe, expect, it } from "vitest";
import { cloneLocation, expandHome, filterRepos, joinPath, knownFor, listCaption, looksLikeLink, newTabRows, readProgress } from "./clone";
import type { KnownRepo, ListedRepo, Recent } from "./types";

const repo = (owner: string, name: string, description: string | null = null): ListedRepo => ({
  host: "github.com",
  owner,
  name,
  private: false,
  archived: false,
  fork: false,
  description,
  pushedAtMs: null,
});

describe("readProgress", () => {
  it("reads git's phase and percent", () => {
    expect(readProgress("Receiving objects:  45% (450/1000), 1.20 MiB | 2.00 MiB/s")).toEqual({
      phase: "Receiving objects",
      percent: 45,
      rest: "(450/1000), 1.20 MiB | 2.00 MiB/s",
    });
    expect(readProgress("remote: Counting objects: 100% (2/2), done.")).toEqual({ phase: "Counting objects", percent: 100, rest: "(2/2)" });
    expect(readProgress("Resolving deltas: 100% (3/3), done.")?.percent).toBe(100);
    expect(readProgress("remote: Enumerating objects: 5, done.")).toEqual({ phase: "Enumerating objects", percent: null, rest: "5" });
  });

  it("takes a line without a percent as its phase alone", () => {
    expect(readProgress("Cloning into '/h/Developer/cat'...")).toEqual({ phase: "Cloning into '/h/Developer/cat'…", percent: null, rest: "" });
    expect(readProgress("   ")).toBeNull();
  });
});

describe("filterRepos", () => {
  const repos = [repo("octo", "website", "The cat site"), repo("hub", "dog"), repo("octo", "cat"), repo("octo", "catalog")];

  it("keeps all of them for no query", () => {
    expect(filterRepos(repos, "  ")).toBe(repos);
  });

  it("matches every word in the name or the description, names that start with it first", () => {
    expect(filterRepos(repos, "cat").map((r) => r.name)).toEqual(["cat", "catalog", "website"]);
    expect(filterRepos(repos, "OCTO cat").map((r) => r.name)).toEqual(["website", "cat", "catalog"]);
    expect(filterRepos(repos, "hub/d").map((r) => r.name)).toEqual(["dog"]);
    expect(filterRepos(repos, "nothing")).toEqual([]);
  });
});

describe("knownFor", () => {
  const known: KnownRepo[] = [
    { path: "/r/old", repoId: null, host: "github.com", owner: "octo", name: "cat" },
    { path: "/r/cat", repoId: 3, host: "github.com", owner: "Octo", name: "Cat" },
    { path: "/r/dog", repoId: null, host: "github.com", owner: "hub", name: "dog" },
  ];

  it("finds an open one before a recent one, in any case", () => {
    expect(knownFor(known, "GitHub.com", "octo", "cat")?.repoId).toBe(3);
    expect(knownFor(known, "github.com", "hub", "dog")?.path).toBe("/r/dog");
    expect(knownFor(known, "ghe.example", "hub", "dog")).toBeNull();
  });
});

describe("paths", () => {
  it("joins a folder to its parent with the parent's separator", () => {
    expect(joinPath("/h/Developer", "cat")).toBe("/h/Developer/cat");
    expect(joinPath("/h/Developer/", "cat")).toBe("/h/Developer/cat");
    expect(joinPath("C:\\Users\\ada\\Projects", "cat")).toBe("C:\\Users\\ada\\Projects\\cat");
  });

  it("reads ~ as the home folder", () => {
    expect(expandHome("~/Developer", "/h")).toBe("/h/Developer");
    expect(expandHome(" ~ ", "/h/")).toBe("/h");
    expect(expandHome("~\\Projects", "C:\\Users\\ada")).toBe("C:\\Users\\ada\\Projects");
    expect(expandHome("/srv/~x", "/h")).toBe("/srv/~x");
    expect(expandHome("~/x", null)).toBe("~/x");
  });

  it("takes the setting's folder, else the platform's", () => {
    expect(cloneLocation("/h/code", "/h/Developer")).toBe("/h/code");
    expect(cloneLocation(null, "/h/Developer")).toBe("/h/Developer");
    expect(cloneLocation(null, null)).toBe("");
  });

  it("says how much of the list it shows", () => {
    expect(listCaption(3, 3)).toBe("Your 3 repositories, most recently pushed first");
    expect(listCaption(1, 1)).toBe("Your one repository");
    expect(listCaption(100, 240)).toBe("Your 100 most recently pushed repositories, of 240. For another, paste its link.");
  });
});

describe("newTabRows", () => {
  const recents: Recent[] = [
    { path: "/r/cat", name: "cat", lastOpenedMs: 2 },
    { path: "/r/dog", name: "dog", lastOpenedMs: 1 },
  ];

  it("offers the folder picker, the recent repositories and both ways to clone", () => {
    expect(newTabRows(recents, new Set(["/r/dog"]), "").map((r) => r.kind)).toEqual(["open", "recent", "recent", "clone", "mine"]);
    const rows = newTabRows(recents, new Set(["/r/dog"]), "");
    expect(rows[2]).toMatchObject({ kind: "recent", open: true });
  });

  it("filters the recent ones; a link typed becomes the clone row's", () => {
    expect(newTabRows(recents, new Set(), "do").map((r) => r.kind)).toEqual(["recent", "clone", "mine"]);
    expect(newTabRows(recents, new Set(), "open").map((r) => r.kind)).toEqual(["open", "clone", "mine"]);
    const rows = newTabRows(recents, new Set(), " https://github.com/octo/cat ");
    expect(rows).toEqual([{ kind: "clone", link: "https://github.com/octo/cat" }]);
    expect(newTabRows(recents, new Set(), "octo/cat").at(-1)).toEqual({ kind: "clone", link: "octo/cat" });
  });

  it("tells a link from a name", () => {
    for (const link of ["https://github.com/octo/cat", "git@github.com:octo/cat.git", "octo/cat", "ssh://git@host/x"]) expect(looksLikeLink(link)).toBe(true);
    for (const name of ["cat", "my repo", "~/code", "/r/cat"]) expect(looksLikeLink(name)).toBe(false);
  });
});

import { describe, expect, it } from "vitest";
import { branchItems, filterLabel, toggleBranch } from "./branchFilter";

describe("branchItems", () => {
  const local = ["main", "feature/login", "fix/typo"];
  const remote = ["origin/main", "origin/fix/typo"];

  it("lists Show all, then local branches, then remote branches under headings", () => {
    expect(branchItems(local, remote, true, "")).toEqual([
      { kind: "all" },
      { kind: "heading", label: "Local branches" },
      { kind: "branch", name: "main", remote: false },
      { kind: "branch", name: "feature/login", remote: false },
      { kind: "branch", name: "fix/typo", remote: false },
      { kind: "heading", label: "Remote branches" },
      { kind: "branch", name: "origin/main", remote: true },
      { kind: "branch", name: "origin/fix/typo", remote: true },
    ]);
  });

  it("leaves remote branches out when remotes are hidden", () => {
    expect(branchItems(local, remote, false, "").map((i) => (i.kind === "branch" ? i.name : i.kind))).toEqual([
      "all",
      "heading",
      "main",
      "feature/login",
      "fix/typo",
    ]);
  });

  it("filters by a case-insensitive substring and drops empty sections", () => {
    expect(branchItems(local, remote, true, "TYPO")).toEqual([
      { kind: "all" },
      { kind: "heading", label: "Local branches" },
      { kind: "branch", name: "fix/typo", remote: false },
      { kind: "heading", label: "Remote branches" },
      { kind: "branch", name: "origin/fix/typo", remote: true },
    ]);
    expect(branchItems(local, remote, true, "login")).toEqual([
      { kind: "all" },
      { kind: "heading", label: "Local branches" },
      { kind: "branch", name: "feature/login", remote: false },
    ]);
    expect(branchItems(local, remote, true, "nothing")).toEqual([{ kind: "all" }]);
  });

  it("stays fast with thousands of branches", () => {
    const many = Array.from({ length: 20_000 }, (_, i) => `feature/branch-${i}`);
    const start = performance.now();
    for (let k = 0; k < 10; k++) branchItems(many, many, true, `branch-${k}9`);
    expect((performance.now() - start) / 10).toBeLessThan(15);
  });
});

describe("toggleBranch", () => {
  it("selects only the branch when all are shown", () => {
    expect(toggleBranch(null, "main")).toEqual(["main"]);
  });
  it("adds and removes branches, in the order chosen", () => {
    expect(toggleBranch(["main"], "fix/typo")).toEqual(["main", "fix/typo"]);
    expect(toggleBranch(["main", "fix/typo"], "main")).toEqual(["fix/typo"]);
  });
  it("shows all again when the last branch is removed", () => {
    expect(toggleBranch(["main"], "main")).toBeNull();
  });
});

describe("filterLabel", () => {
  it("names the selection", () => {
    expect(filterLabel(null)).toBe("Show all");
    expect(filterLabel([])).toBe("Show all");
    expect(filterLabel(["main"])).toBe("1 branch");
    expect(filterLabel(["main", "dev", "x"])).toBe("3 branches");
  });
});

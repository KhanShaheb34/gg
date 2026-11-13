import { describe, expect, it } from "vitest";
import { disabled, find, ids, inProgress, label, RUNNING, snap } from "./menuTestData";
import { tagMenu } from "./tagMenu";

const tag = label({ kind: "tag", name: "v1.0" });

describe("tagMenu", () => {
  it("lists push, delete and copy name", () => {
    const items = tagMenu(tag, snap());
    expect(ids(items)).toEqual(["push", "delete", "-", "copy.name"]);
    expect(disabled(items)).toEqual([]);
    expect(find(items, "push").label).toBe("Push…");
    expect(find(items, "delete").danger).toBe(true);
  });

  it("cannot push without a remote", () => {
    const items = tagMenu(tag, snap({ remotes: [] }));
    expect(disabled(items)).toEqual(["push"]);
    expect(find(items, "push").disabledReason).toBe("This repository has no remote to push to");
  });

  it("stays fully enabled while an operation runs or one is in progress", () => {
    expect(disabled(tagMenu(tag, snap(RUNNING)))).toEqual([]);
    expect(disabled(tagMenu(tag, snap(inProgress("merge"))))).toEqual([]);
  });
});

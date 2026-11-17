import { describe, expect, it } from "vitest";
import type { CommitRow } from "../types";
import { rowSubject } from "./rowSubject";

const row = (kind: CommitRow["kind"], subject: string) => ({ kind, subject }) as CommitRow;

describe("rowSubject", () => {
  it("shows the snapshot's subject on the uncommitted row, not the one it was read with", () => {
    const snapshot = { uncommittedSubject: "Uncommitted changes (4)" };
    expect(rowSubject(row("uncommitted", "Uncommitted changes (1)"), snapshot)).toBe("Uncommitted changes (4)");
  });

  it("shows a commit's own subject", () => {
    const snapshot = { uncommittedSubject: "Uncommitted changes (4)" };
    expect(rowSubject(row("commit", "Fix the parser"), snapshot)).toBe("Fix the parser");
  });
});

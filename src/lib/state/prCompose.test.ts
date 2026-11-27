import { beforeEach, describe, expect, it, vi } from "vitest";
import type { OpOutcome, OpRequest } from "../types";

// Each operation asked for, answered when the test says; each reload of the details.
const ran = vi.hoisted(() => [] as { request: OpRequest; resolve: (o: Partial<OpOutcome>) => void; reject: (e: unknown) => void }[]);
const reloads = vi.hoisted(() => [] as { number: number; resolve: () => void }[]);
vi.mock("./ops.svelte", () => ({
  runOp: vi.fn((_repoId: number, request: OpRequest) => new Promise((resolve, reject) => ran.push({ request, resolve, reject }))),
}));
vi.mock("./prDetails.svelte", () => ({
  loadDetails: vi.fn((_repoId: number, _root: string, number: number) => new Promise<void>((resolve) => reloads.push({ number, resolve }))),
}));

import { repoTarget, sendDraft, setResolved } from "./prCompose.svelte";
import { PrCompose } from "./prDrafts.svelte";

const settle = () => new Promise((r) => setTimeout(r, 0));
const target = repoTarget(1, "/r", 31);

describe("what the Pull request tab writes", () => {
  let c: PrCompose;
  beforeEach(() => {
    ran.length = 0;
    reloads.length = 0;
    c = new PrCompose();
  });

  it("keeps a draft per pull request and thread, the same one each time", () => {
    c.draft(31, "T1").text = "a reply";
    c.draft(31, null).text = "a comment";
    expect(c.draft(31, "T1").text).toBe("a reply");
    expect(c.draft(31, null).text).toBe("a comment");
    expect(c.draft(32, "T1").text).toBe("");
    expect(c.draft(31, "T1")).toBe(c.draft(31, "T1"));
  });

  it("sends a reply as typed, and clears the draft only once the details are loaded again", async () => {
    const d = c.draft(31, "T1");
    d.text = "  Fixed in `abc`.\n";
    const sent = sendDraft(c, target, "T1");
    expect(d.sending).toBe(true);
    expect(ran.map((r) => r.request)).toEqual([{ op: "pr_reply", number: 31, threadId: "T1", body: "  Fixed in `abc`.\n" }]);
    // Asked again while it sends: nothing more is sent.
    void sendDraft(c, target, "T1");
    expect(ran).toHaveLength(1);
    ran[0].resolve({ status: "done", message: "Replied", logFrom: 3 });
    await settle();
    expect(reloads.map((r) => r.number)).toEqual([31]);
    expect(d.text).not.toBe("");
    reloads[0].resolve();
    await sent;
    expect(d.text).toBe("");
    expect(d.sending).toBe(false);
    expect(d.error).toBeNull();
  });

  it("sends a comment on the conversation with pr_comment", async () => {
    c.draft(31, null).text = "@ada thanks";
    const sent = sendDraft(c, target, null);
    expect(ran[0].request).toEqual({ op: "pr_comment", number: 31, body: "@ada thanks" });
    ran[0].resolve({ status: "done", message: "", logFrom: 0 });
    await settle();
    reloads[0].resolve();
    await sent;
    expect(c.draft(31, null).text).toBe("");
  });

  it("never sends a blank draft", async () => {
    for (const text of ["", "   ", "\n\t"]) {
      c.draft(31, "T1").text = text;
      await sendDraft(c, target, "T1");
    }
    expect(ran).toHaveLength(0);
  });

  it("keeps the draft when sending fails, and says why with the log's place", async () => {
    const d = c.draft(31, "T1");
    d.text = "Fixed.";
    const sent = sendDraft(c, target, "T1");
    ran[0].resolve({ status: "failed", message: "gh could not post the reply: rate limited", logFrom: 7 });
    await sent;
    expect(d.text).toBe("Fixed.");
    expect(d.error).toBe("gh could not post the reply: rate limited");
    expect(d.logFrom).toBe(7);
    expect(reloads).toHaveLength(0);
    // An error from the core (a refused request): said the same way.
    const again = sendDraft(c, target, "T1");
    expect(d.error).toBeNull();
    ran[1].reject(new Error("Nothing was sent: the comment is empty.\nmore"));
    await again;
    expect(d.error).toBe("Nothing was sent: the comment is empty.");
    expect(d.text).toBe("Fixed.");
  });

  it("resolves and unresolves a thread, once at a time, and says a failure", async () => {
    const r = c.resolving(31, "T1");
    const done = setResolved(c, target, "T1", true);
    expect(r.pending).toBe(true);
    void setResolved(c, target, "T1", true);
    expect(ran.map((x) => x.request)).toEqual([{ op: "pr_resolve", number: 31, threadId: "T1", resolved: true }]);
    ran[0].resolve({ status: "done", message: "", logFrom: 0 });
    await settle();
    reloads[0].resolve();
    await done;
    expect(r.pending).toBe(false);
    const undone = setResolved(c, target, "T1", false);
    expect(ran[1].request).toEqual({ op: "pr_resolve", number: 31, threadId: "T1", resolved: false });
    ran[1].resolve({ status: "failed", message: "gh could not unresolve the conversation: no", logFrom: 2 });
    await undone;
    expect(r.error).toBe("gh could not unresolve the conversation: no");
  });
});

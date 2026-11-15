import { describe, expect, it, vi } from "vitest";

const view = vi.hoisted(() => ({ snapshot: { op: null as null | { id: number; title: string; queued: number } } }));
vi.mock("svelte", async (importOriginal) => ({ ...(await importOriginal<typeof import("svelte")>()), onDestroy: vi.fn() }));
vi.mock("../../state/repo.svelte", () => ({ findRepoView: vi.fn(() => view) }));
let answer: (message: string | null) => void = () => {};
vi.mock("./run", () => ({ runNamed: vi.fn(() => new Promise((r) => (answer = r))) }));

describe("NamedSubmit.waitingFor", () => {
  it("names only the operation that ran when the request was sent, never the request's own", async () => {
    const { NamedSubmit } = await import("./named.svelte");
    const named = new NamedSubmit(1, () => {});
    view.snapshot.op = { id: 5, title: "Check out fix/typo", queued: 1 };
    const sent = named.submit("x", { op: "create_branch", name: "x", at: "a".repeat(40), checkout: false });
    expect(named.waitingFor).toBe("Check out fix/typo");
    expect(named.waitingFor).toBe("Check out fix/typo");
    // The next one shown is the request's own (the one queued between ran unseen).
    view.snapshot.op = { id: 7, title: "Create branch x", queued: 0 };
    expect(named.waitingFor).toBeNull();
    answer(null);
    await sent;
    expect(named.waitingFor).toBeNull();
  });

  it("names nothing when nothing ran as it was sent", async () => {
    const { NamedSubmit } = await import("./named.svelte");
    const named = new NamedSubmit(1, () => {});
    view.snapshot.op = null;
    const sent = named.submit("x", { op: "create_branch", name: "x", at: "a".repeat(40), checkout: false });
    view.snapshot.op = { id: 8, title: "Create branch x", queued: 0 };
    expect(named.waitingFor).toBeNull();
    answer(null);
    await sent;
  });
});

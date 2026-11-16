import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Component } from "svelte";

let overlays: typeof import("./overlays.svelte");

const Form = { name: "Form" } as unknown as Component<Record<string, unknown>>;
const Confirm = { name: "Confirm" } as unknown as Component<Record<string, unknown>>;

beforeEach(async () => {
  vi.resetModules();
  overlays = await import("./overlays.svelte");
});

describe("dialogs", () => {
  it("a dialog opened while another is open goes on top, and the one below is kept as it is", () => {
    const formProps = { name: "typed so far" };
    const form = overlays.openDialog(Form, formProps);
    const confirm = overlays.openDialog(Confirm, {});
    expect(overlays.overlays.dialogs.map((d) => d.id)).toEqual([form, confirm]);
    expect(overlays.overlays.dialog?.id).toBe(confirm);
    expect(overlays.overlays.dialogs[0].props).toBe(formProps);

    overlays.closeDialog(confirm);
    expect(overlays.overlays.dialog?.id).toBe(form);
    expect(overlays.overlays.dialog?.props).toBe(formProps);
  });

  it("closing one closes only that one, wherever it is; closing it again does nothing", () => {
    const a = overlays.openDialog(Form, {});
    const b = overlays.openDialog(Confirm, {});
    overlays.closeDialog(a);
    expect(overlays.overlays.dialogs.map((d) => d.id)).toEqual([b]);
    overlays.closeDialog(a);
    expect(overlays.overlays.dialogs.map((d) => d.id)).toEqual([b]);
    overlays.closeDialog(b);
    expect(overlays.overlays.dialog).toBeNull();
  });

  it("gives each dialog its own id", () => {
    expect(overlays.openDialog(Form, {})).not.toBe(overlays.openDialog(Form, {}));
  });

  it("a native menu's command is ignored while a dialog is open, as the keys are (Task 10 review minor 9)", () => {
    const action = vi.fn();
    overlays.fromAppMenu(action);
    expect(action).toHaveBeenCalledTimes(1);
    const id = overlays.openDialog(Form, {});
    overlays.fromAppMenu(action);
    expect(action).toHaveBeenCalledTimes(1);
    overlays.closeDialog(id);
    overlays.fromAppMenu(action);
    expect(action).toHaveBeenCalledTimes(2);
  });
});

describe("dialogs loaded on first use (final review: launch)", () => {
  it("a lazy dialog opens at once, with its loader and props, and is shown once loaded", async () => {
    const load = vi.fn(async () => ({ default: Form }));
    const lazy = overlays.lazyDialog(load);
    const props = { name: "x" };
    const id = overlays.openDialog(lazy, props);
    const open = overlays.overlays.dialog!;
    expect(open.id).toBe(id);
    expect(overlays.isShowing(lazy)).toBe(true);
    expect(open.props).toEqual({ load: lazy.load, props });
    expect(load).not.toHaveBeenCalled(); // the host loads it when it renders
    overlays.closeDialog(id);
    expect(overlays.isShowing(lazy)).toBe(false);
  });

  it("once loaded (preloaded after start-up), a lazy dialog opens as itself, with no wait", async () => {
    const lazy = overlays.lazyDialog(async () => ({ default: Confirm }));
    overlays.preloadDialogs();
    await vi.waitFor(() => expect(lazy.loaded()).toBe(Confirm));
    const id = overlays.openDialog(lazy, { a: 1 });
    expect(overlays.overlays.dialog).toMatchObject({ id, component: Confirm, props: { a: 1 } });
    expect(overlays.isShowing(lazy)).toBe(true);
  });
});

describe("the start-up bundle (final review: launch)", () => {
  it("no module imports a dialog or a sheet statically: they load when first opened", () => {
    // Every source module under src/, as text (keys relative to this file).
    const sources = import.meta.glob(["../../**/*.ts", "../../**/*.svelte", "!../../**/*.test.ts"], {
      query: "?raw",
      import: "default",
      eager: true,
    }) as Record<string, string>;
    expect(Object.keys(sources).length).toBeGreaterThan(50);
    const offenders = Object.entries(sources).flatMap(([file, text]) => {
      // What is itself loaded on first use may import the rest of its own code.
      if (file.includes("/components/dialogs/") || /\/(Settings|ShortcutsSheet)\.svelte$/.test(file)) return [];
      const imports = [...text.matchAll(/^\s*import\s+(?!type\b)[^;]*?from\s+"([^"]+)"/gm)].map((m) => m[1]);
      return imports
        .filter((spec) => /\/dialogs\/[^/]+\.svelte$|\/(Settings|ShortcutsSheet)\.svelte$/.test(spec))
        .map((spec) => `${file}: ${spec}`);
    });
    expect(offenders).toEqual([]);
  });
});

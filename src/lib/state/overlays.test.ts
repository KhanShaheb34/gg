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

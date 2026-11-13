// The Tauri transport, with Tauri's modules replaced: every failure is a CoreError.
import { describe, expect, it, vi } from "vitest";

vi.mock("./platform", () => ({ isTauri: () => true }));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (_cmd: string, { cmd }: { cmd: string }) => {
    if (cmd === "get_app_state") throw { code: "io", message: "core says no", detail: "d" };
    throw new Error("IPC is down");
  }),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(async () => {
    throw "the dialog plugin is not allowed";
  }),
}));

const close = vi.fn(async () => {});
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ close }) }));

const { api, CoreError } = await import("./ipc");

describe("Tauri transport", () => {
  it("passes the core's errors on as they are", async () => {
    const error = await api.getAppState().catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreError);
    expect(error).toMatchObject({ code: "io", message: "core says no", detail: "d" });
  });

  it("turns any other failure of a call into an io CoreError", async () => {
    const error = await api.refresh(1).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreError);
    expect(error).toMatchObject({ code: "io" });
    expect((error as InstanceType<typeof CoreError>).detail).toContain("IPC is down");
  });

  it("turns a failing folder picker into an io CoreError", async () => {
    const error = await api.pickFolder().catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreError);
    expect(error).toMatchObject({ code: "io", message: "Could not show the folder picker" });
    expect((error as InstanceType<typeof CoreError>).detail).toContain("not allowed");
  });

  it("closes the window", async () => {
    await api.closeWindow();
    expect(close).toHaveBeenCalledTimes(1);
  });
});

import { describe, expect, it } from "vitest";
import { trackWindowFocus } from "./windowFocus";

/** Window and document stand-ins whose focus and visibility the test sets. */
function fakeEnv() {
  const listeners = new Map<string, Set<() => void>>();
  const on = (name: string, fn: () => void) => {
    if (!listeners.has(name)) listeners.set(name, new Set());
    listeners.get(name)!.add(fn);
  };
  const off = (name: string, fn: () => void) => listeners.get(name)?.delete(fn);
  const env = {
    focused: true,
    visible: true,
    fire(name: string) {
      for (const fn of [...(listeners.get(name) ?? [])]) fn();
    },
    count: () => [...listeners.values()].reduce((n, s) => n + s.size, 0),
    win: { addEventListener: on, removeEventListener: off },
    doc: {
      addEventListener: on,
      removeEventListener: off,
      hasFocus: () => env.focused,
      get visibilityState() {
        return env.visible ? "visible" : "hidden";
      },
    },
  };
  return env;
}

describe("trackWindowFocus", () => {
  it("reports the initial state, then each change once", () => {
    const env = fakeEnv();
    const seen: boolean[] = [];
    const stop = trackWindowFocus(env.win, env.doc, (f) => seen.push(f));
    expect(seen).toEqual([true]);

    env.focused = false;
    env.fire("blur");
    env.fire("blur");
    env.visible = false;
    env.fire("visibilitychange");
    expect(seen).toEqual([true, false]);

    env.focused = true;
    env.fire("focus");
    expect(seen).toEqual([true, false]); // still hidden

    env.visible = true;
    env.fire("visibilitychange");
    env.fire("focus");
    expect(seen).toEqual([true, false, true]);

    stop();
    expect(env.count()).toBe(0);
  });
});

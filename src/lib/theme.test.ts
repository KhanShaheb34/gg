import { describe, expect, it } from "vitest";
import { nextTheme, resolveTheme } from "./theme";

describe("resolveTheme", () => {
  it("uses the setting unless it is system, which follows the OS", () => {
    expect(resolveTheme("light", true)).toBe("light");
    expect(resolveTheme("dark", false)).toBe("dark");
    expect(resolveTheme("system", true)).toBe("dark");
    expect(resolveTheme("system", false)).toBe("light");
  });

  it("treats an unknown cached value as system", () => {
    expect(resolveTheme("purple", true)).toBe("dark");
    expect(resolveTheme(null, false)).toBe("light");
  });
});

describe("nextTheme", () => {
  it("cycles system, light, dark", () => {
    expect(nextTheme("system")).toBe("light");
    expect(nextTheme("light")).toBe("dark");
    expect(nextTheme("dark")).toBe("system");
  });
});

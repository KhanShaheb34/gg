import { describe, expect, it } from "vitest";
import { shortHash } from "./format";

describe("shortHash", () => {
  it("returns the first 8 characters of a hash", () => {
    expect(shortHash("471663edabcdef")).toBe("471663ed");
  });
});

import { describe, expect, it } from "vitest";
import { versionNewer } from "./version";

describe("versionNewer", () => {
  it("compara número a número, não como texto", () => {
    expect(versionNewer("0.15.0", "0.14.2")).toBe(true);
    expect(versionNewer("0.10.0", "0.9.9")).toBe(true);
    expect(versionNewer("0.9.9", "0.10.0")).toBe(false);
    expect(versionNewer("1.0.0", "1.0.0")).toBe(false);
  });

  it("ignora o v e o sufixo", () => {
    expect(versionNewer("v0.15.0", "0.14.0")).toBe(true);
    expect(versionNewer("0.15.0-rc.1", "0.15.0")).toBe(false);
    expect(versionNewer("0.15", "0.14.9")).toBe(true);
  });
});

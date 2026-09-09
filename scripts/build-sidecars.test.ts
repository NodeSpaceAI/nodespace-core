// Covers the profile-flag logic in scripts/build-sidecars.ts. The script's
// own build (cargo build + cp + chmod) is behind `import.meta.main`, so
// importing it here runs nothing -- this exercises only the pure flag-array
// logic, not a real multi-minute cargo build.
import { describe, expect, test } from "bun:test";
import { cargoProfileFlags } from "./build-sidecars";

describe("cargoProfileFlags", () => {
  test("debug contributes zero flags", () => {
    // The regression this guards: a bare '--debug' -> '' string interpolated
    // into Bun's `$` template becomes a literal empty argument, which cargo
    // rejects ("unexpected argument ''"). An empty array must contribute
    // zero arguments instead.
    expect(cargoProfileFlags(true)).toEqual([]);
  });

  test("release contributes exactly --release", () => {
    expect(cargoProfileFlags(false)).toEqual(["--release"]);
  });
});

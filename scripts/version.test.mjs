import assert from "node:assert/strict";
import test from "node:test";
import { normalizeVersion } from "./version.mjs";

test("accepts different valid SemVer prerelease labels without changing them", () => {
  for (const version of [
    "0.1.2",
    "0.1.2-beta",
    "0.1.2-alpha.1",
    "0.1.2-anything",
    "0.1.2-oneneko",
    "0.1.2-custom-label.2",
  ]) {
    assert.equal(normalizeVersion(version), version);
  }
});

test("rejects malformed or incomplete versions", () => {
  for (const version of ["", "v0.1.2", "01.2.3", "0.1", "0.1.2-01"]) {
    assert.throws(() => normalizeVersion(version), /valid SemVer/);
  }
});

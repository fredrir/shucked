import assert from "node:assert/strict";
import { test } from "node:test";
import { load } from "./support/load.mjs";

const { launchDirectoryLabel } = await load("environment.ts");

test("startup status never presents post-startup terminal cwd as entry context", () => {
  assert.match(launchDirectoryLabel(true, { sessionId: "terminal", cwd: "/configured" }, "/after-startup"), /unknown at startup/);
  assert.equal(launchDirectoryLabel(false, { sessionId: "terminal" }, "/live"), "/live");
  assert.equal(launchDirectoryLabel(false, { cwd: "/configured" }), "/configured");
});

test("an assumed workspace directory is labelled as assumed", () => {
  assert.equal(launchDirectoryLabel(false, {}, undefined, "/workspace"), "/workspace (assumed)");
  assert.equal(launchDirectoryLabel(false, { cwd: "/explicit" }, undefined, "/workspace"), "/explicit");
  assert.equal(launchDirectoryLabel(false, {}), "unknown workspace");
  assert.equal(launchDirectoryLabel(true, { cwd: "/configured" }), "/configured", "startup files without a session keep their configured directory");
});

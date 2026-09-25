import assert from "node:assert/strict";
import { test } from "node:test";
import { load } from "./support/load.mjs";

const { launchDirectoryLabel, contextLabel, loginShellName } = await load("environment.ts");

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

test("the login shell name follows the setting, then $SHELL, then the platform default", () => {
  assert.equal(loginShellName({ loginShell: "/opt/homebrew/bin/fish" }, { SHELL: "/bin/zsh" }, "darwin"), "fish");
  assert.equal(loginShellName({}, { SHELL: "/usr/local/bin/bash" }, "darwin"), "bash");
  assert.equal(loginShellName({ loginShell: "" }, { SHELL: "/bin/zsh" }, "linux"), "zsh", "an empty setting defers to $SHELL");
  assert.equal(loginShellName({}, {}, "darwin"), "zsh");
  assert.equal(loginShellName({}, {}, "linux"), "bash");
});

test("the status label names each execution context", () => {
  assert.equal(contextLabel({}, undefined, undefined, "zsh"), "Workspace (local)");
  assert.equal(contextLabel({ policy: "workspace" }, undefined, "ssh-remote", "zsh"), "Workspace (ssh-remote)");
  assert.equal(contextLabel({ policy: "login-shell" }, undefined, undefined, "zsh"), "Login shell (zsh)");
  assert.equal(contextLabel({ policy: "portable" }, undefined, undefined, "zsh"), "Portable");
  assert.equal(contextLabel({ targetInventory: "/target.json", policy: "portable" }, undefined, undefined, "zsh"), "Captured");
});

test("terminal contexts show connection state and dropped hook payloads", () => {
  assert.equal(contextLabel({ sessionId: "t" }, undefined, undefined, "zsh"), "Terminal pending");
  assert.equal(contextLabel({ sessionId: "t" }, { connected: true, shell: "fish" }, undefined, "zsh"), "Terminal (fish)");
  assert.equal(contextLabel({ sessionId: "t" }, { connected: false }, undefined, "zsh"), "Terminal disconnected");
  assert.equal(contextLabel({ sessionId: "t" }, { connected: undefined, problem: "size" }, undefined, "zsh"), "Terminal unavailable (size)", "a dropped payload no longer looks like a pending attachment");
  assert.equal(contextLabel({ sessionId: "t", policy: "login-shell" }, { connected: true, shell: "zsh" }, undefined, "zsh"), "Terminal (zsh)", "an attached terminal wins over the policy");
});

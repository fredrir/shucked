import assert from "node:assert/strict";
import { test } from "node:test";
import { load } from "./support/load.mjs";

const { validateSessionMessage } = await load("terminal.ts");
const valid = {
  id: "a".repeat(32), token: "b".repeat(64), generation: 1, pid: 123, shell: "zsh", cwd: process.cwd(),
  path: ["", "/usr/bin"], aliases: { ls: ["eza", "--icons"] }, functions: ["greet"], options: { aliases: "on" },
  private: false, ignore: [], connected: true,
};

test("well-formed shell metadata is accepted", () => {
  assert.equal(validateSessionMessage(valid), true);
  assert.equal(validateSessionMessage({ ...valid, historyFile: "", liveCompletion: true, liveSignal: "SIGUSR2", acceptedHistoryHash: "c".repeat(64) }), true);
});

test("shell metadata rejects malformed and oversized identities", () => {
  for (const patch of [
    { generation: -1 }, { generation: Infinity }, { generation: 0 }, { pid: 0 },
    { id: "A".repeat(32) }, { id: "a".repeat(31) }, { token: "no" },
    { shell: "powershell" }, { cwd: "relative" }, { cwd: "/tmp/\0" },
    { path: [null] }, { functions: Array(16385).fill("f") }, { ignore: Array(33).fill("x") },
    { aliases: { ls: "eza" } }, { aliases: { "bad name": ["x"] } }, { aliases: [] },
    { options: { "bad key": "x" } }, { options: { key: "x".repeat(128) } },
    { acceptedHistoryHash: "short" }, { historyFile: "relative/history" }, { historyFile: 42 },
    { liveCompletion: "yes" }, { liveSignal: "SIGKILL" }, { private: "no" }, { connected: false },
  ]) {
    assert.equal(validateSessionMessage({ ...valid, ...patch }), false, JSON.stringify(patch).slice(0, 80));
  }
  for (const value of [null, undefined, "text", 42, []]) {
    assert.equal(validateSessionMessage(value), false);
  }
});

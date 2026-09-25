import assert from "node:assert/strict";
import { test } from "node:test";
import { load } from "./support/load.mjs";
import { extensionContext, fakeVscode, outputChannel } from "./support/fake-vscode.mjs";

async function commands({ status = "ready", messages = [] } = {}) {
  const vscode = fakeVscode({ messages });
  const { registerCommands } = await load("commands.ts", { vscode });
  const calls = [];
  const client = { restart: async () => calls.push("restart") };
  const output = outputChannel();
  registerCommands(extensionContext(), client, output, { currentStatus: status });
  return { vscode, calls, output };
}

test("all palette commands and the status item command are registered", async () => {
  const { vscode } = await commands();
  assert.deepEqual([...vscode.registered.keys()].sort(), ["shucked.restartServer", "shucked.showOutputChannel", "shucked.showVersion", "shucked.statusClicked"]);
});

test("restart and show-logs commands reach the client and the output channel", async () => {
  const { vscode, calls, output } = await commands();
  await vscode.registered.get("shucked.restartServer")();
  vscode.registered.get("shucked.showOutputChannel")();
  assert.deepEqual(calls, ["restart"]);
  assert.ok(output.lines.some(([kind]) => kind === "show"));
});

test("clicking the status item in an error state offers a restart", async () => {
  const { vscode, calls } = await commands({ status: "error", messages: ["Restart Server"] });
  await vscode.registered.get("shucked.statusClicked")();
  assert.deepEqual(calls, ["restart"]);
  const [, message, ...choices] = vscode.calls.find(([name]) => name === "showErrorMessage");
  assert.match(message, /error state/);
  assert.deepEqual(choices, ["Restart Server", "Show Logs"]);
});

test("clicking the status item in an error state can open the logs instead", async () => {
  const { vscode, calls, output } = await commands({ status: "error", messages: ["Show Logs"] });
  await vscode.registered.get("shucked.statusClicked")();
  assert.deepEqual(calls, []);
  assert.ok(output.lines.some(([kind]) => kind === "show"));
});

test("clicking the status item otherwise opens the logs", async () => {
  const { vscode, output } = await commands({ status: "ready" });
  await vscode.registered.get("shucked.statusClicked")();
  assert.equal(vscode.calls.some(([name]) => name === "showErrorMessage"), false);
  assert.ok(output.lines.some(([kind]) => kind === "show"));
});

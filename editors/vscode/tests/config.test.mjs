import assert from "node:assert/strict";
import { test } from "node:test";
import { load } from "./support/load.mjs";
import { extensionContext, fakeVscode, outputChannel } from "./support/fake-vscode.mjs";

async function watcher() {
  const vscode = fakeVscode();
  const { registerConfigWatcher } = await load("config.ts", { vscode });
  const calls = [];
  const client = { synchronizeConfiguration: async () => calls.push("synchronize"), restart: async () => calls.push("restart") };
  const context = extensionContext();
  registerConfigWatcher(context, client, outputChannel());
  const change = async (...sections) => {
    calls.length = 0;
    await vscode.configurationListener({ affectsConfiguration: section => sections.some(changed => changed === section || changed.startsWith(`${section}.`)) });
    return [...calls];
  };
  return { change, context };
}

test("Shucked settings are sent to the running server without a restart", async () => {
  const { change } = await watcher();
  assert.deepEqual(await change("shucked.lint.enable"), ["synchronize"]);
  assert.deepEqual(await change("shucked.environment.policy"), ["synchronize"]);
});

test("changing the server program or its arguments restarts the server", async () => {
  const { change } = await watcher();
  assert.deepEqual(await change("shucked.server.path"), ["synchronize", "restart"]);
  assert.deepEqual(await change("shucked.server.extraArgs"), ["synchronize", "restart"]);
});

test("other extensions' settings are ignored", async () => {
  const { change, context } = await watcher();
  assert.deepEqual(await change("editor.fontSize"), []);
  assert.equal(context.subscriptions.length, 1);
});

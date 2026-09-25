import assert from "node:assert/strict";
import { test } from "node:test";
import { load } from "./support/load.mjs";
import { fakeVscode } from "./support/fake-vscode.mjs";

async function statusBar() {
  const vscode = fakeVscode();
  const { StatusBarManager } = await load("status.ts", { vscode });
  const manager = new StatusBarManager();
  return { manager, item: vscode.statusItems[0] };
}

test("the status item starts visible, clickable, and starting", async () => {
  const { manager, item } = await statusBar();
  assert.equal(manager.currentStatus, "starting");
  assert.equal(item.visible, true);
  assert.equal(item.command, "shucked.statusClicked");
  assert.match(item.text, /Starting/);
});

test("each state has its own text and default tooltip", async () => {
  const { manager, item } = await statusBar();
  const expected = { starting: /Starting$/, ready: /^\$\(check\) Shucked$/, busy: /Indexing$/, error: /Error$/, stopped: /Stopped$/ };
  for (const [state, text] of Object.entries(expected)) {
    manager.setStatus(state);
    assert.equal(manager.currentStatus, state);
    assert.match(item.text, text, state);
    assert.ok(item.tooltip, `${state} has a tooltip`);
  }
});

test("a detail replaces the default tooltip", async () => {
  const { manager, item } = await statusBar();
  manager.setStatus("error", "binary not found");
  assert.equal(item.tooltip, "binary not found");
  manager.setStatus("ready");
  assert.notEqual(item.tooltip, "binary not found");
});

test("disposing the manager disposes the item", async () => {
  const { manager, item } = await statusBar();
  manager.dispose();
  assert.equal(item.disposed, true);
});

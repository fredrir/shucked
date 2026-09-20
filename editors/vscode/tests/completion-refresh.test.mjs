import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { runInNewContext } from "node:vm";
import { build } from "esbuild";

const compiled = await build({ entryPoints: [fileURLToPath(new URL("../src/completion-refresh.ts", import.meta.url))], bundle: true, platform: "node", format: "cjs", external: ["vscode"], write: false });

function fixture(t) {
  const handlers = new Map();
  const calls = [];
  const event = name => handler => { handlers.set(name, handler); return { dispose() { handlers.delete(name); } }; };
  const position = { line: 0, character: 4, isEqual(other) { return this.line === other.line && this.character === other.character; } };
  const document = { uri: { toString: () => "file:///fixture.sh" }, version: 1 };
  const editor = { document, selection: { active: position, isEmpty: true } };
  const vscode = {
    commands: {
      registerCommand: (name, handler) => event(name)(handler),
      async executeCommand(name, ...args) { calls.push([name, ...args]); },
    },
    workspace: { onDidChangeTextDocument: event("change") },
    window: { activeTextEditor: editor, state: { focused: true }, onDidChangeTextEditorSelection: event("selection"), onDidChangeActiveTextEditor: event("editor"), onDidChangeWindowState: event("focus") },
  };
  const module = { exports: {} };
  const scheduled = [];
  runInNewContext(compiled.outputFiles[0].text, { module, exports: module.exports, require: () => vscode, performance,
    setTimeout: (fn, ms) => { const timer = { fn, ms }; scheduled.push(timer); return timer; },
    clearTimeout: timer => { const index = scheduled.indexOf(timer); if (index >= 0) { scheduled.splice(index, 1); } },
  });
  const refresh = new module.exports.CompletionRefresh({ trace(message) { calls.push(["trace", message]); } });
  t.after(() => refresh.dispose());
  const token = { onCancellationRequested: event("cancel") };
  const ready = (generation = 1) => ({ uri: document.uri.toString(), version: 1, position: { line: 0, character: 4 }, generation });
  const provide = (result = { items: [], isIncomplete: true }) => refresh.provide(document, position, {}, token, async () => result);
  return { refresh, handlers, calls, position, document, editor, vscode, token, ready, provide, scheduled,
    triggers: () => calls.filter(([name]) => name === "editor.action.triggerSuggest").length };
}

test("ready native results refresh an idle completion once per generation", async t => {
  const f = fixture(t);
  await f.provide();
  f.refresh.ready(f.ready());
  await Promise.resolve();
  f.refresh.ready(f.ready());
  assert.equal(f.triggers(), 1);
});

test("a fast native response waits for the initial completion response", async t => {
  const f = fixture(t);
  let resolve;
  const response = new Promise(done => { resolve = done; });
  const pending = f.refresh.provide(f.document, f.position, {}, f.token, () => response);
  f.refresh.ready(f.ready());
  assert.equal(f.triggers(), 0);
  resolve({ items: [], isIncomplete: true });
  await pending;
  f.scheduled.find(timer => timer.ms === 0).fn();
  await Promise.resolve();
  assert.equal(f.triggers(), 1);
});

test("Escape dismisses suggestions and late native results cannot reopen them", async t => {
  const f = fixture(t);
  await f.provide();
  await f.handlers.get("shucked.dismissCompletion")();
  f.refresh.ready(f.ready());
  assert.equal(f.triggers(), 0);
  assert.ok(f.calls.some(([command]) => command === "hideSuggestWidget"));
});

test("Escape during a refresh prevents the replacement popup", async t => {
  const f = fixture(t);
  await f.provide();
  f.refresh.ready(f.ready());
  await f.handlers.get("shucked.dismissCompletion")();
  await Promise.resolve();
  assert.equal(f.triggers(), 0);
});

test("deliberate keyboard navigation retains selection without late refresh", async t => {
  for (const command of ["selectNextSuggestion", "selectPrevSuggestion", "selectNextPageSuggestion", "selectPrevPageSuggestion"]) {
    const f = fixture(t);
    await f.provide();
    await f.handlers.get("shucked.navigateCompletion")(command);
    f.refresh.ready(f.ready());
    assert.equal(f.triggers(), 0);
    assert.ok(f.calls.some(([name]) => name === command));
  }
});

test("moving the cursor, editing, changing editors, or losing focus rejects stale results", async t => {
  for (const change of [
    f => { f.editor.selection.active = { line: 0, character: 3, isEqual: () => false }; f.handlers.get("selection")(); },
    f => { f.document.version++; f.handlers.get("change")({ document: f.document }); },
    f => f.handlers.get("editor")(),
    f => { f.vscode.window.state.focused = false; },
    f => { f.handlers.get("focus")({ focused: false }); f.handlers.get("focus")({ focused: true }); },
  ]) {
    const f = fixture(t);
    await f.provide();
    change(f);
    f.refresh.ready(f.ready());
    assert.equal(f.triggers(), 0);
  }
});

test("complete responses and programmatic queries never request an idle popup", async t => {
  const f = fixture(t);
  await f.provide({ items: [{ label: "local" }], isIncomplete: false });
  f.refresh.ready(f.ready());
  assert.equal(f.triggers(), 0);
  f.editor.selection.active = { isEqual: () => false };
  await f.provide();
  f.refresh.ready(f.ready(2));
  assert.equal(f.triggers(), 0);
});

test("refresh generations are bounded even if every response remains incomplete", async t => {
  const f = fixture(t);
  for (let generation = 1; generation <= 10; generation++) {
    await f.provide();
    f.refresh.ready(f.ready(generation));
    await Promise.resolve();
  }
  assert.equal(f.triggers(), 3);
});

test("environment invalidations have a separate bounded retry budget", async t => {
  const f = fixture(t);
  for (let generation = 1; generation <= 12; generation++) {
    await f.provide();
    f.refresh.ready({ ...f.ready(generation), reason: "environmentChanged", candidateCount: 0 });
    await Promise.resolve();
  }
  assert.equal(f.triggers(), 8);
  await f.provide();
  f.refresh.ready(f.ready(13));
  await Promise.resolve();
  assert.equal(f.triggers(), 9, "invalidation retries preserve the native-result refresh budget");
});

test("expired and cancelled requests do not reopen suggestions", async t => {
  const f = fixture(t);
  await f.provide({ items: [{ label: "local" }], isIncomplete: true });
  f.scheduled.find(timer => timer.ms === 5000).fn();
  f.refresh.ready(f.ready());
  assert.equal(f.triggers(), 0);
  let resolve;
  const response = new Promise(done => { resolve = done; });
  const pending = f.refresh.provide(f.document, f.position, {}, f.token, () => response);
  f.handlers.get("cancel")();
  resolve({ items: [], isIncomplete: true });
  await pending;
  f.refresh.ready(f.ready(2));
  assert.equal(f.triggers(), 0);
});

test("cold empty responses allow delayed workspace results but expire without extending on retries", async t => {
  const f = fixture(t);
  await f.provide();
  assert.equal(f.scheduled.some(timer => timer.ms === 5000), false);
  const expiry = f.scheduled.find(timer => timer.ms === 30000);
  assert.ok(expiry);
  f.refresh.ready({ ...f.ready(), reason: "analysisReady" });
  await Promise.resolve();
  assert.equal(f.triggers(), 1);
  await f.provide();
  assert.equal(f.scheduled.find(timer => timer.ms === 30000), expiry, "incomplete retries retain the original deadline");
  expiry.fn();
  f.refresh.ready(f.ready(2));
  assert.equal(f.triggers(), 1);
});

test("Escape interception is scoped to pending editor completion", async () => {
  const manifest = JSON.parse(await readFile(new URL("../package.json", import.meta.url), "utf8"));
  const binding = manifest.contributes.keybindings.find(binding => binding.command === "shucked.dismissCompletion");
  assert.equal(binding.key, "escape");
  assert.equal(binding.when, "editorTextFocus && shucked.completionPending");
});

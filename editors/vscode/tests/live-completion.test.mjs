import assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { bundle, evaluate } from "./support/load.mjs";

const compiled = await bundle("live-completion.ts");

function load(vscode = {}, overrides = {}) {
  const { process: environment = process, ...modules } = overrides;
  return evaluate(compiled, { vscode, modules, globals: { process: environment } });
}

test('live request limits are inclusive and apply to every field', () => {
  const { validLiveParams } = load();
  const valid = { uri: 'file:///fixture.zsh', version: 1, sessionId: 'b'.repeat(32), generation: 1, dialect: 'zsh', words: ['custom'], prefix: 'live' };
  assert.equal(validLiveParams({ ...valid, prefix: 'a'.repeat(8192) }), true);
  assert.equal(validLiveParams({ ...valid, words: Array(256).fill('w') }), true);
  assert.equal(validLiveParams({ ...valid, words: Array(257).fill('w') }), false);
  assert.equal(validLiveParams({ ...valid, words: Array(4).fill('w'.repeat(7600)) }), false, 'the combined request is bounded');
  assert.equal(validLiveParams({ ...valid, prefix: 'é'.repeat(4097) }), false, 'limits count bytes, not characters');
  for (const patch of [{ sessionId: 'B'.repeat(32) }, { version: 1.5 }, { uri: 42 }, { words: 'custom' }, { words: [1] }]) {
    assert.equal(validLiveParams({ ...valid, ...patch }), false, JSON.stringify(patch));
  }
  for (const value of [null, undefined, 'request', 7]) { assert.equal(validLiveParams(value), false); }
});

test('live request rejects oversized and executable-data-invalid inputs', () => {
  const { validLiveParams } = load();
  const valid = { uri: 'file:///fixture.zsh', version: 1, sessionId: 'b'.repeat(32), generation: 1, dialect: 'zsh', words: ['custom'], prefix: 'live' };
  assert.equal(validLiveParams(valid), true);
  for (const patch of [{ prefix: 'a'.repeat(8193) }, { words: ['a'.repeat(8193)] }, { prefix: 'x\0y' }, { words: [] }, { generation: 0 }, { dialect: 'powershell' }]) {
    assert.equal(validLiveParams({ ...valid, ...patch }), false);
  }
});
test('cancellation during delayed request creation removes the eventual file and never signals the shell', async () => {
  const directory = await fs.mkdtemp(join(tmpdir(), 'shucked-live-race-'));
  let releaseWrite, cancel, request, cleanupDone;
  let written = false, signals = 0;
  const cleaned = new Promise(resolve => { cleanupDone = resolve; });
  const writeGate = new Promise(resolve => { releaseWrite = resolve; });
  const document = { uri: { toString: () => 'file:///fixture.zsh' }, version: 1, fileName: '/fixture.zsh' };
  const id = 'b'.repeat(32);
  const { LiveCompletionManager } = load({ workspace: { isTrusted: true, textDocuments: [document] } }, {
    process: { ...process, kill: () => { signals++; } },
    'node:fs/promises': { ...fs,
      writeFile: async (...args) => { await writeGate; await fs.writeFile(...args); written = true; },
      rm: async (...args) => { await fs.rm(...args); if (written) { cleanupDone(); } },
    },
  });
  // Signals are observed without touching a real process.
  const session = { id, token: 'c'.repeat(64), generation: 1, shell: 'zsh', pid: 2147483647, directory, metadata: { liveCompletion: true, liveSignal: 'SIGUSR1' } };
  const manager = new LiveCompletionManager({ onRequest: (_method, handler) => { request = handler; return { dispose() { return undefined; } }; } }, { selection: () => ({ sessionId: id, policy: 'workspace' }) }, () => session);
  try {
    const response = request({ uri: document.uri.toString(), version: 1, sessionId: id, generation: 1, dialect: 'zsh', words: ['custom'], prefix: 'live' }, { isCancellationRequested: false, onCancellationRequested: callback => { cancel = callback; return { dispose() { return undefined; } }; } });
    cancel();
    assert.equal((await response).reason, 'Completion cancelled');
    releaseWrite();
    await cleaned;
    assert.equal(signals, 0);
    assert.deepEqual(await fs.readdir(directory), []);
  } finally { releaseWrite(); manager.dispose(); await fs.rm(directory, { recursive: true, force: true }); }
});

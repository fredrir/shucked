import assert from "node:assert/strict";
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

test('a helper greeting names a session, a shell, both processes and a reserved signal', () => {
  const { validLiveHelperHello, LIVE_SIGNALS } = load();
  const hello = { kind: 'liveHelper', phase: 'hello', id: 'b'.repeat(32), token: 'c'.repeat(64), shell: 'bash', pid: 4242, shellPid: 4200, signal: 'SIGWINCH' };
  assert.equal(validLiveHelperHello(hello), true);
  assert.deepEqual([...LIVE_SIGNALS], ['SIGUSR1', 'SIGUSR2', 'SIGWINCH']);
  for (const patch of [{ kind: 'liveCompletion' }, { phase: 'result' }, { id: 'short' }, { token: 'x' }, { shell: 'powershell' }, { pid: 1 }, { shellPid: 0 }, { signal: 'SIGKILL' }, { signal: 'SIGALRM' }]) {
    assert.equal(validLiveHelperHello({ ...hello, ...patch }), false, JSON.stringify(patch));
  }
});

// A fake helper connection: records frames the manager writes and lets a test
// inject frames the helper would send.
function fakeSocket() {
  const written = [];
  const listeners = new Map();
  const socket = {
    destroyed: false, timeouts: [],
    write(data) { written.push(JSON.parse(String(data))); },
    setTimeout(value) { socket.timeouts.push(value); },
    removeAllListeners(event) { listeners.delete(event); },
    on(event, listener) { listeners.set(event, listener); return socket; },
    end() { socket.ended = true; },
    destroy() { socket.destroyed = true; listeners.get('close')?.(); },
    emit(event, ...args) { listeners.get(event)?.(...args); },
    written,
  };
  return socket;
}

function manager(overrides = {}) {
  const document = { uri: { toString: () => 'file:///fixture.sh' }, version: 1, fileName: '/fixture.sh' };
  const id = 'b'.repeat(32);
  const session = { id, token: 'c'.repeat(64), generation: 3, shell: 'bash', pid: 4200, directory: '/private', metadata: { liveCompletion: true, liveSignal: 'SIGWINCH' }, ...overrides.session };
  const logged = [];
  const output = { debug: message => logged.push(message) };
  const { LiveCompletionManager } = load({ workspace: { isTrusted: true, textDocuments: [document] } }, overrides.modules ?? {});
  let request;
  const instance = new LiveCompletionManager({ onRequest: (_method, handler) => { request = handler; return { dispose() { return undefined; } }; } }, { selection: () => ({ sessionId: id, policy: 'workspace' }) }, wanted => wanted === id ? session : undefined, output);
  const cancellation = { isCancellationRequested: false, onCancellationRequested: callback => { cancellation.cancel = callback; return { dispose() { return undefined; } }; } };
  const params = { uri: document.uri.toString(), version: 1, sessionId: id, generation: 3, dialect: 'bash', words: ['custom'], prefix: 'live' };
  const hello = { kind: 'liveHelper', phase: 'hello', id, token: session.token, shell: 'bash', pid: 4242, shellPid: 4200, signal: 'SIGWINCH' };
  return { instance, request: () => request(params, cancellation), cancellation, params, session, hello, logged };
}

test('without a connected helper a request is declined immediately and nothing is signalled', async () => {
  const { instance, request } = manager();
  try {
    const reply = await request();
    assert.equal(reply.reason, 'The live completion helper is not connected');
    assert.equal(reply.candidates.length, 0);
  } finally { instance.dispose(); }
});

test('adopting a helper connection requires the session token, shell and shell process', () => {
  const { instance, hello } = manager();
  try {
    for (const patch of [{ token: 'd'.repeat(64) }, { shell: 'zsh' }, { shellPid: 4201 }, { id: 'a'.repeat(32) }]) {
      const socket = fakeSocket();
      assert.equal(instance.adopt({ ...hello, ...patch }, socket, Buffer.alloc(0)), false, JSON.stringify(patch));
      assert.equal(socket.timeouts.length, 0, 'a rejected connection is left to the caller');
    }
    const socket = fakeSocket();
    assert.equal(instance.adopt(hello, socket, Buffer.alloc(0)), true);
    assert.deepEqual(socket.timeouts, [0], 'the persistent connection has no idle timeout');
    assert.equal(instance.helperConnected(hello.id), true);
  } finally { instance.dispose(); }
});

test('a request is one frame to the helper and its result frame answers it with timing', async () => {
  const { instance, request, hello, params, session, logged } = manager();
  const socket = fakeSocket();
  try {
    assert.equal(instance.adopt(hello, socket, Buffer.alloc(0)), true);
    const pending = request();
    await new Promise(resolve => setImmediate(resolve));
    const [frame] = socket.written;
    assert.equal(frame.kind, 'request');
    assert.match(frame.query, /^[a-f0-9]{32}$/);
    assert.deepEqual([frame.generation, frame.prefix, frame.words], [params.generation, params.prefix, params.words]);
    const result = { kind: 'liveCompletion', phase: 'result', token: session.token, id: session.id, query: frame.query, generation: 3, pid: 0, candidates: [{ text: 'live_first', description: '' }, { text: 'file\\ ', description: 'd', encoding: 'bashWord' }], partial: false, elapsedMs: 7 };
    // Frames may arrive split across chunks.
    const bytes = Buffer.from(JSON.stringify(result) + '\n');
    socket.emit('data', bytes.subarray(0, 20));
    socket.emit('data', bytes.subarray(20));
    const reply = await pending;
    assert.equal(JSON.stringify(reply), JSON.stringify({ candidates: result.candidates, partial: false }));
    assert.equal(socket.written.length, 1, 'an answered request is not cancelled');
    assert.match(logged.at(-1), /^Live completion \(bash\): \d+ms round trip, 7ms in the shell, 2 candidates$/);
  } finally { instance.dispose(); }
});

test('a result with the wrong token, session or generation is ignored and the deadline cancels the request', async t => {
  const { instance, request, hello, session } = manager();
  const socket = fakeSocket();
  t.after(() => instance.dispose());
  assert.equal(instance.adopt(hello, socket, Buffer.alloc(0)), true);
  const pending = request();
  await new Promise(resolve => setImmediate(resolve));
  const { query } = socket.written[0];
  const result = { kind: 'liveCompletion', phase: 'result', token: session.token, id: session.id, query, generation: 3, pid: 0, candidates: [{ text: 'x', description: '' }], partial: false };
  for (const patch of [{ token: 'd'.repeat(64) }, { id: 'a'.repeat(32) }, { generation: 2 }, { candidates: [{ text: 'x\0y', description: '' }] }, { candidates: Array(2001).fill({ text: 'x', description: '' }) }]) {
    socket.emit('data', Buffer.from(JSON.stringify({ ...result, ...patch }) + '\n'));
  }
  const reply = await pending;
  assert.equal(reply.reason, 'Live completion timed out');
  assert.deepEqual(socket.written.at(-1), { kind: 'cancel', query }, 'the helper is told to stop the worker');
});

test('a newer request supersedes the running one and cancellation reaches the helper', async () => {
  const { instance, request, hello, cancellation } = manager();
  const socket = fakeSocket();
  try {
    assert.equal(instance.adopt(hello, socket, Buffer.alloc(0)), true);
    const first = request();
    await new Promise(resolve => setImmediate(resolve));
    const second = request();
    await new Promise(resolve => setImmediate(resolve));
    assert.equal((await first).reason, 'Superseded completion');
    assert.deepEqual(socket.written.map(frame => frame.kind), ['request', 'cancel', 'request']);
    cancellation.cancel();
    assert.equal((await second).reason, 'Completion cancelled');
    assert.deepEqual(socket.written.map(frame => frame.kind), ['request', 'cancel', 'request', 'cancel']);
  } finally { instance.dispose(); }
});

test('losing the helper connection fails pending requests and a replacement helper stops the old one', async () => {
  const { instance, request, hello } = manager();
  const first = fakeSocket();
  try {
    assert.equal(instance.adopt(hello, first, Buffer.alloc(0)), true);
    const pending = request();
    await new Promise(resolve => setImmediate(resolve));
    first.emit('close');
    assert.equal((await pending).reason, 'The live completion helper disconnected');
    assert.equal(instance.helperConnected(hello.id), false);
    const second = fakeSocket(), third = fakeSocket();
    assert.equal(instance.adopt(hello, second, Buffer.alloc(0)), true);
    assert.equal(instance.adopt({ ...hello, pid: 4243 }, third, Buffer.alloc(0)), true);
    assert.deepEqual(second.written, [{ kind: 'stop' }]);
    assert.equal(second.ended, true);
    instance.stopHelper(hello.id);
    assert.deepEqual(third.written, [{ kind: 'stop' }]);
    assert.equal(instance.helperConnected(hello.id), false);
  } finally { instance.dispose(); }
});

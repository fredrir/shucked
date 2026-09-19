/* eslint @typescript-eslint/no-require-imports: "off" -- Ephemeral Fish state transfer supervisor. */
'use strict';
const { spawn } = require('node:child_process');
const net = require('node:net');
const [query, generation, shell, prefix, ...words] = process.argv.slice(2);
const quote = text => `'${text.replaceAll('\\', '\\\\').replaceAll("'", "\\'")}'`;
let child, finished = false, size = 0, outputSize = 0, statePartial = false;
const state = [], output = [];
const timer = setTimeout(() => finish(true, 'Live Fish completion exceeded its time limit'), 1100);
function finish(partial, reason) {
  if (finished) { return; } finished = true; clearTimeout(timer);
  if (child?.pid) { try { process.kill(-child.pid, 'SIGKILL'); } catch { /* Worker already exited. */ } }
  const lines = Buffer.concat(output).toString('utf8').split('\n').filter(Boolean);
  if (statePartial || lines.length > 2000) { partial = true; reason ??= 'Live Fish completion exceeded its state or candidate limit'; }
  const candidates = lines.slice(0, 2000).map(line => {
    const tab = line.indexOf('\t'); return { text: tab < 0 ? line : line.slice(0, tab), description: tab < 0 ? '' : line.slice(tab + 1) };
  }).filter(item => item.text.length <= 8192 && item.description.length <= 16384);
  const message = { kind: 'liveCompletion', phase: 'result', token: process.env.SHUCKED_SESSION_TOKEN, id: process.env.SHUCKED_SESSION_ID, query, generation: Number(generation), pid: 0, candidates, partial, reason };
  const socket = net.createConnection(process.env.SHUCKED_SESSION_SOCKET);
  const stop = setTimeout(() => process.exit(0), 200);
  socket.on('error', () => process.exit(0));
  socket.on('connect', () => socket.end(JSON.stringify(message) + '\n'));
  socket.on('close', () => { clearTimeout(stop); process.exit(0); });
}
process.on('SIGTERM', () => finish(true, 'Live completion cancelled'));
process.stdin.on('data', chunk => {
  size += chunk.length;
  if (size > 1024 * 1024) { process.stdin.destroy(); finish(true, 'Live Fish state exceeds the transfer limit'); return; }
  state.push(chunk);
});
process.stdin.on('end', () => {
  if (finished) { return; }
  // Trusted definitions and escaped values travel through a private OS pipe only.
  // Each editor word is quoted before complete sees it; no editor text is sourced.
  const line = [...words, prefix].map(quote).join(' ');
  statePartial = Buffer.concat(state).includes(Buffer.from('# __shucked_live_state_truncated\n'));
  const program = Buffer.concat([...state, Buffer.from(`\ncomplete -C ${quote(line)}\n`)]);
  // Older Fish rejects Node's socket-backed stdin. A fixed cat pipeline supplies
  // an ordinary OS pipe without writing definitions to disk or evaluating words.
  child = spawn('/bin/sh', ['-c', '/bin/cat | "$1" --no-config --private', 'shucked-live-fish', shell], { detached: true, stdio: ['pipe', 'pipe', 'ignore'] });
  child.on('error', () => finish(true, 'Live Fish worker unavailable'));
  child.stdin.on('error', () => undefined);
  child.stdin.end(program);
  child.stdout.on('data', chunk => {
    outputSize += chunk.length;
    if (outputSize > 192 * 1024) { finish(true, 'Live completion output limit reached'); return; }
    output.push(chunk);
  });
  child.on('exit', code => finish(code !== 0, code === 0 ? undefined : 'Live Fish state could not be reconstructed'));
});

/* eslint @typescript-eslint/no-require-imports: "off" -- Standalone CommonJS helper for the extension host runtime. */
'use strict';
// One persistent process per attached terminal, started by the shell hook.
//
// The extension used to reach an attached shell by writing a request file,
// signalling the shell, and letting the trap spawn Node (VS Code's Electron
// binary) several times per request: once to read the file, once as a
// watchdog, and once or twice to report back over the session socket. Each
// start cost hundreds of milliseconds out of a 1.3 s budget.
//
// This helper replaces those spawns. It keeps one connection to the
// extension's session socket, writes each request into the private session
// directory, signals its parent shell, and reads the worker's records from a
// FIFO it owns. The shell side is builtins only: read the request, fork a
// worker with the current completion state, print records into the FIFO.
//
// Record stream (NUL-delimited fields written by the worker):
//   R <query> <pids>       the worker for <query> started; <pids> is a comma
//                          separated list of process groups to stop
//   M <text> <description> a candidate (readline quotes it itself)
//   B <text> <description> a candidate in shell-word encoding (bash)
//   P <reason>             the result is partial
//   E                      end of the result
//   F <program>            (fish) definitions plus the completion command; the
//                          helper runs them in a private fish and reads lines
//
// Lifetime: the helper exits when its shell exits (its parent changes or the
// stdin pipe the shell holds closes), when the extension tells it to stop, or
// when the session socket stays unreachable.
const fs = require('node:fs');
const net = require('node:net');
const path = require('node:path');
const { spawn, execFileSync } = require('node:child_process');

const LIMITS = { candidates: 2000, text: 8192, description: 16384, payload: 192 * 1024, program: 1024 * 1024, words: 256, word: 8192, request: 30000, frame: 1024 * 1024 };
// A request that has not produced its result by then is stopped and reported
// as partial; the extension's own budget is 1300 ms.
const DEADLINE = 1250;
const SIGNALS = ['SIGUSR1', 'SIGUSR2', 'SIGWINCH'];
const SHELLS = ['bash', 'zsh', 'fish'];

const [shell, shellPidText, signal, fishPath] = process.argv.slice(2);
const shellPid = Number(shellPidText);
const directory = process.env.SHUCKED_LIVE_DIRECTORY;
const socketPath = process.env.SHUCKED_SESSION_SOCKET;
const token = process.env.SHUCKED_SESSION_TOKEN;
const id = process.env.SHUCKED_SESSION_ID;
if (!SHELLS.includes(shell) || !Number.isSafeInteger(shellPid) || shellPid <= 1 || shellPid !== process.ppid
  || !SIGNALS.includes(signal) || typeof directory !== 'string' || !path.isAbsolute(directory)
  || typeof socketPath !== 'string' || !socketPath || typeof token !== 'string' || typeof id !== 'string'
  || (shell === 'fish' && (typeof fishPath !== 'string' || !path.isAbsolute(fishPath)))) {
  process.exit(1);
}

const fifoPath = path.join(directory, 'live.fifo');
const requestPath = path.join(directory, 'request');
const jobs = new Map();
let stopping = false;
let connection = null;
let inbound = Buffer.alloc(0);
let failures = 0;
let reconnectDelay = 500;
let stream = Buffer.alloc(0);
let mode = 'idle';
let active = null;

function exit(code = 0) {
  if (stopping) { return; }
  stopping = true;
  for (const job of [...jobs.values()]) { finish(job, true, 'Live completion stopped', true); }
  for (const file of [fifoPath, requestPath]) { try { fs.unlinkSync(file); } catch { /* Already gone. */ } }
  if (connection) { try { connection.end(); } catch { /* Closing anyway. */ } }
  process.exit(code);
}

// --- FIFO: worker records -------------------------------------------------
try { fs.unlinkSync(fifoPath); } catch { /* No stale FIFO. */ }
let created = false;
for (const mkfifo of ['/usr/bin/mkfifo', '/bin/mkfifo']) {
  try { execFileSync(mkfifo, ['-m', '600', fifoPath], { stdio: 'ignore' }); created = true; break; } catch { /* Try the next location. */ }
}
if (!created) { process.exit(1); }
// Opening both ends keeps the FIFO alive between workers: writers never block
// or fail for want of a reader, and the helper never sees an early EOF.
const fifo = fs.openSync(fifoPath, 'r+');
let reader;
try { reader = new net.Socket({ fd: fifo, readable: true, writable: false }); }
catch { reader = fs.createReadStream(null, { fd: fifo, autoClose: false }); }
reader.on('data', chunk => { stream = stream.length ? Buffer.concat([stream, chunk]) : chunk; drain(); });
reader.on('error', () => undefined);

function field() {
  const at = stream.indexOf(0);
  if (at < 0) { return undefined; }
  const value = stream.subarray(0, at).toString('utf8');
  stream = stream.subarray(at + 1);
  return value;
}
function pids(text) {
  return text.split(',').map(Number).filter(pid => Number.isSafeInteger(pid) && pid > 1 && pid !== shellPid && pid !== process.pid);
}
function drain() {
  for (;;) {
    if (mode === 'idle') {
      // Resynchronise on the header of a live request; a stopped worker may
      // have left a partial record behind, and only the random query names it.
      let best = -1, job = null;
      for (const candidate of jobs.values()) {
        if (candidate.finished || candidate.headerSeen) { continue; }
        const at = stream.indexOf(Buffer.from(`R\0${candidate.query}\0`, 'latin1'));
        if (at >= 0 && (best < 0 || at < best)) { best = at; job = candidate; }
      }
      if (!job) {
        // A worker nobody waits for any more (its request expired before the
        // shell served it) still announces itself; stop it and its children.
        const stale = /R\0([0-9a-f]{32})\0([0-9,]{1,256})\0/.exec(stream.toString('latin1'));
        if (stale && !jobs.has(stale[1])) { kill(pids(stale[2])); stream = stream.subarray(stale.index + stale[0].length); continue; }
        if (stream.length > 512) { stream = stream.subarray(stream.length - 512); }
        return;
      }
      stream = stream.subarray(best + 35);
      active = job; job.headerSeen = true; mode = 'pids';
    }
    if (mode === 'pids') {
      const value = field();
      if (value === undefined) { return; }
      active.pids = pids(value);
      if (active.cancelled) { const job = active; mode = 'idle'; active = null; kill(job.pids); continue; }
      mode = shell === 'fish' ? 'program-tag' : 'records';
      continue;
    }
    if (mode === 'records') {
      const first = stream.indexOf(0);
      if (first < 0) { return; }
      const tag = stream.subarray(0, first).toString('utf8');
      if (tag === 'E') { stream = stream.subarray(first + 1); const job = active; mode = 'idle'; active = null; finish(job, job.partial, job.reason); continue; }
      if (tag === 'P') {
        const second = stream.indexOf(0, first + 1);
        if (second < 0) { return; }
        active.partial = true; active.reason = stream.subarray(first + 1, second).toString('utf8').slice(0, 512);
        stream = stream.subarray(second + 1); continue;
      }
      if (tag === 'M' || tag === 'B') {
        const second = stream.indexOf(0, first + 1);
        if (second < 0) { return; }
        const third = stream.indexOf(0, second + 1);
        if (third < 0) { return; }
        addCandidate(active, stream.subarray(first + 1, second).toString('utf8'), stream.subarray(second + 1, third).toString('utf8'), tag === 'B');
        stream = stream.subarray(third + 1); continue;
      }
      // Out of sync: the worker was stopped mid-record. Report what arrived.
      stream = stream.subarray(first + 1);
      const job = active; mode = 'idle'; active = null;
      finish(job, true, 'Live completion output was interrupted'); continue;
    }
    if (mode === 'program-tag') {
      const tag = field();
      if (tag === undefined) { return; }
      if (tag !== 'F') { const job = active; mode = 'idle'; active = null; finish(job, true, 'Live completion output was interrupted'); continue; }
      mode = 'program'; continue;
    }
    if (mode === 'program') {
      const at = stream.indexOf(0);
      const piece = at < 0 ? stream : stream.subarray(0, at);
      if (piece.length) {
        active.programSize += piece.length;
        if (active.programSize <= LIMITS.program) { active.program.push(Buffer.from(piece)); }
        else { active.partial = true; active.reason ??= 'Live Fish state exceeds the transfer limit'; }
      }
      stream = at < 0 ? Buffer.alloc(0) : stream.subarray(at + 1);
      if (at < 0) { return; }
      mode = 'program-end'; continue;
    }
    if (mode === 'program-end') {
      const tag = field();
      if (tag === undefined) { return; }
      const job = active; mode = 'idle'; active = null;
      if (tag === 'E') { runFish(job); } else { finish(job, true, 'Live completion output was interrupted'); }
      continue;
    }
    return;
  }
}

function addCandidate(job, text, description, bashWord) {
  if (job.finished) { return; }
  if (text.length > LIMITS.text || description.length > LIMITS.description || job.candidates.length >= LIMITS.candidates) {
    job.partial = true; job.reason ??= 'Live candidate limit reached'; return;
  }
  job.size += Buffer.byteLength(text) + Buffer.byteLength(description);
  if (job.size > LIMITS.payload) { job.partial = true; job.reason ??= 'Live completion output limit reached'; return; }
  job.candidates.push(bashWord ? { text, description, encoding: 'bashWord' } : { text, description });
}

function runFish(job) {
  if (job.finished) { return; }
  const program = Buffer.concat(job.program);
  if (program.includes('# __shucked_live_state_truncated\n')) { job.partial = true; job.reason ??= 'Live Fish completion exceeded its state limit'; }
  const output = []; let outputSize = 0;
  let child;
  // Older Fish rejects Node's socket-backed stdin; a fixed cat pipeline gives it
  // an ordinary OS pipe. Definitions never touch the disk and no editor word is
  // sourced: the hook escaped each of them before writing the program.
  try { child = spawn('/bin/sh', ['-c', '/bin/cat | "$1" --no-config --private', 'shucked-live-fish', fishPath], { detached: true, stdio: ['pipe', 'pipe', 'ignore'] }); }
  catch { finish(job, true, 'Live Fish worker unavailable'); return; }
  job.child = child;
  child.on('error', () => finish(job, true, 'Live Fish worker unavailable'));
  child.stdin.on('error', () => undefined);
  child.stdin.end(program);
  child.stdout.on('data', chunk => {
    outputSize += chunk.length;
    if (outputSize > LIMITS.payload) { job.partial = true; job.reason ??= 'Live completion output limit reached'; child.stdout.destroy(); return; }
    output.push(chunk);
  });
  child.on('exit', code => {
    for (const line of Buffer.concat(output).toString('utf8').split('\n').filter(Boolean)) {
      const tab = line.indexOf('\t');
      addCandidate(job, tab < 0 ? line : line.slice(0, tab), tab < 0 ? '' : line.slice(tab + 1), false);
    }
    finish(job, job.partial || code !== 0, job.reason ?? (code === 0 ? undefined : 'Live Fish state could not be reconstructed'));
  });
}

function kill(targets) {
  for (const pid of targets) {
    if (!Number.isSafeInteger(pid) || pid <= 1 || pid === shellPid || pid === process.pid) { continue; }
    try { process.kill(-pid, 'SIGKILL'); } catch { /* Not a process group leader, or already gone. */ }
    try { process.kill(pid, 'SIGKILL'); } catch { /* Already gone. */ }
  }
}

function finish(job, partial, reason, silent = false) {
  if (job.finished) { return; }
  job.finished = true;
  clearTimeout(job.timer);
  jobs.delete(job.query);
  if (active === job) { mode = 'idle'; active = null; }
  kill(job.pids);
  if (job.child?.pid) { kill([job.child.pid]); }
  // A request the shell never got to serve must not start a worker later.
  try {
    const current = fs.readFileSync(requestPath, 'utf8').split('\0')[0];
    if (current === job.query) { fs.unlinkSync(requestPath); }
  } catch { /* Already consumed or removed. */ }
  if (!silent) {
    send({ kind: 'liveCompletion', phase: 'result', token, id, query: job.query, generation: job.generation, pid: 0, candidates: job.candidates, partial: partial || job.partial, reason: reason ?? job.reason, elapsedMs: Math.round(performance.now() - job.started) });
  }
}

// --- Requests from the extension -------------------------------------------
function request(message) {
  const { query, generation, prefix, words } = message;
  if (typeof query !== 'string' || !/^[a-f0-9]{32}$/.test(query) || jobs.has(query)
    || !Number.isSafeInteger(generation) || generation <= 0 || typeof prefix !== 'string'
    || !Array.isArray(words) || words.length === 0 || words.length > LIMITS.words
    || ![...words, prefix].every(word => typeof word === 'string' && !word.includes('\0') && Buffer.byteLength(word) <= LIMITS.word)) { return; }
  const body = [query, String(generation), prefix, String(words.length), ...words].join('\0') + '\0';
  if (Buffer.byteLength(body) > LIMITS.request + 1024) { return; }
  // One request at a time per shell: a newer one supersedes what is running.
  for (const job of [...jobs.values()]) { finish(job, true, 'Superseded completion', true); }
  const job = { query, generation, started: performance.now(), pids: [], candidates: [], partial: false, reason: undefined, size: 0, program: [], programSize: 0, headerSeen: false, cancelled: false, finished: false, child: undefined, timer: undefined };
  jobs.set(query, job);
  job.timer = setTimeout(() => finish(job, true, 'Live completion timed out'), DEADLINE);
  try {
    const temporary = path.join(directory, `.request-${process.pid}`);
    fs.writeFileSync(temporary, body, { mode: 0o600 });
    fs.renameSync(temporary, requestPath);
  } catch { finish(job, true, 'Could not prepare live completion'); return; }
  if (process.ppid !== shellPid) { exit(); return; }
  try { process.kill(shellPid, signal); } catch { finish(job, true, 'The shell query hook is unavailable'); }
}
function cancel(query) {
  const job = jobs.get(query);
  if (!job) { return; }
  job.cancelled = true;
  finish(job, true, 'Completion cancelled', true);
}
function handle(message) {
  if (!message || typeof message !== 'object') { return; }
  if (message.kind === 'request') { request(message); }
  else if (message.kind === 'cancel' && typeof message.query === 'string') { cancel(message.query); }
  else if (message.kind === 'stop') { exit(); }
}

// --- Session socket --------------------------------------------------------
function send(message) {
  if (!connection || connection.destroyed) { return false; }
  try { connection.write(JSON.stringify(message) + '\n'); return true; } catch { return false; }
}
function connect() {
  if (stopping) { return; }
  const socket = net.createConnection(socketPath);
  socket.on('connect', () => {
    connection = socket; failures = 0; reconnectDelay = 500;
    send({ kind: 'liveHelper', phase: 'hello', token, id, shell, pid: process.pid, shellPid, signal });
  });
  socket.on('data', chunk => {
    inbound = inbound.length ? Buffer.concat([inbound, chunk]) : chunk;
    if (inbound.length > LIMITS.frame) { socket.destroy(); return; }
    let at;
    while ((at = inbound.indexOf(10)) >= 0) {
      const line = inbound.subarray(0, at);
      inbound = inbound.subarray(at + 1);
      try { handle(JSON.parse(line.toString('utf8'))); } catch { /* A malformed frame is ignored. */ }
    }
  });
  socket.on('error', () => undefined);
  socket.on('close', () => {
    if (connection === socket) { connection = null; }
    inbound = Buffer.alloc(0);
    if (stopping) { return; }
    failures += 1;
    // The extension removes its socket when it goes away; a socket that stays
    // unreachable for a minute means the same.
    if ((failures >= 3 && !fs.existsSync(socketPath)) || failures >= 12) { exit(); return; }
    setTimeout(connect, reconnectDelay);
    reconnectDelay = Math.min(reconnectDelay * 2, 8000);
  });
}
connect();

// --- Lifetime --------------------------------------------------------------
setInterval(() => {
  if (process.ppid !== shellPid) { exit(); return; }
  try { process.kill(shellPid, 0); } catch { exit(); }
}, 1000);
try {
  if (fs.fstatSync(0).isFIFO()) {
    process.stdin.on('end', () => exit());
    process.stdin.on('error', () => exit());
    process.stdin.resume();
  }
} catch { /* No usable stdin; the parent check covers the shell's exit. */ }
for (const name of ['SIGHUP', 'SIGTERM', 'SIGINT']) { process.on(name, () => exit()); }

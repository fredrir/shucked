/* eslint @typescript-eslint/no-require-imports: "off" -- Standalone CommonJS helper for the extension host runtime. */
'use strict';
const net = require('node:net');
const { createHash } = require('node:crypto');
// The extension accepts one frame of this size (MAX_FRAME in terminal.ts) and
// the server truncates inventories beyond MAX_NAMES; anything larger is dropped
// with a notice instead of silently.
const MAX = 1024 * 1024;
// Electron-as-node startup is slow on some hosts; the read deadline covers the
// shell's report while the socket write has its own bound.
const READ_DEADLINE = 3000;
const WRITE_DEADLINE = 1000;
let bytes = 0;
let overflow = false;
let finished = false;
const chunks = [];
const identity = () => ({ token: process.env.SHUCKED_SESSION_TOKEN, id: process.env.SHUCKED_SESSION_ID, generation: Number(process.argv[2]), pid: Number(process.argv[3]), shell: process.argv[4] });
const timeout = setTimeout(() => finish({ kind: 'hookDropped', ...identity(), reason: 'deadline' }), READ_DEADLINE);
process.stdin.on('data', chunk => {
  if (overflow) {return;}
  bytes += chunk.length;
  if (bytes > MAX) { overflow = true; chunks.length = 0; return; }
  chunks.push(chunk);
});
process.stdin.on('end', () => {
  if (overflow) { finish({ kind: 'hookDropped', ...identity(), reason: 'size' }); return; }
  const fields = Buffer.concat(chunks).toString('utf8').split('\0');
  const message = { ...identity(), cwd: '', path: [], aliases: Object.create(null), functions: [], options: {}, private: false, ignore: [], connected: true, liveCompletion: false };
  for (let i = 0; i + 1 < fields.length; i += 2) {
    const key = fields[i], value = fields[i + 1];
    if (key === 'cwd') {message.cwd = value;}
    else if (key === 'live-signal' && ['SIGUSR1', 'SIGUSR2', 'SIGWINCH'].includes(value)) {message.liveSignal = value; message.liveCompletion = true;}
    else if (key === 'history-file') {message.historyFile = value;}
    else if (key === 'searchpath') {message.path = value.split(':');}
    else if (key === 'path') {message.path.push(value);}
    else if (key === 'function' && /^[\w.:-]{1,256}$/.test(value) && !value.startsWith('__shucked')) {message.functions.push(value);}
    else if (key === 'alias') {
      const line = value.replace(/^alias /, ''); const at = line.indexOf('='); if (at < 1) {continue;}
      const name = line.slice(0, at); let expansion = line.slice(at + 1);
      // Shell alias output wraps the full value in single quotes. Decode that representation first.
      if (expansion.startsWith("'") && expansion.endsWith("'")) {expansion = expansion.slice(1, -1).replace(/'\\''/g, "'");}
      const words = simpleAlias(expansion); if (/^[\w.:-]{1,256}$/.test(name) && words) {message.aliases[name] = words;} else if (/^[\w.:-]{1,256}$/.test(name)) {message.functions.push(name);}
    } else if (key === 'private') {message.private ||= value === '1';}
    else if (key === 'accepted-history') {message.acceptedHistoryHash = createHash('sha256').update(value.trim()).digest('hex');}
    else if (key === 'ignore') {message.ignore.push(value);}
    else if (key === 'option') { const at = value.indexOf('='); if (at > 0) {message.options[value.slice(0, at)] = value.slice(at + 1);} }
  }
  finish(message);
});
function finish(message) {
  if (finished) {return;}
  finished = true;
  clearTimeout(timeout);
  const socket = net.createConnection(process.env.SHUCKED_SESSION_SOCKET);
  socket.setTimeout(WRITE_DEADLINE, () => { socket.destroy(); process.exit(0); });
  socket.on('error', () => process.exit(0));
  socket.on('connect', () => socket.end(JSON.stringify(message) + '\n'));
  socket.on('close', () => process.exit(0));
}
function simpleAlias(text) {
  const words = []; let word = '', quote = '', escaped = false;
  for (const ch of text) {
    if (escaped) { word += ch; escaped = false; continue; }
    if (ch === '\\' && quote !== "'") { escaped = true; continue; }
    if (ch === "'" || ch === '"') { if (quote === ch) {quote = '';} else if (!quote) {quote = ch;} else {word += ch;} continue; }
    if (/[`$;|&<>()\n]/.test(ch)) {return undefined;}
    if (!quote && /\s/.test(ch)) { if (word) {words.push(word);} word = ''; } else {word += ch;}
  }
  if (escaped || quote || /\s$/.test(text)) {return undefined;}
  if (word) {words.push(word);}
  // Export only command mappings and simple switches, omitting value-bearing arguments.
  return words.length && words.length <= 32 && /^[\w./+-]+$/.test(words[0]) && words.slice(1).every(word => /^--?[\w-]+$/.test(word)) ? words : undefined;
}

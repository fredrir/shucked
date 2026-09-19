'use strict';
const net = require('node:net');
const { createHash } = require('node:crypto');
const MAX = 256 * 1024;
let bytes = 0;
const chunks = [];
const timeout = setTimeout(() => process.exit(0), 1200);
process.stdin.on('data', chunk => { bytes += chunk.length; if (bytes > MAX) process.exit(0); chunks.push(chunk); });
process.stdin.on('end', () => {
  const fields = Buffer.concat(chunks).toString('utf8').split('\0');
  const message = { token: process.env.SHUCKED_SESSION_TOKEN, id: process.env.SHUCKED_SESSION_ID, generation: Number(process.argv[2]), pid: Number(process.argv[3]), shell: process.argv[4], cwd: '', path: [], aliases: Object.create(null), functions: [], options: {}, private: false, ignore: [], connected: true };
  for (let i = 0; i + 1 < fields.length; i += 2) {
    const key = fields[i], value = fields[i + 1];
    if (key === 'cwd') message.cwd = value;
    else if (key === 'searchpath') message.path = value.split(':');
    else if (key === 'path') message.path.push(value);
    else if (key === 'function' && /^[\w.:-]{1,256}$/.test(value) && !value.startsWith('__shucked')) message.functions.push(value);
    else if (key === 'alias') {
      const line = value.replace(/^alias /, ''); const at = line.indexOf('='); if (at < 1) continue;
      const name = line.slice(0, at); let expansion = line.slice(at + 1);
      // Shell alias output wraps the full value in single quotes. Decode that representation first.
      if (expansion.startsWith("'") && expansion.endsWith("'")) expansion = expansion.slice(1, -1).replace(/'\\''/g, "'");
      const words = simpleAlias(expansion); if (/^[\w.:-]{1,256}$/.test(name) && words) message.aliases[name] = words; else if (/^[\w.:-]{1,256}$/.test(name)) message.functions.push(name);
    } else if (key === 'private') message.private ||= value === '1';
    else if (key === 'accepted-history') message.acceptedHistoryHash = createHash('sha256').update(value.trim()).digest('hex');
    else if (key === 'ignore') message.ignore.push(value);
    else if (key === 'option') { const at = value.indexOf('='); if (at > 0) message.options[value.slice(0, at)] = value.slice(at + 1); }
  }
  const socket = net.createConnection(process.env.SHUCKED_SESSION_SOCKET);
  socket.on('error', () => process.exit(0));
  socket.on('connect', () => socket.end(JSON.stringify(message) + '\n'));
  socket.on('close', () => { clearTimeout(timeout); process.exit(0); });
});
function simpleAlias(text) {
  const words = []; let word = '', quote = '', escaped = false;
  for (const ch of text) {
    if (escaped) { word += ch; escaped = false; continue; }
    if (ch === '\\' && quote !== "'") { escaped = true; continue; }
    if (ch === "'" || ch === '"') { if (quote === ch) quote = ''; else if (!quote) quote = ch; else word += ch; continue; }
    if (/[`$;|&<>()\n]/.test(ch)) return undefined;
    if (!quote && /\s/.test(ch)) { if (word) words.push(word); word = ''; } else word += ch;
  }
  if (escaped || quote || /\s$/.test(text)) return undefined;
  if (word) words.push(word);
  // Export only command mappings and simple switches, omitting value-bearing arguments.
  return words.length && words.length <= 32 && /^[\w./+-]+$/.test(words[0]) && words.slice(1).every(word => /^--?[\w-]+$/.test(word)) ? words : undefined;
}

/* eslint @typescript-eslint/no-require-imports: "off" -- Fixed standalone shell IPC helper. */
'use strict';
const net = require('node:net');
const [phase, query, generation, pid] = process.argv.slice(2);
const message = { kind: 'liveCompletion', token: process.env.SHUCKED_SESSION_TOKEN, id: process.env.SHUCKED_SESSION_ID, query, generation: Number(generation), phase, pid: Number(pid), candidates: [], partial: false };
const timer = setTimeout(() => process.exit(0), 1200);
function send() {
  const socket = net.createConnection(process.env.SHUCKED_SESSION_SOCKET);
  socket.on('error', () => process.exit(0));
  socket.on('connect', () => socket.end(JSON.stringify(message) + '\n'));
  socket.on('close', () => { clearTimeout(timer); process.exit(0); });
}
if (phase === 'started') { send(); }
else {
  const chunks = []; let size = 0;
  process.stdin.on('data', chunk => {
    size += chunk.length;
    if (size > 192 * 1024) { message.partial = true; process.stdin.destroy(); send(); return; }
    chunks.push(chunk);
  });
  process.stdin.on('end', () => {
    const fields = Buffer.concat(chunks).toString('utf8').split('\0');
    for (let index = 0; index < fields.length; index++) {
      if (fields[index] === 'M') {
        const text = fields[++index], description = fields[++index];
        if (typeof text === 'string' && typeof description === 'string' && text.length <= 8192 && description.length <= 16384 && message.candidates.length < 2000) { message.candidates.push({ text, description }); }
        else { message.partial = true; }
      } else if (fields[index] === 'P') { message.partial = true; message.reason = fields[++index]?.slice(0, 512); }
    }
    send();
  });
}

/* eslint @typescript-eslint/no-require-imports: "off" -- Private request bytes are data, never shell source. */
'use strict';
const fs = require('node:fs');
const path = require('node:path');
try {
  const directory = process.env.SHUCKED_LIVE_DIRECTORY;
  const name = fs.readdirSync(directory).find(name => /^request-[a-f0-9]{32}$/.test(name));
  if (name) {
    const filename = path.join(directory, name);
    const fd = fs.openSync(filename, fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW ?? 0));
    try {
      const stat = fs.fstatSync(fd);
      if (stat.isFile() && stat.size <= 32768) { process.stdout.write(fs.readFileSync(fd)); }
    } finally { fs.closeSync(fd); fs.unlinkSync(filename); }
  }
} catch { /* A cancelled or invalid request has no work to perform. */ }
